//! Texture statistics of recorded vs modeled cymbal tails, per band:
//! bloom (time of peak band energy), tonality (spectral flatness at 0.5 s;
//! 1 = noise, → 0 = tonal), shimmer (std. dev. of the detrended band level
//! in dB over 0.2–1.5 s, 10 ms frames).
use soundroom_engine::{drums::Voice, measured_drums, spectral::spectrum, wave::WaveReader};
fn stats(x: &[f32]) -> Vec<(f64, f64, f64)> {
    let bands = [(1000., 3000.), (3000., 8000.), (8000., 16000.)];
    let frames = (x.len().saturating_sub(4096) / 480).min(200);
    let spec: Vec<Vec<f64>> = (0..frames).map(|f| spectrum(x, f * 480, 2048)).collect();
    bands.iter().map(|&(lo, hi)| {
        let w = 48000. / 2048.;
        let (a, b) = ((lo / w) as usize, (hi / w) as usize);
        let track: Vec<f64> = spec.iter().map(|s| 10. * s[a..b].iter().map(|v| v * v).sum::<f64>().max(1e-20).log10()).collect();
        let peak = track.iter().enumerate().fold((0, f64::MIN), |m, (i, &v)| if v > m.1 { (i, v) } else { m }).0;
        let bloom = peak as f64 * 10.;
        let s = spectrum(x, 24000, 8192);
        let w8 = 48000. / 8192.;
        let bins: Vec<f64> = s[(lo / w8) as usize..(hi / w8) as usize].iter().map(|v| v * v + 1e-30).collect();
        let flat = (bins.iter().map(|v| v.ln()).sum::<f64>() / bins.len() as f64).exp() / (bins.iter().sum::<f64>() / bins.len() as f64);
        let seg: Vec<f64> = track.iter().skip(20).take(130).cloned().collect();
        let n = seg.len() as f64;
        let xs: Vec<f64> = (0..seg.len()).map(|i| i as f64).collect();
        let (mx, my) = (xs.iter().sum::<f64>() / n, seg.iter().sum::<f64>() / n);
        let slope = xs.iter().zip(&seg).map(|(x, y)| (x - mx) * (y - my)).sum::<f64>() / xs.iter().map(|x| (x - mx).powi(2)).sum::<f64>().max(1e-9);
        let resid = (xs.iter().zip(&seg).map(|(x, y)| (y - my - slope * (x - mx)).powi(2)).sum::<f64>() / n).sqrt();
        (bloom, flat, resid)
    }).collect()
}
fn main() {
    let kit = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    for (voice, dir) in [(Voice::Crash, "crash/center"), (Voice::Ride, "ride/center"), (Voice::OpenHat, "openHat/center")] {
        let mut files: Vec<_> = std::fs::read_dir(kit.join(dir)).unwrap().flatten().map(|e| e.path()).collect();
        files.sort();
        let file = &files[files.len() * 3 / 4];
        let mut r = WaveReader::open(file).unwrap();
        let n = r.info.frames as usize;
        let mut st = vec![vec![0f32; n]; 6];
        let mut at = 0;
        while at < n { let k = 8192.min(n - at); let mut v: Vec<&mut [f32]> = st.iter_mut().map(|s| &mut s[at..at + k]).collect(); r.read_planar(at as u64, &mut v).unwrap(); at += k; }
        let real: Vec<f32> = (0..n).map(|i| st[2][i] + st[3][i]).collect();
        let model = measured_drums::direct_hit(voice, "center", 0.75, 1).unwrap();
        for (name, x) in [("real ", &real), ("model", &model)] {
            let s = stats(x);
            println!("{:<8} {name}  {}", voice.name(), s.iter().map(|(b, f, m)| format!("bloom {b:4.0}ms flat {f:.3} shimmer {m:4.2}dB | ")).collect::<String>());
        }
    }
}
