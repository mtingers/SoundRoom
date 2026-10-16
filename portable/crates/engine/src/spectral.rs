//! Small radix-2 FFT and spectrum helpers for offline analysis and synthesis
//! (worker threads only; never called from an audio callback).
use std::f64::consts::PI;

/// In-place complex FFT; `re.len()` must be a power of two. `inverse`
/// computes the unscaled inverse transform.
pub fn fft(re: &mut [f64], im: &mut [f64], inverse: bool) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && im.len() == n);
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let sign = if inverse { 1. } else { -1. };
    let mut len = 2;
    while len <= n {
        let (ws, wc) = (sign * 2. * PI / len as f64).sin_cos();
        for start in (0..n).step_by(len) {
            let (mut c, mut s) = (1., 0.);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * c - im[b] * s, re[b] * s + im[b] * c);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                (c, s) = (c * wc - s * ws, c * ws + s * wc);
            }
        }
        len <<= 1;
    }
}
/// Hann-windowed magnitude spectrum of `n` samples from `start` (zero
/// padded), scaled so a steady sine of amplitude A reads A at its bin.
pub fn spectrum(x: &[f32], start: usize, n: usize) -> Vec<f64> {
    let mut re = vec![0.; n];
    let mut im = vec![0.; n];
    for (i, r) in re.iter_mut().enumerate() {
        let w = 0.5 - 0.5 * (2. * PI * i as f64 / n as f64).cos();
        *r = x.get(start + i).copied().unwrap_or(0.) as f64 * w;
    }
    fft(&mut re, &mut im, false);
    (0..n / 2)
        .map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt() * 4. / n as f64)
        .collect()
}
/// Unit-RMS noise limited to `lo..hi` Hz, `frames` long, from `seed`.
pub fn band_noise(frames: usize, rate: f64, lo: f64, hi: f64, seed: u64) -> Vec<f32> {
    let n = frames.max(2).next_power_of_two();
    let (mut re, mut im) = (vec![0.; n], vec![0.; n]);
    let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407) | 1;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    let bin = rate / n as f64;
    for k in 1..n / 2 {
        let hz = k as f64 * bin;
        if hz >= lo && hz < hi {
            let phase = next() * 2. * PI;
            let (s, c) = phase.sin_cos();
            re[k] = c;
            im[k] = s;
            re[n - k] = c;
            im[n - k] = -s;
        }
    }
    fft(&mut re, &mut im, true);
    let rms = (re.iter().map(|v| v * v).sum::<f64>() / n as f64).sqrt();
    let scale = if rms > 0. { 1. / rms } else { 0. };
    re.iter().take(frames).map(|v| (v * scale) as f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spectrum_reads_sine_amplitude_and_band_noise_is_unit_rms_in_band() {
        let x: Vec<f32> = (0..4096).map(|i| (0.5 * (2. * PI * 1000. * i as f64 / 48000.).sin()) as f32).collect();
        let s = spectrum(&x, 0, 4096);
        let k = (1000_f64 / (48000. / 4096.)).round() as usize;
        let top = s[k - 1..=k + 1].iter().cloned().fold(0., f64::max);
        assert!((top - 0.5).abs() < 0.08, "{top}");
        let noise = band_noise(48000, 48000., 2000., 4000., 7);
        let rms = (noise.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / 48000.).sqrt();
        assert!((rms - 1.).abs() < 0.05, "{rms}");
        let s = spectrum(&noise, 0, 8192);
        let energy = |lo: f64, hi: f64| (0..s.len()).filter(|&k| { let hz = k as f64 * 48000. / 8192.; hz >= lo && hz < hi }).map(|k| s[k] * s[k]).sum::<f64>();
        assert!(energy(2000., 4000.) > 1000. * energy(5000., 20000.));
    }
}
