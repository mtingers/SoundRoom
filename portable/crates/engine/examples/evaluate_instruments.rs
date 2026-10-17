//! Scores generated instrument notes against held-out recordings:
//! evaluate_instruments <instruments dir> <measured-instruments.json>
//! Mean absolute difference (dB) of 24-band × 10 ms energy maps over 1.5 s,
//! each normalized to its own maximum, floored at -60 dB.
use soundroom_engine::designer::{Instrument, Pattern, Voice};
use soundroom_engine::measured_instruments::{InstrumentShape, ModelNote};
use soundroom_engine::{spectral::spectrum, wave::WaveReader};
const RATE: f64 = 48_000.;
fn map(x: &[f32]) -> Vec<f64> {
    let edges: Vec<f64> = (0..=24)
        .map(|b| 30. * (18_000f64 / 30.).powf(b as f64 / 24.))
        .collect();
    let mut out = vec![];
    for f in 0..150 {
        let s = spectrum(x, f * 480, 2048);
        let w = RATE / 2048.;
        for b in 0..24 {
            let (lo, hi) = (
                (edges[b] / w) as usize,
                ((edges[b + 1] / w) as usize).max((edges[b] / w) as usize + 1),
            );
            out.push(
                10. * (lo..hi.min(s.len()))
                    .map(|k| s[k] * s[k])
                    .sum::<f64>()
                    .max(1e-20)
                    .log10(),
            );
        }
    }
    let top = out.iter().cloned().fold(f64::MIN, f64::max);
    out.iter().map(|v| (v - top).max(-60.)).collect()
}
fn distance(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f64>() / a.len() as f64
}
fn read(path: &std::path::Path) -> Vec<f32> {
    let mut r = WaveReader::open(path).unwrap();
    let n = r.info.frames as usize;
    let mut p = vec![vec![0f32; n]; r.info.channels as usize];
    let mut at = 0;
    while at < n {
        let k = 8192.min(n - at);
        let mut v: Vec<&mut [f32]> = p.iter_mut().map(|c| &mut c[at..at + k]).collect();
        r.read_planar(at as u64, &mut v).unwrap();
        at += k;
    }
    (0..n)
        .map(|i| p.iter().map(|c| c[i]).sum::<f32>())
        .collect()
}
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = std::path::Path::new(&args[1]);
    let model: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&args[2]).unwrap()).unwrap();
    let (mut all_m, mut all_s) = (vec![], vec![]);
    for instrument in Instrument::ALL
        .into_iter()
        .filter(|i| i.recording().is_some())
    {
        let id = instrument.recording().unwrap();
        let notes = model["instruments"][id]["notes"].as_array().unwrap();
        let (mut dm, mut ds) = (vec![], vec![]);
        for n in notes.iter().filter(|n| n["heldOut"] == true) {
            let pitch = n["pitch"].as_i64().unwrap() as i32;
            let velocity = n["velocity"].as_f64().unwrap();
            let real = read(&root.join(id).join(n["file"].as_str().unwrap()));
            let frames = (1.6 * RATE) as usize;
            let mut generated = ModelNote::new(
                instrument,
                pitch,
                velocity,
                1,
                0.,
                RATE,
                InstrumentShape::default(),
                false,
            )
            .unwrap();
            let modeled: Vec<f32> = (0..frames).map(|_| generated.next(1.) as f32).collect();
            let pattern = Pattern {
                instrument,
                ..Pattern::default()
            };
            let mut voice = Voice::new(pitch, velocity, &pattern, RATE, 0.);
            let synth: Vec<f32> = (0..frames).map(|_| voice.render_next()).collect();
            let r = map(&real);
            let g = map(&modeled);
            if std::env::var("DEBUG_INSTRUMENT").ok().as_deref() == Some(id) && dm.is_empty() {
                for f in [0usize, 1, 3, 6, 12, 25, 50, 100, 149] {
                    let row = |m: &Vec<f64>| {
                        (0..24)
                            .step_by(2)
                            .map(|b| format!("{:4.0}", m[f * 24 + b]))
                            .collect::<String>()
                    };
                    eprintln!(
                        "p{pitch} v{velocity:.2} t={:4}ms real {}\n                     gen  {}",
                        f * 10,
                        row(&r),
                        row(&g)
                    );
                }
            }
            dm.push(distance(&r, &g));
            ds.push(distance(&r, &map(&synth)));
        }
        let mean = |v: &Vec<f64>| v.iter().sum::<f64>() / v.len().max(1) as f64;
        println!(
            "{:<15} held-out {:>2}  generated {:5.2} dB   current synth {:5.2} dB",
            id,
            dm.len(),
            mean(&dm),
            mean(&ds)
        );
        all_m.extend(dm);
        all_s.extend(ds);
    }
    let mean = |v: &Vec<f64>| v.iter().sum::<f64>() / v.len().max(1) as f64;
    println!(
        "{:<15} held-out {:>2}  generated {:5.2} dB   current synth {:5.2} dB",
        "ALL",
        all_m.len(),
        mean(&all_m),
        mean(&all_s)
    );
}
