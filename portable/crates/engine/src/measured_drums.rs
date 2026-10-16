//! Drum hits generated from measurements of real recorded hits
//! (`examples/analyze_drum_kit.rs`, data in `assets/measured-drums.json`,
//! derived from DRSKit 2.1 by DrumGizmo.org, CC BY 4.0).
//!
//! A hit is the sum of
//! * resonant modes: measured frequency, level and T60, each a linear
//!   function of velocity, with the measured early pitch glide (head
//!   tension) on drums,
//! * 24 log-spaced noise bands: measured level, a fast decay over the first
//!   20 dB and a quieter slow tail (stick, wires, cymbal wash),
//! * the room: band-shaped decaying noise with the measured per-band T60,
//!   delay and level relative to the direct sound.
//!
//! Round robins perturb every parameter by its measured hit-to-hit spread.
use crate::{
    drums::{Articulation, Kit, Voice},
    spectral::{band_noise, fft},
    Error, Result,
};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    f64::consts::PI,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
};

const RATE: f64 = 48_000.;
/// Level-matches the modeled kit to the recorded natural flavor.
const OUTPUT_GAIN: f64 = 0.8;
const CACHE_BYTES: usize = 256 * 1024 * 1024;

/// `[a, b, spread]`: value = a + b·velocity, spread = residual std. dev.
type Line = [f64; 3];
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Mode {
    hz: Line,
    db: Line,
    ln_fast: Line,
    ln_slow: Line,
    slow_db: Line,
    presence: f64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Band {
    db: Line,
    ln_fast: Line,
    ln_slow: Line,
    slow_db: Line,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Model {
    peak_db: Line,
    glide: Line,
    glide_time: Line,
    modes: Vec<Mode>,
    noise: Vec<Band>,
    room: Line,
    /// ln(natural-mix level / direct level) against velocity.
    mix_gain: Line,
    room_delay: f64,
    room_t60: Vec<f64>,
    /// Seconds until a cymbal's ring peaks (energy cascading upward).
    #[serde(default)]
    bloom: Line,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Models {
    band_edges: Vec<f64>,
    models: BTreeMap<String, Model>,
}
/// Kit-designer adjustments for one piece, applied on top of its measured
/// model. The default is the measured kit unchanged.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Shape {
    /// Head tension in semitones: moves the resonances, keeps the timing.
    pub tension: f64,
    /// Multiplies every decay time.
    pub sustain: f64,
    /// Resonant body (the drum tone), dB.
    pub body: f64,
    /// Stick/beater contact click, dB.
    pub snap: f64,
    /// Noise: snare wires, cymbal wash, head rasp, dB.
    pub wires: f64,
    /// Spectral tilt, -1 (dark) … 1 (bright): ±4 dB per octave about 1 kHz.
    pub tone: f64,
    /// Multiplies the pitch drop after the strike.
    pub pitch_drop: f64,
    /// Room level, dB.
    pub room: f64,
    /// Multiplies room decay and pre-delay.
    pub room_size: f64,
    /// Multiplies the round-robin hit-to-hit variation.
    pub variation: f64,
}
impl Default for Shape {
    fn default() -> Self {
        Self { tension: 0., sustain: 1., body: 0., snap: 0., wires: 0., tone: 0., pitch_drop: 1., room: 0., room_size: 1., variation: 1. }
    }
}
impl Shape {
    /// Finite values within each control's range.
    pub fn sanitized(self) -> Self {
        let c = |v: f64, lo: f64, hi: f64, d: f64| if v.is_finite() { v.clamp(lo, hi) } else { d };
        Self {
            tension: c(self.tension, -12., 12., 0.),
            sustain: c(self.sustain, 0.25, 3., 1.),
            body: c(self.body, -24., 12., 0.),
            snap: c(self.snap, -24., 12., 0.),
            // -60 dB: snare wires off.
            wires: c(self.wires, -60., 12., 0.),
            tone: c(self.tone, -1., 1., 0.),
            pitch_drop: c(self.pitch_drop, 0., 3., 1.),
            room: c(self.room, -36., 9., 0.),
            room_size: c(self.room_size, 0.4, 2.5, 1.),
            variation: c(self.variation, 0., 2.5, 1.),
        }
    }
    /// Bit pattern identifying these settings in render caches.
    pub fn cache_key(self) -> [u64; 10] {
        [self.tension, self.sustain, self.body, self.snap, self.wires, self.tone, self.pitch_drop, self.room, self.room_size, self.variation].map(f64::to_bits)
    }
    /// Gain of the tone tilt at `hz`.
    fn tilt(self, hz: f64) -> f64 {
        10f64.powf(self.tone * 4. * (hz.max(20.) / 1000.).log2() / 20.)
    }
}
fn db_gain(db: f64) -> f64 {
    10f64.powf(db / 20.)
}
fn models() -> Option<&'static Models> {
    static MODELS: OnceLock<Option<Models>> = OnceLock::new();
    MODELS
        .get_or_init(|| serde_json::from_str(include_str!("../assets/measured-drums.json")).ok())
        .as_ref()
}
type Cached = Arc<[Vec<f32>; 2]>;
/// (voice, articulation, velocity/32, robin, damping/20, shape)
type HitKey = (Voice, Articulation, i32, i32, i32, [u64; 10]);
type HitCache = Mutex<(BTreeMap<HitKey, Cached>, usize)>;
fn cache() -> &'static HitCache {
    static CACHE: OnceLock<HitCache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Deterministic standard-normal draws for one hit.
struct Draws(u64);
impl Draws {
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    fn normal(&mut self) -> f64 {
        let (u, v) = (self.uniform().max(1e-12), self.uniform());
        (-2. * u.ln()).sqrt() * (2. * PI * v).cos()
    }
}
/// A line at `v`, nudged by `variation` × its measured spread (at most
/// `limit` either way, so a noisy fit cannot produce an outlier).
fn eval_limited(line: &Line, v: f64, draws: &mut Draws, variation: f64, limit: f64) -> f64 {
    let v = v.clamp(0., 1.);
    line[0] + line[1] * v + (line[2] * variation * draws.normal().clamp(-2., 2.)).clamp(-limit, limit)
}
/// Level lines (dB): round robins vary by at most ±3 dB.
fn eval(line: &Line, v: f64, draws: &mut Draws, variation: f64) -> f64 {
    eval_limited(line, v, draws, variation, 3.)
}
/// Log-time lines: at most ±35 %.
fn eval_ln(line: &Line, v: f64, draws: &mut Draws, variation: f64) -> f64 {
    eval_limited(line, v, draws, variation, 0.3)
}
/// Kit position (drummer's perspective).
fn position(voice: Voice) -> f64 {
    match voice {
        Voice::Kick => 0.,
        Voice::Snare | Voice::Clap => -0.08,
        Voice::ClosedHat | Voice::OpenHat => -0.5,
        Voice::HighTom => -0.15,
        Voice::LowTom => 0.45,
        Voice::Ride => 0.5,
        Voice::Crash => -0.4,
    }
}
fn cymbal(voice: Voice) -> bool {
    matches!(voice, Voice::ClosedHat | Voice::OpenHat | Voice::Ride | Voice::Crash)
}

/// Mono direct sound of one hit.
#[allow(clippy::too_many_arguments)] // mirrors drums::render_voice plus model and shape
fn direct(model: &Model, edges: &[f64], voice: Voice, v: f64, robin: i32, damping: f64, shape: Shape, cancelled: &AtomicBool) -> Result<Vec<f32>> {
    let mut draws = Draws(0x9E37_79B9_7F4A_7C15 ^ ((voice as u64) << 40) ^ (robin as u64).wrapping_mul(0xD1B5_4A32_D192_ED03) | 1);
    // Robin 0 is the measured mean; others vary by the measured spread.
    let variation = if robin == 0 { 0. } else { 0.8 * shape.variation };
    let peak_db = eval(&model.peak_db, v, &mut draws, variation);
    let ring = if damping > 0.5 { 1. - (damping - 0.5) * 1.2 } else { 1. } * shape.sustain;
    let tension = 2f64.powf(shape.tension / 12.);
    let glide = if cymbal(voice) { 0. } else { (eval(&model.glide, v, &mut draws, variation) * shape.pitch_drop).clamp(0., 0.6) };
    let glide_time = eval_ln(&model.glide_time, v, &mut draws, variation).clamp(0.003, 0.2);
    // (Hz, level, fast T60, slow T60, slow-stage dB, phase)
    let modes: Vec<(f64, f64, f64, f64, f64, f64)> = model
        .modes
        .iter()
        .filter(|m| m.presence >= 0.5)
        .map(|m| {
            let hz = (m.hz[0] + m.hz[1] * v) * tension;
            let hz = hz * (1. + (0.003 * variation * draws.normal()).clamp(-0.01, 0.01));
            let level = 10f64.powf((peak_db + eval(&m.db, v, &mut draws, variation).min(6.)) / 20.) * db_gain(shape.body) * shape.tilt(hz);
            // Bound the measured times first, then scale by sustain: the slow
            // stage's lower bound must never exceed its upper bound.
            let fast = eval_ln(&m.ln_fast, v, &mut draws, variation).exp().clamp(0.005, 6.);
            let slow = eval_ln(&m.ln_slow, v, &mut draws, variation).exp().clamp(fast, 8.) * ring;
            let fast = fast * ring;
            let slow_db = eval(&m.slow_db, v, &mut draws, variation).clamp(-120., -6.);
            // A stick strike starts every mode together (zero phase); the
            // twin detuning (0.15–0.45 %) gives the beating of split modes.
            let twin = (0.0015 + 0.003 * draws.uniform()) * if draws.uniform() < 0.5 { -1. } else { 1. };
            (hz, level, fast, slow, slow_db, twin)
        })
        .filter(|m| m.0 > 20. && m.0 < 20_000. && m.1 > 1e-6)
        .collect();
    let bands: Vec<(usize, f64, f64, f64, f64)> = model
        .noise
        .iter()
        .enumerate()
        .map(|(b, band)| {
            let centre = (edges[b] * edges[b + 1]).sqrt();
            let level = 10f64.powf((peak_db + eval(&band.db, v, &mut draws, variation).min(6.)) / 20.) * shape.tilt(centre);
            let fast = eval_ln(&band.ln_fast, v, &mut draws, variation).exp().clamp(0.005, 4.);
            let slow = eval_ln(&band.ln_slow, v, &mut draws, variation).exp().clamp(fast, 6.) * ring;
            let fast = fast * ring;
            let slow_db = eval(&band.slow_db, v, &mut draws, variation).clamp(-120., -6.);
            (b, level, fast, slow, slow_db)
        })
        .filter(|b| b.1 > 1e-6)
        .collect();
    // Two-stage envelopes: a fast decay plus a quieter slow tail, summed in power.
    let band_length = |fast: f64, slow: f64, slow_db: f64| {
        // Time until both stages are 60 dB below the strike.
        (fast).max(slow * (1. + slow_db / 60.)).max(0.01)
    };
    // Long enough for the slowest audible component to fall 60 dB, capped.
    let tail = modes
        .iter()
        .map(|m| band_length(m.2, m.3, m.4))
        .chain(bands.iter().map(|b| band_length(b.2, b.3, b.4)))
        .fold(0.05, f64::max)
        .min(4.5);
    let frames = (tail * RATE) as usize;
    let mut out = vec![0f64; frames];
    // Cymbal swell applies to the measured modes as well (see the bands).
    let mode_bloom = if cymbal(voice) { (model.bloom[0] + model.bloom[1] * v).clamp(0., 0.4) } else { 0. };
    let mode_grow = if mode_bloom > 0.005 { (-2.5 / (mode_bloom * RATE)).exp() } else { 1. };
    for (index, &(hz, level, fast, slow, slow_db, twin)) in modes.iter().enumerate() {
        let mut rise = if mode_bloom > 0.005 { 0.8 } else { 0. };
        if index % 4 == 0 && cancelled.load(Ordering::Relaxed) {
            return Err(Error::Invalid("Drum render cancelled".into()));
        }
        // Power envelope as two decaying exponentials, advanced by multiplication.
        let (mut e1, mut e2) = (1., 10f64.powf(slow_db / 10.));
        let (d1, d2) = (10f64.powf(-6. / (fast * RATE)), 10f64.powf(-6. / (slow * RATE)));
        let (mut theta, mut beat) = (0f64, 0f64);
        let length = ((band_length(fast, slow, slow_db) * RATE) as usize).min(frames);
        // Exact phase while the pitch glides; afterwards a rotating phasor
        // (two multiplies per sample instead of two sines).
        let settled = if glide > 0. { ((glide_time * 8. * RATE) as usize).min(length) } else { 0 };
        for (i, o) in out.iter_mut().enumerate().take(settled) {
            let t = i as f64 / RATE;
            let step = 2. * PI * hz * (1. + glide * (-t / glide_time).exp()) / RATE;
            theta += step;
            beat += step * (1. + twin);
            *o += (0.93 * theta.sin() + 0.36 * beat.sin()) * level * ((e1 + e2) * (1. - rise)).sqrt();
            e1 *= d1;
            e2 *= d2;
            rise *= mode_grow;
        }
        let step = 2. * PI * hz / RATE;
        let (rs, rc) = step.sin_cos();
        let (bs, bc) = (step * (1. + twin)).sin_cos();
        let (mut s1, mut c1) = theta.sin_cos();
        let (mut s2, mut c2) = beat.sin_cos();
        for (n, o) in out.iter_mut().enumerate().take(length).skip(settled) {
            (s1, c1) = (s1 * rc + c1 * rs, c1 * rc - s1 * rs);
            (s2, c2) = (s2 * bc + c2 * bs, c2 * bc - s2 * bs);
            // Renormalize occasionally against rounding drift.
            if n & 4095 == 0 {
                let (a, b) = ((s1 * s1 + c1 * c1).sqrt(), (s2 * s2 + c2 * c2).sqrt());
                (s1, c1, s2, c2) = (s1 / a, c1 / a, s2 / b, c2 / b);
            }
            *o += (0.93 * s1 + 0.36 * s2) * level * ((e1 + e2) * (1. - rise)).sqrt();
            e1 *= d1;
            e2 *= d2;
            rise *= mode_grow;
        }
    }
    let wires = db_gain(shape.wires);
    // Contact click: a band-shaped impulse carrying ~1.5 ms of each band's
    // strike level, so the attack is coherent rather than smeared noise.
    let click_energy: f64 = bands.iter().map(|b| b.1 * b.1).sum::<f64>() * 0.0015 * RATE;
    if click_energy > 0. && shape.snap > -24. {
        let click = contact_click(edges, &bands.iter().map(|b| (b.0, b.1)).collect::<Vec<_>>());
        let energy = click.iter().map(|v| v * v).sum::<f64>().max(1e-30);
        let gain = (click_energy / energy).sqrt() * db_gain(shape.snap) * 0.7;
        for (o, c) in out.iter_mut().zip(&click) {
            *o += c * gain;
        }
    }
    // Cymbals: noise only for the chaotic burst after the strike; the ring is
    // a dense cloud of inharmonic partials (plain band noise there sounds
    // like hiss or dither, not shimmer).
    let shimmer = cymbal(voice);
    for &(b, level, fast, slow, slow_db) in &bands {
        let level = level * wires;
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::Invalid("Drum render cancelled".into()));
        }
        let length = (band_length(fast, slow, slow_db) * RATE).min(frames as f64) as usize;
        let noise = filtered_noise(length.max(64), edges[b], edges[b + 1], draws.0 ^ b as u64);
        let (mut e1, mut e2) = (1., 10f64.powf(slow_db / 10.));
        let (d1, d2) = (10f64.powf(-6. / (fast * RATE)), 10f64.powf(-6. / (slow * RATE)));
        // Cymbals: the noise share of the band's power fades over ~40 ms
        // (strike chaos); the partial cloud below carries the rest.
        // A noise bed stays under the ring: thin in the mids, more of the
        // power up top (a crash's wash stays noisy above 8 kHz), and a hard
        // crash stays chaotic longer before it settles into shimmer.
        let centre = (edges[b] * edges[b + 1]).sqrt();
        let bed = (0.12 + 0.25 * ((centre / 4000.).log2() / 2.).clamp(0., 1.)).min(0.4);
        let chaos_time = if voice == Voice::Crash { 0.03 + 0.13 * v } else { 0.04 };
        let (mut chaos, settle) = (1f64, if shimmer { (-1. / (chaos_time * RATE)).exp() } else { 1. });
        // Cymbal swell: the whole band rises to its peak over the measured
        // bloom time (from 20 % power at the strike).
        let bloom = if shimmer { (model.bloom[0] + model.bloom[1] * v).clamp(0., 0.4) } else { 0. };
        // The swell reaches the top bands last (energy cascades upward).
        let band_bloom = bloom * (centre / 2000.).max(0.5).powf(0.3);
        let (mut rise, grow) = if band_bloom > 0.005 { (0.8, (-2.5 / (band_bloom * RATE)).exp()) } else { (0., 1.) };
        for (i, n) in noise.iter().enumerate().take(length) {
            let share = if shimmer { bed + (1. - bed) * chaos } else { 1. };
            out[i] += *n as f64 * level * ((e1 + e2) * share * (1. - rise)).sqrt();
            e1 *= d1;
            e2 *= d2;
            chaos *= settle;
            rise *= grow;
        }
        if shimmer {
            let (lo, hi) = (edges[b], edges[b + 1].min(RATE * 0.45));
            if hi <= lo {
                continue;
            }
            // Measured texture (examples/cymbal_texture.rs): real cymbals are
            // peaky (a few strong partials over many weak ones), their band
            // levels wobble by 1–1.7 dB (beating), and crashes swell for
            // ~0.1–0.16 s after the strike.
            let partials = ((hi - lo) / 50.).round().clamp(3., 32.) as usize;
            let weights: Vec<f64> = (0..partials).map(|_| (0.8 * draws.normal().clamp(-3., 3.)).exp()).collect();
            let total: f64 = weights.iter().map(|w| w * w).sum::<f64>().max(1e-12);
            let base = level * (2. / total).sqrt();
            let swell = (band_bloom / 2.5).max(1e-4);
            for weight in &weights {
                let hz = lo * (hi / lo).powf(draws.uniform());
                // The band's two-stage envelope, each partial a little faster
                // or slower so the texture keeps changing as it decays.
                let spread = 0.75 + 0.5 * draws.uniform();
                let (fd, sd) = (10f64.powf(-6. / (fast * spread * RATE)), 10f64.powf(-6. / (slow * spread * RATE)));
                let (mut p1, mut p2) = (1f64, 10f64.powf(slow_db / 10.));
                let mut chaos = 1f64;
                // A detuned twin (0.5–4 Hz away) makes the partial beat.
                let detune = (0.5 + 5.5 * draws.uniform()) * if draws.uniform() < 0.5 { -1. } else { 1. };
                let twin_level = 0.7 + 0.3 * draws.uniform();
                let norm = 1. / (1. + twin_level * twin_level).sqrt();
                let (rs, rc) = (2. * PI * hz / RATE).sin_cos();
                let (ts, tc) = (2. * PI * (hz + detune) / RATE).sin_cos();
                let (mut sn, mut cs) = (draws.uniform() * 2. * PI).sin_cos();
                let (mut tn, mut tcs) = (draws.uniform() * 2. * PI).sin_cos();
                let a = base * weight * norm;
                let grow = (-1. / (swell * RATE)).exp();
                let mut rise = if band_bloom > 0.005 { 0.8 } else { 0. };
                let n = ((band_length(fast * spread, slow * spread, slow_db) * RATE) as usize).min(frames);
                for (k, o) in out.iter_mut().enumerate().take(n) {
                    (sn, cs) = (sn * rc + cs * rs, cs * rc - sn * rs);
                    (tn, tcs) = (tn * tc + tcs * ts, tcs * tc - tn * ts);
                    if k & 4095 == 0 {
                        let (r1, r2) = ((sn * sn + cs * cs).sqrt(), (tn * tn + tcs * tcs).sqrt());
                        (sn, cs, tn, tcs) = (sn / r1, cs / r1, tn / r2, tcs / r2);
                    }
                    *o += (sn + twin_level * tn) * a * ((p1 + p2) * (1. - bed) * (1. - chaos) * (1. - rise)).sqrt();
                    p1 *= fd;
                    p2 *= sd;
                    chaos *= settle;
                    rise *= grow;
                }
            }
        }
    }
    // 0.3 ms onset ramp: no step at the first sample.
    for (i, o) in out.iter_mut().take(15).enumerate() {
        *o *= i as f64 / 15.;
    }
    Ok(out.into_iter().map(|v| v as f32).collect())
}

