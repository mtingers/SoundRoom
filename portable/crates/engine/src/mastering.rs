//! Full-mix mastering analysis and the conservative, editable Master proposal.
//! A port of `SoundroomCore/MasteringAnalysis.swift`: the same measurements,
//! gating, proposal math and the source fingerprint that native clients verify.
use crate::{
    dsp::{limiter::TruePeak, loudness::Loudness, resonance::Resonance},
    model::{AutomationPoint, Effect, Session, Track},
    project::Project,
    render::{descriptors, Renderer},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Number, Value};
use std::sync::atomic::{AtomicBool, Ordering};

/// Session JSON key, matching the Swift `Session.masteringAnalysis` Codable field.
pub const SESSION_KEY: &str = "masteringAnalysis";
const SECTION_FRAMES: u64 = 144_000;
const BLOCK: usize = 8192;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MasteringSection {
    pub start: u64,
    pub end: u64,
    pub loudness: f32,
    pub true_peak: f32,
    pub resonance: f32,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MasteringAnalysis {
    #[serde(rename = "integratedLUFS")]
    pub integrated_lufs: f32,
    #[serde(rename = "truePeakDB")]
    pub true_peak_db: f32,
    #[serde(rename = "crestDB")]
    pub crest_db: f32,
    pub bass_fraction: f32,
    pub air_fraction: f32,
    pub sections: Vec<MasteringSection>,
    pub analyzed_frames: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_fingerprint: Option<String>,
}

impl MasteringAnalysis {
    pub fn stored(session: &Session) -> Option<Self> {
        serde_json::from_value(session.extra.get(SESSION_KEY)?.clone()).ok()
    }
    pub fn matches(&self, session: &Session) -> bool {
        self.source_fingerprint.as_deref() == Some(fingerprint(session).as_str())
    }
    pub fn is_silent(&self) -> bool {
        self.true_peak_db < -70.
    }
    /// Conservative proposal, identical to the Swift reference: bounded shelf
    /// cuts, section-smoothed resonance depth and limiter input automation.
    pub fn proposal(
        &self,
        target_lufs: f32,
        intensity: f32,
        mut new_id: impl FnMut() -> Result<String>,
    ) -> Result<Vec<Effect>> {
        if self.is_silent() {
            return Ok(vec![]);
        }
        let strength = intensity.clamp(0., 1.);
        let mut eq = new_effect("equalizer", new_id()?, &[]);
        let low = if self.bass_fraction > 0.55 {
            -((self.bass_fraction - 0.55) * 6.).min(1.5) * strength
        } else {
            0.
        };
        let high = if self.air_fraction > 0.35 {
            -((self.air_fraction - 0.35) * 6.).min(1.5) * strength
        } else {
            0.
        };
        eq.parameters.insert("lowGain".into(), low);
        eq.parameters.insert("highGain".into(), high);
        eq.parameters.insert("lowFrequency".into(), 180.);
        eq.parameters.insert("highFrequency".into(), 6500.);
        eq.enabled = low.abs() + high.abs() > 0.1;

        let mut resonance = new_effect(
            "resonance",
            new_id()?,
            &[
                ("low", 300.),
                ("depth", 3.),
                ("sensitivity", 0.35),
                ("release", 250.),
            ],
        );
        let mean = self.sections.iter().fold(0_f32, |sum, s| sum + s.resonance)
            / self.sections.len().max(1) as f32;
        resonance
            .parameters
            .insert("depth".into(), (mean * 1.5).clamp(0., 4.) * strength);
        resonance.enabled = mean > 0.8 && strength > 0.;
        let depth = parameter("resonance", "depth");
        resonance.parameter_automation.insert(
            "depth".into(),
            self.smoothed_points(|part| {
                depth.normalized((part.resonance * 1.5).clamp(0., 4.) * strength)
            }),
        );

        let mut warmth = new_effect(
            "warmth",
            new_id()?,
            &[
                ("character", 1.),
                ("drive", 6.),
                ("tone", 14000.),
                ("match", 1.),
            ],
        );
        warmth.parameters.insert("drive".into(), 3.);
        warmth.parameters.insert("mix".into(), 0.2 * strength);
        warmth.enabled = false; // Creative color requires the musician's choice.

        let mut limiter = new_effect("limiter", new_id()?, &[]);
        let requested = (target_lufs - self.integrated_lufs).clamp(-6., 8.) * strength;
        limiter.parameters.insert("input".into(), requested);
        limiter.parameters.insert("ceiling".into(), -1.);
        let input = parameter("limiter", "input");
        limiter.parameter_automation.insert(
            "input".into(),
            self.smoothed_points(|part| {
                if part.loudness < -60. {
                    return input.normalized(requested);
                }
                // At most ±1.5 dB section balancing; peak reduction bounded near 3 dB.
                let correction = (self.integrated_lufs - part.loudness).clamp(-1.5, 1.5) * strength;
                let budget = -1. + 3. - part.true_peak;
                input.normalized((requested + correction).min(budget.max(-6.)))
            }),
        );
        select(&mut limiter, "input");
        select(&mut resonance, "depth");
        Ok(vec![eq, resonance, warmth, limiter])
    }
    fn smoothed_points(&self, value: impl Fn(&MasteringSection) -> f32) -> Vec<AutomationPoint> {
        let Some(first) = self.sections.first() else {
            return vec![];
        };
        let mut last = value(first);
        let mut points = vec![point(0, last, "smooth")];
        for part in &self.sections {
            let next = value(part);
            last = last * 0.5 + next * 0.5;
            points.push(point((part.start + part.end) / 2, last, "smooth"));
        }
        points.push(point(self.analyzed_frames, last, "linear"));
        points
    }
}

fn normalized_amount(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0., 1.)
    } else {
        0.5
    }
}
fn point(frame: u64, value: f32, curve: &str) -> AutomationPoint {
    AutomationPoint {
        frame,
        value: normalized_amount(value),
        curve: curve.into(),
        curve_start: 0.,
        curve_end: 1.,
        extra: Default::default(),
    }
}
fn parameter(kind: &str, key: &str) -> crate::render::Descriptor {
    descriptors(kind)
        .into_iter()
        .find(|d| d.key == key)
        .expect("built-in effect parameter")
}
fn default_amount(kind: &str) -> f32 {
    match kind {
        "lowPass" => 0.25,
        "echo" => 0.4,
        "warmth" => 0.3,
        _ => 0.5,
    }
}
fn select(effect: &mut Effect, key: &str) {
    effect
        .extra
        .insert("selectedAutomationParameter".into(), json!(key));
}
/// `RealtimeEffect(kind:parameters:)` with factory defaults merged with `overrides`.
fn new_effect(kind: &str, id: String, overrides: &[(&str, f32)]) -> Effect {
    let mut parameters = descriptors(kind)
        .into_iter()
        .map(|d| (d.key.to_string(), d.default))
        .collect::<std::collections::BTreeMap<_, _>>();
    if kind == "room" {
        parameters.insert("character".into(), 1.);
    }
    for (key, value) in overrides {
        parameters.insert((*key).into(), *value);
    }
    let mut effect = Effect {
        id,
        kind: kind.into(),
        amount: default_amount(kind),
        enabled: true,
        automation: vec![],
        parameters,
        parameter_automation: Default::default(),
        extra: Default::default(),
    };
    if let Some(first) = descriptors(kind).first() {
        select(&mut effect, first.key);
    }
    effect.extra.insert("pitchEdits".into(), json!([]));
    effect.extra.insert("pitchEditsOnly".into(), json!(false));
    effect.extra.insert("bakedRanges".into(), json!([]));
    effect
}

