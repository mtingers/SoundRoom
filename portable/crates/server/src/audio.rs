//! Callback-backed control bridge. This module does not open an audio device.
//! Preparation and pointer reclamation stay on workers; the callback publishes the clock.
use crate::{
    service::{Service, Shared},
    Error,
};
use soundroom_engine::{
    paged::PagedLoader,
    project::Project,
    realtime::{GraphControl, Telemetry},
    render::{CheckpointCache, LoopRange, PreparedGraph},
};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{sync_channel, Receiver, SyncSender},
        Arc, Mutex, OnceLock, Weak,
    },
    time::{Duration, Instant},
};
#[path = "audio_updates.rs"]
mod updates;
#[path = "audio_write.rs"]
pub mod write;
const NO_REVISION: u64 = u64::MAX;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphRevisions {
    pub authored: u64,
    pub pending: Option<u64>,
    pub applied: Option<u64>,
}
#[derive(Clone, Copy, Debug)]
pub enum Command {
    Play,
    Pause,
    Seek(u64),
}
#[derive(Clone)]
pub struct Backend {
    inner: Arc<Inner>,
}
struct Inner {
    priority: SyncSender<Message>,
    ordinary: SyncSender<Message>,
    epoch: AtomicU64,
    authored_revision: AtomicU64,
    busy: AtomicBool,
    playing: AtomicBool,
    current_cancel: Mutex<Option<Arc<AtomicBool>>>,
    canonical: Mutex<Option<Canonical>>,
    loader: Mutex<Option<Arc<Mutex<PagedLoader>>>>,
    stop: Arc<AtomicBool>,
    updates: updates::Queue,
    telemetry: Mutex<Option<Telemetry>>,
    pending_revision: AtomicU64,
    applied_revision: AtomicU64,
    update_status: Mutex<Option<(u64, String)>>,
    driver_thread: OnceLock<std::thread::Thread>,
    write: write::Sender,
    /// Transport cycle; graphs are prepared with it and cached per range.
    loop_range: Mutex<Option<LoopRange>>,
    /// Latest actual callback timestamp anchor (recording alignment input).
    anchor: Mutex<Option<soundroom_engine::capture_clock::OutputClockAnchor>>,
    preview: soundroom_engine::preview::PreviewControl,
    count_in: soundroom_engine::realtime::CountIn,
    metronome: soundroom_engine::realtime::Metronome,
    click: Mutex<Click>,
    live: soundroom_engine::realtime::LivePublisher,
    /// The published live patch awaiting the callback: (taken count that
    /// includes it, rejections before it, its revision).
    live_pending: Mutex<Option<(u64, u64, u64)>>,
}
/// The transport click: the project's grid, the operator's toggle, and a
/// recording that asked for the click until the transport next stops.
struct Click {
    grid: soundroom_engine::metronome::Grid,
    enabled: bool,
    recording: bool,
}
impl Inner {
    fn update_click(&self, change: impl FnOnce(&mut Click)) {
        if let Ok(mut click) = self.click.lock() {
            change(&mut click);
            self.metronome
                .set((click.enabled || click.recording).then_some(click.grid));
        }
    }
    // Called only by control/preparation workers. The audio callback keeps its
    // existing atomic telemetry path and never wakes or locks this thread.
    fn wake_driver(&self) {
        if let Some(driver) = self.driver_thread.get() {
            driver.unpark();
        }
    }
    fn set_status(&self, revision: u64, message: String) {
        if let Ok(mut status) = self.update_status.lock() {
            if self.authored_revision.load(Ordering::Acquire) == revision {
                *status = Some((revision, message.chars().take(512).collect()));
            }
        }
    }
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.wake_driver();
        if let Ok(cancel) = self.current_cancel.lock() {
            if let Some(cancel) = cancel.as_ref() {
                cancel.store(true, Ordering::Release);
            }
        }
    }
}
pub(crate) fn audible_variant(session: &soundroom_engine::model::Session) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    session.tracks.len().hash(&mut hasher);
    for track in &session.tracks {
        track.id.hash(&mut hasher);
        track.muted.hash(&mut hasher);
        track.solo.hash(&mut hasher);
    }
    hasher.finish()
}
struct Canonical {
    revision: u64,
    /// Which tracks are audible: the same revision may be heard soloed (audition)
    /// or with a recording click track, and must not reuse the other's graph.
    variant: u64,
    loop_range: Option<LoopRange>,
    graph: PreparedGraph,
    checkpoints: CheckpointCache,
    loader: Arc<Mutex<PagedLoader>>,
}
type Completion = SyncSender<Result<Telemetry, Error>>;
enum Message {
    Playing {
        playing: bool,
        epoch: u64,
        reply: Completion,
    },
    Aligned {
        graph: Box<PreparedGraph>,
        revision: u64,
        epoch: u64,
        baseline: u64,
        clock: u64,
        cancel: Arc<AtomicBool>,
        loader: Arc<Mutex<PagedLoader>>,
        reply: Completion,
    },
    Publish {
        graph: Box<PreparedGraph>,
        revision: u64,
        epoch: u64,
        resume: bool,
        cancel: Arc<AtomicBool>,
        reply: Completion,
    },
}
struct Pending {
    reply: Completion,
    sequence: u64,
    token: Option<u64>,
    revision: Option<u64>,
    playing: bool,
    cancel: Option<Arc<AtomicBool>>,
    deadline: Instant,
    live: bool,
    loader: Option<Arc<Mutex<PagedLoader>>>,
}
fn protocol(s: &str) -> Error {
    Error::Protocol(s.into())
}
impl Backend {
    pub fn attach(shared: &Shared, control: GraphControl) -> Self {
        let (priority, pr) = sync_channel(8);
        let (ordinary, or) = sync_channel(8);
        let stop = Arc::new(AtomicBool::new(false));
        let (updates, update_wake) = updates::channel();
        let (write, write_driver) = write::channel();
        let preview = control.preview();
        let count_in = control.count_in();
        let metronome = control.metronome();
        let live = control.live_publisher();
        let inner = Arc::new(Inner {
            preview,
            count_in,
            metronome,
            click: Mutex::new(Click {
                grid: soundroom_engine::metronome::Grid::new(120., 4, 4),
                enabled: false,
                recording: false,
            }),
            live,
            live_pending: Mutex::new(None),
            priority,
            ordinary,
            epoch: AtomicU64::new(1),
            authored_revision: AtomicU64::new(0),
            busy: AtomicBool::new(false),
            playing: AtomicBool::new(false),
            current_cancel: Mutex::new(None),
            canonical: Mutex::new(None),
            loader: Mutex::new(None),
            stop: stop.clone(),
            updates,
            telemetry: Mutex::new(None),
            pending_revision: AtomicU64::new(NO_REVISION),
            applied_revision: AtomicU64::new(NO_REVISION),
            update_status: Mutex::new(None),
            driver_thread: OnceLock::new(),
            loop_range: Mutex::new(None),
            anchor: Mutex::new(None),
            write,
        });
        let weak = Arc::downgrade(shared);
        let backend = Arc::downgrade(&inner);
        let pages = Arc::downgrade(&inner);
        let page_stop = stop.clone();
        let update_backend = backend.clone();
        std::thread::spawn(move || updates::run(update_backend, update_wake));
        std::thread::spawn(move || pump_pages(pages, page_stop));
        std::thread::spawn(move || drive(weak, backend, control, pr, or, stop, write_driver));
        Self { inner }
    }
    pub(crate) fn observe_revision(&self, revision: u64) {
        self.inner
            .authored_revision
            .store(revision, Ordering::Release);
    }
    /// Trusted host adapter; protocol identity, target/version proof and role
    /// validation remain in Service. This API never acknowledges an audio value.
    pub fn enqueue_write(&self, action: write::Action, priority: bool) -> Result<(), Error> {
        self.inner.write.send(action, priority)?;
        self.inner.wake_driver();
        Ok(())
    }
    /// Worker preparation from the applied canonical graph. Authored ambiguous
    /// source frames are declined without changing Read or normalizing metadata.
    pub fn prepare_write(
        &self,
        target: &soundroom_engine::automation_write::WriteTarget,
        points: &[soundroom_engine::model::AutomationPoint],
        revision: u64,
    ) -> Result<write::Prepared, Error> {
        if points.len() > crate::write_curve::MAX_LANE_POINTS
            || points
                .iter()
                .map(|p| p.frame)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != points.len()
        {
            return Err(protocol(
                "Ambiguous or oversized original Write lane requires review",
            ));
        }
        let revisions = self.graph_revisions();
        if revisions.authored != revision
            || revisions.applied != Some(revision)
            || revisions.pending.is_some()
        {
            return Err(protocol(
                "Audio must apply the current authored graph before Write",
            ));
        }
        let canonical = self
            .inner
            .canonical
            .lock()
            .map_err(|_| protocol("Write graph is unavailable"))?;
        let canonical = canonical
            .as_ref()
            .filter(|c| c.revision == revision)
            .ok_or_else(|| protocol("Write graph is stale"))?;
        let resolved = canonical
            .graph
            .resolve_write_target(target)
            .map_err(|e| protocol(&e.to_string()))?;
        let mapping = canonical
            .graph
            .write_mapping(resolved)
            .map_err(|e| protocol(&e.to_string()))?;
        let original = soundroom_engine::automation::Lane::new(
            points,
            mapping.baseline_normalized,
            mapping.interpolation_scale
                == soundroom_engine::automation_write::WriteInterpolation::Decibels,
        );
        write::Prepared::new(resolved, mapping, original)
    }
    /// Takes effect at the next prepared Play/Seek; the caller restarts playback.
    pub fn set_loop(&self, range: Option<LoopRange>) {
        if let Ok(mut current) = self.inner.loop_range.lock() {
            *current = range;
        }
    }
    /// Actual callback timing for recording alignment; polling may skip anchors.
    pub fn output_anchor(&self) -> Option<soundroom_engine::capture_clock::OutputClockAnchor> {
        self.inner.anchor.lock().ok().and_then(|v| *v)
    }
    /// Arms sample-accurate count-in clicks for the next Play.
    pub fn arm_count_in(&self, clicks: Vec<f32>) {
        self.inner.count_in.arm(soundroom_engine::preview::Buffer {
            right: clicks.clone(),
            left: clicks,
            looping: false,
            gain: 1.,
            ..Default::default()
        });
    }
    /// Turns the transport click on or off; it applies to the running graph.
    pub fn set_metronome(&self, enabled: bool) {
        self.inner.update_click(|click| click.enabled = enabled);
    }
    pub fn set_metronome_grid(&self, grid: soundroom_engine::metronome::Grid) {
        self.inner.update_click(|click| click.grid = grid);
    }
    /// A recording's click lasts until the transport next stops.
    pub fn metronome_until_stop(&self) {
        self.inner.update_click(|click| click.recording = true);
    }
    pub fn preview(&self) -> soundroom_engine::preview::PreviewControl {
        self.inner.preview.clone()
    }
    pub fn telemetry(&self) -> Option<Telemetry> {
        self.inner.telemetry.lock().ok().and_then(|v| *v)
    }
    pub fn loop_range(&self) -> Option<LoopRange> {
        self.inner.loop_range.lock().ok().and_then(|v| *v)
    }
    pub fn graph_revisions(&self) -> GraphRevisions {
        let revision = |value: u64| (value != NO_REVISION).then_some(value);
        GraphRevisions {
            authored: self.inner.authored_revision.load(Ordering::Acquire),
            pending: revision(self.inner.pending_revision.load(Ordering::Acquire)),
            applied: revision(self.inner.applied_revision.load(Ordering::Acquire)),
        }
    }
    /// Applies a parameter-only edit to the playing graph instead of preparing
    /// and splicing a new one, so insert state carries on. Declines (false)
    /// unless playback is steady: no transport job and no graph update pending.
    pub(crate) fn publish_live(
        &self,
        patch: soundroom_engine::render::LivePatch,
        mut canonical_patch: soundroom_engine::render::LivePatch,
        revision: u64,
        variant: u64,
    ) -> bool {
        if !self.inner.playing.load(Ordering::Acquire)
            || self.inner.busy.load(Ordering::Acquire)
            || self.inner.pending_revision.load(Ordering::Acquire) != NO_REVISION
        {
            return false;
        }
        // The cached position-0 graph serves seeks and Write: patch it too, or
        // drop it so it is rebuilt rather than reverting the edit.
        let Ok(mut canonical) = self.inner.canonical.try_lock() else {
            return false;
        };
        // It was prepared with the playing graph (play, seek or update), so the
        // same patch applies; apply_live refuses a structural mismatch.
        let patched = canonical
            .as_mut()
            .is_some_and(|c| c.graph.apply_live(&mut canonical_patch));
        if patched {
            let c = canonical.as_mut().unwrap();
            c.revision = revision;
            // Mute and solo are live edits now: the patched graph has them.
            c.variant = variant;
            // Checkpoints hold the old parameters' DSP state.
            c.checkpoints = CheckpointCache::new(32 * 1024 * 1024);
        } else {
            *canonical = None;
        }
        drop(canonical);
        // Older in-flight splices would revert the edit; this revision supersedes them.
        self.observe_revision(revision);
        let (taken, rejected) = self.inner.live.outcomes();
        if let Ok(mut pending) = self.inner.live_pending.lock() {
            *pending = Some((taken + 1, rejected, revision));
        }
        self.inner.live.publish(Box::new(patch));
        true
    }
    pub(crate) fn schedule_update(&self, source: crate::service::GraphSource, revision: u64) {
        self.observe_revision(revision);
        self.inner
            .pending_revision
            .store(revision, Ordering::Release);
        self.inner
            .updates
            .schedule(source, revision, self.inner.epoch.load(Ordering::Acquire));
    }
    #[cfg(test)]
    pub(crate) fn inner_telemetry_for_test(&self) -> Option<Telemetry> {
        *self.inner.telemetry.lock().unwrap()
    }
    pub fn active(&self) -> bool {
        self.inner.busy.load(Ordering::Acquire) || self.inner.playing.load(Ordering::Acquire)
    }
    pub fn execute(
        &self,
        command: Command,
        project: Box<Project>,
        revision: u64,
        cancel: Arc<AtomicBool>,
    ) -> Result<Telemetry, Error> {
        self.execute_tracked(command, project, revision, cancel, None)
    }
    /// Reports this preparation's own generation, even when Stop supersedes it.
    pub(crate) fn execute_recording(
        &self,
        command: Command,
        project: Box<Project>,
        revision: u64,
        cancel: Arc<AtomicBool>,
        generation: &AtomicU64,
    ) -> Result<Telemetry, Error> {
        self.execute_tracked(command, project, revision, cancel, Some(generation))
    }
    /// Cleanup must never supersede the explicit Stop that cancelled startup.
    pub(crate) fn pause_if_current(&self, generation: u64) -> Result<Option<Telemetry>, Error> {
        self.inner.count_in.cancel();
        self.inner.update_click(|click| click.recording = false);
        let next = generation
            .checked_add(1)
            .ok_or_else(|| protocol("Transport generation exhausted"))?;
        if self
            .inner
            .epoch
            .compare_exchange(generation, next, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(None);
        }
        self.inner.updates.cancel();
        if let Ok(current) = self.inner.current_cancel.lock() {
            if let Some(current) = current.as_ref() {
                current.store(true, Ordering::Release);
            }
        }
        self.playing(false, next).map(Some)
    }
    fn execute_tracked(
        &self,
        command: Command,
        project: Box<Project>,
        revision: u64,
        cancel: Arc<AtomicBool>,
        generation: Option<&AtomicU64>,
    ) -> Result<Telemetry, Error> {
        if matches!(command, Command::Pause) {
            self.inner.count_in.cancel();
            self.inner.update_click(|click| click.recording = false);
            let epoch = self.inner.epoch.fetch_add(1, Ordering::AcqRel) + 1;
            self.inner.updates.cancel();
            if let Ok(current) = self.inner.current_cancel.lock() {
                if let Some(current) = current.as_ref() {
                    current.store(true, Ordering::Release);
                }
            }
            return self.playing(false, epoch);
        }
        let observed_epoch = self.inner.epoch.load(Ordering::Acquire);
        if cancel.load(Ordering::Acquire) {
            return Err(protocol("Audio preparation was stopped"));
        }
        if self
            .inner
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Error::QueueFull);
        }
        struct Busy(Arc<Inner>);
        impl Drop for Busy {
            fn drop(&mut self) {
                self.0.busy.store(false, Ordering::Release);
                if let Ok(mut cancel) = self.0.current_cancel.lock() {
                    *cancel = None;
                }
            }
        }
        let _busy = Busy(self.inner.clone());
        *self
            .inner
            .current_cancel
            .lock()
            .map_err(|_| protocol("Audio preparation state is unavailable"))? =
            Some(cancel.clone());
        let epoch = observed_epoch
            .checked_add(1)
            .ok_or_else(|| protocol("Transport generation exhausted"))?;
        if cancel.load(Ordering::Acquire)
            || self
                .inner
                .epoch
                .compare_exchange(observed_epoch, epoch, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return Err(protocol("Audio preparation was stopped"));
        }
        if let Some(generation) = generation {
            generation.store(epoch, Ordering::Release);
        }
        self.inner.updates.cancel();
        let was_playing = self.inner.playing.load(Ordering::Acquire);
        let frozen = self.playing(false, epoch)?;
        let cycle = self.loop_range();
        let target = match command {
            Command::Seek(frame) => frame,
            Command::Play => frozen.frame,
            Command::Pause => unreachable!(),
        };
        // Starting outside the cycle starts at its beginning, as on the Swift host.
        let target = match cycle {
            Some(range) if target < range.start || target >= range.end => range.start,
            _ => target,
        };
        if cancel.load(Ordering::Acquire) || self.inner.epoch.load(Ordering::Acquire) != epoch {
            return Err(protocol("Audio preparation was stopped"));
        }
        // One preparation worker holds this cache. Stop and callback telemetry never lock it.
        let mut canonical = self
            .inner
            .canonical
            .lock()
            .map_err(|_| protocol("Canonical audio cache is unavailable"))?;
        let variant = audible_variant(&project.session);
        if canonical
            .as_ref()
            .map(|c| (c.revision, c.loop_range, c.variant))
            != Some((revision, cycle, variant))
        {
            let (graph, loader) =
                PreparedGraph::prepare_paged(&project, 32 * 1024 * 1024, &cancel, cycle)
                    .map_err(|e| protocol(&e.to_string()))?;
            let loader = Arc::new(Mutex::new(loader));
            *canonical = Some(Canonical {
                revision,
                variant,
                loop_range: cycle,
                graph,
                checkpoints: CheckpointCache::new(32 * 1024 * 1024),
                loader,
            });
        }
        let canonical = canonical.as_mut().unwrap();
        let mut graph = canonical.graph.fork();
        graph
            .seek(target, &canonical.checkpoints, &cancel)
            .map_err(|e| protocol(&e.to_string()))?;
        // A huge DSP state can exceed the checkpoint budget; playback remains valid without it.
        let _ = canonical.checkpoints.store(&graph);
        canonical
            .loader
            .lock()
            .map_err(|_| protocol("Page loader is unavailable"))?
            .prepare_graph(&graph, 32768, &cancel)
            .map_err(|e| protocol(&e.to_string()))?;
        *self
            .inner
            .loader
            .lock()
            .map_err(|_| protocol("Page loader state is unavailable"))? =
            Some(canonical.loader.clone());
        if cancel.load(Ordering::Acquire) || self.inner.epoch.load(Ordering::Acquire) != epoch {
            return Err(protocol("Audio preparation was stopped"));
        }
        let (tx, rx) = sync_channel(1);
        let resume = matches!(command, Command::Play) || was_playing;
        self.inner
            .ordinary
            .try_send(Message::Publish {
                graph: Box::new(graph),
                revision,
                epoch,
                resume,
                cancel,
                reply: tx,
            })
            .map_err(|_| Error::QueueFull)?;
        self.inner.wake_driver();
        wait(rx, Duration::from_secs(3))
    }
    fn playing(&self, playing: bool, epoch: u64) -> Result<Telemetry, Error> {
        let (tx, rx) = sync_channel(1);
        self.inner
            .priority
            .try_send(Message::Playing {
                playing,
                epoch,
                reply: tx,
            })
            .map_err(|_| Error::QueueFull)?;
        self.inner.wake_driver();
        wait(rx, Duration::from_secs(3))
    }
}
fn wait(rx: Receiver<Result<Telemetry, Error>>, timeout: Duration) -> Result<Telemetry, Error> {
    rx.recv_timeout(timeout).map_err(|e| match e {
        std::sync::mpsc::RecvTimeoutError::Timeout => Error::Timeout,
        _ => Error::Disconnected,
    })?
}
/// Marks a live patch's revision applied once the callback took it. A patch
/// the callback refused (its graph changed underneath) falls back to a full
/// graph update, so playback never keeps stale parameters.
fn settle_live(inner: &Inner, service: &Mutex<Service>) {
    let Ok(mut pending) = inner.live_pending.lock() else {
        return;
    };
    let Some((target, rejected_before, revision)) = *pending else {
        return;
    };
    let (taken, rejected) = inner.live.outcomes();
    if taken < target {
        return;
    }
    if rejected == rejected_before {
        inner.applied_revision.store(revision, Ordering::Release);
        *pending = None;
    } else if let Ok(mut state) = service.try_lock() {
        *pending = None;
        drop(pending);
        state.rebuild_audio();
    }
}
fn drive(
    shared: Weak<Mutex<Service>>,
    backend: Weak<Inner>,
    mut control: GraphControl,
    priority: Receiver<Message>,
    ordinary: Receiver<Message>,
    stop: Arc<AtomicBool>,
    mut writes: write::Driver,
) {
    let mut pending: Vec<Pending> = vec![];
    let mut revisions = std::collections::VecDeque::<(u64, u64)>::new();
    let mut applied_token = 0;
    // The engine's cancellation mailbox names one exact token. Keep only one
    // unacknowledged publication, including after its request deadline, so a
    // later cancellation cannot overwrite an older callback-owned candidate.
    let mut inflight = None;
    let mut last_publish = Instant::now();
    if let Some(inner) = backend.upgrade() {
        let _ = inner.driver_thread.set(std::thread::current());
    }
    while !stop.load(Ordering::Acquire) {
        let Some(inner) = backend.upgrade() else {
            break;
        };
        let Some(service) = shared.upgrade() else {
            break;
        };
        let mut processed = 0;
        for _ in 0..8 {
            let message = priority
                .try_recv()
                .ok()
                .or_else(|| ordinary.try_recv().ok());
            let Some(message) = message else {
                break;
            };
            processed += 1;
            match message {
                Message::Playing {
                    playing,
                    epoch,
                    reply,
                } => {
                    if inner.epoch.load(Ordering::Acquire) != epoch {
                        let _ = reply.try_send(Err(protocol("Transport request was superseded")));
                        continue;
                    }
                    if !playing {
                        if let Some(token) = inflight {
                            control.cancel_handoff(token);
                        }
                        for p in pending.drain(..) {
                            if let Some(token) = p.token {
                                control.cancel_handoff(token);
                            }
                            let _ = p
                                .reply
                                .try_send(Err(protocol("Transport request was stopped")));
                        }
                    }
                    if pending.len() >= 8 {
                        let _ = reply.try_send(Err(Error::QueueFull));
                        continue;
                    }
                    match control.request_playing(playing) {
                        Some(sequence) => pending.push(Pending {
                            reply,
                            sequence,
                            token: None,
                            revision: None,
                            playing,
                            cancel: None,
                            deadline: Instant::now() + Duration::from_secs(2),
                            live: false,
                            loader: None,
                        }),
                        None => {
                            let _ =
                                reply.try_send(Err(protocol("Audio command sequence exhausted")));
                        }
                    }
                }
                Message::Aligned {
                    graph,
                    revision,
                    epoch,
                    baseline,
                    clock,
                    cancel,
                    loader,
                    reply,
                } => {
                    let state = control.telemetry();
                    let valid = inner.authored_revision.load(Ordering::Acquire) == revision
                        && inner.epoch.load(Ordering::Acquire) == epoch
                        && !cancel.load(Ordering::Acquire)
                        && state.is_some_and(|state| {
                            state.playing
                                && state.handoff_token == baseline
                                && state.playback_clock <= clock
                        });
                    if !valid {
                        let _ = reply.try_send(Err(Error::Conflict(
                            "Prepared splice was superseded or late".into(),
                        )));
                        continue;
                    }
                    if pending.len() >= 8 || inflight.is_some() {
                        let _ = reply.try_send(Err(Error::QueueFull));
                        continue;
                    }
                    match control.publish_aligned_on(graph, clock, baseline) {
                        Ok(token) => {
                            inflight = Some(token);
                            revisions.push_back((token, revision));
                            pending.push(Pending {
                                reply,
                                sequence: state.unwrap().command_sequence,
                                token: Some(token),
                                revision: Some(revision),
                                playing: true,
                                cancel: Some(cancel),
                                deadline: Instant::now() + Duration::from_secs(2),
                                live: true,
                                loader: Some(loader),
                            });
                        }
                        Err(_) => {
                            let _ = reply.try_send(Err(Error::QueueFull));
                        }
                    }
                }
                Message::Publish {
                    graph,
                    revision,
                    epoch,
                    resume,
                    cancel,
                    reply,
                } => {
                    let valid = inner.authored_revision.load(Ordering::Acquire) == revision
                        && inner.epoch.load(Ordering::Acquire) == epoch
                        && !cancel.load(Ordering::Acquire);
                    if !valid {
                        let _ = reply.try_send(Err(Error::Conflict(
                            "Session or transport changed while preparing audio".into(),
                        )));
                        continue;
                    }
                    if pending.len() >= 8 || inflight.is_some() {
                        let _ = reply.try_send(Err(Error::QueueFull));
                        continue;
                    }
                    match control.publish(graph) {
                        Ok(token) => {
                            inflight = Some(token);
                            revisions.push_back((token, revision));
                            if let Some(sequence) = control.request_playing(resume) {
                                pending.push(Pending {
                                    reply,
                                    sequence,
                                    token: Some(token),
                                    revision: Some(revision),
                                    playing: resume,
                                    cancel: Some(cancel),
                                    deadline: Instant::now() + Duration::from_secs(2),
                                    live: false,
                                    loader: None,
                                });
                            } else {
                                control.cancel_handoff(token);
                                let _ = reply
                                    .try_send(Err(protocol("Audio command sequence exhausted")));
                            }
                        }
                        Err(_) => {
                            let _ = reply.try_send(Err(Error::QueueFull));
                        }
                    }
                }
            }
        }
        writes.tick(&mut control);
        inner.updates.cancel_expired();
        if let Some(telemetry) = control.telemetry() {
            if inflight.is_some_and(|token| {
                telemetry.handoff_token == token || telemetry.rejected_handoff == token
            }) {
                inflight = None;
            }
            if let Ok(mut cache) = inner.telemetry.lock() {
                *cache = Some(telemetry);
            }
            if let Ok(mut anchor) = inner.anchor.lock() {
                *anchor = control.output_anchor();
            }
            settle_live(&inner, &service);
            // Only an adoption sets the applied revision: a live patch applied
            // since then advances it, and must not be reset to the graph's own.
            if let Some((_, revision)) = revisions
                .iter()
                .find(|(token, _)| *token == telemetry.handoff_token)
                .filter(|(token, _)| *token != applied_token)
            {
                inner.applied_revision.store(*revision, Ordering::Release);
                applied_token = telemetry.handoff_token;
                inner
                    .pending_revision
                    .compare_exchange(*revision, NO_REVISION, Ordering::AcqRel, Ordering::Acquire)
                    .ok();
            }
            while revisions.len() > 32 {
                if revisions
                    .front()
                    .is_some_and(|(token, _)| *token == applied_token)
                {
                    let active = revisions.pop_front().unwrap();
                    revisions.push_back(active);
                } else {
                    revisions.pop_front();
                }
            }

            inner.playing.store(telemetry.playing, Ordering::Release);
            let mut i = 0;
            while i < pending.len() {
                let p = &pending[i];
                let rejected = p
                    .token
                    .is_some_and(|token| telemetry.rejected_handoff == token);
                let ack = telemetry.command_sequence >= p.sequence
                    && telemetry.playing == p.playing
                    && p.token
                        .map_or(true, |token| telemetry.handoff_token == token);
                let cancelled = p
                    .cancel
                    .as_ref()
                    .is_some_and(|cancel| cancel.load(Ordering::Acquire));
                let expired = Instant::now() > p.deadline;
                if rejected || ack || expired || cancelled {
                    let p = pending.remove(i);
                    if ack {
                        if let Some(loader) = &p.loader {
                            if let Ok(mut current) = inner.loader.lock() {
                                *current = Some(loader.clone());
                            }
                        }
                        if !p.live {
                            if let Some(revision) = p.revision {
                                inner.set_status(revision, "Ready".into());
                            }
                        }
                    }
                    let result = if rejected {
                        Err(protocol("Audio callback rejected the prepared graph"))
                    } else if expired || cancelled {
                        if let Some(token) = p.token {
                            control.cancel_handoff(token);
                        }
                        if !p.live {
                            let _ = control.request_playing(false);
                        }
                        if cancelled {
                            Err(protocol("Audio request was cancelled"))
                        } else {
                            Err(Error::Timeout)
                        }
                    } else {
                        let valid = p.revision.map_or(true, |revision| {
                            inner.authored_revision.load(Ordering::Acquire) == revision
                        });
                        if valid {
                            Ok(telemetry)
                        } else {
                            Err(Error::Conflict(
                                "Session changed before audio adoption".into(),
                            ))
                        }
                    };
                    let _ = p.reply.try_send(result);
                } else {
                    i += 1;
                }
            }
            if last_publish.elapsed() >= Duration::from_millis(33) {
                if let Ok(mut state) = service.try_lock() {
                    state.audio_telemetry(telemetry);
                    let (left, right) = control.take_peaks();
                    state.audio_meters(left, right);
                    state.publish_auditions();
                    state.finish_completed_punch(service.clone());
                }
                last_publish = Instant::now();
            }
        }
        // Missing callbacks must also expire; absence of telemetry is not an acknowledgement.
        let mut i = 0;
        while i < pending.len() {
            if Instant::now() > pending[i].deadline {
                let p = pending.remove(i);
                if let Some(token) = p.token {
                    control.cancel_handoff(token);
                }
                if !p.live {
                    let _ = control.request_playing(false);
                }
                let _ = p.reply.try_send(Err(Error::Timeout));
            } else {
                i += 1;
            }
        }
        // Retain the latest bounded status until the authority owner is available.
        // Audio acknowledgement itself never waits for this control-plane lock.
        if let Ok(mut status) = inner.update_status.try_lock() {
            if status.is_some() {
                if let Ok(mut state) = service.try_lock() {
                    let (revision, message) = status.take().unwrap();
                    state.audio_update_status(revision, message);
                }
            }
        }
        control.reclaim_retired();
        // Commands wake the parked owner immediately, including a wake that
        // precedes parking. Only an outstanding callback acknowledgement needs
        // a short sampling interval; idle/steady telemetry follows its33ms rate.
        let wait = if pending.is_empty() && inflight.is_none() && !writes.active() {
            Duration::from_millis(33)
        } else {
            Duration::from_millis(1)
        };
        drop(inner);
        drop(service);
        if processed == 8 {
            std::thread::yield_now();
        } else {
            std::thread::park_timeout(wait);
        }
    }
    for p in pending {
        let _ = p.reply.try_send(Err(Error::Disconnected));
    }
    let _ = control.request_playing(false);
    control.reclaim_retired();
}

