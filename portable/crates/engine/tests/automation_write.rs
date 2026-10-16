//! Simulated native callback coordinates only. No device is enumerated/opened.
use soundroom_engine::{
    automation_write::{
        ReleaseReason, WriteError, WriteLease, WriteReceipt, WriteSubmission, WriteTarget,
    },
    capture::CaptureTiming,
    project::Project,
    realtime::{channel, AudioHost, GraphControl},
    render::{CallbackStatus, PreparedGraph},
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    path::PathBuf,
    sync::atomic::AtomicBool,
};
thread_local! {static TRACK:Cell<bool>=const {Cell::new(false)};static ALLOC:Cell<usize>=const {Cell::new(0)};static FREE:Cell<usize>=const {Cell::new(0)};}
struct Probe;
unsafe impl GlobalAlloc for Probe {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if TRACK.try_with(Cell::get).unwrap_or(false) {
            let _ = ALLOC.try_with(|v| v.set(v.get() + 1));
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if TRACK.try_with(Cell::get).unwrap_or(false) {
            let _ = FREE.try_with(|v| v.set(v.get() + 1));
        }
        unsafe { System.dealloc(p, l) }
    }
}
#[global_allocator]
static PROBE: Probe = Probe;
fn project(name: &str) -> Project {
    Project::open(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(format!("{name}.soundroom")),
    )
    .unwrap()
}
struct Harness {
    control: GraphControl,
    host: AudioHost,
    native: u64,
    audio: [Vec<f32>; 2],
}
impl Harness {
    fn new(graph: PreparedGraph) -> Self {
        let (mut control, host) = channel();
        control.publish(Box::new(graph)).ok().unwrap();
        control.request_playing(true).unwrap();
        Self {
            control,
            host,
            native: 0,
            audio: [vec![], vec![]],
        }
    }
    fn render(&mut self, frames: usize) {
        let mut left = vec![0.; frames];
        let mut right = left.clone();
        ALLOC.with(|v| v.set(0));
        FREE.with(|v| v.set(0));
        TRACK.with(|v| v.set(true));
        let status = self.host.render_timed(
            &mut left,
            &mut right,
            CaptureTiming {
                sample_time: self.native as f64,
                host_time: 1_000_000_000_000 + self.native * 20_833,
                flags: 3,
            },
        );
        TRACK.with(|v| v.set(false));
        assert_eq!(status, CallbackStatus::Rendered);
        assert_eq!(ALLOC.with(Cell::get), 0);
        assert_eq!(FREE.with(Cell::get), 0);
        self.native += frames as u64;
        self.audio[0].extend(left);
        self.audio[1].extend(right);
    }
    fn receipts(&mut self, lease: WriteLease) -> Vec<WriteReceipt> {
        let mut receipts = vec![];
        loop {
            match self.control.poll_write(lease) {
                Ok(Some(receipt)) => receipts.push(receipt),
                Ok(None) | Err(WriteError::StaleLease) => break,
                other => panic!("{other:?}"),
            }
        }
        receipts
    }
}
fn prepare(project: &Project) -> PreparedGraph {
    PreparedGraph::prepare_resident(project, 16 * 1024 * 1024, &AtomicBool::new(false)).unwrap()
}
fn amount_target(project: &Project) -> WriteTarget {
    WriteTarget::EffectAmount {
        track_id: project.session.tracks[0].id.clone(),
        effect_id: project.session.tracks[0].realtime_effects[0].id.clone(),
    }
}
#[test]
fn callback_receipts_coalescing_and_touch_end_preserve_actual_ramps_without_heap_work() {
    let p = project("legacy-gain");
    let graph = prepare(&p);
    let target = graph.resolve_write_target(&amount_target(&p)).unwrap();
    let mut h = Harness::new(graph.fork());
    assert_eq!(
        h.control.begin_write(1, target),
        Err(WriteError::Unavailable)
    );
    h.render(256);
    let lease = h.control.begin_write(1, target).unwrap();
    assert!(h.receipts(lease).is_empty());
    h.render(128);
    let began = h.receipts(lease);
    assert!(
        matches!(began.as_slice(),[WriteReceipt::Began{stamp,..}] if stamp.output_frame==256 && stamp.control_frame==256)
    );
    assert_eq!(
        h.control.write_value(lease, 1, 0.8).unwrap(),
        WriteSubmission::Queued
    );
    assert!(h.receipts(lease).is_empty());
    h.render(64);
    let first = h.receipts(lease);
    assert!(
        matches!(first.as_slice(),[WriteReceipt::Applied{sequence:1,stamp,ramp,..}] if stamp.control_frame==384 && ramp.frames==256)
    );
    h.control.write_value(lease, 2, 0.2).unwrap();
    assert_eq!(
        h.control.write_value(lease, 3, 0.9).unwrap(),
        WriteSubmission::Superseded { sequence: 2 }
    );
    h.render(64);
    let second = h.receipts(lease);
    assert!(
        matches!(second.as_slice(),[WriteReceipt::Applied{sequence:3,stamp,..}] if stamp.control_frame==448)
    );
    h.control
        .end_write(lease, 3, ReleaseReason::Finished)
        .unwrap();
    h.render(256);
    let mut tail = h.receipts(lease);
    h.render(64);
    tail.extend(h.receipts(lease));
    for _ in 0..8 {
        h.render(64);
        tail.extend(h.receipts(lease));
    }
    assert!(tail.iter().any(|r| matches!(
        r,
        WriteReceipt::Restoring {
            reason: ReleaseReason::Finished,
            ..
        }
    )));
    assert!(tail.iter().any(|r| matches!(
        r,
        WriteReceipt::Released {
            reason: ReleaseReason::Finished,
            last_applied: Some(_),
            ..
        }
    )));
    assert_eq!(
        h.control.write_value(lease, 4, 0.2),
        Err(WriteError::StaleLease)
    );
}
#[test]
fn priority_end_can_overtake_last_value_and_stop_rejects_unapplied_points() {
    let p = project("legacy-gain");
    let graph = prepare(&p);
    let target = graph.resolve_write_target(&amount_target(&p)).unwrap();
    let mut h = Harness::new(graph.fork());
    h.render(256);
    let lease = h.control.begin_write(5, target).unwrap();
    h.render(64);
    h.receipts(lease);
    h.control
        .end_write(lease, 1, ReleaseReason::Finished)
        .unwrap();
    h.control.write_value(lease, 1, 0.9).unwrap();
    h.render(128);
    assert!(h
        .receipts(lease)
        .iter()
        .any(|r| matches!(r, WriteReceipt::Applied { sequence: 1, .. })));
    for _ in 0..10 {
        h.render(128);
        h.receipts(lease);
    }
    let lease = h.control.begin_write(6, target).unwrap();
    h.render(64);
    h.receipts(lease);
    h.control.write_value(lease, 1, 0.8).unwrap();
    h.control.request_playing(false).unwrap();
    h.render(64);
    let receipts = h.receipts(lease);
    assert!(receipts.iter().any(|r| matches!(
        r,
        WriteReceipt::Rejected {
            sequence: Some(1),
            ..
        }
    )));
    assert!(receipts.iter().any(|r| matches!(
        r,
        WriteReceipt::Released {
            reason: ReleaseReason::Stopped,
            last_applied: None,
            ..
        }
    )));
}

