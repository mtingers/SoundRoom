/// Fixed fractional-delay storage; phase is intentionally f64, as in SoundroomCore.
#[derive(Clone)]
pub struct Chorus {
    lines: [Vec<f32>; 2],
    cursor: usize,
    phase: f64,
}
impl Default for Chorus {
    fn default() -> Self {
        Self {
            lines: [vec![0.; 4096], vec![0.; 4096]],
            cursor: 0,
            phase: 0.,
        }
    }
}
impl Chorus {
    pub fn sample(&mut self, x: [f32; 2], v: &[f32]) -> [f32; 2] {
        if v[5] == 0. && v[4] == 0. {
            for (c, input) in x.iter().enumerate() {
                self.lines[c][self.cursor] = *input;
            }
            self.cursor = (self.cursor + 1) % 4096;
            self.phase += 2. * std::f64::consts::PI * v[0] as f64 / 48000.;
            if self.phase >= 2. * std::f64::consts::PI {
                self.phase -= 2. * std::f64::consts::PI;
            }
            return x;
        }
        let delays = [
            (v[2] + v[1] * ((self.phase.sin() + 1.) * 0.5) as f32) * 48000. / 1000.,
            (v[2]
                + v[1]
                    * (((self.phase + std::f64::consts::PI * v[3] as f64).sin() + 1.) * 0.5)
                        as f32)
                * 48000.
                / 1000.,
        ];
        let mut out = [0.; 2];
        for c in 0..2 {
            let pos = self.cursor as f32 - delays[c].clamp(1., 4094.);
            let floor = pos.floor() as i64;
            let fraction = pos - floor as f32;
            let first = floor.rem_euclid(4096) as usize;
            let wet = self.lines[c][first] * (1. - fraction)
                + self.lines[c][(first + 1) % 4096] * fraction;
            self.lines[c][self.cursor] = x[c] + wet * v[4];
            out[c] = x[c] * (1. - v[5]) + wet * v[5];
        }
        self.cursor = (self.cursor + 1) % 4096;
        self.phase += 2. * std::f64::consts::PI * v[0] as f64 / 48000.;
        if self.phase >= 2. * std::f64::consts::PI {
            self.phase -= 2. * std::f64::consts::PI
        }
        out
    }
}
fn uniform(key: u64) -> f32 {
    let mut v = key.wrapping_add(0x9e3779b97f4a7c15);
    v = (v ^ (v >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    v = (v ^ (v >> 27)).wrapping_mul(0x94d049bb133111eb);
    v ^= v >> 31;
    (v >> 40) as f32 / 16777216.
}
pub fn dither(x: f32, frame: i64, channel: u64, seed: u64, v: &[f32]) -> f32 {
    let step = 2_f32.powf(1. - v[0].round().clamp(8., 24.));
    let key = seed
        ^ (frame as u64).wrapping_mul(0xd6e8feb86659fd93)
        ^ channel.wrapping_mul(0xa0761d6478bd642f);
    let noise = (uniform(key) - uniform(key ^ 0xe7037ed1a0b428db)) * step * v[1];
    let y = x + noise;
    if v[2] >= 0.5 {
        (y / step).round() * step
    } else {
        y
    }
}
