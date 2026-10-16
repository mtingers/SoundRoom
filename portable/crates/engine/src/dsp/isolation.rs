//! Real-time spectral cleanup and component isolation. These masks discriminate
//! sustained/transient or mid/side content; they do not identify instruments.
//! All FFT, median and overlap-add storage is allocated before processing.
const N: usize = 2048;
const HOP: usize = 512;
const RING: usize = 4096;
const BINS: usize = N / 2 + 1;
const HISTORY: usize = 9;

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

fn median(values: &mut [f32]) -> f32 {
    // Fixed arrays of at most 17 elements: insertion sort has a bounded cost and
    // does not allocate, unlike general-purpose sorting scratch storage.
    for i in 1..values.len() {
        let value = values[i];
        let mut j = i;
        while j > 0 && values[j - 1] > value {
            values[j] = values[j - 1];
            j -= 1;
        }
        values[j] = value;
    }
    values[values.len() / 2]
}

#[derive(Clone)]
pub struct Isolation {
    transform: Transform,
    window: Vec<f32>,
    input: [Vec<f32>; 2],
    output: [Vec<f32>; 2],
    spectra: [Vec<f32>; 2],
    imaginary: [Vec<f32>; 2],
    levels: Vec<f32>,
    history: Vec<f32>,
    masks: Vec<f32>,
    gains: Vec<f32>,
    mix_history: Vec<f32>,
    strength_history: Vec<f32>,
    listen_history: Vec<bool>,
    input_index: usize,
    output_index: usize,
    hop_clock: usize,
    history_index: usize,
    initialized: bool,
    mode: usize,
    strength: f32,
    floor_power: f32,
    low: f32,
    high: f32,
    mix: f32,
    listen: bool,
    pub last_dry: [f32; 2],
    pub reduction_db: f32,
    pub display: [f32; 12],
}
impl Default for Isolation {
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
            levels: vec![0.; BINS],
            history: vec![0.; BINS * HISTORY],
            masks: vec![1.; BINS],
            gains: vec![1.; BINS],
            mix_history: vec![1.; N],
            strength_history: vec![0.75; N],
            listen_history: vec![false; N],
            input_index: 0,
            output_index: 0,
            hop_clock: 0,
            history_index: 0,
            initialized: false,
            mode: 0,
            strength: 0.75,
            // Linked channel power is averaged. sqrt-Hann window energy = N/2.
            floor_power: 10_f32.powf(-55. / 10.) * N as f32 * 0.5,
            low: 20.,
            high: 20000.,
            mix: 1.,
            listen: false,
            last_dry: [0.; 2],
            reduction_db: 0.,
            display: [0.; 12],
        }
    }
}
impl Isolation {
    pub const LATENCY_FRAMES: usize = N;

    /// [mode, strength, broadband noise-floor dBFS, low Hz, high Hz, mix, listen].
    /// Modes: 0 cleanup, 1 sustained/harmonic, 2 transient/percussive, 3 center,
    /// 4 sides. Listen selects the removed component, after mix.
    pub fn configure(&mut self, v: &[f32]) {
        let value = |i: usize, fallback: f32, low: f32, high: f32| {
            let x = v.get(i).copied().unwrap_or(fallback);
            if x.is_finite() {
                x.clamp(low, high)
            } else {
                fallback
            }
        };
        self.mode = value(0, 0., 0., 4.).round() as usize;
        self.strength = value(1, 0.75, 0., 1.);
        self.floor_power = 10_f32.powf(value(2, -55., -90., -20.) / 10.) * N as f32 * 0.5;
        self.low = value(3, 20., 20., 12000.);
        self.high = value(4, 20000., 200., 20000.).max(self.low);
        self.mix = value(5, 1., 0., 1.);
        self.listen = value(6, 0., 0., 1.) >= 0.5;
        if !self.initialized {
            self.mix_history.fill(self.mix);
            self.strength_history.fill(self.strength);
            self.listen_history.fill(self.listen);
            self.initialized = true;
        }
    }

