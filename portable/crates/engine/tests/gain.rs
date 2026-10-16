use soundroom_engine::{
    automation::Lane,
    model::{AutomationPoint, Session, Track},
    project::Project,
    render::Renderer,
    wave::WaveReader,
};
use std::path::PathBuf;

#[test]
fn legacy_scalar_gain_bounds_and_normalized_automation_are_distinct() {
    for version in 1..=11 {
        for gain in [0., 1.5, 2., 4.] {
            let session = Session {
                version,
                tracks: vec![Track {
                    id: "22222222-2222-4222-8222-222222222222".into(),
                    gain,
                    ..Track::default()
                }],
                ..Session::default()
            };
            session.validate().unwrap();
            let restored: Session =
                serde_json::from_slice(&serde_json::to_vec(&session).unwrap()).unwrap();
            assert_eq!(restored.tracks[0].gain, gain);
        }
    }
    for gain in [-0.001, 4.0001, f32::NAN, f32::INFINITY] {
        let mut session = Session::default();
        session.master_track.gain = gain;
        assert!(session.validate().is_err());
    }
    let point: AutomationPoint =
        serde_json::from_value(serde_json::json!({"frame":100,"value":0.3})).unwrap();
    let lane = Lane::new(&[point], 4. / 1.5, false);
    assert!(lane.has_points());
    assert_eq!(lane.value_signed(-1), 1.);
    assert_eq!(lane.value(99), 1.);
    assert_eq!(lane.value(100), 0.3);
}

#[test]
fn scalar_and_delayed_automation_match_actual_swift_renders() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/gain");
    for name in [
        "scalar-track",
        "scalar-master",
        "scalar-both",
        "delayed-track-linear",
        "delayed-track-decibels",
        "delayed-master-linear",
        "delayed-master-decibels",
        "delayed-both",
    ] {
        let project = Project::open(&root.join(format!("{name}.soundroom"))).unwrap();
        for block in [1, 128, 257, 8192] {
            let mut renderer = Renderer::new(&project).unwrap();
            let mut reference = WaveReader::open(&root.join(format!("{name}-swift.wav"))).unwrap();
            let mut frame = 0;
            let mut maximum_error = 0_f32;
            let mut nonzero_reference = false;
            while frame < reference.info.frames {
                let count = (reference.info.frames - frame).min(block as u64) as usize;
                let mut left = vec![0.; count];
                let mut right = left.clone();
                let mut expected_left = left.clone();
                let mut expected_right = left.clone();
                renderer.render(&mut left, &mut right).unwrap();
                reference
                    .read_stereo(frame, &mut expected_left, &mut expected_right)
                    .unwrap();
                for ((l, r), (el, er)) in left
                    .iter()
                    .zip(&right)
                    .zip(expected_left.iter().zip(&expected_right))
                {
                    assert!(l.is_finite() && r.is_finite());
                    nonzero_reference |= el.abs() > 0.01 || er.abs() > 0.01;
                    maximum_error = maximum_error.max((l - el).abs()).max((r - er).abs());
                }
                frame += count as u64;
            }
            assert!(nonzero_reference, "Empty reference: {name}");
            assert!(
                maximum_error < 5e-6,
                "{name}, block {block}: {maximum_error}"
            );
            println!("gain-reference {name} block={block} max_error={maximum_error}");
        }
    }
}
