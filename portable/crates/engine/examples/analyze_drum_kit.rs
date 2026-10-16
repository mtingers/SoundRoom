//! Measures what makes recorded drum hits sound real and writes a compact
//! generative model per kit piece (`measured_drums.rs` synthesizes from it).
//!
//! analyze_drum_kit <kit-dir with hits.jsonl> <out.json>
//!
//! Per hit, on the direct sound (close stems; overheads for cymbals):
//! * modes: spectral peaks tracked over time → frequency, level, T60,
//! * pitch glide: early frequency drop of the strongest mode (tension),
//! * noise: 24 log bands with partial bins removed → peak level, fast and
//!   slow decay and the knee between them,
//! * space: overhead and room energy relative to the direct sound, and the
//!   room's per-band decay.
//!
//! Each parameter is then fitted linearly against velocity (normalized log
//! power), with the residual spread kept for round-robin variation.
use soundroom_engine::wave::WaveReader;
use std::collections::BTreeMap;
use std::f64::consts::PI;
use std::path::Path;

const RATE: f64 = 48_000.;
const BANDS: usize = 24;
const MODES: usize = 24;
/// Per matched mode: (velocity, Hz, dB, ln fast T60, ln slow T60, slow dB).
type ModeSample = (f64, f64, f64, f64, f64, f64);

