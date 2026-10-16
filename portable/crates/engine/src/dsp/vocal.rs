//! Vocal finishing with conservative, stereo-linked dynamics and no model dependency.
//! All histories are fixed-size. The 144-frame latency comprises a 16-frame,
//! four-times-oversampled warmth stage and a 128-frame true-peak safety lookahead.
//! Detector values are estimates, not source identification or speech recognition.

use super::limiter::TruePeak;

#[cfg(test)]
#[path = "vocal_tests.rs"]
mod tests;

pub const LATENCY: usize = 144;
pub const DEFAULTS: [f32; 9] = [0.45, 0., 0.45, 0.5, 0.45, 0.2, 0.2, 0., 1.];
const RATE: f64 = 48000.;
const LOOKAHEAD: usize = 128;
const PEAK_WINDOW: i64 = 140;
const QUEUE: usize = 144;
const CEILING: f32 = 0.94406086; // -0.5 dBFS sample ceiling with reconstruction headroom.

fn coefficient(seconds: f64) -> f64 {
    1. - (-1. / (seconds * RATE)).exp()
}
fn db(power: f64) -> f64 {
    10. * power.max(1e-20).log10()
}
fn gain(db: f64) -> f64 {
    10_f64.powf(db / 20.)
}
fn unit(value: f64) -> f64 {
    value.clamp(0., 1.)
}
fn smooth(value: f64) -> f64 {
    let x = unit(value);
    x * x * (3. - 2. * x)
}
fn power(x: [f64; 2]) -> f64 {
    (x[0] * x[0] + x[1] * x[1]) * 0.5
}
fn follow(state: &mut f64, target: f64, attack: f64, release: f64) {
    *state += if target > *state { attack } else { release } * (target - *state);
}

/// Register-independent tonal activity hint. Correlation is evaluated on a
/// bounded 10.7 ms window every 10 ms; a coarse lag search plus local refinement
/// keeps the work bounded. Stereo energies are summed, never the waveforms, so
/// opposite-polarity channels do not cancel. It deliberately protects hum too.
#[derive(Clone)]
struct Periodicity {
    history: [[f64; 1200]; 2],
    scratch: [[f64; 1200]; 2],
    prefix: [f64; 1201],
    index: usize,
    count: usize,
    clock: usize,
    confidence: f64,
}
impl Default for Periodicity {
    fn default() -> Self {
        Self {
            history: [[0.; 1200]; 2],
            scratch: [[0.; 1200]; 2],
            prefix: [0.; 1201],
            index: 0,
            count: 0,
            clock: 0,
            confidence: 0.,
        }
    }
}
impl Periodicity {
    fn sample(&mut self, x: [f64; 2]) -> f64 {
        for (c, value) in x.iter().enumerate() {
            self.history[c][self.index] = *value;
        }
        self.index = (self.index + 1) % 1200;
        self.count = (self.count + 1).min(1200);
        self.clock += 1;
        if self.clock == 480 {
            self.clock = 0;
            if self.count == 1200 {
                self.analyze();
            }
        }
        self.confidence
    }
    fn analyze(&mut self) {
        self.prefix[0] = 0.;
        for i in 0..1200 {
            let cursor = (self.index + i) % 1200;
            for c in 0..2 {
                self.scratch[c][i] = self.history[c][cursor];
            }
            self.prefix[i + 1] =
                self.prefix[i] + self.scratch[0][i].powi(2) + self.scratch[1][i].powi(2);
        }
        let energy = self.prefix[1200] - self.prefix[688];
        if energy < 1e-16 {
            self.confidence = 0.;
            return;
        }
        let mut best = 0.;
        let mut best_lag: usize = 40;
        for lag in (40..=684).step_by(4) {
            let value = self.correlation(lag, energy);
            if value > best {
                best = value;
                best_lag = lag;
            }
        }
        for lag in best_lag.saturating_sub(4).max(40)..=(best_lag + 4).min(685) {
            best = best.max(self.correlation(lag, energy));
        }
        self.confidence = smooth((best - 0.65) / 0.25);
    }
    fn correlation(&self, lag: usize, energy: f64) -> f64 {
        let shifted = self.prefix[1200 - lag] - self.prefix[688 - lag];
        if shifted < 1e-16 {
            return 0.;
        }
        let mut dot = 0.;
        for i in 688..1200 {
            dot += self.scratch[0][i] * self.scratch[0][i - lag]
                + self.scratch[1][i] * self.scratch[1][i - lag];
        }
        (dot / (energy * shifted).sqrt()).clamp(-1., 1.)
    }
}

