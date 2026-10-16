use soundroom_engine::drums::{self, Articulation, Kit, Pattern, Step, Voice};
use soundroom_engine::sampled_drums;
use std::sync::atomic::AtomicBool;

/// 6-channel 24-bit 48 kHz hit: a decaying tone of `amplitude` in every stem,
/// with the close stem panned slightly left so stereo is observable.
fn write_hit(path: &std::path::Path, amplitude: f32, frames: usize) {
    let mut out = Vec::new();
    let data = frames * 6 * 3;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&6u16.to_le_bytes());
    out.extend_from_slice(&48_000u32.to_le_bytes());
    out.extend_from_slice(&(48_000u32 * 18).to_le_bytes());
    out.extend_from_slice(&18u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data as u32).to_le_bytes());
    for i in 0..frames {
        let v = amplitude * (i as f32 * 0.05).sin() * (-(i as f32) / 2000.).exp();
        for scale in [1., 0.6, 0.8, 0.8, 0.5, 0.5] {
            let q = ((v * scale * 0.5).clamp(-1., 1.) * 8_388_607.) as i32;
            out.extend_from_slice(&q.to_le_bytes()[..3]);
        }
    }
    std::fs::write(path, out).unwrap();
}

#[test]
fn recorded_kits_choose_layers_by_velocity_vary_by_robin_and_mix_stems_in_stereo() {
    let root = std::env::temp_dir().join(format!("soundroom-kits-{}", std::process::id()));
    let kit = root.join(sampled_drums::KIT_DIRECTORY);
    std::fs::create_dir_all(kit.join("snare/center")).unwrap();
    std::fs::create_dir_all(kit.join("snare/sideStick")).unwrap();
    let mut lines = String::new();
    for (i, amplitude) in [0.05f32, 0.1, 0.2, 0.4, 0.8].into_iter().enumerate() {
        let file = format!("snare/center/{i:02}.wav");
        write_hit(&kit.join(&file), amplitude, 9600 + i * 10);
        lines += &format!("{{\"voice\":\"snare\",\"articulation\":\"center\",\"file\":\"{file}\",\"power\":{}}}\n", amplitude * amplitude);
    }
    write_hit(&kit.join("snare/sideStick/00.wav"), 0.3, 2400);
    lines += "{\"voice\":\"snare\",\"articulation\":\"sideStick\",\"file\":\"snare/sideStick/00.wav\",\"power\":0.09}\n";
    std::fs::write(kit.join("hits.jsonl"), lines).unwrap();
    sampled_drums::set_kit_roots(vec![root.clone()]);
    assert!(sampled_drums::available());
    let cancel = AtomicBool::new(false);
    let render = |velocity, robin, articulation| {
        sampled_drums::render(Kit::StudioNatural, Voice::Snare, velocity, robin, articulation, 0.5, &cancel)
            .unwrap()
            .expect("recorded snare")
    };
    let peak = |a: &[Vec<f32>; 2]| a.iter().flatten().fold(0f32, |m, v| m.max(v.abs()));
    let (soft, hard) = (render(0.05, 0, Articulation::Center), render(1., 0, Articulation::Center));
    assert!(peak(&hard) > 6. * peak(&soft), "velocity selects louder recorded layers");
    assert_ne!(soft[0], soft[1], "stems keep their stereo image");
    let (a, b) = (render(0.5, 0, Articulation::Center), render(0.5, 1, Articulation::Center));
    assert_ne!(a[0].len(), b[0].len(), "round robin picks a neighbouring take");
    let rim = render(0.8, 0, Articulation::Rimshot);
    assert!(peak(&rim) > peak(&render(0.8, 0, Articulation::Center)), "rimshot layers the rim click");
    // Clap has no recording: the modeled voice is used instead.
    assert!(sampled_drums::render(Kit::StudioNatural, Voice::Clap, 1., 0, Articulation::Center, 0.5, &cancel).unwrap().is_none());
    // Patterns render recorded kits in stereo and stay deterministic.
    let pattern = Pattern {
        kit: Kit::StudioRoom,
        steps: (0..4).map(|i| Step { articulation: None, voice: Voice::Snare, index: i * 4, velocity: 0.3 + 0.2 * i as f64, pan: 0., probability: 1. }).collect(),
        ..Pattern::default()
    };
    let first = pattern.render(1, false, &cancel).unwrap();
    assert!(peak(&first) > 0.01 && first[0] != first[1]);
    assert_eq!(first[0], pattern.render(1, false, &cancel).unwrap()[0]);
    let vintage = Pattern { kit: Kit::StudioVintage, ..pattern.clone() }.render(1, false, &cancel).unwrap();
    assert_ne!(vintage[0], first[0], "flavors mix the stems differently");
    let _ = drums::MAX_RENDER_FRAMES;
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn measured_kit_is_finite_deterministic_dynamic_and_varies_by_round_robin() {
    use soundroom_engine::measured_drums;
    let cancel = AtomicBool::new(false);
    let peak = |a: &[Vec<f32>; 2]| a.iter().flatten().fold(0f32, |m, v| m.max(v.abs()));
    for voice in Voice::ALL.into_iter().filter(|v| *v != Voice::Clap) {
        let render = |v, robin| measured_drums::render(Kit::StudioModeled, voice, v, robin, Articulation::Center, 0.5, None, &cancel).unwrap().expect("measured voice");
        let (soft, hard) = (render(0.2, 0), render(0.9, 0));
        assert!(soft.iter().flatten().chain(hard.iter().flatten()).all(|v| v.is_finite()));
        assert!(peak(&hard) > 2. * peak(&soft), "{voice:?} dynamics");
        assert!(peak(&hard) < 1.5, "{voice:?} bounded level");
        assert_eq!(render(0.9, 0)[0], hard[0], "{voice:?} deterministic");
        assert_ne!(render(0.9, 3)[0], hard[0], "{voice:?} round robins differ");
        assert!(hard[0].first().unwrap().abs() < 1e-3, "{voice:?} starts without a step");
    }
    // Other kits are not measured.
    assert!(measured_drums::render(Kit::Acoustic, Voice::Kick, 0.8, 0, Articulation::Center, 0.5, None, &cancel).unwrap().is_none());
}

#[test]
fn kit_designer_shape_changes_the_modeled_hit_and_defaults_are_neutral() {
    use soundroom_engine::measured_drums::{self, Shape};
    let cancel = AtomicBool::new(false);
    let render = |shape: Option<Shape>| measured_drums::render(Kit::StudioModeled, Voice::Snare, 0.8, 0, Articulation::Center, 0.5, shape, &cancel).unwrap().unwrap();
    let plain = render(None);
    assert_eq!(render(Some(Shape::default()))[0], plain[0], "default shape is the measured kit");
    // Wires live above the drum tone: compare first-difference (high-passed) energy.
    let energy = |a: &[Vec<f32>; 2]| a[0].windows(2).map(|w| ((w[1] - w[0]) as f64).powi(2)).sum::<f64>();
    let quiet_wires = render(Some(Shape { wires: -24., ..Shape::default() }));
    assert!(energy(&quiet_wires) < 0.8 * energy(&plain), "wires control the noise");
    let short = render(Some(Shape { sustain: 0.3, ..Shape::default() }));
    assert!(short[0].len() < plain[0].len(), "sustain shortens the ring");
    let tense = render(Some(Shape { tension: 7., ..Shape::default() }));
    assert_ne!(tense[0], plain[0]);
    let wild = render(Some(Shape { tension: f64::NAN, room: 1e9, ..Shape::default() }));
    assert!(wild.iter().flatten().all(|v| v.is_finite()), "out-of-range shapes are sanitized");
}

#[test]
fn audition_plays_the_piece_through_its_lane_kit_and_shape() {
    use soundroom_engine::drums::Lane;
    use soundroom_engine::measured_drums::Shape;
    let cancel = AtomicBool::new(false);
    let mut pattern = Pattern { kit: Kit::Acoustic, ..Pattern::default() };
    let synth = pattern.audition(Voice::Snare, 0.9, &cancel).unwrap();
    pattern.lanes.insert("snare".into(), Lane { kit: Kit::StudioModeled, ..Lane::default() });
    let modeled = pattern.audition(Voice::Snare, 0.9, &cancel).unwrap();
    assert_ne!(synth[0], modeled[0], "audition follows the lane's kit");
    pattern.lanes.get_mut("snare").unwrap().shape = Some(Shape { sustain: 0.3, ..Shape::default() });
    let short = pattern.audition(Voice::Snare, 0.9, &cancel).unwrap();
    assert!(short[0].len() < modeled[0].len(), "and its kit-designer shape");
    assert!(modeled[0].len() < 48_000 * 6, "trimmed to the hit");
}

#[test]
fn first_pass_starts_silent_while_the_loop_carries_the_last_hits_tail() {
    let cancel = AtomicBool::new(false);
    let pattern = Pattern {
        kit: Kit::Acoustic,
        steps: vec![Step { articulation: None, voice: Voice::Crash, index: 15, velocity: 0.9, pan: 0., probability: 1. }],
        ..Pattern::default()
    };
    let energy = |x: &[f32]| x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>();
    let looped = pattern.render(1, false, &cancel).unwrap();
    let first = pattern.render_first_pass(1, &cancel).unwrap();
    assert_eq!(first[0].len(), looped[0].len());
    assert!(energy(&looped[0][..4800]) > 0., "the loop's first beat carries the crash tail");
    assert_eq!(energy(&first[0][..4800]), 0., "the first pass starts from silence");
}

#[test]
fn every_piece_renders_with_every_control_at_both_extremes() {
    use soundroom_engine::measured_drums::{self, Shape};
    let cancel = AtomicBool::new(false);
    let extremes: Vec<Shape> = (0..10)
        .flat_map(|field| [false, true].map(move |high| (field, high)))
        .map(|(field, high)| {
            let mut s = Shape::default();
            let pick = |lo: f64, hi: f64| if high { hi } else { lo };
            match field {
                0 => s.tension = pick(-12., 12.),
                1 => s.sustain = pick(0.25, 3.),
                2 => s.body = pick(-24., 12.),
                3 => s.snap = pick(-24., 12.),
                4 => s.wires = pick(-60., 12.),
                5 => s.tone = pick(-1., 1.),
                6 => s.pitch_drop = pick(0., 3.),
                7 => s.room = pick(-36., 9.),
                8 => s.room_size = pick(0.4, 2.5),
                _ => s.variation = pick(0., 2.5),
            }
            s
        })
        .chain([Shape { sustain: 3., tension: 12., pitch_drop: 3., room_size: 2.5, ..Shape::default() }])
        .collect();
    for voice in Voice::ALL.into_iter().filter(|v| *v != Voice::Clap) {
        for shape in &extremes {
            let hit = measured_drums::render(Kit::StudioModeled, voice, 1., 3, Articulation::Center, 0.5, Some(*shape), &cancel)
                .unwrap_or_else(|e| panic!("{voice:?} {shape:?}: {e}"))
                .unwrap();
            assert!(hit.iter().flatten().all(|v| v.is_finite()), "{voice:?} {shape:?}");
        }
    }
}
