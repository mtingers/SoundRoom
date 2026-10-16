//! Scores generated drum hits against held-out recorded hits:
//! mean absolute difference (dB) of 24-band × 10 ms energy maps, each
//! normalized to its own maximum, floored at -60 dB.
//! evaluate_drum_models <kit-dir with hits.jsonl>
use soundroom_engine::drums::{render_voice, Articulation, Kit, Voice};
use soundroom_engine::{measured_drums, spectral::spectrum, wave::WaveReader};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;
const RATE: f64 = 48_000.;
fn map(x: &[f32], seconds: f64) -> Vec<f64> {
    let edges: Vec<f64> = (0..=24).map(|b| 30. * (18_000f64 / 30.).powf(b as f64 / 24.)).collect();
    let frames = (seconds * 100.) as usize;
    let mut out = vec![];
    for f in 0..frames {
        let s = spectrum(x, f * 480, 2048);
        let w = RATE / 2048.;
        for b in 0..24 {
            let (lo, hi) = ((edges[b] / w) as usize, ((edges[b + 1] / w) as usize).max((edges[b] / w) as usize + 1));
            out.push(10. * (lo..hi.min(s.len())).map(|k| s[k] * s[k]).sum::<f64>().max(1e-20).log10());
        }
    }
    let top = out.iter().cloned().fold(f64::MIN, f64::max);
    out.iter().map(|v| (v - top).max(-60.)).collect()
}
fn distance(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f64>() / a.len() as f64
}
fn main() {
    let kit = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let mut groups: BTreeMap<(String, String), Vec<(String, f64)>> = BTreeMap::new();
    for line in std::fs::read_to_string(kit.join("hits.jsonl")).unwrap().lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        groups.entry((v["voice"].as_str().unwrap().into(), v["articulation"].as_str().unwrap().into())).or_default().push((v["file"].as_str().unwrap().into(), v["power"].as_f64().unwrap()));
    }
    let (mut all_m, mut all_a) = (vec![], vec![]);
    for ((voice_name, articulation), mut hits) in groups {
        hits.sort_by(|a, b| a.1.total_cmp(&b.1));
        let voice = Voice::ALL.into_iter().find(|v| v.name() == voice_name).unwrap();
        let cymbal = matches!(voice, Voice::Crash | Voice::Ride | Voice::OpenHat | Voice::ClosedHat);
        let seconds = if cymbal { 1.2 } else { 0.8 };
        let art = match articulation.as_str() { "sideStick" => Articulation::SideStick, "brush" => Articulation::Brush, "bell" => Articulation::Bell, _ => Articulation::Center };
        let (lo, hi) = (hits[0].1.ln(), hits[hits.len() - 1].1.ln());
        let (mut dm, mut da) = (vec![], vec![]);
        for (i, (file, power)) in hits.iter().enumerate() {
            if i % 4 != 3 { continue; }
            let v = (power.ln() - lo) / (hi - lo);
            let mut reader = WaveReader::open(&kit.join(file)).unwrap();
            let frames = reader.info.frames as usize;
            let mut stems = vec![vec![0f32; frames]; 6];
            let mut at = 0;
            while at < frames { let n = 8192.min(frames - at); let mut views: Vec<&mut [f32]> = stems.iter_mut().map(|s| &mut s[at..at + n]).collect(); reader.read_planar(at as u64, &mut views).unwrap(); at += n; }
            let (a, b) = if cymbal { (2, 3) } else { (0, 1) };
            let real: Vec<f32> = (0..frames).map(|k| stems[a][k] + stems[b][k]).collect();
            let real_map = map(&real, seconds);
            let modeled = measured_drums::direct_hit(voice, &articulation, v, 0).unwrap();
            let acoustic = render_voice(Kit::Acoustic, voice, v, 0, art, 0.5, &AtomicBool::new(false)).unwrap();
            let model_map = map(&modeled, seconds);
            if std::env::var("DEBUG_VOICE").ok().as_deref() == Some(voice_name.as_str()) && articulation == "center" {
                for f in [0usize, 1, 3, 6, 12, 25, 50, 79] {
                    let row = |m: &Vec<f64>| (0..24).step_by(2).map(|b| format!("{:4.0}", m[f * 24 + b])).collect::<Vec<_>>().join("");
                    eprintln!("v={v:.2} t={:3}ms real {}\n                modl {}", f * 10, row(&real_map), row(&model_map));
                }
            }
            dm.push(distance(&real_map, &model_map));
            da.push(distance(&real_map, &map(&acoustic, seconds)));
        }
        let mean = |x: &Vec<f64>| x.iter().sum::<f64>() / x.len().max(1) as f64;
        println!("{:<18} held-out {:>2}  modeled {:5.2} dB   current synth {:5.2} dB", format!("{voice_name}/{articulation}"), dm.len(), mean(&dm), mean(&da));
        all_m.extend(dm); all_a.extend(da);
    }
    let mean = |x: &Vec<f64>| x.iter().sum::<f64>() / x.len().max(1) as f64;
    println!("{:<18} held-out {:>2}  modeled {:5.2} dB   current synth {:5.2} dB", "ALL", all_m.len(), mean(&all_m), mean(&all_a));
    let _ = Path::new("");
}
