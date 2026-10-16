//! Swift-compatible monophonic YIN detector and stereo-linked PSOLA correction.
//! All storage is allocated when preparing the state; process does no allocation.
const CAPACITY: usize = 16384;
const MASK: i64 = CAPACITY as i64 - 1;
pub const LATENCY: usize = 4096;
#[derive(Clone, Copy, Debug)]
pub struct Controls {
    pub key: i32,
    pub scale: i32,
    pub mask: i32,
    pub reference: f32,
    pub retune: f32,
    pub strength: f32,
    pub humanize: f32,
    pub tolerance: f32,
    pub vibrato: f32,
    pub formant: f32,
    pub mix: f32,
    pub manual_target: f32,
}
impl Default for Controls {
    fn default() -> Self {
        Self {
            key: 0,
            scale: 0,
            mask: 4095,
            reference: 440.,
            retune: 40.,
            strength: 1.,
            humanize: 0.35,
            tolerance: 5.,
            vibrato: 0.5,
            formant: 0.,
            mix: 1.,
            manual_target: -1.,
        }
    }
}
impl Controls {
    pub fn from_values(v: &[f32]) -> Self {
        Self {
            key: v[0] as i32,
            scale: v[1] as i32,
            mask: v[2] as i32,
            reference: v[3],
            retune: v[4],
            strength: v[5],
            humanize: v[6],
            tolerance: v[7],
            vibrato: v[8],
            formant: v[9],
            mix: v[10],
            manual_target: v[11],
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Meter {
    pub source_frame: i64,
    pub reference_frequency: f32,
    pub input_frequency: f32,
    pub confidence: f32,
    pub target_frequency: f32,
    pub correction_cents: f32,
    pub wet_blend: f32,
}
#[derive(Clone)]
pub struct Pitch {
    input: [Vec<f32>; 2],
    pulse: Vec<f32>,
    frequencies: Vec<f32>,
    confidences: Vec<f32>,
    output: [Vec<f32>; 2],
    coverage: Vec<f32>,
    targets: Vec<f32>,
    corrections: Vec<f32>,
    activity: Vec<f32>,
    weights: Vec<f32>,
    controls: Vec<Controls>,
    decimated: [f32; 512],
    difference: [f32; 242],
    refined: [f32; 7],
    hann: Vec<f32>,
    frame: i64,
    decimated_count: i64,
    lowpass: [f32; 2],
    energy: [f32; 2],
    use_right: bool,
    next_epoch: f64,
    last_pulse: Option<f64>,
    last_period: f32,
    correction: f32,
    center_pitch: f32,
    previous_target: f32,
    sustained: f32,
    dc_input: [f32; 2],
    dc_output: [f32; 2],
    voiced_blend: f32,
    rendered_manual: f32,
    pub meter: Meter,
    pub last_dry: [f32; 2],
    pub detected_frequency: f32,
    pub confidence: f32,
    pub target_frequency: f32,
}
impl Default for Pitch {
    fn default() -> Self {
        Self {
            input: [vec![0.; CAPACITY], vec![0.; CAPACITY]],
            pulse: vec![0.; CAPACITY],
            frequencies: vec![0.; CAPACITY],
            confidences: vec![0.; CAPACITY],
            output: [vec![0.; CAPACITY], vec![0.; CAPACITY]],
            coverage: vec![0.; CAPACITY],
            targets: vec![0.; CAPACITY],
            corrections: vec![0.; CAPACITY],
            activity: vec![0.; CAPACITY],
            weights: vec![0.; CAPACITY],
            controls: vec![Controls::default(); CAPACITY],
            decimated: [0.; 512],
            difference: [0.; 242],
            refined: [0.; 7],
            hann: (0..=2048)
                .map(|i| (0.5 + 0.5 * (std::f64::consts::PI * i as f64 / 2048.).cos()) as f32)
                .collect(),
            frame: 0,
            decimated_count: 0,
            lowpass: [0.; 2],
            energy: [0.; 2],
            use_right: false,
            next_epoch: 0.,
            last_pulse: None,
            last_period: 0.,
            correction: 0.,
            center_pitch: 0.,
            previous_target: f32::NEG_INFINITY,
            sustained: 0.,
            dc_input: [0.; 2],
            dc_output: [0.; 2],
            voiced_blend: 0.,
            rendered_manual: 0.,
            meter: Meter::default(),
            last_dry: [0.; 2],
            detected_frequency: 0.,
            confidence: 0.,
            target_frequency: 0.,
        }
    }
}
fn clamp(v: f32, lo: f32, hi: f32, fallback: f32) -> f32 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        fallback
    }
}
fn sample(ring: &[f32], pos: i64, frame: i64) -> f32 {
    if pos >= 0 && pos <= frame && frame - pos < CAPACITY as i64 {
        ring[(pos & MASK) as usize]
    } else {
        0.
    }
}
fn interpolated(ring: &[f32], pos: f64, frame: i64) -> f32 {
    let first = pos.floor() as i64;
    let f = (pos - first as f64) as f32;
    sample(ring, first, frame) * (1. - f) + sample(ring, first + 1, frame) * f
}
pub fn nearest_note(midi: f32, key: i32, scale: i32, mask: i32) -> i32 {
    let root = key.rem_euclid(12);
    let bits = match scale {
        1 => 0b101010110101,
        2 => 0b010110101101,
        3 => mask & 4095,
        _ => return midi.round() as i32,
    };
    if bits == 0 {
        return midi.round() as i32;
    }
    let middle = midi.round() as i32;
    let mut nearest = middle;
    let mut distance = f32::INFINITY;
    for note in middle - 12..=middle + 12 {
        let class = note.rem_euclid(12);
        let bit = if scale == 3 {
            class
        } else {
            (class - root).rem_euclid(12)
        };
        let d = (note as f32 - midi).abs();
        if bits & (1 << bit) != 0 && d < distance {
            nearest = note;
            distance = d;
        }
    }
    nearest
}
impl Pitch {
    pub fn sample(&mut self, input: [f32; 2], p: Controls) -> [f32; 2] {
        let input = input.map(|v| if v.is_finite() { v } else { 0. });
        let index = (self.frame & MASK) as usize;
        for (c, &v) in input.iter().enumerate() {
            self.input[c][index] = v;
            self.energy[c] += 0.001 * (v * v - self.energy[c]);
        }
        self.controls[index] = p;
        if self.energy[1] > 1.5 * self.energy[0] + 0.000000001 {
            self.use_right = true;
        } else if self.energy[0] > 1.5 * self.energy[1] + 0.000000001 {
            self.use_right = false;
        }
        let mono = input[usize::from(self.use_right)];
        self.lowpass[0] += 0.23 * (mono - self.lowpass[0]);
        self.lowpass[1] += 0.23 * (self.lowpass[0] - self.lowpass[1]);
        self.pulse[index] = self.lowpass[1];
        if self.frame & 3 == 0 {
            self.decimated[(self.decimated_count & 511) as usize] = self.lowpass[1];
            self.decimated_count += 1;
            if self.decimated_count >= 512 && self.decimated_count & 63 == 0 {
                self.analyze();
            }
        }
        self.frequencies[index] = self.detected_frequency;
        self.confidences[index] = self.confidence;
        while self.next_epoch <= (self.frame + 2048) as f64 {
            self.synthesize();
        }
        let source = self.frame - LATENCY as i64;
        self.last_dry = [
            sample(&self.input[0], source, self.frame),
            sample(&self.input[1], source, self.frame),
        ];
        let weight = self.weights[index];
        let delayed = if source >= 0 {
            self.controls[(source & MASK) as usize]
        } else {
            Controls::default()
        };
        let coverage = self.coverage[index];
        let natural = delayed.formant.abs() < 0.001;
        let frequency = if source >= 0 {
            sample(&self.frequencies, source + 1024, self.frame)
        } else {
            0.
        };
        let confidence = if source >= 0 {
            sample(&self.confidences, source + 1024, self.frame)
        } else {
            0.
        };
        let voiced = (50. ..=1000.).contains(&frequency) && confidence > 0.75;
        let manual_weight = if delayed.manual_target >= 0. && (weight > 0.00001 || voiced) {
            if weight > 0.00001 {
                self.rendered_manual = self.corrections[index] / weight;
            }
            ((-self.rendered_manual - 600.) / 600.).clamp(0., 1.)
        } else {
            self.rendered_manual = 0.;
            0.
        };
        let divisor = if natural && coverage > 0.00001 {
            if manual_weight > 0. {
                coverage * (1. - manual_weight) + manual_weight
            } else {
                coverage
            }
        } else {
            1.
        };
        for c in 0..2 {
            let unfiltered = self.output[c][index] / divisor;
            self.dc_output[c] = unfiltered - self.dc_input[c] + 0.998692 * self.dc_output[c];
            self.dc_input[c] = unfiltered;
        }
        let active = voiced && weight > 0.00001 && self.activity[index] / weight > 0.01;
        self.voiced_blend += (if active {
            1. - self.voiced_blend
        } else {
            -self.voiced_blend
        }) * (if active { 1. / 480. } else { 1. / 96. });
        let covered = if natural {
            (coverage * 4.).clamp(0., 1.)
        } else {
            1.
        };
        let available = if manual_weight > 0. {
            covered * (1. - manual_weight) + manual_weight
        } else {
            covered
        };
        let blend = self.voiced_blend * available * clamp(delayed.mix, 0., 1., 1.);
        self.meter = Meter {
            source_frame: source,
            reference_frequency: clamp(delayed.reference, 420., 460., 440.),
            input_frequency: if voiced { frequency } else { 0. },
            confidence,
            target_frequency: if voiced && weight > 0.00001 {
                self.targets[index] / weight
            } else {
                0.
            },
            correction_cents: if voiced && weight > 0.00001 {
                self.corrections[index] / weight
            } else {
                0.
            },
            wet_blend: blend,
        };
        self.output[0][index] = 0.;
        self.output[1][index] = 0.;
        self.weights[index] = 0.;
        self.activity[index] = 0.;
        self.coverage[index] = 0.;
        self.targets[index] = 0.;
        self.corrections[index] = 0.;
        self.frame += 1;
        [
            self.last_dry[0] + (self.dc_output[0] - self.last_dry[0]) * blend,
            self.last_dry[1] + (self.dc_output[1] - self.last_dry[1]) * blend,
        ]
    }
    fn analyze(&mut self) {
        let start = self.decimated_count - 512;
        let mut energy = 0.;
        for j in 0..256 {
            let v = self.decimated[((start + j) & 511) as usize];
            energy += v * v;
        }
        if energy / 256. <= 0.000003 {
            self.confidence = 0.;
            self.detected_frequency = 0.;
            return;
        }
        self.difference[0] = 1.;
        let mut cumulative = 0.;
        for lag in 1..=241 {
            let mut sum = 0.;
            let centered = start + 128 - lag as i64 / 2;
            for j in 0..256 {
                let d = self.decimated[((centered + j) & 511) as usize]
                    - self.decimated[((centered + j + lag as i64) & 511) as usize];
                sum += d * d;
            }
            cumulative += sum;
            self.difference[lag] = if cumulative > 0. {
                sum * lag as f32 / cumulative
            } else {
                1.
            };
        }
        let mut lag = 12;
        while lag < 240 {
            if self.difference[lag] < 0.15 {
                while lag < 240 && self.difference[lag + 1] < self.difference[lag] {
                    lag += 1;
                }
                break;
            }
            lag += 1;
        }
        if self.difference[lag] >= 0.25 {
            self.confidence = 0.;
            self.detected_frequency = 0.;
            return;
        }
        let a = self.difference[lag - 1];
        let b = self.difference[lag];
        let c = self.difference[lag + 1];
        let denominator = a - 2. * b + c;
        let adjustment = if denominator.abs() > 0.000001 {
            (0.5 * (a - c) / denominator).clamp(-0.5, 0.5)
        } else {
            0.
        };
        let base = (4. * (lag as f32 + adjustment)).round() as i64;
        for index in 0..7 {
            let candidate = base + index as i64 - 3;
            let window = self.frame - 1024 - 512 - candidate / 2;
            debug_assert!((43..=965).contains(&candidate));
            debug_assert!(
                window >= 0
                    && window + 1023 + candidate <= self.frame
                    && self.frame - window < CAPACITY as i64
            );
            let mut sum = 0.;
            for j in 0..1024 {
                let d = self.pulse[((window + j) & MASK) as usize]
                    - self.pulse[((window + j + candidate) & MASK) as usize];
                sum += d * d;
            }
            self.refined[index] = sum;
        }
        let mut best = 1;
        for index in 2..=5 {
            if self.refined[index] < self.refined[best] {
                best = index;
            }
        }
        let a = self.refined[best - 1];
        let b_refined = self.refined[best];
        let c = self.refined[best + 1];
        let denominator = a - 2. * b_refined + c;
        let fraction = if denominator.abs() > 0.000001 {
            (0.5 * (a - c) / denominator).clamp(-0.5, 0.5)
        } else {
            0.
        };
        self.detected_frequency = 48000. / ((base + best as i64 - 3) as f32 + fraction);
        self.confidence = 1. - b;
    }
    fn synthesize(&mut self) {
        let output_center = self.next_epoch;
        let source_center = output_center - LATENCY as f64;
        let source_frame = source_center as i64;
        let p = if source_frame >= 0 && source_frame <= self.frame {
            self.controls[(source_frame & MASK) as usize]
        } else {
            Controls::default()
        };
        let f0 = sample(&self.frequencies, source_frame + 1024, self.frame);
        let certainty = sample(&self.confidences, source_frame + 1024, self.frame);
        if !(50. ..=1000.).contains(&f0) || certainty <= 0.75 {
            self.target_frequency = 0.;
            self.last_pulse = None;
            self.last_period = 0.;
            self.center_pitch = 0.;
            self.correction = 0.;
            self.previous_target = f32::NEG_INFINITY;
            self.sustained = 0.;
            self.next_epoch += 128.;
            return;
        }
        let period = 48000. / f0;
        let reference = clamp(p.reference, 420., 460., 440.);
        let midi = 69. + 12. * (f0 / reference).log2();
        let interval = period.max(1.);
        if self.center_pitch == 0. || (midi - self.center_pitch).abs() > 3. {
            self.center_pitch = midi;
        }
        self.center_pitch +=
            (1. - (-interval / (48000. * 0.12)).exp()) * (midi - self.center_pitch);
        let vibrato = clamp(p.vibrato, 0., 1., 0.5);
        let targeting = midi * (1. - vibrato) + self.center_pitch * vibrato;
        let manual = if p.manual_target.is_finite() && p.manual_target >= 0. {
            Some(p.manual_target.min(95.))
        } else {
            None
        };
        let mut target =
            manual.unwrap_or_else(|| nearest_note(targeting, p.key, p.scale, p.mask) as f32);
        if manual.is_none()
            && self.previous_target.is_finite()
            && target != self.previous_target
            && nearest_note(self.previous_target, p.key, p.scale, p.mask) as f32
                == self.previous_target
            && (targeting - self.previous_target).abs() <= (targeting - target).abs() + 0.12
        {
            target = self.previous_target;
        }
        if target == self.previous_target {
            self.sustained += interval;
        } else {
            self.sustained = 0.;
            self.previous_target = target;
        }
        self.target_frequency = reference * 2_f32.powf((target - 69.) / 12.);
        let mut desired = (target - midi + vibrato * (midi - self.center_pitch)) * 100.;
        if manual.is_none() && p.scale == 3 && p.mask & 4095 == 0 {
            desired = 0.;
            self.target_frequency = f0;
        }
        if manual.is_some() {
            desired = desired.clamp(-1200., 1200.);
        }
        if desired.abs() <= clamp(p.tolerance, 0., 100., 5.) {
            desired = 0.;
        }
        desired *= clamp(p.strength, 0., 1., 1.);
        let time = clamp(p.retune, 0., 500., 40.)
            + clamp(p.humanize, 0., 1., 0.35) * 250. * (self.sustained / 48000.).min(1.);
        let alpha = if time <= 0. {
            1.
        } else {
            1. - (-interval / (time * 0.001 * 48000.)).exp()
        };
        self.correction += alpha * (desired - self.correction);
        let ratio = 2_f32.powf(self.correction / 1200.).clamp(0.5, 2.);
        let formant = clamp(p.formant, -12., 12., 0.);
        let formant_ratio = 2_f32.powf(formant / 12.);
        let radius = (period / formant_ratio).clamp(24., 1920.);
        let (pulse_center, tracked) = if let Some(previous) = self
            .last_pulse
            .filter(|_| (period - self.last_period).abs() < period * 0.2)
        {
            (
                previous + ((source_center - previous) / period as f64).round() * period as f64,
                true,
            )
        } else {
            (source_center, false)
        };
        let middle = pulse_center.round() as i64;
        let search = ((period * if tracked { 0.2 } else { 0.5 }) as i64).max(1);
        let mut pulse = middle;
        let mut peak = f32::NEG_INFINITY;
        for pos in middle - search..=middle + search {
            let v = sample(&self.pulse, pos, self.frame);
            if v > peak {
                peak = v;
                pulse = pos;
            }
        }
        self.last_pulse = Some(pulse as f64);
        self.last_period = period;
        let normalization = (radius / (period / ratio)).max(0.125);
        let first = (output_center - radius as f64).ceil() as i64;
        let last = (output_center + radius as f64).floor() as i64;
        for pos in first.max(self.frame)..=last {
            let displacement = (pos as f64 - output_center) as f32;
            let window_index = ((displacement.abs() / radius * 2048.) as usize).min(2048);
            let window = self.hann[window_index];
            let source_position = pulse as f64 + (displacement * formant_ratio) as f64;
            let index = (pos & MASK) as usize;
            for c in 0..2 {
                self.output[c][index] += interpolated(&self.input[c], source_position, self.frame)
                    * window
                    / normalization;
            }
            self.weights[index] += window;
            self.coverage[index] += window / normalization;
            self.targets[index] += self.target_frequency * window;
            self.corrections[index] += self.correction * window;
            if self.correction.abs() > 0.01 || formant.abs() > 0.001 {
                self.activity[index] += window;
            }
        }
        self.next_epoch += (period / ratio) as f64;
    }
}