    pub fn sample(&mut self, x: [f32; 2]) -> [f32; 2] {
        let mix = self.mix_history[self.input_index];
        let strength = self.strength_history[self.input_index];
        let listen = self.listen_history[self.input_index];
        let mut wet = [0.; 2];
        for (c, value) in x.iter().enumerate() {
            self.last_dry[c] = self.input[c][self.input_index];
            // Squared bin energies and inverse FFT sums must remain finite,
            // including when a corrupt but finite float enters the graph.
            self.input[c][self.input_index] = if value.is_finite() {
                value.clamp(-16., 16.)
            } else {
                0.
            };
            wet[c] = self.output[c][self.output_index];
            self.output[c][self.output_index] = 0.;
        }
        self.mix_history[self.input_index] = self.mix;
        self.strength_history[self.input_index] = self.strength;
        self.listen_history[self.input_index] = self.listen;
        self.input_index = (self.input_index + 1) % N;
        self.output_index = (self.output_index + 1) % RING;
        self.hop_clock += 1;
        if self.hop_clock == HOP {
            self.analyze();
            self.hop_clock = 0;
        }
        // Bit-exact neutral controls and mix. Wet still runs so restoring it
        // neither resets overlap history nor produces an empty look-ahead gap.
        if mix == 0. || strength == 0. {
            return if listen { [0.; 2] } else { self.last_dry };
        }
        if listen {
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
        for k in 0..BINS {
            let power = 0.5
                * (self.spectra[0][k].powi(2)
                    + self.imaginary[0][k].powi(2)
                    + self.spectra[1][k].powi(2)
                    + self.imaginary[1][k].powi(2));
            self.levels[k] = power.max(0.).sqrt();
            self.history[self.history_index * BINS + k] = self.levels[k];
        }
        self.reduction_db = 0.;
        self.display = [0.; 12];
        for k in 0..BINS {
            let hz = k as f32 * 48000. / N as f32;
            // Raised shoulders retain content at a specified focus boundary;
            // the transition occupies at least two bins, avoiding hard ringing.
            let shoulder = (self.low * 0.15).max(48000. / N as f32 * 2.);
            let upper_shoulder = (self.high * 0.1).max(48000. / N as f32 * 2.);
            let focus = ((hz - self.low) / shoulder)
                .clamp(0., 1.)
                .min(((self.high - hz) / upper_shoulder).clamp(0., 1.));
            let target = match self.mode {
                0 => {
                    let power = self.levels[k] * self.levels[k];
                    let cleaned = (1. - self.floor_power / power.max(1e-20))
                        .clamp(0., 1.)
                        .sqrt();
                    // Clean outside the focus band remains untouched.
                    1. - focus * (1. - cleaned)
                }
                1 | 2 => {
                    let mut temporal = [0.; HISTORY];
                    for (i, x) in temporal.iter_mut().enumerate() {
                        *x = self.history[i * BINS + k];
                    }
                    let harmonic = median(&mut temporal);
                    let mut frequency = [0.; 17];
                    for (i, x) in frequency.iter_mut().enumerate() {
                        let bin =
                            (k as isize + i as isize - 8).clamp(0, (BINS - 1) as isize) as usize;
                        *x = self.levels[bin];
                    }
                    let percussive = median(&mut frequency);
                    let h2 = harmonic * harmonic;
                    let p2 = percussive * percussive;
                    let harmonic_mask = if h2 + p2 > 1e-20 { h2 / (h2 + p2) } else { 0.5 };
                    focus
                        * if self.mode == 1 {
                            harmonic_mask
                        } else {
                            1. - harmonic_mask
                        }
                }
                _ => focus,
            };
            // Cleanup follows a slow opening to reject isolated noise-bin peaks;
            // musical component masks follow attacks without a slow gain ramp.
            if self.mode == 0 {
                let coefficient = if target > self.masks[k] { 0.35 } else { 0.8 };
                self.masks[k] += coefficient * (target - self.masks[k]);
            } else {
                self.masks[k] = target;
            }
            self.gains[k] = 1. - self.strength * (1. - self.masks[k]);
            let measured_gain = if self.mode >= 3 && self.levels[k] > 1e-8 {
                let side_sign = if self.mode == 3 { 1. } else { -1. };
                let weight = self.strength * self.masks[k];
                let dry = 1. - self.strength;
                let re = 0.5 * (self.spectra[0][k] + side_sign * self.spectra[1][k]);
                let im = 0.5 * (self.imaginary[0][k] + side_sign * self.imaginary[1][k]);
                let power = 0.5
                    * ((dry * self.spectra[0][k] + weight * re).powi(2)
                        + (dry * self.spectra[1][k] + side_sign * weight * re).powi(2)
                        + (dry * self.imaginary[0][k] + weight * im).powi(2)
                        + (dry * self.imaginary[1][k] + side_sign * weight * im).powi(2));
                (power.max(0.).sqrt() / self.levels[k]).clamp(0., 1.)
            } else {
                self.gains[k]
            };
            let reduction = if self.levels[k] > 0.001 {
                -20. * measured_gain.max(1e-6).log10()
            } else {
                0.
            };
            self.reduction_db = self.reduction_db.max(reduction);
            let band =
                (((hz.max(80.) / 80.).log2() / (20000_f32 / 80.).log2() * 12.) as usize).min(11);
            self.display[band] = self.display[band].max(reduction);
        }
        self.history_index = (self.history_index + 1) % HISTORY;
        for k in 0..BINS {
            for bin in [k, if k > 0 && k < N / 2 { N - k } else { k }] {
                // DC and Nyquist need one application, not a squared mask.
                if bin == k && (k == 0 || k == N / 2) {
                    self.apply_bin(bin, k);
                    break;
                }
                self.apply_bin(bin, k);
            }
        }
        for c in 0..2 {
            self.transform
                .execute(&mut self.spectra[c], &mut self.imaginary[c], true);
            for i in 0..N {
                self.output[c][(self.output_index + i) % RING] +=
                    self.spectra[c][i] * self.window[i] / (N * 2) as f32;
            }
        }
    }

    fn apply_bin(&mut self, bin: usize, k: usize) {
        if self.mode < 3 {
            for c in 0..2 {
                self.spectra[c][bin] *= self.gains[k];
                self.imaginary[c][bin] *= self.gains[k];
            }
        } else {
            let weight = self.strength * self.masks[k];
            let dry = 1. - self.strength;
            let side_sign = if self.mode == 3 { 1. } else { -1. };
            let real = 0.5 * (self.spectra[0][bin] + side_sign * self.spectra[1][bin]);
            let imaginary = 0.5 * (self.imaginary[0][bin] + side_sign * self.imaginary[1][bin]);
            self.spectra[0][bin] = dry * self.spectra[0][bin] + weight * real;
            self.spectra[1][bin] = dry * self.spectra[1][bin] + side_sign * weight * real;
            self.imaginary[0][bin] = dry * self.imaginary[0][bin] + weight * imaginary;
            self.imaginary[1][bin] = dry * self.imaginary[1][bin] + side_sign * weight * imaginary;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn run(input: &[[f32; 2]], controls: &[f32]) -> Vec<[f32; 2]> {
        let mut dsp = Isolation::default();
        dsp.configure(controls);
        let mut output: Vec<_> = input
            .iter()
            .chain(std::iter::repeat_n(&[0.; 2], N))
            .map(|&x| dsp.sample(x))
            .collect();
        output.drain(..N);
        output
    }
    fn tone(i: usize, hz: f32) -> f32 {
        (2. * PI * hz * i as f32 / 48000.).sin()
    }
    fn rms(x: &[[f32; 2]], start: usize, end: usize) -> f32 {
        (x[start..end].iter().map(|x| x[0] * x[0]).sum::<f32>() / (end - start) as f32).sqrt()
    }

    #[test]
    fn neutral_strength_and_mix_preserve_exact_delayed_samples() {
        let input: Vec<_> = (0..8192)
            .map(|i| [tone(i, 711.) * 0.4, tone(i, 999.) * 0.3])
            .collect();
        assert_eq!(run(&input, &[4., 0., -55., 20., 20000., 1., 0.]), input);
        assert_eq!(run(&input, &[1., 1., -55., 20., 20000., 0., 0.]), input);
    }

    #[test]
    fn center_sides_and_removed_audio_reconstruct_the_mix() {
        let input: Vec<_> = (0..24000)
            .map(|i| {
                let center = 0.25 * tone(i, 750.);
                let side = 0.2 * tone(i, 2250.);
                [center + side, center - side]
            })
            .collect();
        let center = run(&input, &[3., 1., -55., 20., 20000., 1., 0.]);
        let removed = run(&input, &[3., 1., -55., 20., 20000., 1., 1.]);
        let sides = run(&input, &[4., 1., -55., 20., 20000., 1., 0.]);
        for i in 4096..20000 {
            let expected_center = 0.25 * tone(i, 750.);
            let expected_side = 0.2 * tone(i, 2250.);
            assert!(
                (center[i][0] - expected_center).abs() < 0.00005,
                "{i}: {:?}",
                center[i]
            );
            assert!((center[i][0] - center[i][1]).abs() < 0.000001);
            assert!((sides[i][0] - expected_side).abs() < 0.00005);
            assert!((sides[i][0] + sides[i][1]).abs() < 0.000001);
            for c in 0..2 {
                assert!((center[i][c] + removed[i][c] - input[i][c]).abs() < 0.000001);
            }
        }
    }

    #[test]
    fn cleanup_reduces_a_noise_bed_without_erasing_a_strong_tone() {
        let mut seed = 0x123456789u64;
        let mut colored = [[0_f32; 2]; 2];
        let mut noise = |channel: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let white = ((seed >> 40) as f32 / (1u32 << 24) as f32 * 2. - 1.) * 0.025;
            // Test audible in-band noise. Frequencies outside the user-selected
            // cleanup band intentionally remain untouched by this processor.
            colored[channel][0] += 0.5 * (white - colored[channel][0]);
            colored[channel][1] += 0.5 * (colored[channel][0] - colored[channel][1]);
            colored[channel][1]
        };
        let input: Vec<_> = (0..48000)
            .map(|i| {
                let music = if i < 24000 { 0.3 * tone(i, 1500.) } else { 0. };
                [music + noise(0), music + noise(1)]
            })
            .collect();
        let output = run(&input, &[0., 1., -30., 20., 20000., 1., 0.]);
        assert!(
            rms(&output, 30000, 44000) < rms(&input, 30000, 44000) * 0.25,
            "{} / {}",
            rms(&output, 30000, 44000),
            rms(&input, 30000, 44000)
        );
        let amplitude = |x: &[[f32; 2]]| {
            2. * (6000..20000).map(|i| x[i][0] * tone(i, 1500.)).sum::<f32>() / 14000.
        };
        assert!(amplitude(&output) > 0.29, "{}", amplitude(&output));
    }

    #[test]
    fn harmonic_and_percussive_masks_discriminate_sustained_tone_and_attacks() {
        let input: Vec<_> = (0..48000)
            .map(|i| {
                let sine = 0.2 * tone(i, 1500.);
                let impulse = if i % 4800 == 2400 { 0.7 } else { 0. };
                [sine + impulse; 2]
            })
            .collect();
        let harmonic = run(&input, &[1., 1., -55., 20., 20000., 1., 0.]);
        let percussive = run(&input, &[2., 1., -55., 20., 20000., 1., 0.]);
        assert!(rms(&harmonic, 12000, 20000) > rms(&percussive, 12000, 20000) * 5.);
        let attack = |x: &[[f32; 2]]| {
            (12000..43000)
                .step_by(4800)
                .map(|i| x[i][0].abs())
                .sum::<f32>()
        };
        assert!(
            attack(&percussive) > attack(&harmonic) * 2.,
            "{} / {}",
            attack(&percussive),
            attack(&harmonic)
        );
    }

    #[test]
    fn clone_carries_history_and_nonfinite_input_never_poison_future_audio() {
        let mut a = Isolation::default();
        a.configure(&[1., 0.75, -55., 20., 20000., 1., 0.]);
        for i in 0..6000 {
            a.sample([tone(i, 333.); 2]);
        }
        let mut b = a.clone();
        for i in 6000..12000 {
            assert_eq!(a.sample([tone(i, 333.); 2]), b.sample([tone(i, 333.); 2]));
        }
        a.configure(&[f32::NAN; 7]);
        for i in 0..10000 {
            let out = a.sample(if i == 0 {
                [f32::NAN, f32::INFINITY]
            } else {
                [tone(i, 333.); 2]
            });
            assert!(out.iter().all(|x| x.is_finite()));
        }
    }

    #[test]
    fn meter_ignores_empty_bins_and_reports_stereo_component_removal() {
        let mut dsp = Isolation::default();
        dsp.configure(&[3., 1., -55., 20., 20000., 1., 0.]);
        for i in 0..8192 {
            let side = 0.2 * tone(i, 1500.);
            dsp.sample([side, -side]);
        }
        assert!(dsp.reduction_db > 50.);
        for _ in 0..8192 {
            dsp.sample([0.; 2]);
        }
        assert_eq!(dsp.reduction_db, 0.);
        assert_eq!(dsp.display, [0.; 12]);
    }

    #[test]
    fn extreme_finite_input_never_overflows_fft_or_poisons_later_silence() {
        for mode in 0..=4 {
            let mut dsp = Isolation::default();
            dsp.configure(&[mode as f32, 1., -55., 20., 20000., 1., 0.]);
            for i in 0..16000 {
                let out = dsp.sample(if i == 0 { [f32::MAX, 1e20] } else { [0.; 2] });
                assert!(out.iter().all(|v| v.is_finite()), "mode {mode}, frame {i}");
            }
            assert!(dsp.sample([0.; 2]).iter().all(|v| v.abs() < 1e-12));
            assert!(dsp.display.iter().all(|v| v.is_finite()));
        }
    }
}
