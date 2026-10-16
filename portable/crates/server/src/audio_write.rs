//! Control-owner adapter for the fixed callback mailbox. No network or authored
//! model work is executed by AudioHost. Host authority owns each returned prefix.
use crate::Error;
use soundroom_engine::{
    automation::Lane,
    automation_write::{
        RampReceipt, ReleaseReason, ResolvedWriteTarget, WriteError, WriteLease, WriteMapping,
        WriteReceipt, WriteStamp,
    },
    realtime::GraphControl,
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::{sync_channel, Receiver, SyncSender},
    },
    time::{Duration, Instant},
};
pub const MAX_CAPTURE: usize = 65_536;
const CAPTURE_BYTES_LIMIT: usize = 128 * 1024 * 1024;
static CAPTURE_BYTES: AtomicUsize = AtomicUsize::new(0);
const MAX_WAITERS: usize = 32;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub credential: String,
    pub runtime: String,
}
#[derive(Clone, Copy, Debug)]
pub struct Captured {
    pub sequence: Option<u64>,
    pub stamp: WriteStamp,
    pub ramp: RampReceipt,
    pub restoring: bool,
}
/// Payload admission follows the actual buffer into queued Begin, live capture,
/// prefix workers and draft serialization. Map removal cannot release its budget
/// while another worker still owns the storage.
pub struct CaptureBuffer {
    values: Vec<Captured>,
    reservation: CaptureReservation,
}
struct CaptureReservation {
    bytes: usize,
    counter: &'static AtomicUsize,
}
impl Drop for CaptureReservation {
    fn drop(&mut self) {
        self.counter.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
impl Default for CaptureBuffer {
    fn default() -> Self {
        Self {
            values: Vec::new(),
            reservation: CaptureReservation {
                bytes: 0,
                counter: &CAPTURE_BYTES,
            },
        }
    }
}
impl std::ops::Deref for CaptureBuffer {
    type Target = [Captured];
    fn deref(&self) -> &Self::Target {
        &self.values
    }
}
impl CaptureBuffer {
    fn new() -> Result<Self, Error> {
        Self::with_budget(&CAPTURE_BYTES, CAPTURE_BYTES_LIMIT)
    }
    fn with_budget(counter: &'static AtomicUsize, limit: usize) -> Result<Self, Error> {
        let bytes = MAX_CAPTURE * std::mem::size_of::<Captured>() + 4096;
        counter
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(bytes).filter(|n| *n <= limit)
            })
            .map_err(|_| Error::QueueFull)?;
        let reservation = CaptureReservation { bytes, counter };
        let mut values = Vec::new();
        values
            .try_reserve_exact(MAX_CAPTURE)
            .map_err(|_| Error::QueueFull)?;
        if values.capacity() > MAX_CAPTURE {
            return Err(Error::QueueFull);
        }
        Ok(Self {
            values,
            reservation,
        })
    }
    fn push(&mut self, value: Captured) {
        assert!(self.values.len() < self.values.capacity());
        self.values.push(value);
    }
    pub fn reserved_bytes(&self) -> usize {
        self.reservation.bytes
    }
}
pub struct Prefix {
    pub gesture: u128,
    pub reason: ReleaseReason,
    pub captured: CaptureBuffer,
    pub terminal: Option<(WriteStamp, Option<(u64, f32)>)>,
    pub failed: bool,
}
pub struct Began {
    pub mapping: WriteMapping,
    pub initial_value: f32,
    pub stamp: WriteStamp,
    pub terminal: Receiver<Prefix>,
}
#[derive(Clone, Copy, Debug)]
pub struct Applied {
    pub requested_sequence: u64,
    pub applied_sequence: u64,
    pub stamp: WriteStamp,
    pub ramp: RampReceipt,
}
type Completion<T> = SyncSender<Result<T, Error>>;
pub struct Prepared {
    target: ResolvedWriteTarget,
    mapping: WriteMapping,
    original: Lane,
    capture: CaptureBuffer,
}
impl Prepared {
    /// Called on a preparation worker. This reserves all capture storage before
    /// a control packet can reach the callback.
    pub fn new(
        target: ResolvedWriteTarget,
        mapping: WriteMapping,
        original: Lane,
    ) -> Result<Self, Error> {
        Ok(Self {
            target,
            mapping,
            original,
            capture: CaptureBuffer::new()?,
        })
    }
    /// Exact prepared DSP mapping, including the saved lane's baseline.
    pub fn mapping(&self) -> WriteMapping {
        self.mapping
    }
}
pub enum Action {
    Begin {
        owner: Owner,
        gesture: u128,
        prepared: Box<Prepared>,
        reply: Completion<Began>,
    },
    Value {
        owner: Owner,
        gesture: u128,
        sequence: u64,
        value: f32,
        reply: Completion<Applied>,
    },
    End {
        owner: Owner,
        gesture: u128,
        sequence: u64,
        reason: ReleaseReason,
        reply: Completion<()>,
    },
    ReleaseOwner {
        owner: Owner,
        reason: ReleaseReason,
    },
}
#[derive(Clone)]
pub struct Sender {
    ordinary: SyncSender<Action>,
    priority: SyncSender<Action>,
}
pub struct Driver {
    ordinary: Receiver<Action>,
    priority: Receiver<Action>,
    records: BTreeMap<u128, Record>,
}
pub fn channel() -> (Sender, Driver) {
    let (o, or) = sync_channel(32);
    let (p, pr) = sync_channel(16);
    (
        Sender {
            ordinary: o,
            priority: p,
        },
        Driver {
            ordinary: or,
            priority: pr,
            records: BTreeMap::new(),
        },
    )
}
impl Sender {
    pub fn send(&self, action: Action, priority: bool) -> Result<(), Error> {
        (if priority {
            &self.priority
        } else {
            &self.ordinary
        })
        .try_send(action)
        .map_err(|_| Error::QueueFull)
    }
}
struct Waiting {
    value: f32,
    replies: Vec<Completion<Applied>>,
}
struct Record {
    owner: Owner,
    engine: WriteLease,
    mapping: WriteMapping,
    original: Lane,
    captured: CaptureBuffer,
    began: Option<Completion<Began>>,
    terminal: Option<SyncSender<Prefix>>,
    terminal_reader: Option<Receiver<Prefix>>,
    values: BTreeMap<u64, Waiting>,
    latest: Option<(u64, f32)>,
    highest: u64,
    closing: Option<u64>,
    last_received: Instant,
    end_deadline: Option<Instant>,
    /// Reason for an owner-wide release; reported if no receipt arrives in time.
    released: Option<ReleaseReason>,
    failed: bool,
    start_clock: Option<u64>,
}
fn err(message: &str) -> Error {
    Error::Protocol(message.into())
}
fn engine_error(error: WriteError) -> Error {
    err(&format!("Audio Write mailbox: {error:?}"))
}
impl Driver {
    pub fn active(&self) -> bool {
        !self.records.is_empty()
    }
    pub fn tick(&mut self, control: &mut GraphControl) {
        for _ in 0..8 {
            let Some(action) = self
                .priority
                .try_recv()
                .ok()
                .or_else(|| self.ordinary.try_recv().ok())
            else {
                break;
            };
            self.action(control, action);
        }
        let gestures: Vec<_> = self.records.keys().copied().collect();
        for gesture in gestures {
            let mut terminal = false;
            let record = self.records.get_mut(&gesture).unwrap();
            if (!record.failed && record.last_received.elapsed() > Duration::from_secs(10))
                || record.end_deadline.is_some_and(|t| Instant::now() > t)
            {
                let reason = record.released.unwrap_or(if record.closing.is_some() {
                    ReleaseReason::Conflict
                } else {
                    ReleaseReason::Disconnected
                });
                let _ = control.end_write(record.engine, record.highest, reason);
                record.failed = true;
                record.end_deadline = None;
                record.latest = None;
                record.incomplete(gesture, reason);
            }
            if !record.failed
                && record
                    .start_clock
                    .zip(control.telemetry())
                    .is_some_and(|(start, t)| {
                        t.playback_clock.saturating_sub(start)
                            > crate::write_curve::MAX_DURATION_FRAMES
                    })
            {
                let _ = control.end_write(record.engine, record.highest, ReleaseReason::Conflict);
                record.latest = None;
                record.incomplete(gesture, ReleaseReason::Conflict);
            }
            // A claimed/unconsumed packet cannot be overwritten. Keep only the
            // latest incoming request and retry it after the actual receipt drains.
            if let Some((sequence, value)) = record.latest {
                match control.write_value(record.engine, sequence, value) {
                    Ok(_) => record.latest = None,
                    Err(WriteError::Busy) => {}
                    Err(e) => {
                        record.latest = None;
                        if let Some(waiting) = record.values.remove(&sequence) {
                            for reply in waiting.replies {
                                let _ = reply.try_send(Err(engine_error(e)));
                            }
                        }
                    }
                }
            }
            for _ in 0..3 {
                let receipt = match control.poll_write(record.engine) {
                    Ok(Some(r)) => r,
                    Ok(None) => break,
                    Err(_) => {
                        record.incomplete(gesture, ReleaseReason::Discontinuity);
                        terminal = true;
                        break;
                    }
                };
                match receipt {
                    WriteReceipt::Began { stamp, .. } => {
                        record.start_clock = Some(stamp.playback_clock);
                        if let Some(reply) = record.began.take() {
                            let result = Began {
                                mapping: record.mapping,
                                initial_value: record.original.value(stamp.control_frame),
                                stamp,
                                terminal: record.terminal_reader.take().unwrap(),
                            };
                            let _ = reply.try_send(Ok(result));
                        }
                    }
                    WriteReceipt::Applied {
                        sequence,
                        stamp,
                        ramp,
                        ..
                    } => {
                        if record.terminal.is_some() && record.captured.len() < MAX_CAPTURE - 1 {
                            record.captured.push(Captured {
                                sequence: Some(sequence),
                                stamp,
                                ramp,
                                restoring: false,
                            });
                        } else {
                            record.failed = true;
                            let _ = control.end_write(
                                record.engine,
                                record.highest,
                                ReleaseReason::Conflict,
                            );
                        }
                        let accepted: Vec<_> =
                            record.values.range(..=sequence).map(|(s, _)| *s).collect();
                        for requested in accepted {
                            let waiting = record.values.remove(&requested).unwrap();
                            for reply in waiting.replies {
                                let _ = reply.try_send(Ok(Applied {
                                    requested_sequence: requested,
                                    applied_sequence: sequence,
                                    stamp,
                                    ramp,
                                }));
                            }
                        }
                    }
                    WriteReceipt::Restoring { stamp, ramp, .. } => {
                        if record.terminal.is_some() && record.captured.len() < MAX_CAPTURE {
                            record.captured.push(Captured {
                                sequence: None,
                                stamp,
                                ramp,
                                restoring: true,
                            });
                        } else {
                            record.failed = true;
                        }
                    }
                    WriteReceipt::Released {
                        reason,
                        stamp,
                        last_applied,
                        ..
                    } => {
                        if let Some(reply) = record.began.take() {
                            let _ = reply
                                .try_send(Err(err("Audio Write ended before Begin was applied")));
                        }
                        for (_, waiting) in std::mem::take(&mut record.values) {
                            for reply in waiting.replies {
                                let _ = reply.try_send(Err(err(
                                    "Value was not applied before the lease ended",
                                )));
                            }
                        }
                        if let Some(out) = record.terminal.take() {
                            let _ = out.try_send(Prefix {
                                gesture,
                                reason,
                                captured: std::mem::take(&mut record.captured),
                                terminal: Some((stamp, last_applied)),
                                failed: record.failed,
                            });
                        }
                        terminal = true;
                        break;
                    }
                    WriteReceipt::Rejected {
                        sequence, error, ..
                    } => {
                        if let Some(sequence) = sequence {
                            if let Some(waiting) = record.values.remove(&sequence) {
                                for reply in waiting.replies {
                                    let _ = reply.try_send(Err(engine_error(error)));
                                }
                            }
                        } else {
                            if let Some(reply) = record.began.take() {
                                let _ = reply.try_send(Err(engine_error(error)));
                            }
                            record.failed = true;
                        }
                    }
                }
            }
            if terminal {
                self.records.remove(&gesture);
            }
        }
    }
    fn action(&mut self, control: &mut GraphControl, action: Action) {
        match action {
            Action::Begin {
                owner,
                gesture,
                prepared,
                reply,
            } => {
                if self.records.len() >= 16 || self.records.contains_key(&gesture) {
                    let _ = reply.try_send(Err(err(
                        "Write gesture already exists or all sixteen leases are occupied",
                    )));
                    return;
                }
                let engine = match control.begin_write(gesture, prepared.target) {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = reply.try_send(Err(engine_error(e)));
                        return;
                    }
                };
                let (terminal, terminal_reader) = sync_channel(1);
                self.records.insert(
                    gesture,
                    Record {
                        owner,
                        engine,
                        mapping: prepared.mapping,
                        original: prepared.original,
                        captured: prepared.capture,
                        began: Some(reply),
                        terminal: Some(terminal),
                        terminal_reader: Some(terminal_reader),
                        values: BTreeMap::new(),
                        latest: None,
                        highest: 0,
                        closing: None,
                        last_received: Instant::now(),
                        end_deadline: None,
                        released: None,
                        failed: false,
                        start_clock: None,
                    },
                );
            }
            Action::Value {
                owner,
                gesture,
                sequence,
                value,
                reply,
            } => {
                let Some(record) = self.records.get_mut(&gesture) else {
                    let _ = reply.try_send(Err(err("Write lease is absent")));
                    return;
                };
                if record.owner != owner
                    || record.began.is_some()
                    || record.failed
                    || record.closing.is_some_and(|s| sequence > s)
                    || sequence == 0
                    || !value.is_finite()
                    || !(0. ..=1.).contains(&value)
                {
                    let _ =
                        reply.try_send(Err(err("Value differs from the owned active Write lease")));
                    return;
                }
                let total: usize = record.values.values().map(|v| v.replies.len()).sum();
                if total >= MAX_WAITERS {
                    let _ = reply.try_send(Err(Error::QueueFull));
                    return;
                }
                if let Some(waiting) = record.values.get_mut(&sequence) {
                    if waiting.value != value {
                        let _ =
                            reply.try_send(Err(err("Write sequence reused with different value")));
                    } else {
                        waiting.replies.push(reply);
                    }
                    return;
                }
                if sequence <= record.highest {
                    let _ = reply.try_send(Err(err(
                        "Write sequence is stale; expired values are never reapplied",
                    )));
                    return;
                }
                record.values.insert(
                    sequence,
                    Waiting {
                        value,
                        replies: vec![reply],
                    },
                );
                record.highest = sequence;
                record.latest = Some((sequence, value));
                record.last_received = Instant::now();
            }
            Action::End {
                owner,
                gesture,
                sequence,
                reason,
                reply,
            } => {
                let Some(record) = self.records.get_mut(&gesture) else {
                    let _ = reply.try_send(Err(err("Write lease is absent")));
                    return;
                };
                if record.owner != owner {
                    let _ = reply.try_send(Err(err("Write lease belongs to another runtime")));
                    return;
                }
                match control.end_write(record.engine, sequence, reason) {
                    Ok(()) => {
                        record.closing = Some(sequence);
                        record.end_deadline = Some(Instant::now() + Duration::from_secs(2));
                        if reason != ReleaseReason::Finished {
                            record.latest = None;
                        }
                        let _ = reply.try_send(Ok(()));
                    }
                    Err(e) => {
                        let _ = reply.try_send(Err(engine_error(e)));
                    }
                }
            }
            Action::ReleaseOwner { owner, reason } => {
                for record in self.records.values_mut().filter(|r| r.owner == owner) {
                    let _ = control.end_write(record.engine, record.highest, reason);
                    record.closing = Some(record.highest);
                    record.latest = None;
                    record.failed = true;
                    // Absent callbacks still yield a bounded, truthful terminal.
                    record.released = Some(reason);
                    record.end_deadline = Some(Instant::now() + Duration::from_secs(2));
                }
            }
        }
    }
}
impl Record {
    fn incomplete(&mut self, gesture: u128, reason: ReleaseReason) {
        self.failed = true;
        if let Some(reply) = self.began.take() {
            let _ = reply.try_send(Err(Error::Timeout));
        }
        for (_, waiting) in std::mem::take(&mut self.values) {
            for reply in waiting.replies {
                let _ = reply.try_send(Err(Error::Timeout));
            }
        }
        // No invented completion stamp. A host must retain these timed-out
        // request IDs as indeterminate rather than automatically replaying them.
        if let Some(out) = self.terminal.take() {
            let _ = out.try_send(Prefix {
                gesture,
                reason,
                captured: std::mem::take(&mut self.captured),
                terminal: None,
                failed: true,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundroom_engine::{
        automation_write::WriteTarget, project::Project, render::PreparedGraph,
    };
    use std::sync::atomic::AtomicBool;
    const TRACK: &str = "12345678-1234-4234-8234-123456789ABC";
    fn owner() -> Owner {
        Owner {
            credential: TRACK.into(),
            runtime: TRACK.into(),
        }
    }
    fn setup() -> (
        Sender,
        Driver,
        GraphControl,
        soundroom_engine::realtime::AudioHost,
        Prepared,
    ) {
        let project = fixture();
        let (graph, mut control, mut host) = {
            let g = PreparedGraph::prepare_resident(&project, 1024 * 1024, &AtomicBool::new(false))
                .unwrap();
            let (c, h) = soundroom_engine::realtime::channel();
            (g, c, h)
        };
        let target = graph
            .resolve_write_target(&WriteTarget::Volume {
                track_id: TRACK.into(),
            })
            .unwrap();
        let mapping = graph.write_mapping(target).unwrap();
        let prepared = Prepared::new(
            target,
            mapping,
            Lane::new(&[], mapping.baseline_normalized, false),
        )
        .unwrap();
        assert!(control.publish(Box::new(graph)).is_ok());
        control.request_playing(true).unwrap();
        render(&mut host);
        let (s, d) = channel();
        (s, d, control, host, prepared)
    }
    fn fixture() -> Project {
        let mut project = Project::open(
            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/modern-gain.soundroom"),
        )
        .unwrap();
        project.session.tracks[0].id = TRACK.into();
        project.session.tracks[0].mix_automation.clear();
        project.session.tracks[0].realtime_effects.clear();
        project.session.master_track.realtime_effects.clear();
        project
    }
    fn render(host: &mut soundroom_engine::realtime::AudioHost) {
        let _ = audio(host);
    }
    fn audio(host: &mut soundroom_engine::realtime::AudioHost) -> [[f32; 128]; 2] {
        static CLOCK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let frame = CLOCK.fetch_add(128, std::sync::atomic::Ordering::Relaxed);
        let mut left = [0.; 128];
        let mut right = [0.; 128];
        host.render_timed(
            &mut left,
            &mut right,
            soundroom_engine::capture::CaptureTiming {
                sample_time: frame as f64,
                host_time: 1_000_000_000_000 + frame * 20_833,
                flags: 3,
            },
        );
        [left, right]
    }
    #[test]
    fn owner_release_without_callbacks_still_reports_a_bounded_truthful_terminal() {
        let (s, mut d, mut c, mut h, p) = setup();
        let (tx, rx) = sync_channel(1);
        s.send(
            Action::Begin {
                owner: owner(),
                gesture: 9,
                prepared: Box::new(p),
                reply: tx,
            },
            false,
        )
        .unwrap();
        d.tick(&mut c);
        render(&mut h);
        d.tick(&mut c);
        let began = rx.try_recv().unwrap().unwrap();
        // Output stops calling back, then the owner disconnects.
        s.send(
            Action::ReleaseOwner {
                owner: owner(),
                reason: ReleaseReason::Disconnected,
            },
            true,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let prefix = loop {
            d.tick(&mut c);
            if let Ok(prefix) = began.terminal.try_recv() {
                break prefix;
            }
            assert!(
                Instant::now() < deadline,
                "terminal must not wait for callbacks"
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(prefix.reason, ReleaseReason::Disconnected);
        assert!(prefix.failed);
        assert!(prefix.terminal.is_none(), "no invented release stamp");
    }
    #[test]
    fn begin_and_values_only_complete_from_real_callback_receipts() {
        let (s, mut d, mut c, mut h, p) = setup();
        let (tx, rx) = sync_channel(1);
        s.send(
            Action::Begin {
                owner: owner(),
                gesture: 1,
                prepared: Box::new(p),
                reply: tx,
            },
            false,
        )
        .unwrap();
        d.tick(&mut c);
        assert!(rx.try_recv().is_err());
        render(&mut h);
        d.tick(&mut c);
        let began = rx.try_recv().unwrap().unwrap();
        let (tx, rx) = sync_channel(1);
        s.send(
            Action::Value {
                owner: owner(),
                gesture: 1,
                sequence: 1,
                value: 0.2,
                reply: tx,
            },
            false,
        )
        .unwrap();
        d.tick(&mut c);
        assert!(rx.try_recv().is_err());
        render(&mut h);
        d.tick(&mut c);
        let value = rx.try_recv().unwrap().unwrap();
        assert_eq!(value.requested_sequence, 1);
        assert_eq!(value.applied_sequence, 1);
        assert!(value.stamp.control_frame > began.stamp.control_frame);
        c.request_playing(false).unwrap();
        for _ in 0..4 {
            render(&mut h);
            d.tick(&mut c);
        }
        let prefix = began.terminal.try_recv().unwrap();
        assert_eq!(prefix.reason, ReleaseReason::Stopped);
        assert!(!prefix.failed);
        let (stamp, last) = prefix.terminal.unwrap();
        assert_eq!(stamp.control_frame, last.unwrap().0 + 1);
        assert!(!d.active());
    }
    #[test]
    fn terminal_overtakes_future_values_but_wrong_owner_cannot_release() {
        let (s, mut d, mut c, mut h, p) = setup();
        let (tx, rx) = sync_channel(1);
        s.send(
            Action::Begin {
                owner: owner(),
                gesture: 1,
                prepared: Box::new(p),
                reply: tx,
            },
            false,
        )
        .unwrap();
        d.tick(&mut c);
        render(&mut h);
        d.tick(&mut c);
        let began = rx.try_recv().unwrap().unwrap();
        let (tx, rx) = sync_channel(1);
        s.send(
            Action::End {
                owner: Owner {
                    credential: "foreign".into(),
                    runtime: TRACK.into(),
                },
                gesture: 1,
                sequence: 1,
                reason: ReleaseReason::Cancelled,
                reply: tx,
            },
            true,
        )
        .unwrap();
        d.tick(&mut c);
        assert!(rx.try_recv().unwrap().is_err());
        let (tx, end) = sync_channel(1);
        s.send(
            Action::End {
                owner: owner(),
                gesture: 1,
                sequence: 1,
                reason: ReleaseReason::Finished,
                reply: tx,
            },
            true,
        )
        .unwrap();
        let (tx, value) = sync_channel(1);
        s.send(
            Action::Value {
                owner: owner(),
                gesture: 1,
                sequence: 1,
                value: 0.3,
                reply: tx,
            },
            false,
        )
        .unwrap();
        d.tick(&mut c);
        assert!(end.try_recv().unwrap().is_ok());
        assert!(value.try_recv().is_err());
        for _ in 0..5 {
            render(&mut h);
            d.tick(&mut c);
        }
        assert!(value.try_recv().unwrap().is_ok());
        assert_eq!(
            began.terminal.try_recv().unwrap().reason,
            ReleaseReason::Finished
        );
    }
    #[test]
    fn superseded_requests_acknowledge_the_actual_newer_applied_sequence() {
        let (s, mut d, mut c, mut h, p) = setup();
        let (tx, rx) = sync_channel(1);
        s.send(
            Action::Begin {
                owner: owner(),
                gesture: 1,
                prepared: Box::new(p),
                reply: tx,
            },
            false,
        )
        .unwrap();
        d.tick(&mut c);
        render(&mut h);
        d.tick(&mut c);
        let _began = rx.try_recv().unwrap().unwrap();
        let (a, ar) = sync_channel(1);
        let (b, br) = sync_channel(1);
        s.send(
            Action::Value {
                owner: owner(),
                gesture: 1,
                sequence: 1,
                value: 0.2,
                reply: a,
            },
            false,
        )
        .unwrap();
        d.tick(&mut c);
        s.send(
            Action::Value {
                owner: owner(),
                gesture: 1,
                sequence: 2,
                value: 0.8,
                reply: b,
            },
            false,
        )
        .unwrap();
        d.tick(&mut c);
        assert!(ar.try_recv().is_err());
        assert!(br.try_recv().is_err());
        render(&mut h);
        d.tick(&mut c);
        assert_eq!(ar.try_recv().unwrap().unwrap().applied_sequence, 2);
        assert_eq!(br.try_recv().unwrap().unwrap().applied_sequence, 2);
    }
    #[test]
    fn callback_prefix_finalizes_to_sample_equivalent_offline_replay() {
        let (s, mut d, mut c, mut h, p) = setup();
        let baseline = p.mapping.baseline_normalized;
        let (tx, rx) = sync_channel(1);
        s.send(
            Action::Begin {
                owner: owner(),
                gesture: 1,
                prepared: Box::new(p),
                reply: tx,
            },
            false,
        )
        .unwrap();
        d.tick(&mut c);
        let mut live = vec![audio(&mut h)];
        d.tick(&mut c);
        let began = rx.try_recv().unwrap().unwrap();
        let (tx, value) = sync_channel(1);
        s.send(
            Action::Value {
                owner: owner(),
                gesture: 1,
                sequence: 1,
                value: 0.1,
                reply: tx,
            },
            false,
        )
        .unwrap();
        d.tick(&mut c);
        live.push(audio(&mut h));
        d.tick(&mut c);
        assert!(value.try_recv().unwrap().is_ok());
        let (tx, end) = sync_channel(1);
        s.send(
            Action::End {
                owner: owner(),
                gesture: 1,
                sequence: 1,
                reason: ReleaseReason::Finished,
                reply: tx,
            },
            true,
        )
        .unwrap();
        d.tick(&mut c);
        assert!(end.try_recv().unwrap().is_ok());
        for _ in 0..5 {
            live.push(audio(&mut h));
            d.tick(&mut c);
        }
        let prefix = began.terminal.try_recv().unwrap();
        let (stamp, last) = prefix.terminal.unwrap();
        let (last_frame, last_value) = last.unwrap();
        let ramps: Vec<_> = prefix
            .captured
            .iter()
            .map(|r| crate::write_curve::AppliedRamp {
                frame: r.stamp.control_frame,
                start: r.ramp.start_value,
                target: r.ramp.target_value,
                duration: r.ramp.frames,
            })
            .collect();
        let lane = crate::write_curve::finalize(
            &[],
            baseline,
            false,
            &ramps,
            Some(crate::write_curve::Released {
                last_frame,
                last_value,
                next_read_frame: stamp.control_frame,
            }),
            crate::write_curve::ErrorBound::Normalized(0.000001),
            &AtomicBool::new(false),
        )
        .unwrap();
        let mut project = fixture();
        project.session.tracks[0]
            .mix_automation
            .insert("volume".into(), lane);
        let graph = PreparedGraph::prepare_resident(&project, 1024 * 1024, &AtomicBool::new(false))
            .unwrap();
        let (mut control, mut host) = soundroom_engine::realtime::channel();
        assert!(control.publish(Box::new(graph)).is_ok());
        control.request_playing(true).unwrap();
        render(&mut host);
        for block in live {
            let replay = audio(&mut host);
            for channel in 0..2 {
                for frame in 0..128 {
                    assert!((block[channel][frame] - replay[channel][frame]).abs() < 0.000002);
                }
            }
        }
    }
    #[test]
    fn missing_final_sequence_emits_incomplete_draft_not_fictional_terminal() {
        let (s, mut d, mut c, mut h, p) = setup();
        let (tx, rx) = sync_channel(1);
        s.send(
            Action::Begin {
                owner: owner(),
                gesture: 1,
                prepared: Box::new(p),
                reply: tx,
            },
            false,
        )
        .unwrap();
        d.tick(&mut c);
        render(&mut h);
        d.tick(&mut c);
        let began = rx.try_recv().unwrap().unwrap();
        let (tx, end) = sync_channel(1);
        s.send(
            Action::End {
                owner: owner(),
                gesture: 1,
                sequence: 1,
                reason: ReleaseReason::Finished,
                reply: tx,
            },
            true,
        )
        .unwrap();
        d.tick(&mut c);
        assert!(end.try_recv().unwrap().is_ok());
        d.records.get_mut(&1).unwrap().end_deadline = Some(Instant::now() - Duration::from_secs(1));
        d.tick(&mut c);
        let prefix = began.terminal.try_recv().unwrap();
        assert!(prefix.failed);
        assert!(prefix.terminal.is_none());
        assert_eq!(prefix.reason, ReleaseReason::Conflict);
        for _ in 0..4 {
            render(&mut h);
            d.tick(&mut c);
        }
        assert!(!d.active());
    }
    #[test]
    fn capture_payload_reservation_survives_prefix_transfer_and_bounds_admission() {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let limit = 2 * (MAX_CAPTURE * std::mem::size_of::<Captured>() + 4096);
        let mut buffers = Vec::new();
        while let Ok(buffer) = CaptureBuffer::with_budget(&COUNTER, limit) {
            buffers.push(buffer);
        }
        assert!(!buffers.is_empty());
        assert!(
            buffers
                .iter()
                .map(CaptureBuffer::reserved_bytes)
                .sum::<usize>()
                <= limit
        );
        let moved = buffers.pop().unwrap();
        let bytes = moved.reserved_bytes();
        let prefix = Prefix {
            gesture: 1,
            reason: ReleaseReason::Conflict,
            captured: moved,
            terminal: None,
            failed: true,
        };
        assert_eq!(prefix.captured.reserved_bytes(), bytes);
        assert!(CaptureBuffer::with_budget(&COUNTER, limit).is_err());
        drop(prefix);
        drop(buffers);
        assert!(CaptureBuffer::with_budget(&COUNTER, limit).is_ok());
    }
}