/// Bilinear one-pole low pass. Its complementary high pass can be subtracted
/// in parallel without the corner-frequency boost a two-pole high pass causes.
#[derive(Clone, Copy)]
struct LowPass {
    b: f64,
    a: f64,
    z: [f64; 2],
}
impl LowPass {
    fn new(hz: f64) -> Self {
        let g = (std::f64::consts::PI * hz / RATE).tan();
        Self {
            b: g / (1. + g),
            a: (g - 1.) / (g + 1.),
            z: [0.; 2],
        }
    }
    fn sample(&mut self, x: [f64; 2]) -> [f64; 2] {
        let mut y = [0.; 2];
        for c in 0..2 {
            y[c] = self.b * x[c] + self.z[c];
            self.z[c] = self.b * x[c] - self.a * y[c];
            if self.z[c].abs() < 1e-30 {
                self.z[c] = 0.;
            }
        }
        y
    }
}

/// RBJ constant-peak band pass; fixed coefficients avoid modulating resonances.
#[derive(Clone, Copy)]
struct BandPass {
    b: f64,
    a1: f64,
    a2: f64,
    z: [[f64; 2]; 2],
}
impl BandPass {
    fn new(hz: f64, q: f64) -> Self {
        let omega = 2. * std::f64::consts::PI * hz / RATE;
        let alpha = omega.sin() / (2. * q);
        let denominator = 1. + alpha;
        Self {
            b: alpha / denominator,
            a1: -2. * omega.cos() / denominator,
            a2: (1. - alpha) / denominator,
            z: [[0.; 2]; 2],
        }
    }
    fn sample(&mut self, x: [f64; 2]) -> [f64; 2] {
        let mut y = [0.; 2];
        for c in 0..2 {
            y[c] = self.b * x[c] + self.z[c][0];
            self.z[c][0] = -self.a1 * y[c] + self.z[c][1];
            self.z[c][1] = -self.b * x[c] - self.a2 * y[c];
            for z in &mut self.z[c] {
                if z.abs() < 1e-30 {
                    *z = 0.;
                }
            }
        }
        y
    }
}

