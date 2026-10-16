//! Worker-prepared manual pitch regions and independent before/after analysis.
pub use crate::dsp::pitch::{nearest_note, Controls, Meter, Pitch, LATENCY};
use crate::{
    automation::Lane,
    model::{AutomationPoint, Effect, Extra, MAX_FRAME},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::{AtomicBool, Ordering},
};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Point {
    #[serde(default)]
    pub id: String,
    pub frame: u64,
    pub note: f32,
    #[serde(default = "linear")]
    pub curve: String,
    #[serde(default)]
    pub curve_start: f32,
    #[serde(default = "one")]
    pub curve_end: f32,
}
fn linear() -> String {
    "linear".into()
}
fn one() -> f32 {
    1.
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Region {
    #[serde(default)]
    pub id: String,
    pub start_frame: u64,
    pub end_frame: u64,
    #[serde(default)]
    pub points: Vec<Point>,
}
#[derive(Clone)]
struct Span {
    start: u64,
    end: u64,
    region: usize,
    gate_start: u64,
    gate_end: u64,
}
#[derive(Clone, Default)]
pub struct Regions {
    spans: Vec<Span>,
    lanes: Vec<Lane>,
}
fn decode(extra: &Extra) -> Result<Vec<Region>> {
    match extra.get("pitchEdits") {
        None | Some(serde_json::Value::Null) => Ok(vec![]),
        Some(v) => Ok(serde_json::from_value(v.clone())?),
    }
}
pub fn validate(effect: &Effect) -> Result<()> {
    let regions = decode(&effect.extra)?;
    if effect
        .extra
        .get("pitchEditsOnly")
        .map(|v| !v.is_boolean())
        .unwrap_or(false)
    {
        return Err(Error::Invalid("Invalid pitch edit scope".into()));
    }
    validate_regions(&regions)
}
fn validate_regions(regions: &[Region]) -> Result<()> {
    if regions.len() > 65_536 || regions.iter().map(|r| r.points.len()).sum::<usize>() > 1_000_000 {
        return Err(Error::Invalid(
            "Too many pitch edit regions or points".into(),
        ));
    }
    for region in regions {
        if region.start_frame > MAX_FRAME || region.end_frame > MAX_FRAME {
            return Err(Error::Invalid("Pitch region exceeds frame bound".into()));
        }
        if !region.id.is_empty() && !crate::model::is_id(&region.id) {
            return Err(Error::Invalid("Invalid pitch region UUID".into()));
        }
        for point in &region.points {
            if point.frame > MAX_FRAME
                || !point.note.is_finite()
                || !point.curve_start.is_finite()
                || !point.curve_end.is_finite()
                || !["linear", "smooth", "easeIn", "easeOut", "hold"]
                    .contains(&point.curve.as_str())
                || !point.id.is_empty() && !crate::model::is_id(&point.id)
            {
                return Err(Error::Invalid("Invalid pitch edit point".into()));
            }
        }
    }
    Ok(())
}
impl Regions {
    /// Conservative retained playback allocation bound: at most two event spans
    /// per saved region; slicing can add at most four boundary-neighbour points
    /// per span. Authored IDs, strings and JSON extras are not retained.
    pub(crate) fn prepared_storage_bound(effect: &Effect) -> Result<usize> {
        let regions = decode(&effect.extra)?;
        validate_regions(&regions)?;
        let count = regions.len();
        let points = regions.iter().map(|r| r.points.len()).sum::<usize>();
        Ok(std::mem::size_of::<Self>()
            + 16
            + count * 2 * (std::mem::size_of::<Span>() * 2 + std::mem::size_of::<Lane>() + 16)
            + Lane::point_storage_bound(points + count * 8))
    }
    pub fn from_effect(effect: &Effect) -> Result<Self> {
        validate(effect)?;
        Self::new(&decode(&effect.extra)?)
    }
    pub fn new(regions: &[Region]) -> Result<Self> {
        validate_regions(regions)?;
        // Resolve overlaps off the processing path: later saved regions replace earlier
        // ones. Retain original lanes to preserve nonlinear interpolation when sliced.
        let mut lanes = vec![];
        let mut points = vec![];
        let mut bounds = vec![];
        let mut events: BTreeMap<u64, Vec<(usize, bool)>> = BTreeMap::new();
        for region in regions {
            if region.start_frame >= region.end_frame || region.points.is_empty() {
                continue;
            }
            let clean = region
                .points
                .iter()
                .map(|p| AutomationPoint {
                    frame: p.frame.clamp(region.start_frame, region.end_frame),
                    value: p.note.clamp(0., 95.) / 95.,
                    curve: p.curve.clone(),
                    curve_start: p.curve_start.clamp(0., 1.),
                    curve_end: p.curve_end.clamp(p.curve_start.clamp(0., 1.), 1.),
                    extra: Extra::new(),
                })
                .map(|p| (p.frame, p))
                .collect::<BTreeMap<_, _>>()
                .into_values()
                .collect::<Vec<_>>();
            let index = lanes.len();
            let first = clean.iter().min_by_key(|p| p.frame).unwrap().value;
            lanes.push(Lane::new(&clean, first, false));
            points.push(clean);
            bounds.push((region.start_frame, region.end_frame));
            events
                .entry(region.start_frame)
                .or_default()
                .push((index, true));
            events
                .entry(region.end_frame)
                .or_default()
                .push((index, false));
        }
        let mut active = BTreeSet::new();
        let mut spans: Vec<Span> = vec![];
        let mut previous = None;
        for (time, changes) in events {
            if let Some(start) = previous {
                if let Some(&region) = active.iter().next_back() {
                    if let Some(last) = spans
                        .last_mut()
                        .filter(|s| s.region == region && s.end == start)
                    {
                        last.end = time;
                        last.gate_end = time;
                    } else {
                        spans.push(Span {
                            start,
                            end: time,
                            region,
                            gate_start: start,
                            gate_end: time,
                        });
                    }
                }
            }
            for (index, start) in changes {
                if start {
                    active.insert(index);
                } else {
                    active.remove(&index);
                }
            }
            previous = Some(time);
        }
        // Match saved-region slicing, including its Float MIDI/normalized
        // roundtrip and fractional curve-domain arithmetic at retained boundaries.
        let mut sliced_lanes = Vec::with_capacity(spans.len());
        let mut sliced_points = Vec::with_capacity(spans.len());
        for span in &mut spans {
            let original = &points[span.region];
            let (original_start, original_end) = bounds[span.region];
            let sliced = span.start != original_start || span.end != original_end;
            let mut curve = if sliced {
                let lower = original
                    .partition_point(|p| p.frame < span.start)
                    .saturating_sub(1);
                let upper =
                    (original.partition_point(|p| p.frame <= span.end) + 1).min(original.len());
                original[lower..upper].to_vec()
            } else {
                original.clone()
            };
            if sliced {
                curve = insert_boundary(curve, span.start);
                curve = insert_boundary(curve, span.end);
                curve.retain(|p| p.frame >= span.start && p.frame <= span.end);
                for point in &mut curve {
                    point.value = (point.value * 95.).clamp(0., 95.) / 95.;
                }
            }
            let baseline = curve.first().unwrap().value;
            span.region = sliced_lanes.len();
            sliced_lanes.push(Lane::new(&curve, baseline, false));
            sliced_points.push(curve);
        }
        lanes = sliced_lanes;
        points = sliced_points;
        let mut first = 0;
        while first < spans.len() {
            let mut last = first;
            while last + 1 < spans.len() && spans[last].end == spans[last + 1].start {
                let before = &spans[last];
                let after = &spans[last + 1];
                let held = points[before.region]
                    .iter()
                    .filter(|p| p.frame < before.end)
                    .max_by_key(|p| p.frame)
                    .map(|p| p.curve == "hold")
                    .unwrap_or(false);
                let left =
                    lanes[before.region].value(if held { before.end - 1 } else { before.end })
                        * 95.;
                let right = lanes[after.region].value(after.start) * 95.;
                if (left - right).abs() >= 0.001 {
                    break;
                }
                last += 1;
            }
            let start = spans[first].start;
            let end = spans[last].end;
            for span in &mut spans[first..=last] {
                span.gate_start = start;
                span.gate_end = end;
            }
            first = last + 1;
        }
        Ok(Self { spans, lanes })
    }
    pub fn sample(&self, frame: i64) -> Option<(f32, f32)> {
        if frame < 0 {
            return None;
        }
        let frame = frame as u64;
        let index = self.spans.partition_point(|s| s.start <= frame);
        if index == 0 {
            return None;
        }
        let span = &self.spans[index - 1];
        if frame >= span.end {
            return None;
        }
        let distance = (frame - span.gate_start).min(span.gate_end - 1 - frame);
        let blend = if distance >= 240 {
            1.
        } else {
            0.5 - 0.5 * (std::f32::consts::PI * (distance as f32 / 240.)).cos()
        };
        Some((self.lanes[span.region].value(frame) * 95., blend))
    }
}
pub(crate) fn insert_boundary(mut points: Vec<AutomationPoint>, frame: u64) -> Vec<AutomationPoint> {
    points.sort_by_key(|p| p.frame);
    if points.iter().any(|p| p.frame == frame) {
        return points;
    }
    let value = Lane::new(&points, points.first().unwrap().value, false).value(frame);
    let lower = points.partition_point(|p| p.frame < frame);
    let mut point = AutomationPoint {
        frame,
        value,
        curve: "linear".into(),
        curve_start: 0.,
        curve_end: 1.,
        extra: Extra::new(),
    };
    if lower > 0 && lower < points.len() {
        let original = &points[lower - 1];
        let next = &points[lower];
        let fraction = (frame - original.frame) as f32 / (next.frame - original.frame) as f32;
        let middle = original.curve_start + (original.curve_end - original.curve_start) * fraction;
        point.curve = original.curve.clone();
        point.curve_start = middle;
        point.curve_end = original.curve_end;
        points[lower - 1].curve_end = middle;
    }
    points.insert(lower, point);
    points
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContourPoint {
    pub frame: u64,
    pub frequency: f32,
    pub confidence: f32,
}
impl ContourPoint {
    pub fn is_voiced(&self) -> bool {
        (50. ..=1000.).contains(&self.frequency) && self.confidence > 0.75
    }
}
/// Analyze measured audio, never desired targets. Cancellation is checked every
/// 8192 frames; chart complexity is bounded independently from detector cadence.
pub fn analyze(
    left: &[f32],
    right: &[f32],
    maximum_points: usize,
    cancelled: &AtomicBool,
) -> Result<Vec<ContourPoint>> {
    if left.len() != right.len() {
        return Err(Error::Invalid(
            "Pitch analysis channel lengths differ".into(),
        ));
    }
    if left.is_empty() {
        return Ok(vec![]);
    }
    let mut detector = Pitch::default();
    let controls = Controls {
        mix: 0.,
        strength: 0.,
        formant: 0.,
        ..Controls::default()
    };
    let limit = maximum_points.clamp(1, 65_536);
    let hop = ((left.len() as f64 / limit as f64 / 256.).ceil() as usize * 256).max(256);
    let mut points = Vec::with_capacity((limit + 1).min(left.len() / hop + 1));
    for index in 0..left.len() + 1024 {
        if index % 8192 == 0 && cancelled.load(Ordering::Relaxed) {
            return Err(Error::Invalid("Pitch analysis cancelled".into()));
        }
        detector.sample(
            [
                left.get(index).copied().unwrap_or(0.),
                right.get(index).copied().unwrap_or(0.),
            ],
            controls,
        );
        if index >= 1024 && index - 1024 < left.len() && (index + 4) % hop == 0 {
            points.push(ContourPoint {
                frame: (index - 1024) as u64,
                frequency: detector.detected_frequency,
                confidence: detector.confidence,
            });
        }
    }
    Ok(points)
}

/// Bounded worker analysis directly from a validated 48 kHz WAV; no source-sized
/// PCM allocation. The detector always receives every sample, including the final
/// 1024-frame analysis window flush.
pub fn analyze_wave(
    reader: &mut crate::wave::WaveReader,
    maximum_points: usize,
    cancelled: &AtomicBool,
) -> Result<Vec<ContourPoint>> {
    if reader.info.sample_rate != 48_000 {
        return Err(Error::Unsupported(
            "Pitch analysis requires a 48 kHz source".into(),
        ));
    }
    let frames = reader.info.frames;
    if frames == 0 {
        return Ok(vec![]);
    }
    let limit = maximum_points.clamp(1, 65_536);
    let hop = ((frames as f64 / limit as f64 / 256.).ceil() as u64 * 256).max(256);
    let mut points = Vec::with_capacity((limit + 1).min((frames / hop + 1) as usize));
    let mut detector = Pitch::default();
    let controls = Controls {
        mix: 0.,
        strength: 0.,
        formant: 0.,
        ..Controls::default()
    };
    let mut left = [0.; 8192];
    let mut right = [0.; 8192];
    let mut offset = 0;
    while offset < frames + 1024 {
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::Invalid("Pitch analysis cancelled".into()));
        }
        let count = (frames + 1024 - offset).min(8192) as usize;
        let available = frames.saturating_sub(offset).min(count as u64) as usize;
        if available > 0 {
            reader.read_stereo(offset, &mut left[..available], &mut right[..available])?;
        }
        left[available..count].fill(0.);
        right[available..count].fill(0.);
        for index in 0..count {
            detector.sample([left[index], right[index]], controls);
            let frame = offset + index as u64;
            if frame >= 1024 && frame - 1024 < frames && (frame + 4) % hop == 0 {
                points.push(ContourPoint {
                    frame: frame - 1024,
                    frequency: detector.detected_frequency,
                    confidence: detector.confidence,
                });
            }
        }
        offset += count as u64;
    }
    Ok(points)
}