/// Transposed direct-form II filter configured exactly as Swift `Biquad`.
#[derive(Clone, Copy)]
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}
impl Biquad {
    fn pass(frequency: f32, q: f32, high: bool) -> Self {
        let w = 2. * std::f32::consts::PI * (48000_f32 * 0.45).min(frequency) / 48000.;
        let c = w.cos();
        let alpha = w.sin() / (2. * q);
        let d = 1. + alpha;
        let (b0, b1) = if high {
            ((1. + c) / 2., -(1. + c))
        } else {
            ((1. - c) / 2., 1. - c)
        };
        Self {
            b: [b0 / d, b1 / d, b0 / d],
            a: [-2. * c / d, (1. - alpha) / d],
            z: [0.; 2],
        }
    }
    fn filter(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        if !y.is_finite() || !self.z[0].is_finite() || !self.z[1].is_finite() {
            self.z = [0.; 2];
            return 0.;
        }
        y
    }
}

#[derive(Default)]
struct Section {
    start: u64,
    count: u64,
    peak: f32,
    power: f64,
    spectral: f64,
}
impl Section {
    fn finish(&mut self, frame: u64) -> MasteringSection {
        let count = self.count.max(1) as f64;
        let section = MasteringSection {
            start: self.start,
            end: frame,
            loudness: (-0.691 + 10. * (self.power / count).max(1e-12).log10()).max(-120.) as f32,
            true_peak: 20. * self.peak.max(1e-6).log10(),
            resonance: (self.spectral / count) as f32,
        };
        *self = Section {
            start: frame,
            ..Default::default()
        };
        section
    }
}