/// Zero-phase impulse with the bands' strike levels as its magnitude
/// response, shifted 0.5 ms and windowed to 3 ms so it is causal and short.
fn contact_click(edges: &[f64], bands: &[(usize, f64)]) -> Vec<f64> {
    let n = 4096;
    let (mut re, mut im) = (vec![0.; n], vec![0.; n]);
    let bin = RATE / n as f64;
    for &(b, level) in bands {
        for k in ((edges[b] / bin) as usize).max(1)..((edges[b + 1] / bin) as usize).min(n / 2) {
            re[k] = level;
            re[n - k] = level;
        }
    }
    fft(&mut re, &mut im, true);
    let (lead, tail) = (24usize, 120usize);
    (0..lead + tail)
        .map(|i| {
            let source = re[(i + n - lead) % n];
            let w = if i < lead { 0.5 - 0.5 * (PI * i as f64 / lead as f64).cos() } else { 0.5 + 0.5 * (PI * (i - lead) as f64 / tail as f64).cos() };
            source * w
        })
        .collect()
}
/// Stereo room response for a voice: a few discrete early reflections, then
/// decaying band noise with the measured per-band T60; unit energy per channel.
fn room_ir(model: &Model, edges: &[f64], voice: Voice, size: f64) -> [Vec<f64>; 2] {
    let t60: Vec<f64> = model.room_t60.iter().map(|t| (t * size).clamp(0.15, 4.)).collect();
    let length = (t60.iter().cloned().fold(0.3, f64::max) * 1.1 * RATE) as usize;
    let delay = (model.room_delay.clamp(0.005, 0.06) * size * RATE) as usize;
    let groups = t60.len().max(1);
    let per = edges.len().saturating_sub(1) / groups;
    let mut ir = [vec![0f64; delay + length], vec![0f64; delay + length]];
    for (c, channel) in ir.iter_mut().enumerate() {
        for (g, t) in t60.iter().enumerate() {
            let (lo, hi) = (edges[g * per], edges[((g + 1) * per).min(edges.len() - 1)]);
            let noise = band_noise(length, RATE, lo, hi, (voice as u64) * 131 + (c as u64) * 7919 + g as u64);
            for (i, n) in noise.iter().enumerate() {
                let t_s = i as f64 / RATE;
                // 5 ms build-up, then exponential decay.
                let env = (t_s / 0.005).min(1.) * (-6.907755 * t_s / t).exp();
                channel[delay + i] += *n as f64 * env;
            }
        }
        // Early reflections (floor, walls, ceiling) with ~35 % of the energy.
        let diffuse: f64 = channel.iter().map(|v| v * v).sum();
        let taps = [(3.1, 0.5), (5.3, -0.42), (7.9, 0.36), (11.2, -0.3), (13.7, 0.26), (17.3, -0.22)];
        let norm = (0.35 * diffuse / taps.iter().map(|t| t.1 * t.1).sum::<f64>()).sqrt();
        // Each reflection is a 1 ms diffuse burst, not a single sample, so
        // the taps do not comb-filter the direct sound into a metallic tone.
        let burst = band_noise(48, RATE, 200., 12_000., (voice as u64) * 17 + c as u64);
        let burst_energy = burst.iter().map(|v| (*v as f64).powi(2)).sum::<f64>().max(1e-12);
        for (k, &(ms, g)) in taps.iter().enumerate() {
            let at = ((ms * if c == 0 { 1. } else { 1.07 + 0.01 * k as f64 }) * size * RATE / 1000.) as usize;
            for (j, b) in burst.iter().enumerate() {
                let w = 0.5 - 0.5 * (2. * PI * j as f64 / 48.).cos();
                if let Some(slot) = channel.get_mut(at + j) {
                    *slot += g * norm * *b as f64 * w * (2. / burst_energy).sqrt();
                }
            }
        }
        let energy = channel.iter().map(|v| v * v).sum::<f64>().sqrt();
        if energy > 0. {
            channel.iter_mut().for_each(|v| *v /= energy);
        }
    }
    ir
}
/// Unit-RMS noise in `lo..hi` Hz from two cascaded band-pass biquads
/// (cheap enough to run per hit; normalization measured once per band).
fn filtered_noise(frames: usize, lo: f64, hi: f64, seed: u64) -> Vec<f32> {
    let centre = (lo * hi).sqrt().min(RATE * 0.45);
    let octaves = (hi / lo).log2().max(0.05);
    let w0 = 2. * PI * centre / RATE;
    let alpha = w0.sin() * ((2f64).ln() / 2. * octaves * w0 / w0.sin()).sinh();
    let a0 = 1. + alpha;
    let (b0, b2, a1, a2) = (alpha / a0, -alpha / a0, -2. * w0.cos() / a0, (1. - alpha) / a0);
    let run = |count: usize, mut state: u64| -> Vec<f64> {
        let (mut x1, mut x2, mut y1, mut y2) = (0., 0., 0., 0.);
        let (mut p1, mut p2, mut q1, mut q2) = (0., 0., 0., 0.);
        (0..count)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let x = (state >> 11) as f64 / (1u64 << 52) as f64 - 1.;
                let y = b0 * x + b2 * x2 - a1 * y1 - a2 * y2;
                (x2, x1, y2, y1) = (x1, x, y1, y);
                let z = b0 * y + b2 * p2 - a1 * q1 - a2 * q2;
                (p2, p1, q2, q1) = (p1, y, q1, z);
                z
            })
            .collect()
    };
    type Norms = Mutex<BTreeMap<(u64, u64), f64>>;
    static NORMS: OnceLock<Norms> = OnceLock::new();
    let key = (lo.to_bits(), hi.to_bits());
    // Bind the lookup first: a guard kept alive into the fallback would deadlock.
    let known = NORMS.get_or_init(Default::default).lock().ok().and_then(|n| n.get(&key).copied());
    let norm = known.unwrap_or_else(|| {
        let probe = run(48_000, 0x2545_F491_4F6C_DD1D);
        let rms = (probe[4800..].iter().map(|v| v * v).sum::<f64>() / (probe.len() - 4800) as f64).sqrt();
        let norm = if rms > 0. { 1. / rms } else { 0. };
        if let Ok(mut n) = NORMS.get_or_init(Default::default).lock() {
            n.insert(key, norm);
        }
        norm
    });
    run(frames, seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1).into_iter().map(|v| (v * norm) as f32).collect()
}
/// Room responses depend only on the piece and room size; build each once.
fn cached_room_ir(model: &Model, edges: &[f64], voice: Voice, size: f64) -> Arc<[Vec<f64>; 2]> {
    type Rooms = Mutex<BTreeMap<(Voice, u64), Arc<[Vec<f64>; 2]>>>;
    static ROOMS: OnceLock<Rooms> = OnceLock::new();
    let rooms = ROOMS.get_or_init(Default::default);
    let key = (voice, size.to_bits());
    if let Some(ir) = rooms.lock().ok().and_then(|r| r.get(&key).cloned()) {
        return ir;
    }
    let ir = Arc::new(room_ir(model, edges, voice, size));
    if let Ok(mut r) = rooms.lock() {
        if r.len() > 64 {
            r.clear();
        }
        r.insert(key, ir.clone());
    }
    ir
}
/// Both room channels in one transform: for real `x`, the inverse FFT of
/// X·FFT(hL + i·hR) is (x∗hL) + i·(x∗hR). The packed IR spectrum is cached.
fn convolve_stereo(x: &[f32], ir: &Arc<[Vec<f64>; 2]>, voice: Voice, size: f64) -> [Vec<f64>; 2] {
    let h = ir[0].len().max(ir[1].len());
    let n = (x.len() + h).next_power_of_two();
    type Spectra = Mutex<BTreeMap<(Voice, u64, usize), Arc<(Vec<f64>, Vec<f64>)>>>;
    static SPECTRA: OnceLock<Spectra> = OnceLock::new();
    let key = (voice, size.to_bits(), n);
    let cached = SPECTRA.get_or_init(Default::default).lock().ok().and_then(|m| m.get(&key).cloned());
    let spectrum = cached.unwrap_or_else(|| {
        let (mut re, mut im) = (vec![0.; n], vec![0.; n]);
        re[..ir[0].len()].copy_from_slice(&ir[0]);
        im[..ir[1].len()].copy_from_slice(&ir[1]);
        fft(&mut re, &mut im, false);
        let spectrum = Arc::new((re, im));
        if let Ok(mut m) = SPECTRA.get_or_init(Default::default).lock() {
            if m.len() > 48 {
                m.clear();
            }
            m.insert(key, spectrum.clone());
        }
        spectrum
    });
    let (mut ar, mut ai) = (vec![0.; n], vec![0.; n]);
    ar.iter_mut().zip(x).for_each(|(a, &v)| *a = v as f64);
    fft(&mut ar, &mut ai, false);
    let (hr, hi) = (&spectrum.0, &spectrum.1);
    for k in 0..n {
        let (r, i) = (ar[k] * hr[k] - ai[k] * hi[k], ar[k] * hi[k] + ai[k] * hr[k]);
        ar[k] = r;
        ai[k] = i;
    }
    fft(&mut ar, &mut ai, true);
    let len = x.len() + h - 1;
    let scale = 1. / n as f64;
    [ar[..len].iter().map(|v| v * scale).collect(), ai[..len].iter().map(|v| v * scale).collect()]
}
#[cfg(test)]
fn convolve(x: &[f32], h: &[f64]) -> Vec<f64> {
    let n = (x.len() + h.len()).next_power_of_two();
    let (mut ar, mut ai) = (vec![0.; n], vec![0.; n]);
    let (mut br, mut bi) = (vec![0.; n], vec![0.; n]);
    ar.iter_mut().zip(x).for_each(|(a, &v)| *a = v as f64);
    br[..h.len()].copy_from_slice(h);
    fft(&mut ar, &mut ai, false);
    fft(&mut br, &mut bi, false);
    for k in 0..n {
        let (r, i) = (ar[k] * br[k] - ai[k] * bi[k], ar[k] * bi[k] + ai[k] * br[k]);
        ar[k] = r;
        ai[k] = i;
    }
    fft(&mut ar, &mut ai, true);
    ar.truncate(x.len() + h.len() - 1);
    ar.iter_mut().for_each(|v| *v /= n as f64);
    ar
}

