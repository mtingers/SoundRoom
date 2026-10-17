//! Designer instruments generated from measurements of recorded notes
//! (`examples/analyze_instruments.rs` → `assets/measured-instruments.json`,
//! derived from the CC0 libraries listed in THIRD_PARTY_NOTICES.md).
//!
//! A note blends the two nearest measured notes (pitch, then velocity layer)
//! and synthesizes:
//! * up to 40 partials at k·f0·√(1 + B k²) (B: string stiffness) with their
//!   measured level, attack and two-stage decay — or, for bowed/blown notes,
//!   a held level with the measured slow wobble — plus vibrato,
//! * a 24-band noise residual (hammer, pluck, bow, breath),
//! * shaped by the instrument designer (`InstrumentShape`).
//!
//! It streams sample by sample without allocating, so it also plays live.
use crate::designer::Instrument;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    f64::consts::PI,
    sync::{Arc, OnceLock},
};

const LOUDEST_PEAK: f64 = 0.7;
const BLOCK: usize = 32;

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Partial {
    /// Frequency / f0 (harmonics with stiffness, or measured bar modes).
    #[serde(default)]
    ratio: f64,
    db: f64,
    #[serde(default)]
    start_db: f64,
    attack: f64,
    fast: f64,
    slow: f64,
    slow_db: f64,
    sustain_db: Option<f64>,
    wobble: Option<f64>,
}
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Band {
    db: f64,
    fast: f64,
    slow: f64,
    slow_db: f64,
    sustain_db: Option<f64>,
}
#[derive(Deserialize, Clone, Copy)]
struct Vibrato {
    depth: f64,
    rate: f64,
    onset: f64,
}
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Measured {
    pitch: i32,
    velocity: f64,
    held_out: bool,
    b: f64,
    peak_db: f64,
    partials: Vec<Partial>,
    noise: Vec<Band>,
    vibrato: Option<Vibrato>,
    sustained: bool,
}
#[derive(Deserialize)]
struct Model {
    release: f64,
    notes: Vec<Measured>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Models {
    band_edges: Vec<f64>,
    instruments: BTreeMap<String, Model>,
}
struct Prepared {
    model: Model,
    norm: f64,
}
/// Noise band edges and the prepared model per instrument id.
type Loaded = (Vec<f64>, BTreeMap<String, Arc<Prepared>>);
fn models() -> Option<&'static Loaded> {
    static MODELS: OnceLock<Option<Loaded>> = OnceLock::new();
    MODELS
        .get_or_init(|| {
            let parsed: Models =
                serde_json::from_str(include_str!("../assets/measured-instruments.json")).ok()?;
            let prepared = parsed
                .instruments
                .into_iter()
                .map(|(id, model)| {
                    let top = model.notes.iter().map(|n| n.velocity).fold(0., f64::max);
                    let mut loud: Vec<f64> = model
                        .notes
                        .iter()
                        .filter(|n| (n.velocity - top).abs() < 1e-9)
                        .map(|n| 10f64.powf(n.peak_db / 20.))
                        .collect();
                    loud.sort_by(f64::total_cmp);
                    let typical = loud
                        .get(loud.len() / 2)
                        .copied()
                        .filter(|v| *v > 1e-9)
                        .unwrap_or(1.);
                    (
                        id,
                        Arc::new(Prepared {
                            model,
                            norm: LOUDEST_PEAK / typical,
                        }),
                    )
                })
                .collect();
            Some((parsed.band_edges, prepared))
        })
        .as_ref()
}
/// True when `instrument` has a measured model.
pub fn available(instrument: Instrument) -> bool {
    instrument
        .recording()
        .and_then(|id| models()?.1.get(id))
        .is_some()
}

