use soundroom_engine::{
    authority::{Authority, Role, Transport},
    automation::Lane,
    model::{AutomationPoint, Session, Timing, Track, MAX_FRAME},
    project::Project,
    render::{export, Renderer},
    wave::{WaveReader, WaveWriter},
};
use std::{collections::BTreeMap, path::PathBuf, sync::atomic::AtomicBool};
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(format!("{name}.soundroom"))
}
fn temporary(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("soundroom-rust-{name}-{}", std::process::id()))
}
fn sample_track() -> Track {
    Track {
        id: "22222222-2222-4222-8222-222222222222".into(),
        ..Track::default()
    }
}
fn lane_point(frame: u64, value: f32) -> AutomationPoint {
    serde_json::from_value(serde_json::json!({"frame":frame,"value":value})).unwrap()
}
#[test]
fn old_projects_default_timing_preserve_unknown_fields() {
    for version in 1..=11 {
        let json = serde_json::json!({"version":version,"tracks":[],"masteringAnalysis":{"custom":42},"futureOptional":{"keep":true}});
        let s: Session = serde_json::from_value(json.clone()).unwrap();
        s.validate().unwrap();
        assert_eq!(s.timing, Timing::default());
        let out = serde_json::to_value(s).unwrap();
        assert_eq!(out["masteringAnalysis"], json["masteringAnalysis"]);
        assert_eq!(out["futureOptional"], json["futureOptional"]);
    }
}
#[test]
fn future_or_invalid_versions_are_rejected() {
    for version in [0, 12, u32::MAX] {
        let s: Session = serde_json::from_value(serde_json::json!({"version":version})).unwrap();
        assert!(s.validate().is_err())
    }
}
#[test]
fn invalid_automation_rejected() {
    let mut track = sample_track();
    track
        .mix_automation
        .insert("volume".into(), vec![lane_point(MAX_FRAME + 1, 0.5)]);
    let s = Session {
        tracks: vec![track],
        ..Session::default()
    };
    assert!(s.validate().is_err());
}
#[test]
fn fractional_tempo_snap_absolute() {
    let t = Timing {
        tempo: 123.,
        beats_per_bar: 7,
        beat_unit: 8,
    };
    for beat in 0..1000 {
        let expected = (beat as f64 * t.frames_per_beat()).round() as u64;
        assert_eq!(t.snap(expected, 1.), expected)
    }
}
#[test]
fn automation_duplicate_domain_and_silence() {
    let mut a = lane_point(0, 0.);
    a.curve = "smooth".into();
    let b = lane_point(100, 1.);
    let lane = Lane::new(&[a, b], 0.5, true);
    assert_eq!(lane.value(0), 0.);
    assert_eq!(lane.value(100), 1.);
    assert!(lane.value(50) > 0. && lane.value(50) < 0.1);
    let lane = Lane::new(&[lane_point(10, 0.2), lane_point(10, 0.8)], 0.3, false);
    assert_eq!(lane.value(9), 0.3);
    assert_eq!(lane.value(10), 0.8);
}
#[test]
fn authored_undo_keeps_unrelated_track_edits() {
    let a = sample_track();
    let b = Track {
        id: "77777777-7777-4777-8777-777777777777".into(),
        ..Track::default()
    };
    let mut state = Authority::new(Session {
        tracks: vec![a.clone(), b.clone()],
        ..Session::default()
    })
    .unwrap();
    let mut a1 = a.clone();
    a1.name = "Alice".into();
    state
        .edit_tracks(
            "alice",
            Role::Editor,
            "one",
            &BTreeMap::from([(a.id.clone(), 0)]),
            vec![a1],
        )
        .unwrap();
    let mut b1 = b.clone();
    b1.name = "Bob".into();
    state
        .edit_tracks(
            "bob",
            Role::Editor,
            "two",
            &BTreeMap::from([(b.id.clone(), 0)]),
            vec![b1.clone()],
        )
        .unwrap();
    state.undo("alice", Role::Editor).unwrap();
    assert_eq!(state.session.tracks[0], a);
    assert_eq!(state.session.tracks[1], b1);
    state.redo("alice", Role::Editor).unwrap();
    assert_eq!(state.session.tracks[0].name, "Alice");
}
#[test]
fn undo_conflicts_when_another_client_changes_same_track() {
    let a = sample_track();
    let mut state = Authority::new(Session {
        tracks: vec![a.clone()],
        ..Session::default()
    })
    .unwrap();
    let mut a1 = a.clone();
    a1.name = "Alice".into();
    state
        .edit_tracks(
            "alice",
            Role::Editor,
            "one",
            &BTreeMap::from([(a.id.clone(), 0)]),
            vec![a1.clone()],
        )
        .unwrap();
    a1.name = "Bob".into();
    state
        .edit_tracks(
            "bob",
            Role::Editor,
            "two",
            &BTreeMap::from([(a.id, 1)]),
            vec![a1],
        )
        .unwrap();
    assert!(state.undo("alice", Role::Editor).is_err());
    assert_eq!(state.session.tracks[0].name, "Bob");
}
#[test]
fn retries_deduplicate_and_reused_id_rejected() {
    let a = sample_track();
    let mut state = Authority::new(Session {
        tracks: vec![a.clone()],
        ..Session::default()
    })
    .unwrap();
    let expected = BTreeMap::from([(a.id.clone(), 0)]);
    let receipt = state
        .edit_tracks("alice", Role::Editor, "one", &expected, vec![a.clone()])
        .unwrap();
    assert_eq!(
        state
            .edit_tracks("alice", Role::Editor, "one", &expected, vec![a.clone()])
            .unwrap(),
        receipt
    );
    let mut changed = a.clone();
    changed.name = "Changed".into();
    assert!(state
        .edit_tracks("alice", Role::Editor, "one", &expected, vec![changed])
        .is_err());
    assert_eq!(state.revision, 1);
}
#[test]
fn timing_undo_refuses_recording_and_foreign_change() {
    let mut state = Authority::new(Session::default()).unwrap();
    state
        .set_timing(
            "alice",
            Role::Editor,
            0,
            Timing {
                tempo: 123.,
                ..Timing::default()
            },
        )
        .unwrap();
    state
        .backend_committed_transport(Transport::Recording { frame: 100 })
        .unwrap();
    assert!(state.undo("alice", Role::Editor).is_err());
    state
        .backend_committed_transport(Transport::Stopped { frame: 100 })
        .unwrap();
    state
        .set_timing(
            "bob",
            Role::Editor,
            1,
            Timing {
                tempo: 130.,
                ..Timing::default()
            },
        )
        .unwrap();
    assert!(state.undo("alice", Role::Editor).is_err());
}
#[test]
fn viewers_cannot_edit_and_backend_only_clock() {
    let mut state = Authority::new(Session::default()).unwrap();
    assert!(state
        .set_timing("viewer", Role::Viewer, 0, Timing::default())
        .is_err());
    state
        .backend_committed_transport(Transport::Playing { frame: 150 })
        .unwrap();
    assert_eq!(state.transport, Transport::Playing { frame: 150 });
}
#[test]
fn revision_exhaustion_is_transactional() {
    let a = sample_track();
    let mut state = Authority::new(Session {
        tracks: vec![a.clone()],
        ..Session::default()
    })
    .unwrap();
    state.revision = MAX_FRAME;
    assert!(state
        .edit_tracks(
            "a",
            Role::Editor,
            "r",
            &BTreeMap::from([(a.id.clone(), 0)]),
            vec![a]
        )
        .is_err());
    assert_eq!(state.track_revisions.values().next(), Some(&0));
}
#[test]
fn wav_streaming_exact_float_and_invalid_range() {
    let path = temporary("float.wav");
    let _ = std::fs::remove_file(&path);
    let mut writer = WaveWriter::create(&path, 4).unwrap();
    writer
        .write(&[0.2, -0.4, f32::NAN, 1.1], &[0., 0.1, -0.1, -1.1])
        .unwrap();
    writer.finish().unwrap();
    let mut reader = WaveReader::open(&path).unwrap();
    let mut l = [0.; 4];
    let mut r = [0.; 4];
    reader.read_stereo(0, &mut l, &mut r).unwrap();
    assert_eq!(l, [0.2, -0.4, 0., 1.1]);
    assert!(reader.read_stereo(1, &mut l, &mut r).is_err());
    std::fs::remove_file(path).unwrap();
}
/// Pitch fixtures whose correction intentionally improved after the Swift
/// engine was retired (octave-stable tracking, exactly followed curves). They
/// keep the block-size and seek invariance checks against one full Rust render.
const REVISED_PITCH: [&str; 10] = [
    "pitch-automatic",
    "pitch-major",
    "pitch-custom",
    "pitch-up",
    "pitch-down",
    "pitch-formant",
    "pitch-edits",
    "pitch-automation",
    "pitch-master",
    "pitch-chain",
];
/// The whole fixture rendered in 8192-frame blocks.
fn render_whole(project: &Project, frames: u64) -> (Vec<f32>, Vec<f32>) {
    let mut renderer = Renderer::new(project).unwrap();
    let (mut left, mut right) = (vec![0.; frames as usize], vec![0.; frames as usize]);
    for (l, r) in left.chunks_mut(8192).zip(right.chunks_mut(8192)) {
        renderer.render(l, r).unwrap();
    }
    (left, right)
}
/// Reference audio for `frames` at `position`: the Swift render, or for a
/// revised pitch fixture the whole Rust render.
fn reference(
    swift: &mut WaveReader,
    own: &Option<(Vec<f32>, Vec<f32>)>,
    position: u64,
    left: &mut [f32],
    right: &mut [f32],
) {
    match own {
        Some((l, r)) => {
            let range = position as usize..position as usize + left.len();
            left.copy_from_slice(&l[range.clone()]);
            right.copy_from_slice(&r[range]);
        }
        None => swift.read_stereo(position, left, right).unwrap(),
    }
}
#[test]
fn swift_render_parity_across_block_sizes() {
    let names = [
        "envelopes",
        "legacy-gain",
        "legacy-lowpass",
        "legacy-echo",
        "legacy-warmth",
        "modern-gain",
        "modern-lowpass",
        "modern-highpass",
        "modern-echo",
        "compressor",
        "chorus",
        "dither",
        "equalizer",
        "room-classic",
        "room-dense",
        "room-transition",
        "warmth-tube",
        "warmth-transition",
        "limiter-track",
        "limiter-master",
        "limiter-bypass",
        "resonance-track",
        "resonance-master",
        "resonance-delta",
        "resonance-bypass",
        "latency-chain",
        "tail-room",
        "tail-chorus",
        "tail-echo",
        "limiter-sample",
        "dither-only",
        "warmth-diode",
        "warmth-classic",
        "mixed-latencies",
        "pitch-automatic",
        "pitch-major",
        "pitch-custom",
        "pitch-mask-zero",
        "pitch-up",
        "pitch-down",
        "pitch-formant",
        "pitch-edits",
        "pitch-automation",
        "pitch-bypass",
        "pitch-master",
        "pitch-chain",
    ];
    for name in names {
        let project = Project::open(&fixture(name)).unwrap();
        let expected = fixture(name)
            .parent()
            .unwrap()
            .join(format!("{name}-swift.wav"));
        let own = REVISED_PITCH
            .contains(&name)
            .then(|| render_whole(&project, WaveReader::open(&expected).unwrap().info.frames));
        for block in [1, 128, 257, 8192] {
            let mut renderer = Renderer::new(&project).unwrap();
            let mut swift = WaveReader::open(&expected).unwrap();
            let mut max_error = 0_f32;
            let end = swift.info.frames;
            if name.starts_with("tail-") {
                assert_eq!(
                    project.session.end() + renderer.tail_frames(),
                    end,
                    "Tail bound mismatch {name}"
                );
            }
            while renderer.position() < end {
                let pos = renderer.position();
                let n = (end - pos).min(block) as usize;
                let mut left = vec![0.; n];
                let mut right = vec![0.; n];
                let mut el = vec![0.; n];
                let mut er = vec![0.; n];
                renderer.render(&mut left, &mut right).unwrap();
                reference(&mut swift, &own, pos, &mut el, &mut er);
                for (a, b) in left.iter().zip(&el).chain(right.iter().zip(&er)) {
                    max_error = max_error.max((a - b).abs());
                }
            }
            println!("Swift parity {name} block {block}: maximum absolute error {max_error}");
            assert!(max_error < 0.000005, "{name} block {block}: {max_error}");
        }
    }
}
#[test]
fn project_copy_preserves_metadata_media_and_rejects_existing() {
    let project = Project::open(&fixture("modern-echo")).unwrap();
    let target = temporary("copy.soundroom");
    let _ = std::fs::remove_dir_all(&target);
    project.save_new(&target).unwrap();
    let restored = Project::open(&target).unwrap();
    assert_eq!(restored.session, project.session);
    assert!(project.save_new(&target).is_err());
    std::fs::remove_dir_all(target).unwrap();
}
#[test]
fn project_replacement_preserves_audio_history_and_unused_sources() {
    let source = Project::open(&fixture("modern-echo")).unwrap();
    let target = temporary("replacement.soundroom");
    let _ = std::fs::remove_dir_all(&target);
    source.save_new(&target).unwrap();
    let unused = target.join("Audio/AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA.wav");
    std::fs::copy(&source.media.values().next().unwrap().path, &unused).unwrap();
    let mut opened = Project::open(&target).unwrap();
    opened.session.name = "Edited while open".into();
    opened.session.tracks[0].gain = 0.375;
    let audio = std::fs::read(&opened.media.values().next().unwrap().path).unwrap();
    opened.save(&target, true).unwrap();
    let restored = Project::open(&target).unwrap();
    assert_eq!(restored.session, opened.session);
    assert_eq!(
        std::fs::read(&restored.media.values().next().unwrap().path).unwrap(),
        audio
    );
    assert_eq!(std::fs::read(&unused).unwrap(), audio);
    // A second save must keep working when source paths name the destination.
    opened.session.tracks[0].gain = 0.5;
    opened.save(&target, true).unwrap();
    assert_eq!(Project::open(&target).unwrap().session.tracks[0].gain, 0.5);
    std::fs::remove_dir_all(target).unwrap();
}
#[test]
fn failed_project_replacement_leaves_the_previous_save_readable() {
    let source = Project::open(&fixture("modern-echo")).unwrap();
    let target = temporary("replacement-failure.soundroom");
    let _ = std::fs::remove_dir_all(&target);
    source.save_new(&target).unwrap();
    let before = std::fs::read(target.join("session.json")).unwrap();
    let mut invalid = Project::open(&target).unwrap();
    invalid.session.name = "Must not be committed".into();
    invalid.media.values_mut().next().unwrap().path = temporary("missing-source.wav");
    assert!(invalid.save(&target, true).is_err());
    assert_eq!(std::fs::read(target.join("session.json")).unwrap(), before);
    assert_eq!(Project::open(&target).unwrap().session, source.session);
    std::fs::remove_dir_all(target).unwrap();
}
#[cfg(unix)]
#[test]
fn project_replacement_rejects_links_and_nonproject_destinations() {
    let source = Project::open(&fixture("modern-echo")).unwrap();
    let directory = temporary("replacement-links");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir(&directory).unwrap();
    let target = directory.join("Link.soundroom");
    std::os::unix::fs::symlink(fixture("modern-echo"), &target).unwrap();
    assert!(source.save(&target, true).is_err());
    assert!(std::fs::symlink_metadata(&target)
        .unwrap()
        .file_type()
        .is_symlink());
    let unrelated = directory.join("Unrelated.soundroom");
    std::fs::create_dir(&unrelated).unwrap();
    std::fs::write(unrelated.join("keep.txt"), "keep me").unwrap();
    assert!(source.save(&unrelated, true).is_err());
    assert_eq!(
        std::fs::read_to_string(unrelated.join("keep.txt")).unwrap(),
        "keep me"
    );
    std::fs::remove_dir_all(directory).unwrap();
}
#[test]
fn unsupported_active_effect_fails_without_creating_output() {
    let mut p = Project::open(&fixture("legacy-gain")).unwrap();
    p.session.tracks[0].realtime_effects[0].kind = "unknownEffect".into();
    let target = temporary("unsupported.wav");
    let _ = std::fs::remove_file(&target);
    assert!(export(&p, &target, 0, 100, &AtomicBool::new(false), |_, _| {}).is_err());
    assert!(!target.exists());
}
#[test]
fn cancellation_cleans_output() {
    let p = Project::open(&fixture("envelopes")).unwrap();
    let target = temporary("cancelled.wav");
    let _ = std::fs::remove_file(&target);
    assert!(export(
        &p,
        &target,
        0,
        p.session.end(),
        &AtomicBool::new(true),
        |_, _| {}
    )
    .is_err());
    assert!(!target.exists());
}
#[test]
fn deterministic_seek_has_effect_history() {
    let p = Project::open(&fixture("modern-echo")).unwrap();
    let mut a = Renderer::new(&p).unwrap();
    a.advance_to(10000, &AtomicBool::new(false)).unwrap();
    let mut l = [0.; 128];
    let mut r = [0.; 128];
    a.render(&mut l, &mut r).unwrap();
    let expected = fixture("modern-echo")
        .parent()
        .unwrap()
        .join("modern-echo-swift.wav");
    let mut reference = WaveReader::open(&expected).unwrap();
    let mut el = [0.; 128];
    let mut er = [0.; 128];
    reference.read_stereo(10000, &mut el, &mut er).unwrap();
    for (a, b) in l.iter().zip(&el).chain(r.iter().zip(&er)) {
        assert!((a - b).abs() < 0.000005)
    }
}

