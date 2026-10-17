//! Per-voice loudness of the recorded natural flavor vs the measured model.
use soundroom_engine::{
    drums::{Articulation, Kit, Voice},
    measured_drums, sampled_drums,
};
use std::sync::atomic::AtomicBool;
fn main() {
    sampled_drums::set_kit_roots(vec![std::env::args().nth(1).unwrap().into()]);
    let c = AtomicBool::new(false);
    let rms = |x: &[Vec<f32>; 2]| {
        let n = x[0].len().min(48000);
        10. * ((0..n)
            .map(|i| (x[0][i] as f64).powi(2) + (x[1][i] as f64).powi(2))
            .sum::<f64>()
            / n as f64)
            .log10()
    };
    for voice in Voice::ALL {
        if voice == Voice::Clap {
            continue;
        }
        for v in [0.3, 0.8] {
            let a = sampled_drums::render(
                Kit::StudioNatural,
                voice,
                v,
                0,
                Articulation::Center,
                0.5,
                &c,
            )
            .unwrap()
            .unwrap();
            let b = measured_drums::render(
                Kit::StudioModeled,
                voice,
                v,
                0,
                Articulation::Center,
                0.5,
                None,
                &c,
            )
            .unwrap()
            .unwrap();
            println!(
                "{:<10} v={v}: recorded {:6.1} dB  modeled {:6.1} dB  (first second RMS)",
                voice.name(),
                rms(&a),
                rms(&b)
            );
        }
    }
}
