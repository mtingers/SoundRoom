use serde::Deserialize;
use soundroom_engine::{
    drums::{self, Articulation, Kit, Pattern, Voice},
    wave::WaveReader,
};
use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/instruments")
        .join(name)
}
#[derive(Deserialize)]
struct VoiceFixture {
    kit: Kit,
    voice: Voice,
    velocity: f64,
    robin: i32,
    articulation: Articulation,
    damping: f64,
}
fn name<T: serde::Serialize>(v: T) -> String {
    serde_json::to_value(v)
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned()
}
fn compare(actual: &[Vec<f32>], file: &str) {
    let mut reader = WaveReader::open(&fixture(file)).unwrap();
    assert_eq!(reader.info.frames as usize, actual[0].len());
    let mut maximum = 0_f32;
    for start in (0..actual[0].len()).step_by(8192) {
        let count = (actual[0].len() - start).min(8192);
        let mut left = vec![0.; count];
        let mut right = left.clone();
        reader
            .read_stereo(start as u64, &mut left, &mut right)
            .unwrap();
        for (i, (&l, &r)) in left.iter().zip(&right).enumerate() {
            maximum = maximum.max((actual[0][start + i] - l).abs());
            if actual.len() > 1 {
                maximum = maximum.max((actual[1][start + i] - r).abs());
            }
        }
    }
    println!("Swift synthesis parity {file}: maximum absolute error {maximum}");
    assert!(maximum < 0.000005, "{file}: {maximum}");
}
#[test]
fn every_drum_kit_voice_matches_swift_with_robin_velocity_articulation_and_damping() {
    let cases: Vec<VoiceFixture> =
        serde_json::from_slice(&std::fs::read(fixture("drum-voices.json")).unwrap()).unwrap();
    // Recorded kits have no Swift synthesis counterpart.
    let modeled = drums::Kit::ALL
        .iter()
        .filter(|k| k.stem_mix().is_none() && **k != drums::Kit::StudioModeled)
        .count();
    assert_eq!(cases.len(), modeled * drums::Voice::ALL.len());
    for f in cases {
        let audio = drums::render_voice(
            f.kit,
            f.voice,
            f.velocity,
            f.robin,
            f.articulation,
            f.damping,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(audio.iter().all(|v| v.is_finite()));
        assert_eq!(audio[0], 0.);
        assert_eq!(*audio.last().unwrap(), 0.);
        compare(
            &[audio],
            &format!("drum-{}-{}-swift.wav", name(f.kit), name(f.voice)),
        );
    }
}
#[test]
fn cyclic_drum_patterns_match_swift_variation_warmup_chokes_tuning_and_tails() {
    for name in ["legacy", "electronic", "extended"] {
        let pattern: Pattern = serde_json::from_slice(
            &std::fs::read(fixture(&format!("drum-pattern-{name}.json"))).unwrap(),
        )
        .unwrap();
        let audio = pattern
            .render(2, name == "extended", &AtomicBool::new(false))
            .unwrap();
        compare(&audio, &format!("drum-pattern-{name}-swift.wav"));
        let repeated = pattern
            .render(2, name == "extended", &AtomicBool::new(false))
            .unwrap();
        assert_eq!(audio, repeated);
    }
}
#[test]
fn drum_silence_invalid_articulation_and_worker_limits() {
    let cancelled = AtomicBool::new(true);
    assert!(drums::render_voice(
        Kit::Acoustic,
        Voice::Kick,
        0.8,
        0,
        Articulation::Center,
        0.5,
        &cancelled
    )
    .is_err());
    let silence = drums::render_voice(
        Kit::Electronic,
        Voice::Crash,
        0.,
        3,
        Articulation::Center,
        0.5,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(silence.iter().all(|v| *v == 0.));
    let center = drums::render_voice(
        Kit::Acoustic,
        Voice::Kick,
        0.8,
        0,
        Articulation::Center,
        0.5,
        &AtomicBool::new(false),
    )
    .unwrap();
    let invalid = drums::render_voice(
        Kit::Acoustic,
        Voice::Kick,
        0.8,
        0,
        Articulation::Bell,
        0.5,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(center, invalid);
    let huge = Pattern {
        bars: 8,
        bpm: 30.,
        ..Pattern::default()
    };
    assert!(huge.render(16, true, &AtomicBool::new(false)).is_err());
    assert!(Pattern::default().render(1, false, &cancelled).is_err());
}
