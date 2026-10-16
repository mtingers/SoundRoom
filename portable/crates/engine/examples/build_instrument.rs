//! Converts instrument note samples for Soundroom's sampled instruments:
//! resample to 48 kHz (Kaiser-windowed sinc), keep up to two channels, trim
//! leading silence (keeping the attack) and the tail below -72 dB, find a
//! crossfaded sustain loop for bowed/blown notes, and store 16-bit audio
//! normalized to -1 dBFS with the gain that restores its level.
//!
//! build_instrument <jobs.json> <results.json>
//! jobs: [{"input": "...", "output": "...", "loop": bool}]
use soundroom_engine::wave::WaveReader;
use std::f64::consts::PI;
use std::path::Path;

const RATE: f64 = 48_000.;
const STORED_PEAK: f32 = 0.89;

fn resample(input: &[f32], from: f64) -> Vec<f32> {
    if (from - RATE).abs() < 0.5 {
        return input.to_vec();
    }
    let ratio = from / RATE;
    let cutoff = (RATE / from).min(1.) * 0.97;
    let taps = 48_i64;
    let beta = 9.;
    let bessel = |x: f64| {
        let (mut sum, mut term, mut k) = (1., 1., 1.);
        while term > 1e-12 * sum {
            term *= (x / (2. * k)).powi(2);
            sum += term;
            k += 1.;
        }
        sum
    };
    let norm = bessel(beta);
    const TABLE: usize = 8192;
    let table: Vec<f64> = (0..=TABLE).map(|k| { let w = k as f64 / TABLE as f64; bessel(beta * (1. - w * w).max(0.).sqrt()) / norm }).collect();
    let frames = (input.len() as f64 / ratio).ceil() as usize;
    (0..frames)
        .map(|i| {
            let position = i as f64 * ratio;
            let centre = position.floor() as i64;
            let mut acc = 0.;
            for j in centre - taps + 1..=centre + taps {
                if j < 0 || j as usize >= input.len() {
                    continue;
                }
                let x = position - j as f64;
                let w = (x / taps as f64).abs();
                if w >= 1. {
                    continue;
                }
                let sinc = if x.abs() < 1e-9 { 1. } else { (PI * cutoff * x).sin() / (PI * cutoff * x) };
                acc += input[j as usize] as f64 * cutoff * sinc * table[(w * TABLE as f64) as usize];
            }
            acc as f32
        })
        .collect()
}

/// Sustain loop: end at 85 % of the note, start where the 30 ms before it
/// best matches the 30 ms before the end (positive-going zero crossings),
/// then crossfade the end into the start so the jump is seamless.
fn find_loop(channels: &mut [Vec<f32>]) -> Option<(usize, usize)> {
    let mono: Vec<f32> = (0..channels[0].len()).map(|i| channels.iter().map(|c| c[i]).sum::<f32>()).collect();
    let n = mono.len();
    if n < (RATE * 1.2) as usize {
        return None;
    }
    let up = |i: usize| i > 0 && mono[i - 1] <= 0. && mono[i] > 0.;
    let window = (0.03 * RATE) as usize;
    let mut end = (n as f64 * 0.85) as usize;
    while end > window && !up(end) {
        end -= 1;
    }
    let lo = ((n as f64 * 0.3) as usize).max(window);
    // Prefer loops of about a second or more: short loops of a vibrato note
    // repeat audibly.
    let minimum = ((end.saturating_sub(lo)) / 2).clamp((0.35 * RATE) as usize, RATE as usize);
    let hi = end.saturating_sub(minimum);
    if hi <= lo {
        return None;
    }
    let target = &mono[end - window..end];
    let mut best = (f64::MIN, 0);
    for start in (lo..hi).filter(|&i| up(i)) {
        let cand = &mono[start - window..start];
        let (mut dot, mut a, mut b) = (0f64, 0f64, 0f64);
        for k in (0..window).step_by(2) {
            dot += (target[k] * cand[k]) as f64;
            a += (target[k] * target[k]) as f64;
            b += (cand[k] * cand[k]) as f64;
        }
        let score = dot / (a * b).sqrt().max(1e-12);
        if score > best.0 {
            best = (score, start);
        }
    }
    let start = best.1;
    if start == 0 {
        return None;
    }
    // Equal-power crossfade of the loop's last 60 ms toward the audio that
    // precedes the loop start.
    let fade = ((0.06 * RATE) as usize).min(start).min(end - start);
    for c in channels.iter_mut() {
        for k in 0..fade {
            let t = (k + 1) as f64 / fade as f64;
            let (g_out, g_in) = ((t * PI / 2.).cos() as f32, (t * PI / 2.).sin() as f32);
            let i = end - fade + k;
            c[i] = c[i] * g_out + c[start - fade + k] * g_in;
        }
    }
    Some((start, end))
}

