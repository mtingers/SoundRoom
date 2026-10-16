#[derive(Clone, Copy)]
struct Filter {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}
impl Filter {
    fn sample(&mut self, x: f32) -> f32 {
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
/// Same stereo K weighting and exact 400 ms / 3 s windows as SoundroomCore.
/// Readings are momentary/short-term, never mislabeled as gated integrated LUFS.
#[derive(Clone)]
pub struct Loudness {
    filters: [[Filter; 2]; 2],
    powers: Vec<f64>,
    cursor: usize,
    update: usize,
    moment_sum: f64,
    short_sum: f64,
    pub momentary: f32,
    pub short_term: f32,
    /// K-weighted stereo power of the most recent sample (Swift `lastPower`).
    pub last_power: f64,
}
#[allow(clippy::excessive_precision)] // Preserve the Swift K-weighting coefficients verbatim.
impl Default for Loudness {
    fn default() -> Self {
        let shelf = Filter {
            b: [1.53512485958697, -2.69169618940638, 1.19839281085285],
            a: [-1.69065929318241, 0.73248077421585],
            z: [0.; 2],
        };
        let high = Filter {
            b: [1., -2., 1.],
            a: [-1.99004745483398, 0.99007225036621],
            z: [0.; 2],
        };
        Self {
            filters: [[shelf, high]; 2],
            powers: vec![0.; 144000],
            cursor: 0,
            update: 0,
            moment_sum: 0.,
            short_sum: 0.,
            momentary: -120.,
            short_term: -120.,
            last_power: 0.,
        }
    }
}
impl Loudness {
    pub fn sample(&mut self, x: [f32; 2]) {
        let mut weighted = [0_f64; 2];
        for (c, v) in x.iter().enumerate() {
            let first = self.filters[c][0].sample(*v);
            weighted[c] = self.filters[c][1].sample(first) as f64;
        }
        let power = weighted[0] * weighted[0] + weighted[1] * weighted[1];
        self.last_power = power;
        self.moment_sum += power - self.powers[(self.cursor + 144000 - 19200) % 144000];
        self.short_sum += power - self.powers[self.cursor];
        self.powers[self.cursor] = power;
        self.cursor = (self.cursor + 1) % 144000;
        self.update += 1;
        if self.update == 480 {
            self.momentary =
                (-0.691 + 10. * (self.moment_sum / 19200.).max(1e-12).log10()).max(-120.) as f32;
            self.short_term =
                (-0.691 + 10. * (self.short_sum / 144000.).max(1e-12).log10()).max(-120.) as f32;
            self.update = 0;
        }
    }
}