fn spectrum(x: &[f32], start: usize, n: usize) -> Vec<f64> {
    soundroom_engine::spectral::spectrum(x, start, n)
}
/// Amplitude envelope (dB, 5 ms hops) of the component near `hz`, by complex
/// demodulation and a triangular low-pass (two box filters) whose length
/// trades time resolution against selectivity.
fn mode_envelope(x: &[f32], hz: f64, seconds: f64, dense: bool) -> Vec<f64> {
    let n = x.len().min((seconds * RATE) as usize);
    // Dense (cymbal) spectra need narrow filters to isolate one mode.
    let half = ((2. * RATE / hz).clamp(if dense { 1536. } else { 240. }, 2048.)) as usize;
    let (mut re, mut im) = (vec![0.; n + 1], vec![0.; n + 1]);
    let w = 2. * PI * hz / RATE;
    for i in 0..n {
        let (s, c) = (w * i as f64).sin_cos();
        re[i + 1] = re[i] + x[i] as f64 * c;
        im[i + 1] = im[i] - x[i] as f64 * s;
    }
    let box1 = |p: &[f64], i: usize| if i + 1 >= half { p[i + 1] - p[i + 1 - half] } else { p[i + 1] };
    let (b1r, b1i): (Vec<f64>, Vec<f64>) = (0..n).map(|i| (box1(&re, i), box1(&im, i))).unzip();
    let prefix = |v: &[f64]| { let mut p = vec![0.; v.len() + 1]; for i in 0..v.len() { p[i + 1] = p[i] + v[i]; } p };
    let (p2r, p2i) = (prefix(&b1r), prefix(&b1i));
    let norm = (half * half) as f64;
    // Report each frame at the triangle centre (delay of `half` samples).
    (0..)
        .map(|f| f * 240 + half)
        .take_while(|&i| i < n)
        .map(|i| {
            let (r, m) = (box1(&p2r, i), box1(&p2i, i));
            db(2. * (r * r + m * m).sqrt() / norm)
        })
        .collect()
}
/// Two-stage decay of a dB track sampled every 5 ms from t = 0:
/// (level at the strike, fast T60, slow T60, slow-stage dB at t=0 re strike).
fn two_stage(track: &[f64], floor: f64) -> (f64, f64, f64, f64) {
    let dt = 240. / RATE;
    let top = track.iter().cloned().fold(f64::MIN, f64::max);
    let at = track.iter().position(|&v| v == top).unwrap_or(0);
    let times: Vec<f64> = (0..track.len()).map(|i| i as f64 * dt).collect();
    let fast_end = (at..track.len()).find(|&i| track[i] < top - 20.).unwrap_or(track.len() - 1).max(at + 2).min(track.len() - 1);
    let (a, fs, _) = fit(&times[at..=fast_end], &track[at..=fast_end]);
    let fast = if fs < -1e-3 { (-60. / fs).clamp(0.005, 6.) } else { 6. };
    let level = a.clamp(top, top + 6.);
    let (slow, slow_db) = match (at..track.len()).find(|&i| track[i] < top - 25.) {
        Some(from) => {
            let idx: Vec<usize> = (from..track.len()).take_while(|&i| track[i] > floor + 6.).collect();
            if idx.len() >= 3 {
                let (a, b, _) = fit(&idx.iter().map(|&i| times[i]).collect::<Vec<_>>(), &idx.iter().map(|&i| track[i]).collect::<Vec<_>>());
                if b < -1e-3 { ((-60. / b).clamp(fast, 8.), (a - level).min(-6.)) } else { (fast, -120.) }
            } else { (fast, -120.) }
        }
        None => (fast, -120.),
    };
    (level, fast, slow, slow_db)
}
/// Mode matching tolerance: low drum modes move more with dynamics (tension).
fn tolerance(hz: f64) -> f64 {
    0.04 + 0.1 * (150. / hz).min(1.)
}
fn band_edges() -> Vec<f64> {
    (0..=BANDS).map(|b| 30. * (18_000f64 / 30.).powf(b as f64 / BANDS as f64)).collect()
}
fn db(x: f64) -> f64 {
    20. * x.max(1e-12).log10()
}
/// Least-squares line y = a + b x.
fn fit(xs: &[f64], ys: &[f64]) -> (f64, f64, f64) {
    let n = xs.len() as f64;
    if n < 2. {
        return (ys.first().copied().unwrap_or(0.), 0., 0.);
    }
    let (mx, my) = (xs.iter().sum::<f64>() / n, ys.iter().sum::<f64>() / n);
    let sxx: f64 = xs.iter().map(|x| (x - mx).powi(2)).sum();
    let sxy: f64 = xs.iter().zip(ys).map(|(x, y)| (x - mx) * (y - my)).sum();
    let b = if sxx > 0. { sxy / sxx } else { 0. };
    let a = my - b * mx;
    let spread = (xs.iter().zip(ys).map(|(x, y)| (y - a - b * x).powi(2)).sum::<f64>() / n).sqrt();
    (a, b, spread)
}
/// Decay of a dB track whose frame `i` is centred at `t0 + i * dt` seconds:
/// (T60, level extrapolated back to t = 0), fitted from the track maximum
/// until it is within 6 dB of `floor`.
fn decay(track: &[f64], t0: f64, dt: f64, floor: f64) -> (f64, f64) {
    let peak = track.iter().enumerate().fold((0, f64::MIN), |m, (i, &v)| if v > m.1 { (i, v) } else { m });
    let idx: Vec<usize> = (peak.0..track.len()).take_while(|&i| track[i] > floor + 6.).collect();
    if idx.len() < 3 {
        return (dt * 2., peak.1);
    }
    let xs: Vec<f64> = idx.iter().map(|&i| t0 + i as f64 * dt).collect();
    let ys: Vec<f64> = idx.iter().map(|&i| track[i]).collect();
    let (a, slope, _) = fit(&xs, &ys);
    if slope >= -1e-3 {
        return (8., peak.1);
    }
    // Never extrapolate more than 12 dB above what was measured.
    ((-60. / slope).clamp(0.005, 8.), a.min(peak.1 + 12.))
}
fn t60(track: &[f64], dt: f64, floor: f64) -> f64 {
    decay(track, 0., dt, floor).0
}

struct HitAnalysis {
    velocity: f64,
    peak_db: f64,
    modes: Vec<(f64, f64, f64, f64, f64)>, // (Hz, dB re peak, fast T60, slow T60, slow dB)
    glide: (f64, f64),           // (ratio at onset - 1, time constant)
    noise: Vec<(f64, f64, f64, f64)>, // per band: (dB re peak, fast T60, slow T60, slow-stage dB at t=0 re start)
    overhead_ratio: f64,
    room_ratio: f64,
    room_t60: Vec<f64>,
    room_delay: f64,
    mix_gain: f64,
    /// Time (s) at which the 1–16 kHz energy peaks (cymbal swell).
    bloom: f64,
}