/// Instrument-designer adjustments on top of the measured model; the default
/// is the measured instrument unchanged.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InstrumentShape {
    /// Upper harmonics, -1 (dark) … 1 (bright): ±4 dB per harmonic octave.
    pub brightness: f64,
    /// Multiplies every decay time (struck/plucked) and the release.
    pub sustain: f64,
    /// Attack noise (hammer, pick, bow, breath), dB.
    pub noise: f64,
    /// Attack softness: 0 as measured … 1 slow swell.
    pub softness: f64,
    /// String stiffness (inharmonicity) multiplier.
    pub stiffness: f64,
    /// Vibrato depth in cents added to the measured vibrato.
    pub vibrato: f64,
    /// Vibrato rate (Hz) when adding vibrato.
    pub vibrato_rate: f64,
    /// Body resonance: low harmonics (below 600 Hz), dB.
    pub body: f64,
    /// Harmonic detune spread in cents (chorus-like richness).
    pub detune: f64,
}
impl Default for InstrumentShape {
    fn default() -> Self {
        Self {
            brightness: 0.,
            sustain: 1.,
            noise: 0.,
            softness: 0.,
            stiffness: 1.,
            vibrato: 0.,
            vibrato_rate: 5.5,
            body: 0.,
            detune: 0.,
        }
    }
}
impl InstrumentShape {
    pub fn sanitized(self) -> Self {
        let c = |v: f64, lo: f64, hi: f64, d: f64| if v.is_finite() { v.clamp(lo, hi) } else { d };
        Self {
            brightness: c(self.brightness, -1., 1., 0.),
            sustain: c(self.sustain, 0.2, 4., 1.),
            noise: c(self.noise, -36., 12., 0.),
            softness: c(self.softness, 0., 1., 0.),
            stiffness: c(self.stiffness, 0., 4., 1.),
            vibrato: c(self.vibrato, 0., 60., 0.),
            vibrato_rate: c(self.vibrato_rate, 2., 9., 5.5),
            body: c(self.body, -18., 12., 0.),
            detune: c(self.detune, 0., 25., 0.),
        }
    }
}

/// Measured parameters blended for one pitch and velocity.
fn blend(model: &Model, pitch: f64, velocity: f64, include_held_out: bool) -> Option<Measured> {
    let notes: Vec<&Measured> = model
        .notes
        .iter()
        .filter(|n| include_held_out || !n.held_out)
        .collect();
    let mut pitches: Vec<i32> = notes.iter().map(|n| n.pitch).collect();
    pitches.sort();
    pitches.dedup();
    let below = pitches
        .iter()
        .rev()
        .find(|p| (**p as f64) <= pitch)
        .or(pitches.first())
        .copied()?;
    let above = pitches
        .iter()
        .find(|p| (**p as f64) >= pitch)
        .or(pitches.last())
        .copied()?;
    let at_pitch = |p: i32| -> Option<Measured> {
        let mut layer: Vec<&&Measured> = notes.iter().filter(|n| n.pitch == p).collect();
        layer.sort_by(|a, b| a.velocity.total_cmp(&b.velocity));
        let lower = layer
            .iter()
            .rev()
            .find(|n| n.velocity <= velocity)
            .or(layer.first())
            .copied()?;
        let upper = layer
            .iter()
            .find(|n| n.velocity >= velocity)
            .or(layer.last())
            .copied()?;
        let t = if upper.velocity > lower.velocity {
            ((velocity - lower.velocity) / (upper.velocity - lower.velocity)).clamp(0., 1.)
        } else {
            0.
        };
        Some(mix(lower, upper, t))
    };
    let (a, b) = (at_pitch(below)?, at_pitch(above)?);
    let t = if above > below {
        ((pitch - below as f64) / (above - below) as f64).clamp(0., 1.)
    } else {
        0.
    };
    Some(mix(&a, &b, t))
}
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}
fn lerp_ln(a: f64, b: f64, t: f64) -> f64 {
    (lerp(a.max(1e-4).ln(), b.max(1e-4).ln(), t)).exp()
}
fn mix(a: &Measured, b: &Measured, t: f64) -> Measured {
    let count = a.partials.len().max(b.partials.len());
    let silent = Partial {
        ratio: 0.,
        db: -120.,
        start_db: 0.,
        attack: 0.,
        fast: 0.1,
        slow: 0.1,
        slow_db: -120.,
        sustain_db: None,
        wobble: None,
    };
    let partials = (0..count)
        .map(|k| {
            let (p, q) = (
                a.partials.get(k).unwrap_or(&silent),
                b.partials.get(k).unwrap_or(&silent),
            );
            Partial {
                ratio: if p.ratio > 0. && q.ratio > 0. {
                    lerp(p.ratio, q.ratio, t)
                } else {
                    p.ratio.max(q.ratio)
                },
                db: lerp(p.db, q.db, t),
                start_db: lerp(p.start_db, q.start_db, t),
                attack: lerp(p.attack, q.attack, t),
                fast: lerp_ln(p.fast, q.fast, t),
                slow: lerp_ln(p.slow, q.slow, t),
                slow_db: lerp(p.slow_db, q.slow_db, t),
                sustain_db: match (p.sustain_db, q.sustain_db) {
                    (Some(x), Some(y)) => Some(lerp(x, y, t)),
                    (x, y) => x.or(y),
                },
                wobble: match (p.wobble, q.wobble) {
                    (Some(x), Some(y)) => Some(lerp(x, y, t)),
                    (x, y) => x.or(y),
                },
            }
        })
        .collect();
    let noise = a
        .noise
        .iter()
        .zip(&b.noise)
        .map(|(p, q)| Band {
            db: lerp(p.db, q.db, t),
            fast: lerp_ln(p.fast, q.fast, t),
            slow: lerp_ln(p.slow, q.slow, t),
            slow_db: lerp(p.slow_db, q.slow_db, t),
            sustain_db: match (p.sustain_db, q.sustain_db) {
                (Some(x), Some(y)) => Some(lerp(x, y, t)),
                (x, y) => x.or(y),
            },
        })
        .collect();
    let vibrato = match (a.vibrato, b.vibrato) {
        (Some(x), Some(y)) => Some(Vibrato {
            depth: lerp(x.depth, y.depth, t),
            rate: lerp(x.rate, y.rate, t),
            onset: lerp(x.onset, y.onset, t),
        }),
        (x, y) => x.or(y),
    };
    Measured {
        pitch: a.pitch,
        velocity: lerp(a.velocity, b.velocity, t),
        held_out: false,
        b: lerp(a.b, b.b, t),
        peak_db: lerp(a.peak_db, b.peak_db, t),
        partials,
        noise,
        vibrato,
        sustained: a.sustained || b.sustained,
    }
}