/// Stereo hit for the measured kit, or `None` for other kits / unmeasured
/// voices (the caller then uses the modeled acoustic voice).
#[allow(clippy::too_many_arguments)] // drums::render_voice's inputs plus the shape
pub fn render(
    kit: Kit,
    voice: Voice,
    velocity: f64,
    robin: i32,
    articulation: Articulation,
    damping: f64,
    shape: Option<Shape>,
    cancelled: &AtomicBool,
) -> Result<Option<[Vec<f32>; 2]>> {
    if kit != Kit::StudioModeled || voice == Voice::Clap {
        return Ok(None);
    }
    let Some(models) = models() else { return Ok(None) };
    let velocity = if velocity.is_finite() { velocity.clamp(0., 1.) } else { 0.8 };
    let damping = if damping.is_finite() { damping.clamp(0., 1.) } else { 0.5 };
    // Velocity is quantized to 1/32 so humanized patterns share hits.
    let shape = shape.unwrap_or_default().sanitized();
    let key = (voice, articulation, (velocity * 32.).round() as i32, robin.rem_euclid(8), (damping * 20.).round() as i32, shape.cache_key());
    if let Some(hit) = cache().lock().ok().and_then(|c| c.0.get(&key).cloned()) {
        return Ok(Some((*hit).clone()));
    }
    let v = key.2 as f64 / 32.;
    let layers: Vec<(&str, f64, f64)> = match (voice, articulation) {
        (Voice::Snare, Articulation::Rimshot) => vec![("snare/center", (v * 1.08).min(1.), 1.), ("snare/sideStick", v, 0.9)],
        (Voice::Snare, Articulation::SideStick) => vec![("snare/sideStick", v, 1.)],
        (Voice::Snare, Articulation::Brush) => vec![("snare/brush", v, 1.)],
        (Voice::Ride, Articulation::Bell) => vec![("ride/bell", v, 1.)],
        _ => vec![(voice_key(voice), v, 1.)],
    };
    let mut mono: Vec<f32> = Vec::new();
    let mut room_level = 0.;
    let mut room = None;
    for (name, v, level) in layers {
        let Some(model) = models.models.get(name) else { continue };
        let hit = direct(model, &models.band_edges, voice, v, key.3, damping, shape, cancelled)?;
        if mono.len() < hit.len() {
            mono.resize(hit.len(), 0.);
        }
        mono.iter_mut().zip(&hit).for_each(|(o, h)| *o += h * level as f32);
        if room.is_none() {
            room_level = model.room[0] + model.room[1] * v;
            room = Some(cached_room_ir(model, &models.band_edges, voice, shape.room_size));
        }
    }
    let Some(ir) = room else { return Ok(None) };
    // Scale to the piece's loudness in the recorded natural mix, sharing that
    // energy between the direct sound and the room.
    let model = models.models.get(voice_key(voice)).or_else(|| models.models.values().next());
    let mix_gain = model.map_or(1., |m| (m.mix_gain[0] + m.mix_gain[1] * v.clamp(0., 1.)).exp().clamp(0.1, 40.));
    let pan = position(voice);
    let (pl, pr) = (((pan + 1.) * PI / 4.).cos() * std::f64::consts::SQRT_2, ((pan + 1.) * PI / 4.).sin() * std::f64::consts::SQRT_2);
    // The IR has unit energy, so the room keeps the measured energy ratio.
    let wet = room_level.clamp(0., 1.5) * db_gain(shape.room);
    // Residual calibration against the recorded natural flavor (first-second
    // RMS, `examples/compare_drum_levels.rs`): the closed hat's overhead-only
    // model misses its close-mic level.
    let trim = match voice {
        Voice::ClosedHat => 2.,
        Voice::Crash => 1.4,
        _ => 1.,
    };
    let level = trim * mix_gain / (1. + wet * wet).sqrt();
    let mut out = [Vec::new(), Vec::new()];
    // The room only needs the audible part of the direct sound.
    let peak = mono.iter().fold(0f32, |m, v| m.max(v.abs()));
    let audible = mono.iter().rposition(|v| v.abs() > peak * 3e-4).map_or(1, |i| i + 1);
    let reverbs = convolve_stereo(&mono[..audible], &ir, voice, shape.room_size);
    for (c, channel) in out.iter_mut().enumerate() {
        let reverb = &reverbs[c];
        let gain = if c == 0 { pl } else { pr };
        let mut wet_part = reverb.clone();
        if wet_part.len() < mono.len() {
            wet_part.resize(mono.len(), 0.);
        }
        *channel = wet_part
            .iter()
            .enumerate()
            .map(|(i, r)| ((mono.get(i).copied().unwrap_or(0.) as f64 * gain + r * wet) * level * OUTPUT_GAIN) as f32)
            .collect();
        // Trim the inaudible end of the room tail.
        let floor = 1e-5;
        let end = channel.iter().rposition(|v| v.abs() > floor).map_or(1, |i| i + 1);
        channel.truncate(end);
    }
    let len = out[0].len().max(out[1].len());
    out.iter_mut().for_each(|c| c.resize(len, 0.));
    let hit: Cached = Arc::new(out);
    if let Ok(mut cache) = cache().lock() {
        let bytes = len * 8;
        if cache.1 + bytes > CACHE_BYTES {
            cache.0.clear();
            cache.1 = 0;
        }
        cache.1 += bytes;
        cache.0.insert(key, hit.clone());
    }
    Ok(Some((*hit).clone()))
}
/// One modeled hit a pattern will need.
#[derive(Clone, Copy, Debug)]
pub struct Request {
    pub voice: Voice,
    pub velocity: f64,
    pub robin: i32,
    pub articulation: Articulation,
    pub damping: f64,
    pub shape: Option<Shape>,
}
/// Renders the distinct requests not yet cached, in parallel.
pub fn prefetch(requests: &[Request], cancelled: &AtomicBool) -> Result<()> {
    let mut seen = std::collections::BTreeSet::new();
    let pending: Vec<Request> = requests
        .iter()
        .filter(|r| {
            let key = (r.voice, r.articulation, (r.velocity.clamp(0., 1.) * 32.).round() as i32, r.robin.rem_euclid(8), (r.damping.clamp(0., 1.) * 20.).round() as i32, r.shape.unwrap_or_default().sanitized().cache_key());
            let cached = cache().lock().map(|c| c.0.contains_key(&key)).unwrap_or(false);
            !cached && seen.insert(key)
        })
        .copied()
        .collect();
    if pending.len() < 2 {
        return Ok(());
    }
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 8);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let failure = Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..workers.min(pending.len()) {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(r) = pending.get(i) else { break };
                if cancelled.load(Ordering::Relaxed) {
                    break;
                }
                if let Err(error) = render(Kit::StudioModeled, r.voice, r.velocity, r.robin, r.articulation, r.damping, r.shape, cancelled) {
                    if let Ok(mut f) = failure.lock() {
                        f.get_or_insert(error);
                    }
                    break;
                }
            });
        }
    });
    match failure.into_inner().ok().flatten() {
        Some(error) => Err(error),
        None if cancelled.load(Ordering::Relaxed) => Err(Error::Invalid("Drum render cancelled".into())),
        None => Ok(()),
    }
}
fn voice_key(voice: Voice) -> &'static str {
    match voice {
        Voice::Kick => "kick/center",
        Voice::Snare | Voice::Clap => "snare/center",
        Voice::ClosedHat => "closedHat/center",
        Voice::OpenHat => "openHat/center",
        Voice::LowTom => "lowTom/center",
        Voice::HighTom => "highTom/center",
        Voice::Ride => "ride/center",
        Voice::Crash => "crash/center",
    }
}
/// Mono direct sound only, for validation against recorded hits.
pub fn direct_hit(voice: Voice, articulation: &str, velocity: f64, robin: i32) -> Option<Vec<f32>> {
    let models = models()?;
    let model = models.models.get(&format!("{}/{articulation}", voice.name()))?;
    direct(model, &models.band_edges, voice, velocity, robin, 0.5, Shape::default(), &AtomicBool::new(false)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filtered_noise_is_unit_rms() {
        for (lo, hi) in [(40., 60.), (1000., 1300.), (9000., 12000.)] {
            let x = filtered_noise(48_000, lo, hi, 9);
            let rms = (x[4800..].iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / 43_200.).sqrt();
            assert!((rms - 1.).abs() < 0.25, "{lo}-{hi}: {rms}");
        }
    }
    #[test]
    fn packed_stereo_convolution_matches_two_real_convolutions() {
        let x: Vec<f32> = (0..3000).map(|i| ((i * 7919 % 101) as f32 / 50.) - 1.).collect();
        let ir = Arc::new([(0..700).map(|i| (-(i as f64) / 90.).exp()).collect::<Vec<_>>(), (0..650).map(|i| ((i as f64) * 0.3).sin() / (1. + i as f64)).collect::<Vec<_>>()]);
        let both = convolve_stereo(&x, &ir, Voice::Kick, 1.234);
        for c in 0..2 {
            let single = convolve(&x, &ir[c]);
            let worst = single.iter().zip(&both[c]).map(|(a, b)| (a - b).abs()).fold(0., f64::max);
            assert!(worst < 1e-9, "{c}: {worst}");
        }
    }
}
