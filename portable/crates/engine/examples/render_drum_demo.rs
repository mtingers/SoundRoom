//! Renders a demo groove with each recorded flavor and the modeled acoustic
//! kit: render_drum_demo <kits-root> <out-dir>
use soundroom_engine::drums::{Articulation, Kit, Pattern, Step, Voice};
use std::sync::atomic::AtomicBool;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    soundroom_engine::sampled_drums::set_kit_roots(vec![args[1].clone().into()]);
    let out = std::path::PathBuf::from(&args[2]);
    std::fs::create_dir_all(&out).unwrap();
    let step = |voice, index, velocity, articulation: Option<Articulation>| Step {
        articulation,
        voice,
        index,
        velocity,
        pan: 0.,
        probability: 1.,
    };
    let mut steps = vec![step(Voice::Crash, 0, 0.85, None)];
    for bar in 0..2 {
        let b = bar * 16;
        for i in 0..8 {
            steps.push(step(
                Voice::ClosedHat,
                b + i * 2,
                if i % 2 == 0 { 0.72 } else { 0.5 },
                None,
            ));
        }
        for k in [0, 7, 10] {
            steps.push(step(Voice::Kick, b + k, 0.9, None));
        }
        for s in [4, 12] {
            steps.push(step(Voice::Snare, b + s, 0.92, None));
        }
        for g in [6, 9, 15] {
            steps.push(step(Voice::Snare, b + g, 0.18, None));
        }
    }
    steps.retain(|s| !(s.voice == Voice::ClosedHat && s.index == 30));
    steps.push(step(Voice::OpenHat, 30, 0.7, None));
    steps.push(step(Voice::HighTom, 26, 0.8, None));
    steps.push(step(Voice::HighTom, 27, 0.75, None));
    steps.push(step(Voice::LowTom, 28, 0.85, None));
    steps.push(step(Voice::LowTom, 29, 0.8, None));
    let cancel = AtomicBool::new(false);
    for kit in [
        Kit::StudioNatural,
        Kit::StudioTight,
        Kit::StudioRoom,
        Kit::StudioVintage,
        Kit::StudioModeled,
        Kit::Acoustic,
    ] {
        let pattern = Pattern {
            kit,
            bpm: 96.,
            bars: 2,
            swing: 0.04,
            humanize: 0.004,
            velocity_humanize: Some(0.04),
            steps: steps.clone(),
            ..Pattern::default()
        };
        let started = std::time::Instant::now();
        let audio = pattern.render(2, true, &cancel).unwrap();
        let ms = started.elapsed().as_millis();
        let peak = audio.iter().flatten().fold(0f32, |m, v| m.max(v.abs()));
        let rms = (audio
            .iter()
            .flatten()
            .map(|v| (*v as f64).powi(2))
            .sum::<f64>()
            / (audio[0].len() * 2) as f64)
            .sqrt();
        let path = out.join(format!("{:?}.wav", kit));
        let _ = std::fs::remove_file(&path);
        let mut w =
            soundroom_engine::wave::WaveWriter::create(&path, audio[0].len() as u64).unwrap();
        for (l, r) in audio[0].chunks(4096).zip(audio[1].chunks(4096)) {
            w.write(l, r).unwrap();
        }
        w.finish().unwrap();
        println!(
            "{kit:?}: peak {:.1} dBFS, rms {:.1} dBFS, rendered in {ms} ms",
            20. * peak.log10(),
            20. * rms.log10()
        );
    }
}