/// Measures the full mix (track inserts, gain and pan; the Master rack is
/// excluded). Cancellation and progress operate on worker blocks.
pub fn analyze(
    project: &Project,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<MasteringAnalysis> {
    let mut source = project.session.clone();
    source.master_track = Session::default().master_track;
    let end = source.end();
    let fingerprint = fingerprint(&project.session);
    if end == 0 {
        return Ok(MasteringAnalysis {
            integrated_lufs: -120.,
            true_peak_db: -120.,
            crest_db: 0.,
            bass_fraction: 0.,
            air_fraction: 0.,
            sections: vec![],
            analyzed_frames: 0,
            source_fingerprint: None,
        });
    }
    let source = Project {
        session: source,
        media: project.media.clone(),
    };
    let mut mixer = Renderer::new_cancellable(&source, cancel)?;
    let mut loudness = Loudness::default();
    let mut detector = TruePeak::default();
    let mut resonance = Resonance::default();
    let gentle = descriptors("resonance")
        .into_iter()
        .map(|d| match d.key {
            "low" => 300.,
            "depth" => 3.,
            "sensitivity" => 0.35,
            "release" => 250.,
            _ => d.default,
        })
        .collect::<Vec<_>>();
    resonance.configure(&gentle);
    let mut bass = [Biquad::pass(250., 0.707, false); 2];
    let mut air = [Biquad::pass(5000., 0.707, true); 2];
    let mut left = vec![0_f32; BLOCK];
    let mut right = vec![0_f32; BLOCK];
    let (mut frame, mut peak) = (0_u64, 0_f32);
    let (mut power, mut bass_power, mut air_power) = (0_f64, 0_f64, 0_f64);
    let mut section = Section::default();
    let mut blocks = vec![];
    let mut sections = vec![];
    while frame < end {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Conflict("Master analysis cancelled".into()));
        }
        let count = (end - frame).min(BLOCK as u64) as usize;
        mixer.render(&mut left[..count], &mut right[..count])?;
        for (&l, &r) in left[..count].iter().zip(&right[..count]) {
            loudness.sample([l, r]);
            let p = detector.sample([l, r]);
            resonance.sample([l, r]);
            peak = peak.max(p);
            section.peak = section.peak.max(p);
            let a = bass[0].filter(l) as f64;
            let b = bass[1].filter(r) as f64;
            let c = air[0].filter(l) as f64;
            let d = air[1].filter(r) as f64;
            power += l as f64 * l as f64 + r as f64 * r as f64;
            bass_power += a * a + b * b;
            air_power += c * c + d * d;
            section.power += loudness.last_power;
            section.spectral += resonance.reduction_db as f64;
            frame += 1;
            section.count += 1;
            if frame >= 19200 && frame % 4800 == 0 {
                blocks.push(10_f64.powf((loudness.momentary as f64 + 0.691) / 10.));
            }
            if section.count == SECTION_FRAMES {
                sections.push(section.finish(frame));
            }
        }
        progress(frame, end);
    }
    if section.count > 0 {
        sections.push(section.finish(frame));
    }
    // Flush the peak estimator's finite interpolation neighborhood.
    for _ in 0..12 {
        peak = peak.max(detector.sample([0.; 2]));
    }
    let gate = 10_f64.powf((-70. + 0.691) / 10.);
    let absolute = blocks.into_iter().filter(|b| *b > gate).collect::<Vec<_>>();
    let absolute_mean = absolute.iter().sum::<f64>() / absolute.len().max(1) as f64;
    let relative = absolute
        .into_iter()
        .filter(|b| *b >= absolute_mean * 0.1)
        .collect::<Vec<_>>();
    let integrated_power = if relative.is_empty() {
        sections.iter().fold(0_f64, |sum, s| {
            sum + 10_f64.powf((s.loudness as f64 + 0.691) / 10.) * (s.end - s.start) as f64
        }) / end as f64
    } else {
        relative.iter().sum::<f64>() / relative.len() as f64
    };
    let integrated = (-0.691 + 10. * integrated_power.max(1e-12).log10()).max(-120.) as f32;
    let peak_db = 20. * peak.max(1e-6).log10();
    let rms_db = (10. * (power / (end * 2) as f64).max(1e-12).log10()) as f32;
    Ok(MasteringAnalysis {
        integrated_lufs: integrated,
        true_peak_db: peak_db,
        crest_db: (peak_db - rms_db).max(0.),
        bass_fraction: (bass_power / power.max(1e-12)) as f32,
        air_fraction: (air_power / power.max(1e-12)) as f32,
        sections,
        analyzed_frames: end,
        source_fingerprint: Some(fingerprint),
    })
}