/// Fundamental (Hz) of the steady part of a note, by the YIN difference
/// function (first dip below 0.15 between 25 Hz and 2.5 kHz).
fn pitch(mono: &[f32]) -> Option<f64> {
    let start = (0.12 * RATE) as usize;
    let size = 4096;
    if mono.len() < start + 2 * size {
        return None;
    }
    let x = &mono[start..start + 2 * size];
    let min_lag = (RATE / 2500.) as usize;
    let max_lag = ((RATE / 25.) as usize).min(size - 1);
    let mut d = vec![0f64; max_lag + 1];
    for (lag, slot) in d.iter_mut().enumerate().skip(1) {
        *slot = (0..size).map(|i| ((x[i] - x[i + lag]) as f64).powi(2)).sum();
    }
    let mut running = 0.;
    let mut cmnd = vec![1f64; max_lag + 1];
    for lag in 1..=max_lag {
        running += d[lag];
        cmnd[lag] = d[lag] * lag as f64 / running.max(1e-20);
    }
    let lag = (min_lag..max_lag).find(|&l| cmnd[l] < 0.15 && cmnd[l] <= cmnd[l + 1])?;
    // Parabolic refinement.
    let (a, b, c) = (cmnd[lag - 1], cmnd[lag], cmnd[lag + 1]);
    let offset = 0.5 * (a - c) / (a - 2. * b + c);
    Some(RATE / (lag as f64 + if offset.is_finite() { offset.clamp(-1., 1.) } else { 0. }))
}
fn write_wav16(path: &Path, channels: &[Vec<f32>], scale: f32) -> std::io::Result<()> {
    let frames = channels[0].len();
    let count = channels.len();
    let data = frames * count * 2;
    let mut out = Vec::with_capacity(44 + data);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(count as u16).to_le_bytes());
    out.extend_from_slice(&(RATE as u32).to_le_bytes());
    out.extend_from_slice(&((RATE as u32) * count as u32 * 2).to_le_bytes());
    out.extend_from_slice(&((count * 2) as u16).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data as u32).to_le_bytes());
    for i in 0..frames {
        for c in channels {
            out.extend_from_slice(&(((c[i] * scale).clamp(-1., 1.) * 32767.).round() as i16).to_le_bytes());
        }
    }
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(path, out)
}

fn convert(input: &Path, output: &Path, looped: bool, max_seconds: f64) -> Result<serde_json::Value, String> {
    let mut reader = WaveReader::open(input).map_err(|e| e.to_string())?;
    let info = reader.info.clone();
    let frames = info.frames as usize;
    let mut planar = vec![vec![0f32; frames]; info.channels as usize];
    let mut at = 0;
    while at < frames {
        let n = 8192.min(frames - at);
        let mut views: Vec<&mut [f32]> = planar.iter_mut().map(|c| &mut c[at..at + n]).collect();
        reader.read_planar(at as u64, &mut views).map_err(|e| e.to_string())?;
        at += n;
    }
    planar.truncate(2);
    let mut channels: Vec<Vec<f32>> = planar.iter().map(|c| resample(c, info.sample_rate as f64)).collect();
    let len = channels[0].len();
    let peak = channels.iter().flatten().fold(0f32, |m, v| m.max(v.abs()));
    if peak <= 0. {
        return Err("silent".into());
    }
    let onset = (0..len).find(|&i| channels.iter().any(|c| c[i].abs() >= peak * 0.01)).unwrap_or(0).saturating_sub(96);
    let floor = peak * 10f32.powf(-72. / 20.);
    let last = (0..len).rev().find(|&i| channels.iter().any(|c| c[i].abs() >= floor)).unwrap_or(len - 1);
    let last = last.min(onset + (max_seconds * RATE) as usize);
    for c in channels.iter_mut() {
        *c = c[onset..=last].to_vec();
    }
    let mono: Vec<f32> = (0..channels[0].len()).map(|i| channels.iter().map(|c| c[i]).sum::<f32>()).collect();
    let detected = pitch(&mono);
    let loop_points = if looped { find_loop(&mut channels) } else { None };
    if loop_points.is_none() {
        let n = channels[0].len();
        let fade = 960.min(n);
        for c in channels.iter_mut() {
            for k in 0..fade {
                c[n - fade + k] *= (0.5 + 0.5 * (PI * k as f64 / fade as f64).cos()) as f32;
            }
        }
    }
    write_wav16(output, &channels, STORED_PEAK / peak).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "output": output.to_string_lossy(), "frames": channels[0].len(), "channels": channels.len(),
        "gain": peak / STORED_PEAK, "loopStart": loop_points.map(|l| l.0), "loopEnd": loop_points.map(|l| l.1), "pitch": detected,
    }))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let jobs: Vec<serde_json::Value> = serde_json::from_slice(&std::fs::read(&args[1]).unwrap()).unwrap();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..std::thread::available_parallelism().map_or(4, |n| n.get()).min(8) {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(job) = jobs.get(i) else { break };
                let input = Path::new(job["input"].as_str().unwrap());
                let output = Path::new(job["output"].as_str().unwrap());
                let max_seconds = job["maxSeconds"].as_f64().unwrap_or(30.);
                let result = match convert(input, output, job["loop"].as_bool().unwrap_or(false), max_seconds) {
                    Ok(v) => v,
                    Err(e) => serde_json::json!({"output": output.to_string_lossy(), "error": e}),
                };
                results.lock().unwrap().push(result);
            });
        }
    });
    std::fs::write(&args[2], serde_json::to_vec_pretty(&results.into_inner().unwrap()).unwrap()).unwrap();
}
