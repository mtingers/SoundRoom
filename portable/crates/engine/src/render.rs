//! Deterministic DSP graph: bounded offline file renderer and worker-prepared resident graph.
//! The resident callback path still requires hardware deadline and host ownership qualification.
use crate::{
    automation::Lane,
    automation_write::{Override, ResolvedWriteTarget, Route, WriteTarget, RAMP_FRAMES},
    envelope::Envelope,
    model::{Effect, Session, Track},
    project::Project,
    wave::{WaveReader, WaveWriter},
    Error, Result,
};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    sync::Arc,
};
const BLOCK: usize = 8192;
/// Immutable source/automation loop domain. DSP history continues across wraps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoopRange {
    pub start: u64,
    pub end: u64,
}
impl LoopRange {
    pub fn new(start: u64, end: u64) -> Result<Self> {
        if end <= start || end > crate::model::MAX_FRAME {
            return Err(Error::Invalid("Invalid loop range".into()));
        }
        Ok(Self { start, end })
    }
    fn wrap(self, frame: i64) -> i64 {
        self.start as i64 + (frame - self.start as i64).rem_euclid((self.end - self.start) as i64)
    }
}
fn timeline_frame(frame: i64, range: Option<LoopRange>) -> i64 {
    range.map_or(frame, |r| r.wrap(frame))
}
/// Frames a live edit ramps over (Swift's `LiveRamp`): ten milliseconds
/// smooths manual edits without changing saved or offline renders.
const LIVE_RAMP: u32 = 480;
/// A linear glide from the value a live edit replaced to its new value.
#[derive(Clone, Copy)]
struct Ramp {
    start: f32,
    target: f32,
    remaining: u32,
    current: f32,
}
impl Ramp {
    fn between(start: f32, target: f32) -> Option<Self> {
        (start != target && start.is_finite() && target.is_finite()).then_some(Self {
            start,
            target,
            remaining: LIVE_RAMP,
            current: start,
        })
    }
    /// Advances one frame; None once the target is reached.
    fn advance(ramp: &mut Option<Self>) -> Option<f32> {
        let r = ramp.as_mut()?;
        r.remaining -= 1;
        r.current = if r.remaining == 0 {
            r.target
        } else {
            r.start + (r.target - r.start) * (LIVE_RAMP - r.remaining) as f32 / LIVE_RAMP as f32
        };
        let value = r.current;
        if r.remaining == 0 {
            *ramp = None;
        }
        Some(value)
    }
}
#[derive(Clone, Copy)]
pub(crate) struct Descriptor {
    pub(crate) key: &'static str,
    min: f32,
    max: f32,
    pub(crate) default: f32,
    log: bool,
    integer: bool,
}
impl Descriptor {
    fn physical(&self, n: f32) -> f32 {
        if self.log {
            let v = (self.min * (self.max / self.min).powf(n)).clamp(self.min, self.max);
            if self.integer {
                v.round()
            } else {
                v
            }
        } else {
            let v = (self.min + (self.max - self.min) * n).clamp(self.min, self.max);
            if self.integer {
                v.round()
            } else {
                v
            }
        }
    }
    pub(crate) fn normalized(&self, v: f32) -> f32 {
        let v = v.clamp(self.min, self.max);
        let v = if self.integer { v.round() } else { v };
        if self.log {
            (v / self.min).ln() / (self.max / self.min).ln()
        } else {
            (v - self.min) / (self.max - self.min)
        }
    }
}
fn descriptor(key: &'static str, min: f32, max: f32, default: f32, log: bool) -> Descriptor {
    Descriptor {
        key,
        min,
        max,
        default,
        log,
        integer: false,
    }
}
fn choice(key: &'static str, min: f32, max: f32, default: f32) -> Descriptor {
    Descriptor {
        key,
        min,
        max,
        default,
        log: false,
        integer: true,
    }
}
/// Canonical default physical parameters for a realtime effect kind (empty if unknown).
pub(crate) fn effect_latency(e: &Effect) -> usize {
    Processor::kind_latency(
        &e.kind,
        e.parameters.is_empty() && e.parameter_automation.is_empty(),
    )
}
pub fn default_parameters(kind: &str) -> BTreeMap<String, f32> {
    descriptors(kind)
        .into_iter()
        .map(|d| (d.key.to_string(), d.default))
        .collect()
}
pub(crate) fn descriptors(kind: &str) -> Vec<Descriptor> {
    match kind {
        "guitarRig" => vec![
            descriptor("input", -24.0, 18.0, 0.0, false),
            descriptor("gate", -90.0, -20.0, -75.0, false),
            choice("pedal", 0.0, 3.0, 0.0),
            descriptor("pedalDrive", 0.0, 30.0, 12.0, false),
            descriptor("pedalTone", 400.0, 12000.0, 4500.0, true),
            descriptor("pedalLevel", -18.0, 12.0, 0.0, false),
            choice("amp", 0.0, 4.0, 1.0),
            descriptor("ampDrive", 0.0, 36.0, 9.0, false),
            descriptor("bass", -12.0, 12.0, 0.0, false),
            descriptor("mid", -12.0, 12.0, 0.0, false),
            descriptor("treble", -12.0, 12.0, 0.0, false),
            descriptor("presence", -12.0, 12.0, 0.0, false),
            descriptor("sag", 0.0, 1.0, 0.2, false),
            descriptor("master", -24.0, 6.0, -3.0, false),
            choice("cabinet", 0.0, 4.0, 1.0),
            descriptor("mic", 0.0, 1.0, 0.35, false),
            descriptor("lowCut", 20.0, 200.0, 60.0, true),
            descriptor("highCut", 2000.0, 16000.0, 8000.0, true),
            choice("modulation", 0.0, 2.0, 0.0),
            descriptor("modRate", 0.05, 8.0, 0.9, true),
            descriptor("modDepth", 0.0, 1.0, 0.35, false),
            descriptor("modMix", 0.0, 1.0, 0.25, false),
            descriptor("delayTone", 800.0, 12000.0, 4500.0, true),
            descriptor("delayTime", 20.0, 1200.0, 380.0, true),
            descriptor("delayFeedback", 0.0, 0.7, 0.3, false),
            descriptor("delayMix", 0.0, 0.6, 0.0, false),
            descriptor("reverbDecay", 0.2, 5.0, 1.4, true),
            descriptor("reverbMix", 0.0, 0.5, 0.08, false),
            descriptor("output", -24.0, 12.0, 0.0, false),
            descriptor("mix", 0.0, 1.0, 1.0, false),
        ],
        "gain" => vec![descriptor("gain", -24., 24., 0., false)],
        "soundPutty" => vec![
            descriptor("start", 45., 800., 95., true),
            descriptor("end", 45., 1600., 560., true),
            descriptor("curve", 0.2, 4., 1.2, false),
            choice("wild", 0., 299., 160.),
            descriptor("width", 1., 90., 22., false),
            descriptor("intensity", 0., 1., 0.7, false),
            choice("wildMode", 0., 3., 0.),
            choice("excite", 0., 2., 0.),
            choice("exciteAt", 0., 299., 145.),
            descriptor("spread", 1., 150., 85., false),
            descriptor("coupling", 0., 0.3, 0.075, false),
            descriptor("decay", 0.1, 5., 1.8, true),
            descriptor("drive", -12., 24., 0., false),
            descriptor("mix", 0., 1., 0.4, false),
            descriptor("output", -24., 12., 0., false),
            choice("variationSeed", 0., 16777215., 1234.),
        ],
        "vocalEnhancer" => vec![
            descriptor("cleanup", 0., 1., 0.45, false),
            descriptor("body", -6., 6., 0., false),
            descriptor("clarity", 0., 1., 0.45, false),
            descriptor("deEss", 0., 1., 0.5, false),
            descriptor("dynamics", 0., 1., 0.45, false),
            descriptor("air", 0., 1., 0.2, false),
            descriptor("warmth", 0., 1., 0.2, false),
            descriptor("output", -12., 12., 0., false),
            descriptor("mix", 0., 1., 1., false),
        ],
        "sourceIsolator" => vec![
            choice("mode", 0., 4., 0.),
            descriptor("strength", 0., 1., 0.75, false),
            descriptor("noiseFloor", -90., -20., -55., false),
            descriptor("low", 20., 12000., 20., true),
            descriptor("high", 200., 20000., 20000., true),
            descriptor("mix", 0., 1., 1., false),
            choice("listen", 0., 1., 0.),
        ],
        "lowPass" => vec![
            descriptor("frequency", 30., 20000., 4000., true),
            descriptor("resonance", 0.5, 8., 0.707, true),
        ],
        "highPass" => vec![
            descriptor("frequency", 20., 12000., 100., true),
            descriptor("resonance", 0.5, 8., 0.707, true),
        ],
        "echo" => vec![
            descriptor("time", 1., 2000., 250., true),
            descriptor("feedback", 0., 0.95, 0.4, false),
            descriptor("mix", 0., 1., 0.25, false),
            descriptor("damping", 200., 20000., 12000., true),
        ],
        "gate" => vec![
            descriptor("threshold", -90., 0., -40., false),
            descriptor("ratio", 1., 20., 4., false),
            descriptor("range", 0., 96., 60., false),
            descriptor("attack", 0.1, 100., 1., true),
            descriptor("hold", 0., 1000., 50., false),
            descriptor("release", 5., 3000., 150., true),
            descriptor("hysteresis", 0., 24., 4., false),
        ],
        "utility" => vec![
            descriptor("gain", -24., 24., 0., false),
            choice("polarityL", 0., 1., 0.),
            choice("polarityR", 0., 1., 0.),
            choice("swap", 0., 1., 0.),
            choice("mono", 0., 1., 0.),
            descriptor("width", 0., 2., 1., false),
            choice("delayL", 0., 4800., 0.),
            choice("delayR", 0., 4800., 0.),
        ],
        "compressor" => vec![
            descriptor("threshold", -60., 0., -18., false),
            descriptor("ratio", 1., 20., 4., false),
            descriptor("attack", 0.1, 200., 10., true),
            descriptor("release", 10., 2000., 100., true),
            descriptor("knee", 0., 24., 6., false),
            descriptor("makeup", -12., 24., 0., false),
            descriptor("mix", 0., 1., 1., false),
        ],
        "chorus" => vec![
            descriptor("rate", 0.05, 8., 0.8, true),
            descriptor("depth", 0., 20., 3., false),
            descriptor("delay", 1., 40., 16., false),
            descriptor("width", 0., 1., 0.7, false),
            descriptor("feedback", 0., 0.85, 0.1, false),
            descriptor("mix", 0., 1., 0.35, false),
        ],
        "pitchCorrection" => vec![
            choice("key", 0., 11., 0.),
            choice("scale", 0., 3., 0.),
            choice("mask", 0., 4095., 4095.),
            descriptor("reference", 420., 460., 440., false),
            descriptor("retune", 0., 500., 40., false),
            descriptor("strength", 0., 1., 1., false),
            descriptor("humanize", 0., 1., 0.35, false),
            descriptor("tolerance", 0., 100., 5., false),
            descriptor("vibrato", 0., 1., 0.5, false),
            descriptor("formant", -12., 12., 0., false),
            descriptor("mix", 0., 1., 1., false),
            descriptor("manualTarget", -1., 95., -1., false),
        ],
        "dither" => vec![
            choice("bits", 8., 24., 24.),
            descriptor("amount", 0., 1., 1., false),
            choice("quantize", 0., 1., 0.),
        ],
        "equalizer" => vec![
            descriptor("lowGain", -18., 18., 0., false),
            descriptor("midFrequency", 150., 10000., 1000., true),
            descriptor("midGain", -18., 18., 0., false),
            descriptor("midQ", 0.3, 8., 1., true),
            descriptor("highGain", -18., 18., 0., false),
            descriptor("output", -24., 12., 0., false),
            descriptor("lowFrequency", 20., 1000., 100., true),
            descriptor("lowQ", 0.3, 8., 0.707, true),
            choice("lowShape", 0., 1., 0.),
            descriptor("highFrequency", 1000., 20000., 8000., true),
            descriptor("highQ", 0.3, 8., 0.707, true),
            choice("highShape", 0., 1., 0.),
            descriptor("highPassFrequency", 20., 1000., 60., true),
            choice("highPassEnabled", 0., 1., 0.),
            descriptor("lowPassFrequency", 1000., 20000., 16000., true),
            choice("lowPassEnabled", 0., 1., 0.),
            descriptor("band4Frequency", 20., 20000., 400., true),
            descriptor("band4Gain", -18., 18., 0., false),
            descriptor("band4Q", 0.3, 12., 1., true),
            descriptor("band4Threshold", -60., 0., -24., false),
            descriptor("band4Range", -18., 0., 0., false),
            descriptor("band5Frequency", 20., 20000., 2000., true),
            descriptor("band5Gain", -18., 18., 0., false),
            descriptor("band5Q", 0.3, 12., 1., true),
            descriptor("band5Threshold", -60., 0., -24., false),
            descriptor("band5Range", -18., 0., 0., false),
            descriptor("band6Frequency", 20., 20000., 6000., true),
            descriptor("band6Gain", -18., 18., 0., false),
            descriptor("band6Q", 0.3, 12., 1., true),
            descriptor("band6Threshold", -60., 0., -24., false),
            descriptor("band6Range", -18., 0., 0., false),
            descriptor("dynamicAttack", 0.1, 200., 10., true),
            descriptor("dynamicRelease", 10., 2000., 150., true),
        ],
        "room" => vec![
            descriptor("decay", 0.1, 10., 1.4, true),
            descriptor("predelay", 0., 200., 12., false),
            descriptor("damping", 200., 20000., 6000., true),
            descriptor("mix", 0., 1., 0.2, false),
            choice("character", 0., 1., 0.),
            descriptor("lowCut", 0., 1000., 120., false),
            descriptor("diffusion", 0., 1., 0.8, false),
            descriptor("width", 0., 1., 1., false),
            descriptor("early", 0., 1., 0.25, false),
            descriptor("output", -24., 12., 0., false),
        ],
        "limiter" => vec![
            descriptor("input", -12., 24., 0., false),
            descriptor("ceiling", -12., 0., -1., false),
            descriptor("release", 20., 1000., 150., true),
            choice("style", 0., 1., 1.),
            choice("truePeak", 0., 1., 1.),
        ],
        "resonance" => vec![
            descriptor("depth", 0., 18., 6., false),
            descriptor("sensitivity", 0., 1., 0.5, false),
            descriptor("sharpness", 0., 1., 0.6, false),
            descriptor("low", 80., 8000., 500., true),
            descriptor("high", 1000., 20000., 16000., true),
            descriptor("attack", 1., 100., 12., true),
            descriptor("release", 20., 1000., 160., true),
            descriptor("transients", 0., 1., 0.7, false),
            descriptor("mix", 0., 1., 1., false),
            choice("delta", 0., 1., 0.),
        ],
        "warmth" => vec![
            descriptor("drive", 0., 30., 9., false),
            descriptor("tone", 200., 20000., 12000., true),
            descriptor("mix", 0., 1., 1., false),
            descriptor("output", -24., 12., 0., false),
            choice("match", 0., 1., 0.),
            choice("character", 0., 4., 0.),
            descriptor("bias", 0., 1., 0.25, false),
            descriptor("emphasis", -12., 12., 0., false),
        ],
        _ => vec![],
    }
}
// Display units match SoundroomCore/EffectParameters.swift; the numerical
// mapping remains the existing DSP descriptor, including logarithmic ranges.
fn parameter_unit(kind: &str, key: &str) -> &'static str {
    match key {
        "gain" | "input" | "output" | "threshold" | "knee" | "makeup" | "lowGain" | "midGain"
        | "highGain" | "emphasis" | "body" => "dB",
        "noiseFloor" => "dBFS",
        "ceiling" => "dBTP",
        "depth" if kind == "resonance" => "dB",
        "depth" if kind == "chorus" => "ms",
        "frequency" | "damping" | "tone" | "rate" | "midFrequency" | "lowFrequency"
        | "highFrequency" | "highPassFrequency" | "lowPassFrequency" | "lowCut" | "low"
        | "high" | "reference" => "Hz",
        "attack" | "release" | "time" | "delay" | "predelay" | "retune" => "ms",
        "resonance" | "midQ" | "lowQ" | "highQ" => "Q",
        "decay" => "s",
        "ratio" => ":1",
        "formant" => "st",
        "tolerance" => "cents",
        "mix" | "feedback" | "width" | "diffusion" | "early" | "sensitivity" | "sharpness"
        | "transients" | "bias" | "strength" | "humanize" | "vibrato" | "amount" | "cleanup"
        | "clarity" | "deEss" | "dynamics" | "air" | "warmth" => "%",
        _ => "",
    }
}
#[derive(Clone, Copy, Default)]
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}
impl Biquad {
    fn configure(&mut self, hz: f32, q: f32, high: bool) {
        let w = 2. * std::f32::consts::PI * hz.min(21600.) / 48000.;
        let c = w.cos();
        let alpha = w.sin() / (2. * q);
        let d = 1. + alpha;
        self.b = if high {
            [(1. + c) / 2., -(1. + c), (1. + c) / 2.]
        } else {
            [(1. - c) / 2., 1. - c, (1. - c) / 2.]
        };
        self.a = [-2. * c, 1. - alpha];
        for x in &mut self.b {
            *x /= d
        }
        for x in &mut self.a {
            *x /= d
        }
    }
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        if !y.is_finite() || self.z.iter().any(|v| !v.is_finite()) {
            self.z = [0.; 2];
            0.
        } else {
            y
        }
    }
}
struct PlaybackEffect {
    id: String,
    kind: String,
    enabled: bool,
    parameters_empty: bool,
    putty_shape: Option<Arc<crate::dsp::sound_putty::Shape>>,
    pitch_edits_only: bool,
    maxima: Vec<f32>,
}
/// Parameter lanes for an insert: automation over the authored baseline.
fn effect_lanes(e: &Effect, descriptors: &[Descriptor]) -> Vec<Lane> {
    descriptors
        .iter()
        .map(|d| {
            Lane::new(
                e.parameter_automation
                    .get(d.key)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
                d.normalized(*e.parameters.get(d.key).unwrap_or(&d.default)),
                false,
            )
        })
        .collect()
}
/// Sorted, merged ranges whose audio was already rendered through the insert.
fn baked_ranges(e: &Effect) -> Result<Vec<[u64; 2]>> {
    let mut baked = vec![];
    if let Some(ranges) = e.extra.get("bakedRanges") {
        if !ranges.is_null() {
            let ranges = ranges
                .as_array()
                .ok_or_else(|| Error::Invalid("Invalid baked ranges".into()))?;
            for r in ranges {
                let start = r
                    .get("startFrame")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| Error::Invalid("Invalid baked range start".into()))?;
                let end = r
                    .get("endFrame")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| Error::Invalid("Invalid baked range end".into()))?;
                if end < start || end > crate::model::MAX_FRAME {
                    return Err(Error::Invalid("Invalid baked range".into()));
                }
                baked.push([start, end]);
            }
        }
    }
    baked.sort_by_key(|r| r[0]);
    let mut merged: Vec<[u64; 2]> = vec![];
    for range in baked {
        if range[0] == range[1] {
            continue;
        }
        if let Some(last) = merged.last_mut().filter(|r| range[0] <= r[1]) {
            last[1] = last[1].max(range[1]);
        } else {
            merged.push(range);
        }
    }
    Ok(merged)
}
fn playback_effect(e: &Effect, descriptors: &[Descriptor]) -> PlaybackEffect {
    PlaybackEffect {
        id: e.id.clone(),
        kind: e.kind.clone(),
        enabled: e.enabled,
        parameters_empty: e.parameters.is_empty(),
        putty_shape: (e.kind == "soundPutty")
            .then(|| Arc::new(crate::dsp::sound_putty::shape(&e.parameters))),
        pitch_edits_only: e
            .extra
            .get("pitchEditsOnly")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        maxima: descriptors
            .iter()
            .map(|d| {
                let baseline = e
                    .parameters
                    .get(d.key)
                    .copied()
                    .unwrap_or(d.default)
                    .clamp(d.min, d.max);
                e.parameter_automation.get(d.key).map_or(baseline, |lane| {
                    lane.iter()
                        .fold(baseline, |v, p| v.max(d.physical(p.value)))
                })
            })
            .collect(),
    }
}
/// An insert's kind, resolved once: the per-sample dispatch matched the
/// kind's name as a string for every insert on every sample.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Gain,
    LowPass,
    HighPass,
    Echo,
    Compressor,
    Gate,
    GuitarRig,
    Utility,
    Chorus,
    Dither,
    Equalizer,
    Room,
    Limiter,
    Resonance,
    Warmth,
    PitchCorrection,
    SoundPutty,
    VocalEnhancer,
    SourceIsolator,
    Other,
}
impl Kind {
    fn of(kind: &str) -> Self {
        match kind {
            "gain" => Self::Gain,
            "lowPass" => Self::LowPass,
            "highPass" => Self::HighPass,
            "echo" => Self::Echo,
            "compressor" => Self::Compressor,
            "gate" => Self::Gate,
            "guitarRig" => Self::GuitarRig,
            "utility" => Self::Utility,
            "chorus" => Self::Chorus,
            "dither" => Self::Dither,
            "equalizer" => Self::Equalizer,
            "room" => Self::Room,
            "limiter" => Self::Limiter,
            "resonance" => Self::Resonance,
            "warmth" => Self::Warmth,
            "pitchCorrection" => Self::PitchCorrection,
            "soundPutty" => Self::SoundPutty,
            "vocalEnhancer" => Self::VocalEnhancer,
            "sourceIsolator" => Self::SourceIsolator,
            _ => Self::Other,
        }
    }
}
#[derive(Clone)]
struct Processor {
    kind: Kind,
    effect: Arc<PlaybackEffect>,
    legacy: bool,
    amount: Lane,
    descriptors: Arc<[Descriptor]>,
    lanes: Arc<[Lane]>,
    biquads: [Biquad; 2],
    filter_controls: Option<[f32; 2]>,
    filters: [f32; 2],
    delay: Vec<[f32; 2]>,
    delay_index: usize,
    envelope: f32,
    external_detector: Option<f32>,
    baked: Arc<[[u64; 2]]>,
    chorus: Option<crate::dsp::chorus::Chorus>,
    gate: Option<crate::dsp::professional::Gate>,
    utility: Option<Box<crate::dsp::professional::Utility>>,
    eq: Option<crate::dsp::equalizer::Equalizer>,
    room: Option<crate::dsp::room::Room>,
    limiter: Option<Box<crate::dsp::limiter::Limiter>>,
    resonance: Option<crate::dsp::resonance::Resonance>,
    putty: Option<Box<crate::dsp::sound_putty::SoundPutty>>,
    vocal: Option<Box<crate::dsp::vocal::VocalEnhancer>>,
    isolation: Option<crate::dsp::isolation::Isolation>,
    guitar_rig: Option<Box<crate::dsp::guitar_rig::GuitarRig>>,
    warmth: Option<Box<crate::dsp::warmth::Warmth>>,
    values: Vec<f32>,
    cached_values: Vec<f32>,
    noise_seed: u64,
    pitch: Option<crate::pitch::Pitch>,
    pitch_regions: Arc<crate::pitch::Regions>,
    amount_write: Option<Override>,
    parameter_writes: Vec<Option<Override>>,
    /// Live meters (callback graphs only) and this sample's gain reduction.
    meter: crate::meters::Accumulator,
    reduction_db: f32,
    /// Limiter output true peak since the last meter flush.
    true_peak: f32,
    /// Live-edit glides of unautomated parameters (normalized), the amount,
    /// and the wet/dry blend when bypass toggles.
    parameter_ramps: Vec<Option<Ramp>>,
    amount_ramp: Option<Ramp>,
    bypass_blend: f32,
    bypass_ramp: Option<Ramp>,
    ramping: bool,
    constants: Vec<f32>,
    /// Every parameter is constant and `values` already holds them (and the
    /// DSP is configured for them): the per-sample refresh is skipped.
    settled: bool,
    all_constant: bool,
    latency_frames: usize,
}
impl Processor {
    /// Effect-specific meter readings (Swift fills these per effect kind).
    fn meter_extras(&mut self) -> crate::meters::Extras {
        let mut extras = crate::meters::Extras::default();
        if let Some(limiter) = &self.limiter {
            extras.true_peak = std::mem::take(&mut self.true_peak);
            extras.momentary = limiter.loudness.momentary;
            extras.short_term = limiter.loudness.short_term;
        }
        if let Some(resonance) = &self.resonance {
            extras.bands = resonance.display;
        }
        if let Some(isolation) = &self.isolation {
            extras.bands = isolation.display;
        }
        if let Some(pitch) = &self.pitch {
            let m = &pitch.meter;
            extras.pitch = Some(crate::meters::PitchReading {
                input_frequency: m.input_frequency,
                target_frequency: m.target_frequency,
                correction_cents: m.correction_cents,
                confidence: m.confidence,
                wet_blend: m.wet_blend,
                reference_frequency: m.reference_frequency,
                source_frame: m.source_frame,
            });
        }
        extras
    }
    fn new(e: &Effect) -> Result<Self> {
        let legacy = e.parameters.is_empty()
            && e.parameter_automation.is_empty()
            && ["gain", "lowPass", "echo", "warmth"].contains(&e.kind.as_str());
        let supported = matches!(
            e.kind.as_str(),
            "gain"
                | "lowPass"
                | "highPass"
                | "echo"
                | "compressor"
                | "guitarRig"
                | "gate"
                | "utility"
                | "chorus"
                | "dither"
                | "equalizer"
                | "room"
                | "limiter"
                | "resonance"
                | "soundPutty"
                | "vocalEnhancer"
                | "sourceIsolator"
                | "warmth"
                | "pitchCorrection"
        ) || e.kind == "warmth" && legacy;
        if e.enabled && !supported {
            return Err(Error::Unsupported(format!(
                "Portable DSP has not qualified {} ({}) yet; use the Swift server",
                e.kind, e.id
            )));
        }
        let descriptors = descriptors(&e.kind);
        let lanes = effect_lanes(e, &descriptors);
        let delay = if e.kind == "echo" {
            vec![[0.; 2]; if legacy { 12000 } else { 96002 }]
        } else if e.kind == "room" {
            vec![[0.; 2]; 9602]
        } else {
            vec![]
        };
        let baked = baked_ranges(e)?;
        let count = descriptors.len();
        let mut noise_seed = 0xcbf29ce484222325_u64;
        let text = e.id.replace('-', "");
        for i in (0..32).step_by(2) {
            let byte = u8::from_str_radix(&text[i..i + 2], 16)
                .map_err(|_| crate::Error::Invalid("Invalid effect UUID".into()))?;
            noise_seed = (noise_seed ^ byte as u64).wrapping_mul(0x100000001b3);
        }
        Ok(Self {
            kind: Kind::of(&e.kind),
            effect: Arc::new(playback_effect(e, &descriptors)),
            legacy,
            amount: Lane::new(&e.automation, e.amount, false),
            descriptors: descriptors.into(),
            lanes: lanes.into(),
            biquads: [Biquad::default(); 2],
            filter_controls: None,
            filters: [0.; 2],
            delay,
            delay_index: 0,
            envelope: 0.,
            external_detector: None,
            baked: baked.into(),
            chorus: if e.kind == "chorus" {
                Some(Default::default())
            } else {
                None
            },
            gate: (e.kind == "gate").then(Default::default),
            utility: (e.kind == "utility").then(Box::default),
            guitar_rig: (e.kind == "guitarRig").then(Box::default),
            eq: if e.kind == "equalizer" {
                Some(Default::default())
            } else {
                None
            },
            room: if e.kind == "room" {
                Some(Default::default())
            } else {
                None
            },
            limiter: if e.kind == "limiter" {
                Some(Box::default())
            } else {
                None
            },
            resonance: if e.kind == "resonance" {
                Some(Default::default())
            } else {
                None
            },
            putty: if e.kind == "soundPutty" {
                Some(Box::default())
            } else {
                None
            },
            vocal: if e.kind == "vocalEnhancer" {
                Some(Box::default())
            } else {
                None
            },
            isolation: if e.kind == "sourceIsolator" {
                Some(Default::default())
            } else {
                None
            },
            warmth: if e.kind == "warmth" && !legacy {
                Some(Box::default())
            } else {
                None
            },
            values: vec![0.; count],
            cached_values: vec![f32::NAN; count],
            noise_seed,
            pitch: if e.kind == "pitchCorrection" {
                Some(Default::default())
            } else {
                None
            },
            pitch_regions: Arc::new(crate::pitch::Regions::from_effect(e)?),
            amount_write: None,
            parameter_writes: vec![None; count],
            meter: Default::default(),
            reduction_db: 0.,
            true_peak: 0.,
            parameter_ramps: vec![None; count],
            amount_ramp: None,
            bypass_blend: if e.enabled { 1. } else { 0. },
            bypass_ramp: None,
            ramping: false,
            constants: vec![f32::NAN; count],
            settled: false,
            all_constant: false,
            latency_frames: Self::kind_latency(&e.kind, legacy),
        })
        .map(|mut processor: Self| {
            if let (Some(state), Some(shape)) =
                (&mut processor.putty, &processor.effect.putty_shape)
            {
                state.set_shape(shape);
            }
            processor.refresh_constants();
            processor
        })
    }
    /// Look-ahead in frames; fixed for an insert (live patches keep its kind).
    fn latency(&self) -> usize {
        self.latency_frames
    }
    fn kind_latency(kind: &str, legacy: bool) -> usize {
        match kind {
            "guitarRig" => crate::dsp::guitar_rig::LATENCY,
            "limiter" => 512,
            "resonance" => 2048,
            "vocalEnhancer" => crate::dsp::vocal::LATENCY,
            "sourceIsolator" => crate::dsp::isolation::Isolation::LATENCY_FRAMES,
            "warmth" if !legacy => 16,
            "pitchCorrection" => 4096,
            _ => 0,
        }
    }
    /// Parameters with no curve hold one value: compute it once instead of
    /// per sample (a lane search and often a `powf` for every parameter of
    /// every insert, 48 000 times a second). NaN marks a varying parameter.
    /// Empty-parameter recipes follow the amount, so they are never constant.
    fn refresh_constants(&mut self) {
        for i in 0..self.constants.len() {
            self.constants[i] = if self.effect.parameters_empty || self.lanes[i].has_points() {
                f32::NAN
            } else {
                self.descriptors[i].physical(self.lanes[i].value(0))
            };
        }
        self.all_constant = self.constants.iter().all(|c| !c.is_nan());
        self.settled = false;
    }
    fn maximum(&self, key: &str) -> f32 {
        self.descriptors
            .iter()
            .position(|d| d.key == key)
            .map_or(0., |i| self.effect.maxima[i])
    }
    fn tail_frames(&self) -> u64 {
        if !self.effect.enabled {
            return 0;
        }
        if self.effect.kind == "utility" {
            return self.maximum("delayL").max(self.maximum("delayR")).round() as u64;
        }
        if self.effect.kind == "guitarRig" {
            let mut seconds = if self.maximum("pedal") > 0. || self.maximum("amp") > 0. || self.maximum("cabinet") > 0. { 0.25 } else { 0. };
            if self.maximum("modulation") > 0. && self.maximum("modMix") > 0. { seconds += 0.03; }
            if self.maximum("delayMix") > 0. {
                let feedback = self.maximum("delayFeedback");
                let repeats = if feedback > 0. { (0.001_f32.ln() / feedback.ln()).ceil().max(1.) } else { 1. };
                seconds += self.maximum("delayTime") / 1000. * repeats;
            }
            if self.maximum("reverbMix") > 0. { seconds += self.maximum("reverbDecay") + 0.15; }
            return (seconds.min(30.) * 48000.) as u64;
        }
        if self.effect.kind == "soundPutty" {
            let minimum = self.effect.putty_shape.as_ref().map_or(0., |shape| {
                shape[2].iter().copied().fold(f64::INFINITY, f64::min)
            });
            return ((6.907756 * self.maximum("decay") as f64 * (-2. * minimum).exp()).min(30.)
                * 48000.) as u64;
        }
        let seconds = match self.effect.kind.as_str() {
            "echo" => {
                let time = if self.effect.parameters_empty {
                    250.
                } else {
                    self.maximum("time")
                };
                let feedback = if self.effect.parameters_empty {
                    0.4
                } else {
                    self.maximum("feedback")
                };
                let repeats = if feedback > 0. {
                    (0.001_f32.ln() / feedback.ln()).ceil().max(1.)
                } else {
                    1.
                };
                time / 1000. * repeats
            }
            "room" => {
                self.maximum("decay")
                    + self.maximum("predelay") / 1000.
                    + if self.maximum("character") >= 0.5 {
                        0.15
                    } else {
                        0.
                    }
            }
            "chorus" => {
                let delay = (self.maximum("delay") + self.maximum("depth")) / 1000.;
                let feedback = self.maximum("feedback");
                delay
                    * if feedback > 0. {
                        (0.001_f32.ln() / feedback.ln()).ceil().max(1.)
                    } else {
                        1.
                    }
            }
            _ => 0.,
        };
        (seconds.min(30.) * 48000.) as u64
    }
    fn amount_value(&mut self, frame: i64) -> f32 {
        let original = self
            .amount_ramp
            .map_or_else(|| self.amount.value_signed(frame), |r| r.current);
        self.amount_write
            .as_mut()
            .filter(|_| frame >= 0)
            .map_or(original, |state| state.sample(frame as u64, original))
    }
    fn parameter(&mut self, index: usize, frame: i64) -> f32 {
        let d = self.descriptors[index];
        if let Some(state) = self.parameter_writes[index].as_mut().filter(|_| frame >= 0) {
            let original = self.lanes[index].value_signed(frame);
            return d.physical(state.sample(frame as u64, original));
        }
        if let Some(ramp) = self.parameter_ramps[index] {
            return d.physical(ramp.current);
        }
        let constant = self.constants[index];
        if !constant.is_nan() {
            return constant;
        }
        self.parameter_original(index, frame, true)
    }
    fn has_recipe_parameter(&self, index: usize) -> bool {
        self.effect.parameters_empty
            && !self.lanes[index].has_points()
            && matches!(
                (self.effect.kind.as_str(), self.descriptors[index].key),
                ("gain", "gain")
                    | ("lowPass", "frequency")
                    | ("echo", "time" | "feedback" | "mix" | "damping")
                    | ("warmth", "drive" | "mix")
            )
    }
    fn parameter_original(&mut self, index: usize, frame: i64, live_amount: bool) -> f32 {
        let d = self.descriptors[index];
        if self.effect.parameters_empty && !self.lanes[index].has_points() {
            let a = if live_amount {
                self.amount_value(frame)
            } else {
                self.amount.value_signed(frame)
            };
            match (self.effect.kind.as_str(), d.key) {
                ("gain", "gain") => return a * 48. - 24.,
                ("lowPass", "frequency") => return 200. * 80_f32.powf(1. - a),
                ("echo", "time") => return 250.,
                ("echo", "feedback") => return 0.4,
                ("echo", "mix") => return a * 0.65,
                ("echo", "damping") => return 20000.,
                ("warmth", "drive") => return 20. * (1. + a * 8.).log10(),
                ("warmth", "mix") => return 1.,
                _ => {}
            }
        }
        d.physical(self.lanes[index].value_signed(frame))
    }
    /// Starts glides from this insert's current values to `next`'s (the
    /// live patch about to be swapped in), as Swift's live transition does.
    fn begin_ramps(&mut self, next: &ProcessorParts) {
        let target = if next.effect.enabled { 1. } else { 0. };
        let blend = self.bypass_ramp.map_or(self.bypass_blend, |r| r.current);
        self.bypass_ramp = Ramp::between(blend, target);
        // Drawn and written curves keep their sample-accurate values.
        if !self.amount.has_points() && !next.amount.has_points() {
            let from = self.amount_ramp.map_or(self.amount.value(0), |r| r.current);
            self.amount_ramp = Ramp::between(from, next.amount.value(0));
        } else {
            self.amount_ramp = None;
        }
        // Empty parameters follow the amount (legacy recipes).
        let recipes = self.effect.parameters_empty || next.effect.parameters_empty;
        for (i, ramp) in self.parameter_ramps.iter_mut().enumerate() {
            let (old, new) = (&self.lanes[i], &next.lanes[i]);
            *ramp = if recipes || old.has_points() || new.has_points() {
                None
            } else {
                Ramp::between(ramp.map_or(old.value(0), |r| r.current), new.value(0))
            };
        }
        self.ramping = true;
    }
    fn advance_ramps(&mut self) {
        if let Some(blend) = Ramp::advance(&mut self.bypass_ramp) {
            self.bypass_blend = blend;
        }
        if !self.ramping {
            return;
        }
        let mut active = Ramp::advance(&mut self.amount_ramp).is_some();
        for ramp in &mut self.parameter_ramps {
            active |= Ramp::advance(ramp).is_some();
        }
        self.ramping = active;
    }
    fn sample(&mut self, input: [f32; 2], frame: i64, loop_range: Option<LoopRange>) -> [f32; 2] {
        if !self.effect.enabled
            && self.bypass_ramp.is_none()
            && self.latency() == 0
            && !matches!(self.kind, Kind::Utility | Kind::Gate)
        {
            return input;
        }
        self.advance_ramps();
        let mut dry = input;
        let a = self.amount_value(frame);
        let mut out = input;
        if self.legacy {
            match self.kind {
                Kind::Gain => {
                    let g = 10_f32.powf((a * 48. - 24.) / 20.);
                    out = [input[0] * g, input[1] * g]
                }
                Kind::LowPass => {
                    let hz = 200. * 80_f32.powf(1. - a);
                    let c = 1. - (-2. * std::f32::consts::PI * hz / 48000.).exp();
                    for (value, filter) in input.iter().zip(&mut self.filters) {
                        *filter += c * (*value - *filter);
                    }
                    out.copy_from_slice(&self.filters);
                }
                Kind::Echo => {
                    let delayed = self.delay[self.delay_index];
                    for i in 0..2 {
                        self.delay[self.delay_index][i] = input[i] + delayed[i] * 0.4;
                        out[i] += delayed[i] * a * 0.65
                    }
                    self.delay_index = (self.delay_index + 1) % self.delay.len()
                }
                Kind::Warmth => {
                    let d = 1. + a * 8.;
                    for i in 0..2 {
                        out[i] = (input[i] * d).tanh() / d.tanh()
                    }
                }
                _ => {}
            }
        } else {
            let fixed = self.all_constant
                && !self.ramping
                && self.parameter_writes.iter().all(Option::is_none);
            let refreshed = !(fixed && self.settled);
            if refreshed {
                for i in 0..self.values.len() {
                    self.values[i] = self.parameter(i, frame);
                }
                self.settled = fixed;
            }
            // Unrefreshed values equal the ones last compared and configured.
            if refreshed && self.values != self.cached_values {
                if let Some(rig) = &mut self.guitar_rig { rig.configure(&self.values); }
                if let Some(eq) = &mut self.eq {
                    eq.configure(&self.values);
                }
                if let Some(room) = &mut self.room {
                    room.configure(&self.values);
                }
                if let Some(limiter) = &mut self.limiter {
                    limiter.configure(&self.values);
                }
                if let Some(resonance) = &mut self.resonance {
                    resonance.configure(&self.values);
                }
                if let Some(putty) = &mut self.putty {
                    putty.configure(&self.values);
                }
                if let Some(vocal) = &mut self.vocal {
                    vocal.configure(&self.values);
                }
                if let Some(isolation) = &mut self.isolation {
                    isolation.configure(&self.values);
                }
                self.cached_values.copy_from_slice(&self.values);
            }
            match self.kind {
                Kind::Gain => {
                    let g = 10_f32.powf(self.parameter(0, frame) / 20.);
                    out = [input[0] * g, input[1] * g]
                }
                Kind::LowPass | Kind::HighPass => {
                    let hz = self.parameter(0, frame);
                    let q = self.parameter(1, frame);
                    if self.filter_controls != Some([hz, q]) {
                        for filter in &mut self.biquads {
                            filter.configure(hz, q, self.effect.kind == "highPass");
                        }
                        self.filter_controls = Some([hz, q]);
                    }
                    for i in 0..2 {
                        out[i] = self.biquads[i].process(input[i]);
                    }
                }
                Kind::Echo => {
                    let delay = (self.parameter(0, frame) / 1000. * 48000.).max(1.);
                    let feedback = self.parameter(1, frame);
                    let mix = self.parameter(2, frame);
                    let c =
                        1. - (-2. * std::f32::consts::PI * self.parameter(3, frame) / 48000.).exp();
                    let cursor = self.delay_index as f32 - delay;
                    let floor = cursor.floor() as i64;
                    let f = cursor - floor as f32;
                    let first = floor.rem_euclid(self.delay.len() as i64) as usize;
                    let next = (first + 1) % self.delay.len();
                    for i in 0..2 {
                        let delayed = self.delay[first][i] * (1. - f) + self.delay[next][i] * f;
                        self.filters[i] += c * (delayed - self.filters[i]);
                        self.delay[self.delay_index][i] = input[i] + self.filters[i] * feedback;
                        out[i] = input[i] * (1. - mix) + delayed * mix;
                    }
                    self.delay_index = (self.delay_index + 1) % self.delay.len()
                }
                Kind::Compressor => {
                    let level = self
                        .external_detector
                        .unwrap_or_else(|| input[0].abs().max(input[1].abs()));
                    let ms = if level > self.envelope {
                        self.parameter(2, frame)
                    } else {
                        self.parameter(3, frame)
                    };
                    self.envelope +=
                        (1. - (-1. / (ms / 1000. * 48000.)).exp()) * (level - self.envelope);
                    let over = 20. * self.envelope.max(0.000001).log10() - self.parameter(0, frame);
                    let slope = 1. / self.parameter(1, frame) - 1.;
                    let knee = self.parameter(4, frame);
                    let reduction = if knee > 0. && over.abs() < knee / 2. {
                        slope * (over + knee / 2.).powi(2) / (2. * knee)
                    } else if over > 0. {
                        slope * over
                    } else {
                        0.
                    };
                    self.reduction_db = -reduction;
                    let gain = 10_f32.powf((reduction + self.parameter(5, frame)) / 20.);
                    let mix = self.parameter(6, frame);
                    for i in 0..2 {
                        out[i] = input[i] * (1. - mix) + input[i] * gain * mix
                    }
                }
                Kind::Chorus => out = self.chorus.as_mut().unwrap().sample(input, &self.values),
                Kind::Dither => {
                    for c in 0..2 {
                        out[c] = crate::dsp::chorus::dither(
                            input[c],
                            frame,
                            c as u64,
                            self.noise_seed,
                            &self.values,
                        );
                    }
                }
                Kind::Gate => {
                    let state = self.gate.as_mut().unwrap();
                    out = state.sample(input, &self.values);
                    self.reduction_db = state.reduction_db;
                }
                Kind::Utility => out = self.utility.as_mut().unwrap().sample(input, &self.values),
                Kind::Equalizer => out = self.eq.as_mut().unwrap().sample(input),
                Kind::Room => {
                    let delay = (self.values[1] / 1000. * 48000.).max(0.);
                    let pos = self.delay_index as f32 - delay;
                    let floor = pos.floor() as i64;
                    let f = pos - floor as f32;
                    let first = floor.rem_euclid(self.delay.len() as i64) as usize;
                    let next = (first + 1) % self.delay.len();
                    let mut delayed = [0.; 2];
                    for c in 0..2 {
                        delayed[c] = self.delay[first][c] * (1. - f) + self.delay[next][c] * f;
                        if delay < 1. {
                            let previous =
                                (self.delay_index + self.delay.len() - 1) % self.delay.len();
                            delayed[c] = input[c] * (1. - delay) + self.delay[previous][c] * delay;
                        }
                        self.delay[self.delay_index][c] = input[c];
                    }
                    self.delay_index = (self.delay_index + 1) % self.delay.len();
                    let room = self.room.as_mut().unwrap();
                    let wet = room.sample(delayed, &self.values);
                    for c in 0..2 {
                        out[c] = (input[c] * (1. - self.values[3]) + wet[c] * self.values[3])
                            * room.output_gain;
                    }
                }
                Kind::Limiter => {
                    let state = self.limiter.as_mut().unwrap();
                    out = state.sample(input);
                    dry = state.last_dry;
                    self.reduction_db = state.reduction_db;
                    self.true_peak = self.true_peak.max(state.output_true_peak);
                }
                Kind::Resonance => {
                    let state = self.resonance.as_mut().unwrap();
                    out = state.sample(input);
                    dry = state.last_dry;
                }
                Kind::GuitarRig => {
                    let state = self.guitar_rig.as_mut().unwrap();
                    out = state.sample(input); dry = state.last_dry;
                }
                Kind::SoundPutty => out = self.putty.as_mut().unwrap().sample(input),
                Kind::VocalEnhancer => {
                    let state = self.vocal.as_mut().unwrap();
                    out = state.sample(input);
                    dry = state.last_dry;
                    self.reduction_db = state.reduction_db;
                }
                Kind::SourceIsolator => {
                    let state = self.isolation.as_mut().unwrap();
                    out = state.sample(input);
                    dry = state.last_dry;
                    self.reduction_db = state.reduction_db;
                }
                Kind::Warmth => {
                    let state = self.warmth.as_mut().unwrap();
                    out = state.sample(input, &self.values);
                    dry = state.last_dry;
                }
                Kind::PitchCorrection => {
                    let mut controls = crate::pitch::Controls::from_values(&self.values);
                    let only = self.effect.pitch_edits_only;
                    if let Some((note, blend)) = self.pitch_regions.sample(frame) {
                        controls.manual_target = note;
                        if only {
                            controls.mix *= blend;
                        }
                    } else if only {
                        controls.mix = 0.;
                    }
                    let state = self.pitch.as_mut().unwrap();
                    out = state.sample(input, controls);
                    dry = state.last_dry;
                }
                _ => {}
            }
        }
        let source = timeline_frame(frame - self.latency() as i64, loop_range);
        let baked = source >= 0 && {
            let index = self.baked.partition_point(|r| r[0] <= source as u64);
            index > 0 && (source as u64) < self.baked[index - 1][1]
        };
        let blend = if self.bypass_ramp.is_some() || self.effect.enabled {
            self.bypass_blend
        } else {
            0.
        };
        if blend <= 0. || baked {
            dry
        } else if blend >= 1. {
            out
        } else {
            [
                dry[0] + (out[0] - dry[0]) * blend,
                dry[1] + (out[1] - dry[1]) * blend,
            ]
        }
    }
}
#[derive(Clone, Copy)]
struct PlaybackTrack {
    gain: f32,
    muted: bool,
    solo: bool,
}
struct TrackRenderer {
    id: Arc<str>,
    track: PlaybackTrack,
    envelopes: Arc<[Envelope]>,
    processors: Vec<Processor>,
    volume: Lane,
    pan: Lane,
    latency: usize,
    active_envelopes: Vec<usize>,
    next_envelope: usize,
    last_read_start: u64,
    loop_reset: Option<(Vec<usize>, usize)>,
    volume_write: Option<Override>,
    pan_write: Option<Override>,
    /// Live-edit glides of unautomated gain and (normalized) pan.
    gain_ramp: Option<Ramp>,
    pan_ramp: Option<Ramp>,
    /// 1 while audible, 0 while muted or soloed out; mute and solo edits
    /// glide between them over `LIVE_RAMP` frames instead of clicking.
    level: f32,
}
impl Clone for TrackRenderer {
    fn clone(&self) -> Self {
        let mut active_envelopes = Vec::with_capacity(self.envelopes.len());
        active_envelopes.extend_from_slice(&self.active_envelopes);
        Self {
            id: Arc::clone(&self.id),
            track: self.track,
            envelopes: self.envelopes.clone(),
            processors: self.processors.clone(),
            volume: self.volume.clone(),
            pan: self.pan.clone(),
            latency: self.latency,
            active_envelopes,
            next_envelope: self.next_envelope,
            last_read_start: self.last_read_start,
            loop_reset: self.loop_reset.clone(),
            volume_write: self.volume_write,
            pan_write: self.pan_write,
            gain_ramp: self.gain_ramp,
            pan_ramp: self.pan_ramp,
            level: self.level,
        }
    }
}
impl TrackRenderer {
    fn new(t: &Track) -> Result<Self> {
        let processors = t
            .realtime_effects
            .iter()
            .map(Processor::new)
            .collect::<Result<Vec<_>>>()?;
        let latency = processors.iter().map(Processor::latency).sum();
        Ok(Self {
            id: t.id.clone().into(),
            track: PlaybackTrack {
                gain: t.gain,
                muted: t.muted,
                solo: t.solo,
            },
            envelopes: Envelope::resolve_playback(t).into(),
            processors,
            volume: volume_lane(t),
            pan: pan_lane(t),
            latency,
            active_envelopes: Vec::with_capacity(t.clips.len()),
            next_envelope: 0,
            last_read_start: 0,
            loop_reset: None,
            volume_write: None,
            pan_write: None,
            gain_ramp: None,
            pan_ramp: None,
            level: if t.muted { 0. } else { 1. },
        })
    }
    fn mix_gain(&mut self, frame: u64) -> f32 {
        let original = if self.volume.has_points() {
            self.volume.value(frame) * 1.5
        } else {
            Ramp::advance(&mut self.gain_ramp).unwrap_or(self.track.gain)
        };
        self.volume_write
            .as_mut()
            .map_or(original, |state| state.sample(frame, original / 1.5) * 1.5)
    }
    fn mix_pan(&mut self, frame: u64) -> f32 {
        let original = if self.pan.has_points() {
            self.pan.value(frame)
        } else {
            Ramp::advance(&mut self.pan_ramp).unwrap_or_else(|| self.pan.value(frame))
        };
        self.pan_write
            .as_mut()
            .map_or(original, |state| state.sample(frame, original))
            * 2.
            - 1.
    }
    fn prepare_loop(&mut self, range: LoopRange) {
        let next = self
            .envelopes
            .partition_point(|e| e.clip.start < range.start);
        let active = (0..next)
            .filter(|&i| self.envelopes[i].clip.start + self.envelopes[i].clip.length > range.start)
            .collect();
        self.loop_reset = Some((active, next));
    }
    /// Runs one block of this track's source audio (in `left`/`right`)
    /// through its inserts and fader, leaving the post-fader signal in place.
    fn audible(&self, solo: bool) -> bool {
        !self.track.muted && (!solo || self.track.solo)
    }
    /// Moves `level` one frame toward audible (1) or silent (0).
    fn step_level(&mut self, audible: bool) -> f32 {
        let step = 1. / LIVE_RAMP as f32;
        self.level = if audible {
            (self.level + step).min(1.)
        } else {
            (self.level - step).max(0.)
        };
        self.level
    }
    fn mix_block(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        position: u64,
        loop_range: Option<LoopRange>,
        audible: bool,
    ) {
        // A settled dry strip is two constant multiplies per channel. Keep
        // automated lanes, Write, glides and insert history on the sample path.
        // Preserving the multiplication order retains the existing PCM bits.
        if self.processors.is_empty() && !self.volume.has_points() && !self.pan.has_points()
            && self.volume_write.is_none() && self.pan_write.is_none()
            && self.gain_ramp.is_none() && self.pan_ramp.is_none()
            && self.level == if audible { 1. } else { 0. } {
            if !audible { left.fill(0.); right.fill(0.); return; }
            let pan = self.pan.value(0) * 2. - 1.;
            let l = if pan > 0. { 1. - pan } else { 1. };
            let r = if pan < 0. { 1. + pan } else { 1. };
            for value in left { *value = *value * self.track.gain * l; }
            for value in right { *value = *value * self.track.gain * r; }
            return;
        }
        let source_start = position + self.latency as u64;
        for i in 0..left.len() {
            let x = self.chain(
                [left[i], right[i]],
                (source_start + i as u64) as i64,
                loop_range,
            );
            let frame = timeline_frame((position + i as u64) as i64, loop_range) as u64;
            let level = self.step_level(audible);
            let sounding = level > 0.;
            let gain = if sounding || self.volume_write.is_some() {
                self.mix_gain(frame)
            } else {
                0.
            };
            let pan = if sounding || self.pan_write.is_some() {
                self.mix_pan(frame)
            } else {
                0.
            };
            let (l, r) = (
                x[0] * gain * if pan > 0. { 1. - pan } else { 1. },
                x[1] * gain * if pan < 0. { 1. + pan } else { 1. },
            );
            // Fully audible stays exact; a glide scales the signal.
            (left[i], right[i]) = if level >= 1. {
                (l, r)
            } else {
                (l * level, r * level)
            };
        }
    }
    fn chain(&mut self, mut x: [f32; 2], source: i64, loop_range: Option<LoopRange>) -> [f32; 2] {
        let mut frame = source;
        for p in &mut self.processors {
            let input = x;
            x = p.sample(x, timeline_frame(frame, loop_range), loop_range);
            if p.meter.active() {
                if p.meter.due() {
                    let extras = p.meter_extras();
                    p.meter.extras(extras);
                }
                p.meter.sample(input, x, p.reduction_db);
            }
            frame -= p.latency() as i64;
        }
        x
    }
}
enum Source {
    Resident(Arc<[Vec<f32>; 2]>),
    Paged(crate::paged::PagedSource),
}
impl Source {
    fn read_stereo(
        &mut self,
        start: u64,
        left: &mut [f32],
        right: &mut [f32],
        worker: bool,
    ) -> Result<()> {
        match self {
            Self::Paged(source) if worker => source.read_worker(start, left, right),
            Self::Paged(source) => {
                if source.read_callback(start, left, right) {
                    Ok(())
                } else {
                    Err(Error::Io(std::io::Error::from(
                        std::io::ErrorKind::WouldBlock,
                    )))
                }
            }
            Self::Resident(audio) => {
                let start = start as usize;
                let end = start + left.len();
                if end > audio[0].len() || left.len() != right.len() {
                    return Err(Error::Io(std::io::Error::from(
                        std::io::ErrorKind::InvalidData,
                    )));
                }
                left.copy_from_slice(&audio[0][start..end]);
                right.copy_from_slice(&audio[1][start..end]);
                Ok(())
            }
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn read_track(
    t: &mut TrackRenderer,
    readers: &mut BTreeMap<String, Source>,
    start: u64,
    left: &mut [f32],
    right: &mut [f32],
    source_left: &mut [f32],
    source_right: &mut [f32],
    worker: bool,
    underruns: &mut u64,
) -> Result<()> {
    left.fill(0.);
    right.fill(0.);
    if start < t.last_read_start {
        t.active_envelopes.clear();
        if let Some((active, next)) = &t.loop_reset {
            t.active_envelopes.extend_from_slice(active);
            t.next_envelope = *next;
        } else {
            t.next_envelope = 0;
        }
    }
    t.last_read_start = start;
    t.active_envelopes
        .retain(|index| t.envelopes[*index].clip.start + t.envelopes[*index].clip.length > start);
    while t.next_envelope < t.envelopes.len()
        && t.envelopes[t.next_envelope].clip.start < start + left.len() as u64
    {
        let index = t.next_envelope;
        t.next_envelope += 1;
        if t.envelopes[index].clip.start + t.envelopes[index].clip.length > start {
            t.active_envelopes.push(index);
        }
    }
    for index in &t.active_envelopes {
        let env = &t.envelopes[*index];
        let c = &env.clip;
        let lo = start.max(c.start);
        let hi = (start + left.len() as u64).min(c.start + c.length);
        if hi <= lo {
            continue;
        }
        let n = (hi - lo) as usize;
        let read = readers.get_mut(&env.source_id).unwrap().read_stereo(
            c.offset + lo - c.start,
            &mut source_left[..n],
            &mut source_right[..n],
            worker,
        );
        if let Err(error) = read {
            if !worker && matches!(&error,Error::Io(e) if e.kind()==std::io::ErrorKind::WouldBlock)
            {
                source_left[..n].fill(0.);
                source_right[..n].fill(0.);
                *underruns = underruns.saturating_add(1);
            } else {
                return Err(error);
            }
        }
        for i in 0..n {
            let gain = env.gain(lo + i as u64 - c.start);
            let destination = (lo - start) as usize + i;
            left[destination] += source_left[i] * gain;
            right[destination] += source_right[i] * gain;
        }
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)]
fn read_track_loop(
    t: &mut TrackRenderer,
    readers: &mut BTreeMap<String, Source>,
    start: u64,
    left: &mut [f32],
    right: &mut [f32],
    source_left: &mut [f32],
    source_right: &mut [f32],
    worker: bool,
    underruns: &mut u64,
    loop_range: Option<LoopRange>,
) -> Result<()> {
    let mut offset = 0;
    while offset < left.len() {
        let frame = timeline_frame((start + offset as u64) as i64, loop_range) as u64;
        let count = loop_range.map_or(left.len() - offset, |r| {
            (left.len() - offset).min((r.end - frame) as usize)
        });
        read_track(
            t,
            readers,
            frame,
            &mut left[offset..offset + count],
            &mut right[offset..offset + count],
            source_left,
            source_right,
            worker,
            underruns,
        )?;
        offset += count;
    }
    Ok(())
}
/// Worker-only sequential renderer. Track and master lookahead are aligned on the source timeline.
#[derive(Clone)]
struct RoutingDelay {
    samples: Vec<[f32; 2]>,
    index: usize,
}
impl RoutingDelay {
    fn new(frames: usize) -> Self {
        Self {
            samples: vec![[0.; 2]; frames],
            index: 0,
        }
    }
    fn sample(&mut self, mut x: [f32; 2]) -> [f32; 2] {
        if !self.samples.is_empty() {
            std::mem::swap(&mut x, &mut self.samples[self.index]);
            self.index = (self.index + 1) % self.samples.len();
        }
        x
    }
}
#[derive(Clone)]
struct RoutingEdge {
    from: usize,
    to: usize,
    gain: f32,
    gain_ramp: Option<Ramp>,
    processor: Option<usize>,
    delay: RoutingDelay,
}
#[derive(Clone)]
struct RoutedState {
    plan: crate::routing::Plan,
    edges: Vec<RoutingEdge>,
    outgoing: Vec<Vec<usize>>,
    inputs: Vec<[Vec<f32>; 2]>,
    input_latency: Vec<usize>,
    output_latency: Vec<usize>,
    local_delays: Vec<RoutingDelay>,
    mixed: Vec<[f32; 2]>,
    detectors: Vec<Vec<f32>>,
    audible: Vec<bool>,
    bus: Vec<bool>,
    has_detector: Vec<bool>,
    key_needed: Vec<bool>,
    latency: usize,
}
impl RoutedState {
    fn new(
        plan: crate::routing::Plan,
        tracks: &[TrackRenderer],
        session: &Session,
    ) -> Result<Self> {
        let n = tracks.len();
        let input_latency = plan.input_latency.clone();
        let output_latency = plan.output_latency.clone();
        let mut edges = vec![];
        let mut outgoing = vec![vec![]; n];
        for e in &plan.edges {
            let delay = input_latency[e.to] - output_latency[e.from];
            if e.detector {
                let mut prefix = 0;
                for (p, processor) in tracks[e.to].processors.iter().enumerate() {
                    if processor.kind == Kind::Compressor {
                        outgoing[e.from].push(edges.len());
                        edges.push(RoutingEdge {
                            from: e.from,
                            to: e.to,
                            gain: 1.,
                            gain_ramp: None,
                            processor: Some(p),
                            delay: RoutingDelay::new(delay + prefix),
                        });
                    }
                    prefix += processor.latency();
                }
            } else {
                outgoing[e.from].push(edges.len());
                edges.push(RoutingEdge {
                    from: e.from,
                    to: e.to,
                    gain: e.gain,
                    gain_ramp: None,
                    processor: None,
                    delay: RoutingDelay::new(delay),
                });
            }
        }
        let frames = edges.iter().map(|e| e.delay.samples.len()).sum::<usize>()
            + input_latency[..n].iter().sum::<usize>();
        if frames > 2_000_000 {
            return Err(Error::Invalid(
                "Routing delay memory budget exceeded".into(),
            ));
        }
        let mut key_needed = vec![false; n + 1];
        for e in &plan.edges {
            if e.detector {
                key_needed[e.from] = true;
            }
        }
        for &i in plan.order.iter().rev() {
            if key_needed[i] {
                for e in plan.edges.iter().filter(|e| e.to == i) {
                    key_needed[e.from] = true;
                }
            }
        }
        Ok(Self {
            key_needed,
            latency: input_latency[n],
            local_delays: input_latency[..n]
                .iter()
                .map(|&v| RoutingDelay::new(v))
                .collect(),
            inputs: (0..n).map(|_| [vec![0.; BLOCK], vec![0.; BLOCK]]).collect(),
            mixed: vec![[0.; 2]; n + 1],
            detectors: tracks
                .iter()
                .map(|t| vec![0.; t.processors.len()])
                .collect(),
            audible: vec![true; n + 1],
            bus: session
                .tracks
                .iter()
                .map(|t| t.routing.as_ref().is_some_and(|r| r.role != "audio"))
                .chain(std::iter::once(true))
                .collect(),
            has_detector: (0..n)
                .map(|i| plan.edges.iter().any(|e| e.to == i && e.detector))
                .collect(),
            plan,
            edges,
            outgoing,
            input_latency,
            output_latency,
        })
    }
    fn render(
        &mut self,
        tracks: &mut [TrackRenderer],
        position: u64,
        left: &mut [f32],
        right: &mut [f32],
        loop_range: Option<LoopRange>,
    ) {
        let n = tracks.len();
        let solo = tracks.iter().any(|t| t.track.solo);
        for (i, t) in tracks.iter().enumerate() {
            self.audible[i] = !solo || t.track.solo;
        }
        self.audible[n] = !solo;
        if solo {
            for &i in self.plan.order.iter().rev() {
                if self.audible[i] && self.bus[i] {
                    for e in self.plan.edges.iter().filter(|e| e.to == i && !e.detector) {
                        self.audible[e.from] = true;
                    }
                }
            }
            for &i in &self.plan.order {
                if self.audible[i] {
                    for e in self
                        .plan
                        .edges
                        .iter()
                        .filter(|e| e.from == i && !e.detector)
                    {
                        self.audible[e.to] = true;
                    }
                }
            }
        }
        for j in 0..left.len() {
            self.mixed.fill([0.; 2]);
            for &i in &self.plan.order {
                if i == n {
                    left[j] = self.mixed[n][0];
                    right[j] = self.mixed[n][1];
                    continue;
                }
                let t = &mut tracks[i];
                let own = self.local_delays[i].sample([self.inputs[i][0][j], self.inputs[i][1][j]]);
                let mut x = [own[0] + self.mixed[i][0], own[1] + self.mixed[i][1]];
                let mut frame = position as i64 + j as i64 - self.input_latency[i] as i64;
                for (p, processor) in t.processors.iter_mut().enumerate() {
                    if self.has_detector[i] && processor.kind == Kind::Compressor {
                        processor.external_detector = Some(self.detectors[i][p]);
                    }
                    let input = x;
                    x = processor.sample(x, timeline_frame(frame, loop_range), loop_range);
                    if processor.meter.active() {
                        if processor.meter.due() {
                            let extras = processor.meter_extras();
                            processor.meter.extras(extras);
                        }
                        processor.meter.sample(input, x, processor.reduction_db);
                    }
                    frame -= processor.latency() as i64;
                }
                let frame = timeline_frame(
                    position as i64 + j as i64 - self.output_latency[i] as i64,
                    loop_range,
                )
                .max(0) as u64;
                let gain = t.mix_gain(frame);
                let level = t.step_level(!t.track.muted && self.audible[i]);
                let pan = t.mix_pan(frame);
                let post = [
                    x[0] * gain * if pan > 0. { 1. - pan } else { 1. },
                    x[1] * gain * if pan < 0. { 1. + pan } else { 1. },
                ];
                let key = if t.track.muted { [0.; 2] } else { post };
                x = [post[0] * level, post[1] * level];
                for &edge in &self.outgoing[i] {
                    let e = &mut self.edges[edge];
                    let y = e.delay.sample(
                        if e.processor.is_some() || self.key_needed[e.to] && !self.audible[e.to] {
                            key
                        } else {
                            x
                        },
                    );
                    if let Some(p) = e.processor {
                        self.detectors[e.to][p] = y[0].abs().max(y[1].abs());
                    } else {
                        let gain = Ramp::advance(&mut e.gain_ramp).unwrap_or(e.gain);
                        self.mixed[e.to][0] += y[0] * gain;
                        self.mixed[e.to][1] += y[1] * gain;
                    }
                }
            }
        }
    }
}

pub struct Renderer {
    routing: Option<RoutedState>,
    tracks: Vec<TrackRenderer>,
    master: TrackRenderer,
    readers: BTreeMap<String, Source>,
    source_left: Vec<f32>,
    source_right: Vec<f32>,
    track_left: Vec<f32>,
    track_right: Vec<f32>,
    position: u64,
    processing_position: u64,
    master_padding: Vec<[f32; 2]>,
    padding_index: usize,
    tail_frames: u64,
    loop_range: Option<LoopRange>,
    source_underruns: u64,
}
impl Renderer {
    /// Attaches live meters to every insert (output-callback graphs only).
    fn metered(mut self) -> Self {
        for track in self
            .tracks
            .iter_mut()
            .chain(std::iter::once(&mut self.master))
        {
            for p in &mut track.processors {
                p.meter.attach(&p.effect.id);
            }
        }
        self
    }
    pub fn new(project: &Project) -> Result<Self> {
        Self::new_cancellable(project, &AtomicBool::new(false))
    }
    pub fn new_cancellable(project: &Project, cancel: &AtomicBool) -> Result<Self> {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Conflict("Render preparation cancelled".into()));
        }
        project.validate_sources()?;
        graph_storage_budget(project, false)?;
        // Offline reads reuse the paged host's 128-handle / 32 MiB decoder
        // pool, instead of opening and buffering every source at construction.
        // The minimum page cache is unused by worker reads and stays bounded.
        let loader = crate::paged::PagedLoader::new(project, 512 * 1024)?;
        let readers = loader.sources().map(|(id, source)| (id, Source::Paged(source))).collect();
        Self::new_with_sources(project, readers, cancel, None)
    }
    fn new_with_sources(
        project: &Project,
        mut readers: BTreeMap<String, Source>,
        cancel: &AtomicBool,
        loop_range: Option<LoopRange>,
    ) -> Result<Self> {
        let plan = crate::routing::Plan::new(&project.session)?;
        if plan.active {
            graph_admission_budget(project)?;
        }
        let origin = loop_range.map_or(0, |r| r.start);
        let mut tracks = project
            .session
            .tracks
            .iter()
            .map(TrackRenderer::new)
            .collect::<Result<Vec<_>>>()?;
        if let Some(range) = loop_range {
            for track in &mut tracks {
                track.prepare_loop(range);
            }
        }
        let audible = plan.audible(&project.session);
        for (i, t) in tracks.iter_mut().enumerate() {
            t.level = if audible[i] && !t.track.muted { 1. } else { 0. };
        }
        let master = TrackRenderer::new(&project.session.master_track)?;
        if master.latency > 4096 {
            return Err(Error::Invalid(
                "Master insert latency exceeds the 4096-frame budget".into(),
            ));
        }
        let lookahead = if master.latency > 0 { 4096 } else { 0 };
        let padding = lookahead - master.latency;
        let mut source_left = vec![0.; BLOCK];
        let mut source_right = vec![0.; BLOCK];
        let mut track_left = vec![0.; BLOCK];
        let mut track_right = vec![0.; BLOCK];
        let mut source_underruns = 0;
        // Prime each insert chain with actual source PCM, exactly the track-owned lookahead.
        for t in &mut tracks {
            let mut start = if plan.active { t.latency } else { 0 };
            while start < t.latency {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Conflict("Render preparation cancelled".into()));
                }
                let n = (t.latency - start).min(BLOCK);
                read_track_loop(
                    t,
                    &mut readers,
                    origin + start as u64,
                    &mut track_left[..n],
                    &mut track_right[..n],
                    &mut source_left,
                    &mut source_right,
                    true,
                    &mut source_underruns,
                    loop_range,
                )?;
                for i in 0..n {
                    t.chain(
                        [track_left[i], track_right[i]],
                        (origin + (start + i) as u64) as i64,
                        loop_range,
                    );
                }
                start += n;
            }
        }
        let track_tails = tracks
            .iter()
            .map(|t| {
                t.processors
                    .iter()
                    .map(Processor::tail_frames)
                    .sum::<u64>()
                    .min(1440000)
            })
            .max()
            .unwrap_or(0);
        let mut tail_frames = (track_tails
            + master
                .processors
                .iter()
                .map(Processor::tail_frames)
                .sum::<u64>())
        .min(1440000);
        if plan.active {
            let mut tails = vec![0; tracks.len() + 1];
            for &i in &plan.order {
                tails[i] += if i < tracks.len() {
                    tracks[i]
                        .processors
                        .iter()
                        .map(Processor::tail_frames)
                        .sum::<u64>()
                } else {
                    master
                        .processors
                        .iter()
                        .map(Processor::tail_frames)
                        .sum::<u64>()
                };
                for e in plan.edges.iter().filter(|e| e.from == i && !e.detector) {
                    tails[e.to] = tails[e.to].max(tails[i]);
                }
            }
            tail_frames = tails[tracks.len()].min(1440000);
        }
        let routing = if plan.active {
            Some(RoutedState::new(plan, &tracks, &project.session)?)
        } else {
            None
        };
        let route_latency = routing.as_ref().map_or(0, |r| r.latency);
        let mut renderer = Self {
            routing,
            tracks,
            master,
            readers,
            source_left,
            source_right,
            track_left,
            track_right,
            position: origin,
            processing_position: origin,
            master_padding: vec![[0.; 2]; padding],
            padding_index: 0,
            tail_frames,
            loop_range,
            source_underruns,
        };
        if route_latency > 0 {
            let mut l = [0.; BLOCK];
            let mut r = [0.; BLOCK];
            let mut remaining = route_latency;
            while remaining > 0 {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Conflict("Render preparation cancelled".into()));
                }
                let count = remaining.min(BLOCK);
                renderer.render_mix(&mut l[..count], &mut r[..count], true)?;
                remaining -= count;
            }
        }
        if lookahead > 0 {
            let mut left = [0.; 4096];
            let mut right = [0.; 4096];
            renderer.render_mix(&mut left, &mut right, true)?;
        }
        Ok(renderer)
    }
    pub fn position(&self) -> u64 {
        self.position
    }
    pub fn tail_frames(&self) -> u64 {
        self.tail_frames
    }
    fn render_mix(&mut self, left: &mut [f32], right: &mut [f32], worker: bool) -> Result<()> {
        let count = left.len();
        left.fill(0.);
        right.fill(0.);
        let solo = self.tracks.iter().any(|t| t.track.solo);
        let route_latency = self.routing.as_ref().map_or(0, |r| r.latency) as u64;
        if let Some(routing) = &mut self.routing {
            for (i, t) in self.tracks.iter_mut().enumerate() {
                let [l, r] = &mut routing.inputs[i];
                read_track_loop(
                    t,
                    &mut self.readers,
                    self.processing_position,
                    &mut l[..count],
                    &mut r[..count],
                    &mut self.source_left,
                    &mut self.source_right,
                    worker,
                    &mut self.source_underruns,
                    self.loop_range,
                )?;
            }
            routing.render(
                &mut self.tracks,
                self.processing_position,
                left,
                right,
                self.loop_range,
            );
        } else if worker && count >= 1024
            && self.tracks.iter().filter(|t| !t.processors.is_empty()).take(2).count() > 1 {
            // Dry tracks have no insert work to distribute. Thread creation,
            // per-track scratch allocation and joining cost more than mixing.
            // Offline/worker renders (seek warm-up, export) run the tracks'
            // insert chains in parallel: tracks are independent until the
            // master bus, and they are summed in the same order, so the
            // result is identical to the sequential path.
            let mut buffers = Vec::with_capacity(self.tracks.len());
            for t in &mut self.tracks {
                let (mut l, mut r) = (vec![0f32; count], vec![0f32; count]);
                read_track_loop(
                    t,
                    &mut self.readers,
                    self.processing_position + t.latency as u64,
                    &mut l,
                    &mut r,
                    &mut self.source_left,
                    &mut self.source_right,
                    worker,
                    &mut self.source_underruns,
                    self.loop_range,
                )?;
                buffers.push([l, r]);
            }
            let (position, loop_range) = (self.processing_position, self.loop_range);
            // Tracks heard at any point in this block (audible, or fading).
            let contributes: Vec<bool> = self
                .tracks
                .iter()
                .map(|t| t.audible(solo) || t.level > 0.)
                .collect();
            let threads = std::thread::available_parallelism()
                .map_or(4, |n| n.get())
                .min(self.tracks.len());
            let per = self.tracks.len().div_ceil(threads);
            std::thread::scope(|scope| {
                for (tracks, buffers) in self.tracks.chunks_mut(per).zip(buffers.chunks_mut(per)) {
                    scope.spawn(move || {
                        for (t, [l, r]) in tracks.iter_mut().zip(buffers) {
                            let audible = t.audible(solo);
                            t.mix_block(l, r, position, loop_range, audible);
                        }
                    });
                }
            });
            for (heard, [l, r]) in contributes.iter().zip(&buffers) {
                if *heard {
                    for i in 0..count {
                        left[i] += l[i];
                        right[i] += r[i];
                    }
                }
            }
        } else {
            for t in &mut self.tracks {
                read_track_loop(
                    t,
                    &mut self.readers,
                    self.processing_position + t.latency as u64,
                    &mut self.track_left[..count],
                    &mut self.track_right[..count],
                    &mut self.source_left,
                    &mut self.source_right,
                    worker,
                    &mut self.source_underruns,
                    self.loop_range,
                )?;
                let audible = t.audible(solo);
                let heard = audible || t.level > 0.;
                t.mix_block(
                    &mut self.track_left[..count],
                    &mut self.track_right[..count],
                    self.processing_position,
                    self.loop_range,
                    audible,
                );
                if heard {
                    for i in 0..count {
                        left[i] += self.track_left[i];
                        right[i] += self.track_right[i];
                    }
                }
            }
        }
        for i in 0..count {
            let frame = timeline_frame(
                (self.processing_position + i as u64) as i64 - route_latency as i64,
                self.loop_range,
            );
            let gain = self.master.mix_gain(frame.max(0) as u64);
            let mut x =
                self.master
                    .chain([left[i] * gain, right[i] * gain], frame, self.loop_range);
            if !self.master_padding.is_empty() {
                std::mem::swap(&mut x, &mut self.master_padding[self.padding_index]);
                self.padding_index = (self.padding_index + 1) % self.master_padding.len();
            }
            let audible = !self.master.track.muted;
            let level = self.master.step_level(audible);
            if level <= 0. {
                x = [0.; 2]
            } else if level < 1. {
                x = [x[0] * level, x[1] * level]
            }
            left[i] = x[0];
            right[i] = x[1];
        }
        self.processing_position += count as u64;
        Ok(())
    }
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) -> Result<()> {
        let count = left.len();
        if count != right.len()
            || count > BLOCK
            || self.processing_position
                - (self.master.latency
                    + self.master_padding.len()
                    + self.routing.as_ref().map_or(0, |r| r.latency)) as u64
                > crate::model::MAX_FRAME - count as u64
        {
            return Err(Error::Invalid("Invalid render block".into()));
        }
        self.render_mix(left, right, true)?;
        self.position =
            timeline_frame((self.position + count as u64) as i64, self.loop_range) as u64;
        Ok(())
    }
    /// Worker-only seek replays DSP history; recreation is required for backward seeks.
    pub fn advance_to(&mut self, frame: u64, cancel: &AtomicBool) -> Result<()> {
        if frame < self.position || frame > crate::model::MAX_FRAME {
            return Err(Error::Invalid(
                "Renderer only advances; recreate for a backward seek".into(),
            ));
        }
        let mut l = [0.; BLOCK];
        let mut r = [0.; BLOCK];
        while self.position < frame {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Conflict("Render cancelled".into()));
            }
            let n = (frame - self.position).min(BLOCK as u64) as usize;
            self.render(&mut l[..n], &mut r[..n])?;
        }
        Ok(())
    }
}
pub fn export(
    project: &Project,
    target: &Path,
    start: u64,
    end: u64,
    cancel: &AtomicBool,
    progress: impl FnMut(u64, u64),
) -> Result<()> {
    export_with_tails(project, target, start, end, false, cancel, progress)
}
#[allow(clippy::too_many_arguments)]
pub fn export_with_tails(
    project: &Project,
    target: &Path,
    start: u64,
    end: u64,
    include_tails: bool,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<()> {
    if end <= start || end > crate::model::MAX_FRAME {
        return Err(Error::Invalid("Invalid export range".into()));
    }
    let mut renderer = Renderer::new_cancellable(project, cancel)?;
    let end = if include_tails {
        end.checked_add(renderer.tail_frames())
            .filter(|v| *v <= crate::model::MAX_FRAME)
            .ok_or_else(|| Error::Invalid("Tail exceeds timeline bound".into()))?
    } else {
        end
    };
    renderer.advance_to(start, cancel)?;
    let mut writer = WaveWriter::create(target, end - start)?;
    let result = (|| {
        let mut left = [0.; BLOCK];
        let mut right = [0.; BLOCK];
        while renderer.position < end {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Conflict("Render cancelled".into()));
            }
            let n = (end - renderer.position).min(BLOCK as u64) as usize;
            renderer.render(&mut left[..n], &mut right[..n])?;
            writer.write(&left[..n], &right[..n])?;
            progress(renderer.position - start, end - start);
        }
        writer.finish()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(target);
    }
    result
}

