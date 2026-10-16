#[derive(Clone)]
pub struct Warmth {
    taps: [f32; 65],
    input: [[f32; 17]; 2],
    high: [[f32; 65]; 2],
    dry: [[f32; 16]; 2],
    mixes: [f32; 16],
    outputs: [f32; 16],
    matches: [bool; 16],
    initialized: bool,
    input_index: usize,
    high_index: usize,
    dry_index: usize,
    tone: [f32; 2],
    presence: [f32; 2],
    dc: [f32; 2],
    weights: [f32; 5],
    target: usize,
    remaining: usize,
    character_initialized: bool,
    input_power: f32,
    output_power: f32,
    match_gain: f32,
    power_coefficient: f32,
    gain_coefficient: f32,
    previous: [f32; 5],
    drive: f32,
    tone_coefficient: f32,
    emphasis_gain: f32,
    output_gain: f32,
    norms: [f32; 5],
    offset: f32,
    offset_tanh: f32,
    pub last_dry: [f32; 2],
}
impl Default for Warmth {
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
        let taps = coefficients.map(|v| (v / sum) as f32);
        Self {
            taps,
            input: [[0.; 17]; 2],
            high: [[0.; 65]; 2],
            dry: [[0.; 16]; 2],
            mixes: [1.; 16],
            outputs: [0.; 16],
            matches: [false; 16],
            initialized: false,
            input_index: 0,
            high_index: 0,
            dry_index: 0,
            tone: [0.; 2],
            presence: [0.; 2],
            dc: [0.; 2],
            weights: [0.; 5],
            target: 0,
            remaining: 0,
            character_initialized: false,
            input_power: 0.,
            output_power: 0.,
            match_gain: 1.,
            power_coefficient: 1. - (-1_f32 / (0.5 * 48000.)).exp(),
            gain_coefficient: 1. - (-1_f32 / (0.02 * 48000.)).exp(),
            previous: [f32::NAN; 5],
            drive: 1.,
            tone_coefficient: 0.,
            emphasis_gain: 0.,
            output_gain: 1.,
            norms: [1.; 5],
            offset: 0.,
            offset_tanh: 0.,
            last_dry: [0.; 2],
        }
    }
}
impl Warmth {
    fn shape(&self, x: f32, character: usize, bias: f32) -> f32 {
        match character {
            1 => (x * 1.4).atan() / self.norms[1],
            2 => ((x + self.offset).tanh() - self.offset_tanh) / self.norms[2],
            3 => x / (1. + x * x).sqrt() * self.norms[3] / self.drive,
            4 => {
                let k = if x >= 0. { 1. + bias } else { 1. - bias * 0.45 };
                (x * k * 1.8).tanh() / self.norms[4]
            }
            _ => x.tanh() / self.norms[0],
        }
    }
    pub fn sample(&mut self, x: [f32; 2], v: &[f32]) -> [f32; 2] {
        if !self.initialized {
            self.mixes.fill(v[2]);
            self.outputs.fill(v[3]);
            self.matches.fill(v[4] >= 0.5);
            self.initialized = true;
        }
        let mix = self.mixes[self.dry_index];
        let output_db = self.outputs[self.dry_index];
        let matched = self.matches[self.dry_index];
        for (c, value) in x.iter().enumerate() {
            self.last_dry[c] = self.dry[c][self.dry_index];
            self.dry[c][self.dry_index] = *value;
        }
        self.mixes[self.dry_index] = v[2];
        self.outputs[self.dry_index] = v[3];
        self.matches[self.dry_index] = v[4] >= 0.5;
        self.dry_index = (self.dry_index + 1) % 16;
        let character = (v[5] as usize).min(4);
        if !self.character_initialized {
            self.target = character;
            self.weights[character] = 1.;
            self.character_initialized = true;
        }
        if character != self.target {
            self.target = character;
            self.remaining = 240;
        }
        if self.remaining > 0 {
            for i in 0..5 {
                let target = if i == self.target { 1. } else { 0. };
                self.weights[i] += (target - self.weights[i]) / self.remaining as f32;
            }
            self.remaining -= 1;
        }
        let presence_coefficient = 1. - (-2. * std::f32::consts::PI * 1800. / 48000.).exp();
        let new_weight = 1. - self.weights[0];
        if self.previous[0] != v[0] || self.previous[4] != v[6] {
            self.drive = 10_f32.powf(v[0] / 20.);
            self.offset = v[6] * 0.8;
            self.offset_tanh = self.offset.tanh();
            self.norms = [
                self.drive.tanh(),
                (self.drive * 1.4).atan(),
                (((self.drive + self.offset).tanh() - (-self.drive + self.offset).tanh()) * 0.5)
                    .max(0.1),
                (1. + self.drive * self.drive).sqrt(),
                (self.drive * 1.8).tanh().max(0.1),
            ];
            self.previous[0] = v[0];
            self.previous[4] = v[6];
        }
        if self.previous[1] != v[1] {
            self.tone_coefficient = 1. - (-2. * std::f32::consts::PI * v[1] / 48000.).exp();
            self.previous[1] = v[1];
        }
        if self.previous[2] != v[7] {
            self.emphasis_gain = 10_f32.powf(v[7] / 20.) - 1.;
            self.previous[2] = v[7];
        }
        if self.previous[3] != output_db {
            self.output_gain = 10_f32.powf(output_db / 20.);
            self.previous[3] = output_db;
        }
        let emphasis = self.emphasis_gain * new_weight;
        for (c, value) in x.iter().enumerate() {
            self.presence[c] += presence_coefficient * (*value - self.presence[c]);
            self.input[c][self.input_index] = *value + (*value - self.presence[c]) * emphasis;
        }
        let drive = self.drive;
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
            let mut shaped = [0.; 2];
            for i in 0..5 {
                if self.weights[i] > 0. {
                    for (c, value) in shaped.iter_mut().enumerate() {
                        *value += self.shape(up[c] * drive, i, v[6]) * self.weights[i];
                    }
                }
            }
            for (c, value) in shaped.iter().enumerate() {
                self.high[c][self.high_index] = *value;
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
        let tone_coefficient = self.tone_coefficient;
        for (c, value) in down.iter_mut().enumerate() {
            self.dc[c] += 0.00065 * (*value - self.dc[c]);
            *value -= self.dc[c] * new_weight;
            self.tone[c] += tone_coefficient * (*value - self.tone[c]);
        }
        self.input_power += self.power_coefficient
            * ((self.last_dry[0] * self.last_dry[0] + self.last_dry[1] * self.last_dry[1]) * 0.5
                - self.input_power);
        self.output_power += self.power_coefficient
            * ((self.tone[0] * self.tone[0] + self.tone[1] * self.tone[1]) * 0.5
                - self.output_power);
        let target = if matched && self.output_power > 1e-12 {
            (self.input_power / self.output_power)
                .sqrt()
                .clamp(0.05, 4.)
        } else {
            1.
        };
        self.match_gain += self.gain_coefficient * (target - self.match_gain);
        let output = self.output_gain;
        [
            (self.last_dry[0] * (1. - mix) + self.tone[0] * self.match_gain * mix) * output,
            (self.last_dry[1] * (1. - mix) + self.tone[1] * self.match_gain * mix) * output,
        ]
    }
}
