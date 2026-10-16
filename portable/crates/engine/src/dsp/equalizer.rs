#[derive(Clone, Copy)]
pub enum Shape {
    Bell,
    LowShelf,
    HighShelf,
    HighPass,
    LowPass,
    BandPass,
}
#[derive(Clone, Copy)]
pub struct Section {
    pub b: [f64; 3],
    pub a: [f64; 2],
    z: [f64; 2],
}
impl Default for Section {
    fn default() -> Self {
        Self {
            b: [1., 0., 0.],
            a: [0.; 2],
            z: [0.; 2],
        }
    }
}
impl Section {
    pub fn sample(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        if !y.is_finite() || self.z.iter().any(|v| !v.is_finite()) {
            self.z = [0.; 2];
            return 0.;
        }
        for z in &mut self.z {
            if z.abs() < 1e-30 {
                *z = 0.
            }
        }
        y
    }
    pub fn configure(&mut self, hz: f32, q: f32, gain: f32, shape: Shape) {
        let omega = 2. * std::f64::consts::PI * (hz as f64).clamp(20., 21600.) / 48000.;
        let c = omega.cos();
        let alpha = omega.sin() / (2. * q as f64);
        let a = 10_f64.powf(gain as f64 / 40.);
        let denominator;
        match shape {
            Shape::Bell => {
                denominator = 1. + alpha / a;
                self.b = [1. + alpha * a, -2. * c, 1. - alpha * a];
                self.a = [-2. * c, 1. - alpha / a]
            }
            Shape::LowShelf => {
                let beta = 2. * a.sqrt() * alpha;
                denominator = (a + 1.) + (a - 1.) * c + beta;
                self.b = [
                    a * ((a + 1.) - (a - 1.) * c + beta),
                    2. * a * ((a - 1.) - (a + 1.) * c),
                    a * ((a + 1.) - (a - 1.) * c - beta),
                ];
                self.a = [
                    -2. * ((a - 1.) + (a + 1.) * c),
                    (a + 1.) + (a - 1.) * c - beta,
                ]
            }
            Shape::HighShelf => {
                let beta = 2. * a.sqrt() * alpha;
                denominator = (a + 1.) - (a - 1.) * c + beta;
                self.b = [
                    a * ((a + 1.) + (a - 1.) * c + beta),
                    -2. * a * ((a - 1.) + (a + 1.) * c),
                    a * ((a + 1.) + (a - 1.) * c - beta),
                ];
                self.a = [
                    2. * ((a - 1.) - (a + 1.) * c),
                    (a + 1.) - (a - 1.) * c - beta,
                ]
            }
            Shape::HighPass => {
                denominator = 1. + alpha;
                self.b = [(1. + c) / 2., -(1. + c), (1. + c) / 2.];
                self.a = [-2. * c, 1. - alpha]
            }
            Shape::BandPass => {
                denominator = 1. + alpha;
                self.b = [alpha, 0., -alpha];
                self.a = [-2. * c, 1. - alpha];
            }
            Shape::LowPass => {
                denominator = 1. + alpha;
                self.b = [(1. - c) / 2., 1. - c, (1. - c) / 2.];
                self.a = [-2. * c, 1. - alpha]
            }
        }
        for v in &mut self.b {
            *v /= denominator
        }
        for v in &mut self.a {
            *v /= denominator
        }
    }
    fn coefficients_from(&mut self, other: Self) {
        self.b = other.b;
        self.a = other.a;
    }
}
#[derive(Clone)]
pub struct Equalizer {
    sections: [[Section; 10]; 2],
    blends: [f64; 4],
    targets: [f64; 4],
    previous: [f32; 33],
    initialized: bool,
    output: f64,
    detectors: [[Section; 3]; 2],
    envelopes: [f32; 3],
    dynamic: [f32; 3],
    tick: usize,
    attack: f32,
    release: f32,
}
impl Default for Equalizer {
    fn default() -> Self {
        Self {
            sections: [[Section::default(); 10]; 2],
            blends: [0.; 4],
            targets: [0.; 4],
            previous: [f32::NAN; 33],
            initialized: false,
            output: 1.,
            detectors: [[Section::default(); 3]; 2],
            envelopes: [0.; 3],
            dynamic: [0.; 3],
            tick: 0,
            attack: 0.,
            release: 0.,
        }
    }
}
impl Equalizer {
    pub fn configure(&mut self, v: &[f32]) {
        let p = self.previous;
        if p[0] != v[0] || p[6] != v[6] || p[7] != v[7] {
            self.sections[0][0].configure(v[6], v[7], v[0], Shape::Bell);
            self.sections[0][1].configure(v[6], v[7], v[0], Shape::LowShelf);
            for i in [0, 1] {
                self.sections[1][i].coefficients_from(self.sections[0][i]);
            }
        }
        if p[1] != v[1] || p[2] != v[2] || p[3] != v[3] {
            self.sections[0][2].configure(v[1], v[3], v[2], Shape::Bell);
            self.sections[1][2].coefficients_from(self.sections[0][2]);
        }
        if p[4] != v[4] || p[9] != v[9] || p[10] != v[10] {
            self.sections[0][3].configure(v[9], v[10], v[4], Shape::Bell);
            self.sections[0][4].configure(v[9], v[10], v[4], Shape::HighShelf);
            for i in [3, 4] {
                self.sections[1][i].coefficients_from(self.sections[0][i]);
            }
        }
        if p[12] != v[12] {
            self.sections[0][5].configure(
                v[12],
                std::f64::consts::FRAC_1_SQRT_2 as f32,
                0.,
                Shape::HighPass,
            );
            self.sections[1][5].coefficients_from(self.sections[0][5]);
        }
        if p[14] != v[14] {
            self.sections[0][6].configure(
                v[14],
                std::f64::consts::FRAC_1_SQRT_2 as f32,
                0.,
                Shape::LowPass,
            );
            self.sections[1][6].coefficients_from(self.sections[0][6]);
        }
        if p[5] != v[5] {
            self.output = 10_f64.powf(v[5] as f64 / 20.)
        }
        for n in 0..3 {
            let k = 16 + n * 5;
            if p[k] != v[k] || p[k + 2] != v[k + 2] {
                for c in 0..2 {
                    self.detectors[c][n].configure(v[k], v[k + 2], 0., Shape::BandPass);
                }
            }
            if p[k] != v[k] || p[k + 1] != v[k + 1] || p[k + 2] != v[k + 2] {
                for c in 0..2 {
                    self.sections[c][7 + n].configure(
                        v[k],
                        v[k + 2],
                        v[k + 1] + self.dynamic[n],
                        Shape::Bell,
                    );
                }
            }
        }
        self.attack = 1. - (-1. / (v[31] * 48.)).exp();
        self.release = 1. - (-1. / (v[32] * 48.)).exp();
        self.previous.copy_from_slice(v);
        self.targets = [v[8] as f64, v[11] as f64, v[13] as f64, v[15] as f64];
        if !self.initialized {
            self.blends = self.targets;
            self.initialized = true;
        }
    }
    pub fn sample(&mut self, x: [f32; 2]) -> [f32; 2] {
        for i in 0..4 {
            self.blends[i] += (self.targets[i] - self.blends[i]).clamp(-1. / 240., 1. / 240.);
        }
        for n in 0..3 {
            let k = 16 + n * 5;
            if self.previous[k + 4] == 0. && self.dynamic[n] == 0. {
                continue;
            }
            let l = self.detectors[0][n].sample(x[0] as f64).abs() as f32;
            let r = self.detectors[1][n].sample(x[1] as f64).abs() as f32;
            let level = l.max(r);
            let coefficient = if level > self.envelopes[n] {
                self.attack
            } else {
                self.release
            };
            self.envelopes[n] += coefficient * (level - self.envelopes[n]);
            let db = 20. * self.envelopes[n].max(1e-9).log10();
            let cut = (-(db - self.previous[k + 3]).max(0.)).max(self.previous[k + 4]);
            if self.tick == 0 && (cut - self.dynamic[n]).abs() > 0.01 {
                self.dynamic[n] = cut;
                self.sections[0][7 + n].configure(
                    self.previous[k],
                    self.previous[k + 2],
                    self.previous[k + 1] + cut,
                    Shape::Bell,
                );
                self.sections[1][7 + n].coefficients_from(self.sections[0][7 + n]);
            }
        }
        self.tick = (self.tick + 1) % 32;
        let mut out = [0.; 2];
        for c in 0..2 {
            let s = &mut self.sections[c];
            let low_bell = s[0].sample(x[c] as f64);
            let low_shelf = s[1].sample(x[c] as f64);
            let mid = s[2].sample(low_bell + (low_shelf - low_bell) * self.blends[0]);
            let high_bell = s[3].sample(mid);
            let high_shelf = s[4].sample(mid);
            let high = high_bell + (high_shelf - high_bell) * self.blends[1];
            let cut_low = s[5].sample(high);
            let passed = high + (cut_low - high) * self.blends[2];
            let cut_high = s[6].sample(passed);
            let mut y = passed + (cut_high - passed) * self.blends[3];
            for section in &mut s[7..10] {
                y = section.sample(y);
            }
            out[c] = (y * self.output) as f32;
        }
        out
    }
}