#[test]
fn exclusive_storage_commit_cannot_replace_an_empty_directory() {
    let source = temporary("stage-dir");
    let target = temporary("existing-dir");
    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&target);
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&target).unwrap();
    std::fs::write(source.join("marker"), b"preserved").unwrap();
    assert!(soundroom_engine::storage::commit_new_directory(&source, &target).is_err());
    assert!(source.join("marker").exists());
    assert!(!target.join("marker").exists());
    std::fs::remove_dir_all(source).unwrap();
    std::fs::remove_dir_all(target).unwrap();
}
#[test]
fn take_metadata_can_reference_the_active_clip_identity() {
    let mut p = Project::open(&fixture("envelopes")).unwrap();
    let clips = p.session.tracks[0].clips.clone();
    p.session.tracks[0]
        .recording_takes
        .push(soundroom_engine::model::Take {
            clips,
            extra: Default::default(),
        });
    p.validate_sources().unwrap();
}
#[test]
fn unsafe_frozen_boundary_is_rejected_before_render() {
    let mut p = Project::open(&fixture("envelopes")).unwrap();
    let c = &mut p.session.tracks[0].clips[0];
    c.render_envelope=Some(serde_json::from_value(serde_json::json!({"referenceLength":100,"attackFrames":0,"releaseFrames":0,"boundaryAttackFrames":0,"boundaryReleaseFrames":0,"additionalBoundaries":[{"start":MAX_FRAME,"length":2,"attack":0,"release":0}]})).unwrap());
    assert!(p.validate_sources().is_err());
}

