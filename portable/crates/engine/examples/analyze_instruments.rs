//! Measures recorded designer instruments note by note and writes the
//! parameters `measured_instruments.rs` synthesizes from.
//!
//! analyze_instruments <instruments dir> <out.json> [--holdout N]
//!
//! Per sampled note: fundamental and inharmonicity (stiff strings), then for
//! each harmonic its level, attack time and two-stage decay (or, for looped
//! sustained notes, its steady level and amplitude wobble), the vibrato of
//! the fundamental, and a 24-band noise residual (hammer, bow, breath) with
//! its own two-stage decay. Every Nth note (by pitch) is held out from the
//! model for validation.
use soundroom_engine::{spectral::spectrum, wave::WaveReader};
use std::f64::consts::PI;
use std::path::Path;

const RATE: f64 = 48_000.;
const HOP: usize = 240;
const PARTIALS: usize = 40;
const BANDS: usize = 24;

fn db(x: f64) -> f64 {
    20. * x.max(1e-12).log10()
}
fn fit(xs: &[f64], ys: &[f64]) -> (f64, f64) {
    let n = xs.len() as f64;
    if n < 2. {
        return (ys.first().copied().unwrap_or(0.), 0.);
    }
    let (mx, my) = (xs.iter().sum::<f64>() / n, ys.iter().sum::<f64>() / n);
    let sxx: f64 = xs.iter().map(|x| (x - mx).powi(2)).sum();
    let sxy: f64 = xs.iter().zip(ys).map(|(x, y)| (x - mx) * (y - my)).sum();
    let b = if sxx > 0. { sxy / sxx } else { 0. };
    (my - b * mx, b)
}
/// Complex demodulation at `hz` with a triangular low-pass; returns the
/// amplitude (linear) and instantaneous frequency (Hz) every HOP samples.
fn demodulate(x: &[f32], hz: f64, half: usize) -> (Vec<f64>, Vec<f64>) {
    let n = x.len();
    let (mut re, mut im) = (vec![0.; n + 1], vec![0.; n + 1]);
    let w = 2. * PI * hz / RATE;
    for i in 0..n {
        let (s, c) = (w * i as f64).sin_cos();
        re[i + 1] = re[i] + x[i] as f64 * c;
        im[i + 1] = im[i] - x[i] as f64 * s;
    }
    let boxed = |p: &[f64], i: usize| if i + 1 >= half { p[i + 1] - p[i + 1 - half] } else { p[i + 1] };
    let (b1r, b1i): (Vec<f64>, Vec<f64>) = (0..n).map(|i| (boxed(&re, i), boxed(&im, i))).unzip();
    let prefix = |v: &[f64]| { let mut p = vec![0.; v.len() + 1]; for i in 0..v.len() { p[i + 1] = p[i] + v[i]; } p };
    let (p2r, p2i) = (prefix(&b1r), prefix(&b1i));
    let norm = (half * half) as f64;
    let mut amp = vec![];
    let mut freq = vec![];
    let mut last = None;
    let mut i = half;
    while i < n {
        let (r, m) = (boxed(&p2r, i), boxed(&p2i, i));
        amp.push(2. * (r * r + m * m).sqrt() / norm);
        let phase = m.atan2(r);
        freq.push(match last {
            Some(prev) => {
                let mut d: f64 = phase - prev;
                while d > PI { d -= 2. * PI; }
                while d < -PI { d += 2. * PI; }
                hz + d / (2. * PI) * RATE / HOP as f64
            }
            None => hz,
        });
        last = Some(phase);
        i += HOP;
    }
    (amp, freq)
}
/// (level dB at the strike/peak, fast T60, slow T60, slow-stage dB re level)
fn two_stage(track: &[f64], floor: f64) -> (f64, f64, f64, f64, f64) {
    let dt = HOP as f64 / RATE;
    let top = track.iter().cloned().fold(f64::MIN, f64::max);
    let at = track.iter().position(|&v| v == top).unwrap_or(0);
    let times: Vec<f64> = (0..track.len()).map(|i| i as f64 * dt).collect();
    let fast_end = (at..track.len()).find(|&i| track[i] < top - 20.).unwrap_or(track.len() - 1).max(at + 2).min(track.len() - 1);
    let (_, fs) = fit(&times[at..=fast_end], &track[at..=fast_end]);
    let fast = if fs < -1e-3 { (-60. / fs).clamp(0.01, 20.) } else { 20. };
    let (slow, slow_db) = match (at..track.len()).find(|&i| track[i] < top - 25.) {
        Some(from) => {
            let idx: Vec<usize> = (from..track.len()).take_while(|&i| track[i] > floor + 6.).collect();
            if idx.len() >= 3 {
                let (a, b) = fit(&idx.iter().map(|&i| times[i]).collect::<Vec<_>>(), &idx.iter().map(|&i| track[i]).collect::<Vec<_>>());
                if b < -1e-3 { ((-60. / b).clamp(fast, 30.), (a - top).min(-6.)) } else { (fast, -120.) }
            } else { (fast, -120.) }
        }
        None => (fast, -120.),
    };
    (top, times[at], fast, slow, slow_db)
}
fn yin(x: &[f32], expected: f64) -> f64 {
    let start = (0.15 * RATE) as usize;
    let size = 4096;
    if x.len() < start + 2 * size {
        return expected;
    }
    let s = &x[start..start + 2 * size];
    let (lo, hi) = (((RATE / (expected * 1.12)) as usize).max(2), ((RATE / (expected / 1.12)) as usize).min(size - 2));
    if hi <= lo {
        return expected;
    }
    let d: Vec<f64> = (0..=hi + 1).map(|lag| (0..size).map(|i| ((s[i] - s[i + lag]) as f64).powi(2)).sum()).collect();
    let lag = (lo..=hi).min_by(|&a, &b| d[a].total_cmp(&d[b])).unwrap();
    let (a, b, c) = (d[lag - 1], d[lag], d[lag + 1]);
    let off = 0.5 * (a - c) / (a - 2. * b + c);
    RATE / (lag as f64 + if off.is_finite() { off.clamp(-1., 1.) } else { 0. })
}
fn band_edges() -> Vec<f64> {
    (0..=BANDS).map(|b| 30. * (18_000f64 / 30.).powf(b as f64 / BANDS as f64)).collect()
}

