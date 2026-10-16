//! Renders a phrase with each designed (generated) instrument and reports
//! clicks: samples whose jump is far above the local high-frequency level.
use soundroom_engine::designer::{Instrument, Note, Pattern};
use soundroom_engine::measured_instruments::InstrumentShape;
use std::sync::atomic::AtomicBool;
fn main() {
    let cancel = AtomicBool::new(false);
    // With a kits root argument, check the recorded instruments instead.
    let recorded = std::env::args().nth(1);
    if let Some(root) = &recorded {
        soundroom_engine::sampled_drums::set_kit_roots(vec![root.into()]);
    }
    let note = |pitch, start: f64, length: f64, velocity| Note { id: String::new(), pitch, velocity, start_beat: start, duration_beats: length, articulation: None };
    for instrument in Instrument::ALL.into_iter().filter(|i| i.recording().is_some()) {
        let base = if matches!(instrument, Instrument::PickedBass | Instrument::UprightBass) { 36 } else { 60 };
        let mut notes: Vec<Note> = [0, 4, 7, 12, 7, 4].iter().enumerate().map(|(i, d)| note(base + d, i as f64 * 0.5, 0.45, 0.5 + 0.07 * i as f64)).collect();
        notes.extend([0, 4, 7].iter().map(|d| note(base + d, 3.5, 3.5, 0.8)));
        let p = Pattern { notes, instrument, bpm: 96., beats: 8., instrument_shape: if recorded.is_some() { None } else { Some(InstrumentShape::default()) }, ..Pattern::default() };
        let x = p.render(48000., &cancel).unwrap().remove(0);
        let d: Vec<f32> = x.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
        let mut clicks = vec![];
        for i in 240..d.len().saturating_sub(240) {
            let local = d[i - 240..i + 240].iter().map(|v| v * v).sum::<f32>() / 480.;
            if d[i] > 0.01 && d[i] * d[i] > 40. * local.max(1e-12) {
                clicks.push(i);
            }
        }
        let tail = x.iter().rposition(|v| v.abs() > 1e-4).map_or(0, |i| i);
        println!("{:<15} clicks {:3} at {:?}  last audible {:.2}s of {:.2}s, final sample {:.4}", format!("{instrument:?}"), clicks.len(), clicks.iter().take(4).map(|i| format!("{:.3}s", *i as f64 / 48000.)).collect::<Vec<_>>(), tail as f64 / 48000., x.len() as f64 / 48000., x.last().copied().unwrap_or(0.));
    }
}