#[derive(Clone)]
struct WarmStage {
    taps: [f32; 65],
    input: [[f32; 17]; 2],
    high: [[f32; 65]; 2],
    dry: [[f32; 16]; 2],
    warmth: [f32; 16],
    output: [f32; 16],
    input_index: usize,
    high_index: usize,
    dry_index: usize,
    initialized: bool,
}
impl Default for WarmStage {
    fn default() -> Self {
        let mut coefficients = [0_f64; 65];
        for (i, value) in coefficients.iter_mut().enumerate() {
            let distance = i as f64 - 32.;
            let sinc = if distance == 0. {
                0.21
            } else {
                (2. * std::f64::consts::PI * 0.105 * distance).sin()
                    / (std::f64::consts::PI * distance)
            };
            let phase = 2. * std::f64::consts::PI * i as f64 / 64.;
            *value = sinc * (0.42 - 0.5 * phase.cos() + 0.08 * (2. * phase).cos());
        }
        let sum: f64 = coefficients.iter().sum();
        Self {
            taps: coefficients.map(|v| (v / sum) as f32),
            input: [[0.; 17]; 2],
            high: [[0.; 65]; 2],
            dry: [[0.; 16]; 2],
            warmth: [0.; 16],
            output: [1.; 16],
            input_index: 0,
            high_index: 0,
            dry_index: 0,
            initialized: false,
        }
    }
}
impl WarmStage {
    fn sample(&mut self, x: [f32; 2], warmth: f32, output: f32) -> [f32; 2] {
        if !self.initialized {
            self.warmth.fill(warmth);
            self.output.fill(output);
            self.initialized = true;
        }
        let blend = 0.35 * self.warmth[self.dry_index];
        let trim = self.output[self.dry_index];
        let dry = [self.dry[0][self.dry_index], self.dry[1][self.dry_index]];
        for (c, value) in x.iter().enumerate() {
            self.dry[c][self.dry_index] = *value;
            self.input[c][self.input_index] = *value;
        }
        self.warmth[self.dry_index] = warmth;
        self.output[self.dry_index] = output;
        self.dry_index = (self.dry_index + 1) % 16;
        let mut down = [0.; 2];
        for phase in 0..4 {
            let mut up = [0.; 2];
            let mut tap = phase;
            let mut history = self.input_index;
            while tap < 65 {
                let k = self.taps[tap] * 4.;
                for (c, value) in up.iter_mut().enumerate() {
                    *value += k * self.input[c][history];
                }
                tap += 4;
                history = (history + 16) % 17;
            }
            for (c, value) in up.iter().enumerate() {
                self.high[c][self.high_index] = (*value * 1.8).tanh() / 1.8;
            }
            if phase == 0 {
                let mut history = self.high_index;
                for k in self.taps {
                    for (c, value) in down.iter_mut().enumerate() {
                        *value += k * self.high[c][history];
                    }
                    history = (history + 64) % 65;
                }
            }
            self.high_index = (self.high_index + 1) % 65;
        }
        self.input_index = (self.input_index + 1) % 17;
        [
            (dry[0] + (down[0] - dry[0]) * blend) * trim,
            (dry[1] + (down[1] - dry[1]) * blend) * trim,
        ]
    }
}

/// A stereo-linked, reconstruction-aware lookahead guard. Only catches peaks;
/// it supplies no automatic loudness makeup and is independent of dynamics.
#[derive(Clone)]
struct PeakGuard {
    audio: [[f32; LOOKAHEAD]; 2],
    values: [f32; QUEUE],
    frames: [i64; QUEUE],
    head: usize,
    tail: usize,
    cursor: usize,
    frame: i64,
    detector: TruePeak,
    gain: f32,
    release: f32,
    reduction: f32,
}
impl Default for PeakGuard {
    fn default() -> Self {
        Self {
            audio: [[0.; LOOKAHEAD]; 2],
            values: [0.; QUEUE],
            frames: [0; QUEUE],
            head: 0,
            tail: 0,
            cursor: 0,
            frame: 0,
            detector: TruePeak::default(),
            gain: 1.,
            release: coefficient(0.08) as f32,
            reduction: 0.,
        }
    }
}
impl PeakGuard {
    fn sample(&mut self, x: [f32; 2]) -> [f32; 2] {
        let delayed = [self.audio[0][self.cursor], self.audio[1][self.cursor]];
        for (c, value) in x.iter().enumerate() {
            self.audio[c][self.cursor] = *value;
        }
        self.cursor = (self.cursor + 1) % LOOKAHEAD;
        let peak = self.detector.sample(x);
        while self.head != self.tail {
            let previous = (self.tail + QUEUE - 1) % QUEUE;
            if self.values[previous] > peak {
                break;
            }
            self.tail = previous;
        }
        self.values[self.tail] = peak;
        self.frames[self.tail] = self.frame;
        self.tail = (self.tail + 1) % QUEUE;
        while self.head != self.tail && self.frames[self.head] < self.frame - PEAK_WINDOW {
            self.head = (self.head + 1) % QUEUE;
        }
        let target = (CEILING / self.values[self.head].max(1e-20)).min(1.);
        let k = if target < self.gain {
            0.08
        } else {
            self.release
        };
        self.gain += k * (target - self.gain);
        let applied = self
            .gain
            .min(CEILING / delayed[0].abs().max(delayed[1].abs()).max(1e-20));
        self.reduction = (-20. * applied.max(1e-20).log10()).max(0.);
        self.frame += 1;
        [delayed[0] * applied, delayed[1] * applied]
    }
}

