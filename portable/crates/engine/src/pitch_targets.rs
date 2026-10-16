//! Pitch-editor target seeding and region slicing (ports of Swift
//! `PitchPointTargets` and `PitchEditRegion.sliced`). Point and region IDs are
//! left empty; the caller assigns UUIDs.
use crate::{
    model::{AutomationPoint, Extra},
    pitch::{insert_boundary, ContourPoint, Point, Region},
};
use std::collections::{BTreeMap, BTreeSet};

const RATE: f32 = 48000.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Natural,
    Tight,
    Robotic,
    Performance,
}
impl Preset {
    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "Natural" => Self::Natural,
            "Tight" => Self::Tight,
            "Robotic" => Self::Robotic,
            "Keep performance" => Self::Performance,
            _ => return None,
        })
    }
    fn strength(self) -> f32 {
        match self {
            Self::Natural => 0.5,
            Self::Tight | Self::Robotic => 1.,
            Self::Performance => 0.,
        }
    }
    fn curve(self) -> &'static str {
        match self {
            Self::Natural => "smooth",
            Self::Robotic => "hold",
            _ => "linear",
        }
    }
}

pub fn allowed_pitch_classes(parameters: &BTreeMap<String, f32>) -> u32 {
    let scale_value = parameters.get("scale").copied().unwrap_or(0.);
    let key_value = parameters.get("key").copied().unwrap_or(0.);
    let scale = if scale_value.is_finite() {
        scale_value.clamp(0., 3.) as u32
    } else {
        0
    };
    if scale == 0 {
        return 4095;
    }
    if scale == 3 {
        let mask = parameters.get("mask").copied().unwrap_or(4095.);
        return if mask.is_finite() {
            mask.clamp(0., 4095.) as u32
        } else {
            4095
        };
    }
    let key = if key_value.is_finite() {
        key_value.clamp(0., 11.) as u32
    } else {
        0
    };
    let base: u32 = if scale == 1 {
        0b101010110101
    } else {
        0b010110101101
    };
    ((base << key) | (base >> (12 - key))) & 4095
}

pub fn nearest_allowed(note: f32, mask: u32) -> f32 {
    if !note.is_finite() || mask & 4095 == 0 {
        return note;
    }
    let (mut nearest, mut distance) = (note, f32::INFINITY);
    for target in 0..=95u32 {
        if mask & (1 << (target % 12)) == 0 {
            continue;
        }
        let delta = (target as f32 - note).abs();
        if delta < distance {
            nearest = target as f32;
            distance = delta;
        }
    }
    nearest
}

fn point(frame: u64, note: f32) -> Point {
    Point {
        id: String::new(),
        frame,
        note,
        curve: "linear".into(),
        curve_start: 0.,
        curve_end: 1.,
    }
}

/// Ramer-Douglas-Peucker over linear values (Swift `AutomationSimplifier`).
fn simplify_automation(points: &[AutomationPoint], tolerance: f32) -> Vec<AutomationPoint> {
    let mut unique = BTreeMap::new();
    for p in points {
        unique.insert(p.frame, p.clone());
    }
    let sorted: Vec<AutomationPoint> = unique.into_values().collect();
    if sorted.len() <= 2 || !tolerance.is_finite() || tolerance <= 0. {
        return sorted;
    }
    let mut keep = BTreeSet::from([0, sorted.len() - 1]);
    for (i, p) in sorted.iter().enumerate().take(sorted.len() - 1) {
        if p.curve != "linear" {
            keep.insert(i);
            keep.insert(i + 1);
        }
    }
    let required: Vec<usize> = keep.iter().copied().collect();
    let mut stack: Vec<(usize, usize)> = required.windows(2).map(|w| (w[0], w[1])).collect();
    while let Some((first, last)) = stack.pop() {
        if last - first <= 1 {
            continue;
        }
        let (a, b) = (sorted[first].value, sorted[last].value);
        let (mut maximum, mut chosen) = (0f32, first);
        for (i, p) in sorted.iter().enumerate().take(last).skip(first + 1) {
            let fraction = (p.frame - sorted[first].frame) as f32
                / (sorted[last].frame - sorted[first].frame) as f32;
            let error = (p.value - (a + (b - a) * fraction)).abs();
            if error > maximum {
                maximum = error;
                chosen = i;
            }
        }
        if maximum > tolerance {
            keep.insert(chosen);
            stack.push((first, chosen));
            stack.push((chosen, last));
        }
    }
    keep.into_iter().map(|i| sorted[i].clone()).collect()
}

