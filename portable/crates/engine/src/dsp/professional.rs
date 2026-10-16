/// Stereo-linked downward expander with hysteresis and a retriggerable hold.
#[derive(Clone, Default)]
pub struct Gate {
    open: bool,
    hold: u32,
    gain_db: f32,
    initialized: bool,
    pub reduction_db: f32,
}
impl Gate {
    pub fn sample(&mut self, x: [f32; 2], v: &[f32]) -> [f32; 2] {
        let db = 20. * x[0].abs().max(x[1].abs()).max(1e-9).log10();
        if db >= v[0] || self.open && db >= v[0] - v[6] {
            self.open = true;
            self.hold = (v[4] * 48.).round() as u32;
        } else if self.hold > 0 {
            self.hold -= 1;
        } else {
            self.open = false;
        }
        let target = if self.open {
            0.
        } else {
            -((v[0] - db).max(0.) * (v[1] - 1.)).min(v[2])
        };
        if !self.initialized {
            self.gain_db = if v[1] <= 1. { 0. } else { -v[2] };
            self.initialized = true;
        }
        let ms = if target > self.gain_db { v[3] } else { v[5] };
        self.gain_db += (1. - (-1. / (ms * 48.)).exp()) * (target - self.gain_db);
        self.reduction_db = -self.gain_db;
        let g = 10_f32.powf(self.gain_db / 20.);
        [x[0] * g, x[1] * g]
    }
}
/// Intentional channel alignment, never reported as compensatable insert latency.
#[derive(Clone)]
pub struct Utility {
    delay: Vec<[f32; 2]>,
    index: usize,
}
impl Default for Utility {
    fn default() -> Self {
        Self {
            delay: vec![[0.; 2]; 4801],
            index: 0,
        }
    }
}
impl Utility {
    pub fn sample(&mut self, mut x: [f32; 2], v: &[f32]) -> [f32; 2] {
        x[0] *= if v[1] >= 0.5 { -1. } else { 1. };
        x[1] *= if v[2] >= 0.5 { -1. } else { 1. };
        if v[3] >= 0.5 {
            x.swap(0, 1);
        }
        let mid = (x[0] + x[1]) * 0.5;
        let side = (x[0] - x[1]) * 0.5 * if v[4] >= 0.5 { 0. } else { v[5] };
        self.delay[self.index] = [mid + side, mid - side];
        let gain = 10_f32.powf(v[0] / 20.);
        let out = std::array::from_fn(|c| {
            self.delay[(self.index + 4801 - v[6 + c].round() as usize) % 4801][c] * gain
        });
        self.index = (self.index + 1) % 4801;
        out
    }
}