#[derive(Clone)]
pub struct VocalEnhancer {
    controls: [f64; 9],
    trim: f32,
    rumble: [LowPass; 2],
    body: LowPass,
    mud: BandPass,
    harsh: BandPass,
    ess: LowPass,
    presence: BandPass,
    air: LowPass,
    total_power: f64,
    voice_power: f64,
    mud_power: f64,
    harsh_power: f64,
    ess_power: f64,
    slow_power: f64,
    noise_power: f64,
    noise_bins: [f64; 200],
    noise_count: usize,
    noise_index: usize,
    noise_clock: usize,
    periodicity: Periodicity,
    activity: f64,
    cleanup_db: f64,
    mud_db: f64,
    harsh_db: f64,
    ess_db: f64,
    level_db: f64,
    compression_envelope: f64,
    compression_db: f64,
    warm: WarmStage,
    guard: PeakGuard,
    dry: [[f32; LATENCY]; 2],
    mixes: [f32; LATENCY],
    reductions: [f32; LATENCY],
    dry_index: usize,
    initialized: bool,
    pub last_dry: [f32; 2],
    pub reduction_db: f32,
}
impl Default for VocalEnhancer {
    fn default() -> Self {
        Self {
            controls: DEFAULTS.map(f64::from),
            trim: 1.,
            rumble: [LowPass::new(65.); 2],
            body: LowPass::new(180.),
            mud: BandPass::new(320., 0.65),
            harsh: BandPass::new(3000., 0.8),
            ess: LowPass::new(5500.),
            presence: BandPass::new(2000., 0.65),
            air: LowPass::new(9000.),
            total_power: 0.,
            voice_power: 0.,
            mud_power: 0.,
            harsh_power: 0.,
            ess_power: 0.,
            slow_power: 0.,
            noise_power: 1e-7,
            noise_bins: [0.; 200],
            noise_count: 0,
            noise_index: 0,
            noise_clock: 0,
            periodicity: Periodicity::default(),
            activity: 0.,
            cleanup_db: 0.,
            mud_db: 0.,
            harsh_db: 0.,
            ess_db: 0.,
            level_db: 0.,
            compression_envelope: 0.,
            compression_db: 0.,
            warm: WarmStage::default(),
            guard: PeakGuard::default(),
            dry: [[0.; LATENCY]; 2],
            mixes: [1.; LATENCY],
            reductions: [0.; LATENCY],
            dry_index: 0,
            initialized: false,
            last_dry: [0.; 2],
            reduction_db: 0.,
        }
    }
}
impl VocalEnhancer {
    pub fn configure(&mut self, values: &[f32]) {
        for (i, default) in DEFAULTS.iter().enumerate() {
            let value = values
                .get(i)
                .copied()
                .filter(|v| v.is_finite())
                .unwrap_or(*default) as f64;
            self.controls[i] = match i {
                1 => value.clamp(-6., 6.),
                7 => value.clamp(-12., 12.),
                _ => unit(value),
            };
        }
        self.trim = gain(self.controls[7]) as f32;
    }
    pub fn sample(&mut self, input: [f32; 2]) -> [f32; 2] {
        // Audio and parameters from an external graph cannot poison recursive
        // state. Absurd finite input is bounded before squaring or conversion.
        let x = input.map(|v| {
            if v.is_finite() {
                v.clamp(-16., 16.)
            } else {
                0.
            }
        });
        let v = self.controls;
        if !self.initialized {
            self.mixes.fill(v[8] as f32);
            self.initialized = true;
        }
        let mix = self.mixes[self.dry_index];
        let old_reduction = self.reductions[self.dry_index];
        let warm_source = (self.dry_index + LATENCY - 16) % LATENCY;
        let warm_dry = [self.dry[0][warm_source], self.dry[1][warm_source]];
        let warm_mix = self.mixes[warm_source];
        for (c, value) in x.iter().enumerate() {
            self.last_dry[c] = self.dry[c][self.dry_index];
            self.dry[c][self.dry_index] = *value;
        }
        self.mixes[self.dry_index] = v[8] as f32;
        let original = x.map(f64::from);
        let low = self.rumble[0].sample(original);
        let high = [original[0] - low[0], original[1] - low[1]];
        let low_again = self.rumble[1].sample(high);
        let cleaned = [
            original[0] + (high[0] - low_again[0] - original[0]) * v[0],
            original[1] + (high[1] - low_again[1] - original[1]) * v[0],
        ];
        let body = self.body.sample(cleaned);
        let mud = self.mud.sample(cleaned);
        let harsh = self.harsh.sample(cleaned);
        let low_ess = self.ess.sample(cleaned);
        let ess = [cleaned[0] - low_ess[0], cleaned[1] - low_ess[1]];
        let presence = self.presence.sample(cleaned);
        let low_air = self.air.sample(cleaned);
        let air = [cleaned[0] - low_air[0], cleaned[1] - low_air[1]];
        follow(
            &mut self.total_power,
            power(cleaned),
            coefficient(0.01),
            coefficient(0.12),
        );
        follow(
            &mut self.voice_power,
            power(body) + power(mud),
            coefficient(0.02),
            coefficient(0.16),
        );
        follow(
            &mut self.mud_power,
            power(mud),
            coefficient(0.025),
            coefficient(0.15),
        );
        follow(
            &mut self.harsh_power,
            power(harsh),
            coefficient(0.006),
            coefficient(0.08),
        );
        follow(
            &mut self.ess_power,
            power(ess),
            coefficient(0.0015),
            coefficient(0.055),
        );
        self.slow_power += coefficient(0.3) * (power(cleaned) - self.slow_power);
        let tonal = self.periodicity.sample(cleaned);

        // Two seconds of 10 ms minima estimate the bed without learning every
        // syllable as noise. A conservative -52 dB ceiling protects quiet takes.
        self.noise_clock += 1;
        if self.noise_clock == 480 {
            self.noise_clock = 0;
            self.noise_bins[self.noise_index] = self.total_power;
            self.noise_index = (self.noise_index + 1) % 200;
            self.noise_count = (self.noise_count + 1).min(200);
            let minimum = self.noise_bins[..self.noise_count]
                .iter()
                .copied()
                .fold(f64::INFINITY, f64::min);
            let target = minimum.clamp(1e-8, 6.3095734448e-6);
            let k = if target > self.noise_power && tonal > 0.7 {
                0.
            } else if target < self.noise_power {
                0.15
            } else {
                0.025
            };
            self.noise_power += k * (target - self.noise_power);
        }
        let total_db = db(self.total_power);
        let noise_db = db(self.noise_power);
        // Body-dominant quiet singing must not disappear simply because a long
        // held note became the minimum. This is a conservative spectral hint,
        // not a voice classifier; hum may remain and should use its own notch.
        let body_hint = smooth((self.voice_power / self.total_power.max(1e-20) - 0.25) / 0.5)
            * smooth((total_db + 78.) / 16.);
        let active = (smooth((total_db - noise_db - 6.) / 12.) * smooth((total_db + 64.) / 14.))
            .max(body_hint)
            .max(tonal * smooth((total_db + 80.) / 18.));
        follow(
            &mut self.activity,
            active,
            coefficient(0.003),
            coefficient(0.25),
        );
        let quiet = smooth((noise_db + 9. - total_db) / 12.);
        let cleanup_target = 14. * v[0] * quiet * (1. - self.activity);
        // Closing is leisurely; opening is fast. There is no hard gate.
        follow(
            &mut self.cleanup_db,
            cleanup_target,
            coefficient(0.08),
            coefficient(0.003),
        );

        let mud_fraction = self.mud_power / self.total_power.max(1e-20);
        let mud_target = 4.
            * v[2]
            * smooth((mud_fraction - 0.6) / 0.35)
            * smooth((db(self.mud_power) + 30.) / 16.);
        follow(
            &mut self.mud_db,
            mud_target,
            coefficient(0.025),
            coefficient(0.15),
        );
        let harsh_fraction = self.harsh_power / self.total_power.max(1e-20);
        let harsh_target = 3.
            * v[2]
            * smooth((harsh_fraction - 0.32) / 0.48)
            * smooth((db(self.harsh_power) + 36.) / 18.);
        follow(
            &mut self.harsh_db,
            harsh_target,
            coefficient(0.008),
            coefficient(0.1),
        );
        let ess_ratio = db(self.ess_power / self.total_power.max(1e-20));
        let ess_target =
            9. * v[3] * smooth((ess_ratio + 11.) / 9.) * smooth((db(self.ess_power) + 50.) / 16.);
        follow(
            &mut self.ess_db,
            ess_target,
            coefficient(0.0015),
            coefficient(0.055),
        );

        let voice =
            self.activity * smooth((self.voice_power / self.total_power.max(1e-20) - 0.04) / 0.2);
        let mut level_target = (-22. - db(self.slow_power)).clamp(-6., 4.5) * v[4] * voice;
        if level_target > 0. {
            level_target *= smooth((total_db - noise_db - 12.) / 12.)
                * smooth((db(self.slow_power) + 50.) / 14.);
        }
        let level_k = if level_target.abs() < self.level_db.abs() {
            coefficient(0.07)
        } else {
            coefficient(0.3)
        };
        self.level_db += level_k * (level_target - self.level_db);
        let body_gain = gain(v[1]) - 1.;
        let mud_cut = 1. - gain(-self.mud_db);
        let harsh_cut = 1. - gain(-self.harsh_db);
        let ess_cut = 1. - gain(-self.ess_db);
        let presence_gain = gain(1.8 * v[2]) - 1.;
        let air_gain = (gain(3. * v[5]) - 1.) * (1. - unit(self.ess_db / 9.));
        let level_gain = gain(self.level_db - self.cleanup_db);
        let mut wet = [0.; 2];
        for c in 0..2 {
            wet[c] = (cleaned[c] + body[c] * body_gain
                - mud[c] * mud_cut
                - harsh[c] * harsh_cut
                - ess[c] * ess_cut
                + presence[c] * presence_gain
                + air[c] * air_gain)
                * level_gain;
        }
        let peak = wet[0].abs().max(wet[1].abs());
        follow(
            &mut self.compression_envelope,
            peak,
            coefficient(0.002),
            coefficient(0.09),
        );
        let over = 20. * self.compression_envelope.max(1e-10).log10() - (-18. - 6. * v[4]);
        let slope = 1. - 1. / (1. + 3. * v[4]);
        let compression_target = if over <= -4.5 {
            0.
        } else if over < 4.5 {
            slope * (over + 4.5).powi(2) / 18.
        } else {
            slope * over
        };
        follow(
            &mut self.compression_db,
            compression_target,
            coefficient(0.002),
            coefficient(0.1),
        );
        let compression_gain = gain(-self.compression_db);
        for value in &mut wet {
            *value *= compression_gain;
        }
        self.reductions[self.dry_index] =
            (self.cleanup_db + self.compression_db + self.ess_db) as f32;
        self.dry_index = (self.dry_index + 1) % LATENCY;
        let warmed = self
            .warm
            .sample(wet.map(|v| v as f32), v[6] as f32, self.trim);
        let mixed = [
            warm_dry[0] + (warmed[0] - warm_dry[0]) * warm_mix,
            warm_dry[1] + (warmed[1] - warm_dry[1]) * warm_mix,
        ];
        let protected = self.guard.sample(mixed);
        self.reduction_db = if mix == 0. {
            0.
        } else {
            (old_reduction * mix).max(self.guard.reduction)
        };
        // An explicit zero mix is bit-exact dry, including during output edits.
        if mix == 0. {
            self.last_dry
        } else {
            protected
        }
    }
}