#[derive(Clone, Copy, Debug)]
enum LaneKind {
    Volume,
    Pan,
    Amount,
    Parameter(&'static str),
}
fn points(
    receipts: &[WriteReceipt],
    baseline: f32,
) -> Vec<soundroom_engine::model::AutomationPoint> {
    use soundroom_engine::model::{AutomationPoint, Extra};
    let mut values: Vec<(u64, f32)> = vec![];
    for receipt in receipts {
        match *receipt {
            WriteReceipt::Applied { stamp, ramp, .. }
            | WriteReceipt::Restoring { stamp, ramp, .. } => {
                values.retain(|(frame, _)| *frame < stamp.control_frame);
                values.push((stamp.control_frame, ramp.start_value));
                values.push((stamp.control_frame + ramp.frames, ramp.target_value));
            }
            WriteReceipt::Released {
                stamp,
                last_applied: Some((frame, value)),
                ..
            } => {
                values.retain(|(f, _)| *f < frame);
                values.push((frame, value));
                values.push((stamp.control_frame, baseline));
            }
            _ => {}
        }
    }
    values.sort_by_key(|(frame, _)| *frame);
    values.dedup_by_key(|(frame, _)| *frame);
    values
        .into_iter()
        .map(|(frame, value)| AutomationPoint {
            frame,
            value,
            curve: "linear".into(),
            curve_start: 0.,
            curve_end: 1.,
            extra: Extra::new(),
        })
        .collect()
}
#[test]
fn actual_ramp_receipts_replay_offline_for_mix_legacy_and_lookahead_effect_paths() {
    for (name, kind, master, decibels) in [
        ("legacy-gain", LaneKind::Amount, false, false),
        ("modern-gain", LaneKind::Volume, false, false),
        ("modern-gain", LaneKind::Volume, false, true),
        ("limiter-master", LaneKind::Volume, true, false),
        ("limiter-master", LaneKind::Volume, true, true),
        ("modern-gain", LaneKind::Pan, false, false),
        ("modern-gain", LaneKind::Parameter("gain"), false, false),
        ("chorus", LaneKind::Parameter("depth"), false, false),
        (
            "equalizer",
            LaneKind::Parameter("midFrequency"),
            false,
            false,
        ),
        (
            "room-transition",
            LaneKind::Parameter("decay"),
            false,
            false,
        ),
        (
            "resonance-track",
            LaneKind::Parameter("depth"),
            false,
            false,
        ),
        ("warmth-tube", LaneKind::Parameter("drive"), false, false),
        ("compressor", LaneKind::Parameter("ratio"), false, false),
        ("dither-only", LaneKind::Parameter("amount"), false, false),
        ("modern-echo", LaneKind::Parameter("time"), false, false),
        (
            "modern-highpass",
            LaneKind::Parameter("frequency"),
            false,
            false,
        ),
        ("room-dense", LaneKind::Parameter("mix"), false, false),
        (
            "modern-lowpass",
            LaneKind::Parameter("frequency"),
            false,
            false,
        ),
        (
            "pitch-automatic",
            LaneKind::Parameter("strength"),
            false,
            false,
        ),
        (
            "limiter-master",
            LaneKind::Parameter("ceiling"),
            true,
            false,
        ),
    ] {
        let mut p = project(name);
        let effect_index = if master { 1 } else { 0 };
        if name == "room-dense" {
            p.session.tracks[0].realtime_effects[0].parameters.clear();
        }
        let track = if master {
            &mut p.session.master_track
        } else {
            &mut p.session.tracks[0]
        };
        let target = match kind {
            LaneKind::Volume => {
                track.gain = 0.75;
                track.mix_automation.remove("volume");
                track.volume_automation_scale =
                    Some(if decibels { "decibels" } else { "linear" }.into());
                WriteTarget::Volume {
                    track_id: track.id.clone(),
                }
            }
            LaneKind::Pan => {
                track.pan = 0.;
                track.mix_automation.remove("pan");
                WriteTarget::Pan {
                    track_id: track.id.clone(),
                }
            }
            LaneKind::Amount => {
                track.realtime_effects[effect_index].amount = 0.5;
                track.realtime_effects[effect_index].automation.clear();
                WriteTarget::EffectAmount {
                    track_id: track.id.clone(),
                    effect_id: track.realtime_effects[effect_index].id.clone(),
                }
            }
            LaneKind::Parameter(key) => {
                track.realtime_effects[effect_index]
                    .parameter_automation
                    .remove(key);
                WriteTarget::EffectParameter {
                    track_id: track.id.clone(),
                    effect_id: track.realtime_effects[effect_index].id.clone(),
                    parameter_id: key.into(),
                }
            }
        };
        let graph = prepare(&p);
        let resolved = graph.resolve_write_target(&target).unwrap();
        let mut live = Harness::new(graph.fork());
        live.render(256);
        let lease = live.control.begin_write(0x100, resolved).unwrap();
        live.render(128);
        let mut receipts = live.receipts(lease);
        live.control.write_value(lease, 1, 0.8).unwrap();
        live.render(64);
        let first = live.receipts(lease);
        let baseline = match first[0] {
            WriteReceipt::Applied { ramp, .. } => ramp.start_value,
            _ => panic!("missing first applied sample"),
        };
        receipts.extend(first);
        live.control.write_value(lease, 2, 0.2).unwrap();
        live.control.write_value(lease, 3, 0.9).unwrap();
        live.render(64);
        receipts.extend(live.receipts(lease));
        live.control
            .end_write(lease, 3, ReleaseReason::Finished)
            .unwrap();
        for _ in 0..10 {
            live.render(128);
            receipts.extend(live.receipts(lease));
        }
        assert!(receipts
            .iter()
            .any(|r| matches!(r, WriteReceipt::Released { .. })));
        for receipt in &receipts {
            match receipt {
                WriteReceipt::Applied { stamp, .. } | WriteReceipt::Restoring { stamp, .. } => {
                    assert_eq!(
                        stamp.control_frame - stamp.playback_clock,
                        resolved.latency_offset_frames()
                    )
                }
                _ => {}
            }
        }
        live.render(8192 - live.native as usize);
        let authored = points(&receipts, baseline);
        let track = if master {
            &mut p.session.master_track
        } else {
            &mut p.session.tracks[0]
        };
        match kind {
            LaneKind::Volume => {
                track.mix_automation.insert("volume".into(), authored);
            }
            LaneKind::Pan => {
                track.mix_automation.insert("pan".into(), authored);
            }
            LaneKind::Amount => track.realtime_effects[effect_index].automation = authored,
            LaneKind::Parameter(key) => {
                track.realtime_effects[effect_index]
                    .parameter_automation
                    .insert(key.into(), authored);
            }
        }
        let mut offline = Harness::new(prepare(&p));
        for _ in 0..64 {
            offline.render(128);
        }
        let error = live
            .audio
            .iter()
            .flatten()
            .zip(offline.audio.iter().flatten())
            .fold(0f32, |err, (a, b)| err.max((a - b).abs()));
        println!("Write offline replay {name} {kind:?},master={master},db={decibels}:error={error},latencyOffset={}",resolved.latency_offset_frames());
        assert!(error < 5e-6, "{name} {kind:?}: {error}");
    }
}

#[test]
fn stop_freezes_partial_ramp_actual_prefix_and_restores_read_without_future_points() {
    let mut p = project("legacy-gain");
    p.session.tracks[0].realtime_effects[0].automation.clear();
    p.session.tracks[0].realtime_effects[0].amount = 0.5;
    let graph = prepare(&p);
    let target = graph.resolve_write_target(&amount_target(&p)).unwrap();
    let mut live = Harness::new(graph.fork());
    live.render(64); // Stop can only render the actually attained partial Play fade.
    let lease = live.control.begin_write(0x200, target).unwrap();
    live.render(32);
    let mut receipts = live.receipts(lease);
    live.control.write_value(lease, 1, 0.9).unwrap();
    live.render(16);
    receipts.extend(live.receipts(lease));
    live.control.request_playing(false).unwrap();
    live.render(512);
    receipts.extend(live.receipts(lease));
    let restored = receipts
        .iter()
        .find_map(|r| match r {
            WriteReceipt::Restoring { stamp, ramp, .. } => Some((*stamp, *ramp)),
            _ => None,
        })
        .unwrap();
    let terminal = receipts
        .iter()
        .find_map(|r| match r {
            WriteReceipt::Released {
                reason,
                stamp,
                last_applied: Some(last),
                ..
            } => Some((*reason, *stamp, *last)),
            _ => None,
        })
        .unwrap();
    assert_eq!(terminal.0, ReleaseReason::Stopped);
    assert_eq!(terminal.1.control_frame, 224);
    assert_eq!(terminal.2 .0, 223);
    assert!(terminal.1.control_frame < restored.0.control_frame + restored.1.frames);
    assert_ne!(terminal.2 .1, restored.1.target_value);
    let authored = points(&receipts, 0.5);
    assert_eq!(authored.last().unwrap().frame, 224);
    p.session.tracks[0].realtime_effects[0].automation = authored;
    let mut offline = Harness::new(prepare(&p));
    offline.render(64);
    offline.render(32);
    offline.render(16);
    offline.control.request_playing(false).unwrap();
    offline.render(512);
    let error = live
        .audio
        .iter()
        .flatten()
        .zip(offline.audio.iter().flatten())
        .fold(0f32, |e, (a, b)| e.max((a - b).abs()));
    assert!(error < 5e-6, "partial Stop replay {error}");
    let pause = live.control.telemetry().unwrap();
    assert!(!pause.playing);
    assert_eq!(pause.frame, 224);
    live.control.request_playing(true).unwrap();
    live.render(256);
    let replacement = live.control.begin_write(0x200, target).unwrap();
    assert_ne!(lease, replacement); // UUID reuse never makes an old handle valid.
    assert_eq!(
        live.control.write_value(lease, 2, 0.3),
        Err(WriteError::StaleLease)
    );
    live.render(1);
    live.receipts(replacement);
    live.control
        .end_write(replacement, 0, ReleaseReason::Cancelled)
        .unwrap();
    live.render(1);
    live.receipts(replacement);
}

#[test]
fn aborts_restore_read_and_terminal_boundaries_are_sample_exact_across_irregular_blocks() {
    for reason in [
        ReleaseReason::Cancelled,
        ReleaseReason::Revoked,
        ReleaseReason::Disconnected,
        ReleaseReason::Conflict,
        ReleaseReason::Finished,
    ] {
        let mut p = project("legacy-gain");
        p.session.tracks[0].realtime_effects[0].automation.clear();
        let graph = prepare(&p);
        let target = graph.resolve_write_target(&amount_target(&p)).unwrap();
        let mut live = Harness::new(graph.fork());
        live.render(256);
        let lease = live.control.begin_write(0x300, target).unwrap();
        live.render(37);
        live.receipts(lease);
        live.control.write_value(lease, 1, 0.8).unwrap();
        live.render(71);
        live.receipts(lease);
        // Explicit abort must release despite a stale/omitted sequence, including an
        // earlier End which was waiting for a final Value that never arrived.
        if reason != ReleaseReason::Finished {
            live.control
                .end_write(lease, 3, ReleaseReason::Finished)
                .unwrap();
        }
        live.control
            .end_write(
                lease,
                if reason == ReleaseReason::Finished {
                    1
                } else {
                    0
                },
                reason,
            )
            .unwrap();
        live.render(777);
        let receipts = live.receipts(lease);
        let start = receipts
            .iter()
            .find_map(|r| match r {
                WriteReceipt::Restoring { stamp, .. } => Some(*stamp),
                _ => None,
            })
            .unwrap();
        let end = receipts
            .iter()
            .find_map(|r| match r {
                WriteReceipt::Released {
                    reason: r,
                    stamp,
                    last_applied: Some(last),
                    ..
                } => Some((*r, *stamp, *last)),
                _ => None,
            })
            .unwrap();
        assert_eq!(end.0, reason);
        assert_eq!(end.1.control_frame, start.control_frame + 256);
        assert_eq!(end.2 .0, end.1.control_frame - 1);
        let n = live.control.begin_write(0x301, target).unwrap();
        live.render(1);
        live.receipts(n);
        live.control
            .end_write(n, 0, ReleaseReason::Cancelled)
            .unwrap();
        live.render(1);
        live.receipts(n);
    }
}

#[test]
fn target_and_capacity_gates_leave_audio_unchanged_and_cancel_all_sixteen_slots() {
    let mut p = project("modern-gain");
    // 17 distinct prepared mix lanes, no callback UUID lookup or dynamic leases.
    let base = p.session.tracks[0].clone();
    for i in 1..17 {
        let mut t = base.clone();
        t.id = format!("00000000-0000-0000-0000-{i:012}");
        t.clips.clear();
        t.realtime_effects.clear();
        p.session.tracks.push(t);
    }
    let graph = prepare(&p);
    let targets: Vec<_> = p
        .session
        .tracks
        .iter()
        .map(|t| {
            graph
                .resolve_write_target(&WriteTarget::Pan {
                    track_id: t.id.clone(),
                })
                .unwrap()
        })
        .collect();
    let mut h = Harness::new(graph.fork());
    h.render(256);
    let leases: Vec<_> = targets[..16]
        .iter()
        .enumerate()
        .map(|(i, t)| h.control.begin_write(i as u128 + 1, *t).unwrap())
        .collect();
    assert_eq!(
        h.control.begin_write(100, targets[16]),
        Err(WriteError::Exhausted)
    );
    assert_eq!(
        h.control.begin_write(200, targets[0]),
        Err(WriteError::Busy)
    );
    h.render(64);
    for l in &leases {
        assert!(matches!(
            h.receipts(*l).as_slice(),
            [WriteReceipt::Began { .. }]
        ));
        h.control.write_value(*l, 1, 0.7).unwrap();
    }
    h.render(64);
    for l in &leases {
        assert!(matches!(
            h.receipts(*l).as_slice(),
            [WriteReceipt::Applied { .. }]
        ));
        h.control
            .end_write(*l, 0, ReleaseReason::Cancelled)
            .unwrap();
    }
    h.render(512);
    for l in &leases {
        assert!(h.receipts(*l).iter().any(|r| matches!(
            r,
            WriteReceipt::Released {
                reason: ReleaseReason::Cancelled,
                ..
            }
        )));
    }
    let mut high = project("modern-gain");
    let id = high.session.tracks[0].id.clone();
    high.session.tracks[0].gain = 4.;
    high.session.tracks[0].mix_automation.remove("volume");
    assert!(prepare(&high)
        .resolve_write_target(&WriteTarget::Volume { track_id: id })
        .is_err());
    let p = project("limiter-master");
    let graph = prepare(&p);
    for target in [
        WriteTarget::Pan {
            track_id: p.session.master_track.id.clone(),
        },
        WriteTarget::EffectParameter {
            track_id: p.session.master_track.id.clone(),
            effect_id: p.session.master_track.realtime_effects[1].id.clone(),
            parameter_id: "style".into(),
        },
        WriteTarget::EffectAmount {
            track_id: p.session.master_track.id.clone(),
            effect_id: p.session.master_track.realtime_effects[1].id.clone(),
        },
    ] {
        assert!(graph.resolve_write_target(&target).is_err());
    }
    let mut bypass = project("legacy-gain");
    bypass.session.tracks[0].realtime_effects[0].enabled = false;
    assert!(prepare(&bypass)
        .resolve_write_target(&amount_target(&bypass))
        .is_err());
}

#[test]
fn empty_callbacks_do_not_apply_values_and_seek_handoff_releases_stale_override() {
    use soundroom_engine::render::CheckpointCache;
    let p = project("legacy-gain");
    let canonical = prepare(&p);
    let target = canonical.resolve_write_target(&amount_target(&p)).unwrap();
    let mut h = Harness::new(canonical.fork());
    h.render(256);
    let lease = h.control.begin_write(0x400, target).unwrap();
    h.render(64);
    h.receipts(lease);
    h.control.write_value(lease, 1, 0.9).unwrap();
    let before = h.control.telemetry().unwrap();
    h.render(0);
    assert!(h.receipts(lease).is_empty());
    assert_eq!(
        h.control.telemetry().unwrap().callback_count,
        before.callback_count
    );
    h.render(64);
    assert!(h
        .receipts(lease)
        .iter()
        .any(|r| matches!(r, WriteReceipt::Applied { sequence: 1, .. })));
    let mut candidate = canonical.fork();
    candidate
        .seek(1024, &CheckpointCache::new(0), &AtomicBool::new(false))
        .unwrap();
    h.control.publish(Box::new(candidate)).ok().unwrap();
    h.render(64);
    assert!(h.receipts(lease).iter().any(|r| matches!(
        r,
        WriteReceipt::Released {
            reason: ReleaseReason::Discontinuity,
            ..
        }
    )));
    assert_eq!(
        h.control.write_value(lease, 2, 0.7),
        Err(WriteError::StaleLease)
    );
    // A new lease for the same UUID/graph is bound to the newly adopted token.
    let new = h.control.begin_write(0x400, target).unwrap();
    h.render(64);
    assert!(h
        .receipts(new)
        .iter()
        .any(|r| matches!(r, WriteReceipt::Began { .. })));
    h.control
        .end_write(new, 0, ReleaseReason::Cancelled)
        .unwrap();
    h.render(64);
    h.receipts(new);
}

#[test]
fn begin_is_inaudible_and_completed_touch_restores_changing_original_curve_exactly() {
    use soundroom_engine::model::{AutomationPoint, Extra};
    for curve in ["hold", "smooth", "easeIn", "easeOut", "linear"] {
        let mut p = project("legacy-gain");
        p.session.tracks[0].realtime_effects[0].automation =
            vec![(1000, 0.3), (1800, 0.7), (2400, 0.4)]
                .into_iter()
                .map(|(frame, value)| AutomationPoint {
                    frame,
                    value,
                    curve: curve.into(),
                    curve_start: 0.2,
                    curve_end: 0.8,
                    extra: Extra::new(),
                })
                .collect();
        let canonical = prepare(&p);
        let target = canonical.resolve_write_target(&amount_target(&p)).unwrap();
        let mut reference = Harness::new(canonical.fork());
        let mut live = Harness::new(canonical.fork());
        live.render(256);
        reference.render(256);
        let lease = live.control.begin_write(0x500, target).unwrap();
        live.render(777);
        reference.render(777);
        live.receipts(lease);
        assert_eq!(
            live.audio, reference.audio,
            "Begin changed original {curve}"
        );
        live.control.write_value(lease, 1, 0.8).unwrap();
        live.render(123);
        reference.render(123);
        live.receipts(lease);
        live.control
            .end_write(lease, 1, ReleaseReason::Finished)
            .unwrap();
        live.render(901);
        reference.render(901);
        let receipts = live.receipts(lease);
        let end = receipts
            .iter()
            .find_map(|r| match r {
                WriteReceipt::Released { stamp, .. } => Some(stamp.control_frame as usize),
                _ => None,
            })
            .unwrap();
        live.render(8192 - live.native as usize);
        reference.render(8192 - reference.native as usize);
        assert_eq!(
            &live.audio[0][end..],
            &reference.audio[0][end..],
            "Read changed after release {curve}"
        );
        assert_eq!(
            &live.audio[1][end..],
            &reference.audio[1][end..],
            "Read changed after release {curve}"
        );
    }
}

#[test]
fn concurrent_control_and_audio_host_keep_applied_and_superseded_sequences_distinct() {
    use std::sync::{atomic::Ordering, Arc};
    use std::time::{Duration, Instant};
    let p = project("legacy-gain");
    let graph = prepare(&p);
    let target = graph.resolve_write_target(&amount_target(&p)).unwrap();
    let (mut control, mut host) = channel();
    control.publish(Box::new(graph)).ok().unwrap();
    control.request_playing(true).unwrap();
    let done = Arc::new(AtomicBool::new(false));
    let stopped = Arc::clone(&done);
    let consumer = std::thread::spawn(move || {
        let mut left = [0.; 64];
        let mut right = [0.; 64];
        let mut frame = 0u64;
        while !stopped.load(Ordering::Acquire) {
            ALLOC.with(|v| v.set(0));
            FREE.with(|v| v.set(0));
            TRACK.with(|v| v.set(true));
            let status = host.render_timed(
                &mut left,
                &mut right,
                CaptureTiming {
                    sample_time: frame as f64,
                    host_time: 1_000_000_000_000 + frame * 20_833,
                    flags: 3,
                },
            );
            TRACK.with(|v| v.set(false));
            assert_eq!(status, CallbackStatus::Rendered);
            assert_eq!(ALLOC.with(Cell::get), 0);
            assert_eq!(FREE.with(Cell::get), 0);
            frame += 64;
            std::thread::yield_now();
        }
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let lease = loop {
        assert!(Instant::now() < deadline);
        match control.begin_write(0x600, target) {
            Ok(l) => break l,
            Err(WriteError::Unavailable) => std::thread::yield_now(),
            other => panic!("{other:?}"),
        }
    };
    loop {
        assert!(Instant::now() < deadline);
        match control.poll_write(lease).unwrap() {
            Some(WriteReceipt::Began { .. }) => break,
            None => std::thread::yield_now(),
            r => panic!("{r:?}"),
        }
    }
    let mut outcomes = vec![0u8; 1001];
    let mut sequence = 1;
    while sequence <= 1000 {
        if let Some(r) = control.poll_write(lease).unwrap() {
            match r {
                WriteReceipt::Applied { sequence: s, .. } => {
                    assert_eq!(outcomes[s as usize], 0);
                    outcomes[s as usize] = 1
                }
                other => panic!("{other:?}"),
            }
        }
        match control.write_value(lease, sequence, (sequence % 10) as f32 / 10.) {
            Ok(WriteSubmission::Queued) => sequence += 1,
            Ok(WriteSubmission::Superseded { sequence: old }) => {
                assert_eq!(outcomes[old as usize], 0);
                outcomes[old as usize] = 2;
                sequence += 1;
            }
            Err(WriteError::Busy) => std::thread::yield_now(),
            other => panic!("{other:?}"),
        }
        assert!(Instant::now() < deadline);
    }
    control
        .end_write(lease, 1000, ReleaseReason::Finished)
        .unwrap();
    loop {
        assert!(Instant::now() < deadline);
        match control.poll_write(lease).unwrap() {
            Some(WriteReceipt::Applied { sequence: s, .. }) => {
                assert_eq!(outcomes[s as usize], 0);
                outcomes[s as usize] = 1
            }
            Some(WriteReceipt::Restoring { .. }) => {}
            Some(WriteReceipt::Released {
                last_applied: Some(last),
                stamp,
                ..
            }) => {
                assert_eq!(last.0 + 1, stamp.control_frame);
                break;
            }
            None => std::thread::yield_now(),
            other => panic!("{other:?}"),
        }
    }
    done.store(true, Ordering::Release);
    consumer.join().unwrap();
    assert!(outcomes[1..].iter().all(|n| [1, 2].contains(n)));
    assert!(outcomes[1..].contains(&1));
}

#[test]
fn mapping_comes_from_prepared_dsp_and_unsupported_targets_remain_explicit() {
    use soundroom_engine::automation_write::WriteInterpolation;
    let mut p = project("modern-lowpass");
    p.session.tracks[0].gain = 1.2;
    p.session.tracks[0].mix_automation.remove("volume");
    p.session.tracks[0].volume_automation_scale = Some("decibels".into());
    let g = prepare(&p);
    let t = &p.session.tracks[0];
    let volume = g
        .resolve_write_target(&WriteTarget::Volume {
            track_id: t.id.clone(),
        })
        .unwrap();
    let m = g.write_mapping(volume).unwrap();
    assert_eq!(
        (
            m.minimum,
            m.maximum,
            m.default_value,
            m.unit,
            m.interpolation_scale
        ),
        (0., 1.5, 1., "gain", WriteInterpolation::Decibels)
    );
    assert_eq!(m.baseline_normalized, 1.2 / 1.5);
    let cutoff = g
        .resolve_write_target(&WriteTarget::EffectParameter {
            track_id: t.id.clone(),
            effect_id: t.realtime_effects[0].id.clone(),
            parameter_id: "frequency".into(),
        })
        .unwrap();
    let m = g.write_mapping(cutoff).unwrap();
    assert_eq!(
        (
            m.minimum,
            m.maximum,
            m.default_value,
            m.unit,
            m.logarithmic,
            m.integer
        ),
        (30., 20_000., 4000., "Hz", true, false)
    );
    assert!(prepare(&p).write_mapping(cutoff).is_err());
    let legacy = project("legacy-gain");
    let g = prepare(&legacy);
    assert!(g
        .resolve_write_target(&WriteTarget::EffectParameter {
            track_id: legacy.session.tracks[0].id.clone(),
            effect_id: legacy.session.tracks[0].realtime_effects[0].id.clone(),
            parameter_id: "gain".into()
        })
        .is_err());
    let g = PreparedGraph::prepare_resident_loop(
        &legacy,
        16 * 1024 * 1024,
        &AtomicBool::new(false),
        soundroom_engine::render::LoopRange::new(0, 1024).unwrap(),
    )
    .unwrap();
    assert!(g.resolve_write_target(&amount_target(&legacy)).is_err());
}
