use soundroom_engine::{
    model::{Effect, Extra},
    pitch::{self, Controls, Point, Region, Regions},
    project::Project,
    render::Renderer,
    wave::WaveReader,
};
use std::sync::atomic::AtomicBool;
fn point(frame: u64, note: f32, curve: &str) -> Point {
    Point {
        id: String::new(),
        frame,
        note,
        curve: curve.into(),
        curve_start: 0.,
        curve_end: 1.,
    }
}
#[test]
fn manual_regions_replace_overlaps_and_ramp_only_discontinuous_boundaries() {
    let regions = Regions::new(&[
        Region {
            id: String::new(),
            start_frame: 0,
            end_frame: 1000,
            points: vec![point(0, 60., "linear"), point(1000, 61., "linear")],
        },
        Region {
            id: String::new(),
            start_frame: 1000,
            end_frame: 2000,
            points: vec![point(1000, 61., "linear"), point(2000, 62., "linear")],
        },
        Region {
            id: String::new(),
            start_frame: 400,
            end_frame: 600,
            points: vec![point(400, 55., "hold"), point(600, 56., "linear")],
        },
    ])
    .unwrap();
    assert!(regions.sample(-1).is_none());
    assert_eq!(regions.sample(400).unwrap(), (55., 0.));
    assert_eq!(regions.sample(599).unwrap().0, 55.);
    assert_eq!(regions.sample(600).unwrap().1, 0.);
    assert_eq!(regions.sample(1000).unwrap(), (61., 1.));
    assert!(regions.sample(2000).is_none());
    assert_eq!(regions.sample(1500).unwrap().0, 61.5);
}
#[test]
fn pitch_control_scale_masks_and_manual_limits() {
    assert_eq!(pitch::nearest_note(60.5, 0, 1, 4095), 60); // equidistant scale tie goes downward
    assert_eq!(pitch::nearest_note(61.2, 0, 1, 4095), 62);
    assert_eq!(pitch::nearest_note(61.2, 7, 3, 1), 60); // custom classes absolute
    assert_eq!(pitch::nearest_note(61.2, 7, 3, 0), 61);
    let mut p = pitch::Pitch::default();
    let controls = Controls {
        manual_target: f32::NAN,
        mix: f32::NAN,
        ..Controls::default()
    };
    for _ in 0..5000 {
        assert_eq!(p.sample([f32::INFINITY, f32::NAN], controls), [0., 0.]);
    }
}
#[test]
fn pitch_analysis_bounds_and_cancellation() {
    assert!(pitch::analyze(&[], &[], 0, &AtomicBool::new(false))
        .unwrap()
        .is_empty());
    assert!(pitch::analyze(&[0.], &[], 20, &AtomicBool::new(false)).is_err());
    assert!(pitch::analyze(&[0.; 100], &[0.; 100], 20, &AtomicBool::new(true)).is_err());
    let result = pitch::analyze(&[0.; 4096], &[0.; 4096], 1, &AtomicBool::new(false)).unwrap();
    assert!(result.len() <= 2);
    assert!(result.iter().all(|p| !p.is_voiced()));
}
#[test]
fn pitch_regions_validate_before_mutating_or_rendering() {
    let effect = Effect {
        id: "11111111-1111-4111-8111-111111111111".into(),
        kind: "pitchCorrection".into(),
        amount: 0.5,
        enabled: true,
        automation: vec![],
        parameters: Default::default(),
        parameter_automation: Default::default(),
        extra: Extra::from([(
            "pitchEdits".into(),
            serde_json::json!([{"startFrame":0,"endFrame":100,"points":[{"frame":20,"note":64,"curve":"unknown"}]}]),
        )]),
    };
    assert!(Regions::from_effect(&effect).is_err());
}
fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}
/// Matches the Swift detector wherever it found a pitch. Where it gave up, the
/// octave-stable tracker may only continue the pitch it was already following.
fn compare_contour(left: &[f32], right: &[f32], name: &str) {
    let actual = pitch::analyze(left, right, 96, &AtomicBool::new(false)).unwrap();
    let expected: Vec<pitch::ContourPoint> =
        serde_json::from_slice(&std::fs::read(fixture(name)).unwrap()).unwrap();
    assert_eq!(actual.len(), expected.len());
    let mut previous: Option<f32> = None;
    for (a, b) in actual.iter().zip(expected) {
        assert_eq!(a.frame, b.frame);
        if b.is_voiced() || !a.is_voiced() {
            assert!(
                (a.frequency - b.frequency).abs() < 0.001,
                "{name} at {}: {} != {}",
                a.frame,
                a.frequency,
                b.frequency
            );
            assert!((a.confidence - b.confidence).abs() < 0.000005);
        } else {
            let tracked = previous.expect("a continuation follows a voiced estimate");
            assert!(
                (a.frequency / tracked - 1.).abs() < 0.12,
                "{name} at {}: {} does not continue {tracked}",
                a.frame,
                a.frequency
            );
        }
        previous = a.is_voiced().then_some(a.frequency).or(previous);
    }
}
#[test]
fn swift_detector_contours_measure_input_and_corrected_output() {
    let path = fixture("pitch-automatic.soundroom/Audio/11111111-1111-4111-8111-111111111111.wav");
    let mut input = WaveReader::open(&path).unwrap();
    let mut left = vec![0.; input.info.frames as usize];
    let mut right = left.clone();
    for (index, (l, r)) in left
        .chunks_mut(8192)
        .zip(right.chunks_mut(8192))
        .enumerate()
    {
        input.read_stereo((index * 8192) as u64, l, r).unwrap();
    }
    compare_contour(&left, &right, "pitch-input-contour-swift.json");
    assert_eq!(
        pitch::analyze_wave(&mut input, 96, &AtomicBool::new(false)).unwrap(),
        pitch::analyze(&left, &right, 96, &AtomicBool::new(false)).unwrap()
    );
    let input_points = pitch::analyze(&left, &right, 96, &AtomicBool::new(false)).unwrap();
    assert!(input_points
        .iter()
        .any(|p| p.frame > 2000 && p.frame < 7000 && (p.frequency - 224.75).abs() < 1.));
    for (name, desired) in [("pitch-up", 300_f32), ("pitch-down", 110_f32)] {
        let project = Project::open(&fixture(&format!("{name}.soundroom"))).unwrap();
        let mut renderer = Renderer::new(&project).unwrap();
        for (l, r) in left.chunks_mut(8192).zip(right.chunks_mut(8192)) {
            renderer.render(l, r).unwrap();
        }
        // Correction itself changed (exactly followed curves, octave-stable
        // tracking), so the output is held to its target rather than to Swift.
        let points = pitch::analyze(&left, &right, 96, &AtomicBool::new(false)).unwrap();
        let voiced: Vec<_> = points
            .iter()
            .filter(|p| p.frame > 3500 && p.frame < 7000 && p.is_voiced())
            .collect();
        assert!(!voiced.is_empty());
        assert!(
            voiced.iter().all(|p| (p.frequency - desired).abs() < 20.),
            "{name}: {voiced:?}"
        );
    }
}
#[test]
fn pitch_state_clone_is_independent_and_sample_identical() {
    let controls = Controls {
        manual_target: 60.,
        retune: 0.,
        humanize: 0.,
        ..Controls::default()
    };
    let mut p = pitch::Pitch::default();
    for i in 0..10000 {
        let x = (i as f32 * 0.031).sin() * 0.2;
        p.sample([x, -x], controls);
    }
    let mut copy = p.clone();
    for i in 10000..15000 {
        let x = (i as f32 * 0.031).sin() * 0.2;
        assert_eq!(p.sample([x, -x], controls), copy.sample([x, -x], controls));
    }
    let mut diverged = false;
    for _ in 0..6000 {
        diverged |= p.sample([0.2, 0.1], controls) != copy.sample([0., 0.], controls);
    }
    assert!(diverged);
}