/// Callback result is fixed-size and never formats/allocates error text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallbackStatus {
    Rendered,
    InvalidBlock,
    SourceFault,
    /// Graph clock/DSP continue with silence only for missing source spans.
    SourceUnderrun,
}
/// A graph admitted with worker-decoded immutable PCM and prepared effect storage.
/// File decoding, construction, forks, seeks and snapshots are worker operations.
/// Resident and worker-paged source admission share the same callback/DSP path.
pub struct PreparedGraph {
    renderer: Renderer,
    generation: u64,
    handoff_token: u64,
    handoff_boundary: Option<u64>,
    handoff_baseline: u64,
    output_boundary: u64,
    state_bytes: usize,
    immutable_bytes: usize,
    source_bytes: usize,
    live_write_history: bool,
}
#[derive(Clone)]
pub struct DspCheckpoint {
    routing: Option<RoutedState>,
    generation: u64,
    tracks: Vec<TrackRenderer>,
    master: TrackRenderer,
    position: u64,
    processing_position: u64,
    padding: Vec<[f32; 2]>,
    padding_index: usize,
    source_underruns: u64,
    playback_clock: u64,
    live_write_history: bool,
    bytes: usize,
}
impl DspCheckpoint {
    pub fn position(&self) -> u64 {
        self.position
    }
    pub fn playback_clock(&self) -> u64 {
        self.playback_clock
    }
    pub fn storage_budget(&self) -> usize {
        self.bytes
    }
}
/// Admission measures the prepared representation, not serialized authored JSON.
/// Immutable automation/envelopes/config are shared by all forks/checkpoints;
/// mutable processor histories, indices and scratch remain separately bounded.
fn graph_admission_budget(project: &Project) -> Result<(usize, usize)> {
    graph_storage_budget(project, true)
}
fn graph_storage_budget(project: &Project, prepared: bool) -> Result<(usize, usize)> {
    const LIMIT: usize = 128 * 1024 * 1024;
    fn add(total: &mut usize, bytes: usize) -> Result<()> {
        *total = total
            .checked_add(bytes)
            .filter(|v| *v <= LIMIT)
            .ok_or_else(|| {
                Error::Invalid(
                    "Prepared DSP and immutable playback data exceed 128 MiB budget".into(),
                )
            })?;
        Ok(())
    }
    let mut mutable = BLOCK * 16 + 4096 * 8 + 4096;
    let mut immutable = 0;
    let routed = project.session.tracks.iter().any(|t| t.routing.is_some());
    if prepared && project.session.tracks.len() > if routed { 129 } else { 128 } {
        return Err(Error::Invalid(
            if routed {
                "Prepared routed graph exceeds 129-track admission bound"
            } else {
                "Prepared graph exceeds 128-track admission bound"
            }
            .into(),
        ));
    }
    for track in project
        .session
        .tracks
        .iter()
        .chain(std::iter::once(&project.session.master_track))
    {
        if prepared && (track.clips.len() > 8192 || track.realtime_effects.len() > 32) {
            return Err(Error::Invalid(
                "Track exceeds prepared clip/insert admission bound".into(),
            ));
        }
        add(&mut mutable, track.clips.len() * 16 + 65536)?;
        add(
            &mut immutable,
            16 + track.id.len() + 16 + track.clips.len() * std::mem::size_of::<Envelope>(),
        )?;
        for clip in &track.clips {
            add(&mut immutable, clip.asset_id.len() * 2)?;
            if let Some(frozen) = &clip.render_envelope {
                add(
                    &mut immutable,
                    frozen.additional_boundaries.len()
                        * std::mem::size_of::<crate::model::Boundary>(),
                )?;
            }
        }
        // Only volume and pan have playback meaning. Unknown authored lanes are
        // preserved in projects but never copied into callback/checkpoint storage.
        for key in ["volume", "pan"] {
            let count = track.mix_automation.get(key).map_or(0, Vec::len);
            add(&mut immutable, Lane::point_storage_bound(count))?;
        }
        for effect in &track.realtime_effects {
            let budget = match effect.kind.as_str() {
                "pitchCorrection" => 3 * 1024 * 1024,
                "limiter" => 2 * 1024 * 1024,
                "resonance" | "room" | "echo" | "sourceIsolator" => 1024 * 1024,
                "guitarRig" => 1024 * 1024,
                "soundPutty" => 64 * 1024,
                "utility" => 4801 * 8 + 512,
                "vocalEnhancer" => 128 * 1024,
                _ => 65536,
            };
            add(&mut mutable, budget)?;
            let fields = descriptors(&effect.kind);
            add(
                &mut immutable,
                std::mem::size_of::<PlaybackEffect>() + effect.kind.len() + effect.id.len() + 16,
            )?;
            add(
                &mut immutable,
                fields.len()
                    * (std::mem::size_of::<Descriptor>() + std::mem::size_of::<Lane>() + 4)
                    + 32,
            )?;
            add(
                &mut immutable,
                Lane::point_storage_bound(effect.automation.len()),
            )?;
            for field in &fields {
                add(
                    &mut immutable,
                    Lane::point_storage_bound(
                        effect
                            .parameter_automation
                            .get(field.key)
                            .map_or(0, Vec::len),
                    ),
                )?;
            }
            let baked = effect
                .extra
                .get("bakedRanges")
                .and_then(|v| v.as_array())
                .map_or(0, Vec::len);
            add(&mut immutable, 16 + baked * 16)?;
            add(
                &mut immutable,
                crate::pitch::Regions::prepared_storage_bound(effect)?,
            )?;
        }
    }
    if crate::routing::Plan::new(&project.session)?.active {
        // Includes every node's block buffers, local/edge delay rings and keys.
        // Buffers cost 64 KiB per track; ring lengths are measured before allocation.
        let plan = crate::routing::Plan::new(&project.session)?;
        add(
            &mut mutable,
            plan.delay_frames * 8 + project.session.tracks.len() * (BLOCK * 8 + 1024),
        )?;
    }
    let total = mutable
        .checked_add(immutable)
        .ok_or_else(|| Error::Invalid("Prepared graph storage overflow".into()))?;
    if total > LIMIT {
        return Err(Error::Invalid(
            "Prepared DSP and immutable playback data exceed 128 MiB budget".into(),
        ));
    }
    Ok((mutable, immutable))
}
static GRAPH_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
fn volume_lane(t: &Track) -> Lane {
    Lane::new(
        t.mix_automation
            .get("volume")
            .map(Vec::as_slice)
            .unwrap_or(&[]),
        t.gain / 1.5,
        t.volume_automation_scale.as_deref() == Some("decibels"),
    )
}
fn pan_lane(t: &Track) -> Lane {
    Lane::new(
        t.mix_automation
            .get("pan")
            .map(Vec::as_slice)
            .unwrap_or(&[]),
        (t.pan + 1.) / 2.,
        false,
    )
}
/// Track index of the master bus in a [`LivePatch`].
const MASTER: usize = usize::MAX;
struct ProcessorParts {
    track: usize,
    index: usize,
    effect: Arc<PlaybackEffect>,
    amount: Lane,
    lanes: Arc<[Lane]>,
    baked: Arc<[[u64; 2]]>,
    pitch_regions: Arc<crate::pitch::Regions>,
}
struct TrackParts {
    track: usize,
    id: String,
    playback: PlaybackTrack,
    volume: Lane,
    pan: Lane,
}
/// Parameter-only edits applied to a running graph instead of rebuilding it,
/// so insert state (delay lines, pitch epochs, envelopes) carries on as the
/// in-process engine's did. Built off the callback; applying only swaps, and
/// the replaced values travel back in the patch to be released off it.
pub struct LivePatch {
    sends: Vec<(usize, usize, f32)>,
    processors: Vec<ProcessorParts>,
    tracks: Vec<TrackParts>,
}
/// The session with every live-patchable value blanked: two sessions with the
/// same structure differ only in insert parameters/automation, pitch edits,
/// bypass, and track gain/pan.
fn structure(session: &Session) -> Session {
    let mut s = session.clone();
    for t in s
        .tracks
        .iter_mut()
        .chain(std::iter::once(&mut s.master_track))
    {
        if let Some(r) = &mut t.routing {
            for send in &mut r.sends {
                send.level = 0.;
            }
        }
        t.name.clear();
        t.color = 0;
        t.armed = false;
        t.gain = 0.;
        t.pan = 0.;
        // Mute and solo glide on the running graph (no rebuild, no seek).
        t.muted = false;
        t.solo = false;
        t.mix_automation.remove("volume");
        t.mix_automation.remove("pan");
        for e in &mut t.realtime_effects {
            // Emptiness selects legacy recipes, so it stays structural.
            let (parameters, automation) =
                (e.parameters.is_empty(), e.parameter_automation.is_empty());
            e.parameters.clear();
            e.parameter_automation.clear();
            if !parameters {
                e.parameters.insert(String::new(), 0.);
            }
            if !automation {
                e.parameter_automation.insert(String::new(), vec![]);
            }
            e.amount = 0.;
            e.enabled = true;
            e.automation.clear();
            for key in ["pitchEdits", "pitchEditsOnly", "bakedRanges"] {
                e.extra.remove(key);
            }
        }
    }
    s
}
impl LivePatch {
    /// The patch turning a graph prepared from `old` into one for `new`, or
    /// None when the change is structural (clips, inserts, routing, timing).
    pub fn between(old: &Session, new: &Session) -> Option<Self> {
        if old.tracks.len() != new.tracks.len() || structure(old) != structure(new) {
            return None;
        }
        let mut patch = Self {
            sends: vec![],
            processors: vec![],
            tracks: vec![],
        };
        for (i, (a, b)) in old.tracks.iter().zip(&new.tracks).enumerate() {
            if let (Some(a), Some(b)) = (&a.routing, &b.routing) {
                for (a, b) in a.sends.iter().zip(&b.sends) {
                    if a.level != b.level {
                        let to = new
                            .tracks
                            .iter()
                            .position(|t| t.id.eq_ignore_ascii_case(&b.track_id))?;
                        patch.sends.push((i, to, b.level));
                    }
                }
            }
        }
        let pairs = old
            .tracks
            .iter()
            .zip(&new.tracks)
            .enumerate()
            .chain(std::iter::once((
                MASTER,
                (&old.master_track, &new.master_track),
            )));
        for (track, (a, b)) in pairs {
            if a.gain != b.gain
                || a.pan != b.pan
                || a.muted != b.muted
                || a.solo != b.solo
                || a.mix_automation != b.mix_automation
            {
                patch.tracks.push(TrackParts {
                    track,
                    id: b.id.clone(),
                    playback: PlaybackTrack {
                        gain: b.gain,
                        muted: b.muted,
                        solo: b.solo,
                    },
                    volume: volume_lane(b),
                    pan: pan_lane(b),
                });
            }
            for (index, (x, y)) in a
                .realtime_effects
                .iter()
                .zip(&b.realtime_effects)
                .enumerate()
            {
                if x == y {
                    continue;
                }
                let descriptors = descriptors(&y.kind);
                patch.processors.push(ProcessorParts {
                    track,
                    index,
                    effect: Arc::new(playback_effect(y, &descriptors)),
                    amount: Lane::new(&y.automation, y.amount, false),
                    lanes: effect_lanes(y, &descriptors).into(),
                    baked: baked_ranges(y).ok()?.into(),
                    pitch_regions: Arc::new(crate::pitch::Regions::from_effect(y).ok()?),
                });
            }
        }
        (!patch.processors.is_empty() || !patch.tracks.is_empty() || !patch.sends.is_empty())
            .then_some(patch)
    }
    pub fn is_empty(&self) -> bool {
        self.processors.is_empty() && self.tracks.is_empty() && self.sends.is_empty()
    }
}
impl Renderer {
    fn live_track(&mut self, index: usize) -> Option<&mut TrackRenderer> {
        if index == MASTER {
            Some(&mut self.master)
        } else {
            self.tracks.get_mut(index)
        }
    }
    /// Swaps the patch's values in. Nothing changes unless every target exists
    /// with the same insert identity. No allocation or release happens here.
    fn apply_live(&mut self, patch: &mut LivePatch) -> bool {
        for p in &patch.processors {
            let matches = self.live_track(p.track).is_some_and(|t| {
                t.processors.get(p.index).is_some_and(|q| {
                    q.effect.id == p.effect.id
                        && q.effect.kind == p.effect.kind
                        && q.lanes.len() == p.lanes.len()
                })
            });
            if !matches {
                return false;
            }
        }
        if patch.tracks.iter().any(|t| {
            self.live_track(t.track)
                .map_or(true, |track| !track.id.eq_ignore_ascii_case(&t.id))
        }) {
            return false;
        }
        if patch.sends.iter().any(|(from, to, _)| {
            self.routing.as_ref().map_or(true, |g| {
                !g.edges
                    .iter()
                    .any(|e| e.from == *from && e.to == *to && e.processor.is_none())
            })
        }) {
            return false;
        }
        if let Some(g) = &mut self.routing {
            for (from, to, gain) in &mut patch.sends {
                let edge = g
                    .edges
                    .iter_mut()
                    .find(|e| e.from == *from && e.to == *to && e.processor.is_none())
                    .unwrap();
                edge.gain_ramp =
                    Ramp::between(edge.gain_ramp.map_or(edge.gain, |r| r.current), *gain);
                std::mem::swap(&mut edge.gain, gain);
            }
        }
        for p in &mut patch.processors {
            let q = &mut self.live_track(p.track).unwrap().processors[p.index];
            q.begin_ramps(p);
            std::mem::swap(&mut q.effect, &mut p.effect);
            std::mem::swap(&mut q.amount, &mut p.amount);
            std::mem::swap(&mut q.lanes, &mut p.lanes);
            std::mem::swap(&mut q.baked, &mut p.baked);
            std::mem::swap(&mut q.pitch_regions, &mut p.pitch_regions);
            if let (Some(state), Some(shape)) = (&mut q.putty, &q.effect.putty_shape) {
                state.set_shape(shape);
            }
            q.refresh_constants();
        }
        for t in &mut patch.tracks {
            let track = self.live_track(t.track).unwrap();
            track.gain_ramp = if track.volume.has_points() || t.volume.has_points() {
                None
            } else {
                Ramp::between(
                    track.gain_ramp.map_or(track.track.gain, |r| r.current),
                    t.playback.gain,
                )
            };
            track.pan_ramp = if track.pan.has_points() || t.pan.has_points() {
                None
            } else {
                Ramp::between(
                    track.pan_ramp.map_or(track.pan.value(0), |r| r.current),
                    t.pan.value(0),
                )
            };
            std::mem::swap(&mut track.track, &mut t.playback);
            std::mem::swap(&mut track.volume, &mut t.volume);
            std::mem::swap(&mut track.pan, &mut t.pan);
        }
        true
    }
}
impl PreparedGraph {
    /// Applies parameter-only edits in place (see [`LivePatch`]).
    pub fn apply_live(&mut self, patch: &mut LivePatch) -> bool {
        self.renderer.apply_live(patch)
    }
    /// Worker-only resolution; no UUID/string lookup occurs on a callback.
    pub fn resolve_write_target(&self, target: &WriteTarget) -> Result<ResolvedWriteTarget> {
        if self.loop_range().is_some() {
            return Err(Error::Unsupported(
                "Loop Write migration remains unqualified".into(),
            ));
        }
        let (track_id, effect_id, parameter_id) = match target {
            WriteTarget::Volume { track_id } | WriteTarget::Pan { track_id } => {
                (track_id, None, None)
            }
            WriteTarget::EffectAmount {
                track_id,
                effect_id,
            } => (track_id, Some(effect_id), None),
            WriteTarget::EffectParameter {
                track_id,
                effect_id,
                parameter_id,
            } => (track_id, Some(effect_id), Some(parameter_id)),
        };
        let tracks = self.renderer.tracks.len();
        let index = self
            .renderer
            .tracks
            .iter()
            .chain(std::iter::once(&self.renderer.master))
            .position(|t| t.id.eq_ignore_ascii_case(track_id))
            .ok_or_else(|| {
                Error::Conflict("Write track is absent from the prepared graph".into())
            })?;
        let track = if index == tracks {
            &self.renderer.master
        } else {
            &self.renderer.tracks[index]
        };
        let master_latency = (self.renderer.master.latency
            + self.renderer.master_padding.len()
            + self.renderer.routing.as_ref().map_or(0, |g| g.latency))
            as u64;
        let mut latency = if let Some(g) = &self.renderer.routing {
            if index < tracks {
                master_latency - g.output_latency[index] as u64
            } else {
                master_latency - g.latency as u64
            }
        } else {
            master_latency
        };
        let route = if let Some(id) = effect_id {
            let effect = track
                .processors
                .iter()
                .position(|p| p.effect.id.eq_ignore_ascii_case(id))
                .ok_or_else(|| {
                    Error::Conflict("Write insert is absent from prepared graph".into())
                })?;
            let p = &track.processors[effect];
            if !p.effect.enabled {
                return Err(Error::Unsupported(
                    "Write of bypassed inserts requires an explicit control policy".into(),
                ));
            }
            let prefix = track.processors[..effect]
                .iter()
                .map(Processor::latency)
                .sum::<usize>() as u64;
            latency = if let Some(g) = &self.renderer.routing {
                master_latency
                    - if index < tracks {
                        g.input_latency[index] as u64
                    } else {
                        g.latency as u64
                    }
                    - prefix
            } else if index == tracks {
                master_latency - prefix
            } else {
                master_latency + track.latency as u64 - prefix
            };
            if let Some(key) = parameter_id {
                let parameter =
                    p.descriptors
                        .iter()
                        .position(|d| d.key == key)
                        .ok_or_else(|| {
                            Error::Unsupported("Unknown continuous Write parameter".into())
                        })?;
                if p.legacy
                    || p.descriptors[parameter].integer
                    || key == "manualTarget"
                    || p.has_recipe_parameter(parameter)
                {
                    return Err(Error::Unsupported(
                        "This target needs discrete/legacy recipe migration before Write".into(),
                    ));
                }
                Route::Parameter {
                    effect: effect as u8,
                    parameter: parameter as u8,
                }
            } else {
                let amount_drives = p.legacy
                    || p.effect.parameters_empty
                        && p.descriptors.iter().enumerate().any(|(i, d)| {
                            !p.lanes[i].has_points()
                                && matches!(
                                    (p.effect.kind.as_str(), d.key),
                                    ("gain", "gain")
                                        | ("lowPass", "frequency")
                                        | ("echo", "mix")
                                        | ("warmth", "drive")
                                )
                        });
                if !amount_drives {
                    return Err(Error::Unsupported(
                        "This insert uses named parameters; amount has no live DSP binding".into(),
                    ));
                }
                Route::Amount {
                    effect: effect as u8,
                }
            }
        } else if matches!(target, WriteTarget::Volume { .. }) {
            if !track.volume.has_points() && track.track.gain > 1.5 {
                return Err(Error::Unsupported(
                    "Unautomated gain above 1.5 needs an extended automation scale before Write"
                        .into(),
                ));
            }
            Route::Volume
        } else {
            if index == tracks {
                return Err(Error::Unsupported(
                    "Master pan needs DSP parity before Write".into(),
                ));
            }
            Route::Pan
        };
        if self
            .playback_clock()
            .checked_add(latency + RAMP_FRAMES)
            .map_or(true, |f| f > crate::model::MAX_FRAME)
        {
            return Err(Error::Invalid(
                "Write source coordinate exceeds session frame bound".into(),
            ));
        }
        Ok(ResolvedWriteTarget {
            generation: self.generation,
            track: index as u16,
            route,
            latency,
        })
    }
    fn write_track_mut(&mut self, target: ResolvedWriteTarget) -> Option<&mut TrackRenderer> {
        if target.generation != self.generation {
            return None;
        }
        if target.track as usize == self.renderer.tracks.len() {
            Some(&mut self.renderer.master)
        } else {
            self.renderer.tracks.get_mut(target.track as usize)
        }
    }
    fn write_field(&mut self, target: ResolvedWriteTarget) -> Option<&mut Option<Override>> {
        let track = self.write_track_mut(target)?;
        match target.route {
            Route::Volume => Some(&mut track.volume_write),
            Route::Pan => Some(&mut track.pan_write),
            Route::Amount { effect } => {
                Some(&mut track.processors.get_mut(effect as usize)?.amount_write)
            }
            Route::Parameter { effect, parameter } => track
                .processors
                .get_mut(effect as usize)?
                .parameter_writes
                .get_mut(parameter as usize),
        }
    }
    fn write_original(&mut self, target: ResolvedWriteTarget, frame: u64) -> Option<(f32, bool)> {
        let t = self.write_track_mut(target)?;
        Some(match target.route {
            Route::Volume => (
                if t.volume.has_points() {
                    t.volume.value(frame)
                } else {
                    t.track.gain / 1.5
                },
                t.volume.decibels(),
            ),
            Route::Pan => (t.pan.value(frame), false),
            Route::Amount { effect } => (
                t.processors.get(effect as usize)?.amount.value(frame),
                false,
            ),
            Route::Parameter { effect, parameter } => {
                let p = t.processors.get_mut(effect as usize)?;
                (p.lanes.get(parameter as usize)?.value(frame), false)
            }
        })
    }
    /// Worker-only metadata bound to this exact prepared generation. Physical
    /// bounds/defaults come from the processor's own descriptors, never the UI.
    pub fn write_mapping(
        &self,
        target: ResolvedWriteTarget,
    ) -> Result<crate::automation_write::WriteMapping> {
        use crate::automation_write::{WriteInterpolation, WriteMapping};
        if target.generation != self.generation {
            return Err(Error::Conflict(
                "Write mapping belongs to another prepared generation".into(),
            ));
        }
        let t = if target.track as usize == self.renderer.tracks.len() {
            &self.renderer.master
        } else {
            self.renderer
                .tracks
                .get(target.track as usize)
                .ok_or_else(|| Error::Conflict("Write mapping track is absent".into()))?
        };
        let (descriptor, baseline, unit, decibels) = match target.route {
            Route::Volume => (
                descriptor("volume", 0., 1.5, 1., false),
                t.volume.value_signed(-1),
                "gain",
                t.volume.decibels(),
            ),
            Route::Pan => (
                descriptor("pan", -1., 1., 0., false),
                t.pan.value_signed(-1),
                "",
                false,
            ),
            Route::Amount { effect } => {
                let p = &t.processors[effect as usize];
                let default = match p.effect.kind.as_str() {
                    "lowPass" => 0.25,
                    "echo" => 0.4,
                    "warmth" => 0.3,
                    _ => 0.5,
                };
                (
                    descriptor("amount", 0., 1., default, false),
                    p.amount.value_signed(-1),
                    "%",
                    false,
                )
            }
            Route::Parameter { effect, parameter } => {
                let p = &t.processors[effect as usize];
                let d = p.descriptors[parameter as usize];
                (
                    d,
                    p.lanes[parameter as usize].value_signed(-1),
                    parameter_unit(&p.effect.kind, d.key),
                    false,
                )
            }
        };
        Ok(WriteMapping {
            minimum: descriptor.min,
            maximum: descriptor.max,
            default_value: descriptor.default,
            unit,
            logarithmic: descriptor.log,
            integer: descriptor.integer,
            baseline_normalized: baseline,
            interpolation_scale: if decibels {
                WriteInterpolation::Decibels
            } else {
                WriteInterpolation::Linear
            },
        })
    }
    pub(crate) fn apply_write(
        &mut self,
        target: ResolvedWriteTarget,
        slot: u8,
        value: f32,
        restore: bool,
    ) -> Option<crate::automation_write::RampReceipt> {
        let frame = self.playback_clock().checked_add(target.latency)?;
        let (original, db) = self.write_original(target, frame)?;
        let current = (*self.write_field(target)?).map_or(original, |state| state.value(frame));
        let destination = if restore {
            self.write_original(target, frame.checked_add(RAMP_FRAMES)?)?
                .0
        } else {
            value
        };
        let state = Override::new(slot, frame, current, destination, db, restore);
        let field = self.write_field(target)?;
        if field.is_some_and(|existing| existing.slot != slot) {
            return None;
        }
        *field = Some(state);
        self.live_write_history = true;
        Some(state.receipt())
    }
    pub(crate) fn write_progress(
        &mut self,
        target: ResolvedWriteTarget,
        slot: u8,
    ) -> Option<(bool, Option<(u64, f32)>)> {
        let frame = self.playback_clock().checked_add(target.latency)?;
        let state = self.write_field(target)?.as_ref()?;
        if state.slot != slot {
            return None;
        }
        Some((state.finished(frame), state.last))
    }
    pub(crate) fn write_remaining(&mut self, target: ResolvedWriteTarget, slot: u8) -> Option<u64> {
        let frame = self.playback_clock().checked_add(target.latency)?;
        let state = self.write_field(target)?.as_ref()?;
        (state.slot == slot).then(|| state.remaining(frame))
    }
    pub(crate) fn clear_write(
        &mut self,
        target: ResolvedWriteTarget,
        slot: u8,
    ) -> Option<(u64, f32)> {
        let field = self.write_field(target)?;
        if field.is_some_and(|state| state.slot == slot) {
            field.take()?.last
        } else {
            None
        }
    }
    pub fn prepare_resident(
        project: &Project,
        maximum_source_bytes: usize,
        cancel: &AtomicBool,
    ) -> Result<Self> {
        Self::prepare_resident_inner(project, maximum_source_bytes, cancel, None)
    }
    pub fn prepare_resident_loop(
        project: &Project,
        maximum_source_bytes: usize,
        cancel: &AtomicBool,
        range: LoopRange,
    ) -> Result<Self> {
        LoopRange::new(range.start, range.end)?;
        if range.end > project.session.end() {
            return Err(Error::Invalid("Loop exceeds session end".into()));
        }
        Self::prepare_resident_inner(project, maximum_source_bytes, cancel, Some(range))
    }
    fn prepare_resident_inner(
        project: &Project,
        maximum_source_bytes: usize,
        cancel: &AtomicBool,
        loop_range: Option<LoopRange>,
    ) -> Result<Self> {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Conflict("Graph preparation cancelled".into()));
        }
        if maximum_source_bytes > 512 * 1024 * 1024 {
            return Err(Error::Invalid(
                "Resident source budget exceeds 512 MiB limit".into(),
            ));
        }
        project.validate_sources()?;
        let (state_bytes, immutable_bytes) = graph_admission_budget(project)?;
        let mut source_bytes = 0_usize;
        let mut readers = BTreeMap::new();
        for id in project.session.playback_asset_ids() {
            let media = &project.media[&id];
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Conflict("Graph preparation cancelled".into()));
            }
            let mut reader = WaveReader::open(&media.path)?;
            source_bytes = source_bytes
                .checked_add(
                    usize::try_from(reader.info.frames)
                        .ok()
                        .and_then(|n| n.checked_mul(8))
                        .ok_or_else(|| Error::Invalid("Source PCM size overflow".into()))?,
                )
                .filter(|bytes| *bytes <= maximum_source_bytes)
                .ok_or_else(|| {
                    Error::Invalid(
                        "Project exceeds resident PCM budget; use the Swift/paged host".into(),
                    )
                })?;
            // Decode and release one file at a time. Many short sources must
            // not retain a decoder buffer and descriptor each during preflight.
            let count = reader.info.frames as usize;
            let mut left = vec![0.; count];
            let mut right = vec![0.; count];
            for (index, (l, r)) in left
                .chunks_mut(BLOCK)
                .zip(right.chunks_mut(BLOCK))
                .enumerate()
            {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Conflict("Graph preparation cancelled".into()));
                }
                reader.read_stereo((index * BLOCK) as u64, l, r)?;
            }
            readers.insert(id, Source::Resident(Arc::new([left, right])));
        }
        let generation = GRAPH_GENERATION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| Error::Conflict("Graph generation exhausted".into()))?;
        Ok(Self {
            renderer: Renderer::new_with_sources(project, readers, cancel, loop_range)?.metered(),
            generation,
            handoff_token: 0,
            handoff_boundary: None,
            handoff_baseline: 0,
            output_boundary: 0,
            state_bytes,
            immutable_bytes,
            source_bytes,
            live_write_history: false,
        })
    }
    /// Worker construction primes DSP through bounded file decoders, then warms
    /// fixed pages before returning a graph whose callback path cannot do file I/O.
    pub fn prepare_paged(
        project: &Project,
        maximum_pcm_bytes: usize,
        cancel: &AtomicBool,
        loop_range: Option<LoopRange>,
    ) -> Result<(Self, crate::paged::PagedLoader)> {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Conflict("Graph preparation cancelled".into()));
        }
        project.validate_sources()?;
        if let Some(range) = loop_range {
            LoopRange::new(range.start, range.end)?;
            if range.end > project.session.end() {
                return Err(Error::Invalid("Loop exceeds session end".into()));
            }
        }
        let (state_bytes, immutable_bytes) = graph_admission_budget(project)?;
        let mut loader = crate::paged::PagedLoader::new(project, maximum_pcm_bytes)?;
        let readers = loader.sources().map(|(id, source)| (id, Source::Paged(source))).collect();
        let generation = GRAPH_GENERATION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| Error::Conflict("Graph generation exhausted".into()))?;
        let graph = Self {
            renderer: Renderer::new_with_sources(project, readers, cancel, loop_range)?.metered(),
            generation,
            handoff_token: 0,
            handoff_boundary: None,
            handoff_baseline: 0,
            output_boundary: 0,
            state_bytes,
            immutable_bytes,
            source_bytes: loader.stats().pcm_bytes,
            live_write_history: false,
        };
        loader.prepare_graph(&graph, BLOCK * 4, cancel)?;
        Ok((graph, loader))
    }
    pub(crate) fn read_ahead_keys(
        &self,
        cache: &Arc<crate::paged::Cache>,
        frames: usize,
        maximum: usize,
    ) -> Result<Vec<u64>> {
        if frames == 0 || frames > BLOCK * 8 {
            return Err(Error::Invalid(
                "Read-ahead window must contain 1 through 65,536 frames".into(),
            ));
        }
        let mut keys = std::collections::BTreeSet::new();
        for track in &self.renderer.tracks {
            let start = self.renderer.processing_position
                + if self.renderer.routing.is_some() {
                    0
                } else {
                    track.latency as u64
                };
            let mut spans = [(start, start + frames as u64), (0, 0)];
            if let Some(range) = self.renderer.loop_range {
                if frames as u64 >= range.end - range.start {
                    spans = [(range.start, range.end), (0, 0)];
                } else {
                    let first = range.wrap(start as i64) as u64;
                    let end = (first + frames as u64).min(range.end);
                    spans = [
                        (first, end),
                        (range.start, range.start + (frames as u64 - (end - first))),
                    ];
                }
            }
            for (start, end) in spans {
                if end <= start {
                    continue;
                }
                for envelope in track.envelopes.iter() {
                    let clip = &envelope.clip;
                    let lo = start.max(clip.start);
                    let hi = end.min(clip.start + clip.length);
                    if hi <= lo {
                        continue;
                    }
                    let Source::Paged(source) = &self.renderer.readers[&envelope.source_id] else {
                        return Err(Error::Invalid("Read-ahead requires a paged graph".into()));
                    };
                    if !Arc::ptr_eq(cache, &source.cache) {
                        return Err(Error::Conflict(
                            "Loader belongs to another source cache".into(),
                        ));
                    }
                    let first = (clip.offset + lo - clip.start) / BLOCK as u64;
                    let last = (clip.offset + hi - clip.start - 1) / BLOCK as u64;
                    for page in first..=last {
                        keys.insert(crate::paged::key(source.source_index(), page));
                        if keys.len() > maximum {
                            return Err(Error::Invalid(
                                "Read-ahead working set exceeds page budget".into(),
                            ));
                        }
                    }
                }
            }
        }
        Ok(keys.into_iter().collect())
    }
    pub fn position(&self) -> u64 {
        self.renderer.position
    }
    /// Monotonic playback coordinate for this graph run, including loop cycles.
    /// Canonical timeline seeks restore the first cycle; aligned publication
    /// preserves this coordinate. It is not elapsed wall-clock time.
    pub fn playback_clock(&self) -> u64 {
        self.renderer.processing_position
            - (self.renderer.master.latency
                + self.renderer.master_padding.len()
                + self.renderer.routing.as_ref().map_or(0, |g| g.latency)) as u64
    }
    pub(crate) fn assign_output_boundary(&mut self, boundary: u64) {
        self.output_boundary = boundary;
    }
    pub(crate) fn output_boundary(&self) -> u64 {
        self.output_boundary
    }
    pub(crate) fn assign_handoff_baseline(&mut self, token: u64) {
        self.handoff_baseline = token;
    }
    pub(crate) fn handoff_baseline(&self) -> u64 {
        self.handoff_baseline
    }
    pub(crate) fn assign_handoff_token(&mut self, token: u64) {
        self.handoff_token = token;
    }
    pub(crate) fn assign_handoff_boundary(&mut self, boundary: Option<u64>) {
        self.handoff_boundary = boundary;
    }
    pub(crate) fn handoff_boundary(&self) -> Option<u64> {
        self.handoff_boundary
    }
    pub fn handoff_token(&self) -> u64 {
        self.handoff_token
    }
    pub fn loop_range(&self) -> Option<LoopRange> {
        self.renderer.loop_range
    }
    fn canonical_loop_position(&self) -> bool {
        self.renderer.loop_range.map_or(true, |range| {
            let r = &self.renderer;
            let initial = range.start
                + (r.master.latency
                    + r.master_padding.len()
                    + r.routing.as_ref().map_or(0, |g| g.latency)) as u64;
            r.processing_position - initial < range.end - range.start
        })
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// Shared prepared automation, envelopes and processor configuration. Charged
    /// once at graph admission, not again for each mutable checkpoint/fork.
    pub fn immutable_storage_budget(&self) -> usize {
        self.immutable_bytes
    }
    /// Independently cloned DSP histories, scheduler state and scratch. Automation
    /// density does not increase this allocation bound.
    pub fn mutable_storage_budget(&self) -> usize {
        self.state_bytes
    }
    pub fn source_storage_bytes(&self) -> usize {
        self.source_bytes
    }
    /// Canonical authored DSP history, excluding source underruns and live Write
    /// performances. Rebuild the authored graph after committing a performance.
    pub fn has_clean_history(&self) -> bool {
        self.renderer.source_underruns == 0 && !self.live_write_history
    }
    pub fn checkpoint_storage_budget(&self) -> usize {
        // Conservative standalone checkpoint custody includes the shared data it
        // can keep alive after its original graph is retired. Caches may contain
        // multiple generations; excluding those Arcs would undercount revisions.
        self.state_bytes + self.immutable_bytes
    }
    pub fn tail_frames(&self) -> u64 {
        self.renderer.tail_frames
    }
    pub fn render_callback(&mut self, left: &mut [f32], right: &mut [f32]) -> CallbackStatus {
        if left.len() != right.len()
            || left.len() > BLOCK
            || self.playback_clock() > crate::model::MAX_FRAME - left.len() as u64
        {
            left.fill(0.);
            right.fill(0.);
            return CallbackStatus::InvalidBlock;
        }
        let underruns = self.renderer.source_underruns;
        if self.renderer.render_mix(left, right, false).is_err() {
            self.renderer.source_underruns = self.renderer.source_underruns.saturating_add(1);
            left.fill(0.);
            right.fill(0.);
            return CallbackStatus::SourceFault;
        }
        self.renderer.position = timeline_frame(
            (self.renderer.position + left.len() as u64) as i64,
            self.renderer.loop_range,
        ) as u64;
        for value in left.iter_mut().chain(right) {
            if !value.is_finite() {
                *value = 0.;
            }
        }
        if self.renderer.source_underruns != underruns {
            CallbackStatus::SourceUnderrun
        } else {
            CallbackStatus::Rendered
        }
    }
    /// Worker-only independent graph fork. Immutable PCM is shared, mutable DSP
    /// and every scheduler buffer are independently allocated before publication.
    pub fn fork(&self) -> Self {
        let readers = self
            .renderer
            .readers
            .iter()
            .map(|(id, source)| {
                let copy = match source {
                    Source::Resident(audio) => Source::Resident(Arc::clone(audio)),
                    Source::Paged(source) => Source::Paged(source.clone()),
                };
                (id.clone(), copy)
            })
            .collect();
        let r = &self.renderer;
        Self {
            renderer: Renderer {
                routing: r.routing.clone(),
                tracks: r.tracks.clone(),
                master: r.master.clone(),
                readers,
                source_left: r.source_left.clone(),
                source_right: r.source_right.clone(),
                track_left: r.track_left.clone(),
                track_right: r.track_right.clone(),
                position: r.position,
                processing_position: r.processing_position,
                master_padding: r.master_padding.clone(),
                padding_index: r.padding_index,
                tail_frames: r.tail_frames,
                loop_range: r.loop_range,
                source_underruns: r.source_underruns,
            },
            generation: self.generation,
            handoff_token: 0,
            handoff_boundary: None,
            handoff_baseline: 0,
            output_boundary: 0,
            state_bytes: self.state_bytes,
            immutable_bytes: self.immutable_bytes,
            source_bytes: self.source_bytes,
            live_write_history: self.live_write_history,
        }
    }
    pub fn checkpoint(&self) -> DspCheckpoint {
        let r = &self.renderer;
        DspCheckpoint {
            generation: self.generation,
            routing: r.routing.clone(),
            tracks: r.tracks.clone(),
            master: r.master.clone(),
            position: r.position,
            processing_position: r.processing_position,
            padding: r.master_padding.clone(),
            padding_index: r.padding_index,
            source_underruns: r.source_underruns,
            playback_clock: self.playback_clock(),
            live_write_history: self.live_write_history,
            bytes: self.checkpoint_storage_budget(),
        }
    }
    pub fn restore_checkpoint(&mut self, checkpoint: &DspCheckpoint) -> Result<()> {
        if checkpoint.generation != self.generation {
            return Err(Error::Conflict(
                "Checkpoint belongs to another prepared graph generation".into(),
            ));
        }
        if checkpoint.source_underruns != 0 || checkpoint.live_write_history {
            return Err(Error::Conflict(
                "An underrun or live Write state cannot restore canonical authored DSP history"
                    .into(),
            ));
        }
        let r = &mut self.renderer;
        self.live_write_history = false;
        r.source_underruns = 0;
        r.routing = checkpoint.routing.clone();
        r.tracks = checkpoint.tracks.clone();
        r.master = checkpoint.master.clone();
        r.position = checkpoint.position;
        r.processing_position = checkpoint.processing_position;
        r.master_padding = checkpoint.padding.clone();
        r.padding_index = checkpoint.padding_index;
        Ok(())
    }
    /// Worker-only warmup from the nearest retained canonical checkpoint.
    pub fn seek(
        &mut self,
        frame: u64,
        checkpoints: &CheckpointCache,
        cancel: &AtomicBool,
    ) -> Result<u64> {
        if let Some(range) = self.renderer.loop_range {
            if frame < range.start || frame >= range.end {
                return Err(Error::Invalid(
                    "Seek must be inside the prepared loop".into(),
                ));
            }
            if !self.canonical_loop_position()
                && checkpoints.nearest(self.generation, frame).is_none()
            {
                return Err(Error::Conflict(
                    "Loop seek requires canonical first-cycle worker checkpoint/fork".into(),
                ));
            }
        }
        if frame > crate::model::MAX_FRAME {
            return Err(Error::Invalid("Seek exceeds timeline bound".into()));
        }
        if let Some(checkpoint) = checkpoints.nearest(self.generation, frame) {
            self.restore_checkpoint(checkpoint)?;
        }
        if !self.has_clean_history() {
            return Err(Error::Conflict(
                "Seek after an underrun or live Write requires a canonical authored worker checkpoint/fork".into(),
            ));
        }
        if frame < self.position() {
            return Err(Error::Conflict(
                "No canonical checkpoint before requested seek; use the frame-zero worker fork"
                    .into(),
            ));
        }
        let start = self.position();
        // Bounded warm start: effect state at `frame` depends only on recent
        // input (inserts forget input after their tail), so a long seek jumps
        // to `frame - warmup` and renders only the warm-up instead of the whole
        // history. The warm-up covers the graph's tail and at least 10 s.
        // Any gap longer than the warm-up jumps (it used to replay gaps of
        // up to 30 s in full, so resuming after a short play lagged).
        if self.renderer.loop_range.is_none() {
            let warmup = self
                .renderer
                .tail_frames
                .saturating_add(48000)
                .max(10 * 48000);
            let target = frame.saturating_sub(warmup);
            if target > start {
                let delta = target - start;
                let r = &mut self.renderer;
                r.position += delta;
                r.processing_position += delta;
                r.master_padding.fill([0.; 2]);
            }
        }
        let start = self.position();
        let mut l = [0.; BLOCK];
        let mut r = l;
        while self.position() < frame {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Conflict("Seek preparation cancelled".into()));
            }
            let n = (frame - self.position()).min(BLOCK as u64) as usize;
            if let Err(error) = self.renderer.render(&mut l[..n], &mut r[..n]) {
                self.renderer.source_underruns = self.renderer.source_underruns.saturating_add(1);
                return Err(error);
            }
        }
        Ok(frame - start)
    }
    /// Worker-only continuous-loop preparation at an unwrapped playback clock.
    /// It retains all prior cycle DSP history; timeline seek remains first-cycle
    /// canonical through seek(). Checkpoints must be independently worker-made.
    pub fn seek_playback_clock(
        &mut self,
        clock: u64,
        checkpoints: &CheckpointCache,
        cancel: &AtomicBool,
    ) -> Result<u64> {
        if self.loop_range().is_none() {
            return self.seek(clock, checkpoints, cancel);
        }
        if clock > crate::model::MAX_FRAME || clock < self.loop_range().unwrap().start {
            return Err(Error::Invalid(
                "Playback clock is outside the prepared run".into(),
            ));
        }
        if let Some(checkpoint) = checkpoints.nearest_clock(self.generation, clock) {
            self.restore_checkpoint(checkpoint)?;
        }
        if !self.has_clean_history() || clock < self.playback_clock() {
            return Err(Error::Conflict(
                "Playback preparation requires a clean earlier worker checkpoint/fork".into(),
            ));
        }
        let start = self.playback_clock();
        let mut left = [0.; BLOCK];
        let mut right = left;
        while self.playback_clock() < clock {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Conflict("Playback preparation cancelled".into()));
            }
            let frames = (clock - self.playback_clock()).min(BLOCK as u64) as usize;
            if let Err(error) = self
                .renderer
                .render(&mut left[..frames], &mut right[..frames])
            {
                self.renderer.source_underruns = self.renderer.source_underruns.saturating_add(1);
                return Err(error);
            }
        }
        Ok(clock - start)
    }
}
/// Worker-owned bounded checkpoint history. Never clone/store/evict this from an
/// audio callback; canonical state is generated by an independent worker graph.
pub struct CheckpointCache {
    entries: std::collections::VecDeque<DspCheckpoint>,
    maximum: usize,
    used: usize,
}
impl CheckpointCache {
    pub fn new(maximum_bytes: usize) -> Self {
        Self {
            entries: Default::default(),
            maximum: maximum_bytes.min(256 * 1024 * 1024),
            used: 0,
        }
    }
    pub fn storage_budget(&self) -> usize {
        self.used
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn store(&mut self, graph: &PreparedGraph) -> Result<()> {
        if !graph.canonical_loop_position() {
            return Err(Error::Conflict(
                "A cycled loop state cannot enter the timeline seek cache".into(),
            ));
        }
        self.store_playback_clock(graph)
    }
    /// Explicit clean continuous-loop worker history. It is excluded from normal
    /// wrapped timeline seeks; only seek_playback_clock() may restore later cycles.
    pub fn store_playback_clock(&mut self, graph: &PreparedGraph) -> Result<()> {
        if !graph.has_clean_history() {
            return Err(Error::Conflict(
                "An underrun state cannot enter the canonical seek cache".into(),
            ));
        }
        let bytes = graph.checkpoint_storage_budget();
        if bytes > self.maximum {
            return Err(Error::Invalid(
                "Graph checkpoint exceeds cache budget".into(),
            ));
        }
        if self
            .entries
            .iter()
            .any(|c| c.generation == graph.generation && c.playback_clock == graph.playback_clock())
        {
            return Ok(());
        }
        while self.used + bytes > self.maximum {
            self.used -= self.entries.pop_front().unwrap().bytes;
        }
        self.entries.push_back(graph.checkpoint());
        self.used += bytes;
        Ok(())
    }
    fn nearest(&self, generation: u64, frame: u64) -> Option<&DspCheckpoint> {
        self.entries
            .iter()
            .filter(|c| {
                c.generation == generation && c.position == c.playback_clock && c.position <= frame
            })
            .max_by_key(|c| c.position)
    }
    fn nearest_clock(&self, generation: u64, clock: u64) -> Option<&DspCheckpoint> {
        self.entries
            .iter()
            .filter(|c| c.generation == generation && c.playback_clock <= clock)
            .max_by_key(|c| c.playback_clock)
    }
}

#[cfg(test)]
mod write_history_tests {
    use super::*;
    #[test]
    fn a_live_performance_cannot_enter_or_restore_an_authored_worker_checkpoint() {
        let p = Project::open(
            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/legacy-gain.soundroom"),
        )
        .unwrap();
        let mut g =
            PreparedGraph::prepare_resident(&p, 16 * 1024 * 1024, &AtomicBool::new(false)).unwrap();
        let canonical = g.checkpoint();
        let target = g
            .resolve_write_target(&WriteTarget::EffectAmount {
                track_id: p.session.tracks[0].id.clone(),
                effect_id: p.session.tracks[0].realtime_effects[0].id.clone(),
            })
            .unwrap();
        assert!(g.apply_write(target, 0, 0.9, false).is_some());
        assert!(!g.has_clean_history());
        let tainted = g.checkpoint();
        assert!(g.restore_checkpoint(&tainted).is_err());
        assert!(CheckpointCache::new(128 * 1024 * 1024).store(&g).is_err());
        let (mut control, _host) = crate::realtime::channel();
        assert!(control.publish(Box::new(g.fork())).is_err());
        g.clear_write(target, 0);
        assert!(!g.has_clean_history());
        g.restore_checkpoint(&canonical).unwrap();
        assert!(g.has_clean_history());
    }
}