fn pump_pages(backend: Weak<Inner>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Acquire) {
        let Some(inner) = backend.upgrade() else {
            break;
        };
        let loader = inner.loader.lock().ok().and_then(|loader| loader.clone());
        if let Some(loader) = loader {
            if let Ok(mut pages) = loader.lock() {
                // File decoding is restricted to this worker, never the control/callback thread.
                let _ = pages.service_requests(64, &stop);
            }
        }
        drop(inner);
        std::thread::sleep(Duration::from_millis(3));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_live_handoff_must_be_callback_acknowledged_before_another_publication() {
        let root = std::env::temp_dir().join(format!(
            "soundroom-handoff-{}",
            soundroom_client::auth::new_id().unwrap()
        ));
        crate::registry::private_directory(&root).unwrap();
        crate::registry::Registry::create(&root, "Handoff qualification").unwrap();
        let service = Service::open(&root, None).unwrap();
        let (control, mut host) = soundroom_engine::realtime::channel();
        let backend = Backend::attach(&service, control);
        let worker = backend.clone();
        let (reply, completion) = sync_channel(1);
        let play = std::thread::spawn(move || {
            reply
                .send(worker.execute(
                    Command::Play,
                    Box::new(Project {
                        session: Default::default(),
                        media: Default::default(),
                    }),
                    0,
                    Arc::new(AtomicBool::new(false)),
                ))
                .unwrap();
        });
        let mut left = [0.; 128];
        let mut right = [0.; 128];
        let deadline = Instant::now() + Duration::from_secs(3);
        let baseline = loop {
            host.render(&mut left, &mut right);
            if let Ok(result) = completion.try_recv() {
                break result.unwrap();
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        };
        play.join().unwrap();
        let target = baseline.playback_clock + 9600;
        let make_message = |cancel: Arc<AtomicBool>, reply: Completion| {
            let (mut graph, mut loader) = PreparedGraph::prepare_paged(
                &Project {
                    session: Default::default(),
                    media: Default::default(),
                },
                512 * 1024,
                &cancel,
                None,
            )
            .unwrap();
            graph
                .seek_playback_clock(target, &CheckpointCache::new(0), &cancel)
                .unwrap();
            loader.prepare_graph(&graph, 1024, &cancel).unwrap();
            Message::Aligned {
                graph: Box::new(graph),
                revision: 0,
                epoch: backend.inner.epoch.load(Ordering::Acquire),
                baseline: baseline.handoff_token,
                clock: target,
                cancel,
                loader: Arc::new(Mutex::new(loader)),
                reply,
            }
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let (first_reply, first_result) = sync_channel(1);
        backend
            .inner
            .ordinary
            .try_send(make_message(cancel.clone(), first_reply))
            .unwrap();
        backend.inner.wake_driver();
        // Driver publishes, callback moves it into the waiting future slot,
        // then no callback runs while the request is cancelled.
        std::thread::sleep(Duration::from_millis(30));
        host.render(&mut left, &mut right);
        cancel.store(true, Ordering::Release);
        assert!(wait(first_result, Duration::from_secs(1)).is_err());
        let (second_reply, second_result) = sync_channel(1);
        backend
            .inner
            .ordinary
            .try_send(make_message(Arc::new(AtomicBool::new(false)), second_reply))
            .unwrap();
        backend.inner.wake_driver();
        assert!(matches!(
            wait(second_result, Duration::from_secs(1)),
            Err(Error::QueueFull)
        ));
        // Once callbacks resume, the old exact-token cancellation is still
        // intact; neither candidate can replace the current graph.
        host.render(&mut left, &mut right);
        let deadline = Instant::now() + Duration::from_secs(1);
        let state = loop {
            let state = backend.inner.telemetry.lock().unwrap().unwrap();
            if state.rejected_handoff > baseline.handoff_token {
                break state;
            }
            assert!(
                Instant::now() < deadline,
                "Callback rejection was not observed: {state:?}"
            );
            std::thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(state.handoff_token, baseline.handoff_token);
        assert!(state.rejected_handoff > baseline.handoff_token);
        assert!(state.playing);
        drop(backend);
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn latest_bounded_status_survives_busy_authority_and_discards_old_revision() {
        let root = std::env::temp_dir().join(format!(
            "soundroom-audio-status-{}",
            soundroom_client::auth::new_id().unwrap()
        ));
        crate::registry::private_directory(&root).unwrap();
        crate::registry::Registry::create(&root, "Status qualification").unwrap();
        let service = Service::open(&root, None).unwrap();
        let (control, _host) = soundroom_engine::realtime::channel();
        let backend = Backend::attach(&service, control);
        let held = service.lock().unwrap();
        backend.inner.set_status(0, "Preparing".into());
        backend.inner.set_status(0, "x".repeat(700));
        // The driver is allowed to retry. It cannot discard the result because
        // another admitted control operation happens to own the mutex.
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(
            backend
                .inner
                .update_status
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .1
                .len(),
            512
        );
        drop(held);
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let state = service.lock().unwrap();
            if state.snapshot().unwrap()["status"] == "x".repeat(512) {
                break;
            }
            assert!(Instant::now() < deadline);
            drop(state);
            std::thread::sleep(Duration::from_millis(2));
        }
        backend.observe_revision(1);
        backend.inner.set_status(0, "Old failed preparation".into());
        assert!(backend.inner.update_status.lock().unwrap().is_none());
        drop(backend);
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }
}
