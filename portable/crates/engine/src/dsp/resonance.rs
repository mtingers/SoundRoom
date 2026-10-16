const N: usize = 2048;
const HOP: usize = 512;
const RING: usize = 4096;
/// Iterative radix-2 complex transform with precomputed roots and bit reversal.
/// All analysis storage is allocated before any sample is processed.
#[derive(Clone)]
struct Transform {
    roots: Vec<[f32; 2]>,
    reverse: Vec<usize>,
}
impl Default for Transform {
    fn default() -> Self {
        Self {
            roots: (0..N / 2)
                .map(|i| {
                    let angle = -2. * std::f64::consts::PI * i as f64 / N as f64;
                    [angle.cos() as f32, angle.sin() as f32]
                })
                .collect(),
            reverse: (0..N)
                .map(|i| i.reverse_bits() >> (usize::BITS - 11))
                .collect(),
        }
    }
}
impl Transform {
    fn execute(&self, real: &mut [f32], imaginary: &mut [f32], inverse: bool) {
        for (i, &j) in self.reverse.iter().enumerate() {
            if j > i {
                real.swap(i, j);
                imaginary.swap(i, j);
            }
        }
        let mut width = 2;
        while width <= N {
            let half = width / 2;
            let stride = N / width;
            for base in (0..N).step_by(width) {
                for j in 0..half {
                    let [c, mut s] = self.roots[j * stride];
                    if inverse {
                        s = -s;
                    }
                    let upper = base + j + half;
                    let lower = base + j;
                    let tr = real[upper] * c - imaginary[upper] * s;
                    let ti = real[upper] * s + imaginary[upper] * c;
                    let a = real[lower];
                    let b = imaginary[lower];
                    real[lower] = a + tr;
                    imaginary[lower] = b + ti;
                    real[upper] = a - tr;
                    imaginary[upper] = b - ti;
                }
            }
            width *= 2;
        }
    }
}
#[derive(Clone)]
pub struct Resonance {
    transform: Transform,
    window: Vec<f32>,
    input: [Vec<f32>; 2],
    output: [Vec<f32>; 2],
    spectra: [Vec<f32>; 2],
    imaginary: [Vec<f32>; 2],
    levels: Vec<f32>,
    prefix: Vec<f32>,
    reductions: Vec<f32>,
    mix_history: Vec<f32>,
    delta_history: Vec<bool>,
    input_index: usize,
    output_index: usize,
    hop_clock: usize,
    initialized: bool,
    depth: f32,
    threshold: f32,
    sharpness: f32,
    low: f32,
    high: f32,
    attack: f32,
    release: f32,
    preserve: f32,
    mix: f32,
    delta: bool,
    previous_energy: f32,
    pub last_dry: [f32; 2],
    pub reduction_db: f32,
    pub display: [f32; 12],
}
impl Default for Resonance {
    fn default() -> Self {
        Self {
            transform: Transform::default(),
            window: (0..N)
                .map(|i| {
                    (0.5 - 0.5 * (2. * std::f32::consts::PI * i as f32 / N as f32).cos()).sqrt()
                })
                .collect(),
            input: [vec![0.; N], vec![0.; N]],
            output: [vec![0.; RING], vec![0.; RING]],
            spectra: [vec![0.; N], vec![0.; N]],
            imaginary: [vec![0.; N], vec![0.; N]],
            levels: vec![-120.; N / 2 + 1],
            prefix: vec![0.; N / 2 + 2],
            reductions: vec![0.; N / 2 + 1],
            mix_history: vec![1.; N],
            delta_history: vec![false; N],
            input_index: 0,
            output_index: 0,
            hop_clock: 0,
            initialized: false,
            depth: 6.,
            threshold: 10.5,
            sharpness: 0.6,
            low: 500.,
            high: 16000.,
            attack: 0.6,
            release: 0.07,
            preserve: 0.7,
            mix: 1.,
            delta: false,
            previous_energy: 0.,
            last_dry: [0.; 2],
            reduction_db: 0.,
            display: [0.; 12],
        }
    }
}
impl Resonance {
    pub fn configure(&mut self, v: &[f32]) {
        self.depth = v[0];
        self.threshold = 18. - 15. * v[1];
        self.sharpness = v[2];
        self.low = v[3];
        self.high = self.low.max(v[4]);
        self.attack = 1. - (-(HOP as f32) / (v[5] * 0.001 * 48000.)).exp();
        self.release = 1. - (-(HOP as f32) / (v[6] * 0.001 * 48000.)).exp();
        self.preserve = v[7];
        self.mix = v[8];
        self.delta = v[9] >= 0.5;
        if !self.initialized {
            self.mix_history.fill(self.mix);
            self.delta_history.fill(self.delta);
            self.initialized = true;
        }
    }
    pub fn sample(&mut self, x: [f32; 2]) -> [f32; 2] {
        let mix = self.mix_history[self.input_index];
        let delta = self.delta_history[self.input_index];
        let mut wet = [0.; 2];
        for (c, value) in x.iter().enumerate() {
            self.last_dry[c] = self.input[c][self.input_index];
            self.input[c][self.input_index] = *value;
            wet[c] = self.output[c][self.output_index];
            self.output[c][self.output_index] = 0.;
        }
        self.mix_history[self.input_index] = self.mix;
        self.delta_history[self.input_index] = self.delta;
        self.input_index = (self.input_index + 1) % N;
        self.output_index = (self.output_index + 1) % RING;
        self.hop_clock += 1;
        if self.hop_clock == HOP {
            self.analyze();
            self.hop_clock = 0;
        }
        if delta {
            [
                (self.last_dry[0] - wet[0]) * mix,
                (self.last_dry[1] - wet[1]) * mix,
            ]
        } else {
            [
                self.last_dry[0] + (wet[0] - self.last_dry[0]) * mix,
                self.last_dry[1] + (wet[1] - self.last_dry[1]) * mix,
            ]
        }
    }
    fn analyze(&mut self) {
        for c in 0..2 {
            for i in 0..N {
                self.spectra[c][i] = self.input[c][(self.input_index + i) % N] * self.window[i];
                self.imaginary[c][i] = 0.;
            }
            self.transform
                .execute(&mut self.spectra[c], &mut self.imaginary[c], false);
        }
        let mut energy = 0.;
        self.prefix[0] = 0.;
        for k in 0..=N / 2 {
            let power = self.spectra[0][k] * self.spectra[0][k]
                + self.imaginary[0][k] * self.imaginary[0][k]
                + self.spectra[1][k] * self.spectra[1][k]
                + self.imaginary[1][k] * self.imaginary[1][k];
            self.levels[k] = 10. * (power / (N * N) as f32).max(1e-12).log10();
            self.prefix[k + 1] = self.prefix[k] + self.levels[k];
            energy += power;
        }
        let flux = if self.previous_energy > 1e-8 {
            ((energy / self.previous_energy - 1.) / 3.).clamp(0., 1.)
        } else {
            1.
        };
        self.previous_energy += 0.25 * (energy - self.previous_energy);
        self.reduction_db = 0.;
        self.display = [0.; 12];
        for k in 0..=N / 2 {
            let hz = k as f32 * 48000. / N as f32;
            let radius = ((k as f32 * (0.04 + 0.12 * self.sharpness)) as usize).max(4);
            let lo = k.saturating_sub(radius);
            let hi = (k + radius).min(N / 2);
            let excluded_lo = k.saturating_sub(1).max(lo);
            let excluded_hi = (k + 1).min(hi);
            let neighbors = hi - lo + 1 - (excluded_hi - excluded_lo + 1);
            let baseline = if neighbors > 0 {
                (self.prefix[hi + 1] - self.prefix[lo] - self.prefix[excluded_hi + 1]
                    + self.prefix[excluded_lo])
                    / neighbors as f32
            } else {
                self.levels[k]
            };
            let prominence = (self.levels[k] - baseline - self.threshold).max(0.);
            let edge = ((hz - self.low) / (self.low * 0.15).max(80.))
                .max(0.)
                .min(((self.high - hz) / (self.high * 0.15).max(200.)).max(0.))
                .min(1.);
            let target = if self.levels[k] > -85. {
                self.depth * (1. - (-prominence / 9.).exp()) * edge * (1. - self.preserve * flux)
            } else {
                0.
            };
            self.reductions[k] += if target > self.reductions[k] {
                self.attack
            } else {
                self.release
            } * (target - self.reductions[k]);
            self.reduction_db = self.reduction_db.max(self.reductions[k]);
            let band =
                (((hz.max(80.) / 80.).log2() / (20000_f32 / 80.).log2() * 12.) as usize).min(11);
            self.display[band] = self.display[band].max(self.reductions[k]);
        }
        for c in 0..2 {
            for k in 0..=N / 2 {
                let gain = 10_f32.powf(-self.reductions[k] / 20.);
                self.spectra[c][k] *= gain;
                self.imaginary[c][k] *= gain;
                if k > 0 && k < N / 2 {
                    self.spectra[c][N - k] *= gain;
                    self.imaginary[c][N - k] *= gain;
                }
            }
            self.transform
                .execute(&mut self.spectra[c], &mut self.imaginary[c], true);
            for i in 0..N {
                self.output[c][(self.output_index + i) % RING] +=
                    self.spectra[c][i] * self.window[i] / (N * 2) as f32;
            }
        }
    }
}