fn simplify(points: Vec<Point>, tolerance_cents: f32) -> Vec<Point> {
    if points.len() <= 2 || !tolerance_cents.is_finite() || tolerance_cents <= 0. {
        return points;
    }
    let normalized: Vec<AutomationPoint> = points
        .iter()
        .map(|p| AutomationPoint {
            frame: p.frame,
            value: p.note / 95.,
            curve: p.curve.clone(),
            curve_start: p.curve_start,
            curve_end: p.curve_end,
            extra: Extra::new(),
        })
        .collect();
    let retained: BTreeSet<u64> = simplify_automation(&normalized, tolerance_cents / 9500.)
        .into_iter()
        .map(|p| p.frame)
        .collect();
    points
        .into_iter()
        .filter(|p| retained.contains(&p.frame))
        .collect()
}

fn stabilized_notes(points: &[Point], mask: u32) -> Vec<f32> {
    let Some(first) = points.first() else {
        return vec![];
    };
    if mask & 4095 == 0 {
        return points.iter().map(|p| p.note).collect();
    }
    let initial_window = (RATE * 0.2) as u64;
    let confirmation = (RATE * 0.11) as u64;
    let mut initial: Vec<f32> = points
        .iter()
        .take_while(|p| p.frame - first.frame < initial_window)
        .map(|p| p.note)
        .collect();
    initial.sort_by(f32::total_cmp);
    let mut established = nearest_allowed(initial[initial.len() / 2], mask);
    let mut result = vec![established; points.len()];
    let (mut pending, mut pending_index) = (None::<f32>, 0usize);
    for (index, p) in points.iter().enumerate() {
        result[index] = established;
        let desired = nearest_allowed(p.note, mask);
        let preference = (p.note - established).abs() - (p.note - desired).abs();
        if desired == established || preference < 0.18 {
            pending = None;
            continue;
        }
        if pending != Some(desired) {
            pending = Some(desired);
            pending_index = index;
        }
        if p.frame - points[pending_index].frame >= confirmation {
            established = desired;
            for accepted in result.iter_mut().take(index + 1).skip(pending_index) {
                *accepted = desired;
            }
            pending = None;
        }
    }
    result
}

/// Seeds only measured voiced runs inside `scope`; unvoiced points split regions.
pub fn seed(
    contour: &[ContourPoint],
    scope: (u64, u64),
    reference: f32,
    preset: Preset,
    tolerance_cents: f32,
    mask: u32,
) -> Vec<Region> {
    let (lower, upper) = scope;
    if lower >= upper {
        return vec![];
    }
    let reference = if reference.is_finite() && reference > 0. {
        reference
    } else {
        440.
    };
    let mut sorted: Vec<ContourPoint> = contour
        .iter()
        .filter(|p| (lower..upper).contains(&p.frame))
        .copied()
        .collect();
    sorted.sort_by_key(|p| p.frame);
    let mut deltas: Vec<u64> = sorted
        .windows(2)
        .map(|w| w[1].frame - w[0].frame)
        .filter(|d| *d > 0)
        .collect();
    deltas.sort();
    let hop = deltas.get(deltas.len() / 2).copied().unwrap_or(256);
    let mut runs: Vec<Vec<ContourPoint>> = vec![];
    let mut run: Vec<ContourPoint> = vec![];
    for p in sorted {
        if (!p.is_voiced() || run.last().is_some_and(|l| p.frame - l.frame > hop * 2))
            && !run.is_empty()
        {
            runs.push(std::mem::take(&mut run));
        }
        if p.is_voiced() {
            run.push(p);
        }
    }
    if !run.is_empty() {
        runs.push(run);
    }
    runs.into_iter()
        .filter_map(|run| {
            let (first, last) = (run.first()?, run.last()?);
            let start = lower.max(first.frame.saturating_sub(hop / 2));
            let end = upper.min(last.frame + (hop / 2).max(1));
            if start >= end {
                return None;
            }
            let measured: Vec<Point> = run
                .iter()
                .filter_map(|p| {
                    let note = 69. + 12. * (p.frequency / reference).log2();
                    note.is_finite().then(|| point(p.frame, note))
                })
                .collect();
            let stable = if preset == Preset::Performance {
                measured.iter().map(|p| p.note).collect()
            } else {
                stabilized_notes(&measured, mask)
            };
            let prepared: Vec<Point> = measured
                .iter()
                .zip(stable)
                .map(|(p, s)| {
                    let mut r = p.clone();
                    r.note += (s - p.note) * preset.strength();
                    r
                })
                .collect();
            let unconstrained = mask & 4095 == 0;
            let targets: Vec<Point> = match preset {
                Preset::Robotic => {
                    let snapped: Vec<Point> = prepared
                        .into_iter()
                        .map(|mut p| {
                            p.curve = if unconstrained { "linear" } else { "hold" }.into();
                            p
                        })
                        .collect();
                    snapped
                        .iter()
                        .enumerate()
                        .filter(|(i, p)| {
                            *i == 0 || *i == snapped.len() - 1 || snapped[i - 1].note != p.note
                        })
                        .map(|(_, p)| p.clone())
                        .collect()
                }
                Preset::Tight => simplify(prepared, tolerance_cents),
                _ => simplify(prepared, tolerance_cents)
                    .into_iter()
                    .map(|mut p| {
                        p.curve = if unconstrained {
                            "linear"
                        } else {
                            preset.curve()
                        }
                        .into();
                        p
                    })
                    .collect(),
            };
            (!targets.is_empty()).then(|| Region {
                id: String::new(),
                start_frame: start,
                end_frame: end,
                points: targets,
            })
        })
        .collect()
}

