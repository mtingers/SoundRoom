//! Converts one DrumGizmo instrument (multi-mic, any rate) into Soundroom's
//! sampled-kit format: per hit, a 6-channel 24-bit 48 kHz WAV holding stereo
//! close-mic, overhead and room stems, plus a JSON line describing the hits.
//!
//! cargo run --release -p soundroom-engine --example build_drum_kit -- \
//!   <instrument.xml> <out-dir> <voice> <articulation> <max-seconds> [--invert-snare-bottom] [--invert-kick-front]
use soundroom_engine::wave::WaveReader;
use std::collections::BTreeMap;
use std::f64::consts::PI;
use std::io::Write;
use std::path::{Path, PathBuf};

const OUT_RATE: f64 = 48_000.;
/// Stored samples are scaled by this; the manifest's `gain` undoes it.
const HEADROOM: f32 = 0.5;

/// Close mic placement (drummer's perspective) and base level.
fn close_mic(name: &str) -> Option<(f64, f64)> {
    Some(match name {
        "Kdrum_back" => (0., 1.),
        "Kdrum_front" => (0., 0.6),
        "Snare_top" => (-0.05, 1.),
        "Snare_bottom" => (-0.05, 0.35),
        "Hihat" => (-0.55, 0.5),
        "Ride" => (0.55, 0.45),
        "Tom1" => (-0.2, 0.85),
        "Tom2" => (0.3, 0.85),
        "Tom3" => (0.5, 0.85),
        _ => return None,
    })
}
/// Close mics belonging to the struck instrument; other close mics are bleed.
fn own_mics(instrument: &str) -> &'static [&'static str] {
    let name = instrument.to_ascii_lowercase();
    if name.starts_with("kdrum") {
        &["Kdrum_back", "Kdrum_front"]
    } else if name.starts_with("snare") {
        &["Snare_top", "Snare_bottom"]
    } else if name.starts_with("hihat") {
        &["Hihat"]
    } else if name.starts_with("ride") {
        &["Ride"]
    } else if name.starts_with("tom1") {
        &["Tom1"]
    } else if name.starts_with("tom2") {
        &["Tom2"]
    } else if name.starts_with("tom3") {
        &["Tom3"]
    } else {
        &[]
    }
}
const BLEED: f64 = 0.5;