fn analyze(x: &[f32], centre: i32, looped: Option<(usize, usize)>, stiff: bool, modal: bool) -> serde_json::Value {
    let nominal = 440. * 2f64.powf((centre - 69) as f64 / 12.);
    let f0 = yin(x, nominal);
    let peak = x.iter().fold(0f32, |m, v| m.max(v.abs())) as f64;
    // Inharmonicity from long-window peaks: (f_k / k f0)^2 = 1 + B k^2.
    let mut b = 0.;
    if stiff {
        let spec = spectrum(x, (0.05 * RATE) as usize, 32768);
        let bin = RATE / 32768.;
        let mut ks = vec![];
        let mut ys = vec![];
        for k in 2..=PARTIALS.min((15_000. / f0) as usize) {
            let guess = k as f64 * f0 * (1. + b * (k * k) as f64).sqrt();
            let (lo, hi) = (((guess * 0.97) / bin) as usize, (((guess * 1.03) / bin) as usize).min(spec.len() - 2));
            if hi <= lo + 2 { continue; }
            let i = (lo..=hi).max_by(|&a, &c| spec[a].total_cmp(&spec[c])).unwrap();
            let (a, m, c) = (spec[i - 1].max(1e-12).ln(), spec[i].max(1e-12).ln(), spec[i + 1].max(1e-12).ln());
            let off = 0.5 * (a - c) / (a - 2. * m + c);
            let fk = (i as f64 + if off.is_finite() { off } else { 0. }) * bin;
            ks.push((k * k) as f64);
            ys.push((fk / (k as f64 * f0)).powi(2) - 1.);
            if ks.len() >= 3 { b = fit(&ks, &ys).1.clamp(0., 2e-3); }
        }
    }
    // Partial frequencies: harmonic (with stiffness) or, for struck bars and
    // tines, the measured resonance peaks.
    let mut ratios: Vec<f64> = (1..=PARTIALS).map(|k| k as f64 * (1. + b * (k * k) as f64).sqrt()).collect();
    if modal {
        let spec = spectrum(x, (0.02 * RATE) as usize, 32768);
        let bin = RATE / 32768.;
        let top = spec.iter().cloned().fold(0., f64::max);
        let mut peaks: Vec<(usize, f64)> = (3..spec.len() - 3)
            .filter(|&i| (i as f64 * bin) > f0 * 0.9 && (i as f64 * bin) < 16_000.)
            .filter(|&i| (1..=3).all(|d| spec[i] > spec[i - d] && spec[i] >= spec[i + d]) && spec[i] > top * 10f64.powf(-55. / 20.))
            .map(|i| (i, spec[i]))
            .collect();
        peaks.sort_by(|a, c| c.1.total_cmp(&a.1));
        let mut chosen: Vec<f64> = vec![];
        for (i, _) in peaks {
            let hz = i as f64 * bin;
            if chosen.iter().all(|c| (c / hz - 1.).abs() > 0.03) {
                chosen.push(hz);
            }
            if chosen.len() == 12 {
                break;
            }
        }
        if !chosen.iter().any(|c| (c / f0 - 1.).abs() < 0.03) {
            chosen.push(f0);
        }
        chosen.sort_by(f64::total_cmp);
        ratios = chosen.iter().map(|hz| hz / f0).collect();
    }
    let half = ((2.2 * RATE / f0) as usize).clamp(240, 4096);
    let seconds = if looped.is_some() { x.len() } else { x.len().min((8. * RATE) as usize) };
    let x = &x[..seconds];
    let mut partials = vec![];
    let mut vibrato = serde_json::json!(null);
    for (index, ratio) in ratios.iter().enumerate() {
        let k = index + 1;
        let fk = ratio * f0;
        if fk > 16_000. { break; }
        let (amp, freq) = demodulate(x, fk, half);
        let track: Vec<f64> = amp.iter().map(|a| db(*a)).collect();
        let floor = track[track.len().saturating_sub(20)..].iter().sum::<f64>() / 20f64.min(track.len() as f64).max(1.);
        let (top, attack, fast, slow, slow_db) = two_stage(&track, floor);
        // Level ~30 ms in, relative to the peak: bowed notes start well
        // below their peak and swell; struck notes start at it.
        let start = track.get(6).copied().unwrap_or(top) - top;
        let mut entry = serde_json::json!({"k": k, "ratio": ratio, "db": top - db(peak), "startDb": start.clamp(-60., 0.), "attack": attack, "fast": fast, "slow": slow, "slowDb": slow_db});
        if let Some((start, end)) = looped {
            // Sustained: steady level and wobble over the loop.
            let (a, z) = (start / HOP, (end / HOP).min(track.len()));
            if z > a + 10 {
                let seg = &track[a..z];
                let mean = seg.iter().sum::<f64>() / seg.len() as f64;
                let wobble = (seg.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / seg.len() as f64).sqrt();
                entry["sustainDb"] = serde_json::json!(mean - db(peak));
                entry["wobble"] = serde_json::json!(wobble.min(12.));
                // Sustained notes: start level and attack relative to the
                // steady level (within 3 dB), not to a stray later peak.
                let reach = track.iter().position(|v| *v >= mean - 3.).unwrap_or(0);
                entry["attack"] = serde_json::json!(reach as f64 * HOP as f64 / RATE);
                entry["startDb"] = serde_json::json!((track.get(6).copied().unwrap_or(mean) - mean).clamp(-60., 0.));
                if k == 1 {
                    // Vibrato of the fundamental: depth (cents) and rate.
                    // Smooth over 40 ms: the raw phase-difference track is noisy.
                    let raw: Vec<f64> = freq[a..z].to_vec();
                    let f: Vec<f64> = (0..raw.len()).map(|i| {
                        let (l, h) = (i.saturating_sub(4), (i + 4).min(raw.len() - 1));
                        raw[l..=h].iter().sum::<f64>() / (h - l + 1) as f64
                    }).collect();
                    let mf = f.iter().sum::<f64>() / f.len() as f64;
                    let cents: Vec<f64> = f.iter().map(|v| 1200. * (v / mf).log2()).collect();
                    // Remove slow drift (0.5 s moving average) before measuring.
                    let cents: Vec<f64> = (0..cents.len()).map(|i| {
                        let (l, h) = (i.saturating_sub(50), (i + 50).min(cents.len() - 1));
                        cents[i] - cents[l..=h].iter().sum::<f64>() / (h - l + 1) as f64
                    }).collect();
                    let depth = (cents.iter().map(|c| c * c).sum::<f64>() / cents.len() as f64).sqrt() * 2f64.sqrt();
                    let crossings = cents.windows(2).filter(|w| w[0] <= 0. && w[1] > 0.).count() as f64;
                    let rate = crossings / (cents.len() as f64 * HOP as f64 / RATE);
                    let onset = attack;
                    vibrato = serde_json::json!({"depth": depth.min(80.), "rate": rate.clamp(0., 9.), "onset": onset});
                }
            }
        }
        partials.push(entry);
    }
    // Noise residual: STFT band energy with harmonic bins removed.
    let edges = band_edges();
    let (n, hop) = (2048, 240);
    let frames = ((x.len().saturating_sub(n)) / hop).min(500);
    let w = RATE / n as f64;
    let mut bands = vec![vec![1e-24; frames]; BANDS];
    for f in 0..frames {
        let s = spectrum(x, f * hop, n);
        for (bi, band) in bands.iter_mut().enumerate() {
            let (lo, hi) = ((edges[bi] / w) as usize, ((edges[bi + 1] / w) as usize).max((edges[bi] / w) as usize + 1).min(s.len()));
            let keep: Vec<f64> = (lo..hi).filter(|&kb| {
                let hz = kb as f64 * w;
                // Hann main lobe is ±2 bins: notch wider so harmonic
                // energy is not counted as noise.
                ratios.iter().all(|r| (hz - r * f0).abs() > (2.5 * w).max(f0 * 0.05))
            }).map(|kb| s[kb] * s[kb]).collect();
            if !keep.is_empty() {
                band[f] = keep.iter().sum::<f64>() / keep.len() as f64 * (hi - lo) as f64 / 3.;
            }
        }
    }
    let noise: Vec<serde_json::Value> = bands.iter().map(|band| {
        let track: Vec<f64> = band.iter().map(|e| 10. * e.log10()).collect();
        let floor = track[track.len().saturating_sub(10)..].iter().sum::<f64>() / 10f64.min(track.len() as f64).max(1.);
        let (top, _, fast, slow, slow_db) = two_stage(&track, floor);
        let mut v = serde_json::json!({"db": top - db(peak), "fast": fast, "slow": slow, "slowDb": slow_db});
        if let Some((start, end)) = looped {
            let (a, z) = (start / hop, (end / hop).min(track.len()));
            if z > a + 4 {
                v["sustainDb"] = serde_json::json!(track[a..z].iter().sum::<f64>() / (z - a) as f64 - db(peak));
            }
        }
        v
    }).collect();
    serde_json::json!({"f0": f0, "b": b, "peakDb": db(peak), "partials": partials, "noise": noise, "vibrato": vibrato})
}

