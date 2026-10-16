//! Audio-driven adaptation of the user's Sound Putty / Deep 300 field.
//! All 300 damped oscillators couple to their immediate neighbours. Stereo
//! histories are independent; coefficients and seeded geometry are shared.
use std::{collections::BTreeMap, f64::consts::TAU};

pub const LANES: usize = 300;
const RATE: f64 = 48000.;
const CONTROL: usize = 64;
pub type Shape = [[f64; LANES]; 3];

pub fn shape(parameters: &BTreeMap<String, f32>) -> Shape {
    let mut result = [[0.; LANES]; 3];
    for (row, prefix) in result
        .iter_mut()
        .zip(["puttyPitch", "puttyEnergy", "puttyDamping"])
    {
        for (i, value) in row.iter_mut().enumerate() {
            *value = parameters
                .get(&format!("{prefix}{i:03}"))
                .copied()
                .filter(|v| v.is_finite())
                .unwrap_or(0.)
                .clamp(-1., 1.) as f64;
        }
    }
    result
}

#[derive(Clone, Copy)]
struct Coefficients {
    spring: [f64; LANES],
    damping: [f64; LANES],
    coupling: [f64; LANES],
    weight: [f64; LANES],
}
impl Default for Coefficients {
    fn default() -> Self {
        Self {
            spring: [0.; LANES],
            damping: [0.; LANES],
            coupling: [0.; LANES],
            weight: [0.; LANES],
        }
    }
}

/// Same integer hash as the supplied JavaScript, without platform RNG state.
pub fn random(lane: usize, seed: u32) -> f64 {
    let mut n = (lane as u32 + 1).wrapping_mul(2654435761) ^ seed;
    n ^= n >> 16;
    n = n.wrapping_mul(2246822519);
    n ^= n >> 13;
    n as f64 / 4294967295. * 2. - 1.
}