/// FNV-1a over the Swift `JSONEncoder` (`.sortedKeys`) encoding of `session.tracks`
/// as a native client re-encodes them, so `MasteringAnalysis.matches` agrees.
pub fn fingerprint(session: &Session) -> String {
    let tracks = Value::Array(session.tracks.iter().map(swift_track).collect());
    let mut text = String::new();
    swift_json(&tracks, &mut text);
    let hash = text.bytes().fold(14695981039346656037_u64, |hash, byte| {
        (hash ^ byte as u64).wrapping_mul(1099511628211)
    });
    format!("{hash:x}")
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}
/// Keeps only the Codable keys Swift encodes, so unknown extras never enter the hash.
fn retain(map: &mut Map<String, Value>, keys: &[&str]) {
    map.retain(|key, _| keys.contains(&key.as_str()));
}
fn uppercase(map: &mut Map<String, Value>, key: &str) {
    if let Some(Value::String(s)) = map.get_mut(key) {
        *s = s.to_ascii_uppercase();
    }
}
fn map_array(map: &mut Map<String, Value>, key: &str, f: impl Fn(Value) -> Value) {
    if let Some(Value::Array(items)) = map.get_mut(key) {
        for item in items.iter_mut() {
            *item = f(item.take());
        }
    }
}
fn swift_track(track: &Track) -> Value {
    let mut value = serde_json::to_value(track).unwrap_or_default();
    numeric_seeds(&mut value);
    let mut map = object(value);
    retain(
        &mut map,
        &[
            "id",
            "name",
            "color",
            "clips",
            "recordingTakes",
            "activeTakeID",
            "gain",
            "pan",
            "mixAutomation",
            "volumeAutomationScale",
            "muted",
            "solo",
            "armed",
            "inputDeviceUID",
            "inputDeviceName",
            "inputChannel",
            "stereoInput",
            "realtimeEffects",
        ],
    );
    map.retain(|_, v| !v.is_null());
    map.entry("inputChannel").or_insert(json!(0));
    map.entry("stereoInput").or_insert(json!(false));
    uppercase(&mut map, "id");
    uppercase(&mut map, "activeTakeID");
    map_array(&mut map, "clips", swift_clip);
    map_array(&mut map, "recordingTakes", |take| {
        let mut take = object(take);
        retain(&mut take, &["id", "name", "clips"]);
        uppercase(&mut take, "id");
        map_array(&mut take, "clips", swift_clip);
        Value::Object(take)
    });
    if let Some(Value::Object(lanes)) = map.get_mut("mixAutomation") {
        for lane in lanes.values_mut() {
            *lane = swift_points(lane.take());
        }
    }
    map_array(&mut map, "realtimeEffects", swift_effect);
    Value::Object(map)
}
const CLIP_STATE_KEYS: [&str; 18] = [
    "id",
    "operation",
    "createdAt",
    "renderEnvelope",
    "assetID",
    "name",
    "offset",
    "length",
    "soundDesign",
    "drumPattern",
    "fadeInFrames",
    "fadeOutFrames",
    "fadeReferenceOffset",
    "fadeReferenceLength",
    "edgeFadeFrames",
    "edgeProtectionDisabled",
    "cutFadeInFrames",
    "cutFadeOutFrames",
];
const ENVELOPE_KEYS: [&str; 11] = [
    "referenceLength",
    "referenceOffset",
    "attackFrames",
    "releaseFrames",
    "boundaryAttackFrames",
    "boundaryReleaseFrames",
    "crossfadeIn",
    "crossfadeOut",
    "cutReferenceOffset",
    "cutReferenceLength",
    "additionalBoundaries",
];
fn swift_envelope(map: &mut Map<String, Value>) {
    if let Some(Value::Object(envelope)) = map.get_mut("renderEnvelope") {
        envelope.retain(|key, v| !v.is_null() && ENVELOPE_KEYS.contains(&key.as_str()));
    }
}
/// Swift decodes wire seed strings as `UInt64` and re-encodes them as numbers.
fn numeric_seeds(value: &mut Value) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(numeric_seeds),
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                if key == "seed" || key == "organicSeed" {
                    if let Some(n) = item.as_str().and_then(|s| s.parse::<u64>().ok()) {
                        *item = json!(n);
                    }
                } else {
                    numeric_seeds(item);
                }
            }
        }
        _ => {}
    }
}
fn swift_clip(clip: Value) -> Value {
    let mut map = object(clip);
    retain(
        &mut map,
        &[
            "id",
            "assetID",
            "name",
            "start",
            "offset",
            "length",
            "soundDesign",
            "drumPattern",
            "previousStates",
            "renderEnvelope",
            "fadeInFrames",
            "fadeOutFrames",
            "fadeReferenceOffset",
            "fadeReferenceLength",
            "edgeFadeFrames",
            "edgeProtectionDisabled",
            "cutFadeInFrames",
            "cutFadeOutFrames",
        ],
    );
    map.retain(|_, v| !v.is_null());
    uppercase(&mut map, "id");
    uppercase(&mut map, "assetID");
    swift_envelope(&mut map);
    if let Some(Value::Array(states)) = map.get_mut("previousStates") {
        let excess = states.len().saturating_sub(5);
        states.drain(..excess);
        for state in states.iter_mut() {
            let mut entry = object(state.take());
            entry.retain(|key, v| !v.is_null() && CLIP_STATE_KEYS.contains(&key.as_str()));
            swift_envelope(&mut entry);
            uppercase(&mut entry, "id");
            uppercase(&mut entry, "assetID");
            *state = Value::Object(entry);
        }
    }
    Value::Object(map)
}
fn swift_points(points: Value) -> Value {
    let Value::Array(points) = points else {
        return points;
    };
    Value::Array(
        points
            .into_iter()
            .map(|point| {
                let map = object(point);
                let number = |key: &str, default: f32| {
                    map.get(key)
                        .and_then(Value::as_f64)
                        .map_or(default, |v| v as f32)
                };
                let start = normalized_amount(number("curveStart", 0.));
                let end = start.max(normalized_amount(number("curveEnd", 1.)));
                json!({
                    "frame": map.get("frame").cloned().unwrap_or(json!(0)),
                    "value": normalized_amount(number("value", 0.)),
                    "curve": map.get("curve").cloned().unwrap_or(json!("linear")),
                    "curveStart": start,
                    "curveEnd": end,
                })
            })
            .collect(),
    )
}
fn swift_effect(effect: Value) -> Value {
    let mut map = object(effect);
    retain(
        &mut map,
        &[
            "id",
            "kind",
            "amount",
            "enabled",
            "automation",
            "parameters",
            "parameterAutomation",
            "selectedAutomationParameter",
            "pitchEdits",
            "pitchEditsOnly",
            "bakedRanges",
        ],
    );
    map.retain(|_, v| !v.is_null());
    uppercase(&mut map, "id");
    let kind = map
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let amount = map
        .get("amount")
        .and_then(Value::as_f64)
        .map_or(default_amount(&kind), |v| v as f32);
    map.insert("amount".into(), json!(normalized_amount(amount)));
    map.entry("enabled").or_insert(json!(true));
    map.entry("parameters").or_insert(json!({}));
    map.entry("parameterAutomation").or_insert(json!({}));
    let automation = map.remove("automation").unwrap_or(json!([]));
    map.insert("automation".into(), swift_points(automation));
    if let Some(Value::Object(lanes)) = map.get_mut("parameterAutomation") {
        for lane in lanes.values_mut() {
            *lane = swift_points(lane.take());
        }
    }
    let has_parameters = map["parameters"].as_object().is_some_and(|p| !p.is_empty());
    let known = descriptors(&kind);
    let fallback = known
        .first()
        .filter(|_| has_parameters)
        .map(|d| json!(d.key));
    let selected = match map
        .get("selectedAutomationParameter")
        .and_then(Value::as_str)
    {
        Some(key) if known.iter().any(|d| d.key == key) => Some(json!(key)),
        _ => fallback,
    };
    match selected {
        Some(key) => map.insert("selectedAutomationParameter".into(), key),
        None => map.remove("selectedAutomationParameter"),
    };
    map.entry("pitchEdits").or_insert(json!([]));
    map.entry("pitchEditsOnly").or_insert(json!(false));
    map.entry("bakedRanges").or_insert(json!([]));
    Value::Object(map)
}