#[test]
fn latency_processors_seek_preserves_dsp_and_automation_history() {
    for name in [
        "warmth-transition",
        "limiter-track",
        "limiter-master",
        "resonance-track",
        "resonance-master",
        "resonance-delta",
        "latency-chain",
        "room-transition",
        "equalizer",
        "pitch-edits",
        "pitch-automation",
        "pitch-down",
        "pitch-formant",
        "pitch-master",
        "pitch-chain",
    ] {
        let p = Project::open(&fixture(name)).unwrap();
        let reference_path = fixture(name)
            .parent()
            .unwrap()
            .join(format!("{name}-swift.wav"));
        let own = REVISED_PITCH
            .contains(&name)
            .then(|| render_whole(&p, WaveReader::open(&reference_path).unwrap().info.frames));
        for position in [0, 5999, 7001, 12345] {
            let mut renderer = Renderer::new(&p).unwrap();
            renderer
                .advance_to(position, &AtomicBool::new(false))
                .unwrap();
            let mut left = [0.; 257];
            let mut right = [0.; 257];
            renderer.render(&mut left, &mut right).unwrap();
            let mut swift = WaveReader::open(&reference_path).unwrap();
            let mut el = [0.; 257];
            let mut er = [0.; 257];
            reference(&mut swift, &own, position, &mut el, &mut er);
            let error = left
                .iter()
                .zip(&el)
                .chain(right.iter().zip(&er))
                .map(|(a, b)| (a - b).abs())
                .fold(0_f32, f32::max);
            assert!(error < 0.000005, "{name} seek {position}: {error}");
        }
    }
}
#[test]
fn master_latency_budget_is_validated_before_output_creation() {
    let mut p = Project::open(&fixture("resonance-master")).unwrap();
    let mut effect = p.session.master_track.realtime_effects[1].clone();
    effect.id = "77777777-7777-4777-8777-777777777777".into();
    p.session.master_track.realtime_effects.push(effect);
    let mut effect = p.session.master_track.realtime_effects[1].clone();
    effect.id = "88888888-8888-4888-8888-888888888888".into();
    p.session.master_track.realtime_effects.push(effect);
    assert!(Renderer::new(&p).is_err());
}
#[test]
fn tail_export_streams_expected_length_and_samples() {
    let p = Project::open(&fixture("tail-room")).unwrap();
    let target = temporary("tail-room.wav");
    let _ = std::fs::remove_file(&target);
    soundroom_engine::render::export_with_tails(
        &p,
        &target,
        0,
        p.session.end(),
        true,
        &AtomicBool::new(false),
        |_, _| {},
    )
    .unwrap();
    let mut actual = WaveReader::open(&target).unwrap();
    let reference_path = fixture("tail-room")
        .parent()
        .unwrap()
        .join("tail-room-swift.wav");
    let mut reference = WaveReader::open(&reference_path).unwrap();
    assert_eq!(actual.info.frames, reference.info.frames);
    let mut pos = 0;
    while pos < actual.info.frames {
        let n = (actual.info.frames - pos).min(8192) as usize;
        let mut l = vec![0.; n];
        let mut r = vec![0.; n];
        let mut el = vec![0.; n];
        let mut er = vec![0.; n];
        actual.read_stereo(pos, &mut l, &mut r).unwrap();
        reference.read_stereo(pos, &mut el, &mut er).unwrap();
        for (a, b) in l.iter().zip(&el).chain(r.iter().zip(&er)) {
            assert!((a - b).abs() < 0.000005);
        }
        pos += n as u64;
    }
    drop(actual);
    std::fs::remove_file(target).unwrap();
}
#[test]
fn cancelled_preparation_never_opens_output() {
    let p = Project::open(&fixture("latency-chain")).unwrap();
    assert!(Renderer::new_cancellable(&p, &AtomicBool::new(true)).is_err());
}