/// Cuts both ends without changing the retained curve (Swift `sliced(to:)`).
/// New boundary points get an empty ID.
pub fn slice(region: &Region, start: u64, end: u64) -> Option<Region> {
    let (start, end) = (region.start_frame.max(start), region.end_frame.min(end));
    let first = region.points.first()?;
    if start >= end {
        return None;
    }
    if start == region.start_frame && end == region.end_frame {
        return Some(region.clone());
    }
    let mut curve: Vec<AutomationPoint> = region
        .points
        .iter()
        .map(|p| AutomationPoint {
            frame: p.frame,
            value: p.note / 95.,
            curve: p.curve.clone(),
            curve_start: p.curve_start,
            curve_end: p.curve_end,
            extra: Extra::new(),
        })
        .collect();
    let _ = first;
    curve = insert_boundary(curve, start);
    curve = insert_boundary(curve, end);
    let points = curve
        .into_iter()
        .filter(|p| p.frame >= start && p.frame <= end)
        .map(|p| Point {
            id: region
                .points
                .iter()
                .find(|o| o.frame == p.frame)
                .map(|o| o.id.clone())
                .unwrap_or_default(),
            frame: p.frame,
            note: p.value * 95.,
            curve: p.curve,
            curve_start: p.curve_start,
            curve_end: p.curve_end,
        })
        .collect();
    Some(Region {
        id: region.id.clone(),
        start_frame: start,
        end_frame: end,
        points,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn contour(note: f32, frames: std::ops::Range<u64>) -> Vec<ContourPoint> {
        frames
            .step_by(256)
            .map(|frame| ContourPoint {
                frame,
                frequency: 440. * 2f32.powf((note - 69.) / 12.),
                confidence: 0.95,
            })
            .collect()
    }
    #[test]
    fn tight_seed_snaps_a_flat_note_to_the_semitone() {
        let regions = seed(
            &contour(69.3, 0..48000),
            (0, 48000),
            440.,
            Preset::Tight,
            5.,
            4095,
        );
        assert_eq!(regions.len(), 1);
        assert!(regions[0]
            .points
            .iter()
            .all(|p| (p.note - 69.).abs() < 1e-4));
        assert!(regions[0].points.len() <= 3, "flat curve simplifies");
    }
    #[test]
    fn performance_seed_keeps_measured_notes_and_unvoiced_gaps_split_regions() {
        let mut points = contour(60.2, 0..12000);
        points.push(ContourPoint {
            frame: 12032,
            frequency: 0.,
            confidence: 0.,
        });
        points.extend(contour(64., 24000..36000));
        let regions = seed(&points, (0, 48000), 440., Preset::Performance, 5., 4095);
        assert_eq!(regions.len(), 2);
        assert!((regions[0].points[0].note - 60.2).abs() < 1e-3);
    }
    #[test]
    fn slicing_interpolates_the_boundaries() {
        let region = Region {
            id: "R".into(),
            start_frame: 0,
            end_frame: 1000,
            points: vec![point(0, 60.), point(1000, 70.)],
        };
        let cut = slice(&region, 250, 750).unwrap();
        assert_eq!((cut.start_frame, cut.end_frame), (250, 750));
        assert!((cut.points[0].note - 62.5).abs() < 1e-3);
        assert!((cut.points.last().unwrap().note - 67.5).abs() < 1e-3);
    }
    #[test]
    fn minor_and_major_masks_match_swift() {
        let major = BTreeMap::from([("scale".to_string(), 1.), ("key".to_string(), 0.)]);
        assert_eq!(allowed_pitch_classes(&major), 0b101010110101);
        assert_eq!(nearest_allowed(61.2, 0b101010110101), 62.);
        assert_eq!(nearest_allowed(60.9, 0b101010110101), 60.);
    }
}