fn attr(tag: &str, key: &str) -> Option<String> {
    let start = tag.find(&format!("{key}=\""))? + key.len() + 2;
    let end = tag[start..].find('"')? + start;
    Some(tag[start..end].to_string())
}
struct Sample {
    power: f64,
    file: String,
    channels: Vec<(String, usize)>,
}
fn parse(xml: &str) -> Vec<Sample> {
    let mut samples = vec![];
    for block in xml.split("<sample ").skip(1) {
        let head = &block[..block.find('>').unwrap_or(block.len())];
        let power = attr(head, "power").and_then(|p| p.parse().ok()).unwrap_or(0.);
        let mut file = String::new();
        let mut channels = vec![];
        for tag in block.split("<audiofile ").skip(1) {
            let tag = &tag[..tag.find('>').unwrap_or(tag.len())];
            if let (Some(c), Some(f), Some(n)) =
                (attr(tag, "channel"), attr(tag, "file"), attr(tag, "filechannel"))
            {
                file = f;
                channels.push((c, n.parse::<usize>().unwrap_or(1).saturating_sub(1)));
            }
        }
        if !file.is_empty() {
            samples.push(Sample { power, file, channels });
        }
    }
    samples
}
/// Windowed-sinc (Kaiser) resampler, 64 taps per side.
fn resample(input: &[f32], from: f64) -> Vec<f32> {
    if (from - OUT_RATE).abs() < 0.5 {
        return input.to_vec();
    }
    let ratio = from / OUT_RATE;
    let cutoff = (OUT_RATE / from).min(1.) * 0.97;
    let taps = 64_i64;
    let beta = 9.;
    fn bessel(x: f64) -> f64 {
        let (mut sum, mut term, mut k) = (1., 1., 1.);
        while term > 1e-12 * sum {
            term *= (x / (2. * k)).powi(2);
            sum += term;
            k += 1.;
        }
        sum
    }
    let norm = bessel(beta);
    const TABLE: usize = 16384;
    let table: Vec<f64> = (0..=TABLE)
        .map(|k| {
            let w = k as f64 / TABLE as f64;
            bessel(beta * (1. - w * w).max(0.).sqrt()) / norm
        })
        .collect();
    let frames = (input.len() as f64 / ratio).ceil() as usize;
    (0..frames)
        .map(|i| {
            let position = i as f64 * ratio;
            let center = position.floor() as i64;
            let mut acc = 0.;
            for j in center - taps + 1..=center + taps {
                if j < 0 || j as usize >= input.len() {
                    continue;
                }
                let x = position - j as f64;
                let w = (x / taps as f64).abs();
                if w >= 1. {
                    continue;
                }
                let window = table[(w * TABLE as f64) as usize];
                let sinc = if x.abs() < 1e-9 {
                    1.
                } else {
                    (PI * cutoff * x).sin() / (PI * cutoff * x)
                };
                acc += input[j as usize] as f64 * cutoff * sinc * window;
            }
            acc as f32
        })
        .collect()
}
fn write_wav24(path: &Path, stems: &[Vec<f32>]) -> std::io::Result<f32> {
    let frames = stems[0].len();
    let channels = stems.len();
    let data = frames * channels * 3;
    let mut out = Vec::with_capacity(44 + data);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(channels as u16).to_le_bytes());
    out.extend_from_slice(&(OUT_RATE as u32).to_le_bytes());
    out.extend_from_slice(&((OUT_RATE as u32) * channels as u32 * 3).to_le_bytes());
    out.extend_from_slice(&((channels * 3) as u16).to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data as u32).to_le_bytes());
    let mut peak = 0f32;
    for i in 0..frames {
        for stem in stems {
            let v = stem[i] * HEADROOM;
            peak = peak.max(v.abs());
            let q = (v.clamp(-1., 1.) * 8_388_607.).round() as i32;
            out.extend_from_slice(&q.to_le_bytes()[..3]);
        }
    }
    std::fs::write(path, out)?;
    Ok(peak)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 6 {
        eprintln!("usage: build_drum_kit <instrument.xml> <out-dir> <voice> <articulation> <max-seconds> [flags]");
        std::process::exit(2);
    }
    let xml_path = PathBuf::from(&args[1]);
    let out_dir = PathBuf::from(&args[2]);
    let (voice, articulation) = (&args[3], &args[4]);
    let max_seconds: f64 = args[5].parse().expect("max seconds");
    let invert_snare_bottom = args.iter().any(|a| a == "--invert-snare-bottom");
    let invert_kick_front = args.iter().any(|a| a == "--invert-kick-front");
    let instrument = xml_path.file_stem().unwrap().to_string_lossy().to_string();
    let base = xml_path.parent().unwrap();
    let xml = std::fs::read_to_string(&xml_path).expect("instrument xml");
    let samples = parse(&xml);
    let own = own_mics(&instrument);
    let target = out_dir.join(voice).join(articulation);
    std::fs::create_dir_all(&target).unwrap();
    let mut lines = vec![];
    let mut correlations: BTreeMap<&str, f64> = BTreeMap::new();
    for (index, sample) in samples.iter().enumerate() {
        let mut reader = WaveReader::open(&base.join(&sample.file)).expect("sample wav");
        let info = reader.info.clone();
        let frames = info.frames as usize;
        let mut planar = vec![vec![0f32; frames]; info.channels as usize];
        let mut at = 0;
        while at < frames {
            let n = 8192.min(frames - at);
            let mut views: Vec<&mut [f32]> = planar.iter_mut().map(|c| &mut c[at..at + n]).collect();
            reader.read_planar(at as u64, &mut views).expect("read");
            at += n;
        }
        let channel = |name: &str| {
            sample
                .channels
                .iter()
                .find(|(c, _)| c == name)
                .map(|(_, n)| planar[*n].clone())
                .unwrap_or_else(|| vec![0.; frames])
        };
        // Polarity diagnostics on the first 30 ms of the loudest hit.
        if index + 1 == samples.len() {
            for (a, b) in [("Snare_top", "Snare_bottom"), ("Kdrum_back", "Kdrum_front")] {
                let (x, y) = (channel(a), channel(b));
                let n = frames.min((info.sample_rate as f64 * 0.03) as usize);
                let dot: f64 = (0..n).map(|i| x[i] as f64 * y[i] as f64).sum();
                let norm = ((0..n).map(|i| (x[i] as f64).powi(2)).sum::<f64>()
                    * (0..n).map(|i| (y[i] as f64).powi(2)).sum::<f64>())
                .sqrt();
                correlations.insert(if a == "Snare_top" { "snare" } else { "kick" }, if norm > 0. { dot / norm } else { 0. });
            }
        }
        let mut close = [vec![0f32; frames], vec![0f32; frames]];
        for (name, _) in &sample.channels {
            let Some((pan, level)) = close_mic(name) else { continue };
            let mut gain = level * if own.contains(&name.as_str()) { 1. } else { BLEED };
            if (name == "Snare_bottom" && invert_snare_bottom) || (name == "Kdrum_front" && invert_kick_front) {
                gain = -gain;
            }
            let angle = (pan + 1.) * PI / 4.;
            let (l, r) = ((angle.cos() * std::f64::consts::SQRT_2 * gain) as f32, (angle.sin() * std::f64::consts::SQRT_2 * gain) as f32);
            let source = channel(name);
            for i in 0..frames {
                close[0][i] += source[i] * l;
                close[1][i] += source[i] * r;
            }
        }
        let stems_in = [
            close[0].clone(),
            close[1].clone(),
            channel("OHL"),
            channel("OHR"),
            channel("AmbL"),
            channel("AmbR"),
        ];
        let rate = info.sample_rate as f64;
        let mut stems: Vec<Vec<f32>> = stems_in.iter().map(|s| resample(s, rate)).collect();
        let len = stems[0].len();
        let peak = stems.iter().flat_map(|s| s.iter()).fold(0f32, |m, v| m.max(v.abs()));
        if peak <= 0. {
            continue;
        }
        // Onset: first frame any stem reaches 3% of the hit peak, minus 1 ms.
        let onset = (0..len)
            .find(|&i| stems.iter().any(|s| s[i].abs() >= peak * 0.03))
            .unwrap_or(0)
            .saturating_sub(48);
        // Tail: last frame above -72 dB of the peak, capped per voice.
        let floor = peak * 10f32.powf(-72. / 20.);
        let last = (0..len).rev().find(|&i| stems.iter().any(|s| s[i].abs() >= floor)).unwrap_or(len - 1);
        let end = (last + 1).min(onset + (max_seconds * OUT_RATE) as usize).min(len);
        let fade = 480.min(end - onset);
        for s in &mut stems {
            *s = s[onset..end].to_vec();
            let n = s.len();
            for k in 0..fade {
                s[n - fade + k] *= 0.5 + 0.5 * ((k as f64 / fade as f64) * PI).cos() as f32;
            }
        }
        let file = format!("{:02}.wav", index + 1);
        let stored = write_wav24(&target.join(&file), &stems).unwrap();
        lines.push(serde_json::json!({
            "voice": voice, "articulation": articulation, "file": format!("{voice}/{articulation}/{file}"),
            "power": sample.power, "frames": stems[0].len(), "storedPeak": stored,
        }));
    }
    let mut out = std::fs::OpenOptions::new().create(true).append(true).open(out_dir.join("hits.jsonl")).unwrap();
    for line in &lines {
        writeln!(out, "{line}").unwrap();
    }
    eprintln!("{instrument}: {} hits → {voice}/{articulation}; polarity correlation {:?}", lines.len(), correlations);
}