#[derive(Clone)]
struct Osc {
    hz: f64,
    sin: f64,
    cos: f64,
    rs: f64,
    rc: f64,
    level: f64,
    attack: f64,
    /// Amplitude at the end of the initial 15 ms onset (1 = peak).
    start: f64,
    e1: f64,
    e2: f64,
    d1: f64,
    d2: f64,
    sustain: Option<f64>,
    wobble: f64,
    wobble_phase: f64,
    wobble_rate: f64,
}
#[derive(Clone)]
struct NoiseBand {
    level: f64,
    e1: f64,
    e2: f64,
    d1: f64,
    d2: f64,
    sustain: Option<f64>,
    filter: [f64; 5],
    state: [f64; 4],
    norm: f64,
}

/// One generated note.
#[derive(Clone)]
pub struct ModelNote {
    oscs: Vec<Osc>,
    bands: Vec<NoiseBand>,
    rate: f64,
    age: usize,
    vibrato: Option<Vibrato>,
    gain: f64,
    pub release: f64,
    releasing: Option<usize>,
    rng: u64,
    pub finished: bool,
    bend_seen: f64,
    detune: f64,
}
impl ModelNote {
    /// Generated note for `instrument`; `include_held_out` is false only for
    /// validation against held-out recordings.
    #[allow(clippy::too_many_arguments)] // note identity plus designer shape
    pub fn new(
        instrument: Instrument,
        pitch: i32,
        velocity: f64,
        robin: u64,
        cents: f64,
        rate: f64,
        shape: InstrumentShape,
        include_held_out: bool,
    ) -> Option<Self> {
        let (edges, prepared) = models()?;
        let prepared = prepared.get(instrument.recording()?)?;
        let shape = shape.sanitized();
        let m = blend(
            &prepared.model,
            pitch as f64,
            velocity.clamp(0., 1.),
            include_held_out,
        )?;
        let f0 = 440. * 2f64.powf((pitch as f64 - 69. + cents / 100.) / 12.);
        let b = m.b * shape.stiffness;
        let mut rng = robin.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ pitch as u64 | 1;
        let mut uniform = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng >> 11) as f64 / (1u64 << 53) as f64
        };
        let peak = 10f64.powf(m.peak_db / 20.);
        let soft = shape.softness;
        let oscs: Vec<Osc> = m
            .partials
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let k = (i + 1) as f64;
                // Stiffness scales the measured stretch; bar modes keep their ratio.
                let harmonic = (p.ratio - k).abs() < 0.5 * k;
                let ratio = if p.ratio <= 0. || (harmonic && m.b > 0.) {
                    k * (1. + b * k * k).sqrt()
                } else {
                    p.ratio
                };
                let hz = ratio * f0;
                let k = ratio.max(1.);
                if hz >= rate * 0.45 || p.db < -90. {
                    return None;
                }
                let tilt = 10f64.powf(shape.brightness * 4. * k.log2() / 20.);
                let body = if hz < 600. {
                    10f64.powf(shape.body / 20.)
                } else {
                    1.
                };
                let level = peak * 10f64.powf(p.db / 20.) * tilt * body;
                let (fast, slow) = (p.fast * shape.sustain, p.slow.max(p.fast) * shape.sustain);
                let phase = if m.sustained { uniform() * 2. * PI } else { 0. };
                let (s, c) = phase.sin_cos();
                Some(Osc {
                    hz,
                    sin: s,
                    cos: c,
                    rs: 0.,
                    rc: 1.,
                    level,
                    // Bowed/blown notes may swell for a second or more.
                    attack: (p.attack.min(if m.sustained { 5. } else { 0.4 }) + soft * 0.25)
                        .max(0.0015)
                        * rate,
                    start: 10f64.powf(p.start_db.min(0.) / 20.) * (1. - soft * 0.8),
                    e1: 1.,
                    e2: 10f64.powf(p.slow_db / 10.),
                    d1: 10f64.powf(-6. / (fast.max(0.005) * rate)),
                    d2: 10f64.powf(-6. / (slow.max(0.005) * rate)),
                    sustain: p.sustain_db.map(|s| 10f64.powf((s - p.db).min(0.) / 20.)),
                    wobble: p.wobble.unwrap_or(0.).min(4.) / 8.686,
                    wobble_phase: uniform() * 2. * PI,
                    wobble_rate: 0.6 + 2.4 * uniform(),
                })
            })
            .collect();
        let noise_gain = 10f64.powf(shape.noise / 20.);
        let bands: Vec<NoiseBand> = m
            .noise
            .iter()
            .enumerate()
            .filter(|(_, n)| n.db > -90.)
            .map(|(bi, n)| {
                let (lo, hi) = (edges[bi], edges[bi + 1].min(rate * 0.45));
                let centre = (lo * hi).sqrt();
                let octaves = (hi / lo).log2().max(0.05);
                let w0 = 2. * PI * centre / rate;
                let alpha = w0.sin() * ((2f64).ln() / 2. * octaves * w0 / w0.sin()).sinh();
                let a0 = 1. + alpha;
                let filter = [
                    alpha / a0,
                    0.,
                    -alpha / a0,
                    -2. * w0.cos() / a0,
                    (1. - alpha) / a0,
                ];
                // White noise through one band-pass: variance ≈ bandwidth share.
                let share = ((hi - lo) / (rate / 2.)).max(1e-6);
                NoiseBand {
                    level: peak * 10f64.powf(n.db / 20.) * noise_gain,
                    e1: 1.,
                    e2: 10f64.powf(n.slow_db / 10.),
                    d1: 10f64.powf(-6. / (n.fast.max(0.005) * shape.sustain * rate)),
                    d2: 10f64.powf(-6. / (n.slow.max(n.fast).max(0.005) * shape.sustain * rate)),
                    sustain: n.sustain_db.map(|s| 10f64.powf((s - n.db).min(0.) / 20.)),
                    filter,
                    state: [0.; 4],
                    norm: 1. / (share.sqrt() * 0.75),
                }
            })
            .collect();
        let mut vibrato = m.vibrato;
        if shape.vibrato > 0. {
            let base = vibrato.unwrap_or(Vibrato {
                depth: 0.,
                rate: shape.vibrato_rate,
                onset: 0.25,
            });
            vibrato = Some(Vibrato {
                depth: base.depth + shape.vibrato,
                rate: if base.depth < 2. {
                    shape.vibrato_rate
                } else {
                    base.rate
                },
                onset: base.onset.max(0.2),
            });
        }
        if !m.sustained {
            vibrato = vibrato.filter(|v| v.depth > 3.);
        }
        Some(Self {
            oscs,
            bands,
            rate,
            age: 0,
            vibrato,
            gain: prepared.norm,
            release: prepared.model.release * shape.sustain.sqrt(),
            releasing: None,
            rng: rng | 1,
            finished: false,
            bend_seen: 0.,
            detune: shape.detune,
        })
    }
    pub fn note_off(&mut self) {
        if self.releasing.is_none() {
            self.releasing = Some(self.age);
        }
    }
    /// Next sample at the pitch-bend ratio.
    pub fn next(&mut self, bend: f64) -> f64 {
        if self.finished {
            return 0.;
        }
        let t = self.age as f64 / self.rate;
        if self.age % BLOCK == 0 || bend != self.bend_seen {
            self.bend_seen = bend;
            let vib = self.vibrato.map_or(1., |v| {
                let ramp = ((t - v.onset) / 0.3).clamp(0., 1.);
                2f64.powf(v.depth * ramp * (2. * PI * v.rate * t).sin() / 1200.)
            });
            for (i, o) in self.oscs.iter_mut().enumerate() {
                // Detune spread alternates sharp/flat by harmonic.
                let spread = if self.detune > 0. {
                    2f64.powf(
                        self.detune
                            * if i % 2 == 0 { 0.5 } else { -0.5 }
                            * ((i as f64 + 1.).ln() / 3.7)
                            / 1200.,
                    )
                } else {
                    1.
                };
                let (s, c) = (2. * PI * o.hz * bend * vib * spread / self.rate).sin_cos();
                o.rs = s;
                o.rc = c;
            }
        }
        let release_gain = match self.releasing {
            Some(start) => {
                let r = 1. - (self.age - start) as f64 / (self.release * self.rate).max(1.);
                if r <= 0. {
                    self.finished = true;
                    return 0.;
                }
                r
            }
            None => 1.,
        };
        let mut sum = 0.;
        for o in &mut self.oscs {
            (o.sin, o.cos) = (o.sin * o.rc + o.cos * o.rs, o.cos * o.rc - o.sin * o.rs);
            // 15 ms onset to the start level, then a dB-linear swell to the
            // peak at the measured attack time.
            let onset = 0.015 * self.rate;
            let age = self.age as f64;
            let rise = if o.attack <= onset {
                // Attack within the onset: a plain fade-in to the peak.
                (age / onset).min(1.)
            } else if age < onset {
                age / onset * o.start
            } else if age < o.attack {
                o.start * (1. / o.start.max(1e-3)).powf((age - onset) / (o.attack - onset).max(1.))
            } else {
                1.
            };
            let envelope = match o.sustain {
                // Sustained: the swell above rises to the steady level.
                Some(level) => {
                    let wob = 1. + o.wobble * (o.wobble_phase + 2. * PI * o.wobble_rate * t).sin();
                    level * wob
                }
                _ => {
                    let e = (o.e1 + o.e2).sqrt();
                    o.e1 *= o.d1;
                    o.e2 *= o.d2;
                    e
                }
            };
            sum += o.sin * o.level * rise * envelope;
        }
        for n in &mut self.bands {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            let x = (self.rng >> 11) as f64 / (1u64 << 52) as f64 - 1.;
            let [b0, _, b2, a1, a2] = n.filter;
            let y = b0 * x + b2 * n.state[1] - a1 * n.state[2] - a2 * n.state[3];
            n.state = [x, n.state[0], y, n.state[2]];
            let envelope = match n.sustain {
                Some(level) => {
                    let settle = (t / 0.15).min(1.);
                    1. + (level - 1.) * settle
                }
                None => {
                    let e = (n.e1 + n.e2).sqrt();
                    n.e1 *= n.d1;
                    n.e2 *= n.d2;
                    e
                }
            };
            // 3 ms fade-in: the noise must not start with a step.
            let onset = (self.age as f64 / (0.003 * self.rate)).min(1.);
            sum += y * n.norm * n.level * envelope * onset;
        }
        self.age += 1;
        // Decaying notes end once inaudible.
        if self.age % 4800 == 0
            && self
                .oscs
                .iter()
                .all(|o| o.sustain.is_none() && (o.e1 + o.e2) < 1e-7)
        {
            self.finished = true;
        }
        sum * self.gain * release_gain
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn noise_band_normalization_gives_unit_rms() {
        use std::f64::consts::PI;
        let rate = 48_000.;
        for (lo, hi) in [(300., 400.), (2000., 2600.), (9000., 12000.)] {
            let (lo, hi): (f64, f64) = (lo, hi);
            let centre = (lo * hi).sqrt();
            let octaves = (hi / lo).log2();
            let w0 = 2. * PI * centre / rate;
            let alpha = w0.sin() * ((2f64).ln() / 2. * octaves * w0 / w0.sin()).sinh();
            let a0 = 1. + alpha;
            let (b0, b2, a1, a2) = (
                alpha / a0,
                -alpha / a0,
                -2. * w0.cos() / a0,
                (1. - alpha) / a0,
            );
            let share: f64 = (hi - lo) / (rate / 2.);
            let norm = 1. / (share.sqrt() * 0.75);
            let mut rng: u64 = 0x1234_5678_9ABC_DEF1;
            let (mut x1, mut x2, mut y1, mut y2) = (0., 0., 0., 0.);
            let mut sum = 0.;
            for _ in 0..200_000 {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                let x = (rng >> 11) as f64 / (1u64 << 52) as f64 - 1.;
                let y = b0 * x + b2 * x2 - a1 * y1 - a2 * y2;
                (x2, x1, y2, y1) = (x1, x, y1, y);
                sum += (y * norm).powi(2);
            }
            let rms = (sum / 200_000.).sqrt();
            assert!((rms - 1.).abs() < 0.2, "{lo}-{hi}: {rms}");
        }
    }
}