fn analyze(stems: &[Vec<f32>], velocity: f64, cymbal: bool, reference: Option<&[f64]>) -> HitAnalysis {
    let n = stems[0].len();
    let mono = |a: usize, b: usize| -> Vec<f32> { (0..n).map(|i| 0.5 * (stems[a][i] + stems[b][i])).collect() };
    let close = mono(0, 1);
    let overhead = mono(2, 3);
    let room = mono(4, 5);
    let direct = if cymbal { &overhead } else { &close };
    let energy = |x: &[f32]| x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>();
    let peak = direct.iter().fold(0f32, |m, v| m.max(v.abs())) as f64;
    // Modes from a long window starting 5 ms in (past the stick transient).
    let size = 16384;
    let start = (0.005 * RATE) as usize;
    let spec = spectrum(direct, start, size);
    let bin_hz = RATE / size as f64;
    let mut peaks: Vec<(usize, f64)> = (3..spec.len() - 3)
        .filter(|&k| k as f64 * bin_hz > 30. && k as f64 * bin_hz < 16_000.)
        .filter(|&k| (1..=3).all(|d| spec[k] > spec[k - d] && spec[k] >= spec[k + d]))
        .map(|k| (k, spec[k]))
        .collect();
    peaks.sort_by(|a, b| b.1.total_cmp(&a.1));
    // Keep prominent, well-separated peaks; with a reference set, measure
    // exactly those modes (±4 %) so every hit contributes to every mode.
    let mut chosen: Vec<(usize, f64)> = vec![];
    if let Some(reference) = reference {
        for &hz in reference {
            let tol = tolerance(hz);
            let (lo, hi) = (((hz * (1. - tol)) / bin_hz) as usize, (((hz * (1. + tol)) / bin_hz) as usize).min(spec.len() - 2));
            if let Some(k) = (lo.max(2)..=hi).max_by(|&a, &b| spec[a].total_cmp(&spec[b])) {
                chosen.push((k, spec[k]));
            }
        }
        peaks.clear();
    }
    for (k, m) in peaks {
        let local = (k.saturating_sub(40)..(k + 40).min(spec.len())).map(|i| spec[i]).sum::<f64>() / 80.;
        if m > 3. * local && chosen.iter().all(|(c, _)| (*c as i64 - k as i64).abs() > 6) {
            chosen.push((k, m));
        }
        if chosen.len() == MODES {
            break;
        }
    }
    // Each mode's envelope by demodulation, then a two-stage decay fit.
    let modes: Vec<(f64, f64, f64, f64, f64)> = chosen
        .iter()
        .map(|&(k, _)| {
            // Parabolic interpolation for the frequency.
            let (a, b, c) = (spec[k - 1].max(1e-12).ln(), spec[k].max(1e-12).ln(), spec[k + 1].max(1e-12).ln());
            let offset = 0.5 * (a - c) / (a - 2. * b + c);
            let hz = (k as f64 + offset) * bin_hz;
            let track = mode_envelope(direct, hz, 3., cymbal);
            let floor = track[track.len().saturating_sub(20)..].iter().sum::<f64>() / 20f64.min(track.len() as f64).max(1.);
            let (level, fast, slow, slow_db) = two_stage(&track, floor);
            (hz, level - db(peak), fast, slow, slow_db)
        })
        .collect();
    // Glide of the strongest low mode: frequency over the first 150 ms.
    let glide = modes
        .iter()
        .filter(|m| m.0 < 1200.)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|&(hz, ..)| {
            let (size, hop) = (2048, 96);
            let points: Vec<(f64, f64)> = (0..(0.15 * RATE) as usize / hop)
                .filter_map(|f| {
                    let s = spectrum(direct, f * hop, size);
                    let w = RATE / size as f64;
                    let lo = ((hz * 0.8) / w) as usize;
                    let hi = (((hz * 1.5) / w) as usize).min(s.len() - 2);
                    let k = (lo.max(1)..=hi).max_by(|&a, &b| s[a].total_cmp(&s[b]))?;
                    let (a, b, c) = (s[k - 1].max(1e-12).ln(), s[k].max(1e-12).ln(), s[k + 1].max(1e-12).ln());
                    let off = 0.5 * (a - c) / (a - 2. * b + c);
                    Some(((f * hop + size / 2) as f64 / RATE, (k as f64 + off) * w))
                })
                .collect();
            if points.len() < 6 {
                return (0., 0.03);
            }
            let early = points[..3].iter().map(|p| p.1).sum::<f64>() / 3.;
            let ratio = (early / hz - 1.).clamp(0., 0.6);
            // Time for the excess to fall by 1/e.
            let target = hz * (1. + ratio / std::f64::consts::E);
            let tau = points.iter().find(|p| p.1 <= target).map(|p| p.0).unwrap_or(0.05).clamp(0.004, 0.2);
            (ratio, tau)
        })
        .unwrap_or((0., 0.03));
    // Noise bands: STFT frames with mode bins notched out.
    let edges = band_edges();
    let (nsize, nhop) = (2048, 240);
    let nframes = ((n.saturating_sub(nsize)) / nhop).min(400);
    let mut bands = vec![vec![0.; nframes]; BANDS];
    for f in 0..nframes {
        let s = spectrum(direct, f * nhop, nsize);
        let w = RATE / nsize as f64;
        for (b, band) in bands.iter_mut().enumerate() {
            let (lo, hi) = ((edges[b] / w) as usize, ((edges[b + 1] / w) as usize).max((edges[b] / w) as usize + 1));
            let bins: Vec<f64> = (lo..hi.min(s.len()))
                .filter(|&k| modes.iter().all(|m| ((k as f64 * w) - m.0).abs() > 1.5 * w))
                .map(|k| s[k] * s[k])
                .collect();
            // Σ|X|² over a band's bins is 3× that band's time-domain power
            // for Hann-windowed noise at this scaling.
            band[f] = if bins.is_empty() { 1e-24 } else { bins.iter().sum::<f64>() / bins.len() as f64 * (hi - lo) as f64 / 3. };
        }
    }
    let noise: Vec<(f64, f64, f64, f64)> = bands
        .iter()
        .map(|band| {
            let track: Vec<f64> = band.iter().map(|e| 10. * e.max(1e-24).log10()).collect();
            let top = track.iter().cloned().fold(f64::MIN, f64::max);
            let at = track.iter().position(|&v| v == top).unwrap_or(0);
            let nf = track[track.len().saturating_sub(10)..].iter().sum::<f64>() / 10f64.min(track.len() as f64).max(1.);
            let times: Vec<f64> = (0..track.len()).map(|i| (i * nhop + nsize / 2) as f64 / RATE).collect();
            // Fast stage: the first 20 dB of decay after the maximum.
            let fast_end = (at..track.len()).find(|&i| track[i] < top - 20.).unwrap_or(track.len() - 1).max(at + 2).min(track.len() - 1);
            let (_, fs, _) = fit(&times[at..=fast_end], &track[at..=fast_end]);
            let fast = if fs < -1e-3 { (-60. / fs).clamp(0.005, 4.) } else { 4. };
            // Slow stage: from 25 dB down until near the noise floor.
            let slow_from = (at..track.len()).find(|&i| track[i] < top - 25.);
            let (slow, slow_db) = match slow_from {
                Some(from) => {
                    let idx: Vec<usize> = (from..track.len()).take_while(|&i| track[i] > nf + 6.).collect();
                    if idx.len() >= 3 {
                        let xs: Vec<f64> = idx.iter().map(|&i| times[i]).collect();
                        let ys: Vec<f64> = idx.iter().map(|&i| track[i]).collect();
                        let (a, b, _) = fit(&xs, &ys);
                        if b < -1e-3 { ((-60. / b).clamp(fast, 8.), (a - top).min(-6.)) } else { (fast, -120.) }
                    } else {
                        (fast, -120.)
                    }
                }
                None => (fast, -120.),
            };
            // Level at the strike: the maximum, extrapolated back along the
            // fast slope from the analysis window centre (at most +6 dB).
            let level = top + (60. / fast * times[at]).min(6.);
            (level - db(peak), fast, slow, slow_db)
        })
        .collect();
    // Space: overhead/room energy relative to the direct sound and room decay.
    let room_peak = room.iter().enumerate().fold((0, 0f32), |m, (i, v)| if v.abs() > m.1 { (i, v.abs()) } else { m });
    let direct_peak = direct.iter().position(|v| v.abs() as f64 >= peak * 0.999).unwrap_or(0);
    let room_t60: Vec<f64> = (0..BANDS)
        .step_by(4)
        .map(|b| {
            let frames = ((n.saturating_sub(1024)) / 480).min(300);
            let track: Vec<f64> = (0..frames)
                .map(|f| {
                    let s = spectrum(&room, f * 480, 1024);
                    let w = RATE / 1024.;
                    let (lo, hi) = ((edges[b] / w) as usize, ((edges[(b + 4).min(BANDS)] / w) as usize).min(s.len()));
                    10. * ((lo..hi.max(lo + 1)).map(|k| s[k.min(s.len() - 1)].powi(2)).sum::<f64>()).max(1e-24).log10()
                })
                .collect();
            let nf = track[track.len().saturating_sub(10)..].iter().sum::<f64>() / 10f64.min(track.len() as f64).max(1.);
            t60(&track, 0.01, nf)
        })
        .collect();
    // Level of this piece in the recorded natural mix (close + 0.8 overhead
    // + 0.45 room, the StudioNatural balance) relative to its direct sound.
    let mix: Vec<f32> = (0..n).map(|i| close[i] + 0.8 * overhead[i] + 0.45 * room[i]).collect();
    let bloom = {
        let frames = (n.saturating_sub(2048) / 480).min(60);
        let track: Vec<f64> = (0..frames)
            .map(|f| {
                let s = spectrum(direct, f * 480, 2048);
                let w = RATE / 2048.;
                (((1000. / w) as usize)..((16000. / w) as usize).min(s.len())).map(|k| s[k] * s[k]).sum::<f64>()
            })
            .collect();
        track.iter().enumerate().fold((0, f64::MIN), |m, (i, &v)| if v > m.1 { (i, v) } else { m }).0 as f64 * 0.01
    };
    HitAnalysis {
        bloom,
        mix_gain: (energy(&mix) / energy(direct).max(1e-24)).sqrt(),
        velocity,
        peak_db: db(peak),
        modes,
        glide,
        noise,
        overhead_ratio: (energy(&overhead) / energy(direct).max(1e-24)).sqrt(),
        room_ratio: (energy(&room) / energy(direct).max(1e-24)).sqrt(),
        room_t60,
        room_delay: (room_peak.0 as f64 - direct_peak as f64).max(0.) / RATE,
    }
}

