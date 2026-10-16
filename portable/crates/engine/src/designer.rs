//! Native Designer patches, organic seeded motion and prepared instrument voices.
//! Voice preparation and pattern bounces are workers; `Voice::next` allocates nothing.
use crate::{model::Extra, Error, Result};
use serde::{Deserialize, Deserializer, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
fn pow(x: f64, y: f64) -> f64 {
    x.powf(y)
}
fn exp(x: f64) -> f64 {
    x.exp()
}
fn sin(x: f64) -> f64 {
    x.sin()
}
fn sqrt(x: f64) -> f64 {
    x.sqrt()
}
fn min(x: f64, y: f64) -> f64 {
    x.min(y)
}
const PI: f64 = std::f64::consts::PI;
fn bounded(v: f64, lo: f64, hi: f64, fallback: f64) -> f64 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        fallback
    }
}
/// Exact numeric project seed; v3 clients may also supply its decimal string form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Seed(pub u64);
impl<'de> Deserialize<'de> for Seed {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = Seed;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an unsigned 64-bit integer or canonical decimal string")
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<Seed, E> {
                Ok(Seed(v))
            }
            fn visit_str<E: serde::de::Error>(self, s: &str) -> std::result::Result<Seed, E> {
                if s.is_empty()
                    || s.len() > 20
                    || s.len() > 1 && s.starts_with('0')
                    || !s.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err(E::custom("Invalid decimal seed"));
                }
                s.parse().map(Seed).map_err(E::custom)
            }
        }
        d.deserialize_any(Visitor)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Organic {
    pub drift: f64,
    pub timbre: f64,
    pub breath: f64,
    pub seed: Seed,
}
impl Default for Organic {
    fn default() -> Self {
        Self {
            drift: 0.,
            timbre: 0.,
            breath: 0.,
            seed: Seed(17),
        }
    }
}
impl Organic {
    pub fn variation(&self, pitch: i32, beat: f64) -> f64 {
        let mut state = self
            .seed
            .0
            .wrapping_add((pitch.clamp(0, 127) as u64).wrapping_mul(0x9E3779B97F4A7C15));
        state ^= bounded(beat, 0., f64::MAX, 0.).to_bits();
        state = (state ^ (state >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        state = (state ^ (state >> 27)).wrapping_mul(0x94D049BB133111EB);
        state ^= state >> 31;
        (state >> 11) as f64 / (1_u64 << 53) as f64 * 2. - 1.
    }
    fn color(&self, p: &Pattern, pitch: i32, beat: f64) -> (f64, f64, f64) {
        if self.drift > 0. || self.timbre > 0. || self.breath > 0. {
            let variation = self.variation(pitch, beat) * bounded(self.timbre, 0., 1., 0.);
            (
                (p.tone + variation * 0.07).clamp(0., 1.),
                (p.character + variation * 0.09).clamp(0., 1.),
                (p.attack * (1. + variation * 0.15)).clamp(0., 2.),
            )
        } else {
            (p.tone, p.character, p.attack)
        }
    }
}
#[derive(Clone)]
struct Motion {
    drift: f64,
    breath: f64,
    phase: f64,
    rate: f64,
    age: u64,
    ratio: f64,
    gain: f64,
}
impl Motion {
    fn new(s: &Organic, pitch: i32, beat: f64, rate: f64) -> Self {
        let mut m = Self {
            drift: bounded(s.drift, 0., 1., 0.),
            breath: bounded(s.breath, 0., 1., 0.),
            phase: s.variation(pitch, beat) * PI,
            rate,
            age: 0,
            ratio: 1.,
            gain: 1.,
        };
        m.update();
        m
    }
    fn update(&mut self) {
        let seconds = self.age as f64 / self.rate;
        if self.drift > 0. {
            let cents = self.drift
                * (4. * (seconds * 2. * PI * 0.31 + self.phase).sin()
                    + 2. * (seconds * 2. * PI * 0.173 - self.phase).sin());
            self.ratio = 2_f64.powf(cents / 1200.);
        }
        if self.breath > 0. {
            self.gain = 1. + self.breath * 0.06 * (seconds * 2. * PI * 0.43 + self.phase).sin();
        }
    }
    fn advance(&mut self) {
        self.age += 1;
        if self.age & 63 == 0 && (self.drift > 0. || self.breath > 0.) {
            self.update();
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Instrument {
    #[default]
    Synthesizer,
    StudioPiano,
    TinePiano,
    NylonStrings,
    Marimba,
    GlassBells,
    TonewheelOrgan,
    StringEnsemble,
    ElectricGuitar,
    PickedBass,
    VocalEnsemble,
    UprightPiano,
    AcousticGuitar,
    UprightBass,
    Vibraphone,
    WoodenFlute,
    SoloCello,
    ConcertHarp,
    Kalimba,
}
impl Instrument {
    pub const ALL: [Self; 19] = [
        Self::Synthesizer,
        Self::StudioPiano,
        Self::TinePiano,
        Self::NylonStrings,
        Self::Marimba,
        Self::GlassBells,
        Self::TonewheelOrgan,
        Self::StringEnsemble,
        Self::ElectricGuitar,
        Self::PickedBass,
        Self::VocalEnsemble,
        Self::UprightPiano,
        Self::AcousticGuitar,
        Self::UprightBass,
        Self::Vibraphone,
        Self::WoodenFlute,
        Self::SoloCello,
        Self::ConcertHarp,
        Self::Kalimba,
    ];
    pub fn apply(self, p: &mut Pattern) {
        p.instrument = self;
        p.character = 0.5;
        p.resonance = 0.5;
        p.tone = 0.86;
        p.attack = 0.003;
        p.decay = 0.12;
        p.sustain = 1.;
        p.release = 0.25;
        p.drive = 1.;
        match self {
            Self::StudioPiano => {
                p.release = 0.24;
                p.character = 0.45;
            }
            Self::TinePiano => {
                p.release = 0.4;
                p.character = 0.6;
            }
            Self::NylonStrings => p.release = 0.18,
            Self::Marimba => p.release = 0.12,
            Self::GlassBells => {
                p.release = 0.9;
                p.resonance = 0.75;
            }
            Self::TonewheelOrgan => {
                p.attack = 0.006;
                p.release = 0.04;
            }
            Self::StringEnsemble => {
                p.attack = 0.18;
                p.release = 0.65;
                p.tone = 0.65;
            }
            Self::ElectricGuitar => {
                p.release = 0.2;
                p.character = 0.6;
                p.tone = 0.72;
            }
            Self::PickedBass => {
                p.release = 0.13;
                p.tone = 0.58;
                p.character = 0.4;
            }
            Self::VocalEnsemble => {
                p.attack = 0.12;
                p.release = 0.45;
                p.tone = 0.82;
            }
            Self::UprightPiano => {
                p.attack = 0.002;
                p.release = 0.3;
                p.character = 0.45;
                p.tone = 0.8;
            }
            Self::AcousticGuitar => {
                p.release = 0.3;
                p.character = 0.4;
                p.tone = 0.82;
            }
            Self::UprightBass => {
                p.release = 0.16;
                p.character = 0.45;
                p.tone = 0.62;
            }
            Self::Vibraphone => {
                p.release = 0.9;
                p.resonance = 0.55;
            }
            Self::WoodenFlute => {
                p.attack = 0.06;
                p.release = 0.14;
                p.character = 0.4;
                p.resonance = 0.45;
                p.tone = 0.78;
            }
            Self::SoloCello => {
                p.attack = 0.09;
                p.release = 0.3;
                p.character = 0.45;
                p.tone = 0.72;
            }
            Self::ConcertHarp => {
                p.release = 0.7;
                p.tone = 0.8;
            }
            Self::Kalimba => {
                p.release = 0.55;
                p.tone = 0.85;
            }
            _ => {}
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Waveform {
    Sine,
    #[default]
    Triangle,
    Saw,
    Pulse,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Articulation {
    Normal,
    Staccato,
    Tenuto,
    Accent,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ControllerKind {
    Sustain,
    Modulation,
    Expression,
    PitchBend,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    #[serde(default)]
    pub id: String,
    pub pitch: i32,
    pub velocity: f64,
    pub start_beat: f64,
    pub duration_beats: f64,
    pub articulation: Option<Articulation>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Controller {
    pub kind: ControllerKind,
    pub beat: f64,
    pub value: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Pattern {
    pub notes: Vec<Note>,
    pub instrument: Instrument,
    pub character: f64,
    pub resonance: f64,
    pub bpm: f64,
    pub beats: f64,
    pub waveform: Waveform,
    pub tone: f64,
    pub attack: f64,
    pub release: f64,
    pub decay: f64,
    pub sustain: f64,
    pub second_waveform: Waveform,
    pub oscillator_mix: f64,
    pub fine_tune: f64,
    pub detune: f64,
    pub pulse_width: f64,
    pub sub_level: f64,
    pub noise_level: f64,
    pub drive: f64,
    pub controllers: Vec<Controller>,
    pub layers: Vec<Layer>,
    pub organic: Organic,
    pub base_gain: f64,
    pub base_pan: f64,
    /// Instrument-designer shaping: when set, the instrument is generated
    /// from its measured model with this shape instead of played from its
    /// recording.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instrument_shape: Option<crate::measured_instruments::InstrumentShape>,
    #[serde(flatten)]
    pub extra: Extra,
}
impl Default for Pattern {
    fn default() -> Self {
        Self {
            notes: [60, 64, 67, 64]
                .iter()
                .enumerate()
                .map(|(i, &pitch)| Note {
                    id: String::new(),
                    pitch,
                    velocity: 0.8,
                    start_beat: i as f64,
                    duration_beats: 0.5,
                    articulation: None,
                })
                .collect(),
            instrument: Instrument::Synthesizer,
            character: 0.5,
            resonance: 0.5,
            bpm: 120.,
            beats: 4.,
            waveform: Waveform::Triangle,
            tone: 0.65,
            attack: 0.01,
            release: 0.15,
            decay: 0.1,
            sustain: 1.,
            second_waveform: Waveform::Saw,
            oscillator_mix: 0.,
            fine_tune: 0.,
            detune: 0.,
            pulse_width: 0.5,
            sub_level: 0.,
            noise_level: 0.,
            drive: 1.,
            controllers: vec![],
            layers: vec![],
            organic: Organic::default(),
            base_gain: 1.,
            base_pan: 0.,
            instrument_shape: None,
            extra: Extra::new(),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Preset {
    #[serde(rename = "Warm keys")]
    WarmKeys,
    #[serde(rename = "Glass pluck")]
    #[default]
    GlassPluck,
    #[serde(rename = "Round bass")]
    RoundBass,
    #[serde(rename = "Wide pad")]
    WidePad,
    #[serde(rename = "Bright lead")]
    BrightLead,
    #[serde(rename = "Soft drum")]
    SoftDrum,
    #[serde(rename = "Studio piano")]
    StudioPiano,
    #[serde(rename = "Tine electric piano")]
    TinePiano,
    #[serde(rename = "Nylon plucked strings")]
    NylonStrings,
    #[serde(rename = "Concert marimba")]
    Marimba,
    #[serde(rename = "Glass bells")]
    GlassBells,
    #[serde(rename = "Tonewheel organ")]
    TonewheelOrgan,
    #[serde(rename = "String ensemble")]
    StringEnsemble,
    #[serde(rename = "Electric guitar")]
    ElectricGuitar,
    #[serde(rename = "Picked bass")]
    PickedBass,
    #[serde(rename = "Vocal ensemble")]
    VocalEnsemble,
    #[serde(rename = "Felt upright piano")]
    FeltUpright,
    #[serde(rename = "Steel acoustic guitar")]
    AcousticGuitar,
    #[serde(rename = "Upright bass")]
    UprightBass,
    #[serde(rename = "Vibraphone")]
    Vibraphone,
    #[serde(rename = "Wooden flute")]
    WoodenFlute,
    #[serde(rename = "Solo cello")]
    SoloCello,
    #[serde(rename = "Concert harp")]
    ConcertHarp,
    #[serde(rename = "Kalimba")]
    Kalimba,
}
impl Preset {
    const ALL: [Self; 24] = [
        Self::WarmKeys,
        Self::GlassPluck,
        Self::RoundBass,
        Self::WidePad,
        Self::BrightLead,
        Self::SoftDrum,
        Self::StudioPiano,
        Self::TinePiano,
        Self::NylonStrings,
        Self::Marimba,
        Self::GlassBells,
        Self::TonewheelOrgan,
        Self::StringEnsemble,
        Self::ElectricGuitar,
        Self::PickedBass,
        Self::VocalEnsemble,
        Self::FeltUpright,
        Self::AcousticGuitar,
        Self::UprightBass,
        Self::Vibraphone,
        Self::WoodenFlute,
        Self::SoloCello,
        Self::ConcertHarp,
        Self::Kalimba,
    ];
    pub fn applying(self, p: &Pattern) -> Pattern {
        let mut r = Pattern {
            notes: p.notes.clone(),
            bpm: bounded(p.bpm, 30., 300., 120.),
            beats: bounded(p.beats, 0.25, 64., 4.),
            ..Pattern::default()
        };
        match self {
            Self::WarmKeys => {
                r.waveform = Waveform::Triangle;
                r.tone = 0.6;
                r.decay = 0.3;
                r.sustain = 0.55;
                r.release = 0.25;
            }
            Self::GlassPluck => {
                r.waveform = Waveform::Sine;
                r.second_waveform = Waveform::Triangle;
                r.oscillator_mix = 0.3;
                r.detune = 12.;
                r.tone = 0.9;
                r.attack = 0.002;
                r.decay = 0.35;
                r.sustain = 0.05;
                r.release = 0.15;
            }
            Self::RoundBass => {
                r.waveform = Waveform::Pulse;
                r.pulse_width = 0.35;
                r.tone = 0.35;
                r.sub_level = 0.8;
                r.decay = 0.2;
                r.sustain = 0.65;
                r.release = 0.08;
                r.drive = 1.4;
            }
            Self::WidePad => {
                r.waveform = Waveform::Saw;
                r.second_waveform = Waveform::Saw;
                r.oscillator_mix = 0.5;
                r.detune = 18.;
                r.tone = 0.5;
                r.attack = 0.5;
                r.decay = 0.5;
                r.sustain = 0.8;
                r.release = 1.2;
            }
            Self::BrightLead => {
                r.waveform = Waveform::Saw;
                r.second_waveform = Waveform::Pulse;
                r.oscillator_mix = 0.35;
                r.detune = -7.;
                r.tone = 0.9;
                r.attack = 0.005;
                r.decay = 0.1;
                r.sustain = 0.8;
                r.release = 0.12;
                r.drive = 2.;
            }
            Self::SoftDrum => {
                r.waveform = Waveform::Sine;
                r.noise_level = 0.8;
                r.sub_level = 0.5;
                r.tone = 0.5;
                r.attack = 0.;
                r.decay = 0.12;
                r.sustain = 0.;
                r.release = 0.05;
            }
            other => {
                let kind = match other {
                    Self::StudioPiano => Instrument::StudioPiano,
                    Self::TinePiano => Instrument::TinePiano,
                    Self::NylonStrings => Instrument::NylonStrings,
                    Self::Marimba => Instrument::Marimba,
                    Self::GlassBells => Instrument::GlassBells,
                    Self::TonewheelOrgan => Instrument::TonewheelOrgan,
                    Self::StringEnsemble => Instrument::StringEnsemble,
                    Self::ElectricGuitar => Instrument::ElectricGuitar,
                    Self::PickedBass => Instrument::PickedBass,
                    Self::FeltUpright => Instrument::UprightPiano,
                    Self::AcousticGuitar => Instrument::AcousticGuitar,
                    Self::UprightBass => Instrument::UprightBass,
                    Self::Vibraphone => Instrument::Vibraphone,
                    Self::WoodenFlute => Instrument::WoodenFlute,
                    Self::SoloCello => Instrument::SoloCello,
                    Self::ConcertHarp => Instrument::ConcertHarp,
                    Self::Kalimba => Instrument::Kalimba,
                    _ => Instrument::VocalEnsemble,
                };
                kind.apply(&mut r);
            }
        }
        r.fine_tune = p.fine_tune;
        r.organic = p.organic.clone();
        r.controllers = p.controllers.clone();
        r.layers = p.layers.clone();
        r.base_gain = p.base_gain;
        r.base_pan = p.base_pan;
        r
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Layer {
    pub id: String,
    pub preset: Preset,
    pub gain: f64,
    pub pan: f64,
    pub transpose: i32,
    pub minimum_velocity: f64,
    pub maximum_velocity: f64,
    pub enabled: bool,
    pub tone_override: Option<f64>,
    pub attack_override: Option<f64>,
    pub release_override: Option<f64>,
    pub character_override: Option<f64>,
    pub velocity_fade: Option<f64>,
    pub fine_tune: Option<f64>,
    pub delay: Option<f64>,
    pub lowest_pitch: Option<i32>,
    pub highest_pitch: Option<i32>,
    pub decay_override: Option<f64>,
    pub sustain_override: Option<f64>,
    pub organic_seed: Option<Seed>,
}
impl Default for Layer {
    fn default() -> Self {
        Self {
            id: String::new(),
            preset: Preset::GlassPluck,
            gain: 0.5,
            pan: 0.,
            transpose: 0,
            minimum_velocity: 0.,
            maximum_velocity: 1.,
            enabled: true,
            tone_override: None,
            attack_override: None,
            release_override: None,
            character_override: None,
            velocity_fade: None,
            fine_tune: None,
            delay: None,
            lowest_pitch: None,
            highest_pitch: None,
            decay_override: None,
            sustain_override: None,
            organic_seed: None,
        }
    }
}
impl Layer {
    pub fn accepts(&self, pitch: i32) -> bool {
        let low = self.lowest_pitch.unwrap_or(0).clamp(0, 127);
        let high = self.highest_pitch.unwrap_or(127).clamp(low, 127);
        pitch >= low && pitch <= high
    }
    pub fn onset_delay(&self) -> f64 {
        bounded(self.delay.unwrap_or(0.), 0., 0.5, 0.)
    }
    pub fn weight(&self, velocity: f64) -> f64 {
        let lo = bounded(self.minimum_velocity, 0., 1., 0.);
        let hi = bounded(self.maximum_velocity, 0., 1., 0.).max(lo);
        if !velocity.is_finite() || velocity < lo || velocity > hi {
            return 0.;
        }
        let fade = bounded(self.velocity_fade.unwrap_or(0.), 0., 0.5, 0.).min((hi - lo) / 2.);
        if fade <= 0. {
            return 1.;
        }
        let input = if lo > 0. {
            (((velocity - lo) / fade).min(1.) * PI / 2.).sin()
        } else {
            1.
        };
        let output = if hi < 1. {
            (((hi - velocity) / fade).min(1.) * PI / 2.).sin()
        } else {
            1.
        };
        input * output
    }
    pub fn instrument(&self, p: &Pattern) -> Pattern {
        let source = Pattern {
            notes: p.notes.clone(),
            bpm: p.bpm,
            beats: p.beats,
            ..Pattern::default()
        };
        let mut result = self.preset.applying(&source);
        result.fine_tune = bounded(p.fine_tune, -50., 50., 0.)
            + bounded(self.fine_tune.unwrap_or(0.), -50., 50., 0.);
        if let Some(v) = self.decay_override {
            result.decay = bounded(v, 0., 2., 0.);
        }
        if let Some(v) = self.sustain_override {
            result.sustain = bounded(v, 0., 1., 0.);
        }
        result.controllers = p.controllers.clone();
        result.organic = p.organic.clone();
        result.organic.seed = self
            .organic_seed
            .unwrap_or(Seed(p.organic.seed.0.wrapping_add(
                (97 + Preset::ALL.iter().position(|v| *v == self.preset).unwrap() * 131) as u64,
            )));
        if let Some(v) = self.tone_override {
            result.tone = bounded(v, 0., 1., 0.);
        }
        if let Some(v) = self.attack_override {
            result.attack = bounded(v, 0., 2., 0.);
        }
        if let Some(v) = self.release_override {
            result.release = bounded(v, 0., 2., 0.);
        }
        if let Some(v) = self.character_override {
            result.character = bounded(v, 0., 1., 0.);
        }
        result.notes = p
            .notes
            .iter()
            .filter(|n| {
                self.accepts(n.pitch)
                    && n.velocity >= bounded(self.minimum_velocity, 0., 1., 0.)
                    && n.velocity <= bounded(self.maximum_velocity, 0., 1., 0.)
            })
            .map(|n| {
                let mut n = n.clone();
                n.velocity *= self.weight(n.velocity);
                n.pitch = n
                    .pitch
                    .saturating_add(self.transpose.clamp(-24, 24))
                    .clamp(0, 127);
                n
            })
            .collect();
        result
    }
}
#[derive(Clone)]
struct Mode {
    sine: f64,
    cosine: f64,
    rotation_sine: f64,
    rotation_cosine: f64,
    radians: f64,
    amplitude: f64,
    damping: f64,
}
impl Mode {
    fn next(&mut self) -> f64 {
        let result = self.sine * self.amplitude;
        let next = self.sine * self.rotation_cosine + self.cosine * self.rotation_sine;
        self.cosine = self.cosine * self.rotation_cosine - self.sine * self.rotation_sine;
        self.sine = next;
        self.amplitude *= self.damping;
        result
    }
}
#[derive(Clone)]
struct MusicalVoice {
    random: u64,
    burst_gain: f64,
    burst_decay: f64,
    burst_filter: f64,
    burst_lp: f64,
    breath_gain: f64,
    breath_low: f64,
    breath_high: f64,
    breath_lp1: f64,
    breath_lp2: f64,
    vibrato_rate: f64,
    vibrato_cents: f64,
    vibrato_delay: f64,
    vibrato_ratio: f64,
    tremolo_depth: f64,
    burst_high: bool,

    modes: Vec<Mode>,
    kind: Instrument,
    character: f64,
    resonance: f64,
    velocity: f64,
    rate: f64,
    frequency: f64,
    age: f64,
    carrier: f64,
    modulator: f64,
    last_ratio: f64,
}
impl MusicalVoice {
    fn new(p: &Pattern, frequency: f64, velocity: f64, rate: f64, character: f64) -> Self {
        let mut v = Self {
            random: p.organic.seed.0 ^ frequency.to_bits() ^ 0x9E3779B97F4A7C15,
            burst_gain: 0.,
            burst_decay: 1.,
            burst_filter: 0.,
            burst_lp: 0.,
            breath_gain: 0.,
            breath_low: 0.,
            breath_high: 0.,
            breath_lp1: 0.,
            breath_lp2: 0.,
            vibrato_rate: 0.,
            vibrato_cents: 0.,
            vibrato_delay: 0.,
            vibrato_ratio: 1.,
            tremolo_depth: 0.,
            burst_high: false,
            modes: Vec::with_capacity(128),
            kind: p.instrument,
            character: bounded(character, 0., 1., 0.5),
            resonance: bounded(p.resonance, 0., 1., 0.5),
            velocity,
            rate,
            frequency,
            age: 0.,
            carrier: 0.,
            modulator: 0.,
            last_ratio: 1.,
        };
        let c = v.character;
        let r = v.resonance;
        let f = frequency;
        let velocity_value = velocity;
        match p.instrument {
            Instrument::UprightPiano => {
                let stiffness = 0.00005 + pow(f / 1000.0, 2.0) * 0.0004;
                let strings = if f < 110.0 { 2 } else { 3 };
                let bright = 0.35 + 1.3 * c + 1.1 * velocity_value;
                for n in 1..=20 {
                    let h = (n) as f64;
                    for d in 0..strings {
                        let detune = ((d) as f64 - (strings - 1) as f64 / 2.0) * (0.6 + 1.6 * r);
                        v.add(
                            h * sqrt((1.0 + stiffness * h * h) / (1.0 + stiffness))
                                * pow(2.0, detune / 1200.0),
                            1.15 * exp(-h / (2.0 + 2.6 * bright)) / pow(h, 0.7) / (strings) as f64,
                            (0.55 + 3.6 * r) / pow(h, 0.75)
                                * pow(440.0 / f, 0.25)
                                * (1.0 + 0.15 * (d) as f64),
                        );
                    }
                }
                v.burst(0.05 + 0.10 * velocity_value, 0.012, 650.0, false);
            }
            Instrument::AcousticGuitar => {
                let p = 0.08 + 0.22 * c;
                for n in 1..=32 {
                    let h = (n) as f64;
                    v.add(
                        h * sqrt(1.0 + 0.00002 * h * h),
                        1.4 * sin(PI * h * p) / pow(h, 0.95)
                            * exp(-h / (8.0 + 16.0 * velocity_value)),
                        (0.9 + 3.0 * r) / pow(h, 0.65) * pow(330.0 / f, 0.15),
                    );
                }
                v.add(102.0 / f, 0.16 * velocity_value, 0.16);
                v.add(205.0 / f, 0.10 * velocity_value, 0.11);
                v.add(390.0 / f, 0.07 * velocity_value, 0.07);
                v.add(580.0 / f, 0.04 * velocity_value, 0.05);
                v.burst(0.05 + 0.07 * velocity_value, 0.004, 2500.0, true);
            }
            Instrument::UprightBass => {
                let p = 0.18 + 0.12 * c;
                for n in 1..=18 {
                    let h = (n) as f64;
                    v.add(
                        h,
                        1.25 * sin(PI * h * p) / pow(h, 1.05)
                            * exp(-h / (2.5 + 4.0 * velocity_value + 5.0 * c)),
                        (0.5 + 2.2 * r) / pow(h, 0.7),
                    );
                }
                v.add(72.0 / f, 0.2 * velocity_value, 0.2);
                v.add(145.0 / f, 0.09 * velocity_value, 0.12);
                v.burst(0.07 + 0.08 * velocity_value, 0.018, 300.0, false);
            }
            Instrument::Vibraphone => {
                v.add(1.0, 1.0, 1.6 + 4.0 * r);
                v.add(
                    3.932,
                    0.18 + 0.3 * velocity_value,
                    (1.6 + 4.0 * r) / pow(3.932, 0.6),
                );
                v.add(
                    9.538,
                    0.06 + 0.08 * velocity_value,
                    (1.6 + 4.0 * r) / pow(9.538, 0.6),
                );
                v.add(16.8, 0.02, (1.6 + 4.0 * r) / pow(16.8, 0.6));
                v.tremolo_depth = 0.08 + 0.42 * c;
                v.burst(0.01 + 0.03 * velocity_value, 0.003, 3500.0, false);
            }
            Instrument::WoodenFlute => {
                for n in 1..=12 {
                    let h = (n) as f64;
                    let gain = if n == 1 {
                        1.0
                    } else {
                        (0.35 + 0.45 * c) / pow(h, 1.7)
                    };
                    v.add(h, gain * (if n % 2 == 0 { 0.8 } else { 1.0 }), 0.0);
                }
                v.breath(0.05 + 0.16 * r, f * 1.2, min(f * 7.0, 9000.0));
                v.vibrato_rate = 5.1;
                v.vibrato_cents = 11.0;
                v.vibrato_delay = 0.3;
                v.burst(0.10 + 0.06 * velocity_value, 0.025, 2600.0, false);
            }
            Instrument::SoloCello => {
                for n in 1..=40 {
                    let h = (n) as f64;
                    let hz = f * h;
                    let weight = 0.18
                        + exp(-0.5 * pow((hz - 280.0) / 90.0, 2.0))
                        + 0.8 * exp(-0.5 * pow((hz - 560.0) / 170.0, 2.0))
                        + 0.55 * exp(-0.5 * pow((hz - 1150.0) / 320.0, 2.0))
                        + 0.3 * exp(-0.5 * pow((hz - 2500.0) / 600.0, 2.0));
                    v.add(h, 0.8 * weight / pow(h, 1.05 - 0.45 * c), 0.0);
                }
                v.breath(0.012 + 0.03 * r, 1400.0, 5200.0);
                v.vibrato_rate = 5.4;
                v.vibrato_cents = 14.0;
                v.vibrato_delay = 0.28;
                v.burst(0.05 + 0.05 * velocity_value, 0.030, 1800.0, false);
            }
            Instrument::ConcertHarp => {
                let p = 0.32 + 0.12 * c;
                for n in 1..=20 {
                    let h = (n) as f64;
                    v.add(
                        h,
                        1.2 * sin(PI * h * p) / pow(h, 1.15)
                            * exp(-h / (6.0 + 9.0 * velocity_value)),
                        (1.3 + 4.2 * r) / pow(h, 0.55) * pow(220.0 / f, 0.2),
                    );
                }
                v.burst(0.03 + 0.03 * velocity_value, 0.006, 1400.0, false);
            }
            Instrument::Kalimba => {
                v.add(1.0, 1.0, 0.8 + 2.6 * r);
                v.add(
                    5.93,
                    0.3 * (0.4 + 0.6 * velocity_value) * (0.5 + c),
                    (0.8 + 2.6 * r) / pow(5.93, 0.7),
                );
                v.add(
                    15.3,
                    0.07 * velocity_value,
                    (0.8 + 2.6 * r) / pow(15.3, 0.7),
                );
                v.add(310.0 / f, 0.06 * velocity_value, 0.05);
                v.burst(0.01 + 0.04 * velocity_value, 0.0025, 4200.0, true);
            }
            Instrument::StudioPiano => {
                let stiffness = 0.00004 + (frequency / 1000.).powi(2) * 0.00035;
                for n in 1..=24 {
                    let h = n as f64;
                    let ratio = h * ((1. + stiffness * h * h) / (1. + stiffness)).sqrt();
                    let brightness = 0.65 + c * 1.6 + velocity * 1.3;
                    let hammer = (-h / (2.5 + brightness * 3.)).exp();
                    v.add(
                        ratio,
                        1.25 * hammer / h.powf(0.65),
                        (0.7 + r * 4.5) / h.powf(0.7) * (440. / frequency).powf(0.22),
                    );
                }
            }
            Instrument::NylonStrings => {
                let position = 0.12 + c * 0.34;
                for n in 1..=24 {
                    let h = n as f64;
                    v.add(
                        h * (1. + 0.000015 * h * h).sqrt(),
                        (PI * h * position).sin() / h.powf(1.12),
                        (0.65 + r * 2.7) / h.powf(0.7),
                    );
                }
            }
            Instrument::Marimba => {
                for (i, ratio) in [1_f64, 4., 9.2, 16., 25.3].into_iter().enumerate() {
                    v.add(
                        ratio,
                        [1.2, 0.35, 0.17, 0.08, 0.035][i]
                            * (0.2 + 0.8 * velocity * c).powf(i as f64 * 0.45),
                        (0.22 + r * 1.5) / ratio.powf(0.55),
                    );
                }
            }
            Instrument::GlassBells => {
                for (i, ratio) in [1_f64, 2.01, 2.76, 4.07, 5.43, 6.81, 8.93, 11.2]
                    .into_iter()
                    .enumerate()
                {
                    v.add(
                        ratio * (1. + c * i as f64 * 0.0008),
                        0.9 / (i as f64 + 1.).powf(1.1),
                        (0.45 + r * 3.5) / ratio.powf(0.35),
                    );
                }
            }
            Instrument::TonewheelOrgan => {
                for (i, ratio) in [0.5, 1., 1.5, 2., 3., 4., 5., 6., 8.]
                    .into_iter()
                    .enumerate()
                {
                    v.add(
                        ratio,
                        [0.3, 1., 0.25, 0.4, 0.23, 0.18, 0.12, 0.08, 0.05][i]
                            * (if i < 2 { 1. } else { 0.2 + c * 1.5 })
                            * 0.55,
                        0.,
                    );
                }
                v.add(3., 0.22 * velocity, 0.045 + r * 0.15);
            }
            Instrument::StringEnsemble => {
                for detune in [-1., 0., 1.] {
                    for n in 1..=12 {
                        let h = n as f64;
                        v.add(
                            h * 2_f64.powf(detune * (3. + c * 16.) / 1200.),
                            0.3 / h.powf(1.8 - r * 0.8),
                            0.,
                        );
                    }
                }
            }
            Instrument::ElectricGuitar => {
                for n in 1..=32 {
                    let h = n as f64;
                    let pickup = (PI * h * (0.06 + c * 0.22)).sin();
                    v.add(
                        h * (1. + 0.000008 * h * h).sqrt(),
                        1.6 * pickup * (-h / (5. + velocity * 12.)).exp() / h.powf(0.9),
                        (0.45 + r * 3.2) / h.powf(0.6),
                    );
                }
            }
            Instrument::PickedBass => {
                for n in 1..=24 {
                    let h = n as f64;
                    v.add(
                        h * (1. + 0.000004 * h * h).sqrt(),
                        1.15 * (-h / (2.5 + c * velocity * 14.)).exp() / h.powf(1.25),
                        (0.3 + r * 2.6) / h.powf(0.65),
                    );
                }
            }
            Instrument::VocalEnsemble => {
                let f1 = if c < 0.5 {
                    350. + c * 900.
                } else {
                    800. - (c - 0.5) * 980.
                };
                let f2 = if c < 0.5 {
                    750. + c * 800.
                } else {
                    1150. + (c - 0.5) * 2200.
                };
                let width = 100. + r * 140.;
                for detune in [-1., 1.] {
                    for n in 1..=64 {
                        let h = n as f64;
                        let hz = frequency * h;
                        let formant = |center: f64, bandwidth: f64| {
                            (-0.5 * ((hz - center) / bandwidth).powi(2)).exp()
                        };
                        let throat = 0.12
                            + 1.2 * formant(f1, width)
                            + 0.85 * formant(f2, width * 1.5)
                            + 0.35 * formant(2700., width * 2.);
                        v.add(
                            h * 2_f64.powf(detune * (2. + r * 5.) / 1200.),
                            throat * 0.38 / h.powf(0.65),
                            0.,
                        );
                    }
                }
            }
            _ => {}
        }
        v
    }
    fn add(&mut self, ratio: f64, gain: f64, decay: f64) {
        let hz = self.frequency * ratio;
        if hz >= self.rate * 0.43 {
            return;
        }
        let radians = 2. * PI * hz / self.rate;
        self.modes.push(Mode {
            sine: 0.,
            cosine: 1.,
            rotation_sine: radians.sin(),
            rotation_cosine: radians.cos(),
            radians,
            amplitude: gain,
            damping: if decay > 0. {
                (-1. / (decay * self.rate)).exp()
            } else {
                1.
            },
        });
    }
    fn burst(&mut self, gain: f64, tau: f64, cutoff: f64, high: bool) {
        self.burst_gain = gain;
        self.burst_decay = (-1. / (tau * self.rate)).exp();
        self.burst_filter = 1. - (-2. * PI * cutoff.min(self.rate * 0.43) / self.rate).exp();
        self.burst_high = high;
    }
    fn breath(&mut self, gain: f64, low: f64, high: f64) {
        self.breath_gain = gain;
        self.breath_low = 1. - (-2. * PI * low.min(self.rate * 0.43) / self.rate).exp();
        self.breath_high = 1. - (-2. * PI * high.min(self.rate * 0.43) / self.rate).exp();
    }
    fn next(&mut self, ratio: f64) -> f64 {
        self.age += 1.;
        if self.kind == Instrument::TinePiano {
            let t = self.age / self.rate;
            let index = ((self.rate * 0.4 / self.frequency - 1.).max(0.) / 2.).min(4.)
                * self.character
                * self.velocity
                * (-t / 0.35).exp();
            self.carrier += 2. * PI * self.frequency * ratio / self.rate;
            self.modulator += 2. * PI * self.frequency * 2. * ratio / self.rate;
            self.carrier %= 2. * PI;
            self.modulator %= 2. * PI;
            return ((self.carrier + index * self.modulator.sin()).sin()
                + if 3. * self.frequency * ratio < self.rate * 0.43 {
                    0.12 * (3. * self.carrier).sin() * (-t / 0.12).exp()
                } else {
                    0.
                })
                * (-t / (0.7 + self.resonance * 4.5)).exp();
        }
        if self.vibrato_cents > 0. && (self.age as u64) & 31 == 0 {
            let t = self.age / self.rate;
            self.vibrato_ratio = 2_f64.powf(
                (2. * PI * self.vibrato_rate * t).sin()
                    * self.vibrato_cents
                    * ((t - self.vibrato_delay) / 0.3).clamp(0., 1.)
                    / 1200.,
            );
        }
        let ratio = ratio * self.vibrato_ratio;
        if ratio != self.last_ratio {
            for mode in &mut self.modes {
                mode.rotation_sine = (mode.radians * ratio).sin();
                mode.rotation_cosine = (mode.radians * ratio).cos();
            }
            self.last_ratio = ratio;
        }
        let mut result: f64 = self.modes.iter_mut().map(Mode::next).sum();
        if self.burst_gain > 0. || self.breath_gain > 0. {
            self.random ^= self.random << 13;
            self.random ^= self.random >> 7;
            self.random ^= self.random << 17;
            let noise = (self.random & 0xFFFF) as f64 / 32767.5 - 1.;
            self.burst_lp += self.burst_filter * (noise - self.burst_lp);
            result += (if self.burst_high {
                noise - self.burst_lp
            } else {
                self.burst_lp
            }) * self.burst_gain;
            self.burst_gain *= self.burst_decay;
            self.breath_lp1 += self.breath_high * (noise - self.breath_lp1);
            self.breath_lp2 += self.breath_low * (noise - self.breath_lp2);
            result += (self.breath_lp1 - self.breath_lp2) * self.breath_gain;
        }
        if self.tremolo_depth > 0. {
            result *= 1.
                - self.tremolo_depth * (0.5 - 0.5 * (2. * PI * 5.5 * self.age / self.rate).cos());
        }
        result
    }
}
fn wavetable(p: &Pattern, frequency: f64, rate: f64, waveform: Waveform) -> Vec<f64> {
    let size = 2048;
    let harmonics = ((rate * 0.45 / frequency).floor() as usize).min(64);
    let mut table = vec![0.; size];
    for h in 1..=harmonics {
        let mut cosine = 0.;
        let coefficient = match waveform {
            Waveform::Sine => {
                if h == 1 {
                    1.
                } else {
                    0.
                }
            }
            Waveform::Triangle => {
                if h % 2 == 1 {
                    (if h % 4 == 1 { 1. } else { -1. }) * 8. / (PI * PI * (h * h) as f64)
                } else {
                    0.
                }
            }
            Waveform::Saw => -2. / (PI * h as f64),
            Waveform::Pulse => {
                let width = bounded(p.pulse_width, 0.05, 0.95, 0.5);
                cosine = 2. * (2. * PI * h as f64 * width).sin() / (PI * h as f64);
                2. * (1. - (2. * PI * h as f64 * width).cos()) / (PI * h as f64)
            }
        };
        if coefficient == 0. && cosine == 0. {
            continue;
        }
        for (i, v) in table.iter_mut().enumerate() {
            let angle = 2. * PI * (h * i) as f64 / size as f64;
            *v += coefficient * angle.sin() + cosine * angle.cos();
        }
    }
    table
}
#[derive(Clone)]
pub struct Voice {
    pre_delay: usize,
    pending_note_off: bool,
    minimum_gate: usize,
    pub pitch: i32,
    table: Vec<f64>,
    second_table: Vec<f64>,
    step: f64,
    second_step: f64,
    mix: f64,
    sub: f64,
    noise: f64,
    attack: f64,
    decay: f64,
    sustain: f64,
    release: f64,
    filter: f64,
    velocity: f64,
    drive: f64,
    phase: f64,
    second_phase: f64,
    sub_phase: f64,
    filtered: f64,
    age: f64,
    release_age: f64,
    release_level: f64,
    gated: bool,
    musical: Option<MusicalVoice>,
    /// The recorded instrument, when installed (replaces the model).
    sampled: Option<crate::sampled_instruments::Note>,
    /// The instrument generated from measurements (instrument designer).
    generated: Option<crate::measured_instruments::ModelNote>,
    motion: Motion,
    random: u64,
    rate: f64,
    smoothing: f64,
    target_bend: f64,
    bend: f64,
    target_mod: f64,
    modulation: f64,
    target_expression: f64,
    expression: f64,
}
impl Voice {
    pub fn new(pitch: i32, velocity: f64, p: &Pattern, rate: f64, beat: f64) -> Self {
        let pitch = pitch.clamp(0, 127);
        let rate = bounded(rate, 8000., 192000., 48000.);
        let velocity = bounded(velocity, 0., 1., 0.);
        let (tone, character, attack) = p.organic.color(p, pitch, beat);
        let tune = bounded(p.fine_tune, -100., 100., 0.);
        let frequency = 440.
            * 2_f64.powf((pitch - 69) as f64 / 12.)
            * if tune == 0. {
                1.
            } else {
                2_f64.powf(tune / 1200.)
            };
        let second = frequency * 2_f64.powf(bounded(p.detune, -50., 50., 0.) / 1200.);
        let mix = bounded(p.oscillator_mix, 0., 1., 0.);
        let synth = p.instrument == Instrument::Synthesizer;
        let tone = bounded(tone, 0., 1., 0.65);
        let cutoff = (250. * 64_f64.powf(tone)).min(rate * 0.45);
        // Alternate takes follow the note's position in the pattern.
        let robin = ((beat.max(0.) * 4.).round() as u64).wrapping_mul(31).wrapping_add(pitch as u64);
        let generated = match (synth, p.instrument_shape) {
            (false, Some(shape)) => crate::measured_instruments::ModelNote::new(p.instrument, pitch, velocity, robin, tune, rate, shape, true),
            _ => None,
        };
        let sampled = if synth || generated.is_some() {
            None
        } else {
            crate::sampled_instruments::Note::new(p.instrument, pitch, velocity, robin, tune, rate)
        };
        let recorded_release = sampled.as_ref().map(|n| n.release).or(generated.as_ref().map(|n| n.release));
        Self {
            pre_delay: 0,
            pending_note_off: false,
            minimum_gate: 0,
            pitch,
            table: if synth {
                wavetable(p, frequency * 2_f64.powf(2.25 / 12.), rate, p.waveform)
            } else {
                vec![]
            },
            second_table: if synth && mix > 0. {
                wavetable(p, second * 2_f64.powf(2.25 / 12.), rate, p.second_waveform)
            } else {
                vec![]
            },
            step: frequency / rate,
            second_step: second / rate,
            mix,
            sub: bounded(p.sub_level, 0., 1., 0.),
            noise: bounded(p.noise_level, 0., 1., 0.),
            attack: bounded(attack, 0., 2., 0.01) * rate,
            decay: bounded(p.decay, 0., 2., 0.1) * rate,
            sustain: bounded(p.sustain, 0., 1., 1.),
            release: recorded_release.unwrap_or_else(|| bounded(p.release, 0., 2., 0.15)) * rate,
            filter: 1. - (-2. * PI * cutoff / rate).exp(),
            velocity,
            drive: bounded(p.drive, 1., 8., 1.),
            phase: 0.,
            second_phase: 0.,
            sub_phase: 0.,
            filtered: 0.,
            age: 0.,
            release_age: 0.,
            release_level: 0.,
            gated: true,
            musical: if synth || sampled.is_some() || generated.is_some() {
                None
            } else {
                Some(MusicalVoice::new(p, frequency, velocity, rate, character))
            },
            sampled,
            generated,
            motion: Motion::new(&p.organic, pitch, beat, rate),
            random: (pitch as u64).wrapping_add(0x9E3779B97F4A7C15),
            rate,
            smoothing: 1. - (-1. / (0.005 * rate)).exp(),
            target_bend: 0.,
            bend: 0.,
            target_mod: 0.,
            modulation: 0.,
            target_expression: 1.,
            expression: 1.,
        }
    }
    pub fn with_delay(mut self, seconds: f64) -> Self {
        self.pre_delay = (bounded(seconds, 0., 0.5, 0.) * self.rate).round() as usize;
        self
    }
    pub fn controller(&mut self, kind: ControllerKind, value: f64, immediate: bool) {
        let value = bounded(
            value,
            0.,
            1.,
            match kind {
                ControllerKind::Expression => 1.,
                ControllerKind::PitchBend => 0.5,
                _ => 0.,
            },
        );
        match kind {
            ControllerKind::PitchBend => self.target_bend = (value - 0.5) * 4.,
            ControllerKind::Modulation => self.target_mod = value,
            ControllerKind::Expression => self.target_expression = value,
            _ => {}
        }
        if immediate {
            self.bend = self.target_bend;
            self.modulation = self.target_mod;
            self.expression = self.target_expression;
        }
    }
    fn envelope(&self) -> f64 {
        if self.attack > 0. && self.age < self.attack {
            self.age / self.attack
        } else if self.decay > 0. && self.age < self.attack + self.decay {
            1. - (1. - self.sustain) * (self.age - self.attack) / self.decay
        } else {
            self.sustain
        }
    }
    /// Release time in seconds (recorded/generated instruments use their own).
    pub fn release_seconds(&self) -> f64 {
        self.release / self.rate
    }
    pub fn note_off(&mut self) {
        if self.pre_delay > 0 {
            self.pending_note_off = true;
            return;
        }
        if self.gated {
            self.release_level = self.envelope();
            self.gated = false;
            self.release_age = 0.;
        }
    }
    pub fn is_finished(&self) -> bool {
        self.pre_delay == 0
            && ((!self.gated && self.release_age >= self.release)
                || self.sampled.as_ref().is_some_and(|n| n.finished)
                || self.generated.as_ref().is_some_and(|n| n.finished))
    }
    pub fn render_next(&mut self) -> f32 {
        if self.pre_delay > 0 {
            self.pre_delay -= 1;
            if self.pre_delay == 0 && self.pending_note_off {
                self.minimum_gate = (self.rate * 0.01).round().max(1.) as usize;
            }
            return 0.;
        }
        if self.minimum_gate > 0 {
            self.minimum_gate -= 1;
            if self.minimum_gate == 0 {
                self.note_off();
            }
        }
        if self.is_finished() {
            return 0.;
        }
        self.bend += (self.target_bend - self.bend) * self.smoothing;
        self.modulation += (self.target_mod - self.modulation) * self.smoothing;
        self.expression += (self.target_expression - self.expression) * self.smoothing;
        let bend = if self.bend == 0. && self.modulation == 0. {
            1.
        } else {
            2_f64.powf(
                (self.bend + (2. * PI * self.age / self.rate * 5.).sin() * self.modulation * 0.25)
                    / 12.,
            )
        };
        if let Some(note) = &mut self.generated {
            // Generated note: its own attack, decay and release.
            if !self.gated {
                note.note_off();
            }
            let value = note.next(bend * self.motion.ratio);
            self.age += 1.;
            if !self.gated {
                self.release_age += 1.;
            }
            let result = (value * self.expression * self.motion.gain) as f32;
            self.motion.advance();
            return result;
        }
        if let Some(note) = &mut self.sampled {
            // Recorded note: its own attack and decay; a linear release fade.
            let value = note.next(bend * self.motion.ratio);
            let gain = if self.gated {
                1.
            } else {
                (1. - self.release_age / self.release.max(1.)).max(0.)
            };
            self.age += 1.;
            if !self.gated {
                self.release_age += 1.;
            }
            let result = (value * gain * self.expression * self.motion.gain) as f32;
            self.motion.advance();
            return result;
        }
        let mut sample = if let Some(v) = &mut self.musical {
            v.next(bend * self.motion.ratio)
        } else {
            lookup(&self.table, self.phase) * (1. - self.mix)
        };
        if self.musical.is_none() {
            if self.mix > 0. {
                sample += lookup(&self.second_table, self.second_phase) * self.mix;
            }
            if self.sub > 0. {
                sample += (2. * PI * self.sub_phase).sin() * self.sub * 0.5;
            }
            if self.noise > 0. {
                self.random ^= self.random << 13;
                self.random ^= self.random >> 7;
                self.random ^= self.random << 17;
                sample += ((self.random & 0xFFFF) as f64 / 32767.5 - 1.) * self.noise * 0.35;
            }
        }
        self.filtered += self.filter * (sample - self.filtered);
        let gain = if self.gated {
            self.envelope()
        } else if self.release > 0. {
            self.release_level * (1. - self.release_age / self.release).max(0.)
        } else {
            0.
        };
        let driven = if self.drive == 1. {
            self.filtered
        } else {
            (self.filtered * self.drive).tanh() / self.drive.tanh()
        };
        self.phase += self.step * bend * self.motion.ratio;
        self.phase -= self.phase.floor();
        self.second_phase += self.second_step * bend * self.motion.ratio;
        self.second_phase -= self.second_phase.floor();
        self.sub_phase += self.step * 0.5 * bend * self.motion.ratio;
        self.sub_phase -= self.sub_phase.floor();
        self.age += 1.;
        if !self.gated {
            self.release_age += 1.;
        }
        let result =
            (driven * gain * self.velocity * 0.35 * self.expression * self.motion.gain) as f32;
        self.motion.advance();
        result
    }
}
fn lookup(table: &[f64], phase: f64) -> f64 {
    let position = phase * table.len() as f64;
    let index = position as usize;
    let fraction = position - index as f64;
    table[index] * (1. - fraction) + table[(index + 1) % table.len()] * fraction
}
struct Cursor {
    events: Vec<(usize, ControllerKind, f64)>,
    index: usize,
    bend: f64,
    modulation: f64,
    expression: f64,
}
impl Cursor {
    fn new(p: &Pattern, rate: f64) -> Self {
        let bpm = bounded(p.bpm, 30., 300., 120.);
        let mut events = p
            .controllers
            .iter()
            .take(4096)
            .filter(|e| {
                e.beat.is_finite() && e.beat >= 0. && e.beat <= p.beats && e.value.is_finite()
            })
            .map(|e| {
                (
                    (e.beat * 60. / bpm * rate).round() as usize,
                    e.kind,
                    e.value.clamp(0., 1.),
                )
            })
            .collect::<Vec<_>>();
        events.sort_by_key(|e| e.0);
        Self {
            events,
            index: 0,
            bend: 0.,
            modulation: 0.,
            expression: 1.,
        }
    }
    fn advance(&mut self, frame: usize) {
        while self.index < self.events.len() && self.events[self.index].0 <= frame {
            let (_, kind, value) = self.events[self.index];
            match kind {
                ControllerKind::PitchBend => self.bend = (value - 0.5) * 4.,
                ControllerKind::Modulation => self.modulation = value,
                ControllerKind::Expression => self.expression = value,
                _ => {}
            }
            self.index += 1;
        }
    }
    fn apply(&self, voice: &mut Voice, immediate: bool) {
        voice.controller(ControllerKind::PitchBend, self.bend / 4. + 0.5, immediate);
        voice.controller(ControllerKind::Modulation, self.modulation, immediate);
        voice.controller(ControllerKind::Expression, self.expression, immediate);
    }
}
impl Pattern {
    pub fn performed_notes(&self) -> Vec<Note> {
        self.performed_notes_up_to(usize::MAX, true)
    }
    pub(crate) fn performed_notes_up_to(&self, maximum: usize, sustain: bool) -> Vec<Note> {
        let mut pedal = self
            .controllers
            .iter()
            .take(4096)
            .filter(|c| {
                sustain
                    && c.kind == ControllerKind::Sustain
                    && c.beat.is_finite()
                    && c.value.is_finite()
                    && c.beat >= 0.
                    && c.beat <= self.beats
            })
            .collect::<Vec<_>>();
        pedal.sort_by(|a, b| a.beat.total_cmp(&b.beat));
        self.notes
            .iter()
            .take(maximum)
            .map(|n| {
                let mut n = n.clone();
                match n.articulation {
                    Some(Articulation::Staccato) => n.duration_beats *= 0.45,
                    Some(Articulation::Tenuto) => n.duration_beats *= 0.95,
                    Some(Articulation::Accent) => n.velocity = (n.velocity * 1.25).min(1.),
                    _ => {}
                }
                let end = n.start_beat + n.duration_beats;
                if let Some(last) = pedal.iter().rev().find(|c| c.beat <= end) {
                    if last.value >= 0.5 {
                        let release = pedal
                            .iter()
                            .find(|c| c.beat > end && c.value < 0.5)
                            .map(|c| c.beat)
                            .unwrap_or(self.beats);
                        n.duration_beats = release.min(self.beats) - n.start_beat;
                    }
                }
                n
            })
            .collect()
    }
    pub fn render(&self, rate: f64, cancelled: &AtomicBool) -> Result<Vec<Vec<f32>>> {
        self.render_internal(rate, true, cancelled)
    }
    fn render_internal(
        &self,
        rate: f64,
        ceiling: bool,
        cancelled: &AtomicBool,
    ) -> Result<Vec<Vec<f32>>> {
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::Invalid("Designer render cancelled".into()));
        }
        if self.notes.len() > 1_000_000
            || self.controllers.len() > 65_536
            || self.layers.len() > 256
        {
            return Err(Error::Invalid(
                "Designer model exceeds worker limits".into(),
            ));
        }
        let rate = bounded(rate, 8000., 192000., 48000.);
        let tempo = bounded(self.bpm, 30., 300., 120.);
        let length = bounded(self.beats, 0.25, 64., 4.);
        if !self.layers.is_empty() || self.base_gain != 1. || self.base_pan != 0. {
            let mut base = self.clone();
            base.layers.clear();
            base.base_gain = 1.;
            base.base_pan = 0.;
            let release = self
                .layers
                .iter()
                .take(6)
                .filter(|l| l.enabled)
                .map(|l| l.instrument(self).release + l.onset_delay())
                .fold(bounded(self.release, 0., 2., 0.15), f64::max);
            let count = ((length * 60. / tempo + release) * rate).round() as usize;
            if count > crate::drums::MAX_RENDER_FRAMES {
                return Err(Error::Invalid(
                    "Designer render exceeds bounded PCM worker budget".into(),
                ));
            }
            let mut output = vec![vec![0.; count], vec![0.; count]];
            let mut add = |audio: Vec<Vec<f32>>, gain: f64, pan: f64, delay: f64| {
                let angle = (bounded(pan, -1., 1., -1.) + 1.) * PI / 4.;
                let level = bounded(gain, 0., 1., 0.);
                let left = (angle.cos() * level) as f32;
                let right = (angle.sin() * level) as f32;
                let offset = (delay * rate).round() as usize;
                for (i, &v) in audio[0]
                    .iter()
                    .take(count.saturating_sub(offset))
                    .enumerate()
                {
                    output[0][i + offset] += v * left;
                    output[1][i + offset] += v * right;
                }
            };
            if self.base_gain > 0. {
                add(
                    base.render_internal(rate, false, cancelled)?,
                    self.base_gain,
                    self.base_pan,
                    0.,
                );
            }
            for layer in self
                .layers
                .iter()
                .take(6)
                .filter(|l| l.enabled && l.gain > 0.)
            {
                add(
                    layer
                        .instrument(self)
                        .render_internal(rate, false, cancelled)?,
                    layer.gain,
                    layer.pan,
                    layer.onset_delay(),
                );
            }
            for channel in &mut output {
                for chunk in channel.chunks_mut(8192) {
                    if cancelled.load(Ordering::Relaxed) {
                        return Err(Error::Invalid("Designer render cancelled".into()));
                    }
                    for v in chunk {
                        *v = (*v as f64).tanh() as f32;
                    }
                }
            }
            return Ok(output);
        }
        let tail = bounded(
            self.release,
            0.,
            2.,
            if self.instrument == Instrument::Synthesizer {
                0.15
            } else {
                0.25
            },
        );
        // Recorded and generated instruments release over their own measured
        // time; size the tail for it so no note is cut off mid-release.
        let probe = (self.instrument != Instrument::Synthesizer).then(|| Voice::new(60, 0.8, self, rate, 0.));
        let tail = probe.as_ref().map_or(tail, |voice| tail.max(voice.release_seconds()));
        // Recorded and generated instruments get a real instrument's polyphony
        // (a strummed chord rings under the next); the modeled voices keep the
        // eight voices the Swift engine renders with.
        let realistic = probe.as_ref().is_some_and(|voice| voice.sampled.is_some() || voice.generated.is_some());
        let polyphony = if realistic { 32 } else { 8 };
        let frames = ((length * 60. / tempo + tail) * rate).round() as usize;
        if frames > crate::drums::MAX_RENDER_FRAMES {
            return Err(Error::Invalid(
                "Designer render exceeds bounded PCM worker budget".into(),
            ));
        }
        let mut output = vec![0.; frames];
        let mut ends = Vec::with_capacity(8);
        let mut notes = self
            .performed_notes_up_to(256, true)
            .into_iter()
            .filter(|n| n.start_beat.is_finite() && n.duration_beats.is_finite())
            .collect::<Vec<_>>();
        notes.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat));
        for note in notes {
            if cancelled.load(Ordering::Relaxed) {
                return Err(Error::Invalid("Designer render cancelled".into()));
            }
            if note.start_beat < 0. || note.start_beat >= length || note.duration_beats <= 0. {
                continue;
            }
            let velocity = bounded(note.velocity, 0., 1., 0.);
            if self.instrument == Instrument::Synthesizer && velocity <= 0. {
                continue;
            }
            let start = (note.start_beat * 60. / tempo * rate).round() as usize;
            ends.retain(|&e| e > start);
            if ends.len() >= polyphony {
                continue;
            }
            let gate = (note.duration_beats.min(length - note.start_beat) * 60. / tempo * rate)
                .round()
                .max(1.) as usize;
            // `tail` covers the instrument's own release, so no note is cut
            // off mid-release.
            let end = frames.min(start + gate + (tail * rate).round() as usize);
            ends.push(end);
            let mut voice = Voice::new(note.pitch, note.velocity, self, rate, note.start_beat);
            if self.instrument == Instrument::Synthesizer {
                voice.random = voice.random.wrapping_add(start as u64);
            }
            let mut controls = Cursor::new(self, rate);
            controls.advance(start);
            controls.apply(&mut voice, true);
            for (frame, out) in output.iter_mut().enumerate().take(end).skip(start) {
                if frame & 4095 == 0 && cancelled.load(Ordering::Relaxed) {
                    return Err(Error::Invalid("Designer render cancelled".into()));
                }
                if frame == start + gate {
                    voice.note_off();
                }
                controls.advance(frame);
                controls.apply(&mut voice, false);
                *out += voice.render_next();
            }
        }
        if ceiling {
            for v in &mut output {
                *v = (*v as f64).tanh() as f32;
            }
        }
        Ok(vec![output])
    }
    pub fn render_loop(&self, cancelled: &AtomicBool) -> Result<Vec<Vec<f32>>> {
        let mut audio = self.render(48000., cancelled)?;
        let frames = (bounded(self.beats, 0.25, 64., 4.) * 60. / bounded(self.bpm, 30., 300., 120.)
            * 48000.)
            .round()
            .max(1.) as usize;
        for channel in &mut audio {
            for f in frames..channel.len() {
                channel[f % frames] += channel[f];
            }
            channel.resize(frames, 0.);
        }
        Ok(audio)
    }
}

/// Prepared gain/pan metadata travels with a voice. Construction and retirement
/// belong to a serialized control worker, outside a hardware callback.
pub struct PreparedVoice {
    pub voice: Voice,
    pub key_pitch: i32,
    left: f32,
    right: f32,
    level: f32,
    /// Remaining samples of a 5 ms steal fade (None while playing normally).
    stealing: Option<u32>,
}
impl PreparedVoice {
    pub fn new(voice: Voice, key_pitch: i32, gain: f64, pan: f64, stereo: bool) -> Self {
        let level = bounded(gain, 0., 1., 0.);
        let pan = bounded(pan, -1., 1., 0.);
        Self {
            voice,
            key_pitch: key_pitch.clamp(0, 127),
            level: level as f32,
            left: (level
                * if stereo {
                    ((pan + 1.) * PI / 4.).cos()
                } else {
                    1.
                }) as f32,
            right: (level
                * if stereo {
                    ((pan + 1.) * PI / 4.).sin()
                } else {
                    1.
                }) as f32,
            stealing: None,
        }
    }
    /// Gain for this sample: 1, or the steal fade; `None` once faded out.
    fn steal_gain(&mut self) -> Option<f32> {
        match &mut self.stealing {
            None => Some(1.),
            Some(0) => None,
            Some(left) => {
                *left -= 1;
                Some(*left as f32 / STEAL_FADE as f32)
            }
        }
    }
    fn faded(&self) -> bool {
        self.stealing == Some(0) || self.voice.is_finished()
    }
}
/// Frames over which a stolen live voice fades out (5 ms at 48 kHz).
const STEAL_FADE: u32 = 240;
/// Bounded live voice bank. Render methods allocate and retire nothing. The owner
/// must serialize controls with rendering and collect finished voices on its worker;
/// this API alone is not a cross-thread command queue or hardware host.
pub struct LiveInstrument {
    voices: Vec<PreparedVoice>,
    maximum: usize,
    held: [bool; 128],
    pending: [bool; 128],
    sustain: bool,
    controllers: [Option<f64>; 3],
}
impl LiveInstrument {
    pub fn new(maximum: usize) -> Self {
        let maximum = maximum.clamp(1, 32);
        Self {
            // Room for voices fading out after being stolen.
            voices: Vec::with_capacity(maximum * 2),
            maximum,
            held: [false; 128],
            pending: [false; 128],
            sustain: false,
            controllers: [None; 3],
        }
    }
    pub fn active_voice_count(&self) -> usize {
        self.voices
            .iter()
            .filter(|v| !v.faded() && v.stealing.is_none())
            .count()
    }
    pub fn add_voice(&mut self, mut prepared: PreparedVoice) {
        self.collect_finished();
        for (index, kind) in [
            ControllerKind::PitchBend,
            ControllerKind::Modulation,
            ControllerKind::Expression,
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(value) = self.controllers[index] {
                prepared.voice.controller(kind, value, true);
            }
        }
        self.held[prepared.key_pitch as usize] = true;
        self.pending[prepared.key_pitch as usize] = false;
        // Steal the oldest sounding voice with a short fade, not a cut.
        let sounding = self.voices.iter().filter(|v| v.stealing.is_none()).count();
        if sounding >= self.maximum {
            if let Some(oldest) = self.voices.iter_mut().find(|v| v.stealing.is_none()) {
                oldest.stealing = Some(STEAL_FADE);
            }
        }
        if self.voices.len() >= self.maximum * 2 {
            self.voices.remove(0);
        }
        self.voices.push(prepared);
    }
    fn release(&mut self, key: usize) {
        for v in &mut self.voices {
            if v.key_pitch as usize == key {
                v.voice.note_off();
            }
        }
    }
    pub fn note_off(&mut self, key: i32) {
        if !(0..=127).contains(&key) {
            return;
        }
        let key = key as usize;
        self.held[key] = false;
        if self.sustain {
            self.pending[key] = true;
        } else {
            self.release(key);
        }
    }
    pub fn controller(&mut self, kind: ControllerKind, value: f64) {
        if kind == ControllerKind::Sustain {
            self.sustain = value >= 0.5;
            if !self.sustain {
                for key in 0..128 {
                    if self.pending[key] {
                        self.release(key);
                        self.pending[key] = false;
                    }
                }
            }
        } else {
            let index = match kind {
                ControllerKind::PitchBend => 0,
                ControllerKind::Modulation => 1,
                _ => 2,
            };
            self.controllers[index] = Some(value);
            for v in &mut self.voices {
                v.voice.controller(kind, value, false);
            }
        }
    }
    pub fn all_notes_off(&mut self) {
        self.sustain = false;
        self.held.fill(false);
        self.pending.fill(false);
        for v in &mut self.voices {
            v.voice.note_off();
        }
    }
    pub fn collect_finished(&mut self) {
        self.voices.retain(|v| !v.faded());
    }
    pub fn reset(&mut self) {
        self.voices.clear();
        self.held.fill(false);
        self.pending.fill(false);
        self.sustain = false;
        self.controllers.fill(None);
    }
    pub fn render_mono(&mut self, output: &mut [f32]) {
        for out in output {
            let mut sum = 0.;
            for v in &mut self.voices {
                if let Some(g) = v.steal_gain() {
                    sum += v.voice.render_next() * v.level * g;
                }
            }
            *out = (sum as f64).tanh() as f32;
        }
    }
    pub fn render_stereo(&mut self, left: &mut [f32], right: &mut [f32]) -> Result<()> {
        if left.len() != right.len() {
            return Err(Error::Invalid("Instrument channel lengths differ".into()));
        }
        for (left, right) in left.iter_mut().zip(right) {
            let mut l = 0.;
            let mut r = 0.;
            for v in &mut self.voices {
                if let Some(g) = v.steal_gain() {
                    let sample = v.voice.render_next() * g;
                    l += sample * v.left;
                    r += sample * v.right;
                }
            }
            *left = (l as f64).tanh() as f32;
            *right = (r as f64).tanh() as f32;
        }
        Ok(())
    }
}