fn read_mono(path: &Path) -> Vec<f32> {
    let mut r = WaveReader::open(path).unwrap();
    let n = r.info.frames as usize;
    let mut planar = vec![vec![0f32; n]; r.info.channels as usize];
    let mut at = 0;
    while at < n {
        let k = 8192.min(n - at);
        let mut v: Vec<&mut [f32]> = planar.iter_mut().map(|c| &mut c[at..at + k]).collect();
        r.read_planar(at as u64, &mut v).unwrap();
        at += k;
    }
    let scale = 1. / planar.len() as f32;
    (0..n).map(|i| planar.iter().map(|c| c[i]).sum::<f32>() * scale).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = Path::new(&args[1]);
    let holdout: usize = args.iter().position(|a| a == "--holdout").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(4);
    let mut instruments = serde_json::Map::new();
    let mut ids: Vec<_> = std::fs::read_dir(root).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| !n.starts_with('.')).collect();
    ids.sort();
    for id in ids {
        let Ok(bytes) = std::fs::read(root.join(&id).join("manifest.json")) else { continue };
        let manifest: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let stiff = matches!(id.as_str(), "studioPiano" | "uprightPiano" | "concertHarp" | "electricGuitar" | "acousticGuitar" | "pickedBass" | "uprightBass");
        let modal = matches!(id.as_str(), "vibraphone" | "marimba" | "kalimba");
        let mut regions: Vec<&serde_json::Value> = manifest["regions"].as_array().unwrap().iter().filter(|r| r["robin"] == 0).collect();
        regions.sort_by_key(|r| (r["centre"].as_i64().unwrap(), r["lovel"].as_i64().unwrap()));
        let centres: Vec<i64> = { let mut c: Vec<i64> = regions.iter().map(|r| r["centre"].as_i64().unwrap()).collect(); c.dedup(); c };
        let started = std::time::Instant::now();
        let notes: Vec<serde_json::Value> = std::thread::scope(|scope| {
            let handles: Vec<_> = regions.iter().map(|r| {
                let path = root.join(&id).join(r["file"].as_str().unwrap());
                let centre = r["centre"].as_i64().unwrap() as i32;
                let looped = r["loopStart"].as_u64().zip(r["loopEnd"].as_u64()).map(|(a, b)| (a as usize, b as usize));
                let velocity = (r["lovel"].as_f64().unwrap() + r["hivel"].as_f64().unwrap()) / 2. / 127.;
                let held = centres.iter().position(|c| *c == centre as i64).unwrap() % holdout == holdout - 1;
                let gain = r["gain"].as_f64().unwrap();
                scope.spawn(move || {
                    let mut x = read_mono(&path);
                    for v in &mut x { *v *= gain as f32; }
                    let mut a = analyze(&x, centre, looped, stiff, modal);
                    a["pitch"] = serde_json::json!(centre);
                    a["velocity"] = serde_json::json!(velocity);
                    a["heldOut"] = serde_json::json!(held);
                    a["file"] = serde_json::json!(path.file_name().unwrap().to_string_lossy());
                    a["sustained"] = serde_json::json!(looped.is_some());
                    a
                })
            }).collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let release = manifest["release"].as_f64().unwrap_or(0.3);
        eprintln!("{id}: {} notes analyzed in {:.1}s", notes.len(), started.elapsed().as_secs_f64());
        instruments.insert(id, serde_json::json!({"release": release, "notes": notes}));
    }
    let out = serde_json::json!({"version": 1, "rate": RATE, "bandEdges": band_edges(), "instruments": instruments});
    std::fs::write(&args[2], serde_json::to_vec(&out).unwrap()).unwrap();
}
