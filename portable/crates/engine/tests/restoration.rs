//! Qualify restoration inserts in the actual compensated playback/export graph.
use soundroom_engine::{
    model::{Clip, Effect, Session, Track},
    project::{Media, Project},
    render::{default_parameters, export, CallbackStatus, LoopRange, PreparedGraph, Renderer},
    wave::{WaveReader, WaveWriter},
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    collections::BTreeMap,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

thread_local! {
    static PROBE: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static RETIREMENTS: Cell<usize> = const { Cell::new(0) };
}
struct Allocator;
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if PROBE.try_with(Cell::get).unwrap_or(false) {
            let _ = ALLOCATIONS.try_with(|v| v.set(v.get() + 1));
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if PROBE.try_with(Cell::get).unwrap_or(false) {
            let _ = RETIREMENTS.try_with(|v| v.set(v.get() + 1));
        }
        unsafe { System.dealloc(pointer, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;
static NEXT: AtomicU64 = AtomicU64::new(0);
const FRAMES: usize = 24_000;
static CANCEL: AtomicBool = AtomicBool::new(false);

struct Fixture {
    directory: PathBuf,
    project: Project,
}
impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "soundroom-restoration-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("source.wav");
        let tone = |i: usize, hz: f32| (i as f32 * hz * std::f32::consts::TAU / 48_000.).sin();
        let left: Vec<_> = (0..FRAMES)
            .map(|i| {
                0.2 * tone(i, 240.) + 0.07 * tone(i, 7100.) + if i % 2400 == 31 { 0.4 } else { 0. }
            })
            .collect();
        let right: Vec<_> = (0..FRAMES)
            .map(|i| 0.17 * tone(i, 240.) - 0.04 * tone(i, 1650.))
            .collect();
        let mut writer = WaveWriter::create(&path, FRAMES as u64).unwrap();
        for (l, r) in left.chunks(8192).zip(right.chunks(8192)) {
            writer.write(l, r).unwrap();
        }
        writer.finish().unwrap();
        let info = WaveReader::open(&path).unwrap().info;
        let asset = "A1A1A1A1-0000-4000-8000-000000000001".to_string();
        let clip: Clip = serde_json::from_value(serde_json::json!({
            "id":"A1A1A1A1-0000-4000-8000-000000000002", "assetID":asset,
            "name":"Restoration fixture", "start":0, "offset":0, "length":FRAMES,
            "edgeProtectionDisabled":true
        }))
        .unwrap();
        let project = Project {
            session: Session {
                tracks: vec![Track {
                    id: "A1A1A1A1-0000-4000-8000-000000000003".into(),
                    clips: vec![clip],
                    ..Track::default()
                }],
                ..Session::default()
            },
            media: BTreeMap::from([(asset, Media { path, info })]),
        };
        Self { directory, project }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
fn insert(kind: &str, index: usize) -> Effect {
    Effect {
        id: format!("A1A1A1A1-0000-4000-8000-{:012}", 100 + index),
        kind: kind.into(),
        amount: 0.5,
        enabled: true,
        automation: vec![],
        parameters: default_parameters(kind),
        parameter_automation: BTreeMap::new(),
        extra: Default::default(),
    }
}
fn render(project: &Project) -> [Vec<f32>; 2] {
    let mut renderer = Renderer::new(project).unwrap();
    let mut output = [vec![0.; FRAMES], vec![0.; FRAMES]];
    let [left, right] = &mut output;
    for (l, r) in left.chunks_mut(257).zip(right.chunks_mut(257)) {
        renderer.render(l, r).unwrap();
    }
    output
}
fn callback(graph: &mut PreparedGraph, count: usize, block: usize) -> [Vec<f32>; 2] {
    let mut output = [vec![0.; count], vec![0.; count]];
    let [left, right] = &mut output;
    ALLOCATIONS.with(|v| v.set(0));
    RETIREMENTS.with(|v| v.set(0));
    PROBE.with(|v| v.set(true));
    for (l, r) in left.chunks_mut(block).zip(right.chunks_mut(block)) {
        assert_eq!(graph.render_callback(l, r), CallbackStatus::Rendered);
    }
    PROBE.with(|v| v.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0, "callback allocated");
    assert_eq!(RETIREMENTS.with(Cell::get), 0, "callback retired storage");
    output
}
fn compare(actual: &[Vec<f32>; 2], expected: &[Vec<f32>; 2]) {
    let error = actual
        .iter()
        .zip(expected)
        .flat_map(|(a, b)| a.iter().zip(b))
        .map(|(a, b)| {
            assert!(a.is_finite());
            (a - b).abs()
        })
        .fold(0_f32, f32::max);
    assert!(error < 2e-6, "maximum sample error {error}");
}

#[test]
fn automation_partitioning_callbacks_and_checkpoint_copies_preserve_restoration() {
    for kinds in [
        vec!["soundPutty"],
        vec!["vocalEnhancer"],
        vec!["sourceIsolator"],
        vec!["vocalEnhancer", "sourceIsolator"],
    ] {
        let mut fixture = Fixture::new();
        for (i, kind) in kinds.iter().enumerate() {
            let mut effect = insert(kind, i);
            let points = serde_json::from_value(serde_json::json!([
                {"frame":0,"value":0.2}, {"frame":1301,"value":0.9},
                {"frame":8007,"value":0.4}, {"frame":16003,"value":1.0}
            ]))
            .unwrap();
            effect.parameter_automation.insert("mix".into(), points);
            fixture.project.session.tracks[0]
                .realtime_effects
                .push(effect);
        }
        let expected = render(&fixture.project);
        assert!(expected[0].iter().any(|v| v.abs() > 0.05));
        let graph =
            PreparedGraph::prepare_resident(&fixture.project, 32 * 1024 * 1024, &CANCEL).unwrap();
        for block in [1, 128, 257, 8192] {
            compare(&callback(&mut graph.fork(), FRAMES, block), &expected);
        }
        let mut warm = graph.fork();
        callback(&mut warm, 6001, 127);
        let checkpoint = warm.checkpoint();
        let mut fork = warm.fork();
        let next = callback(&mut warm, 7003, 128);
        compare(&callback(&mut fork, 7003, 257), &next);
        warm.restore_checkpoint(&checkpoint).unwrap();
        compare(&callback(&mut warm, 7003, 8192), &next);
    }
}

#[test]
fn bypass_and_zero_mix_preserve_dry_timing_on_tracks_and_master() {
    for master in [false, true] {
        for kind in ["vocalEnhancer", "sourceIsolator", "soundPutty"] {
            let mut fixture = Fixture::new();
            let dry = render(&fixture.project);
            for bypass in [false, true] {
                let mut effect = insert(kind, 0);
                effect.enabled = !bypass;
                if !bypass {
                    effect.parameters.insert("mix".into(), 0.);
                }
                let track = if master {
                    &mut fixture.project.session.master_track
                } else {
                    &mut fixture.project.session.tracks[0]
                };
                track.realtime_effects = vec![effect];
                compare(&render(&fixture.project), &dry);
            }
        }
    }
}

#[test]
fn selection_export_preserves_length_and_compensated_chain_timing() {
    let mut fixture = Fixture::new();
    fixture.project.session.tracks[0].realtime_effects =
        vec![insert("vocalEnhancer", 0), insert("sourceIsolator", 1)];
    let expected = render(&fixture.project);
    let (start, end) = (3001_u64, 21007_u64);
    let path = fixture.directory.join("selection.wav");
    export(&fixture.project, &path, start, end, &CANCEL, |_, _| {}).unwrap();
    let mut reader = WaveReader::open(&path).unwrap();
    assert_eq!(reader.info.frames, end - start);
    let mut actual = [
        vec![0.; (end - start) as usize],
        vec![0.; (end - start) as usize],
    ];
    let [left, right] = &mut actual;
    for (index, (l, r)) in left
        .chunks_mut(8192)
        .zip(right.chunks_mut(8192))
        .enumerate()
    {
        reader.read_stereo((index * 8192) as u64, l, r).unwrap();
    }
    compare(
        &actual,
        &expected.map(|v| v[start as usize..end as usize].to_vec()),
    );
}

#[test]
fn restoration_chain_callback_timing_fixture() {
    let mut fixture = Fixture::new();
    fixture.project.session.tracks[0].realtime_effects =
        vec![insert("vocalEnhancer", 0), insert("sourceIsolator", 1)];
    let mut graph = PreparedGraph::prepare_resident_loop(
        &fixture.project,
        32 * 1024 * 1024,
        &CANCEL,
        LoopRange::new(0, FRAMES as u64).unwrap(),
    )
    .unwrap();
    let (mut left, mut right) = ([0.; 128], [0.; 128]);
    let mut timings = Vec::with_capacity(2000);
    for _ in 0..2000 {
        let start = std::time::Instant::now();
        assert_eq!(
            graph.render_callback(&mut left, &mut right),
            CallbackStatus::Rendered
        );
        timings.push(start.elapsed().as_secs_f64() * 1000.);
        assert!(left.iter().chain(&right).all(|v| v.is_finite()));
    }
    timings.sort_by(f64::total_cmp);
    println!("One vocal+isolator chain, stereo 128-frame callbacks: p95={:.3}ms max={:.3}ms; block budget=2.667ms",
        timings[1900], timings[1999]);
    // This records machine/load evidence, not a portable timing promise.
    if std::env::var_os("SOUNDROOM_STRICT_REALTIME").is_some() {
        assert!(
            timings[1999] < 128. / 48.,
            "callback exceeded device block budget"
        );
    }
}

#[test]
fn sound_putty_live_sculpt_preserves_history_and_allocates_nothing_in_callback() {
    use soundroom_engine::render::LivePatch;
    let mut fixture = Fixture::new();
    fixture.project.session.tracks[0].realtime_effects = vec![insert("soundPutty", 0)];
    let mut graph = PreparedGraph::prepare_resident_loop(
        &fixture.project,
        32 * 1024 * 1024,
        &CANCEL,
        LoopRange::new(0, FRAMES as u64).unwrap(),
    )
    .unwrap();
    callback(&mut graph, 2048, 128);
    let mut before = graph.fork();
    let mut edited = fixture.project.session.clone();
    for lane in 0..300 {
        edited.tracks[0].realtime_effects[0]
            .parameters
            .insert(format!("puttyPitch{lane:03}"), 0.8);
    }
    let mut patch = LivePatch::between(&fixture.project.session, &edited).unwrap();
    ALLOCATIONS.with(|n| n.set(0));
    RETIREMENTS.with(|n| n.set(0));
    PROBE.with(|p| p.set(true));
    let applied = graph.apply_live(&mut patch);
    let mut left = [0.; 128];
    let mut right = [0.; 128];
    let status = graph.render_callback(&mut left, &mut right);
    PROBE.with(|p| p.set(false));
    assert!(applied);
    assert_eq!(status, CallbackStatus::Rendered);
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert_eq!(RETIREMENTS.with(Cell::get), 0);
    let mut old = callback(&mut before, 128, 128);
    assert!(old[0]
        .iter_mut()
        .zip(left)
        .any(|(a, b)| (*a - b).abs() > 1e-7));
    let mut timings = Vec::with_capacity(500);
    for _ in 0..500 {
        let start = std::time::Instant::now();
        assert_eq!(
            graph.render_callback(&mut left, &mut right),
            CallbackStatus::Rendered
        );
        timings.push(start.elapsed().as_secs_f64() * 1000.);
        assert!(left.iter().chain(&right).all(|v| v.is_finite()));
    }
    timings.sort_by(f64::total_cmp);
    println!(
        "Sound Putty stereo 128-frame callbacks: p95={:.3}ms max={:.3}ms; block budget=2.667ms",
        timings[475], timings[499]
    );
}

fn route(role: &str) -> soundroom_engine::routing::TrackRouting {
    soundroom_engine::routing::TrackRouting {
        role: role.into(),
        output_id: None,
        sends: vec![],
        sidechain_source_id: None,
    }
}
fn bus(index: usize, role: &str) -> Track {
    Track {
        id: format!("B1B1B1B1-0000-4000-8000-{:012}", index),
        name: role.into(),
        routing: Some(route(role)),
        ..Track::default()
    }
}
#[test]
fn routing_rejects_cycles_missing_destinations_invalid_sends_and_recording_buses() {
    let mut f = Fixture::new();
    let mut group = bus(1, "bus");
    let id = group.id.clone();
    f.project.session.tracks[0].routing = Some(route("audio"));
    f.project.session.tracks[0]
        .routing
        .as_mut()
        .unwrap()
        .output_id = Some(id.clone());
    group.routing.as_mut().unwrap().output_id = Some(id.clone());
    f.project.session.tracks.push(group);
    assert!(f.project.session.validate().is_err());
    f.project.session.tracks[1]
        .routing
        .as_mut()
        .unwrap()
        .output_id = None;
    assert!(f.project.session.validate().is_ok());
    f.project.session.tracks[1].armed = true;
    assert!(f.project.session.validate().is_err());
    f.project.session.tracks[1].armed = false;
    f.project.session.tracks[0]
        .routing
        .as_mut()
        .unwrap()
        .sends
        .push(soundroom_engine::routing::TrackSend {
            track_id: id,
            level: 1.,
        });
    assert!(f.project.session.validate().is_err());
    f.project.session.tracks[0]
        .routing
        .as_mut()
        .unwrap()
        .sends
        .clear();
    f.project.session.tracks[1]
        .routing
        .as_mut()
        .unwrap()
        .sidechain_source_id = Some(f.project.session.tracks[0].id.clone());
    assert!(f.project.session.validate().is_ok());
    f.project.session.tracks[0]
        .routing
        .as_mut()
        .unwrap()
        .sidechain_source_id = Some(f.project.session.tracks[1].id.clone());
    assert!(f.project.session.validate().is_err());
}
#[test]
fn routed_groups_sends_and_nested_latency_are_sample_aligned() {
    let mut f = Fixture::new();
    let dry = render(&f.project);
    let mut group = bus(1, "bus");
    group.gain = 0.5;
    let mut latency = insert("limiter", 1);
    latency.enabled = false;
    group.realtime_effects.push(latency);
    let mut return_track = bus(2, "return");
    return_track.gain = 0.25;
    let mut delay = insert("limiter", 2);
    delay.enabled = false;
    return_track.realtime_effects.push(delay);
    return_track.routing.as_mut().unwrap().output_id = Some(group.id.clone());
    let r = f.project.session.tracks[0].routing.insert(route("audio"));
    r.output_id = Some(group.id.clone());
    r.sends.push(soundroom_engine::routing::TrackSend {
        track_id: return_track.id.clone(),
        level: 0.8,
    });
    f.project.session.tracks.extend([group, return_track]);
    let expected = dry.map(|v| v.into_iter().map(|x| x * 0.6).collect());
    compare(&render(&f.project), &expected);
    let mut graph = PreparedGraph::prepare_resident(&f.project, 4 * 1024 * 1024, &CANCEL).unwrap();
    let prefix = callback(&mut graph, 4000, 127);
    compare(&prefix, &expected.clone().map(|x| x[..4000].to_vec()));
    let checkpoint = graph.checkpoint();
    let next = callback(&mut graph, 5000, 257);
    graph.restore_checkpoint(&checkpoint).unwrap();
    compare(&callback(&mut graph, 5000, 1024), &next);
    let path = f.directory.join("routed.wav");
    export(&f.project, &path, 0, FRAMES as u64, &CANCEL, |_, _| {}).unwrap();
    let mut reader = WaveReader::open(&path).unwrap();
    let mut out = [vec![0.; FRAMES], vec![0.; FRAMES]];
    let [l, r] = &mut out;
    for (i, (l, r)) in l.chunks_mut(8192).zip(r.chunks_mut(8192)).enumerate() {
        reader.read_stereo((i * 8192) as u64, l, r).unwrap();
    }
    compare(&out, &expected);
    f.project.session.tracks[1].solo = true;
    compare(&render(&f.project), &expected);
    let target = f.directory.join("Routed.soundroom");
    f.project.save_new(&target).unwrap();
    assert_eq!(Project::open(&target).unwrap().session.version, 10);
}
#[test]
fn sidechain_detects_external_audio_and_compensates_prior_insert_latency() {
    let mut f = Fixture::new();
    let mut key = f.project.session.tracks[0].clone();
    key.id = "B1B1B1B1-0000-4000-8000-000000000010".into();
    key.clips[0].id = "B1B1B1B1-0000-4000-8000-000000000011".into();
    let mut sink = bus(3, "bus");
    sink.muted = true;
    key.routing = Some(route("audio"));
    key.routing.as_mut().unwrap().output_id = Some(sink.id.clone());
    f.project.session.tracks[0].gain = 0.5;
    let r = f.project.session.tracks[0].routing.insert(route("audio"));
    r.sidechain_source_id = Some(key.id.clone());
    let mut compressor = insert("compressor", 3);
    compressor.parameters.insert("threshold".into(), -40.);
    compressor.parameters.insert("attack".into(), 0.1);
    f.project.session.tracks[0]
        .realtime_effects
        .push(compressor);
    f.project.session.tracks.extend([key, sink]);
    let external = render(&f.project);
    let stem = Project {
        session: f
            .project
            .session
            .isolated_track(&f.project.session.tracks[0].id)
            .unwrap(),
        media: f.project.media.clone(),
    };
    compare(&render(&stem), &external);
    f.project.session.tracks[0].solo = true;
    compare(&render(&f.project), &external);
    f.project.session.tracks[0].solo = false;
    let mut limiter = insert("limiter", 4);
    limiter.enabled = false;
    f.project.session.tracks[0]
        .realtime_effects
        .insert(0, limiter);
    compare(&render(&f.project), &external);
    f.project.session.tracks[1].muted = true;
    let unkeyed = render(&f.project);
    let power = |x: &[f32]| x[12000..].iter().map(|v| v * v).sum::<f32>();
    assert!(power(&external[0]) < power(&unkeyed[0]) * 0.1);
}
#[test]
fn professional_inserts_are_finite_partition_invariant_and_allocation_free() {
    for kind in ["gate", "utility", "equalizer", "guitarRig"] {
        let mut f = Fixture::new();
        let mut e = insert(kind, 5);
        if kind == "utility" {
            e.parameters.insert("delayL".into(), 317.);
            e.parameters.insert("delayR".into(), 4800.);
            e.parameters.insert("width".into(), 1.5);
        }
        if kind == "equalizer" {
            e.parameters.insert("band4Gain".into(), 5.);
            e.parameters.insert("band5Range".into(), -12.);
            e.parameters.insert("band5Threshold".into(), -40.);
        }
        if kind == "guitarRig" {
            e.parameter_automation = serde_json::from_value(serde_json::json!({
                "ampDrive":[{"frame":0,"value":0.2},{"frame":12000,"value":0.9}],
                "mic":[{"frame":0,"value":0.1},{"frame":24000,"value":0.9}],
                "cabinet":[{"frame":0,"value":0.25},{"frame":18000,"value":1.0}]
            }))
            .unwrap();
        }
        f.project.session.tracks[0].realtime_effects.push(e);
        let expected = render(&f.project);
        let mut graph =
            PreparedGraph::prepare_resident(&f.project, 4 * 1024 * 1024, &CANCEL).unwrap();
        compare(&callback(&mut graph, FRAMES, 127), &expected);
    }
}

#[test]
fn send_live_edits_preserve_graph_and_stem_retains_key_dependencies() {
    use soundroom_engine::render::LivePatch;
    let mut f = Fixture::new();
    let ret = bus(31, "return");
    f.project.session.version = 10;
    let r = f.project.session.tracks[0].routing.insert(route("audio"));
    r.sends.push(soundroom_engine::routing::TrackSend {
        track_id: ret.id.clone(),
        level: 0.2,
    });
    f.project.session.tracks.push(ret);
    let mut graph = PreparedGraph::prepare_resident(&f.project, 4 * 1024 * 1024, &CANCEL).unwrap();
    callback(&mut graph, 3000, 257);
    let mut next = f.project.session.clone();
    next.tracks[0].routing.as_mut().unwrap().sends[0].level = 0.6;
    let mut patch =
        LivePatch::between(&f.project.session, &next).expect("send level is a live edit");
    ALLOCATIONS.with(|v| v.set(0));
    RETIREMENTS.with(|v| v.set(0));
    PROBE.with(|v| v.set(true));
    assert!(graph.apply_live(&mut patch));
    PROBE.with(|v| v.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert_eq!(RETIREMENTS.with(Cell::get), 0);
    let actual = callback(&mut graph, 4000, 127);
    f.project.session = next;
    let expected = render(&f.project);
    compare(
        &actual.clone().map(|v| v[480..].to_vec()),
        &expected.map(|v| v[3480..7000].to_vec()),
    );
    let stem = Project {
        session: f
            .project
            .session
            .isolated_track(&f.project.session.tracks[0].id)
            .unwrap(),
        media: f.project.media.clone(),
    };
    let mut dry = Fixture::new();
    dry.project.session.tracks[0].routing = None;
    compare(&render(&stem), &render(&dry.project));
}

#[test]
fn bypassed_utility_keeps_its_delay_filled_with_current_audio() {
    use soundroom_engine::render::LivePatch;
    let mut f = Fixture::new();
    f.project.session.version = 10;
    let mut effect = insert("utility", 51);
    effect.parameters.insert("delayL".into(), 4800.);
    effect.parameters.insert("delayR".into(), 4800.);
    f.project.session.tracks[0].realtime_effects.push(effect);
    let expected = render(&f.project);
    let mut graph = PreparedGraph::prepare_resident(&f.project, 4 * 1024 * 1024, &CANCEL).unwrap();
    callback(&mut graph, 6000, 257);
    let mut disabled = f.project.session.clone();
    disabled.tracks[0].realtime_effects[0].enabled = false;
    graph.apply_live(&mut LivePatch::between(&f.project.session, &disabled).unwrap());
    callback(&mut graph, 6000, 257);
    graph.apply_live(&mut LivePatch::between(&disabled, &f.project.session).unwrap());
    let actual = callback(&mut graph, 3000, 127);
    compare(
        &actual.map(|v| v[480..].to_vec()),
        &expected.map(|v| v[12480..15000].to_vec()),
    );
}

#[test]
fn utility_export_tail_preserves_exact_integer_sample_delays() {
    let mut f = Fixture::new();
    for frames in [1, 7, 13, 14, 26, 53, 317, 4800] {
        let mut effect = insert("utility", 61);
        effect.parameters.insert("delayL".into(), frames as f32);
        f.project.session.tracks[0].realtime_effects = vec![effect];
        assert_eq!(Renderer::new(&f.project).unwrap().tail_frames(), frames);
    }
}

#[test]
fn guitar_rig_callback_timing_fixture() {
    for (name, amp, pedal, modulation) in [
        ("clean", 1., 0., 0.),
        ("high-gain", 4., 1., 0.),
        ("chorus-delay-room", 2., 1., 1.),
    ] {
        let mut fixture = Fixture::new();
        let mut effect = insert("guitarRig", 71);
        effect.parameters.insert("amp".into(), amp);
        effect.parameters.insert("pedal".into(), pedal);
        effect.parameters.insert("modulation".into(), modulation);
        if modulation > 0. {
            effect.parameters.insert("delayMix".into(), 0.3);
        }
        fixture.project.session.tracks[0].realtime_effects = vec![effect];
        let mut graph = PreparedGraph::prepare_resident_loop(
            &fixture.project,
            32 * 1024 * 1024,
            &CANCEL,
            LoopRange::new(0, FRAMES as u64).unwrap(),
        )
        .unwrap();
        let (mut left, mut right) = ([0.; 128], [0.; 128]);
        for _ in 0..100 {
            graph.render_callback(&mut left, &mut right);
        }
        let mut timings = Vec::with_capacity(2000);
        for _ in 0..2000 {
            let start = std::time::Instant::now();
            assert_eq!(
                graph.render_callback(&mut left, &mut right),
                CallbackStatus::Rendered
            );
            timings.push(start.elapsed().as_secs_f64() * 1000.);
            assert!(left.iter().chain(&right).all(|v| v.is_finite()));
        }
        timings.sort_by(f64::total_cmp);
        println!("Guitar rig {name}, stereo 128-frame callbacks: median={:.4}ms p95={:.4}ms max={:.4}ms; block budget=2.667ms", timings[1000], timings[1900], timings[1999]);
    }
}
