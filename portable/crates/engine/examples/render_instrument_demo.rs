//! Renders a short phrase on every recorded designer instrument, recorded
//! and modeled: render_instrument_demo <kits-root> <out-dir>
use soundroom_engine::designer::{Instrument, Note, Pattern};
use soundroom_engine::sampled_instruments;
use std::sync::atomic::AtomicBool;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = std::path::PathBuf::from(&args[2]);
    std::fs::create_dir_all(&out).unwrap();
    let cancel = AtomicBool::new(false);
    let note = |pitch, start: f64, length: f64, velocity| Note {
        id: String::new(),
        pitch,
        velocity,
        start_beat: start,
        duration_beats: length,
        articulation: None,
    };
    for (pass, roots) in [
        ("modeled", vec![]),
        ("recorded", vec![args[1].clone().into()]),
    ] {
        if pass == "recorded" {
            soundroom_engine::sampled_drums::set_kit_roots(roots);
        }
        for instrument in Instrument::ALL
            .into_iter()
            .filter(|i| i.recording().is_some())
        {
            let low = matches!(instrument, Instrument::PickedBass | Instrument::UprightBass);
            let base = if low {
                36
            } else if instrument == Instrument::SoloCello {
                48
            } else {
                60
            };
            let mut notes: Vec<Note> = [0, 4, 7, 12, 7, 4]
                .iter()
                .enumerate()
                .map(|(i, d)| note(base + d, i as f64 * 0.5, 0.45, 0.45 + 0.08 * i as f64))
                .collect();
            notes.extend([0, 4, 7].iter().map(|d| note(base + d, 3.5, 3.5, 0.8)));
            let mut p = Pattern {
                notes,
                instrument,
                bpm: 96.,
                beats: 8.,
                ..Pattern::default()
            };
            p.instrument = instrument;
            let audio = p.render(48000., &cancel).unwrap();
            let left = &audio[0];
            let right = audio.get(1).unwrap_or(left);
            let peak = left.iter().chain(right).fold(0f32, |m, v| m.max(v.abs()));
            let rms =
                (left.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / left.len() as f64).sqrt();
            let path = out.join(format!("{:?}-{pass}.wav", instrument));
            let _ = std::fs::remove_file(&path);
            let mut w =
                soundroom_engine::wave::WaveWriter::create(&path, left.len() as u64).unwrap();
            for (l, r) in left.chunks(4096).zip(right.chunks(4096)) {
                w.write(l, r).unwrap();
            }
            w.finish().unwrap();
            println!(
                "{:<15} {pass:<9} installed {:<5} peak {:6.1} dBFS rms {:6.1} dBFS",
                format!("{instrument:?}"),
                sampled_instruments::available(instrument),
                20. * peak.log10(),
                20. * rms.log10()
            );
        }
    }
}
