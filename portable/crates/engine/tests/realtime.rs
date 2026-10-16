//! Prepared graph qualification. Allocation probes exclude worker construction,
//! checkpoint copies and file reads, and include every output sample rendered.
use soundroom_engine::{
    project::Project,
    render::{CallbackStatus, CheckpointCache, PreparedGraph},
    wave::WaveReader,
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    path::PathBuf,
    sync::atomic::AtomicBool,
};
thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static RETIREMENTS: Cell<usize> = const { Cell::new(0) };
    static REQUESTED_BYTES: Cell<usize> = const { Cell::new(0) };
}
struct Probe;
unsafe impl GlobalAlloc for Probe {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if TRACK.try_with(Cell::get).unwrap_or(false) {
            let _ = ALLOCATIONS.try_with(|v| v.set(v.get() + 1));
            let _ = REQUESTED_BYTES.try_with(|v| v.set(v.get() + layout.size()));
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if TRACK.try_with(Cell::get).unwrap_or(false) {
            let _ = RETIREMENTS.try_with(|v| v.set(v.get() + 1));
        }
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Probe = Probe;
const NAMES: [&str; 46] = [
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
fn fixture(name: &str, extension: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(format!("{name}{extension}"))
}
fn prepare(name: &str) -> PreparedGraph {
    PreparedGraph::prepare_resident(
        &Project::open(&fixture(name, ".soundroom")).unwrap(),
        16 * 1024 * 1024,
        &AtomicBool::new(false),
    )
    .unwrap()
}
fn render_no_alloc(graph: &mut PreparedGraph, count: usize, block: usize) -> [Vec<f32>; 2] {
    let mut audio = [vec![0.; count], vec![0.; count]];
    let [left, right] = &mut audio;
    ALLOCATIONS.with(|v| v.set(0));
    RETIREMENTS.with(|v| v.set(0));
    let mut status = CallbackStatus::Rendered;
    TRACK.with(|v| v.set(true));
    for (l, r) in left.chunks_mut(block).zip(right.chunks_mut(block)) {
        let result = graph.render_callback(l, r);
        if result != CallbackStatus::Rendered {
            status = result;
        }
    }
    TRACK.with(|v| v.set(false));
    assert_eq!(status, CallbackStatus::Rendered);
    assert_eq!(ALLOCATIONS.with(Cell::get), 0, "callback allocated");
    assert_eq!(
        RETIREMENTS.with(Cell::get),
        0,
        "callback retired heap storage"
    );
    audio
}
fn compare(name: &str, start: u64, audio: &[Vec<f32>; 2]) {
    let mut reference = WaveReader::open(&fixture(name, "-swift.wav")).unwrap();
    let mut error = 0_f32;
    for index in (0..audio[0].len()).step_by(8192) {
        let n = (audio[0].len() - index).min(8192);
        let mut left = vec![0.; n];
        let mut right = left.clone();
        reference
            .read_stereo(start + index as u64, &mut left, &mut right)
            .unwrap();
        for (a, b) in audio[0][index..index + n]
            .iter()
            .zip(&left)
            .chain(audio[1][index..index + n].iter().zip(&right))
        {
            error = error.max((a - b).abs());
        }
    }
    println!("Prepared Swift parity {name} at {start}: maximum absolute error {error}");
    assert!(error < 0.000005, "{name}: {error}");
}
#[test]
fn all_dsp_callback_paths_match_swift_and_allocate_or_retire_nothing() {
    for name in NAMES {
        let graph = prepare(name);
        let reference = WaveReader::open(&fixture(name, "-swift.wav")).unwrap();
        for block in [1, 128, 257, 8192] {
            // Fork is a worker operation. Tests ensure a cloned empty scheduler
            // retains its reserved capacity when future clips become active.
            let mut fork = graph.fork();
            let audio = render_no_alloc(&mut fork, reference.info.frames as usize, block);
            compare(name, 0, &audio);
        }
    }
}
#[test]
fn canonical_worker_checkpoints_restore_latency_and_automation_exactly() {
    for name in [
        "envelopes",
        "warmth-transition",
        "limiter-track",
        "limiter-master",
        "resonance-track",
        "resonance-master",
        "latency-chain",
        "room-transition",
        "pitch-edits",
        "pitch-automation",
        "pitch-down",
        "pitch-formant",
        "pitch-master",
        "pitch-chain",
    ] {
        let mut worker = prepare(name);
        let mut output = worker.fork();
        let mut cache = CheckpointCache::new(worker.checkpoint_storage_budget() * 3);
        cache.store(&worker).unwrap();
        let cancel = AtomicBool::new(false);
        worker.seek(5999, &cache, &cancel).unwrap();
        cache.store(&worker).unwrap();
        worker.seek(10000, &cache, &cancel).unwrap();
        cache.store(&worker).unwrap();
        for position in [12345, 7001, 6000, 0] {
            let warmed = output.seek(position, &cache, &cancel).unwrap();
            assert!(warmed < 6000);
            compare(name, position, &render_no_alloc(&mut output, 257, 257));
        }
        let checkpoint = worker.checkpoint();
        assert!(prepare(name).restore_checkpoint(&checkpoint).is_err());
        assert!(cache.storage_budget() <= worker.checkpoint_storage_budget() * 3);
    }
}
#[test]
fn admission_cancellation_invalid_blocks_and_checkpoint_eviction_are_bounded() {
    let p = Project::open(&fixture("envelopes", ".soundroom")).unwrap();
    assert!(PreparedGraph::prepare_resident(&p, 1, &AtomicBool::new(false)).is_err());
    assert!(PreparedGraph::prepare_resident(&p, 1024 * 1024, &AtomicBool::new(true)).is_err());
    let mut graph = prepare("envelopes");
    let mut left = [1.; 8];
    let mut right = [1.; 7];
    assert_eq!(
        graph.render_callback(&mut left, &mut right),
        CallbackStatus::InvalidBlock
    );
    assert_eq!(left, [0.; 8]);
    assert_eq!(right, [0.; 7]);
    assert_eq!(graph.position(), 0);
    let budget = graph.checkpoint_storage_budget();
    assert!(CheckpointCache::new(budget - 1).store(&graph).is_err());
    let mut cache = CheckpointCache::new(budget);
    cache.store(&graph).unwrap();
    cache.store(&graph).unwrap();
    assert_eq!(cache.len(), 1);
    graph.seek(100, &cache, &AtomicBool::new(false)).unwrap();
    cache.store(&graph).unwrap();
    assert_eq!(cache.len(), 1);
    assert!(graph.seek(0, &cache, &AtomicBool::new(false)).is_err());
    assert!(graph.seek(10000, &cache, &AtomicBool::new(true)).is_err());
}
#[test]
fn callback_handoff_backpressure_transport_fades_and_worker_retirement() {
    use soundroom_engine::realtime::channel;
    let (mut control, mut host) = channel();
    let mut left = [0.; 128];
    let mut right = left;
    let play = control.request_playing(true).unwrap();
    host.render(&mut left, &mut right);
    assert_eq!(control.telemetry().unwrap().command_sequence, 0);
    assert_eq!(control.telemetry().unwrap().frame, 0);
    let first = prepare("pitch-chain");
    let first_id = first.generation();
    control.publish(Box::new(first)).ok().unwrap();
    let second = prepare("room-transition");
    let second_id = second.generation();
    assert!(control.publish(Box::new(second.fork())).is_err());
    ALLOCATIONS.with(|v| v.set(0));
    RETIREMENTS.with(|v| v.set(0));
    TRACK.with(|v| v.set(true));
    for _ in 0..8 {
        host.render(&mut left, &mut right);
    }
    TRACK.with(|v| v.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert_eq!(RETIREMENTS.with(Cell::get), 0);
    let state = control.telemetry().unwrap();
    assert_eq!(state.frame, 1024);
    assert_eq!(state.command_sequence, play);
    assert_eq!(state.graph_generation, first_id);
    assert!(state.playing);
    control.publish(Box::new(second)).ok().unwrap();
    TRACK.with(|v| v.set(true));
    for _ in 0..8 {
        host.render(&mut left, &mut right);
    }
    TRACK.with(|v| v.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert_eq!(RETIREMENTS.with(Cell::get), 0);
    assert_eq!(control.telemetry().unwrap().graph_generation, second_id);
    let third = prepare("limiter-track");
    let third_id = third.generation();
    control.publish(Box::new(third)).ok().unwrap();
    host.render(&mut left, &mut right);
    // The second graph cannot hand off while the first is still awaiting retirement.
    assert_eq!(control.telemetry().unwrap().graph_generation, second_id);
    assert!(control.reclaim_retired());
    assert!(!control.reclaim_retired());
    host.render(&mut left, &mut right);
    assert_eq!(control.telemetry().unwrap().graph_generation, third_id);
    let pause = control.request_playing(false).unwrap();
    host.render(&mut left, &mut right);
    assert!(control.telemetry().unwrap().playing);
    host.render(&mut left, &mut right);
    let state = control.telemetry().unwrap();
    assert!(!state.playing);
    assert_eq!(state.command_sequence, pause);
    let stopped_frame = state.frame;
    host.render(&mut left, &mut right);
    assert_eq!(left, [0.; 128]);
    assert_eq!(right, [0.; 128]);
    assert_eq!(control.telemetry().unwrap().frame, stopped_frame);
    assert_eq!(control.telemetry().unwrap().faults, 0);
    // Lifecycle destruction happens after the callback is stopped.
    drop(host);
    control.reclaim_retired();
}
#[test]
fn same_generation_seek_requires_its_own_callback_handoff_acknowledgement() {
    use soundroom_engine::realtime::channel;
    let base = prepare("pitch-edits");
    let generation = base.generation();
    let mut sought = base.fork();
    sought
        .seek(7001, &CheckpointCache::new(0), &AtomicBool::new(false))
        .unwrap();
    let (mut control, mut host) = channel();
    let first = control.publish(Box::new(base)).ok().unwrap();
    let mut left = [0.; 128];
    let mut right = left;
    host.render(&mut left, &mut right);
    assert_eq!(control.telemetry().unwrap().handoff_token, first);
    let second = control.publish(Box::new(sought)).ok().unwrap();
    assert_ne!(first, second);
    // Publishing on a worker is not callback acceptance, even with the same DSP key.
    assert_eq!(control.telemetry().unwrap().handoff_token, first);
    host.render(&mut left, &mut right);
    let accepted = control.telemetry().unwrap();
    assert_eq!(accepted.handoff_token, second);
    assert_eq!(accepted.graph_generation, generation);
    assert_eq!(accepted.frame, 7001);
    assert!(!accepted.playing);
}
#[test]
fn future_live_edit_splices_at_exact_sample_and_late_or_cancelled_edits_do_not_jump() {
    use soundroom_engine::realtime::channel;
    let p = Project::open(&fixture("legacy-gain", ".soundroom")).unwrap();
    let base = prepare("legacy-gain");
    let mut changed = p;
    changed.session.master_track.gain *= 0.3;
    let mut candidate =
        PreparedGraph::prepare_resident(&changed, 16 * 1024 * 1024, &AtomicBool::new(false))
            .unwrap();
    candidate
        .seek(1000, &CheckpointCache::new(0), &AtomicBool::new(false))
        .unwrap();
    let (mut control, mut host) = channel();
    control.publish(Box::new(base)).ok().unwrap();
    control.request_playing(true).unwrap();
    let mut prefix_l = [0.; 512];
    let mut prefix_r = prefix_l;
    host.render(&mut prefix_l, &mut prefix_r);
    let old_token = control.telemetry().unwrap().handoff_token;
    let new_token = control
        .publish_aligned(Box::new(candidate), 1000)
        .ok()
        .unwrap();
    assert_eq!(control.telemetry().unwrap().handoff_token, old_token);
    let mut left = [0.; 1024];
    let mut right = left;
    ALLOCATIONS.with(|v| v.set(0));
    RETIREMENTS.with(|v| v.set(0));
    TRACK.with(|v| v.set(true));
    assert_eq!(host.render(&mut left, &mut right), CallbackStatus::Rendered);
    TRACK.with(|v| v.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert_eq!(RETIREMENTS.with(Cell::get), 0);
    let telemetry = control.telemetry().unwrap();
    assert_eq!(telemetry.frame, 1536);
    assert_eq!(telemetry.handoff_token, new_token);
    assert_eq!(telemetry.callback_count, 2);
    let mut reference = WaveReader::open(&fixture("legacy-gain", "-swift.wav")).unwrap();
    let mut expected_l = [0.; 1024];
    let mut expected_r = expected_l;
    reference
        .read_stereo(512, &mut expected_l, &mut expected_r)
        .unwrap();
    for i in 0..1024 {
        let mix = if i < 488 {
            0.
        } else {
            ((i - 488 + 1) as f32 / 256.).min(1.)
        };
        let scale = 1. - mix + 0.3 * mix;
        assert!(
            (left[i] - expected_l[i] * scale).abs() < 0.000005,
            "left {i}"
        );
        assert!(
            (right[i] - expected_r[i] * scale).abs() < 0.000005,
            "right {i}"
        );
    }
    assert!(control.reclaim_retired());
    let mut late = prepare("legacy-gain");
    late.seek(1400, &CheckpointCache::new(0), &AtomicBool::new(false))
        .unwrap();
    let late_token = control.publish_aligned(Box::new(late), 1400).ok().unwrap();
    TRACK.with(|v| v.set(true));
    host.render(&mut left[..128], &mut right[..128]);
    TRACK.with(|v| v.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert_eq!(RETIREMENTS.with(Cell::get), 0);
    let telemetry = control.telemetry().unwrap();
    assert_eq!(telemetry.frame, 1664);
    assert_eq!(telemetry.handoff_token, new_token);
    assert_eq!(telemetry.rejected_handoff, late_token);
    assert_eq!(telemetry.rejection_count, 1);
    assert!(control.reclaim_retired());
    let mut future = prepare("legacy-gain");
    future
        .seek(3000, &CheckpointCache::new(0), &AtomicBool::new(false))
        .unwrap();
    let cancelled = control
        .publish_aligned(Box::new(future), 3000)
        .ok()
        .unwrap();
    host.render(&mut left[..128], &mut right[..128]);
    control.cancel_handoff(cancelled);
    host.render(&mut left[..128], &mut right[..128]);
    let telemetry = control.telemetry().unwrap();
    assert_eq!(telemetry.rejected_handoff, cancelled);
    assert_eq!(telemetry.rejection_count, 2);
    assert_eq!(telemetry.frame, 1920);
    assert_eq!(telemetry.handoff_token, new_token);
    assert!(control.reclaim_retired());
    assert!(control
        .publish_aligned(Box::new(prepare("legacy-gain")), 1)
        .is_err());
}
#[derive(serde::Deserialize)]
struct LoopCase {
    project: String,
    start: u64,
    end: u64,
    frames: usize,
    reference: String,
}
#[test]
fn continuous_loop_dsp_source_wrap_latency_and_automation_match_swift_without_allocations() {
    use soundroom_engine::render::LoopRange;
    let cases: Vec<LoopCase> =
        serde_json::from_slice(&std::fs::read(fixture("loop-cases", ".json")).unwrap()).unwrap();
    assert_eq!(cases.len(), 32);
    for case in cases {
        let project = Project::open(&fixture(&case.project, ".soundroom")).unwrap();
        let graph = PreparedGraph::prepare_resident_loop(
            &project,
            16 * 1024 * 1024,
            &AtomicBool::new(false),
            LoopRange::new(case.start, case.end).unwrap(),
        )
        .unwrap();
        for block in [1, 128, 257, 8192] {
            let mut fork = graph.fork();
            let audio = render_no_alloc(&mut fork, case.frames, block);
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures")
                .join(&case.reference);
            let mut reader = WaveReader::open(&path).unwrap();
            let mut error = 0_f32;
            for i in (0..case.frames).step_by(8192) {
                let n = (case.frames - i).min(8192);
                let mut l = vec![0.; n];
                let mut r = l.clone();
                reader.read_stereo(i as u64, &mut l, &mut r).unwrap();
                for (a, b) in audio[0][i..i + n]
                    .iter()
                    .zip(&l)
                    .chain(audio[1][i..i + n].iter().zip(&r))
                {
                    error = error.max((a - b).abs());
                }
            }
            println!(
                "Swift loop parity {} block{block}: maximum absolute error{error}",
                case.reference
            );
            assert!(error < 0.000005, "{} block{block}: {error}", case.reference);
            assert_eq!(
                fork.position(),
                case.start + case.frames as u64 % (case.end - case.start)
            );
            assert!(CheckpointCache::new(fork.checkpoint_storage_budget())
                .store(&fork)
                .is_err());
        }
        let mut fork = graph.fork();
        assert!(fork
            .seek(case.end, &CheckpointCache::new(0), &AtomicBool::new(false))
            .is_err());
        assert!(fork
            .seek(
                case.start - 1,
                &CheckpointCache::new(0),
                &AtomicBool::new(false)
            )
            .is_err());
    }
}
#[test]
fn bounded_paged_callback_graphs_match_all_swift_samples_without_file_reads_or_heap_work() {
    let cancel = AtomicBool::new(false);
    for name in NAMES {
        let project = Project::open(&fixture(name, ".soundroom")).unwrap();
        let (base, mut loader) =
            PreparedGraph::prepare_paged(&project, 512 * 1024, &cancel, None).unwrap();
        let reference = WaveReader::open(&fixture(name, "-swift.wav")).unwrap();
        assert_eq!(loader.stats().pcm_bytes, 512 * 1024);
        for block in [1, 128, 257, 8192] {
            let mut graph = base.fork();
            let before = loader.stats().worker_file_reads;
            compare(
                name,
                0,
                &render_no_alloc(&mut graph, reference.info.frames as usize, block),
            );
            assert_eq!(
                loader.stats().worker_file_reads,
                before,
                "callback read a file"
            );
        }
        let mut worker = base.fork();
        let before = loader.stats().worker_file_reads;
        worker
            .seek(12345, &CheckpointCache::new(0), &cancel)
            .unwrap();
        assert!(
            loader.stats().worker_file_reads > before,
            "worker seek must use bounded decoding"
        );
        loader.prepare_graph(&worker, 8192, &cancel).unwrap();
        let before = loader.stats().worker_file_reads;
        compare(name, 12345, &render_no_alloc(&mut worker, 257, 257));
        assert_eq!(loader.stats().worker_file_reads, before);
    }
}
#[test]
fn paged_loops_use_continuous_histories_and_never_decode_on_callback() {
    use soundroom_engine::render::LoopRange;
    let cases: Vec<LoopCase> =
        serde_json::from_slice(&std::fs::read(fixture("loop-cases", ".json")).unwrap()).unwrap();
    for case in cases {
        let project = Project::open(&fixture(&case.project, ".soundroom")).unwrap();
        let (mut graph, loader) = PreparedGraph::prepare_paged(
            &project,
            512 * 1024,
            &AtomicBool::new(false),
            Some(LoopRange::new(case.start, case.end).unwrap()),
        )
        .unwrap();
        let reads = loader.stats().worker_file_reads;
        let audio = render_no_alloc(&mut graph, case.frames, 257);
        assert_eq!(loader.stats().worker_file_reads, reads);
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(&case.reference);
        let mut reader = WaveReader::open(&path).unwrap();
        let mut error = 0_f32;
        for i in (0..case.frames).step_by(8192) {
            let n = (case.frames - i).min(8192);
            let mut l = vec![0.; n];
            let mut r = l.clone();
            reader.read_stereo(i as u64, &mut l, &mut r).unwrap();
            for (a, b) in audio[0][i..i + n]
                .iter()
                .zip(&l)
                .chain(audio[1][i..i + n].iter().zip(&r))
            {
                error = error.max((a - b).abs());
            }
        }
        assert!(error < 0.000005, "{}:{error}", case.reference);
    }
}
#[test]
fn very_long_sparse_source_streams_bounded_pages_and_reports_underruns_without_freezing_clock() {
    use soundroom_engine::{render::Renderer, wave::WaveWriter};
    use std::{
        fs::OpenOptions,
        io::{Seek, SeekFrom, Write},
    };
    let path =
        std::env::temp_dir().join(format!("soundroom-paged-long-{}.wav", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let frames = 200_000_000_u64; //1.6GB declared source; sparse storage, no large PCM allocation.
    let mut writer = WaveWriter::create(&path, frames).unwrap();
    let left = [0.17; 8192];
    let right = [-0.13; 8192];
    for _ in 0..16 {
        writer.write(&left, &right).unwrap();
    }
    drop(writer);
    let mut file = OpenOptions::new().write(true).open(&path).unwrap();
    file.set_len(44 + frames * 8).unwrap();
    file.seek(SeekFrom::Start(44 + 500000 * 8)).unwrap();
    let mut stereo = vec![0_u8; 8192 * 8];
    for frame in stereo.chunks_exact_mut(8) {
        frame[..4].copy_from_slice(&0.17_f32.to_le_bytes());
        frame[4..].copy_from_slice(&(-0.13_f32).to_le_bytes());
    }
    file.write_all(&stereo).unwrap();
    drop(file);
    let mut project = Project::open(&fixture("modern-gain", ".soundroom")).unwrap();
    let id = project.session.tracks[0].clips[0].asset_id.clone();
    let media = project.media.get_mut(&id.to_uppercase()).unwrap();
    media.path = path.clone();
    media.info = WaveReader::open(&path).unwrap().info;
    project.session.tracks[0].clips[0].length = frames;
    assert!(
        PreparedGraph::prepare_resident(&project, 16 * 1024 * 1024, &AtomicBool::new(false))
            .is_err()
    );
    let cancel = AtomicBool::new(false);
    let (mut graph, mut loader) =
        PreparedGraph::prepare_paged(&project, 512 * 1024, &cancel, None).unwrap();
    assert_eq!(graph.source_storage_bytes(), 512 * 1024);
    let canonical = graph.fork();
    let checkpoint = graph.checkpoint();
    let mut expected = Renderer::new(&project).unwrap();
    let mut l = [0.; 257];
    let mut r = l;
    let mut el = l;
    let mut er = l;
    for _ in 0..500 {
        ALLOCATIONS.with(|v| v.set(0));
        RETIREMENTS.with(|v| v.set(0));
        TRACK.with(|v| v.set(true));
        let status = graph.render_callback(&mut l, &mut r);
        TRACK.with(|v| v.set(false));
        assert_eq!(status, CallbackStatus::Rendered);
        assert_eq!(ALLOCATIONS.with(Cell::get), 0);
        assert_eq!(RETIREMENTS.with(Cell::get), 0);
        expected.render(&mut el, &mut er).unwrap();
        assert_eq!(l, el);
        assert_eq!(r, er);
        loader.service_requests(64, &cancel).unwrap();
    }
    assert_eq!(graph.position(), 128500);
    let mut underruns = 0;
    for _ in 0..1000 {
        let status = graph.render_callback(&mut l, &mut r);
        if status == CallbackStatus::SourceUnderrun {
            underruns += 1;
        }
    }
    assert!(underruns > 0);
    assert_eq!(graph.position(), 385500);
    // Canonical worker seek uses decoder I/O; publish only after warming the
    // target window, rather than confusing missing cache data with real silence.
    assert!(CheckpointCache::new(graph.checkpoint_storage_budget())
        .store(&graph)
        .is_err());
    assert!(canonical
        .fork()
        .restore_checkpoint(&graph.checkpoint())
        .is_err());
    assert!(graph
        .seek(500000, &CheckpointCache::new(0), &cancel)
        .is_err());
    graph.restore_checkpoint(&checkpoint).unwrap();
    graph
        .seek(500000, &CheckpointCache::new(0), &cancel)
        .unwrap();
    loader.prepare_graph(&graph, 8192, &cancel).unwrap();
    let reads = loader.stats().worker_file_reads;
    assert_eq!(
        graph.render_callback(&mut l, &mut r),
        CallbackStatus::Rendered
    );
    assert_eq!(loader.stats().worker_file_reads, reads);
    assert!(l.iter().any(|v| v.abs() > 0.001));
    assert!(loader.service_requests(1025, &cancel).is_err());
    assert!(loader.service_requests(1, &AtomicBool::new(true)).is_err());
    // External source loss during worker replay must poison partial DSP state,
    // rather than accidentally admitting it as a successful canonical checkpoint.
    OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(44 + 4096 * 8)
        .unwrap();
    assert!(graph
        .seek(600000, &CheckpointCache::new(0), &cancel)
        .is_err());
    let mut rejected_cache = CheckpointCache::new(graph.checkpoint_storage_budget());
    assert!(rejected_cache.store(&graph).is_err());
    assert!(rejected_cache.store_playback_clock(&graph).is_err());
    assert!(!graph.has_clean_history());
    assert!(soundroom_engine::realtime::channel()
        .0
        .publish(Box::new(graph.fork()))
        .is_err());
    drop(expected);
    drop(graph);
    drop(loader);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn pause_receipt_waits_for_settled_64_frame_fade_and_handles_reversal() {
    use soundroom_engine::realtime::channel;
    let (mut control, mut host) = channel();
    control
        .publish(Box::new(prepare("legacy-gain")))
        .ok()
        .unwrap();
    let play = control.request_playing(true).unwrap();
    let mut left = [0.; 64];
    let mut right = left;
    for _ in 0..4 {
        host.render(&mut left, &mut right);
    }
    let initial_frame = control.telemetry().unwrap().frame;
    control.request_playing(false).unwrap();
    host.render(&mut left, &mut right);
    let fading = control.telemetry().unwrap();
    assert!(fading.playing);
    assert_eq!(fading.command_sequence, play);
    assert_eq!(fading.frame, initial_frame + 64);
    // Reversing a still-in-flight fade acknowledges only the newest Play.
    let resume = control.request_playing(true).unwrap();
    host.render(&mut left, &mut right);
    assert_eq!(control.telemetry().unwrap().command_sequence, resume);
    let pause = control.request_playing(false).unwrap();
    for i in 0..4 {
        host.render(&mut left, &mut right);
        let state = control.telemetry().unwrap();
        assert_eq!(state.playing, i < 3);
        assert_eq!(state.command_sequence, if i < 3 { resume } else { pause });
    }
    let settled = control.telemetry().unwrap().frame;
    for _ in 0..4 {
        host.render(&mut left, &mut right);
        assert_eq!(control.telemetry().unwrap().frame, settled);
        assert_eq!(left, [0.; 64]);
        assert_eq!(right, [0.; 64]);
    }
}

#[test]
fn bounded_capture_queue_reports_gaps_and_preserves_raw_input_without_heap_work() {
    use soundroom_engine::capture::{capture_channel, CaptureConfig, CaptureStatus, CaptureTiming};
    let (mut writer, mut reader) = capture_channel(CaptureConfig {
        channels: 2,
        maximum_frames: 64,
        blocks: 2,
        sample_rate: 48000,
    })
    .unwrap();
    let mut left = [0.1; 64];
    let right = [-0.2; 64];
    left[2] = f32::NAN;
    let timing = CaptureTiming {
        sample_time: 45.,
        host_time: 999,
        flags: 3,
    };
    ALLOCATIONS.with(|v| v.set(0));
    RETIREMENTS.with(|v| v.set(0));
    TRACK.with(|v| v.set(true));
    assert_eq!(
        writer.push(&[&left, &right], timing),
        CaptureStatus::Captured
    );
    assert_eq!(
        writer.push(&[&left, &right], timing),
        CaptureStatus::Captured
    );
    assert_eq!(
        writer.push(&[&left, &right], timing),
        CaptureStatus::Overflow
    );
    assert_eq!(
        writer.push(&[&left[..10], &right], timing),
        CaptureStatus::InvalidBlock
    );
    TRACK.with(|v| v.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert_eq!(RETIREMENTS.with(Cell::get), 0);
    let mut l = [0.; 64];
    let mut r = l;
    let first = reader.read_planar(&mut [&mut l, &mut r]).unwrap().unwrap();
    assert_eq!(first.source_frame, 0);
    assert_eq!(first.timing, timing);
    assert_eq!(l[2].to_bits(), left[2].to_bits());
    assert_eq!(
        reader
            .read_planar(&mut [&mut l, &mut r])
            .unwrap()
            .unwrap()
            .source_frame,
        64
    );
    writer.push(&[&left, &right], timing);
    assert_eq!(
        reader
            .read_planar(&mut [&mut l, &mut r])
            .unwrap()
            .unwrap()
            .source_frame,
        192
    );
    assert!(reader.read_planar(&mut [&mut l, &mut r]).unwrap().is_none());
    let stats = reader.stats();
    assert_eq!(stats.received_frames, 256);
    assert_eq!(stats.dropped_frames, 64);
    assert_eq!(stats.invalid_samples, 3);
    assert!(capture_channel(CaptureConfig {
        channels: 64,
        maximum_frames: 8192,
        blocks: 32,
        sample_rate: 48000
    })
    .is_err());
    assert!(capture_channel(CaptureConfig {
        channels: 2,
        maximum_frames: 64,
        blocks: 3,
        sample_rate: 48000
    })
    .is_err());
}

#[test]
fn concurrent_capture_preserves_packet_pcm_and_source_gaps_under_queue_pressure() {
    use soundroom_engine::capture::{capture_channel, CaptureConfig, CaptureTiming};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let (mut writer, mut reader) = capture_channel(CaptureConfig {
        channels: 2,
        maximum_frames: 64,
        blocks: 8,
        sample_rate: 48000,
    })
    .unwrap();
    let done = Arc::new(AtomicBool::new(false));
    let signal = done.clone();
    let producer = std::thread::spawn(move || {
        let mut left = [0.; 64];
        let mut right = left;
        ALLOCATIONS.with(|v| v.set(0));
        RETIREMENTS.with(|v| v.set(0));
        for block in 0..10_000 {
            let frame = block * 64;
            left.fill(frame as f32);
            right.fill(-(frame as f32));
            TRACK.with(|v| v.set(true));
            writer.push(
                &[&left, &right],
                CaptureTiming {
                    sample_time: frame as f64,
                    host_time: block,
                    flags: 3,
                },
            );
            TRACK.with(|v| v.set(false));
            if block % 17 == 0 {
                std::thread::yield_now();
            }
        }
        let counts = (ALLOCATIONS.with(Cell::get), RETIREMENTS.with(Cell::get));
        signal.store(true, Ordering::Release);
        counts
    });
    let mut left = [0.; 64];
    let mut right = left;
    let mut copied = 0;
    let mut previous = None;
    loop {
        if let Some(block) = reader.read_planar(&mut [&mut left, &mut right]).unwrap() {
            assert!(previous.map_or(true, |frame| block.source_frame > frame));
            previous = Some(block.source_frame);
            assert!(left.iter().all(|&v| v == block.source_frame as f32));
            assert!(right.iter().all(|&v| v == -(block.source_frame as f32)));
            assert_eq!(block.timing.sample_time, block.source_frame as f64);
            copied += block.frames as u64;
        } else if done.load(Ordering::Acquire) {
            break;
        } else {
            std::thread::yield_now();
        }
    }
    assert_eq!(producer.join().unwrap(), (0, 0));
    let stats = reader.stats();
    assert_eq!(stats.received_frames, 640000);
    assert_eq!(copied + stats.dropped_frames, stats.received_frames);
    assert!(copied > 0);
    assert_eq!(stats.pcm_bytes, 2 * 64 * 8 * 4);
}

#[test]
fn continuous_loop_worker_clock_checkpoints_and_paged_seeks_match_swift_history() {
    use soundroom_engine::render::LoopRange;
    let cases: Vec<LoopCase> =
        serde_json::from_slice(&std::fs::read(fixture("loop-cases", ".json")).unwrap()).unwrap();
    let cancel = AtomicBool::new(false);
    for case in cases {
        let project = Project::open(&fixture(&case.project, ".soundroom")).unwrap();
        let range = LoopRange::new(case.start, case.end).unwrap();
        let base = PreparedGraph::prepare_resident_loop(&project, 16 * 1024 * 1024, &cancel, range)
            .unwrap();
        let length = range.end - range.start;
        let mut worker = base.fork();
        let mut cache = CheckpointCache::new(base.checkpoint_storage_budget() * 3);
        cache.store(&worker).unwrap();
        worker
            .seek_playback_clock(range.start + 2 * length + 17, &cache, &cancel)
            .unwrap();
        assert_eq!(worker.position(), range.start + 17);
        assert_eq!(
            worker.checkpoint().playback_clock(),
            range.start + 2 * length + 17
        );
        assert!(cache.store(&worker).is_err());
        cache.store_playback_clock(&worker).unwrap();
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(&case.reference);
        let mut reference = WaveReader::open(&path).unwrap();
        for elapsed in [0, length / 2, length + 31, 3 * length + 9, 4 * length + 11] {
            let clock = range.start + elapsed;
            worker.seek_playback_clock(clock, &cache, &cancel).unwrap();
            assert_eq!(worker.playback_clock(), clock);
            assert_eq!(worker.position(), range.start + elapsed % length);
            let audio = render_no_alloc(&mut worker, 64, 64);
            let mut left = [0.; 64];
            let mut right = left;
            reference
                .read_stereo(elapsed, &mut left, &mut right)
                .unwrap();
            for (a, b) in audio[0].iter().zip(left).chain(audio[1].iter().zip(right)) {
                assert!((*a - b).abs() < 0.000005, "{} clock{clock}", case.reference);
            }
        }
        // Wrapped timeline seeking must ignore continuous-cycle snapshots even
        // when their wrapped position happens to precede the requested frame.
        worker.seek(range.start + 50, &cache, &cancel).unwrap();
        assert_eq!(worker.playback_clock(), range.start + 50);
        let audio = render_no_alloc(&mut worker, 64, 64);
        let mut left = [0.; 64];
        let mut right = left;
        reference.read_stereo(50, &mut left, &mut right).unwrap();
        for (a, b) in audio[0].iter().zip(left).chain(audio[1].iter().zip(right)) {
            assert!((*a - b).abs() < 0.000005);
        }
        let (mut paged, mut loader) =
            PreparedGraph::prepare_paged(&project, 512 * 1024, &cancel, Some(range)).unwrap();
        let mut paged_cache = CheckpointCache::new(paged.checkpoint_storage_budget() * 2);
        paged_cache.store(&paged).unwrap();
        for elapsed in [3 * length + 9, length + 31] {
            paged
                .seek_playback_clock(range.start + elapsed, &paged_cache, &cancel)
                .unwrap();
            loader.prepare_graph(&paged, 8192, &cancel).unwrap();
            let reads = loader.stats().worker_file_reads;
            let audio = render_no_alloc(&mut paged, 64, 64);
            assert_eq!(reads, loader.stats().worker_file_reads);
            reference
                .read_stereo(elapsed, &mut left, &mut right)
                .unwrap();
            for (a, b) in audio[0].iter().zip(left).chain(audio[1].iter().zip(right)) {
                assert!(
                    (*a - b).abs() < 0.000005,
                    "paged {} elapsed{elapsed}",
                    case.reference
                );
            }
        }
        assert!(worker
            .seek_playback_clock(range.start - 1, &cache, &cancel)
            .is_err());
        assert!(worker
            .seek_playback_clock(range.start + length, &cache, &AtomicBool::new(true))
            .is_err());
    }
}

#[test]
fn checked_loop_live_update_splices_on_unwrapped_clock_and_rejects_stale_baseline() {
    use soundroom_engine::{realtime::channel, render::LoopRange};
    let cancel = AtomicBool::new(false);
    for offset in [0, 43] {
        let project = Project::open(&fixture("chorus", ".soundroom")).unwrap();
        let range = LoopRange::new(5990, 6320).unwrap();
        let base = PreparedGraph::prepare_resident_loop(&project, 16 * 1024 * 1024, &cancel, range)
            .unwrap();
        let mut changed = project;
        changed.session.master_track.gain *= 0.3;
        let mut candidate =
            PreparedGraph::prepare_resident_loop(&changed, 16 * 1024 * 1024, &cancel, range)
                .unwrap();
        let target = range.start + 2 * (range.end - range.start) + offset;
        candidate
            .seek_playback_clock(target, &CheckpointCache::new(0), &cancel)
            .unwrap();
        let (mut control, mut host) = channel();
        let original = control.publish(Box::new(base)).ok().unwrap();
        control.request_playing(true).unwrap();
        let mut prefix_l = [0.; 512];
        let mut prefix_r = prefix_l;
        host.render(&mut prefix_l, &mut prefix_r);
        assert_eq!(
            control.telemetry().unwrap().playback_clock,
            range.start + 512
        );
        let updated = control
            .publish_aligned_on(Box::new(candidate.fork()), target, original)
            .ok()
            .unwrap();
        let mut left = [0.; 600];
        let mut right = left;
        ALLOCATIONS.with(|v| v.set(0));
        RETIREMENTS.with(|v| v.set(0));
        TRACK.with(|v| v.set(true));
        assert_eq!(host.render(&mut left, &mut right), CallbackStatus::Rendered);
        TRACK.with(|v| v.set(false));
        assert_eq!(ALLOCATIONS.with(Cell::get), 0);
        assert_eq!(RETIREMENTS.with(Cell::get), 0);
        let state = control.telemetry().unwrap();
        assert_eq!(state.handoff_token, updated);
        assert_eq!(state.playback_clock, range.start + 1112);
        assert_eq!(state.frame, range.start + 1112 % (range.end - range.start));
        let mut reference = WaveReader::open(&fixture("loop-chorus-short", "-swift.wav")).unwrap();
        let mut rl = [0.; 600];
        let mut rr = rl;
        reference.read_stereo(512, &mut rl, &mut rr).unwrap();
        let splice = (target - (range.start + 512)) as usize;
        for i in 0..600 {
            let gain = if i < splice {
                1.
            } else {
                let fraction = ((i - splice + 1) as f32 / 256.).min(1.);
                1. - 0.7 * fraction
            };
            // The output dips through a short fade at each loop wrap.
            let gain = gain * soundroom_engine::realtime::loop_wrap_gain(range, range.start + 512 + i as u64);
            assert!((left[i] - rl[i] * gain).abs() < 0.000005, "left {i}");
            assert!((right[i] - rr[i] * gain).abs() < 0.000005, "right {i}");
        }
        assert!(control.reclaim_retired());
        // A candidate prepared from the old callback baseline must never replace the
        // new graph, even when its future clock and loop range are otherwise valid.
        let future = state.playback_clock + 300;
        candidate
            .seek_playback_clock(future, &CheckpointCache::new(0), &cancel)
            .unwrap();
        let stale = control
            .publish_aligned_on(Box::new(candidate), future, original)
            .ok()
            .unwrap();
        host.render(&mut prefix_l, &mut prefix_r);
        let state = control.telemetry().unwrap();
        assert_eq!(state.rejected_handoff, stale);
        assert_eq!(state.handoff_token, updated);
        assert_eq!(state.playback_clock, range.start + 1624);
        assert!(control.reclaim_retired());
        // A loop range change requires an explicit new run, never an aligned edit
        // that silently changes the current source/automation domain.
        let other_range = LoopRange::new(range.start, range.end + 10).unwrap();
        let mut wrong_range =
            PreparedGraph::prepare_resident_loop(&changed, 16 * 1024 * 1024, &cancel, other_range)
                .unwrap();
        let future = state.playback_clock + 300;
        wrong_range
            .seek_playback_clock(future, &CheckpointCache::new(0), &cancel)
            .unwrap();
        let rejected = control
            .publish_aligned_on(Box::new(wrong_range), future, updated)
            .ok()
            .unwrap();
        host.render(&mut prefix_l, &mut prefix_r);
        assert_eq!(control.telemetry().unwrap().rejected_handoff, rejected);
        assert_eq!(control.telemetry().unwrap().handoff_token, updated);
        assert!(control.reclaim_retired());
        let mut next =
            PreparedGraph::prepare_resident_loop(&changed, 16 * 1024 * 1024, &cancel, range)
                .unwrap();
        let future = control.telemetry().unwrap().playback_clock + 300;
        next.seek_playback_clock(future, &CheckpointCache::new(0), &cancel)
            .unwrap();
        let cancelled = control
            .publish_aligned_on(Box::new(next), future, updated)
            .ok()
            .unwrap();
        control.cancel_handoff(cancelled);
        host.render(&mut prefix_l, &mut prefix_r);
        assert_eq!(control.telemetry().unwrap().rejected_handoff, cancelled);
        assert_eq!(control.telemetry().unwrap().handoff_token, updated);
        assert_eq!(control.telemetry().unwrap().rejection_count, 3);
        assert!(control.reclaim_retired());
    }
}

#[test]
fn actual_output_timestamp_anchors_follow_continuous_edits_and_reset_after_seek_pause() {
    use soundroom_engine::{capture::CaptureTiming, realtime};
    let (mut control, mut host) = realtime::channel();
    let mut l = [0.; 64];
    let mut r = l;
    let stamp = |sample: u64| CaptureTiming {
        sample_time: 90000. + sample as f64,
        host_time: 1_000_000_000 + sample * 1000000000 / 48000,
        flags: 3,
    };
    control
        .publish(Box::new(prepare("legacy-gain")))
        .ok()
        .unwrap();
    control.request_playing(true).unwrap();
    ALLOCATIONS.with(|v| v.set(0));
    RETIREMENTS.with(|v| v.set(0));
    TRACK.with(|v| v.set(true));
    assert_eq!(
        host.render_timed(&mut l, &mut r, stamp(0)),
        CallbackStatus::Rendered
    );
    let first = control.output_anchor().unwrap();
    assert_eq!(first.playback_clock, 0);
    assert_eq!(first.frames, 64);
    assert_eq!(first.timing, stamp(0));
    TRACK.with(|v| v.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert_eq!(RETIREMENTS.with(Cell::get), 0);
    let cancel = AtomicBool::new(false);
    let cache = CheckpointCache::new(0);
    let worker = prepare("legacy-gain");
    let mut next = worker.fork();
    next.seek_playback_clock(128, &cache, &cancel).unwrap();
    control
        .publish_aligned_on(
            Box::new(next),
            128,
            control.telemetry().unwrap().handoff_token,
        )
        .ok()
        .unwrap();
    host.render_timed(&mut l, &mut r, stamp(64));
    host.render_timed(&mut l, &mut r, stamp(128));
    assert_eq!(control.output_anchor().unwrap().epoch, first.epoch);
    for sample in (192..512).step_by(64) {
        host.render_timed(&mut l, &mut r, stamp(sample));
    }
    control.reclaim_retired();
    let mut seek = worker.fork();
    seek.seek(1000, &cache, &cancel).unwrap();
    control.publish(Box::new(seek)).ok().unwrap();
    host.render_timed(&mut l, &mut r, stamp(512));
    let sought = control.output_anchor().unwrap();
    assert_eq!(sought.playback_clock, 1000);
    assert!(sought.epoch > first.epoch);
    control.request_playing(false).unwrap();
    for sample in (576..896).step_by(64) {
        host.render_timed(&mut l, &mut r, stamp(sample));
    }
    assert!(control.output_anchor().is_none());
    let frozen = control.telemetry().unwrap().playback_clock;
    control.request_playing(true).unwrap();
    host.render_timed(&mut l, &mut r, stamp(896));
    let resumed = control.output_anchor().unwrap();
    assert_eq!(resumed.playback_clock, frozen);
    assert!(resumed.epoch > sought.epoch);
    host.render(&mut l, &mut r);
    assert!(control.output_anchor().is_none());
}

#[test]
fn dense_automation_uses_shared_playback_storage_and_repeated_play_remains_bounded() {
    use soundroom_engine::{
        model::{AutomationPoint, Extra},
        realtime::channel,
    };
    fn allocated<T>(build: impl FnOnce() -> T) -> (T, usize, usize) {
        ALLOCATIONS.with(|v| v.set(0));
        REQUESTED_BYTES.with(|v| v.set(0));
        TRACK.with(|v| v.set(true));
        let value = build();
        TRACK.with(|v| v.set(false));
        (
            value,
            ALLOCATIONS.with(Cell::get),
            REQUESTED_BYTES.with(Cell::get),
        )
    }
    let sparse = prepare("modern-gain");
    let mut project = Project::open(&fixture("modern-gain", ".soundroom")).unwrap();
    let points: Vec<_> = (0..100_000)
        .map(|frame| AutomationPoint {
            frame,
            value: 0.5,
            curve: "smooth".into(),
            curve_start: 0.,
            curve_end: 1.,
            extra: Extra::new(),
        })
        .collect();
    let track = &mut project.session.tracks[0];
    track.mix_automation.insert("volume".into(), points.clone());
    track.realtime_effects[0].automation = points.clone();
    track.realtime_effects[0]
        .parameter_automation
        .insert("gain".into(), points);
    // Authored metadata must survive projection yet never inflate DSP checkpoints.
    track.extra.insert(
        "editorOnly".into(),
        serde_json::json!({"notes":"x".repeat(1_000_000)}),
    );
    let dense =
        PreparedGraph::prepare_resident(&project, 16 * 1024 * 1024, &AtomicBool::new(false))
            .unwrap();
    assert_eq!(
        dense.mutable_storage_budget(),
        sparse.mutable_storage_budget()
    );
    assert!(dense.immutable_storage_budget() < 8 * 1024 * 1024);
    let (fork, sparse_calls, sparse_bytes) = allocated(|| sparse.fork());
    drop(fork);
    let (fork, dense_calls, dense_bytes) = allocated(|| dense.fork());
    drop(fork);
    assert_eq!(
        dense_calls, sparse_calls,
        "fork must not clone authored point strings/metadata"
    );
    assert_eq!(
        dense_bytes, sparse_bytes,
        "fork bytes must be independent of immutable lane density"
    );
    assert!(dense_bytes <= dense.checkpoint_storage_budget());
    let (checkpoint, sparse_calls, sparse_bytes) = allocated(|| sparse.checkpoint());
    drop(checkpoint);
    let (checkpoint, dense_calls, dense_bytes) = allocated(|| dense.checkpoint());
    drop(checkpoint);
    assert_eq!((dense_calls, dense_bytes), (sparse_calls, sparse_bytes));
    assert!(dense_bytes <= dense.checkpoint_storage_budget());
    println!("100K-point x3 graph: shared bound {} bytes; mutable bound {}; checkpoint allocations {} bytes in {} calls",dense.immutable_storage_budget(),dense.mutable_storage_budget(),dense_bytes,dense_calls);
    assert_eq!(
        project.session.tracks[0].mix_automation["volume"].len(),
        100_000
    );
    assert_eq!(
        project.session.tracks[0].extra["editorOnly"]["notes"]
            .as_str()
            .unwrap()
            .len(),
        1_000_000
    );
    let (mut control, mut host) = channel();
    control.publish(Box::new(dense.fork())).ok().unwrap();
    let mut left = [0.; 128];
    let mut right = left;
    for _ in 0..3 {
        let play = control.request_playing(true).unwrap();
        host.render(&mut left, &mut right);
        assert_eq!(control.telemetry().unwrap().command_sequence, play);
        assert!(control.telemetry().unwrap().playing);
        let pause = control.request_playing(false).unwrap();
        for _ in 0..2 {
            host.render(&mut left, &mut right);
        }
        assert_eq!(control.telemetry().unwrap().command_sequence, pause);
        assert!(!control.telemetry().unwrap().playing);
    }
    let mut worker = dense.fork();
    let mut cache = CheckpointCache::new(dense.checkpoint_storage_budget() * 2);
    cache.store(&worker).unwrap();
    worker.seek(1234, &cache, &AtomicBool::new(false)).unwrap();
    render_no_alloc(&mut worker, 2048, 128);
}

#[test]
fn shared_playback_representation_preserves_aggregate_128_mib_admission_limit() {
    let mut project = Project::open(&fixture("pitch-automatic", ".soundroom")).unwrap();
    let effect = project.session.tracks[0].realtime_effects[0].clone();
    project.session.tracks[0].realtime_effects = (0..32)
        .map(|index| {
            let mut effect = effect.clone();
            effect.id = format!("10000000-0000-4000-8000-{index:012x}");
            effect
        })
        .collect();
    let mut second = project.session.tracks[0].clone();
    second.id = "20000000-0000-4000-8000-000000000001".into();
    second.clips.clear();
    project.session.tracks.push(second);
    let error = match PreparedGraph::prepare_resident(
        &project,
        16 * 1024 * 1024,
        &AtomicBool::new(false),
    ) {
        Ok(_) => panic!("128MiB aggregate limit was raised"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("128 MiB"), "{error}");
    let error = match soundroom_engine::render::Renderer::new(&project) {
        Ok(_) => panic!("Offline preparation bypassed the DSP memory budget"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("128 MiB"), "{error}");
}

#[test]
fn capture_sealing_fences_pcm_fault_counters_and_requires_final_drain_without_heap_work() {
    use soundroom_engine::capture::{capture_channel, CaptureConfig, CaptureStatus, CaptureTiming};
    let (mut writer, mut reader) = capture_channel(CaptureConfig {
        channels: 1,
        maximum_frames: 4,
        blocks: 2,
        sample_rate: 48000,
    })
    .unwrap();
    let control = writer.control();
    assert!(control.final_stats().is_none());
    assert!(reader.seal_receipt().is_none());
    assert_eq!(
        writer.push(&[&[0.1; 4]], CaptureTiming::default()),
        CaptureStatus::Captured
    );
    assert_eq!(
        writer.push(&[&[0.2; 4]], CaptureTiming::default()),
        CaptureStatus::Captured
    );
    assert_eq!(
        writer.push(&[&[0.3; 4]], CaptureTiming::default()),
        CaptureStatus::Overflow
    );
    writer.device_fault(-10851);
    ALLOCATIONS.with(|v| v.set(0));
    RETIREMENTS.with(|v| v.set(0));
    TRACK.with(|v| v.set(true));
    control.seal();
    control.seal();
    let final_stats = control.final_stats().unwrap();
    assert!(reader.seal_receipt().is_none());
    writer.device_fault(-50);
    assert_eq!(
        writer.push(&[&[f32::NAN; 4]], CaptureTiming::default()),
        CaptureStatus::Sealed
    );
    let mut audio = [0.; 4];
    let first = reader.read_planar(&mut [&mut audio]).unwrap().unwrap();
    assert_eq!(audio, [0.1; 4]);
    assert_eq!(first.source_frame, 0);
    assert!(reader.seal_receipt().is_none());
    let second = reader.read_planar(&mut [&mut audio]).unwrap().unwrap();
    assert_eq!(audio, [0.2; 4]);
    assert_eq!(second.source_frame, 4);
    let receipt = reader.seal_receipt().unwrap();
    assert!(reader.read_planar(&mut [&mut audio]).unwrap().is_none());
    TRACK.with(|v| v.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert_eq!(RETIREMENTS.with(Cell::get), 0);
    assert_eq!(receipt.stats(), final_stats);
    assert_eq!(receipt.stats().received_frames, 12);
    assert_eq!(receipt.stats().dropped_frames, 4);
    assert_eq!(receipt.stats().device_faults, 1);
    assert_eq!(receipt.stats().last_device_error, -10851);
    assert_eq!(receipt.stats().invalid_samples, 0);
}

#[test]
fn live_patch_edits_parameters_in_place_without_rebuilding() {
    use soundroom_engine::render::LivePatch;
    // Without drawn volume/pan curves, so track gain edits glide.
    let open = || {
        let mut project = Project::open(&fixture("pitch-automatic", ".soundroom")).unwrap();
        project.session.tracks[0].mix_automation.clear();
        project
    };
    let base = open();
    let cancel = AtomicBool::new(false);
    // Moving a clip is structural: it needs a rebuilt graph.
    let mut moved = base.session.clone();
    moved.tracks[0].clips[0].start += 1;
    assert!(LivePatch::between(&base.session, &moved).is_none());
    assert!(LivePatch::between(&base.session, &base.session).is_none());
    // A patched running graph matches one prepared with the new track gain.
    let mut quieter = open();
    quieter.session.tracks[0].gain = 0.5;
    let mut running = PreparedGraph::prepare_resident(&base, 16 * 1024 * 1024, &cancel).unwrap();
    let mut expected =
        PreparedGraph::prepare_resident(&quieter, 16 * 1024 * 1024, &cancel).unwrap();
    render_no_alloc(&mut running, 4096, 128);
    render_no_alloc(&mut expected, 4096, 128);
    let mut patch = LivePatch::between(&base.session, &quieter.session).unwrap();
    ALLOCATIONS.with(|v| v.set(0));
    RETIREMENTS.with(|v| v.set(0));
    TRACK.with(|v| v.set(true));
    let applied = running.apply_live(&mut patch);
    TRACK.with(|v| v.set(false));
    assert!(applied);
    assert_eq!(ALLOCATIONS.with(Cell::get), 0, "patching allocated");
    assert_eq!(RETIREMENTS.with(Cell::get), 0, "patching released storage");
    drop(patch);
    // The new gain glides in over 10 ms instead of stepping (a click), then
    // matches the rebuilt graph exactly.
    let after = render_no_alloc(&mut running, 8192, 128);
    let target = render_no_alloc(&mut expected, 8192, 128);
    for c in 0..2 {
        assert_eq!(after[c][480..], target[c][480..]);
        for i in 0..480 {
            let gain = 0.82 - 0.32 * (i + 1) as f32 / 480.;
            assert!((after[c][i] - target[c][i] * gain / 0.5).abs() < 1e-5, "frame {i}");
        }
    }
    assert!(after[0].iter().any(|v| v.abs() > 0.001));
    // Bypassing an insert crossfades to dry over the same 10 ms.
    let mut bypassed = open();
    bypassed.session.tracks[0].gain = 0.5;
    bypassed.session.tracks[0].realtime_effects[0].enabled = false;
    let mut dry = PreparedGraph::prepare_resident(&bypassed, 16 * 1024 * 1024, &cancel).unwrap();
    render_no_alloc(&mut dry, 4096 + 8192, 128);
    let mut patch = LivePatch::between(&quieter.session, &bypassed.session).unwrap();
    assert!(running.apply_live(&mut patch));
    let after = render_no_alloc(&mut running, 4096, 128);
    let wet = render_no_alloc(&mut expected, 4096, 128);
    let target = render_no_alloc(&mut dry, 4096, 128);
    for c in 0..2 {
        assert_eq!(after[c][480..], target[c][480..]);
        for i in 0..480 {
            let blend = 1. - (i + 1) as f32 / 480.;
            let mixed = target[c][i] + (wet[c][i] - target[c][i]) * blend;
            assert!((after[c][i] - mixed).abs() < 1e-5, "bypass frame {i}");
        }
    }
    // Insert parameters patch in place; a graph of another session refuses it.
    let mut retuned = quieter.session.clone();
    retuned.tracks[0].realtime_effects[0]
        .parameters
        .insert("retune".into(), 0.);
    let mut patch = LivePatch::between(&quieter.session, &retuned).unwrap();
    let mut other = prepare("chorus");
    assert!(!other.apply_live(&mut patch));
    assert!(running.apply_live(&mut patch));
}

#[test]
fn live_mute_and_solo_glide_without_rebuilding() {
    use soundroom_engine::render::LivePatch;
    let cancel = AtomicBool::new(false);
    let base = Project::open(&fixture("pitch-automatic", ".soundroom")).unwrap();
    let mut muted = base.session.clone();
    muted.tracks[0].muted = true;
    let mut patch = LivePatch::between(&base.session, &muted).expect("mute is a live edit");
    let mut running = PreparedGraph::prepare_resident(&base, 16 * 1024 * 1024, &cancel).unwrap();
    let mut reference = PreparedGraph::prepare_resident(&base, 16 * 1024 * 1024, &cancel).unwrap();
    render_no_alloc(&mut running, 4096, 128);
    render_no_alloc(&mut reference, 4096, 128);
    assert!(running.apply_live(&mut patch));
    let after = render_no_alloc(&mut running, 2048, 128);
    let open = render_no_alloc(&mut reference, 2048, 128);
    for i in 0..480 {
        let level = 1. - (i + 1) as f32 / 480.;
        assert!((after[0][i] - open[0][i] * level).abs() < 1e-5, "fade {i}");
    }
    assert!(after[0][480..].iter().chain(&after[1][480..]).all(|v| *v == 0.));
    assert!(open[0][480..].iter().any(|v| v.abs() > 1e-3));
    // Unmuting glides back in.
    let mut patch = LivePatch::between(&muted, &base.session).unwrap();
    assert!(running.apply_live(&mut patch));
    let back = render_no_alloc(&mut running, 1024, 128);
    let open = render_no_alloc(&mut reference, 1024, 128);
    assert!((back[0][479] - open[0][479]).abs() < 1e-5 && back[0][..479].iter().zip(&open[0]).all(|(a, b)| a.abs() <= b.abs() + 1e-6));
    // Solo is live too.
    let mut soloed = base.session.clone();
    soloed.tracks[0].solo = true;
    assert!(LivePatch::between(&base.session, &soloed).is_some());
}

#[test]
fn history_and_alternate_takes_stay_in_custody_without_loading_their_pcm_for_playback() {
    use soundroom_engine::model::{ClipHistory, Extra, Take};
    let mut project = Project::open(&fixture("modern-gain", ".soundroom")).unwrap();
    let original = project.session.tracks[0].clips[0].asset_id.to_uppercase();
    let media = project.media[&original].clone();
    let pcm_bytes = media.info.frames as usize * 8;
    let expected = prepare("modern-gain");
    let mut alternatives = Vec::new();
    for index in 0..256 {
        let id = format!("30000000-0000-4000-8000-{index:012X}");
        project.media.insert(id.clone(), media.clone());
        if index < 5 {
            project.session.tracks[0].clips[0].previous_states.push(ClipHistory {
                asset_id: id.clone(), offset: 0, length: 100, extra: Extra::new(),
            });
        }
        let mut take = project.session.tracks[0].clips[0].clone();
        take.id = format!("40000000-0000-4000-8000-{index:012X}");
        take.asset_id = id;
        take.previous_states.clear();
        alternatives.push(take);
    }
    project.session.tracks[0].recording_takes.push(Take { clips: alternatives, extra: Extra::new() });
    let saved = project.session.clone();
    assert_eq!(project.session.asset_ids().len(), 257);
    assert_eq!(project.session.playback_asset_ids().len(), 1);
    let mut resident = PreparedGraph::prepare_resident(&project, pcm_bytes, &AtomicBool::new(false)).unwrap();
    assert_eq!(resident.source_storage_bytes(), pcm_bytes);
    let (mut paged, mut loader) = PreparedGraph::prepare_paged(&project, 512 * 1024, &AtomicBool::new(false), None).unwrap();
    let reference = render_no_alloc(&mut expected.fork(), 8192, 128);
    assert_eq!(render_no_alloc(&mut resident, 8192, 128), reference);
    loader.prepare_graph(&paged, 32768, &AtomicBool::new(false)).unwrap();
    assert_eq!(render_no_alloc(&mut paged, 8192, 128), reference);
    REQUESTED_BYTES.with(|v| v.set(0));
    TRACK.with(|v| v.set(true));
    let offline = soundroom_engine::render::Renderer::new(&project);
    TRACK.with(|v| v.set(false));
    offline.unwrap();
    assert!(REQUESTED_BYTES.with(Cell::get) < 4 * 1024 * 1024,
        "Offline preparation retained decoder buffers for historical sources");
    assert_eq!(project.session, saved, "Playback projection cannot discard undo or take metadata");
    // Promoting a historical source to a current clip prepares that source.
    project.session.tracks[0].clips[0].asset_id = "30000000-0000-4000-8000-000000000000".into();
    let promoted = PreparedGraph::prepare_resident(&project, pcm_bytes, &AtomicBool::new(false)).unwrap();
    assert_eq!(promoted.source_storage_bytes(), pcm_bytes);
}