#[derive(Clone)]
pub struct SoundPutty {
    x: [[f64; LANES]; 2],
    velocity: [[f64; LANES]; 2],
    next: [[f64; LANES]; 2],
    shape: Shape,
    controls: [f64; 16],
    coefficients: Coefficients,
    target: Coefficients,
    dirty: bool,
    initialized: bool,
    gliding: bool,
    phase: usize,
    drive_gain: f64,
    output_gain: f64,
    center: f64,
    blow_alpha: f64,
    previous_input: [f64; 2],
    dc: [f64; 2],
    blow: [f64; 2],
}
impl Default for SoundPutty {
    fn default() -> Self {
        let center = (95_f64 * 560.).sqrt();
        Self {
            x: [[0.; LANES]; 2],
            velocity: [[0.; LANES]; 2],
            next: [[0.; LANES]; 2],
            shape: [[0.; LANES]; 3],
            controls: [
                95., 560., 1.2, 160., 22., 0.7, 0., 0., 145., 85., 0.075, 1.8, 0., 0.4, 0., 1234.,
            ],
            coefficients: Default::default(),
            target: Default::default(),
            dirty: true,
            initialized: false,
            gliding: false,
            phase: 0,
            drive_gain: 1.,
            output_gain: 1.,
            center,
            blow_alpha: 1. - (-TAU * center * 2. / RATE).exp(),
            previous_input: [0.; 2],
            dc: [0.; 2],
            blow: [0.; 2],
        }
    }
}
impl SoundPutty {
    pub fn set_shape(&mut self, shape: &Shape) {
        self.shape = *shape;
        self.dirty = true;
    }
    pub fn configure(&mut self, values: &[f32]) {
        for (i, (a, b)) in self.controls.iter_mut().zip(values).enumerate() {
            let value = *b as f64;
            if (i < 12 || i == 15) && *a != value {
                self.dirty = true;
            }
            *a = value;
        }
        self.drive_gain = 10_f64.powf(self.controls[12] / 20.);
        self.output_gain = 10_f64.powf(self.controls[14] / 20.);
        self.center = (self.controls[0] * self.controls[1]).sqrt();
        self.blow_alpha = 1. - (-TAU * (self.center * 2.).min(12000.) / RATE).exp();
    }
    fn rebuild(&mut self) {
        let p = self.controls;
        for i in 0..LANES {
            let q = (i as f64 / (LANES - 1) as f64).powf(p[2]);
            let zone = (-0.5 * ((i as f64 - p[3]) / p[4]).powi(2)).exp() * p[5];
            let mut frequency = p[0] * (p[1] / p[0]).powf(q) * (self.shape[0][i] * 0.65).exp();
            if p[6].round() == 0. {
                frequency *= (zone * random(i, p[15].round() as u32) * 1.5).exp();
            }
            let damping = ((self.shape[2][i] * 2.).exp() / p[11]
                + if p[6].round() == 2. { zone * 15. } else { 0. })
            .max(0.25);
            // Semi-implicit Euler at 48 kHz. The upper bound keeps omega*dt
            // below 1.81, including sculpt/detune extremes and neighbour forces.
            self.target.spring[i] = (TAU * frequency.clamp(15., 13800.) / RATE).powi(2);
            self.target.damping[i] = 2. * damping / RATE;
            self.target.coupling[i] =
                (p[10] + if p[6].round() == 3. { zone * 0.28 } else { 0. }) * 1e6 / (RATE * RATE);
            self.target.weight[i] = (-0.5 * ((i as f64 - p[8]) / p[9]).powi(2)).exp()
                * (self.shape[1][i] * 2.).exp()
                * if p[6].round() == 1. {
                    1. - 2. * zone
                } else {
                    1.
                };
        }
        if !self.initialized {
            self.coefficients = self.target;
            self.initialized = true;
        } else {
            self.gliding = true;
        }
        self.dirty = false;
    }
    pub fn sample(&mut self, input: [f32; 2]) -> [f32; 2] {
        if self.phase == 0 && self.dirty {
            self.rebuild();
        }
        let remaining = (CONTROL - self.phase) as f64;
        let c = &mut self.coefficients;
        // A bounded coefficient glide also smooths freshly painted curves.
        if self.gliding {
            for i in 0..LANES {
                c.spring[i] += (self.target.spring[i] - c.spring[i]) / remaining;
                c.damping[i] += (self.target.damping[i] - c.damping[i]) / remaining;
                c.coupling[i] += (self.target.coupling[i] - c.coupling[i]) / remaining;
                c.weight[i] += (self.target.weight[i] - c.weight[i]) / remaining;
            }
        }
        let p = self.controls;
        let mut result = input;
        for channel in 0..2 {
            let dry = if input[channel].is_finite() {
                input[channel] as f64
            } else {
                0.
            };
            let source = (dry * self.drive_gain).clamp(-4., 4.);
            self.dc[channel] += 0.002614570 * (source - self.dc[channel]); // 20 Hz input DC rejection.
            let drive = source - self.dc[channel];
            self.blow[channel] += self.blow_alpha * (drive - self.blow[channel]);
            let excitation = match p[7].round() as u8 {
                0 => ((drive - self.previous_input[channel]) * RATE / (TAU * self.center))
                    .clamp(-4., 4.),
                1 => drive,
                _ => self.blow[channel],
            };
            self.previous_input[channel] = drive;
            for i in 0..LANES {
                let x = self.x[channel][i];
                let left = self.x[channel][i.saturating_sub(1)];
                let right = self.x[channel][(i + 1).min(LANES - 1)];
                self.next[channel][i] = (1. - c.damping[i]) * self.velocity[channel][i]
                    - c.spring[i] * x
                    + c.coupling[i] * (left + right - 2. * x)
                    + c.weight[i] * excitation * (100000. / (RATE * RATE));
            }
            let mut sum = 0.;
            for i in 0..LANES {
                let velocity = self.next[channel][i];
                let value = self.x[channel][i] + velocity;
                // Bounded states contain extreme edits/hostile inputs. Ordinary
                // motion is linear; only pathological excitation hits this guard.
                if !value.is_finite() || !velocity.is_finite() {
                    self.x[channel][i] = 0.;
                    self.velocity[channel][i] = 0.;
                } else {
                    self.x[channel][i] = if value.abs() < 1e-20 {
                        0.
                    } else {
                        value.clamp(-32., 32.)
                    };
                    self.velocity[channel][i] = velocity.clamp(-32., 32.);
                }
                sum += self.x[channel][i];
            }
            let wet = (sum * (4. / LANES as f64)).tanh();
            result[channel] = (dry * (1. - p[13]) + wet * p[13] * self.output_gain) as f32;
        }
        self.phase = (self.phase + 1) % CONTROL;
        if self.phase == 0 {
            self.gliding = false;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn values(edits: &[(&str, f32)]) -> Vec<f32> {
        let mut p = crate::render::default_parameters("soundPutty");
        for (key, value) in edits {
            p.insert((*key).into(), *value);
        }
        crate::render::descriptors("soundPutty")
            .iter()
            .map(|d| p[d.key])
            .collect()
    }
    #[test]
    fn silence_stays_silent_dry_is_exact_and_channels_do_not_leak() {
        let mut effect = SoundPutty::default();
        effect.configure(&values(&[("mix", 1.)]));
        for _ in 0..1024 {
            assert_eq!(effect.sample([0.; 2]), [0.; 2]);
        }
        for i in 0..4096 {
            let output = effect.sample([if i == 0 { 1. } else { 0. }, 0.]);
            assert_eq!(output[1], 0.);
            assert!(output[0].is_finite());
        }
        effect.configure(&values(&[("mix", 0.), ("output", 12.)]));
        for i in 0..1024 {
            let input = [(i as f32 * 0.13).sin() * 0.2, -0.31];
            assert_eq!(effect.sample(input), input);
        }
        assert!(effect
            .sample([f32::NAN, f32::INFINITY])
            .iter()
            .all(|x| x.is_finite()));
    }
    #[test]
    fn field_endpoints_and_last_lane_sculpting_are_real() {
        let mut effect = SoundPutty::default();
        effect.configure(&values(&[
            ("intensity", 0.),
            ("exciteAt", 299.),
            ("spread", 1.),
        ]));
        let mut sculpt = [[0.; LANES]; 3];
        sculpt[0][299] = 1.;
        sculpt[1][299] = 1.;
        sculpt[2][299] = -1.;
        effect.set_shape(&sculpt);
        effect.sample([0.; 2]);
        assert!((effect.target.spring[0] - (TAU * 95. / RATE).powi(2)).abs() < 1e-12);
        assert!(
            (effect.target.spring[299] - (TAU * 560. * 0.65_f64.exp() / RATE).powi(2)).abs()
                < 1e-12
        );
        assert!((effect.target.weight[299] - 2_f64.exp()).abs() < 1e-10);
        assert_eq!(effect.target.damping[299], 0.5 / RATE); // Bounded longest decay.
        assert!(shape(&BTreeMap::from([("puttyPitch299".into(), 99.)]))[0][299] == 1.);
    }
    #[test]
    fn motion_propagates_through_neighbours_and_decay_releases() {
        let mut independent = SoundPutty::default();
        independent.configure(&values(&[("coupling", 0.), ("intensity", 0.)]));
        let mut coupled = SoundPutty::default();
        coupled.configure(&values(&[("coupling", 0.3), ("intensity", 0.)]));
        independent.x[0][145] = 0.1;
        coupled.x[0][145] = 0.1;
        for _ in 0..1024 {
            independent.sample([0.; 2]);
            coupled.sample([0.; 2]);
        }
        assert_eq!(independent.x[0][150], 0.);
        assert!(coupled.x[0][150].abs() > 1e-8);
        let mut decay = SoundPutty::default();
        decay.configure(&values(&[
            ("decay", 0.1),
            ("coupling", 0.),
            ("intensity", 0.),
            ("mix", 1.),
            ("excite", 1.),
        ]));
        let mut early = 0.;
        let mut late = 0.;
        for i in 0..48000 {
            let y = decay.sample([
                if i < 2000 {
                    (i as f32 * 0.017).sin() * 0.3
                } else {
                    0.
                },
                0.,
            ])[0] as f64;
            if (2000..8000).contains(&i) {
                early += y * y;
            }
            if i >= 42000 {
                late += y * y;
            }
        }
        assert!(early > 1e-8);
        assert!(late < early * 1e-4, "tail: {early} -> {late}");
    }
    #[test]
    fn modes_extremes_and_live_sculpting_remain_finite_and_deterministic() {
        for wildcard in 0..4 {
            for excitation in 0..3 {
                let mut a = SoundPutty::default();
                a.configure(&values(&[
                    ("start", 800.),
                    ("end", 1600.),
                    ("curve", 0.2),
                    ("width", 90.),
                    ("intensity", 1.),
                    ("wildMode", wildcard as f32),
                    ("excite", excitation as f32),
                    ("coupling", 0.3),
                    ("decay", 5.),
                    ("drive", 24.),
                    ("output", 12.),
                    ("mix", 1.),
                ]));
                let mut b = a.clone();
                for i in 0..4096 {
                    if i == 1024 {
                        let shape = [[1.; LANES], [1.; LANES], [-1.; LANES]];
                        a.set_shape(&shape);
                        b.set_shape(&shape);
                    }
                    let x = [(i as f32 * 0.33).sin() * 5., (i as f32 * 0.71).cos() * 4.];
                    let y = a.sample(x);
                    assert_eq!(y, b.sample(x));
                    assert!(y.iter().all(|x| x.is_finite() && x.abs() <= 4.));
                }
            }
        }
    }
}
