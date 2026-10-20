//! Single audio-owner graph handoff. Workers prepare/reclaim; callbacks only
//! exchange bounded pointers and render into preallocated scratch.
use crate::{
    automation_write::{
        ControlLease, Message, ReleaseReason, ResolvedWriteTarget, RuntimeLease, Slot, WriteError,
        WriteLease, WriteReceipt, WriteStamp, WriteSubmission, MAX_WRITE_SLOTS,
    },
    capture::CaptureTiming,
    capture_clock::OutputClockAnchor,
    render::{CallbackStatus, LivePatch, LoopRange, PreparedGraph},
};
use std::sync::{
    atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering},
    Arc,
};
const BLOCK: usize = 8192;
const FADE: usize = 256;
/// Loop playback dips through a 2 ms fade at the wrap, like a clip edge:
/// jumping from the loop's end back to its start splices two unrelated
/// waveforms, which clicks.
const LOOP_FADE: u64 = 96;
/// Output gain at `frame` frames past the loop start, counting through wraps.
pub fn loop_wrap_gain(range: LoopRange, frame: u64) -> f32 {
    let length = range.end - range.start;
    let fade = LOOP_FADE.min(length / 4).max(1);
    let offset = (frame - range.start) % length;
    let x = if offset < fade {
        (offset as f32 + 0.5) / fade as f32
    } else if length - offset <= fade {
        ((length - offset) as f32 - 0.5) / fade as f32
    } else {
        return 1.;
    };
    0.5 - 0.5 * (std::f32::consts::PI * x).cos()
}
struct Shared {
    /// Max-hold output peaks (non-negative f32 bits order like u32), taken by control.
    peak_left: AtomicU32,
    peak_right: AtomicU32,
    output_available: AtomicBool,
    output_boundary: AtomicU64,
    anchor_output_boundary: AtomicU64,
    writes: [Slot; MAX_WRITE_SLOTS],
    pending: AtomicPtr<PreparedGraph>,
    retired: AtomicPtr<PreparedGraph>,
    command: AtomicU64,
    version: AtomicU64,
    position: AtomicU64,
    playback_clock: AtomicU64,
    acknowledged: AtomicU64,
    graph_generation: AtomicU64,
    handoff_token: AtomicU64,
    callback_count: AtomicU64,
    faults: AtomicU64,
    cancel_handoff: AtomicU64,
    rejected_handoff: AtomicU64,
    rejection_count: AtomicU64,
    anchor_sample: AtomicU64,
    anchor_host: AtomicU64,
    anchor_flags: AtomicU64,
    anchor_clock: AtomicU64,
    anchor_frames: AtomicU64,
    anchor_epoch: AtomicU64,
    /// Frames of silence before the next Play starts the graph (count-in).
    start_delay: AtomicU64,
    /// Transport click: frames per beat as f64 bits (0 is off) and beats per bar.
    metronome_beat: AtomicU64,
    metronome_meter: AtomicU32,
    /// Parameter-only edits for the current graph, and the applied (or
    /// rejected) patch handed back for release off the callback.
    live: AtomicPtr<LivePatch>,
    live_retired: AtomicPtr<LivePatch>,
    live_taken: AtomicU64,
    live_rejected: AtomicU64,
}
impl Drop for Shared {
    fn drop(&mut self) {
        // Arc's last owner is retired by the non-realtime host/control lifecycle.
        for slot in [&self.pending, &self.retired] {
            let ptr = slot.swap(std::ptr::null_mut(), Ordering::AcqRel);
            if !ptr.is_null() {
                unsafe {
                    drop(Box::from_raw(ptr));
                }
            }
        }
        for slot in [&self.live, &self.live_retired] {
            let ptr = slot.swap(std::ptr::null_mut(), Ordering::AcqRel);
            if !ptr.is_null() {
                unsafe {
                    drop(Box::from_raw(ptr));
                }
            }
        }
    }
}
/// Arms count-in clicks and a matching start delay: the callback that first
/// sees the next Play starts the clicks and holds the graph for their length.
#[derive(Clone)]
pub struct CountIn {
    shared: Arc<Shared>,
    preview: crate::preview::PreviewControl,
}
impl CountIn {
    pub fn cancel(&self) {
        self.shared.start_delay.store(0, Ordering::Release);
        self.preview.release(crate::preview::COUNT_IN_VOICE);
    }
    pub fn arm(&self, clicks: crate::preview::Buffer) {
        let frames = clicks.left.len() as u64;
        self.preview.arm(crate::preview::COUNT_IN_VOICE, clicks);
        self.shared.start_delay.store(frames, Ordering::Release);
    }
}
/// Switches the transport click. The callback mixes it into the playing graph
/// at the timeline position, so it follows seeks, loops and graph handoffs.
#[derive(Clone)]
pub struct Metronome {
    shared: Arc<Shared>,
}
impl Metronome {
    pub fn set(&self, grid: Option<crate::metronome::Grid>) {
        match grid.filter(|g| g.beat_frames.is_finite() && g.beat_frames >= 1.) {
            Some(grid) => {
                self.shared
                    .metronome_meter
                    .store(grid.beats_per_bar.max(1), Ordering::Release);
                self.shared
                    .metronome_beat
                    .store(grid.beat_frames.to_bits(), Ordering::Release);
            }
            None => self.shared.metronome_beat.store(0, Ordering::Release),
        }
    }
}
/// Control-thread endpoint. Mutable methods enforce one command/graph publisher.
pub struct GraphControl {
    shared: Arc<Shared>,
    preview: crate::preview::PreviewControl,
    next_handoff: u64,
    next_write: u64,
    writes: [Option<ControlLease>; MAX_WRITE_SLOTS],
}
/// Publishes parameter-only edits for the running graph (see [`LivePatch`]).
#[derive(Clone)]
pub struct LivePublisher {
    shared: Arc<Shared>,
}
impl LivePublisher {
    /// A newer patch replaces one the callback has not taken yet.
    pub fn publish(&self, patch: Box<LivePatch>) {
        self.reclaim();
        let old = self
            .shared
            .live
            .swap(Box::into_raw(patch), Ordering::AcqRel);
        if !old.is_null() {
            // Never taken by the callback: still owned here.
            drop(unsafe { Box::from_raw(old) });
        }
    }
    /// Patches the callback has taken, and how many of those it refused
    /// because they did not match its current graph.
    pub fn outcomes(&self) -> (u64, u64) {
        (
            self.shared.live_taken.load(Ordering::Acquire),
            self.shared.live_rejected.load(Ordering::Acquire),
        )
    }
    /// Releases the patch the callback handed back (holding replaced values).
    pub fn reclaim(&self) {
        let ptr = self
            .shared
            .live_retired
            .swap(std::ptr::null_mut(), Ordering::AcqRel);
        if !ptr.is_null() {
            drop(unsafe { Box::from_raw(ptr) });
        }
    }
}
/// Hardware callback owns this exclusively. Construct and destroy off-callback.
pub struct AudioHost {
    writes: [Option<RuntimeLease>; MAX_WRITE_SLOTS],
    preview: crate::preview::PreviewMixer,
    shared: Arc<Shared>,
    current: *mut PreparedGraph,
    transition: *mut PreparedGraph,
    waiting: *mut PreparedGraph,
    rejected_handoff: u64,
    rejection_count: u64,
    transition_remaining: usize,
    gain: f32,
    run_epoch: u64,
    output_boundary: u64,
    last_running: bool,
    old_left: [f32; BLOCK],
    old_right: [f32; BLOCK],
    /// Play command whose scheduled delay was already taken, and frames left.
    delay_command: u64,
    delay: u64,
}
// Pointers represent unique graph ownership transferred by release/acquire slots.
// AudioHost is movable between threads but deliberately not Sync.
unsafe impl Send for AudioHost {}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Telemetry {
    pub frame: u64,
    /// Unwrapped graph-run clock; frame remains the wrapped timeline position.
    pub playback_clock: u64,
    pub command_sequence: u64,
    pub playing: bool,
    pub graph_generation: u64,
    pub handoff_token: u64,
    pub callback_count: u64,
    pub faults: u64,
    pub rejected_handoff: u64,
    pub rejection_count: u64,
}
pub fn channel() -> (GraphControl, AudioHost) {
    let shared = Arc::new(Shared {
        peak_left: AtomicU32::new(0),
        peak_right: AtomicU32::new(0),
        output_available: AtomicBool::new(true),
        output_boundary: AtomicU64::new(0),
        anchor_output_boundary: AtomicU64::new(0),
        writes: std::array::from_fn(|_| Slot::default()),
        pending: AtomicPtr::new(std::ptr::null_mut()),
        retired: AtomicPtr::new(std::ptr::null_mut()),
        command: AtomicU64::new(0),
        version: AtomicU64::new(0),
        position: AtomicU64::new(0),
        playback_clock: AtomicU64::new(0),
        acknowledged: AtomicU64::new(0),
        graph_generation: AtomicU64::new(0),
        handoff_token: AtomicU64::new(0),
        callback_count: AtomicU64::new(0),
        faults: AtomicU64::new(0),
        cancel_handoff: AtomicU64::new(0),
        rejected_handoff: AtomicU64::new(0),
        rejection_count: AtomicU64::new(0),
        anchor_sample: AtomicU64::new(0),
        anchor_host: AtomicU64::new(0),
        anchor_flags: AtomicU64::new(0),
        anchor_clock: AtomicU64::new(0),
        anchor_frames: AtomicU64::new(0),
        anchor_epoch: AtomicU64::new(0),
        start_delay: AtomicU64::new(0),
        metronome_beat: AtomicU64::new(0),
        metronome_meter: AtomicU32::new(4),
        live: AtomicPtr::new(std::ptr::null_mut()),
        live_retired: AtomicPtr::new(std::ptr::null_mut()),
        live_taken: AtomicU64::new(0),
        live_rejected: AtomicU64::new(0),
    });
    let (preview_control, preview_mixer) = crate::preview::channel();
    (
        GraphControl {
            shared: shared.clone(),
            preview: preview_control,
            next_handoff: 1,
            next_write: 1,
            writes: [None; MAX_WRITE_SLOTS],
        },
        AudioHost {
            writes: [None; MAX_WRITE_SLOTS],
            preview: preview_mixer,
            shared,
            current: std::ptr::null_mut(),
            transition: std::ptr::null_mut(),
            waiting: std::ptr::null_mut(),
            rejected_handoff: 0,
            rejection_count: 0,
            transition_remaining: 0,
            gain: 0.,
            run_epoch: 0,
            output_boundary: 0,
            last_running: false,
            old_left: [0.; BLOCK],
            old_right: [0.; BLOCK],
            delay_command: 0,
            delay: 0,
        },
    )
}
/// Native lifecycle owner may invalidate an anchor without touching callback DSP.
/// No musical command is acknowledged or clock advanced by these notifications.
#[cfg(target_os = "macos")]
#[derive(Clone)]
pub(crate) struct OutputLifecycleControl {
    shared: Arc<Shared>,
}
#[cfg(target_os = "macos")]
impl OutputLifecycleControl {
    pub fn attach(&self) {
        self.shared.output_available.store(false, Ordering::SeqCst);
        if self.shared.callback_count.load(Ordering::SeqCst) != 0 {
            self.bump_boundary();
        }
        self.shared.anchor_flags.store(0, Ordering::SeqCst);
    }
    fn bump_boundary(&self) {
        let _ = self
            .shared
            .output_boundary
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_add(1));
    }
    pub fn unavailable(&self) {
        if self.shared.output_available.swap(false, Ordering::SeqCst) {
            self.bump_boundary();
        }
        self.shared.anchor_flags.store(0, Ordering::SeqCst);
    }
    #[cfg(test)]
    pub(crate) fn exhaust_boundary_for_test(&self) {
        self.shared
            .output_boundary
            .store(u64::MAX, Ordering::SeqCst);
    }
    /// An exhausted boundary counter could no longer distinguish output runs.
    pub fn can_start(&self) -> bool {
        self.shared.output_boundary.load(Ordering::SeqCst) != u64::MAX
    }
    /// Re-admits anchors after a native start. Does not bump the boundary: the
    /// preceding unavailable() already separated this run from the last one.
    pub fn available(&self) {
        self.shared.output_available.store(true, Ordering::SeqCst);
    }
}
impl GraphControl {
    /// Control worker submits only a worker-resolved target. The callback rechecks
    /// generation/token/epoch/playing; this request is not a Begin receipt.
    /// Reserve a bounded lease against actual playing callback telemetry. This
    /// queues a Begin; only a later Began receipt proves callback acceptance.
    /// Beginning never overrides or authors a sample. Requires valid hardware
    /// timestamp fields; caller-driven clocks are not a substitute.
    pub fn begin_write(
        &mut self,
        gesture: u128,
        target: ResolvedWriteTarget,
    ) -> Result<WriteLease, WriteError> {
        if gesture == 0 {
            return Err(WriteError::StaleLease);
        }
        let telemetry = self.telemetry().ok_or(WriteError::Unavailable)?;
        let anchor = self.output_anchor().ok_or(WriteError::Unavailable)?;
        if !telemetry.playing || telemetry.graph_generation != target.graph_generation() {
            return Err(WriteError::Unavailable);
        }
        if self
            .writes
            .iter()
            .flatten()
            .any(|w| w.lease.cookie == gesture || w.target == target)
        {
            return Err(WriteError::Busy);
        }
        let index = self
            .writes
            .iter()
            .position(Option::is_none)
            .ok_or(WriteError::Exhausted)?;
        let nonce = self.next_write;
        let next = nonce.checked_add(1).ok_or(WriteError::Exhausted)?;
        let lease = WriteLease {
            slot: index as u8,
            cookie: gesture,
            nonce,
        };
        self.shared.writes[index].publish(Message::Begin {
            cookie: gesture,
            nonce,
            target,
            token: telemetry.handoff_token,
            epoch: anchor.epoch,
        })?;
        self.next_write = next;
        self.writes[index] = Some(ControlLease {
            lease,
            target,
            sequence: 0,
            closing: false,
            final_sequence: None,
        });
        Ok(lease)
    }
    /// Publish/coalesce only an unclaimed Value. Applied receipts have reserved
    /// storage and cannot be overwritten by a faster control publisher. Busy
    /// means consume/retry boundedly on the control worker, never the callback.
    pub fn write_value(
        &mut self,
        lease: WriteLease,
        sequence: u64,
        value: f32,
    ) -> Result<WriteSubmission, WriteError> {
        if !value.is_finite() || !(0. ..=1.).contains(&value) {
            return Err(WriteError::InvalidValue);
        }
        let row = self
            .writes
            .get_mut(lease.slot as usize)
            .and_then(Option::as_mut)
            .filter(|r| r.lease == lease)
            .ok_or(WriteError::StaleLease)?;
        if row.closing
            && row
                .final_sequence
                .map_or(true, |final_sequence| sequence > final_sequence)
        {
            return Err(WriteError::StaleLease);
        }
        if sequence == 0 || sequence <= row.sequence {
            return Err(WriteError::InvalidSequence);
        }
        let receipt = self.shared.writes[lease.slot as usize].publish(Message::Value {
            cookie: lease.cookie,
            nonce: lease.nonce,
            sequence,
            value,
        })?;
        row.sequence = sequence;
        if row.final_sequence == Some(sequence) {
            self.shared.writes[lease.slot as usize].request_release(ReleaseReason::Finished);
        }
        Ok(receipt)
    }
    /// Finished allows priority End to overtake Values through final_sequence.
    /// The host must bound that wait by its lease deadline and explicitly abort
    /// when the final Value never arrives. Other reasons release regardless of
    /// a stale final_sequence. None of these requests is an audio receipt.
    pub fn end_write(
        &mut self,
        lease: WriteLease,
        final_sequence: u64,
        reason: ReleaseReason,
    ) -> Result<(), WriteError> {
        let row = self
            .writes
            .get_mut(lease.slot as usize)
            .and_then(Option::as_mut)
            .filter(|r| r.lease == lease)
            .ok_or(WriteError::StaleLease)?;
        if reason == ReleaseReason::Finished
            && (final_sequence < row.sequence
                || row.final_sequence.is_some_and(|f| f != final_sequence))
        {
            return Err(WriteError::InvalidSequence);
        }
        row.closing = true;
        if reason == ReleaseReason::Finished {
            row.final_sequence = Some(final_sequence);
            if row.sequence == final_sequence {
                self.shared.writes[lease.slot as usize].request_release(reason);
            }
        } else {
            row.final_sequence = None;
            self.shared.writes[lease.slot as usize].request_release(reason);
        }
        Ok(())
    }
    /// Consume actual callback receipts in order: Applied/Rejected, Restoring,
    /// Released. Do not turn an unapplied/superseded request into an authored
    /// point. A terminal receipt retires the opaque nonce-bound lease.
    pub fn poll_write(&mut self, lease: WriteLease) -> Result<Option<WriteReceipt>, WriteError> {
        self.writes
            .get(lease.slot as usize)
            .and_then(Option::as_ref)
            .filter(|r| r.lease == lease)
            .ok_or(WriteError::StaleLease)?;
        let slot = &self.shared.writes[lease.slot as usize];
        let receipt = slot.poll();
        if matches!(
            receipt,
            Some(WriteReceipt::Released { .. })
                | Some(WriteReceipt::Rejected { sequence: None, .. })
        ) {
            slot.reset();
            self.writes[lease.slot as usize] = None;
        }
        Ok(receipt)
    }
    /// Returns the graph when a pending handoff already exists; successful
    /// publication returns a token acknowledged only by the callback.
    pub fn publish(
        &mut self,
        graph: Box<PreparedGraph>,
    ) -> std::result::Result<u64, Box<PreparedGraph>> {
        self.publish_inner(graph, None, 0)
    }
    /// Live edits must be warmed to a future source frame. The callback splices
    /// there sample-accurately, or rejects a candidate that arrived too late.
    pub fn publish_aligned(
        &mut self,
        graph: Box<PreparedGraph>,
        frame: u64,
    ) -> std::result::Result<u64, Box<PreparedGraph>> {
        if graph.position() != frame || graph.loop_range().is_some() {
            return Err(graph);
        }
        let Some(state) = self.telemetry() else {
            return Err(graph);
        };
        self.publish_aligned_on(graph, frame, state.handoff_token)
    }
    /// Sample-aligned live update bound to the actual baseline callback token.
    /// The worker must prepare the candidate to the exact unwrapped clock,
    /// including continuous loop history. Late/stale/range-changed edits reject.
    pub fn publish_aligned_on(
        &mut self,
        graph: Box<PreparedGraph>,
        clock: u64,
        expected_handoff: u64,
    ) -> std::result::Result<u64, Box<PreparedGraph>> {
        if expected_handoff == 0 || graph.playback_clock() != clock {
            return Err(graph);
        }
        self.publish_inner(graph, Some(clock), expected_handoff)
    }
    fn publish_inner(
        &mut self,
        mut graph: Box<PreparedGraph>,
        boundary: Option<u64>,
        baseline: u64,
    ) -> std::result::Result<u64, Box<PreparedGraph>> {
        if !graph.has_clean_history() {
            return Err(graph);
        }
        graph.assign_output_boundary(self.shared.output_boundary.load(Ordering::SeqCst));
        graph.assign_handoff_boundary(boundary);
        graph.assign_handoff_baseline(baseline);
        let Some(next) = self.next_handoff.checked_add(1) else {
            return Err(graph);
        };
        graph.assign_handoff_token(self.next_handoff);
        let ptr = Box::into_raw(graph);
        match self.shared.pending.compare_exchange(
            std::ptr::null_mut(),
            ptr,
            Ordering::Release,
            Ordering::Relaxed,
        ) {
            Ok(_) => {
                let token = self.next_handoff;
                self.next_handoff = next;
                Ok(token)
            }
            Err(_) => Err(unsafe { Box::from_raw(ptr) }),
        }
    }
    /// Cancellation is callback-acknowledged through rejected_handoff. A graph
    /// already adopted is never destroyed or rolled back by this request.
    pub fn cancel_handoff(&mut self, token: u64) {
        self.shared.cancel_handoff.store(token, Ordering::Release);
    }
    /// Reclaims exactly one predecessor on the worker; never call on a callback.
    /// A handle that publishes [`LivePatch`]es from any control thread.
    pub fn live_publisher(&self) -> LivePublisher {
        LivePublisher {
            shared: self.shared.clone(),
        }
    }
    pub fn reclaim_retired(&mut self) -> bool {
        self.live_publisher().reclaim();
        let ptr = self
            .shared
            .retired
            .swap(std::ptr::null_mut(), Ordering::AcqRel);
        if ptr.is_null() {
            false
        } else {
            unsafe {
                drop(Box::from_raw(ptr));
            }
            true
        }
    }
    /// Latest desired transport; acknowledgement is only published by rendering.
    /// A request is not proof the backend has accepted or started playback.
    pub fn request_playing(&mut self, playing: bool) -> Option<u64> {
        let old = self.shared.command.load(Ordering::Relaxed);
        let next = (old & !1).checked_add(2)? | u64::from(playing);
        if !playing {
            for (index, row) in self.writes.iter_mut().enumerate() {
                if let Some(row) = row {
                    row.closing = true;
                    row.final_sequence = None;
                    self.shared.writes[index].request_release(ReleaseReason::Stopped);
                }
            }
        }
        self.shared.command.store(next, Ordering::Release);
        Some(next >> 1)
    }
    /// Actual backend callback timestamp, not a synthesized UI/transport clock.
    /// Polling may skip anchors; a recording worker must validate continuity and
    /// freshness against packet host times. Paused/untimed callbacks return None.
    pub fn output_anchor(&self) -> Option<OutputClockAnchor> {
        if !self.shared.output_available.load(Ordering::SeqCst) {
            return None;
        }
        for _ in 0..4 {
            let version = self.shared.version.load(Ordering::SeqCst);
            if version & 1 != 0 {
                continue;
            }
            let flags = self.shared.anchor_flags.load(Ordering::SeqCst) as u32;
            let anchor = OutputClockAnchor {
                epoch: self.shared.anchor_epoch.load(Ordering::SeqCst),
                playback_clock: self.shared.anchor_clock.load(Ordering::SeqCst),
                frames: self.shared.anchor_frames.load(Ordering::SeqCst) as usize,
                timing: CaptureTiming {
                    sample_time: f64::from_bits(self.shared.anchor_sample.load(Ordering::SeqCst)),
                    host_time: self.shared.anchor_host.load(Ordering::SeqCst),
                    flags,
                },
            };
            if self.shared.version.load(Ordering::SeqCst) == version {
                return (flags & 3 == 3
                    && anchor.frames != 0
                    && anchor.epoch != 0
                    && self.shared.output_available.load(Ordering::SeqCst)
                    && self.shared.anchor_output_boundary.load(Ordering::SeqCst)
                        == self.shared.output_boundary.load(Ordering::SeqCst))
                .then_some(anchor);
            }
        }
        None
    }
    /// Output peak magnitudes since the previous call (meters only; not a clock).
    /// Preview voices mixed after the transport graph.
    pub fn preview(&self) -> crate::preview::PreviewControl {
        self.preview.clone()
    }
    /// Sample-accurate count-in for the next Play.
    pub fn count_in(&self) -> CountIn {
        CountIn {
            shared: self.shared.clone(),
            preview: self.preview.clone(),
        }
    }
    pub fn metronome(&self) -> Metronome {
        Metronome {
            shared: self.shared.clone(),
        }
    }
    pub fn take_peaks(&self) -> (f32, f32) {
        (
            f32::from_bits(self.shared.peak_left.swap(0, Ordering::AcqRel)),
            f32::from_bits(self.shared.peak_right.swap(0, Ordering::AcqRel)),
        )
    }
    /// Coherent bounded read; callers retry on their next telemetry tick.
    pub fn telemetry(&self) -> Option<Telemetry> {
        for _ in 0..4 {
            let a = self.shared.version.load(Ordering::SeqCst);
            if a & 1 != 0 {
                continue;
            }
            let command = self.shared.acknowledged.load(Ordering::SeqCst);
            let value = Telemetry {
                frame: self.shared.position.load(Ordering::SeqCst),
                playback_clock: self.shared.playback_clock.load(Ordering::SeqCst),
                command_sequence: command >> 1,
                playing: command & 1 != 0,
                graph_generation: self.shared.graph_generation.load(Ordering::SeqCst),
                handoff_token: self.shared.handoff_token.load(Ordering::SeqCst),
                callback_count: self.shared.callback_count.load(Ordering::SeqCst),
                faults: self.shared.faults.load(Ordering::SeqCst),
                rejected_handoff: self.shared.rejected_handoff.load(Ordering::SeqCst),
                rejection_count: self.shared.rejection_count.load(Ordering::SeqCst),
            };
            if self.shared.version.load(Ordering::SeqCst) == a {
                return Some(value);
            }
        }
        None
    }
}
impl AudioHost {
    #[cfg(target_os = "macos")]
    pub(crate) fn output_lifecycle_control(&self) -> OutputLifecycleControl {
        OutputLifecycleControl {
            shared: Arc::clone(&self.shared),
        }
    }
    /// Output teardown only, after the native gate proves no callback is active
    /// or can be admitted. Open leases end with a Discontinuity at the actual
    /// last rendered clock of their own epoch; unclaimed Begins are rejected.
    #[cfg(target_os = "macos")]
    pub(crate) fn release_output_writes(&mut self) {
        for index in 0..MAX_WRITE_SLOTS {
            if self.writes[index].is_some() {
                let epoch = self.writes[index].map_or(self.run_epoch, |row| row.epoch);
                let saved = std::mem::replace(&mut self.run_epoch, epoch);
                self.freeze_write(index, ReleaseReason::Discontinuity);
                self.run_epoch = saved;
                self.flush_terminal(index);
            } else if let Some(message) = self.shared.writes[index].claim() {
                let (gesture, sequence) = match message {
                    Message::Begin { cookie, .. } => (cookie, None),
                    Message::Value {
                        cookie, sequence, ..
                    } => (cookie, Some(sequence)),
                };
                self.shared.writes[index].applied(WriteReceipt::Rejected {
                    gesture,
                    sequence,
                    error: WriteError::Unavailable,
                });
            }
        }
    }
    fn write_stamp(&self, target: ResolvedWriteTarget, epoch: u64) -> WriteStamp {
        let (frame, clock) = if self.current.is_null() {
            (0, 0)
        } else {
            let graph = unsafe { &*self.current };
            (graph.position(), graph.playback_clock())
        };
        WriteStamp {
            transport_epoch: epoch,
            output_frame: frame,
            control_frame: clock.saturating_add(target.latency_offset_frames()),
            playback_clock: clock,
            latency_offset_frames: target.latency_offset_frames(),
        }
    }
    fn freeze_write(&mut self, index: usize, reason: ReleaseReason) {
        let Some(mut row) = self.writes[index] else {
            return;
        };
        self.shared.writes[index].request_release(reason);
        if row.terminal.is_some() {
            return;
        }
        let stamp = self.write_stamp(
            row.target,
            if reason == ReleaseReason::Discontinuity {
                self.run_epoch
            } else {
                row.epoch
            },
        );
        let last = if self.current.is_null() {
            None
        } else {
            unsafe { &mut *self.current }.clear_write(row.target, index as u8)
        };
        row.terminal = Some(WriteReceipt::Released {
            gesture: row.cookie,
            reason,
            stamp,
            last_applied: last,
        });
        row.touched = false;
        self.writes[index] = Some(row);
    }
    fn flush_terminal(&mut self, index: usize) -> bool {
        let Some(row) = self.writes[index] else {
            return false;
        };
        let Some(receipt) = row.terminal else {
            return false;
        };
        let slot = &self.shared.writes[index];
        if slot.editing() {
            return true;
        }
        if let Some(message) = slot.claim() {
            let sequence = match message {
                Message::Value { sequence, .. } => Some(sequence),
                Message::Begin { .. } => None,
            };
            slot.applied(WriteReceipt::Rejected {
                gesture: row.cookie,
                sequence,
                error: WriteError::StaleLease,
            });
        }
        self.writes[index] = None;
        slot.terminal(receipt);
        true
    }
    fn process_writes(&mut self, playing: bool, timed: bool) {
        for index in 0..MAX_WRITE_SLOTS {
            if self.flush_terminal(index) {
                continue;
            }
            if let Some(row) = self.writes[index] {
                let graph = if self.current.is_null() {
                    None
                } else {
                    Some(unsafe { &*self.current })
                };
                let valid = timed
                    && row.epoch == self.run_epoch
                    && graph.is_some_and(|g| {
                        g.generation() == row.target.graph_generation()
                            && g.handoff_token() == row.token
                            && g.loop_range().is_none()
                    });
                if !valid {
                    self.freeze_write(index, ReleaseReason::Discontinuity);
                    self.flush_terminal(index);
                    continue;
                }
                if !playing {
                    self.shared.writes[index].request_release(ReleaseReason::Stopped);
                }
            }
            let reason = self.shared.writes[index].release_reason();
            if let Some(message) = self.shared.writes[index].claim() {
                match message {
                    Message::Begin {
                        cookie,
                        nonce,
                        target,
                        token,
                        epoch,
                    } => {
                        let valid = playing
                            && timed
                            && reason.is_none()
                            && epoch == self.run_epoch
                            && !self.current.is_null()
                            && {
                                let graph = unsafe { &*self.current };
                                graph.generation() == target.graph_generation()
                                    && graph.handoff_token() == token
                                    && graph.loop_range().is_none()
                            };
                        if valid {
                            self.writes[index] = Some(RuntimeLease {
                                cookie,
                                nonce,
                                target,
                                token,
                                epoch,
                                reason: None,
                                restoring: false,
                                touched: false,
                                terminal: None,
                            });
                            self.shared.writes[index].applied(WriteReceipt::Began {
                                gesture: cookie,
                                stamp: self.write_stamp(target, epoch),
                            });
                        } else {
                            self.shared.writes[index].applied(WriteReceipt::Rejected {
                                gesture: cookie,
                                sequence: None,
                                error: WriteError::Unavailable,
                            });
                        }
                    }
                    Message::Value {
                        cookie,
                        nonce,
                        sequence,
                        value,
                    } => {
                        let allowed =
                            playing && reason.map_or(true, |r| r == ReleaseReason::Finished);
                        if let Some(mut row) = self.writes[index].filter(|r| {
                            r.cookie == cookie
                                && r.nonce == nonce
                                && r.terminal.is_none()
                                && !r.restoring
                                && allowed
                        }) {
                            let ramp = unsafe { &mut *self.current }.apply_write(
                                row.target,
                                index as u8,
                                value,
                                false,
                            );
                            if let Some(ramp) = ramp {
                                row.touched = true;
                                self.writes[index] = Some(row);
                                self.shared.writes[index].applied(WriteReceipt::Applied {
                                    gesture: cookie,
                                    sequence,
                                    stamp: self.write_stamp(row.target, row.epoch),
                                    ramp,
                                });
                            } else {
                                self.shared.writes[index].applied(WriteReceipt::Rejected {
                                    gesture: cookie,
                                    sequence: Some(sequence),
                                    error: WriteError::Unavailable,
                                });
                            }
                        } else {
                            self.shared.writes[index].applied(WriteReceipt::Rejected {
                                gesture: cookie,
                                sequence: Some(sequence),
                                error: WriteError::StaleLease,
                            });
                        }
                    }
                }
            }
            let Some(mut row) = self.writes[index] else {
                continue;
            };
            if let Some(reason) = reason {
                row.reason = Some(reason);
                self.writes[index] = Some(row);
                if !row.touched {
                    self.freeze_write(index, reason);
                    self.flush_terminal(index);
                    continue;
                }
                let settled = unsafe { &mut *self.current }
                    .write_progress(row.target, index as u8)
                    .is_some_and(|p| p.0);
                if !row.restoring && (reason != ReleaseReason::Finished || settled) {
                    if let Some(ramp) =
                        unsafe { &mut *self.current }.apply_write(row.target, index as u8, 0., true)
                    {
                        row.restoring = true;
                        self.writes[index] = Some(row);
                        self.shared.writes[index].restoring(WriteReceipt::Restoring {
                            gesture: row.cookie,
                            reason,
                            stamp: self.write_stamp(row.target, row.epoch),
                            ramp,
                        });
                    } else {
                        self.freeze_write(index, reason);
                        self.flush_terminal(index);
                    }
                }
            }
        }
    }
    fn finish_writes(&mut self, paused: bool) {
        for index in 0..MAX_WRITE_SLOTS {
            let Some(row) = self.writes[index] else {
                continue;
            };
            if let Some(reason) = row.reason {
                let done = row.restoring
                    && !self.current.is_null()
                    && unsafe { &mut *self.current }
                        .write_progress(row.target, index as u8)
                        .is_some_and(|p| p.0);
                if done || paused {
                    self.freeze_write(index, reason);
                    self.flush_terminal(index);
                }
            }
        }
    }
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) -> CallbackStatus {
        self.render_timed(left, right, CaptureTiming::default())
    }
    pub fn render_timed(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        timing: CaptureTiming,
    ) -> CallbackStatus {
        if left.len() != right.len() || left.len() > BLOCK {
            left.fill(0.);
            right.fill(0.);
            return CallbackStatus::InvalidBlock;
        }
        if left.is_empty() {
            return CallbackStatus::Rendered;
        }
        let output_boundary = self.shared.output_boundary.load(Ordering::SeqCst);
        if output_boundary != self.output_boundary {
            self.output_boundary = output_boundary;
            self.run_epoch = self.run_epoch.saturating_add(1);
            for i in 0..MAX_WRITE_SLOTS {
                if self.writes[i].is_some() {
                    self.freeze_write(i, ReleaseReason::Discontinuity);
                    self.flush_terminal(i);
                }
            }
        }
        let command = self.shared.command.load(Ordering::Acquire);
        if command & 1 == 0 {
            self.delay = 0;
        }
        if command & 1 != 0 && command != self.delay_command {
            self.delay_command = command;
            self.delay = self.shared.start_delay.swap(0, Ordering::AcqRel);
            if self.gain > 0. {
                // Already audible: a count-in only precedes a start from silence.
                self.delay = 0;
            }
            if self.delay > 0 {
                // The count-in voice starts on the same sample as the delay.
                self.preview.trigger(crate::preview::COUNT_IN_VOICE);
            }
        }
        let mut delayed = 0;
        left.fill(0.);
        right.fill(0.);
        let mut status = CallbackStatus::Rendered;
        let mut offset = 0;
        let mut start_clock = 0;
        let mut start_epoch = 0;
        let metronome = match self.shared.metronome_beat.load(Ordering::Acquire) {
            0 => None,
            bits => Some(crate::metronome::Grid {
                beat_frames: f64::from_bits(bits),
                beats_per_bar: self.shared.metronome_meter.load(Ordering::Acquire),
            }),
        };
        self.apply_live();
        loop {
            if command & 1 != 0 && self.delay > 0 && self.gain == 0. {
                // Keep adopting the graph published with this Play while held.
                let _ = self.consider_handoff(offset == 0);
                let n = (self.delay as usize).min(left.len() - offset);
                offset += n;
                delayed += n;
                self.delay -= n as u64;
                if offset == left.len() {
                    break;
                }
                continue;
            }
            let boundary = self.consider_handoff(offset == 0);
            let playing = command & 1 != 0 && !self.current.is_null();
            if offset == 0 {
                if playing && self.gain == 0. && !self.last_running {
                    self.run_epoch = self.run_epoch.saturating_add(1);
                }
                start_clock = if self.current.is_null() {
                    0
                } else {
                    unsafe { &*self.current }.playback_clock()
                };
                start_epoch = self.run_epoch;
            }
            self.process_writes(
                playing,
                timing.flags & 3 == 3 && timing.host_time != 0 && timing.sample_time.is_finite(),
            );
            let write_active = self.writes.iter().any(Option::is_some)
                || self.shared.writes.iter().any(Slot::pending);
            let available = (left.len() - offset).min(if write_active { 256 } else { BLOCK });
            let available = self
                .writes
                .iter()
                .enumerate()
                .filter_map(|(index, row)| {
                    let row = row.as_ref()?;
                    if row.restoring || row.reason == Some(ReleaseReason::Finished) {
                        let frames = unsafe { &mut *self.current }
                            .write_remaining(row.target, index as u8)?;
                        (frames != 0).then_some(frames as usize)
                    } else {
                        None
                    }
                })
                .fold(available, usize::min);
            let active = if playing {
                available
            } else {
                available.min((self.gain * FADE as f32).ceil() as usize)
            };
            if active == 0 {
                self.finish_writes(true);
                self.retire_transition();
                break;
            }
            let frame = unsafe { &*self.current }.playback_clock();
            let count = boundary.map_or(active, |target| {
                active.min(usize::try_from(target - frame).unwrap_or(usize::MAX))
            });
            let l = &mut left[offset..offset + count];
            let r = &mut right[offset..offset + count];
            let first = unsafe { &*self.current }.position();
            let wrap = unsafe { &*self.current }
                .loop_range()
                .map(|range| (range, first));
            let rendered = unsafe { &mut *self.current }.render_callback(l, r);
            if rendered != CallbackStatus::Rendered {
                status = rendered;
            }
            if !self.transition.is_null() {
                let old_count = count.min(self.transition_remaining);
                let old = unsafe { &mut *self.transition }.render_callback(
                    &mut self.old_left[..old_count],
                    &mut self.old_right[..old_count],
                );
                if old != CallbackStatus::Rendered {
                    status = old;
                }
                for i in 0..old_count {
                    let mix = (FADE - self.transition_remaining + i + 1) as f32 / FADE as f32;
                    l[i] = self.old_left[i] * (1. - mix) + l[i] * mix;
                    r[i] = self.old_right[i] * (1. - mix) + r[i] * mix;
                }
                self.transition_remaining -= old_count;
                if self.transition_remaining == 0 {
                    self.retire_transition();
                }
            }
            if let Some((range, first)) = wrap {
                for i in 0..count {
                    let gain = loop_wrap_gain(range, first + i as u64);
                    l[i] *= gain;
                    r[i] *= gain;
                }
            }
            if let Some(grid) = metronome {
                for i in 0..count {
                    let mut frame = first + i as u64;
                    if let Some((range, _)) = wrap.filter(|(range, _)| frame >= range.end) {
                        frame = range.start + (frame - range.start) % (range.end - range.start);
                    }
                    let click = grid.sample(frame as f64);
                    l[i] += click;
                    r[i] += click;
                }
            }
            for i in 0..count {
                self.gain = if playing {
                    (self.gain + 1. / FADE as f32).min(1.)
                } else {
                    (self.gain - 1. / FADE as f32).max(0.)
                };
                l[i] *= self.gain;
                r[i] *= self.gain;
            }
            offset += count;
            self.finish_writes(!playing && self.gain == 0.);
            if offset == left.len() {
                break;
            }
        }
        if matches!(
            status,
            CallbackStatus::SourceFault | CallbackStatus::InvalidBlock
        ) {
            left.fill(0.);
            right.fill(0.);
        }
        self.preview.mix(left, right);
        let peak = |samples: &[f32]| {
            samples
                .iter()
                .fold(0f32, |m, v| if v.is_finite() { m.max(v.abs()) } else { m })
                .to_bits()
        };
        self.shared
            .peak_left
            .fetch_max(peak(left), Ordering::AcqRel);
        self.shared
            .peak_right
            .fetch_max(peak(right), Ordering::AcqRel);
        let playing = command & 1 != 0 && !self.current.is_null();
        self.last_running = playing;
        self.shared.version.fetch_add(1, Ordering::SeqCst);
        self.shared
            .anchor_sample
            .store(timing.sample_time.to_bits(), Ordering::SeqCst);
        self.shared
            .anchor_host
            .store(timing.host_time, Ordering::SeqCst);
        self.shared.anchor_flags.store(
            if timing.sample_time.is_finite()
                && self.shared.output_available.load(Ordering::SeqCst)
                && output_boundary == self.shared.output_boundary.load(Ordering::SeqCst)
            {
                u64::from(timing.flags & 3)
            } else {
                0
            },
            Ordering::SeqCst,
        );
        self.shared
            .anchor_output_boundary
            .store(output_boundary, Ordering::SeqCst);
        self.shared
            .anchor_clock
            .store(start_clock, Ordering::SeqCst);
        self.shared
            .anchor_frames
            // A block that began with scheduled silence has no graph-aligned anchor.
            .store(
                if delayed > 0 { 0 } else { offset as u64 },
                Ordering::SeqCst,
            );
        self.shared
            .anchor_epoch
            .store(start_epoch, Ordering::SeqCst);
        let frame = if self.current.is_null() {
            0
        } else {
            unsafe { &*self.current }.position()
        };
        let generation = if self.current.is_null() {
            0
        } else {
            unsafe { &*self.current }.generation()
        };
        let handoff = if self.current.is_null() {
            0
        } else {
            unsafe { &*self.current }.handoff_token()
        };
        self.shared
            .rejected_handoff
            .store(self.rejected_handoff, Ordering::SeqCst);
        self.shared
            .rejection_count
            .store(self.rejection_count, Ordering::SeqCst);
        self.shared.handoff_token.store(handoff, Ordering::SeqCst);
        self.shared.position.store(frame, Ordering::SeqCst);
        self.shared.playback_clock.store(
            if self.current.is_null() {
                0
            } else {
                unsafe { &*self.current }.playback_clock()
            },
            Ordering::SeqCst,
        );
        self.shared
            .graph_generation
            .store(generation, Ordering::SeqCst);
        // Missing graph cannot acknowledge a requested Play.
        let acknowledged = if (command & 1 != 0 && !playing) || (command & 1 == 0 && self.gain > 0.)
        {
            self.shared.acknowledged.load(Ordering::SeqCst)
        } else {
            command
        };
        self.shared
            .acknowledged
            .store(acknowledged, Ordering::SeqCst);
        self.shared.callback_count.fetch_add(1, Ordering::SeqCst);
        if status != CallbackStatus::Rendered {
            self.shared.faults.fetch_add(1, Ordering::SeqCst);
        }
        self.shared.version.fetch_add(1, Ordering::SeqCst);
        status
    }
    fn retire_transition(&mut self) {
        if !self.transition.is_null() {
            self.shared
                .retired
                .store(self.transition, Ordering::Release);
            self.transition = std::ptr::null_mut();
        }
    }
    /// Returns a strictly future splice boundary. Pending ownership is first
    /// acquired into a private slot; workers never dereference callback pointers.
    /// Swaps a published parameter patch into the current graph at a block
    /// boundary. The patch (now holding the replaced values) goes back through
    /// the retired slot; nothing is allocated or released here.
    fn apply_live(&mut self) {
        if !self.shared.live_retired.load(Ordering::Acquire).is_null() {
            return;
        }
        let patch = self
            .shared
            .live
            .swap(std::ptr::null_mut(), Ordering::AcqRel);
        if patch.is_null() {
            return;
        }
        if self.current.is_null() || !unsafe { (*self.current).apply_live(&mut *patch) } {
            self.shared.live_rejected.fetch_add(1, Ordering::AcqRel);
        }
        self.shared.live_retired.store(patch, Ordering::Release);
        self.shared.live_taken.fetch_add(1, Ordering::AcqRel);
    }
    fn consider_handoff(&mut self, allow_immediate: bool) -> Option<u64> {
        if self.waiting.is_null() {
            self.waiting = self
                .shared
                .pending
                .swap(std::ptr::null_mut(), Ordering::AcqRel);
        }
        if self.waiting.is_null()
            || !self.transition.is_null()
            || !self.shared.retired.load(Ordering::Acquire).is_null()
        {
            return None;
        }
        let candidate = unsafe { &*self.waiting };
        let token = candidate.handoff_token();
        let boundary = candidate.handoff_boundary();
        let frame = if self.current.is_null() {
            0
        } else {
            unsafe { &*self.current }.playback_clock()
        };
        let cancelled = self.shared.cancel_handoff.load(Ordering::Acquire) == token;
        let late = boundary
            .is_some_and(|target| frame > target || (self.current.is_null() && target != 0));
        let baseline = candidate.handoff_baseline();
        let stale = baseline != 0
            && (self.current.is_null()
                || unsafe { &*self.current }.handoff_token() != baseline
                || unsafe { &*self.current }.loop_range() != candidate.loop_range());
        let stale_output =
            candidate.output_boundary() != self.shared.output_boundary.load(Ordering::SeqCst);
        if cancelled || late || stale || stale_output {
            self.rejected_handoff = token;
            self.rejection_count = self.rejection_count.saturating_add(1);
            self.shared.retired.store(self.waiting, Ordering::Release);
            self.waiting = std::ptr::null_mut();
            return None;
        }
        if let Some(target) = boundary {
            if frame < target {
                return Some(target);
            }
        }
        if boundary.is_none() {
            // An immediate seek starts at a hardware callback boundary so one
            // timestamp never describes two unrelated session clock domains.
            if !allow_immediate {
                return None;
            }
            self.run_epoch = self.run_epoch.saturating_add(1);
        }
        for index in 0..MAX_WRITE_SLOTS {
            if self.writes[index].is_some() {
                self.freeze_write(index, ReleaseReason::Discontinuity);
                self.flush_terminal(index);
            }
        }
        self.transition = self.current;
        self.current = self.waiting;
        self.waiting = std::ptr::null_mut();
        self.transition_remaining = if self.transition.is_null() { 0 } else { FADE };
        None
    }
}
impl Drop for AudioHost {
    fn drop(&mut self) {
        for ptr in [self.current, self.transition, self.waiting] {
            if !ptr.is_null() {
                unsafe {
                    drop(Box::from_raw(ptr));
                }
            }
        }
    }
}

