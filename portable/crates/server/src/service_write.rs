//! Owner-leased continuous automation Write. Requests only enqueue fixed-size
//! actions for the callback driver and wait boundedly on workers; the authored
//! lane is merged into current content by a terminal worker as one Undo action.
//! The host never advertises this until explicitly enabled (see `enable`).
use super::{edits, new_id, protocol, wire, Artifact, Identity, Service, Shared};
use crate::{
    audio::write::{Action, Applied, Captured, Owner, Prefix},
    write_curve::{self, AppliedRamp, ErrorBound, Released},
    write_source::Snapshot,
    Error,
};
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use soundroom_client::{
    models::{
        AutomationValueScale, RemoteArtifact, RemoteAutomationWriteBeginReply,
        RemoteAutomationWriteBeginRequest, RemoteAutomationWriteCapturedRamp,
        RemoteAutomationWriteCapturedRelease, RemoteAutomationWriteDraftContent,
        RemoteAutomationWriteDraftReply, RemoteAutomationWriteDraftRequest,
        RemoteAutomationWriteEndRequest, RemoteAutomationWriteErrorDomain,
        RemoteAutomationWriteLeaseUpdate, RemoteAutomationWriteLimits,
        RemoteAutomationWriteMapping, RemoteAutomationWritePhase, RemoteAutomationWriteStamp,
        RemoteAutomationWriteValueDisposition, RemoteAutomationWriteValueReply,
        RemoteAutomationWriteValueRequest, RemoteEditorIntent, RemoteRequest, RemoteRole,
    },
    validation,
};
use soundroom_engine::automation_write::{
    ReleaseReason, WriteInterpolation, WriteMapping, WriteStamp, RAMP_FRAMES,
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender},
        Arc, Weak,
    },
    time::{Duration, Instant},
};
pub(super) const OPERATIONS: [&str; 4] = [
    "automation.writeBegin",
    "automation.writeValue",
    "automation.writeEnd",
    "automation.writeDraft",
];
const MAX_LEASES: usize = 16;
const MAX_DRAFTS: usize = 16;
const MAX_DRAFT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_TOTAL_DRAFT_BYTES: u64 = 64 * 1024 * 1024;
const DRAFT_LIFETIME: Duration = Duration::from_secs(3600);
const MAX_OUTCOMES: usize = 64;
const MAX_END_WAITERS: usize = 4;
/// Matches the driver's value-idle expiry.
const LEASE_TIMEOUT_MILLISECONDS: i64 = 10_000;
const BEGIN_WAIT: Duration = Duration::from_secs(2);
const GRAPH_WAIT: Duration = Duration::from_millis(1500);
const VALUE_WAIT: Duration = Duration::from_secs(2);
const ENQUEUE_WAIT: Duration = Duration::from_secs(1);
const END_OUTCOME_WAIT: Duration = Duration::from_secs(5);
const COMMIT_ATTEMPTS: usize = 4;
/// Per-owner admission. End always keeps a reserved slot beyond ordinary Values.
pub(super) const OWNER_WRITE_JOBS: usize = 6;
pub(super) const OWNER_TERMINAL_RESERVE: usize = 2;
pub(super) const TOTAL_WRITE_JOBS: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Beginning,
    Writing,
    Finalizing,
}
struct Lease {
    owner: Identity,
    snapshot: Arc<Snapshot>,
    track_revision: u64,
    mapping: RemoteAutomationWriteMapping,
    bound: ErrorBound,
    baseline: f32,
    phase: Phase,
    cancel: Arc<AtomicBool>,
    waiters: Vec<SyncSender<RemoteAutomationWriteLeaseUpdate>>,
    last_applied: Option<i64>,
    stamp: Option<RemoteAutomationWriteStamp>,
}
struct Outcome {
    gesture: String,
    credential: String,
    runtime: String,
    update: RemoteAutomationWriteLeaseUpdate,
}
struct Draft {
    credential: String,
    runtime: String,
    reply: RemoteAutomationWriteDraftReply,
    expires: Instant,
}
/// Lease, outcome and draft indexes. All owner checks use the authenticated
/// credential and runtime; knowing a gesture or draft UUID confers nothing.
#[derive(Default)]
pub(super) struct Registry {
    enabled: bool,
    leases: BTreeMap<String, Lease>,
    outcomes: VecDeque<Outcome>,
    drafts: BTreeMap<String, Draft>,
}
pub(super) enum Task {
    Begin {
        backend: crate::audio::Backend,
        request: RemoteAutomationWriteBeginRequest,
        snapshot: Arc<Snapshot>,
        revision: u64,
    },
    Value {
        backend: crate::audio::Backend,
        request: RemoteAutomationWriteValueRequest,
    },
    End {
        backend: crate::audio::Backend,
        request: RemoteAutomationWriteEndRequest,
    },
}
impl Task {
    pub(super) fn terminal(&self) -> bool {
        matches!(self, Self::End { .. })
    }
    pub(super) fn begin_gesture(&self) -> Option<String> {
        match self {
            Self::Begin { request, .. } => Some(request.gesture_id.to_ascii_uppercase()),
            _ => None,
        }
    }
}
fn conflict(message: &str) -> Error {
    Error::Conflict(message.into())
}
fn gesture_key(id: &str) -> Result<String, Error> {
    if !validation::uuid(id) {
        return Err(protocol("Write gesture must be a UUID"));
    }
    Ok(id.to_ascii_uppercase())
}
fn cookie(gesture: &str) -> Result<u128, Error> {
    u128::from_str_radix(&gesture.replace('-', ""), 16)
        .ok()
        .filter(|v| *v != 0)
        .ok_or_else(|| protocol("Invalid Write gesture"))
}
fn owner(peer: &Identity) -> Owner {
    Owner {
        credential: peer.credential.clone(),
        runtime: peer.runtime.clone(),
    }
}
fn owns(lease_owner: &Identity, peer: &Identity) -> bool {
    lease_owner.credential == peer.credential && lease_owner.runtime == peer.runtime
}
fn payload<T: serde::de::DeserializeOwned>(intent: &RemoteEditorIntent) -> Result<T, Error> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(
            intent
                .payload
                .as_deref()
                .ok_or_else(|| protocol("Write payload is required"))?,
        )
        .map_err(|_| protocol("Invalid Write payload"))?;
    Ok(serde_json::from_slice(&bytes)?)
}
fn stamp(value: WriteStamp) -> RemoteAutomationWriteStamp {
    RemoteAutomationWriteStamp {
        transport_epoch: value.transport_epoch as i64,
        output_frame: value.output_frame as i64,
        control_frame: value.control_frame as i64,
        playback_clock: value.playback_clock as i64,
        latency_offset_frames: value.latency_offset_frames as i64,
    }
}
/// Host-declared accuracy in the audible domain. Normalized 0.1% of range
/// (linear/physical, logarithmic via ln-span) or 0.05 dB for decibel volume.
pub(super) fn mapping(
    value: &WriteMapping,
) -> Result<(RemoteAutomationWriteMapping, ErrorBound), Error> {
    if value.integer {
        return Err(conflict(
            "Discrete parameters are not yet supported by live Write",
        ));
    }
    let ordered = value.minimum.partial_cmp(&value.maximum) == Some(std::cmp::Ordering::Less);
    if !ordered || value.logarithmic && value.minimum <= 0. {
        return Err(conflict("This parameter mapping cannot be written live"));
    }
    let decibels = value.interpolation_scale == WriteInterpolation::Decibels;
    let (domain, tolerance, bound) = if decibels {
        (
            RemoteAutomationWriteErrorDomain::Decibels,
            0.05,
            ErrorBound::Decibels(0.05),
        )
    } else if value.logarithmic {
        (
            RemoteAutomationWriteErrorDomain::Logarithmic,
            0.001 * (value.maximum / value.minimum).ln(),
            ErrorBound::Normalized(0.001),
        )
    } else {
        (
            RemoteAutomationWriteErrorDomain::Physical,
            0.001 * (value.maximum - value.minimum),
            ErrorBound::Normalized(0.001),
        )
    };
    Ok((
        RemoteAutomationWriteMapping {
            minimum: value.minimum,
            maximum: value.maximum,
            unit: value.unit.into(),
            logarithmic: value.logarithmic,
            integer: false,
            interpolation_scale: if decibels {
                AutomationValueScale::Decibels
            } else {
                AutomationValueScale::Linear
            },
            error_domain: domain,
            error_tolerance: tolerance.clamp(0.000001, 1.),
        },
        bound,
    ))
}
fn limits() -> RemoteAutomationWriteLimits {
    RemoteAutomationWriteLimits {
        maximum_leases: MAX_LEASES as i64,
        maximum_capture_samples: write_curve::MAX_CAPTURED_RAMPS as i64,
        maximum_duration_frames: write_curve::MAX_DURATION_FRAMES as i64,
        maximum_written_points: write_curve::MAX_WRITTEN_POINTS as i64,
        maximum_lane_points: write_curve::MAX_LANE_POINTS as i64,
        maximum_draft_bytes: MAX_DRAFT_BYTES as i64,
        lease_timeout_milliseconds: LEASE_TIMEOUT_MILLISECONDS,
        ramp_frames: RAMP_FRAMES as i64,
    }
}
fn update(
    gesture: &str,
    phase: RemoteAutomationWritePhase,
    reason: &str,
) -> RemoteAutomationWriteLeaseUpdate {
    RemoteAutomationWriteLeaseUpdate {
        gesture_id: gesture.into(),
        phase,
        reason: reason.chars().take(200).collect(),
        revision: None,
        last_applied_sequence: None,
        stamp: None,
        draft_id: None,
    }
}
impl Registry {
    fn outcome(&self, gesture: &str, peer: &Identity) -> Option<&RemoteAutomationWriteLeaseUpdate> {
        self.outcomes
            .iter()
            .rev()
            .find(|o| {
                o.gesture == gesture && o.credential == peer.credential && o.runtime == peer.runtime
            })
            .map(|o| &o.update)
    }
    /// A Begin that never ran (admission/cancellation) or failed leaves no lease.
    pub(super) fn abandon_beginning(&mut self, task: &Task) {
        if let Some(gesture) = task.begin_gesture() {
            self.abandon_gesture(&gesture);
        }
    }
    pub(super) fn abandon_gesture(&mut self, gesture: &str) {
        if self
            .leases
            .get(gesture)
            .is_some_and(|l| l.phase == Phase::Beginning)
        {
            self.leases.remove(gesture);
        }
    }
    pub(super) fn expire(&mut self) {
        let now = Instant::now();
        self.drafts.retain(|_, d| d.expires > now);
    }
}
impl Service {
    /// Development opt-in only. Advertising and accepting live Write requires
    /// an attached callback backend and this explicit host decision.
    pub fn enable_automation_write(&mut self) {
        self.writes.enabled = true;
    }
    pub(super) fn automation_write_available(&self) -> bool {
        self.writes.enabled && self.backend.is_some()
    }
    /// Disconnect or revocation restores Read promptly. The driver releases the
    /// override; the terminal worker retains a reviewable draft, never authors.
    pub(super) fn release_write_owner(
        &mut self,
        matches: impl Fn(&Identity) -> bool,
        reason: ReleaseReason,
    ) {
        let mut owners = Vec::new();
        for lease in self.writes.leases.values().filter(|l| matches(&l.owner)) {
            lease.cancel.store(true, Ordering::Release);
            if !owners.iter().any(|o: &Owner| {
                o.runtime == lease.owner.runtime && o.credential == lease.owner.credential
            }) {
                owners.push(owner(&lease.owner));
            }
        }
        if let Some(backend) = &self.backend {
            for owner in owners {
                let _ = backend.enqueue_write(Action::ReleaseOwner { owner, reason }, true);
            }
        }
    }
    fn owner_event(&self, owner: &Identity, value: &RemoteAutomationWriteLeaseUpdate) {
        let Ok(update) = wire(value) else {
            return;
        };
        if let Some(peer) = self.peers.get(&owner.runtime) {
            if peer.identity.credential == owner.credential {
                let _ = peer.out.send(
                    &json!({"version":3,"kind":"event","event":{"type":"automationWrite","update":update}}),
                    true,
                );
            }
        }
    }
    /// Validates ownership/target/version under the control lock and returns a
    /// worker task, or a direct payload for draft review.
    pub(super) fn automation_write(
        &mut self,
        peer: &Identity,
        r: &RemoteRequest,
        intent: &RemoteEditorIntent,
    ) -> Result<super::Handled, Error> {
        if !self.automation_write_available() {
            return Err(protocol(
                "Live automation Write is not enabled on this host",
            ));
        }
        if peer.role == RemoteRole::Viewer
            || !self.registry.authorizes(&peer.credential, &peer.role)
        {
            return Err(protocol("Current editor or operator role is required"));
        }
        let backend = self.backend.clone().unwrap();
        self.writes.expire();
        match intent.operation.as_str() {
            "automation.writeBegin" => {
                let request: RemoteAutomationWriteBeginRequest = payload(intent)?;
                let gesture = gesture_key(&request.gesture_id)?;
                cookie(&gesture)?;
                let track_id = request.track_id.to_ascii_uppercase();
                let current = *self
                    .authority
                    .track_revisions
                    .get(&track_id)
                    .ok_or_else(|| conflict("Write target track is unavailable"))?;
                let expected = Service::expected(r);
                if expected.len() != 1 || expected.get(&track_id) != Some(&current) {
                    return Err(conflict(
                        "Begin requires exactly the current target track version",
                    ));
                }
                if self.writes.leases.contains_key(&gesture)
                    || self.writes.outcomes.iter().any(|o| o.gesture == gesture)
                {
                    return Err(conflict("Write gesture was already used"));
                }
                if self.writes.leases.len() >= MAX_LEASES {
                    return Err(Error::QueueFull);
                }
                let snapshot =
                    Snapshot::capture(&self.authority.session, &track_id, request.target.clone())?;
                if self.writes.leases.values().any(|l| {
                    l.snapshot.track_id == snapshot.track_id && l.snapshot.target == snapshot.target
                }) {
                    return Err(conflict("Another owner is writing this target"));
                }
                let snapshot = Arc::new(snapshot);
                self.writes.leases.insert(
                    gesture,
                    Lease {
                        owner: peer.clone(),
                        snapshot: snapshot.clone(),
                        track_revision: current,
                        mapping: placeholder_mapping(),
                        bound: ErrorBound::Normalized(0.001),
                        baseline: 0.,
                        phase: Phase::Beginning,
                        cancel: Arc::new(AtomicBool::new(false)),
                        waiters: Vec::new(),
                        last_applied: None,
                        stamp: None,
                    },
                );
                Ok(super::Handled::Job(super::JobKind::Write {
                    task: Box::new(Task::Begin {
                        backend,
                        request,
                        snapshot,
                        revision: self.authority.revision,
                    }),
                }))
            }
            "automation.writeValue" => {
                let request: RemoteAutomationWriteValueRequest = payload(intent)?;
                let gesture = gesture_key(&request.gesture_id)?;
                let lease = self
                    .writes
                    .leases
                    .get(&gesture)
                    .filter(|l| owns(&l.owner, peer))
                    .ok_or_else(|| conflict("No owned active Write lease"))?;
                if lease.phase != Phase::Writing {
                    return Err(conflict("Write lease is not accepting values"));
                }
                Ok(super::Handled::Job(super::JobKind::Write {
                    task: Box::new(Task::Value { backend, request }),
                }))
            }
            "automation.writeEnd" => {
                let request: RemoteAutomationWriteEndRequest = payload(intent)?;
                let gesture = gesture_key(&request.gesture_id)?;
                if let Some(done) = self.writes.outcome(&gesture, peer) {
                    return Ok(super::Handled::Payload(Some(wire(done)?)));
                }
                let lease = self
                    .writes
                    .leases
                    .get(&gesture)
                    .filter(|l| owns(&l.owner, peer))
                    .ok_or_else(|| conflict("No owned Write lease"))?;
                if lease.phase == Phase::Beginning {
                    return Err(conflict("Begin has not been confirmed by audio"));
                }
                Ok(super::Handled::Job(super::JobKind::Write {
                    task: Box::new(Task::End { backend, request }),
                }))
            }
            "automation.writeDraft" => {
                let request: RemoteAutomationWriteDraftRequest = payload(intent)?;
                let draft = match (&request.draft_id, &request.gesture_id) {
                    (Some(id), None) => self.writes.drafts.get(&id.to_ascii_uppercase()),
                    (None, Some(gesture)) => {
                        let gesture = gesture_key(gesture)?;
                        self.writes
                            .drafts
                            .values()
                            .find(|d| d.reply.gesture_id == gesture)
                    }
                    _ => None,
                }
                .filter(|d| {
                    d.credential == peer.credential
                        && d.runtime == peer.runtime
                        && d.expires > Instant::now()
                })
                .ok_or_else(|| protocol("Draft is unavailable"))?;
                let mut reply = draft.reply.clone();
                reply.expires_in_seconds = draft
                    .expires
                    .saturating_duration_since(Instant::now())
                    .as_secs()
                    .clamp(1, 3600) as i64;
                Ok(super::Handled::Payload(Some(wire(&reply)?)))
            }
            _ => Err(protocol("Unknown Write operation")),
        }
    }
}
fn placeholder_mapping() -> RemoteAutomationWriteMapping {
    RemoteAutomationWriteMapping {
        minimum: 0.,
        maximum: 1.,
        unit: String::new(),
        logarithmic: false,
        integer: false,
        interpolation_scale: AutomationValueScale::Linear,
        error_domain: RemoteAutomationWriteErrorDomain::Physical,
        error_tolerance: 0.001,
    }
}
fn lock(shared: &Shared) -> Result<std::sync::MutexGuard<'_, Service>, Error> {
    shared
        .lock()
        .map_err(|_| protocol("Authority is unavailable"))
}
impl Task {
    pub(super) fn run(
        self,
        shared: &Shared,
        peer: &Identity,
        cancel: &AtomicBool,
    ) -> Result<Option<Value>, Error> {
        match self {
            Task::Begin {
                backend,
                request,
                snapshot,
                revision,
            } => begin(shared, peer, cancel, backend, request, snapshot, revision),
            Task::Value { backend, request } => value(shared, peer, backend, request),
            Task::End { backend, request } => end(shared, peer, backend, request),
        }
    }
}
fn abandon(shared: &Shared, gesture: &str) {
    if let Ok(mut state) = shared.lock() {
        state.writes.abandon_gesture(gesture);
    }
}
fn begin(
    shared: &Shared,
    peer: &Identity,
    cancel: &AtomicBool,
    backend: crate::audio::Backend,
    request: RemoteAutomationWriteBeginRequest,
    snapshot: Arc<Snapshot>,
    revision: u64,
) -> Result<Option<Value>, Error> {
    let gesture = gesture_key(&request.gesture_id)?;
    let result = (|| {
        // A just-authored revision (e.g. the previous gesture's commit) may still
        // be adopting its graph. Wait boundedly for audio rather than decline.
        let deadline = Instant::now() + GRAPH_WAIT;
        while backend.graph_revisions().authored == revision
            && backend.graph_revisions().applied != Some(revision)
            && Instant::now() < deadline
            && !cancel.load(Ordering::Acquire)
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        let prepared =
            backend.prepare_write(&snapshot.engine_target(), snapshot.points(), revision)?;
        let engine_mapping = prepared.mapping();
        let (wire_mapping, bound) = mapping(&engine_mapping)?;
        if cancel.load(Ordering::Acquire) {
            return Err(protocol("Begin was cancelled"));
        }
        let (reply, receipt) = sync_channel(1);
        backend.enqueue_write(
            Action::Begin {
                owner: owner(peer),
                gesture: cookie(&gesture)?,
                prepared: Box::new(prepared),
                reply,
            },
            false,
        )?;
        let began = match receipt.recv_timeout(BEGIN_WAIT) {
            Ok(result) => result?,
            Err(_) => {
                // Never leave an unowned driver lease: cancel it at priority.
                let (reply, _) = sync_channel(1);
                let _ = backend.enqueue_write(
                    Action::End {
                        owner: owner(peer),
                        gesture: cookie(&gesture)?,
                        sequence: 0,
                        reason: ReleaseReason::Cancelled,
                        reply,
                    },
                    true,
                );
                return Err(Error::Timeout);
            }
        };
        Ok((
            began,
            wire_mapping,
            bound,
            engine_mapping.baseline_normalized,
        ))
    })();
    let (began, wire_mapping, bound, baseline) = match result {
        Ok(v) => v,
        Err(e) => {
            abandon(shared, &gesture);
            return Err(e);
        }
    };
    let terminal = began.terminal;
    let mut state = lock(shared)?;
    let current = state.registry.authorizes(&peer.credential, &peer.role)
        && state
            .peers
            .get(&peer.runtime)
            .is_some_and(|p| p.identity.credential == peer.credential);
    let lease = state
        .writes
        .leases
        .get_mut(&gesture)
        .filter(|l| l.phase == Phase::Beginning && !l.cancel.load(Ordering::Acquire));
    let Some(lease) = lease.filter(|_| current && !cancel.load(Ordering::Acquire)) else {
        // Revoked, disconnected or cancelled while audio accepted Begin. The
        // watcher still receives the release and reports it truthfully.
        let (reply, _) = sync_channel(1);
        let _ = backend.enqueue_write(
            Action::End {
                owner: owner(peer),
                gesture: cookie(&gesture)?,
                sequence: 0,
                reason: ReleaseReason::Cancelled,
                reply,
            },
            true,
        );
        if let Some(lease) = state.writes.leases.get_mut(&gesture) {
            lease.phase = Phase::Writing;
        }
        drop(state);
        watch(shared, gesture, terminal);
        return Err(protocol("Write owner changed before audio confirmed Begin"));
    };
    lease.phase = Phase::Writing;
    lease.mapping = wire_mapping.clone();
    lease.bound = bound;
    lease.baseline = baseline.clamp(0., 1.);
    lease.stamp = Some(stamp(began.stamp));
    let reply = RemoteAutomationWriteBeginReply {
        gesture_id: gesture.clone(),
        track_id: snapshot.track_id.clone(),
        target: snapshot.target.clone(),
        track_revision: lease.track_revision as i64,
        initial_value: began.initial_value.clamp(0., 1.),
        mapping: wire_mapping,
        limits: limits(),
        stamp: stamp(began.stamp),
    };
    drop(state);
    watch(shared, gesture, terminal);
    Ok(Some(wire(&reply)?))
}
fn value(
    shared: &Shared,
    peer: &Identity,
    backend: crate::audio::Backend,
    request: RemoteAutomationWriteValueRequest,
) -> Result<Option<Value>, Error> {
    let gesture = gesture_key(&request.gesture_id)?;
    let (reply, receipt) = sync_channel(1);
    backend.enqueue_write(
        Action::Value {
            owner: owner(peer),
            gesture: cookie(&gesture)?,
            sequence: request.sequence as u64,
            value: request.value,
            reply,
        },
        false,
    )?;
    let applied: Applied = match receipt.recv_timeout(VALUE_WAIT) {
        Ok(result) => result?,
        Err(_) => return Err(Error::Timeout),
    };
    let result = RemoteAutomationWriteValueReply {
        gesture_id: gesture.clone(),
        sequence: applied.requested_sequence as i64,
        applied_sequence: applied.applied_sequence as i64,
        disposition: if applied.applied_sequence == applied.requested_sequence {
            RemoteAutomationWriteValueDisposition::Applied
        } else {
            RemoteAutomationWriteValueDisposition::Superseded
        },
        target_value: applied.ramp.target_value.clamp(0., 1.),
        ramp_start_value: applied.ramp.start_value.clamp(0., 1.),
        ramp_frames: applied.ramp.frames as i64,
        stamp: stamp(applied.stamp),
    };
    if let Ok(mut state) = shared.lock() {
        if let Some(lease) = state.writes.leases.get_mut(&gesture) {
            if lease
                .last_applied
                .map_or(true, |s| s < result.applied_sequence)
            {
                lease.last_applied = Some(result.applied_sequence);
                lease.stamp = Some(result.stamp.clone());
            }
        }
    }
    Ok(Some(wire(&result)?))
}
/// End acknowledgement from the driver is enqueue-only. The reply waits a
/// bounded time for the actual terminal outcome, else reports `finalizing`.
fn end(
    shared: &Shared,
    peer: &Identity,
    backend: crate::audio::Backend,
    request: RemoteAutomationWriteEndRequest,
) -> Result<Option<Value>, Error> {
    let gesture = gesture_key(&request.gesture_id)?;
    let (waiter, outcome) = sync_channel(1);
    {
        let mut state = lock(shared)?;
        if let Some(done) = state.writes.outcome(&gesture, peer) {
            return Ok(Some(wire(done)?));
        }
        let lease = state
            .writes
            .leases
            .get_mut(&gesture)
            .filter(|l| owns(&l.owner, peer))
            .ok_or_else(|| conflict("No owned Write lease"))?;
        if lease.waiters.len() >= MAX_END_WAITERS {
            return Err(Error::QueueFull);
        }
        lease.waiters.push(waiter);
    }
    let (reply, receipt) = sync_channel(1);
    backend.enqueue_write(
        Action::End {
            owner: owner(peer),
            gesture: cookie(&gesture)?,
            sequence: request.final_sequence as u64,
            reason: if request.cancel {
                ReleaseReason::Cancelled
            } else {
                ReleaseReason::Finished
            },
            reply,
        },
        true,
    )?;
    match receipt.recv_timeout(ENQUEUE_WAIT) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err(Error::Timeout),
    }
    match outcome.recv_timeout(END_OUTCOME_WAIT) {
        Ok(done) => Ok(Some(wire(&done)?)),
        Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
            let state = lock(shared)?;
            if let Some(done) = state.writes.outcome(&gesture, peer) {
                return Ok(Some(wire(done)?));
            }
            let mut pending = update(
                &gesture,
                RemoteAutomationWritePhase::Finalizing,
                "The recorded gesture is being finalized; its outcome follows as an event.",
            );
            if let Some(lease) = state.writes.leases.get(&gesture) {
                pending.last_applied_sequence = lease.last_applied;
            }
            Ok(Some(wire(&pending)?))
        }
    }
}
fn watch(shared: &Shared, gesture: String, terminal: Receiver<Prefix>) {
    let weak = Arc::downgrade(shared);
    std::thread::spawn(move || {
        let prefix = terminal.recv().ok();
        finish(&weak, &gesture, prefix);
    });
}
struct Context {
    owner: Identity,
    snapshot: Arc<Snapshot>,
    track_revision: u64,
    mapping: RemoteAutomationWriteMapping,
    bound: ErrorBound,
    baseline: f32,
    cancel: Arc<AtomicBool>,
}
/// Terminal worker: finalize the actual prefix, merge only the leased lane into
/// current content as one original-author action, or retain a review draft.
fn finish(weak: &Weak<std::sync::Mutex<Service>>, gesture: &str, prefix: Option<Prefix>) {
    let Some(shared) = weak.upgrade() else {
        return;
    };
    let context = {
        let Ok(mut state) = shared.lock() else {
            return;
        };
        let Some(lease) = state.writes.leases.get_mut(gesture) else {
            return;
        };
        lease.phase = Phase::Finalizing;
        let context = Context {
            owner: lease.owner.clone(),
            snapshot: lease.snapshot.clone(),
            track_revision: lease.track_revision,
            mapping: lease.mapping.clone(),
            bound: lease.bound,
            baseline: lease.baseline,
            cancel: lease.cancel.clone(),
        };
        let mut notice = update(
            gesture,
            RemoteAutomationWritePhase::Finalizing,
            "Finishing the recorded gesture.",
        );
        notice.last_applied_sequence = lease.last_applied;
        notice.stamp = lease.stamp.clone();
        state.owner_event(&context.owner, &notice);
        context
    };
    let mut outcome = conclude(&shared, gesture, &context, prefix);
    outcome.reason = outcome.reason.chars().take(200).collect();
    let Ok(mut state) = shared.lock() else {
        return;
    };
    if let Some(lease) = state.writes.leases.remove(gesture) {
        for waiter in lease.waiters {
            let _ = waiter.try_send(outcome.clone());
        }
    }
    if state.writes.outcomes.len() >= MAX_OUTCOMES {
        state.writes.outcomes.pop_front();
    }
    state.writes.outcomes.push_back(Outcome {
        gesture: gesture.into(),
        credential: context.owner.credential.clone(),
        runtime: context.owner.runtime.clone(),
        update: outcome.clone(),
    });
    state.owner_event(&context.owner, &outcome);
}
fn release_phase(reason: ReleaseReason, owner_connected: bool) -> RemoteAutomationWritePhase {
    match reason {
        ReleaseReason::Cancelled => RemoteAutomationWritePhase::Cancelled,
        ReleaseReason::Revoked => RemoteAutomationWritePhase::Failed,
        ReleaseReason::Disconnected if owner_connected => RemoteAutomationWritePhase::Expired,
        ReleaseReason::Disconnected => RemoteAutomationWritePhase::Disconnected,
        ReleaseReason::Conflict | ReleaseReason::Discontinuity => {
            RemoteAutomationWritePhase::Conflict
        }
        ReleaseReason::Finished | ReleaseReason::Stopped => RemoteAutomationWritePhase::Failed,
    }
}
fn release_reason(reason: ReleaseReason) -> &'static str {
    match reason {
        ReleaseReason::Cancelled => "Write was cancelled; Read is restored.",
        ReleaseReason::Revoked => "The credential was revoked; Read is restored.",
        ReleaseReason::Disconnected => {
            "The lease ended without values or connection; Read is restored."
        }
        ReleaseReason::Conflict => "The gesture could not complete; Read is restored.",
        ReleaseReason::Discontinuity => {
            "Playback was interrupted (seek, graph change or output restart); Read is restored."
        }
        ReleaseReason::Finished | ReleaseReason::Stopped => {
            "The recorded gesture could not be authored."
        }
    }
}
fn conclude(
    shared: &Shared,
    gesture: &str,
    context: &Context,
    prefix: Option<Prefix>,
) -> RemoteAutomationWriteLeaseUpdate {
    let connected = shared.lock().is_ok_and(|state| {
        state
            .peers
            .get(&context.owner.runtime)
            .is_some_and(|p| p.identity.credential == context.owner.credential)
    });
    let Some(prefix) = prefix else {
        return update(
            gesture,
            RemoteAutomationWritePhase::Failed,
            "Audio ended without a release receipt; nothing was authored.",
        );
    };
    let applied = prefix
        .captured
        .iter()
        .filter(|c| c.sequence.is_some())
        .count();
    let last_sequence = prefix.captured.iter().filter_map(|c| c.sequence).max();
    let first_epoch = prefix.captured.first().map(|c| c.stamp.transport_epoch);
    // A release from a different clock epoch (Seek/output restart) is not part
    // of the saved prefix; the draft then stays explicitly incomplete.
    let release = prefix
        .terminal
        .filter(|(s, _)| first_epoch.map_or(true, |e| s.transport_epoch == e));
    let terminal_stamp = release
        .map(|(s, _)| stamp(s))
        .or_else(|| prefix.captured.last().map(|c| stamp(c.stamp)));
    let mut result = |phase, reason: &str| {
        let mut value = update(gesture, phase, reason);
        value.last_applied_sequence = last_sequence.map(|s| s as i64);
        value.stamp = terminal_stamp.clone();
        value
    };
    let committable = matches!(
        prefix.reason,
        ReleaseReason::Finished | ReleaseReason::Stopped
    ) && !prefix.failed
        && !context.cancel.load(Ordering::Acquire)
        && connected;
    if applied == 0 {
        let reason = if committable || prefix.reason == ReleaseReason::Cancelled {
            "No value was applied; the curve is unchanged."
        } else {
            release_reason(prefix.reason)
        };
        let phase = if committable {
            RemoteAutomationWritePhase::Cancelled
        } else {
            release_phase(prefix.reason, connected)
        };
        return result(phase, reason);
    }
    let review = |phase,
                  reason: &str,
                  result: &mut dyn FnMut(
        RemoteAutomationWritePhase,
        &str,
    ) -> RemoteAutomationWriteLeaseUpdate| {
        let mut value = result(phase, reason);
        match store_draft(shared, gesture, context, &prefix, release, reason) {
            Ok(id) => value.draft_id = Some(id),
            Err(e) => {
                value.reason = format!("{reason} The review draft was not retained: {e}");
            }
        }
        value
    };
    if !committable {
        let phase = if prefix.failed
            && matches!(
                prefix.reason,
                ReleaseReason::Finished | ReleaseReason::Stopped
            ) {
            RemoteAutomationWritePhase::Conflict
        } else {
            release_phase(prefix.reason, connected)
        };
        let reason = if !connected
            && matches!(
                prefix.reason,
                ReleaseReason::Finished | ReleaseReason::Stopped
            ) {
            "The owner disconnected before authoring; review the recorded draft."
        } else if prefix.failed
            && matches!(
                prefix.reason,
                ReleaseReason::Finished | ReleaseReason::Stopped
            )
        {
            "The gesture exceeded a bound or its final value was not applied; review the recorded draft."
        } else {
            release_reason(prefix.reason)
        };
        return review(phase, reason, &mut result);
    }
    let Some((release_stamp, Some((last_frame, last_value)))) = release else {
        return review(
            RemoteAutomationWritePhase::Conflict,
            "The applied prefix has no actual final sample; review the recorded draft.",
            &mut result,
        );
    };
    let ramps: Vec<AppliedRamp> = prefix
        .captured
        .iter()
        .map(|c| AppliedRamp {
            frame: c.stamp.control_frame,
            start: c.ramp.start_value,
            target: c.ramp.target_value,
            duration: c.ramp.frames,
        })
        .collect();
    let decibels = context.mapping.interpolation_scale == AutomationValueScale::Decibels;
    let points = match write_curve::finalize(
        context.snapshot.points(),
        context.baseline,
        decibels,
        &ramps,
        Some(Released {
            last_frame,
            last_value,
            next_read_frame: release_stamp.control_frame,
        }),
        context.bound,
        &context.cancel,
    ) {
        Ok(points) => points,
        Err(failure) => {
            return review(
                RemoteAutomationWritePhase::Conflict,
                &format!("The prefix could not be simplified within its bounds ({failure:?}); review the recorded draft."),
                &mut result,
            )
        }
    };
    match commit(shared, context, points) {
        Ok(revision) => {
            let mut value = result(
                RemoteAutomationWritePhase::Committed,
                "Recorded on the server. Undo restores the previous curve.",
            );
            value.revision = Some(revision as i64);
            value
        }
        Err(e) => review(
            RemoteAutomationWritePhase::Conflict,
            &format!("{e} Review the recorded draft."),
            &mut result,
        ),
    }
}
/// Merges only the leased lane into the then-current session. Unrelated
/// concurrent commits retry against fresh content; target changes conflict.
fn commit(
    shared: &Shared,
    context: &Context,
    points: Vec<soundroom_engine::model::AutomationPoint>,
) -> Result<u64, Error> {
    for _ in 0..COMMIT_ATTEMPTS {
        let (task, expected) = {
            let state = lock(shared)?;
            if context.cancel.load(Ordering::Acquire)
                || !state
                    .registry
                    .authorizes(&context.owner.credential, &context.owner.role)
                || context.owner.role == RemoteRole::Viewer
            {
                return Err(conflict(
                    "The owner's role or lease changed before authoring.",
                ));
            }
            if !state
                .peers
                .get(&context.owner.runtime)
                .is_some_and(|p| p.identity.credential == context.owner.credential)
            {
                return Err(conflict("The owner disconnected before authoring."));
            }
            let current = *state
                .authority
                .track_revisions
                .get(&context.snapshot.track_id)
                .ok_or_else(|| conflict("The written track was removed."))?;
            (
                edits::Task::capture_reserved(&state, &context.owner)?,
                BTreeMap::from([(context.snapshot.track_id.clone(), current)]),
            )
        };
        match task.run_transform(
            shared,
            &context.owner,
            &context.cancel,
            edits::TransformAction {
                label: "Write automation",
                gesture: None,
                expected_revision: None,
                expected: &expected,
            },
            |before| context.snapshot.merge(before, points.clone()),
        ) {
            Ok(revision) => return Ok(revision),
            Err(Error::Conflict(message))
                if message == "Session changed during authored preparation" =>
            {
                continue
            }
            Err(e) => return Err(e),
        }
    }
    Err(conflict("The session kept changing while authoring."))
}
fn store_draft(
    shared: &Shared,
    gesture: &str,
    context: &Context,
    prefix: &Prefix,
    release: Option<(WriteStamp, Option<(u64, f32)>)>,
    reason: &str,
) -> Result<String, Error> {
    let ramps: Vec<_> = prefix.captured.iter().map(captured).collect();
    let terminal = release.map(|(s, last)| RemoteAutomationWriteCapturedRelease {
        stamp: stamp(s),
        last_control_frame: last.map(|(f, _)| f as i64),
        last_value: last.map(|(_, v)| v.clamp(0., 1.)),
    });
    let content = RemoteAutomationWriteDraftContent {
        version: 1,
        gesture_id: gesture.into(),
        track_id: context.snapshot.track_id.clone(),
        target: context.snapshot.target.clone(),
        track_revision: context.track_revision as i64,
        baseline: context.baseline,
        mapping: context.mapping.clone(),
        reason: reason.chars().take(200).collect(),
        // Exact original bytes/metadata; capture already validated each point.
        original_points: serde_json::from_value(serde_json::to_value(context.snapshot.points())?)?,
        captured_ramps: ramps,
        terminal,
    };
    let body = wire(&content)?;
    validation::validate_model(&body, "RemoteAutomationWriteDraftContent")?;
    let bytes = serde_json::to_vec(&body)?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_DRAFT_BYTES {
        return Err(protocol("Draft exceeds its 32 MiB bound"));
    }
    let sha256 = hex(&Sha256::digest(&bytes));
    let id = new_id()?.to_ascii_uppercase();
    let path = {
        let state = lock(shared)?;
        state.root.join("Artifacts").join(format!("{id}.json"))
    };
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, &bytes)?;
    let first = prefix
        .captured
        .iter()
        .find(|c| c.sequence.is_some())
        .map(|c| stamp(c.stamp));
    let last = content
        .terminal
        .as_ref()
        .map(|t| t.stamp.clone())
        .or_else(|| content.captured_ramps.last().map(|r| r.stamp.clone()));
    let reply = RemoteAutomationWriteDraftReply {
        draft_id: id.clone(),
        gesture_id: gesture.into(),
        track_id: content.track_id.clone(),
        target: content.target.clone(),
        reason: content.reason.clone(),
        applied_value_count: prefix
            .captured
            .iter()
            .filter(|c| c.sequence.is_some())
            .count() as i64,
        first_stamp: first,
        last_stamp: last,
        mapping: content.mapping.clone(),
        expires_in_seconds: DRAFT_LIFETIME.as_secs() as i64,
        artifact: RemoteArtifact {
            id: id.clone(),
            name: format!(
                "automation-write-{}.json",
                &gesture[..8].to_ascii_lowercase()
            ),
            bytes: bytes.len() as i64,
            sha256,
        },
    };
    validation::validate_model(&wire(&reply)?, "RemoteAutomationWriteDraftReply")?;
    let mut state = lock(shared)?;
    state.writes.expire();
    let total: u64 = state
        .writes
        .drafts
        .values()
        .map(|d| d.reply.artifact.bytes as u64)
        .sum();
    if state.writes.drafts.len() >= MAX_DRAFTS
        || total + bytes.len() as u64 > MAX_TOTAL_DRAFT_BYTES
        || state.artifacts.len() >= 32
    {
        let _ = std::fs::remove_file(&path);
        return Err(Error::QueueFull);
    }
    let expires = Instant::now() + DRAFT_LIFETIME;
    state.artifacts.insert(
        id.clone(),
        Artifact {
            owner: context.owner.credential.clone(),
            runtime: Some(context.owner.runtime.clone()),
            path,
            bytes: bytes.len() as u64,
            expires,
        },
    );
    state.writes.drafts.insert(
        id.clone(),
        Draft {
            credential: context.owner.credential.clone(),
            runtime: context.owner.runtime.clone(),
            reply,
            expires,
        },
    );
    Ok(id)
}
fn captured(c: &Captured) -> RemoteAutomationWriteCapturedRamp {
    RemoteAutomationWriteCapturedRamp {
        sequence: c.sequence.map(|s| s as i64),
        restoring: c.restoring,
        stamp: stamp(c.stamp),
        start_value: c.ramp.start_value.clamp(0., 1.),
        target_value: c.ramp.target_value.clamp(0., 1.),
        ramp_frames: c.ramp.frames as i64,
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::super::{publication_tests::control_fixture, Inbox, Outbox, Service};
    use super::*;
    use std::sync::Mutex;
    struct Harness {
        root: std::path::PathBuf,
        shared: Shared,
        owner: Identity,
        out: Outbox,
        inbox: Inbox,
        track: String,
        events: Mutex<Vec<Value>>,
        stop: Arc<AtomicBool>,
        callback: Option<std::thread::JoinHandle<()>>,
    }
    impl Drop for Harness {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(callback) = self.callback.take() {
                let _ = callback.join();
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn editor_op(operation: &str, value: &impl serde::Serialize) -> Value {
        let bytes = serde_json::to_vec(&wire(value).unwrap()).unwrap();
        json!({"type":"editor","intent":{"operation":operation,"context":{"cursorFrame":0,"clipIDs":[]},
            "integers":{},"numbers":{},"strings":{},"flags":{},
            "payload":base64::engine::general_purpose::STANDARD.encode(bytes)}})
    }
    fn decode(reply: &Value) -> Value {
        serde_json::from_slice(
            &base64::engine::general_purpose::STANDARD
                .decode(reply["payload"].as_str().expect("reply payload"))
                .unwrap(),
        )
        .unwrap()
    }
    fn harness(enabled: bool) -> Harness {
        let (root, shared, _editor, track) = control_fixture(0);
        let credential = shared
            .lock()
            .unwrap()
            .registry
            .issue("Operator", RemoteRole::Operator)
            .unwrap();
        let owner = Identity {
            runtime: new_id().unwrap(),
            credential: credential["credentialID"].as_str().unwrap().into(),
            name: "Operator".into(),
            role: RemoteRole::Operator,
        };
        let (control, mut host) = soundroom_engine::realtime::channel();
        Service::attach_audio(&shared, control).unwrap();
        if enabled {
            shared.lock().unwrap().enable_automation_write();
        }
        let stop = Arc::new(AtomicBool::new(false));
        let callback_stop = stop.clone();
        // Manually pumped, timestamped callback; no device is opened.
        let callback = std::thread::spawn(move || {
            let mut left = [0.; 128];
            let mut right = [0.; 128];
            let mut frame = 0u64;
            while !callback_stop.load(Ordering::Acquire) {
                host.render_timed(
                    &mut left,
                    &mut right,
                    soundroom_engine::capture::CaptureTiming {
                        sample_time: frame as f64,
                        host_time: 1_000_000_000_000 + frame * 20_833,
                        flags: 3,
                    },
                );
                frame += 128;
                std::thread::sleep(Duration::from_micros(500));
            }
        });
        let global = shared.lock().unwrap().global();
        let (out, inbox) = Outbox::pair(global);
        shared
            .lock()
            .unwrap()
            .join(owner.clone(), out.clone())
            .unwrap();
        Harness {
            root,
            shared,
            owner,
            out,
            inbox,
            track,
            events: Mutex::new(vec![]),
            stop,
            callback: Some(callback),
        }
    }
    impl Harness {
        fn version(&self) -> u64 {
            self.shared.lock().unwrap().authority.track_revisions[&self.track]
        }
        fn send_as(
            &self,
            peer: &Identity,
            out: &Outbox,
            inbox: &Inbox,
            command: Value,
            tracks: Value,
        ) -> Value {
            let id = new_id().unwrap();
            let frame = json!({"version":3,"kind":"request","id":id,"clientID":peer.runtime,
                "expectedTrackRevisions":tracks,"command":command});
            if let Err(e) = Service::dispatch(&self.shared, peer, out, frame) {
                return json!({"status":"error","message":e.to_string()});
            }
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(bytes) = inbox
                    .priority
                    .try_recv()
                    .ok()
                    .or_else(|| inbox.ordinary.try_recv().ok())
                {
                    inbox.received(bytes.len());
                    let value: Value = serde_json::from_slice(&bytes).unwrap();
                    if value["kind"] == "reply" && value["requestID"] == id {
                        return value;
                    }
                    if value["event"]["type"] == "automationWrite" {
                        self.events
                            .lock()
                            .unwrap()
                            .push(value["event"]["update"].clone());
                    }
                }
                assert!(Instant::now() < deadline, "RPC receipt timeout");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        fn send(&self, command: Value, tracks: Value) -> Value {
            self.send_as(&self.owner, &self.out, &self.inbox, command, tracks)
        }
        fn play(&self) {
            let reply = self.send(json!({"type":"play"}), json!({}));
            assert_eq!(reply["status"], "accepted", "{reply}");
            let deadline = Instant::now() + Duration::from_secs(5);
            while self
                .shared
                .lock()
                .unwrap()
                .backend
                .as_ref()
                .unwrap()
                .graph_revisions()
                .applied
                .is_none()
            {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        fn begin(&self, gesture: &str) -> Value {
            self.send(
                editor_op(
                    "automation.writeBegin",
                    &RemoteAutomationWriteBeginRequest {
                        gesture_id: gesture.into(),
                        track_id: self.track.clone(),
                        target: soundroom_client::models::RemoteAutomationWriteTarget {
                            kind: soundroom_client::models::RemoteAutomationWriteTargetKind::Volume,
                            effect_id: None,
                            parameter_id: None,
                        },
                        mode: soundroom_client::models::RemoteAutomationWriteMode::Touch,
                    },
                ),
                json!({ &self.track: self.version() }),
            )
        }
        fn value(&self, gesture: &str, sequence: i64, value: f32) -> Value {
            self.send(
                editor_op(
                    "automation.writeValue",
                    &RemoteAutomationWriteValueRequest {
                        gesture_id: gesture.into(),
                        sequence,
                        value,
                    },
                ),
                json!({}),
            )
        }
        fn end(&self, gesture: &str, final_sequence: i64, cancel: bool) -> Value {
            self.send(
                editor_op(
                    "automation.writeEnd",
                    &RemoteAutomationWriteEndRequest {
                        gesture_id: gesture.into(),
                        final_sequence,
                        cancel,
                    },
                ),
                json!({}),
            )
        }
        fn volume_lane(&self) -> Vec<soundroom_engine::model::AutomationPoint> {
            self.shared.lock().unwrap().authority.session.tracks[0]
                .mix_automation
                .get("volume")
                .cloned()
                .unwrap_or_default()
        }
        fn record(&self, gesture: &str) -> i64 {
            let begin = self.begin(gesture);
            assert_eq!(begin["status"], "accepted", "{begin}");
            let began = decode(&begin);
            validation::validate_model(&began, "RemoteAutomationWriteBeginReply").unwrap();
            for (sequence, value) in [(1, 0.2), (2, 0.4), (3, 0.6)] {
                let reply = self.value(gesture, sequence, value);
                assert_eq!(reply["status"], "accepted", "{reply}");
                let applied = decode(&reply);
                validation::validate_model(&applied, "RemoteAutomationWriteValueReply").unwrap();
                assert_eq!(
                    applied["stamp"]["transportEpoch"],
                    began["stamp"]["transportEpoch"]
                );
            }
            3
        }
        fn outcome(&self, gesture: &str) -> Value {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(update) = self.events.lock().unwrap().iter().find(|u| {
                    u["gestureID"] == gesture
                        && u["phase"] != "finalizing"
                        && u["phase"] != "writing"
                }) {
                    return update.clone();
                }
                while let Ok(bytes) = self
                    .inbox
                    .ordinary
                    .try_recv()
                    .or_else(|_| self.inbox.priority.try_recv())
                {
                    self.inbox.received(bytes.len());
                    let value: Value = serde_json::from_slice(&bytes).unwrap();
                    if value["event"]["type"] == "automationWrite" {
                        self.events
                            .lock()
                            .unwrap()
                            .push(value["event"]["update"].clone());
                    }
                }
                assert!(Instant::now() < deadline, "no terminal Write event");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
    fn gesture() -> String {
        new_id().unwrap().to_ascii_uppercase()
    }
    #[test]
    fn unadvertised_host_rejects_write_and_capabilities_stay_absent() {
        let h = harness(false);
        let capabilities = h.shared.lock().unwrap().endpoint_capabilities();
        assert!(!capabilities["features"]
            .as_array()
            .unwrap()
            .contains(&json!("automation-write")));
        h.play();
        let reply = h.begin(&gesture());
        assert_eq!(reply["status"], "rejected", "{reply}");
        assert!(h.shared.lock().unwrap().writes.leases.is_empty());
        let enabled = harness(true);
        let capabilities = enabled.shared.lock().unwrap().endpoint_capabilities();
        assert!(capabilities["features"]
            .as_array()
            .unwrap()
            .contains(&json!("automation-write")));
        for op in OPERATIONS {
            assert!(capabilities["editorOperations"]
                .as_array()
                .unwrap()
                .contains(&json!(op)));
        }
    }
    #[test]
    fn end_commits_one_authored_undo_and_merges_into_concurrent_unrelated_edit() {
        let h = harness(true);
        h.play();
        // Begin requires exactly the current version of the target track.
        let stale = h.send(
            editor_op(
                "automation.writeBegin",
                &RemoteAutomationWriteBeginRequest {
                    gesture_id: gesture(),
                    track_id: h.track.clone(),
                    target: soundroom_client::models::RemoteAutomationWriteTarget {
                        kind: soundroom_client::models::RemoteAutomationWriteTargetKind::Volume,
                        effect_id: None,
                        parameter_id: None,
                    },
                    mode: soundroom_client::models::RemoteAutomationWriteMode::Touch,
                },
            ),
            json!({ &h.track: h.version() + 7 }),
        );
        assert_eq!(stale["status"], "conflict", "{stale}");
        let g = gesture();
        let last = h.record(&g);
        // A second Begin on the same target from the same owner conflicts.
        assert_eq!(h.begin(&gesture())["status"], "conflict");
        // Another author renames the track after release, before the lane
        // commit: the worker retries and preserves that unrelated edit.
        let editor = h.owner.clone();
        let other = Identity {
            runtime: new_id().unwrap(),
            ..editor
        };
        let global = h.shared.lock().unwrap().global();
        let (other_out, other_inbox) = Outbox::pair(global);
        h.shared
            .lock()
            .unwrap()
            .join(other.clone(), other_out.clone())
            .unwrap();
        let fired = Arc::new(AtomicBool::new(false));
        let hook_shared = Arc::downgrade(&h.shared);
        let hook_track = h.track.clone();
        let hook_fired = fired.clone();
        let hook_author = other.runtime.clone();
        h.shared.lock().unwrap().edit_hook = Some(Arc::new(move || {
            if hook_fired.swap(true, Ordering::AcqRel) {
                return;
            }
            let shared = hook_shared.upgrade().unwrap();
            let mut state = shared.lock().unwrap();
            let version = state.authority.track_revisions[&hook_track];
            let mut session = state.authority.session.clone();
            session.tracks[0].name = "Renamed by another author".into();
            state
                .authority
                .edit_session(
                    &hook_author,
                    soundroom_engine::authority::Role::Editor,
                    None,
                    &BTreeMap::from([(hook_track.clone(), version)]),
                    session,
                )
                .unwrap();
        }));
        let ended = h.end(&g, last, false);
        assert_eq!(ended["status"], "accepted", "{ended}");
        let update = decode(&ended);
        validation::validate_model(&update, "RemoteAutomationWriteLeaseUpdate").unwrap();
        assert_eq!(update["phase"], "committed", "{update}");
        assert!(
            fired.load(Ordering::Acquire),
            "unrelated edit raced the commit"
        );
        let revision = update["revision"].as_u64().unwrap();
        let state = h.shared.lock().unwrap();
        assert_eq!(state.authority.revision, revision);
        assert_eq!(
            state.authority.session.tracks[0].name,
            "Renamed by another author"
        );
        drop(state);
        let lane = h.volume_lane();
        assert!(!lane.is_empty(), "committed lane");
        assert!(lane.iter().any(|p| (p.value - 0.6).abs() < 0.01));
        // Owner-only event carried the same committed outcome.
        assert_eq!(h.outcome(&g)["revision"], update["revision"]);
        assert!(other_inbox
            .priority
            .try_iter()
            .chain(other_inbox.ordinary.try_iter())
            .all(|bytes| {
                let value: Value = serde_json::from_slice(&bytes).unwrap();
                value["event"]["type"] != "automationWrite"
            }));
        // A second gesture right after the commit waits for the new graph.
        let second = gesture();
        let last2 = h.record(&second);
        let ended2 = decode(&h.end(&second, last2, false));
        assert_eq!(ended2["phase"], "committed", "{ended2}");
        let undo = h.send(json!({"type":"undo"}), json!({}));
        assert_eq!(undo["status"], "accepted", "{undo}");
        // A retried End returns the retained outcome, never a second authoring.
        assert_eq!(
            decode(&h.end(&g, last, false))["revision"],
            update["revision"]
        );
        // One original-author Undo restores the previous lane only.
        let undo = h.send(json!({"type":"undo"}), json!({}));
        assert_eq!(undo["status"], "accepted", "{undo}");
        assert!(h.volume_lane().is_empty());
        assert_eq!(
            h.shared.lock().unwrap().authority.session.tracks[0].name,
            "Renamed by another author"
        );
        h.shared.lock().unwrap().edit_hook = None;
        h.shared.lock().unwrap().leave(&other, &other_out);
    }
    #[test]
    fn target_conflict_retains_owner_runtime_scoped_verified_draft() {
        let h = harness(true);
        h.play();
        let g = gesture();
        let last = h.record(&g);
        // A foreign runtime of the same credential cannot drive this lease.
        let other = Identity {
            runtime: new_id().unwrap(),
            ..h.owner.clone()
        };
        let global = h.shared.lock().unwrap().global();
        let (other_out, other_inbox) = Outbox::pair(global);
        h.shared
            .lock()
            .unwrap()
            .join(other.clone(), other_out.clone())
            .unwrap();
        let foreign = h.send_as(
            &other,
            &other_out,
            &other_inbox,
            editor_op(
                "automation.writeValue",
                &RemoteAutomationWriteValueRequest {
                    gesture_id: g.clone(),
                    sequence: 9,
                    value: 0.9,
                },
            ),
            json!({}),
        );
        assert_eq!(foreign["status"], "conflict", "{foreign}");
        // The leased target changes underneath (static gain is part of Volume Read).
        let gain = h.send_as(
            &other,
            &other_out,
            &other_inbox,
            json!({"type":"setTrackGain","trackID":h.track,"gain":0.5}),
            json!({ &h.track: h.version() }),
        );
        assert_eq!(gain["status"], "accepted", "{gain}");
        let ended = h.end(&g, last, false);
        let update = if ended["payload"].is_string() {
            decode(&ended)
        } else {
            h.outcome(&g)
        };
        let update = if update["phase"] == "finalizing" {
            h.outcome(&g)
        } else {
            update
        };
        assert!(
            matches!(update["phase"].as_str(), Some("conflict")),
            "{update}"
        );
        assert!(h.volume_lane().is_empty(), "nothing authored");
        let draft_id = update["draftID"]
            .as_str()
            .expect("review draft")
            .to_string();
        let request = |peer: &Identity, out: &Outbox, inbox: &Inbox| {
            h.send_as(
                peer,
                out,
                inbox,
                editor_op(
                    "automation.writeDraft",
                    &RemoteAutomationWriteDraftRequest {
                        draft_id: Some(draft_id.clone()),
                        gesture_id: None,
                    },
                ),
                json!({}),
            )
        };
        let denied = request(&other, &other_out, &other_inbox);
        assert_eq!(denied["status"], "rejected", "{denied}");
        let artifact_denied = h.send_as(
            &other,
            &other_out,
            &other_inbox,
            json!({"type":"readArtifact","id":draft_id,"offset":0,"bytes":262144}),
            json!({}),
        );
        assert_eq!(artifact_denied["status"], "rejected");
        let reply = request(&h.owner, &h.out, &h.inbox);
        assert_eq!(reply["status"], "accepted", "{reply}");
        let draft = decode(&reply);
        validation::validate_model(&draft, "RemoteAutomationWriteDraftReply").unwrap();
        assert_eq!(draft["appliedValueCount"], 3);
        let mut body = Vec::new();
        loop {
            let chunk = h.send(
                json!({"type":"readArtifact","id":draft_id,"offset":body.len(),"bytes":262144}),
                json!({}),
            );
            let chunk = decode(&chunk);
            body.extend(
                base64::engine::general_purpose::STANDARD
                    .decode(chunk["data"].as_str().unwrap())
                    .unwrap(),
            );
            if chunk["complete"] == true {
                break;
            }
        }
        assert_eq!(hex(&Sha256::digest(&body)), draft["artifact"]["sha256"]);
        let content: Value = serde_json::from_slice(&body).unwrap();
        validation::validate_model(&content, "RemoteAutomationWriteDraftContent").unwrap();
        assert_eq!(
            content["capturedRamps"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| !r["sequence"].is_null())
                .count(),
            3
        );
        // Lookup by known gesture also works for the owner runtime only.
        let by_gesture = h.send(
            editor_op(
                "automation.writeDraft",
                &RemoteAutomationWriteDraftRequest {
                    draft_id: None,
                    gesture_id: Some(g.to_lowercase()),
                },
            ),
            json!({}),
        );
        assert_eq!(decode(&by_gesture)["draftID"], draft_id.as_str());
    }
    #[test]
    fn owner_disconnect_restores_read_and_keeps_draft_without_authoring() {
        let h = harness(true);
        h.play();
        let g = gesture();
        h.record(&g);
        let revision = h.shared.lock().unwrap().authority.revision;
        h.shared
            .lock()
            .unwrap()
            .leave(&h.owner.clone(), &h.out.clone());
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let state = h.shared.lock().unwrap();
            if let Some(outcome) = state.writes.outcomes.iter().find(|o| o.gesture == g) {
                assert_eq!(
                    outcome.update.phase,
                    RemoteAutomationWritePhase::Disconnected
                );
                assert!(outcome.update.draft_id.is_some());
                assert_eq!(state.authority.revision, revision, "nothing authored");
                assert!(state.writes.leases.is_empty());
                break;
            }
            drop(state);
            assert!(Instant::now() < deadline, "disconnect outcome");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(h.volume_lane().is_empty());
    }
}