/// Swift `JSONEncoder` with `.sortedKeys`: ordinal key order, escaped `/`, and
/// `Float`/`Double` `description` formatting with a trailing `.0` removed.
pub fn swift_json(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => swift_number(n, out),
        Value::String(s) => swift_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                swift_json(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                swift_string(key, out);
                out.push(':');
                swift_json(&map[key], out);
            }
            out.push('}');
        }
    }
}
fn swift_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '/' => out.push_str("\\/"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}
fn swift_number(n: &Number, out: &mut String) {
    if let Some(i) = n.as_i64() {
        out.push_str(&i.to_string());
    } else if let Some(u) = n.as_u64() {
        out.push_str(&u.to_string());
    } else if let Some(x) = n.as_f64() {
        // Values exactly representable as Float are Swift `Float` fields here
        // (gain, pan, parameters, automation); others are `Double`.
        if x.is_finite() && x.abs() <= f32::MAX as f64 && (x as f32) as f64 == x {
            swift_float(
                x == 0. && x.is_sign_negative(),
                &format!("{:e}", x as f32),
                x.abs() > 16777216.,
                out,
            )
        } else {
            swift_float(
                x == 0. && x.is_sign_negative(),
                &format!("{x:e}"),
                x.abs() > 9007199254740992.,
                out,
            )
        }
    }
}
/// Lays out shortest round-trip digits from Rust `{:e}` as Swift `description`.
fn swift_float(negative_zero: bool, scientific: &str, large: bool, out: &mut String) {
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((scientific, "0"));
    let exponent = exponent.parse::<i32>().unwrap_or(0);
    let (sign, mantissa) = match mantissa.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", mantissa),
    };
    let digits = mantissa.replace('.', "");
    if digits.bytes().all(|b| b == b'0') {
        out.push_str(if negative_zero || !sign.is_empty() {
            "-0"
        } else {
            "0"
        });
        return;
    }
    out.push_str(sign);
    if large || exponent < -4 {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if exponent < 0 { '-' } else { '+' });
        out.push_str(&format!("{:02}", exponent.abs()));
    } else if exponent < 0 {
        out.push_str("0.");
        for _ in 0..(-exponent - 1) {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        let point = exponent as usize + 1;
        if digits.len() <= point {
            out.push_str(&digits);
            for _ in digits.len()..point {
                out.push('0');
            }
        } else {
            out.push_str(&digits[..point]);
            out.push('.');
            out.push_str(&digits[point..]);
        }
    }
}

#[cfg(test)]
#[path = "mastering_tests.rs"]
mod tests;