#[cfg(test)]
mod count_in_tests {
    use super::*;
    #[test]
    fn loop_wrap_dips_smoothly_through_silence() {
        let range = LoopRange::new(1000, 49000).unwrap();
        let gains: Vec<f32> = (47000..51000).map(|f| loop_wrap_gain(range, f)).collect();
        // Fully open away from the wrap, near silent across it, no steps.
        assert_eq!(gains[0], 1.);
        assert!(gains[1999] < 0.001 && gains[2000] < 0.001);
        assert!(gains.windows(2).all(|w| (w[1] - w[0]).abs() < 0.03));
        assert_eq!(gains[2000 + 96], 1.);
        // Loops shorter than four fades use a quarter of their length.
        let short = LoopRange::new(0, 100).unwrap();
        assert_eq!(loop_wrap_gain(short, 50), 1.);
        assert!(loop_wrap_gain(short, 0) < 0.05 && loop_wrap_gain(short, 99) < 0.05);
    }
    #[test]
    fn armed_count_in_starts_with_the_play_command_and_holds_the_graph() {
        let (mut control, mut host) = channel();
        control.count_in().arm(crate::preview::Buffer {
            left: vec![1.; 300],
            right: vec![1.; 300],
            looping: false,
            gain: 1.,
            ..Default::default()
        });
        let (mut left, mut right) = (vec![0.; 256], vec![0.; 256]);
        host.render(&mut left, &mut right);
        assert!(left.iter().all(|v| *v == 0.), "armed clicks wait for Play");
        control.request_playing(true);
        host.render(&mut left, &mut right);
        assert!(
            left.iter().all(|v| *v == 1.),
            "clicks start on the Play block's first sample"
        );
        assert_eq!(host.delay, 300 - 256);
        host.render(&mut left, &mut right);
        assert!(left[..44].iter().all(|v| *v == 1.) && left[44..].iter().all(|v| *v == 0.));
        assert_eq!(host.delay, 0);
    }
    #[test]
    fn metronome_follows_the_playing_timeline_and_switches_live() {
        let project = crate::project::Project {
            session: Default::default(),
            media: Default::default(),
        };
        let graph = crate::render::PreparedGraph::prepare_resident(
            &project,
            1 << 20,
            &AtomicBool::new(false),
        )
        .unwrap();
        let (mut control, mut host) = channel();
        assert!(control.publish(Box::new(graph)).is_ok());
        let metronome = control.metronome();
        metronome.set(Some(crate::metronome::Grid::new(120., 4, 4)));
        let (mut left, mut right) = (vec![0.; 4096], vec![0.; 4096]);
        host.render(&mut left, &mut right);
        assert!(left.iter().all(|v| *v == 0.), "stopped transport is silent");
        control.request_playing(true);
        let mut rendered = vec![];
        for _ in 0..8 {
            host.render(&mut left, &mut right);
            rendered.extend_from_slice(&left);
        }
        assert_eq!(left, right);
        assert!(rendered[..1200].iter().any(|v| *v != 0.));
        assert!(rendered[1200..24000].iter().all(|v| *v == 0.));
        assert!(rendered[24000..25200].iter().any(|v| *v != 0.));
        metronome.set(None);
        for _ in 0..8 {
            host.render(&mut left, &mut right);
            assert!(left.iter().all(|v| *v == 0.), "switched off while playing");
        }
    }
    #[test]
    fn cancelled_count_in_does_not_delay_a_later_play() {
        let (mut control, mut host) = channel();
        control.count_in().arm(crate::preview::Buffer {
            left: vec![1.; 10000],
            right: vec![1.; 10000],
            ..Default::default()
        });
        control.request_playing(true);
        let (mut left, mut right) = ([0.; 128], [0.; 128]);
        host.render(&mut left, &mut right);
        assert!(host.delay > 0);
        control.count_in().cancel();
        control.request_playing(false);
        host.render(&mut left, &mut right);
        assert_eq!(host.delay, 0);
        control.request_playing(true);
        host.render(&mut left, &mut right);
        assert_eq!(host.delay, 0);
    }
}