fn read(path: &Path) -> Vec<Vec<f32>> {
    let mut reader = WaveReader::open(path).unwrap();
    let frames = reader.info.frames as usize;
    let mut stems = vec![vec![0f32; frames]; reader.info.channels as usize];
    let mut at = 0;
    while at < frames {
        let n = 8192.min(frames - at);
        let mut views: Vec<&mut [f32]> = stems.iter_mut().map(|s| &mut s[at..at + n]).collect();
        reader.read_planar(at as u64, &mut views).unwrap();
        at += n;
    }
    for s in &mut stems {
        for v in s.iter_mut() {
            *v *= 2.;
        }
    }
    stems
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let kit = Path::new(&args[1]);
    let mut groups: BTreeMap<(String, String), Vec<(String, f64)>> = BTreeMap::new();
    for line in std::fs::read_to_string(kit.join("hits.jsonl")).unwrap().lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        groups
            .entry((v["voice"].as_str().unwrap().into(), v["articulation"].as_str().unwrap().into()))
            .or_default()
            .push((v["file"].as_str().unwrap().into(), v["power"].as_f64().unwrap()));
    }
    let holdout_every = 4; // every 4th layer is held out for validation
    let mut models = serde_json::Map::new();
    for ((voice, articulation), mut hits) in groups {
        hits.sort_by(|a, b| a.1.total_cmp(&b.1));
        let (lo, hi) = (hits[0].1.ln(), hits[hits.len() - 1].1.ln());
        let cymbal = matches!(voice.as_str(), "crash" | "ride" | "openHat" | "closedHat");
        // Reference modes from the loudest training hit.
        let loudest = (0..hits.len()).rev().find(|i| i % holdout_every != holdout_every - 1).unwrap();
        let reference: Vec<f64> = analyze(&read(&kit.join(&hits[loudest].0)), 1., cymbal, None).modes.iter().map(|m| m.0).collect();
        let analyses: Vec<(bool, HitAnalysis)> = hits
            .iter()
            .enumerate()
            .map(|(i, (file, power))| {
                let v = if hi > lo { (power.ln() - lo) / (hi - lo) } else { 1. };
                (i % holdout_every == holdout_every - 1, analyze(&read(&kit.join(file)), v, cymbal, Some(&reference)))
            })
            .collect();
        let train: Vec<&HitAnalysis> = analyses.iter().filter(|a| !a.0).map(|a| &a.1).collect();
        let vs: Vec<f64> = train.iter().map(|a| a.velocity).collect();
        let line = |ys: Vec<f64>| {
            let (a, b, s) = fit(&vs, &ys);
            serde_json::json!([a, b, s])
        };
        // Modes: reference set from the loudest training hit; each mode's
        // parameters are fitted across hits by matching the nearest peak.
        let reference = train.iter().max_by(|a, b| a.velocity.total_cmp(&b.velocity)).unwrap();
        let modes: Vec<serde_json::Value> = reference
            .modes
            .iter()
            .map(|&(hz, ..)| {
                let matched: Vec<ModeSample> = train
                    .iter()
                    .filter_map(|a| {
                        let m = a.modes.iter().min_by(|x, y| (x.0 - hz).abs().total_cmp(&(y.0 - hz).abs()))?;
                        ((m.0 / hz - 1.).abs() < tolerance(hz)).then_some((a.velocity, m.0, m.1, m.2.ln(), m.3.ln(), m.4))
                    })
                    .collect();
                let mv: Vec<f64> = matched.iter().map(|m| m.0).collect();
                let col = |f: fn(&ModeSample) -> f64| {
                    let (a, b, s) = fit(&mv, &matched.iter().map(f).collect::<Vec<_>>());
                    serde_json::json!([a, b, s])
                };
                serde_json::json!({"hz": col(|m| m.1), "db": col(|m| m.2), "lnFast": col(|m| m.3), "lnSlow": col(|m| m.4), "slowDb": col(|m| m.5), "presence": matched.len() as f64 / train.len() as f64})
            })
            .collect();
        let noise: Vec<serde_json::Value> = (0..BANDS)
            .map(|b| {
                serde_json::json!({
                    "db": line(train.iter().map(|a| a.noise[b].0).collect()),
                    "lnFast": line(train.iter().map(|a| a.noise[b].1.ln()).collect()),
                    "lnSlow": line(train.iter().map(|a| a.noise[b].2.ln()).collect()),
                    "slowDb": line(train.iter().map(|a| a.noise[b].3).collect()),
                })
            })
            .collect();
        let room_t60: Vec<f64> = (0..train[0].room_t60.len())
            .map(|i| train.iter().map(|a| a.room_t60[i]).sum::<f64>() / train.len() as f64)
            .collect();
        let model = serde_json::json!({
            "hits": hits.len(),
            "heldOut": analyses.iter().filter(|a| a.0).count(),
            "peakDb": line(train.iter().map(|a| a.peak_db).collect()),
            "glide": line(train.iter().map(|a| a.glide.0).collect()),
            "glideTime": line(train.iter().map(|a| a.glide.1).collect()),
            "modes": modes,
            "noise": noise,
            "overhead": line(train.iter().map(|a| a.overhead_ratio).collect()),
            "room": line(train.iter().map(|a| a.room_ratio).collect()),
            "mixGain": line(train.iter().map(|a| a.mix_gain.ln()).collect()),
            "bloom": line(train.iter().map(|a| a.bloom).collect()),
            "roomDelay": train.iter().map(|a| a.room_delay).sum::<f64>() / train.len() as f64,
            "roomT60": room_t60,
            "heldOutVelocities": analyses.iter().filter(|a| a.0).map(|a| a.1.velocity).collect::<Vec<_>>(),
        });
        eprintln!(
            "{voice}/{articulation}: {} hits, {} modes, glide {:.3} @ {:.0} ms, peak {:.1}→{:.1} dB",
            hits.len(),
            modes.len(),
            model["glide"][0].as_f64().unwrap() + model["glide"][1].as_f64().unwrap(),
            1000. * model["glideTime"][0].as_f64().unwrap(),
            model["peakDb"][0].as_f64().unwrap(),
            model["peakDb"][0].as_f64().unwrap() + model["peakDb"][1].as_f64().unwrap(),
        );
        models.insert(format!("{voice}/{articulation}"), model);
    }
    let edges = band_edges();
    let out = serde_json::json!({"version": 1, "source": "DRSKit 2.1 (DrumGizmo.org), CC BY 4.0", "rate": RATE, "bandEdges": edges, "models": models});
    std::fs::write(&args[2], serde_json::to_string_pretty(&out).unwrap()).unwrap();
}
