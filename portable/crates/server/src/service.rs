//! Serialized control authority, bounded receipts, immutable worker jobs and media storage.
use crate::{
    registry::{private_directory, valid_name, Registry},
    Error,
};
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use soundroom_client::{
    auth::new_id,
    models::{RemoteCommand, RemotePeer, RemoteRequest, RemoteRole},
    validation,
};
use soundroom_engine::{
    authority::{Authority, ControlEdit, MixControls, Role},
    metronome::Grid,
    model::{Clip, Session, Track},
    project::{Media, Project},
    wave::WaveReader,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
const QUEUE_BYTES: usize = 32 * 1024 * 1024;
const GLOBAL_BYTES: usize = 128 * 1024 * 1024;
pub(crate) const GLOBAL_CONTROL_RESERVE: usize = 2 * 1024 * 1024;
const LIVE_EVENT_BYTES: usize = 64 * 1024;
/// Queued messages above which high-rate readings are skipped for a peer.
const LOSSY_BACKLOG: usize = 16;
#[cfg(test)]
#[path = "service_artifact_tests.rs"]
mod artifact_tests;
#[path = "service_autosave.rs"]
mod autosave;
#[path = "service_media.rs"]
mod custody;
#[path = "session_edits.rs"]
mod edits;
#[path = "service_master.rs"]
mod master;
#[path = "service_names.rs"]
mod names;
#[path = "service_pitch.rs"]
mod pitch;
#[path = "service_preview.rs"]
mod preview;
#[path = "service_process.rs"]
mod process;
#[cfg(target_os = "macos")]
#[path = "service_projects.rs"]
mod projects;
#[path = "service_record.rs"]
pub mod record;
#[cfg(all(test, unix))]
#[path = "request_acceptance.rs"]
mod request_acceptance;
#[path = "service_separation.rs"]
mod separation;
#[path = "service_waveform.rs"]
mod waveform;
#[path = "service_write.rs"]
mod writes;
#[derive(Clone)]
pub struct Outbox {
    priority: std::sync::mpsc::SyncSender<Vec<u8>>,
    ordinary: std::sync::mpsc::SyncSender<Vec<u8>>,
    bytes: Arc<AtomicUsize>,
    global: Arc<AtomicUsize>,
    /// Messages queued but not yet taken by the socket writer.
    pending: Arc<AtomicUsize>,
    pub closed: Arc<AtomicBool>,
    snapshot_active: Arc<AtomicBool>,
    snapshot_preparing: Arc<AtomicBool>,
    live: Arc<Mutex<BTreeMap<&'static str, Value>>>,
    compact_controls: Arc<AtomicBool>,
    snapshot_pending: Arc<Mutex<Option<Arc<SnapshotDocument>>>>,
    wake: Arc<Mutex<Option<Arc<crate::readiness::Signal>>>>,
}
pub struct Inbox {
    pub priority: std::sync::mpsc::Receiver<Vec<u8>>,
    pub ordinary: std::sync::mpsc::Receiver<Vec<u8>>,
    bytes: Arc<AtomicUsize>,
    global: Arc<AtomicUsize>,
    pending: Arc<AtomicUsize>,
}
impl Outbox {
    pub fn pair(global: Arc<AtomicUsize>) -> (Self, Inbox) {
        let (p, pr) = std::sync::mpsc::sync_channel(8);
        let (o, or) = std::sync::mpsc::sync_channel(56);
        let bytes = Arc::new(AtomicUsize::new(0));
        let pending = Arc::new(AtomicUsize::new(0));
        (
            Self {
                priority: p,
                ordinary: o,
                bytes: bytes.clone(),
                global: global.clone(),
                pending: pending.clone(),
                closed: Arc::new(AtomicBool::new(false)),
                snapshot_active: Arc::new(AtomicBool::new(false)),
                snapshot_preparing: Arc::new(AtomicBool::new(false)),
                live: Arc::new(Mutex::new(BTreeMap::new())),
                compact_controls: Arc::new(AtomicBool::new(false)),
                snapshot_pending: Arc::new(Mutex::new(None)),
                wake: Arc::new(Mutex::new(None)),
            },
            Inbox {
                priority: pr,
                ordinary: or,
                bytes,
                global,
                pending,
            },
        )
    }
    pub(crate) fn wake_on_send(&self, signal: Arc<crate::readiness::Signal>) -> Result<(), Error> {
        *self
            .wake
            .lock()
            .map_err(|_| protocol("Connection wake is unavailable"))? = Some(signal);
        self.notify();
        Ok(())
    }
    fn notify(&self) {
        if let Ok(signal) = self.wake.lock() {
            if let Some(signal) = signal.as_ref() {
                signal.notify();
            }
        }
    }
    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.notify();
    }
    pub fn send(&self, value: &Value, priority: bool) -> Result<(), Error> {
        let kind = if value["kind"] == "event" {
            match value["event"]["type"].as_str() {
                Some("transport") => Some("transport"),
                Some("status") => Some("status"),
                Some("presence") => Some("presence"),
                Some("devices") => Some("devices"),
                _ => None,
            }
        } else {
            None
        };
        if let Some(kind) = kind {
            let bytes = serde_json::to_vec(value)?;
            if bytes.len() > LIVE_EVENT_BYTES {
                return Err(protocol("Live metadata exceeds its 64 KiB budget"));
            }
            let mut live = self
                .live
                .lock()
                .map_err(|_| protocol("Live metadata is unavailable"))?;
            live.insert(kind, value.clone());
            // Metadata always shares one FIFO with pages. A completed scheduler can
            // still have pages queued for the socket, so priority metadata could overtake
            // an older replay even after snapshot_active becomes false.
            self.send_bytes(bytes, false)
        } else {
            self.send_bytes(serde_json::to_vec(value)?, priority)
        }
    }
    /// Sends a high-rate reading (meters, play-position updates) only while
    /// this peer keeps up. A slow reader misses readings instead of filling its
    /// queue, which would close the connection.
    pub fn send_lossy(&self, value: &Value) -> Result<(), Error> {
        if self.pending.load(Ordering::Acquire) >= LOSSY_BACKLOG {
            return Ok(());
        }
        self.send(value, false)
    }
    /// Seed a newly attached peer from capture-time metadata. Later arrival wins;
    /// older worker input must never overwrite an event already observed by it.
    fn seed_live(&self, overlays: BTreeMap<&'static str, Value>) -> Result<(), Error> {
        for value in overlays.values() {
            validation::validate_event(&value["event"])?;
            if encoded_size(value)? > LIVE_EVENT_BYTES {
                return Err(protocol("Live metadata exceeds its 64 KiB budget"));
            }
        }
        let mut live = self
            .live
            .lock()
            .map_err(|_| protocol("Live metadata is unavailable"))?;
        for (kind, value) in overlays {
            live.entry(kind).or_insert(value);
        }
        Ok(())
    }
    fn replay_live(&self) -> Result<(), Error> {
        let live = self
            .live
            .lock()
            .map_err(|_| protocol("Live metadata is unavailable"))?;
        for value in live.values() {
            self.send_bytes(serde_json::to_vec(value)?, false)?;
        }
        Ok(())
    }
    pub fn send_bytes(&self, data: Vec<u8>, priority: bool) -> Result<(), Error> {
        if self.closed.load(Ordering::Acquire) || data.len() > validation::MAX_FRAME_BYTES {
            return Err(Error::Disconnected);
        }
        let count = data.len();
        let limit = if priority {
            QUEUE_BYTES
        } else {
            QUEUE_BYTES - 65536
        };
        if self
            .bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                v.checked_add(count).filter(|n| *n <= limit)
            })
            .is_err()
        {
            self.close();
            return Err(Error::QueueFull);
        }
        if self
            .global
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                v.checked_add(count).filter(|n| {
                    *n <= if priority {
                        GLOBAL_BYTES
                    } else {
                        GLOBAL_BYTES - GLOBAL_CONTROL_RESERVE
                    }
                })
            })
            .is_err()
        {
            self.bytes.fetch_sub(count, Ordering::AcqRel);
            self.close();
            return Err(Error::QueueFull);
        }
        if (if priority {
            &self.priority
        } else {
            &self.ordinary
        })
        .try_send(data)
        .is_err()
        {
            if std::env::var_os("SOUNDROOM_DEBUG_CONNECTIONS").is_some() {
                eprintln!(
                    "Soundroom outbox full: pending={} bytes={} priority={priority}",
                    self.pending.load(Ordering::Acquire),
                    self.bytes.load(Ordering::Acquire)
                );
            }
            self.bytes.fetch_sub(count, Ordering::AcqRel);
            self.global.fetch_sub(count, Ordering::AcqRel);
            self.close();
            return Err(Error::QueueFull);
        }
        self.pending.fetch_add(1, Ordering::AcqRel);
        self.notify();
        Ok(())
    }
}
impl Inbox {
    fn taken(&self) {
        let _ = self
            .pending
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| v.checked_sub(1));
    }
    pub(crate) fn delivery_lease(&self, count: usize) -> DeliveryLease {
        self.taken();
        DeliveryLease {
            bytes: self.bytes.clone(),
            global: self.global.clone(),
            count,
        }
    }
    pub fn received(&self, count: usize) {
        self.taken();
        self.bytes.fetch_sub(count, Ordering::AcqRel);
        self.global.fetch_sub(count, Ordering::AcqRel);
    }
}
pub(crate) struct DeliveryLease {
    bytes: Arc<AtomicUsize>,
    global: Arc<AtomicUsize>,
    count: usize,
}
impl Drop for DeliveryLease {
    fn drop(&mut self) {
        self.bytes.fetch_sub(self.count, Ordering::AcqRel);
        self.global.fetch_sub(self.count, Ordering::AcqRel);
    }
}
impl Drop for Inbox {
    fn drop(&mut self) {
        for bytes in self.priority.try_iter().chain(self.ordinary.try_iter()) {
            self.received(bytes.len());
        }
    }
}
#[derive(Clone)]
pub struct Identity {
    pub runtime: String,
    pub credential: String,
    pub name: String,
    pub role: RemoteRole,
}
struct Peer {
    identity: Identity,
    out: Outbox,
}
struct Upload {
    owner: String,
    name: String,
    path: PathBuf,
    total: u64,
    offset: u64,
    hash: Sha256,
    expected: String,
    touched: Instant,
}
impl Drop for Upload {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
#[derive(Clone)]
struct Artifact {
    owner: String,
    /// Owner-runtime-bound artifacts (Write drafts) are unreadable by any other runtime.
    runtime: Option<String>,
    path: PathBuf,
    bytes: u64,
    expires: Instant,
}
struct Receipt {
    key: (String, String, String),
    digest: [u8; 32],
    reply: Value,
    bytes: usize,
}
pub(crate) struct ValidatedRequest {
    request: RemoteRequest,
    digest: [u8; 32],
    credential: String,
    runtime: String,
    role: RemoteRole,
    transport_generation: Option<u64>,
}
impl ValidatedRequest {
    pub(crate) fn received_in_transport_generation(mut self, generation: u64) -> Self {
        self.transport_generation = Some(generation);
        self
    }
    pub(crate) fn authored(&self) -> bool {
        matches!(
            self.request.command,
            RemoteCommand::UpdateTrackControls { .. }
                | RemoteCommand::SetTrackName { .. }
                | RemoteCommand::SetTrackGain { .. }
                | RemoteCommand::SetTrackPan { .. }
                | RemoteCommand::SetTrackMute { .. }
                | RemoteCommand::SetTrackSolo { .. }
                | RemoteCommand::SetTrackArm { .. }
                | RemoteCommand::SetTrackInput { .. }
                | RemoteCommand::AddTrack { .. }
                | RemoteCommand::RemoveTrack { .. }
                | RemoteCommand::SplitClip { .. }
                | RemoteCommand::DeleteClips { .. }
                | RemoteCommand::MoveClip { .. }
                | RemoteCommand::ReplaceTracks { .. }
                | RemoteCommand::EditTracks { .. }
                | RemoteCommand::Undo
                | RemoteCommand::Redo
                | RemoteCommand::FinishUpload { .. }
                | RemoteCommand::OpenProject { .. }
                | RemoteCommand::SaveProject { .. }
        ) || matches!(&self.request.command, RemoteCommand::Editor { intent }
                if matches!(intent.operation.as_str(), "session.timing" | "session.rename" | "designer.commit" | "drums.commit" | "effects.apply" | "clips.combine" | "pitch.apply" | "pitch.manualApply" | "pitch.targets"
                    | "master.analyze" | "master.applyProposal"))
    }
}
struct Pending {
    authored: bool,
    critical: bool,
    transport_start: bool,
    /// Write lease traffic has its own bounded class with reserved End capacity.
    write: bool,
    save: bool,
    waveform: bool,
    digest: [u8; 32],
    cancel: Arc<AtomicBool>,
}
struct Job {
    peer: Identity,
    request: RemoteRequest,
    kind: JobKind,
    cancel: Arc<AtomicBool>,
}
enum JobKind {
    Waveform {
        task: waveform::Task,
    },
    Recover {
        task: Box<recovery::Task>,
    },
    Edit {
        task: Box<edits::Task>,
    },
    Transport {
        backend: crate::audio::Backend,
        command: crate::audio::Command,
        project: Box<Project>,
        revision: u64,
        /// Seek here before playing.
        start: Option<u64>,
        /// An editor request's context, echoed with the stopped cursor.
        context: Option<Value>,
    },
    Synthesize {
        task: Box<crate::synthesis::Task>,
    },
    Process {
        task: Box<process::Task>,
    },
    Pitch {
        task: Box<pitch::Task>,
    },
    Preview {
        task: Box<preview::Task>,
    },
    /// Renders a saved kit-designer kit into its sample library.
    KitExport {
        design: Box<soundroom_engine::custom_kits::KitDesign>,
    },
    Master {
        task: Box<master::Task>,
    },
    Write {
        task: Box<writes::Task>,
    },
    Record {
        plan: Box<record::Plan>,
        backend: crate::audio::Backend,
        project: Box<Project>,
        revision: u64,
        /// First recorded timeline frame (punch-in or playhead).
        target: u64,
        /// Transport start (target minus pre-roll).
        playback: u64,
        /// Count-in clicks played before the transport starts.
        count_in: Vec<f32>,
    },
    FinishRecording {
        active: record::Active,
        backend: crate::audio::Backend,
    },
    #[cfg(target_os = "macos")]
    Import {
        path: PathBuf,
        name: String,
        track_id: Option<String>,
        frame: u64,
    },
    Export {
        project: Box<Project>,
        path: PathBuf,
        name: String,
        start: u64,
        end: u64,
        tails: bool,
        stems: Option<Vec<String>>,
    },
    ReadArtifact {
        path: PathBuf,
        offset: u64,
        count: usize,
        total: u64,
        #[cfg(test)]
        hook: Option<Arc<dyn Fn() + Send + Sync>>,
    },
    Save {
        project: Box<Project>,
        path: PathBuf,
        expected: u64,
        replace_existing: bool,
    },
    #[cfg(target_os = "macos")]
    Package {
        project: Box<Project>,
    },
    Open {
        path: PathBuf,
        expected: u64,
    },
    #[cfg(target_os = "macos")]
    ImportProject {
        zip: PathBuf,
        name: String,
        expected: u64,
    },
}
#[path = "recovery.rs"]
pub mod recovery;

/// A request to the process's output owner thread.
pub struct OutputRequest {
    pub uid: Option<String>,
    pub reply: std::sync::mpsc::SyncSender<Result<(), String>>,
}
pub struct Service {
    pub registry: Registry,
    pub authority: Authority,
    media: BTreeMap<String, Media>,
    recovery: recovery::State,
    root: PathBuf,
    project_name: Option<String>,
    project_id: String,
    autosave: Arc<Mutex<autosave::State>>,
    peers: BTreeMap<String, Peer>,
    receipts: VecDeque<Receipt>,
    pending: BTreeMap<(String, String, String), Pending>,
    /// Scoped cancellation survives an urgent Cancel overtaking job admission.
    /// Unconsumed entries are bounded, and cannot expire ahead of a queued job.
    cancelled_requests: BTreeSet<(String, String, String)>,
    uploads: BTreeMap<String, Upload>,
    artifacts: BTreeMap<String, Artifact>,
    operator: Option<String>,
    /// The local GUI's own credential (`serve-local`); never revocable remotely.
    protected_credential: Option<String>,
    global: Arc<AtomicUsize>,
    started: Instant,
    backend: Option<crate::audio::Backend>,
    audio_status: Option<String>,
    writes: writes::Registry,
    /// Output meters were last sent as active (playing or recording).
    meters_active: bool,
    shutdown: Option<Arc<AtomicBool>>,
    shutdown_requested: bool,
    /// Advanced by admitted emergency stops, shared with every peer's ingress queue.
    transport_generation: Arc<AtomicU64>,
    recording: Option<record::Active>,
    recording_preparing: Option<Arc<AtomicBool>>,
    recording_finalizing: bool,
    previews: preview::Previews,
    solo_audition: Option<String>,
    audition_request: bool,
    /// The client's cursor, where the next editor Play starts from a stop.
    play_start: Option<u64>,
    /// The session the playing graph was built from: parameter-only edits
    /// against it are patched into that graph instead of rebuilding it.
    running: Mutex<Option<Arc<Session>>>,
    /// Self handle for helper threads (MIDI dispatch); set with the audio host.
    this: Option<std::sync::Weak<Mutex<Service>>>,
    input_monitoring: bool,
    effect_meters: BTreeMap<String, Vec<String>>,
    output_uid: Option<String>,
    output_switch: Option<std::sync::mpsc::SyncSender<OutputRequest>>,
    input_uid: Option<String>,
    simulated_input: Option<record::SimulatedInput>,
    #[cfg(test)]
    pub(crate) edit_hook: Option<Arc<dyn Fn() + Send + Sync>>,
    #[cfg(test)]
    artifact_read_hook: Option<Arc<dyn Fn() + Send + Sync>>,
    #[cfg(test)]
    waveform_hook: Option<Arc<dyn Fn() + Send + Sync>>,
    publication: PublicationWorker,
    witness: Arc<Mutex<Option<SessionWitness>>>,
}
pub type Shared = Arc<Mutex<Service>>;
fn protocol(s: &str) -> Error {
    Error::Protocol(s.into())
}
fn engine(e: soundroom_engine::Error) -> Error {
    match e {
        soundroom_engine::Error::Conflict(s) => Error::Conflict(s),
        _ => protocol(&e.to_string()),
    }
}
fn role(role: &RemoteRole) -> Role {
    match role {
        RemoteRole::Viewer => Role::Viewer,
        RemoteRole::Editor => Role::Editor,
        RemoteRole::Operator => Role::Operator,
    }
}
fn key(peer: &Identity, request: &RemoteRequest) -> (String, String, String) {
    (
        peer.credential.clone(),
        peer.runtime.clone(),
        request.id.to_ascii_uppercase(),
    )
}
fn response(id: &str, status: &str, revision: u64, message: &str, payload: Option<Value>) -> Value {
    let mut v = json!({"version":3,"kind":"reply","requestID":id,"status":status,"revision":revision,"message":message});
    if let Some(p) = payload {
        v["payload"] = Value::String(
            base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&p).unwrap()),
        );
    }
    v
}
/// Default clip edge fade: 2 ms at 48 kHz. Short enough to keep joins tight,
/// long enough to hide the step (click) where a clip starts or stops mid-wave.
pub(crate) const EDGE_FADE_FRAMES: u64 = 96;
/// Clips this host once created without edge protection start and stop with
/// a hard step that clicks at every join. Give them the default edge fade,
/// and cut-boundary fades for fragments copied from them. Clips with an
/// explicit edge setting (including 0) or protection turned off are kept.
pub(crate) fn protect_unset_clip_edges(session: &mut Session) {
    for clip in session
        .tracks
        .iter_mut()
        .chain(std::iter::once(&mut session.master_track))
        .flat_map(|t| t.clips.iter_mut())
    {
        if clip.edge_fade_frames.is_some() || clip.edge_protection_disabled == Some(true) {
            continue;
        }
        clip.edge_fade_frames = Some(EDGE_FADE_FRAMES);
        clip.cut_fade_in_frames.get_or_insert(EDGE_FADE_FRAMES);
        clip.cut_fade_out_frames.get_or_insert(EDGE_FADE_FRAMES);
    }
}
pub fn normalize_seeds(value: &mut Value, wire: bool) {
    match value {
        Value::Array(a) => {
            for v in a {
                normalize_seeds(v, wire)
            }
        }
        Value::Object(o) => {
            for (k, v) in o {
                if k == "seed" || k == "organicSeed" {
                    if wire {
                        if let Some(n) = v.as_u64() {
                            *v = Value::String(n.to_string());
                        }
                    } else if let Some(n) = v.as_str().and_then(|s| s.parse::<u64>().ok()) {
                        *v = Value::Number(n.into());
                    }
                } else {
                    normalize_seeds(v, wire)
                }
            }
        }
        _ => {}
    }
}
fn canonical_ids(value: &mut Value) {
    match value {
        Value::Array(a) => {
            for v in a {
                canonical_ids(v)
            }
        }
        Value::Object(o) => {
            for (k, v) in o {
                // Core Audio UIDs are opaque, case-sensitive device identities.
                if k == "id" || (k.ends_with("ID") && !k.ends_with("UID")) {
                    if let Some(s) = v.as_str().filter(|s| validation::uuid(s)) {
                        *v = Value::String(s.to_ascii_uppercase());
                    }
                } else {
                    canonical_ids(v)
                }
            }
        }
        _ => {}
    }
}
fn normalize_loaded_session(session: Session) -> Result<Session, Error> {
    let mut value = serde_json::to_value(session)?;
    canonical_ids(&mut value);
    let mut session = serde_json::from_value(value)?;
    protect_unset_clip_edges(&mut session);
    Ok(session)
}
fn wire<T: serde::Serialize>(value: &T) -> Result<Value, Error> {
    let mut v = serde_json::to_value(value)?;
    normalize_seeds(&mut v, true);
    Ok(v)
}
impl Service {
    pub fn open(root: &Path, project: Option<&Path>) -> Result<Shared, Error> {
        let registry = Registry::load(root)?;
        for d in ["Media", "Uploads", "Artifacts", "Projects"] {
            private_directory(&root.join(d))?;
        }
        let (session, media) = match project {
            Some(path) => {
                let p = Project::open(path).map_err(engine)?;
                (p.session, p.media)
            }
            None => (Session::default(), BTreeMap::new()),
        };
        let authority = Authority::new(normalize_loaded_session(session)?).map_err(engine)?;
        let media = custody::Staged::load(root, media)?.commit(root)?;
        let global = Arc::new(AtomicUsize::new(0));
        let witness = Arc::new(Mutex::new(None));
        let publication = PublicationWorker::new(global.clone(), witness.clone());
        Ok(Arc::new(Mutex::new(Self {
            registry,
            authority,
            media,
            recovery: Default::default(),
            root: root.into(),
            project_name: project
                .and_then(|path| path.strip_prefix(root.join("Projects")).ok())
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned()),
            project_id: new_id()?,
            autosave: Arc::new(Mutex::new(autosave::State::default())),
            peers: BTreeMap::new(),
            receipts: VecDeque::new(),
            pending: BTreeMap::new(),
            cancelled_requests: BTreeSet::new(),
            uploads: BTreeMap::new(),
            artifacts: BTreeMap::new(),
            operator: None,
            protected_credential: None,
            global,
            publication,
            witness,
            started: Instant::now(),
            backend: None,
            audio_status: None,
            writes: Default::default(),
            meters_active: false,
            shutdown: None,
            shutdown_requested: false,
            transport_generation: Arc::new(AtomicU64::new(0)),
            recording: None,
            recording_preparing: None,
            recording_finalizing: false,
            previews: Default::default(),
            solo_audition: None,
            audition_request: false,
            play_start: None,
            running: Mutex::new(None),
            this: None,
            input_monitoring: false,
            effect_meters: BTreeMap::new(),
            output_uid: None,
            output_switch: None,
            input_uid: None,
            simulated_input: None,
            #[cfg(test)]
            edit_hook: None,
            #[cfg(test)]
            artifact_read_hook: None,
            #[cfg(test)]
            waveform_hook: None,
        })))
    }
    /// Attaches only a callback-owned control endpoint. Caller owns/opened hardware lifecycle.
    pub fn attach_audio(
        shared: &Shared,
        control: soundroom_engine::realtime::GraphControl,
    ) -> Result<(), Error> {
        let backend = crate::audio::Backend::attach(shared, control);
        let mut state = shared
            .lock()
            .map_err(|_| protocol("Authority is unavailable"))?;
        if state.backend.is_some() {
            return Err(protocol("An audio backend is already attached"));
        }
        state.this = Some(Arc::downgrade(shared));
        backend.observe_revision(state.authority.revision);
        state.backend = Some(backend);
        state.sync_metronome_grid();
        Ok(())
    }
    /// Control-plane diagnostics. Applied means an actual callback adopted that graph.
    pub fn graph_revisions(&self) -> Option<crate::audio::GraphRevisions> {
        self.backend
            .as_ref()
            .map(|backend| backend.graph_revisions())
    }
    pub(crate) fn status_event(&self, message: &str) {
        let message = message.chars().take(512).collect::<String>();
        self.broadcast(
            &json!({"version":3,"kind":"event","event":{"type":"status","message":message}}),
            false,
        );
    }
    pub(crate) fn audio_update_status(&mut self, revision: u64, message: String) {
        if self.authority.revision != revision {
            return;
        }
        let message = message.chars().take(512).collect::<String>();
        self.audio_status = Some(message.clone());
        self.broadcast(
            &json!({"version":3,"kind":"event","event":{"type":"status","message":message}}),
            false,
        );
    }
    pub(crate) fn audio_telemetry(&mut self, telemetry: soundroom_engine::realtime::Telemetry) {
        let transport = if telemetry.playing && self.recording.is_some() {
            soundroom_engine::authority::Transport::Recording {
                frame: telemetry.frame,
            }
        } else if telemetry.playing {
            soundroom_engine::authority::Transport::Playing {
                frame: telemetry.frame,
            }
        } else {
            soundroom_engine::authority::Transport::Stopped {
                frame: telemetry.frame,
            }
        };
        let changed = self.authority.transport != transport;
        // Only play/stop transitions must arrive; position updates may be skipped.
        let moved_only = matches!(
            self.authority.transport,
            soundroom_engine::authority::Transport::Stopped { .. }
        ) == !telemetry.playing;
        if self
            .authority
            .backend_committed_transport(transport)
            .is_err()
        {
            return;
        }
        if changed {
            let transport = self.transport_value(telemetry.frame, telemetry.playing, false);
            let event = json!({"version":3,"kind":"event","event":{"type":"transport","transport":transport}});
            for p in self.peers.values() {
                let _ = if moved_only {
                    p.out.send_lossy(&event)
                } else {
                    p.out.send(&event, false)
                };
            }
        }
    }
    fn transport_value(&self, frame: u64, playing: bool, recording: bool) -> Value {
        let recording = recording || self.recording.is_some();
        let mut value = json!({"frame":frame,"isPlaying":playing,"isRecording":recording,
            "isPreparing":self.recording_finalizing || self.recording_preparing.is_some(),"hostTime":self.started.elapsed().as_secs_f64(),"operatorID":self.operator});
        if let Some(range) = self.backend.as_ref().and_then(|b| b.loop_range()) {
            value["loopStart"] = json!(range.start);
            value["loopEnd"] = json!(range.end);
        }
        if let Some(active) = &self.recording {
            value["recordingStartFrame"] = json!(active.start_frame);
        }
        value
    }
    fn begin_recording(
        &mut self,
        peer: &Identity,
        intent: Option<&soundroom_client::models::RemoteEditorIntent>,
    ) -> Result<Handled, Error> {
        self.require_transport_owner(peer, false)?;
        self.require_editable()?;
        if !self.admits_job(peer, false) {
            return Err(Error::QueueFull);
        }
        if self.pending.values().any(|pending| pending.authored) {
            return Err(Error::Conflict(
                "Finish pending edits before recording".into(),
            ));
        }
        let number = |key: &str| {
            intent
                .and_then(|i| i.numbers.get(key).copied())
                .filter(|v| v.is_finite())
        };
        let flag = |key: &str| {
            intent
                .and_then(|i| i.flags.get(key).copied())
                .unwrap_or(false)
        };
        let offset_ms = number("offsetMS").unwrap_or(0.);
        let timing = self.authority.session.timing.clone();
        let tempo = timing.tempo.clamp(20., 300.);
        let cursor = match intent {
            Some(i) => i.context.cursor_frame.max(0) as u64,
            None => match self.authority.transport {
                soundroom_engine::authority::Transport::Stopped { frame }
                | soundroom_engine::authority::Transport::Playing { frame }
                | soundroom_engine::authority::Transport::Recording { frame } => frame,
            },
        };
        let (target, end) = if flag("punch") {
            match intent.and_then(|i| i.context.range_start.zip(i.context.range_end)) {
                Some((a, b)) if a >= 0 && b > a => (a as u64, Some(b as u64)),
                _ => return Err(protocol("Select the punch-in/out range first.")),
            }
        } else {
            (cursor, None)
        };
        let pre_roll = number("preRollSeconds").unwrap_or(0.).clamp(0., 30.);
        let playback = target.saturating_sub((pre_roll * 48000.) as u64);
        let bars = intent
            .and_then(|i| i.integers.get("countInBars").copied())
            .unwrap_or(0)
            .clamp(0, 4) as usize;
        let grid = Grid::new(tempo, timing.beats_per_bar, timing.beat_unit);
        // The count-in continues the project's bar grid into the transport
        // click, including from a start between beats.
        let count_in = grid.count_in(
            (bars as f64 * grid.beats_per_bar as f64 * grid.beat_frames).round() as usize,
            playback,
        );
        let metronome = flag("metronome");
        if peer.role != RemoteRole::Operator {
            return Err(protocol("Operator role is required"));
        }
        if self.recording_finalizing {
            return Err(Error::Conflict(
                "The previous take is still finishing".into(),
            ));
        }
        if self.recording.is_some() {
            return Err(Error::Conflict("Already recording".into()));
        }
        let backend = self.backend.clone().ok_or(Error::QueueFull)?;
        if backend.loop_range().is_some() {
            return Err(Error::Conflict("Turn off the loop before recording".into()));
        }
        let device = self.recording_device()?;
        let mut routes = vec![];
        let mut monitor_routes = vec![];
        for track in self.authority.session.tracks.iter().filter(|t| t.armed) {
            let channel = track
                .extra
                .get("inputChannel")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let stereo = track
                .extra
                .get("stereoInput")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let channels = if stereo {
                vec![channel, channel + 1]
            } else {
                vec![channel]
            };
            if channels.iter().any(|&c| c >= device.channels()) {
                return Err(Error::Conflict(format!(
                    "{} uses an input channel this device does not have",
                    track.name
                )));
            }
            monitor_routes.push(record::MonitorRoute {
                channels: channels.clone(),
                gain: if track.muted { 0. } else { track.gain },
                pan: track.pan,
            });
            routes.push(record::Route {
                track_id: track.id.clone(),
                channels,
            });
        }
        if routes.is_empty() {
            return Err(Error::Conflict("Arm a track to record".into()));
        }
        let offset = if offset_ms.is_finite() {
            offset_ms.clamp(-2000., 2000.)
        } else {
            0.
        };
        let project = Project {
            session: self.authority.session.clone(),
            media: self.media.clone(),
        };
        if intent.is_some() {
            self.input_monitoring = flag("inputMonitoring");
        }
        self.set_running(&project.session);
        self.operator = Some(peer.runtime.clone());
        Ok(Handled::Job(JobKind::Record {
            plan: Box::new(record::Plan {
                device,
                routes,
                offset_frames: (offset * 48.).round() as i64,
                directory: self.root.join("Recordings"),
                end_frame: end,
                metronome,
                monitor: backend
                    .preview()
                    .monitor_writer()
                    .map(|writer| record::Monitor {
                        writer,
                        enabled: Arc::new(AtomicBool::new(self.input_monitoring)),
                        routes: monitor_routes,
                    }),
            }),
            backend,
            project: Box::new(project),
            revision: self.authority.revision,
            target,
            playback,
            count_in,
        }))
    }
    fn broadcast_transport(&self) {
        let (frame, playing) = match self.authority.transport {
            soundroom_engine::authority::Transport::Playing { frame } => (frame, true),
            soundroom_engine::authority::Transport::Stopped { frame } => (frame, false),
            soundroom_engine::authority::Transport::Recording { frame, .. } => (frame, true),
        };
        let transport = self.transport_value(frame, playing, false);
        self.broadcast(
            &json!({"version":3,"kind":"event","event":{"type":"transport","transport":transport}}),
            false,
        );
    }
    /// Output peak meters for every peer while transport runs or anything is
    /// audible (previews, live notes), coalesced at the telemetry rate; one
    /// zero reading follows.
    pub(crate) fn audio_meters(&mut self, left: f32, right: f32) {
        self.effect_meter_events();
        let active = self.recording.is_some()
            || left.max(right) > 1e-6
            || !matches!(
                self.authority.transport,
                soundroom_engine::authority::Transport::Stopped { .. }
            );
        if !std::mem::replace(&mut self.meters_active, active) && !active {
            return;
        }
        let (left, right) = if active { (left, right) } else { (0., 0.) };
        let event = json!({"version":3,"kind":"event","event":{"type":"meters","left":left,"right":right,"input":0.0}});
        for peer in self.peers.values() {
            let _ = if active {
                peer.out.send_lossy(&event)
            } else {
                peer.out.send(&event, false)
            };
        }
    }
    /// Drains live insert meters for every subscribed effect and sends each
    /// subscriber its own readings.
    fn effect_meter_events(&mut self) {
        self.effect_meters
            .retain(|runtime, _| self.peers.contains_key(runtime));
        let ids: std::collections::BTreeSet<&String> =
            self.effect_meters.values().flatten().collect();
        let mut readings = BTreeMap::new();
        for id in ids {
            if let Some(m) = soundroom_engine::meters::take(id) {
                let e = m.extras;
                let mut reading = json!({
                    "inputPeak": m.input_peak, "outputPeak": m.output_peak,
                    "inputRMS": m.input_rms, "outputRMS": m.output_rms,
                    "gainReductionDB": m.gain_reduction_db,
                    "truePeak": e.true_peak.max(m.output_peak),
                    "momentaryLUFS": e.momentary, "shortTermLUFS": e.short_term,
                    "resonance": (0..12).map(|i| (format!("b{i}"), json!(e.bands[i]))).collect::<serde_json::Map<_, _>>(),
                    "frameCount": m.frames, "revision": m.revision,
                });
                if let Some(p) = e.pitch {
                    reading["pitch"] = json!({
                        "inputFrequency": p.input_frequency, "targetFrequency": p.target_frequency,
                        "correctionCents": p.correction_cents, "confidence": p.confidence,
                        "wetBlend": p.wet_blend, "referenceFrequency": p.reference_frequency,
                        "sourceFrame": p.source_frame,
                    });
                }
                readings.insert(id.clone(), reading);
            }
        }
        if readings.is_empty() {
            return;
        }
        for (runtime, wanted) in &self.effect_meters {
            let mine: serde_json::Map<String, Value> = wanted
                .iter()
                .filter_map(|id| readings.get(id).map(|v| (id.clone(), v.clone())))
                .collect();
            if mine.is_empty() {
                continue;
            }
            if let Some(peer) = self.peers.get(runtime) {
                let _ = peer.out.send_lossy(
                    &json!({"version":3,"kind":"event","event":{"type":"effectMeters","metersByEffect":mine}}),
                );
            }
        }
    }
    /// The process owner's stop signal; `server.shutdown` saves and sets it.
    pub fn set_shutdown_signal(&mut self, stop: Arc<AtomicBool>) {
        self.shutdown = Some(stop);
    }
    /// Lets `setDevices` move output to another device (the owner thread
    /// closes the unit, reopens on the new device and restarts it).
    pub fn set_output_switch(&mut self, switch: std::sync::mpsc::SyncSender<OutputRequest>) {
        self.output_switch = Some(switch);
    }
    fn device_event(&self) -> Value {
        #[cfg(target_os = "macos")]
        let (devices, output, input) = {
            let list = soundroom_engine::core_audio::core_audio_devices().unwrap_or_default();
            let output = list
                .iter()
                .find(|d| match &self.output_uid {
                    Some(uid) => &d.uid == uid,
                    None => d.is_default_output,
                })
                .map_or(0, |d| d.id());
            let input = list
                .iter()
                .find(|d| match &self.input_uid {
                    Some(uid) => &d.uid == uid,
                    None => d.is_default_input,
                })
                .map_or(0, |d| d.id());
            let devices: Vec<Value> = list
                .iter()
                .map(|d| json!({"id":d.id(),"uid":d.uid,"name":d.name,"inputChannels":d.input_channels,"outputChannels":d.output_channels}))
                .collect();
            (devices, output, input)
        };
        #[cfg(not(target_os = "macos"))]
        let (devices, output, input): (Vec<Value>, u32, u32) = (vec![], 0, 0);
        json!({"version":3,"kind":"event","event":{"type":"devices","devices":devices,"inputID":input,"outputID":output}})
    }
    pub fn endpoint_capabilities(&self) -> Value {
        let mut capabilities = Self::capabilities();
        if self.backend.is_some() {
            capabilities["deviceBackends"] = json!(["callback-owned-output"]);
            capabilities["commands"].as_array_mut().unwrap().extend([
                json!("play"),
                json!("stop"),
                json!("seek"),
                json!("setLoop"),
                json!("record"),
                json!("setDevices"),
                json!("setTrackInput"),
            ]);
            capabilities["editorOperations"]
                .as_array_mut()
                .unwrap()
                .extend([
                    json!("transport.play"),
                    json!("transport.stop"),
                    json!("transport.pause"),
                    json!("transport.toggle"),
                ]);
            capabilities["features"]
                .as_array_mut()
                .unwrap()
                .push(json!("callback-acknowledged-transport"));
        }
        capabilities["editorOperations"]
            .as_array_mut()
            .unwrap()
            .extend(
                [
                    "transport.record",
                    "transport.finishRecording",
                    "recording.settings",
                    "transport.metronome",
                    "devices.inputVolume",
                    "server.shutdown",
                    "project.new",
                    "devices.refresh",
                    "meters.subscribe",
                    "designer.midiSources",
                ]
                .map(|op| json!(op)),
            );
        if self.automation_write_available() {
            capabilities["editorOperations"]
                .as_array_mut()
                .unwrap()
                .extend(writes::OPERATIONS.iter().map(|op| json!(op)));
            capabilities["features"]
                .as_array_mut()
                .unwrap()
                .push(json!("automation-write"));
        }
        capabilities["editorOperations"]
            .as_array_mut()
            .unwrap()
            .push(json!("sources.status"));
        capabilities["features"]
            .as_array_mut()
            .unwrap()
            .push(json!("model-source-separation"));
        capabilities
    }
    /// Marks the local GUI's credential as not remotely revocable.
    pub fn protect_credential(&mut self, id: &str) {
        self.protected_credential = Some(id.to_owned());
    }
    fn transport_job(
        &mut self,
        peer: &Identity,
        command: crate::audio::Command,
    ) -> Result<Handled, Error> {
        let stopping = matches!(command, crate::audio::Command::Pause);
        self.require_transport_owner(peer, stopping)?;
        if !stopping {
            self.require_editable()?;
        }
        // Stopping while recording finishes the take(s) first.
        if matches!(command, crate::audio::Command::Pause) && self.recording.is_some() {
            let backend = self.backend.clone().ok_or(Error::QueueFull)?;
            self.operator = Some(peer.runtime.clone());
            let active = self.recording.take().unwrap();
            self.recording_finalizing = true;
            return Ok(Handled::Job(JobKind::FinishRecording { active, backend }));
        }
        if matches!(command, crate::audio::Command::Pause) {
            self.operator = Some(peer.runtime.clone());
        } else if self.operator.as_ref().is_some_and(|id| id != &peer.runtime)
            && (self
                .backend
                .as_ref()
                .is_some_and(|backend| backend.active())
                || self.recording.is_some()
                || self.recording_finalizing)
        {
            return Err(Error::Conflict(
                "Another operator controls transport; take transport explicitly".into(),
            ));
        } else {
            self.operator = Some(peer.runtime.clone());
        }
        let backend = self.backend.clone().ok_or(Error::QueueFull)?;
        self.stop_previews();
        // A single-track audition lasts until the transport stops or plays normally.
        match command {
            crate::audio::Command::Pause => self.solo_audition = None,
            crate::audio::Command::Play if !std::mem::take(&mut self.audition_request) => {
                self.solo_audition = None
            }
            _ => {}
        }
        let session = self.audible(&self.authority.session);
        if !matches!(command, crate::audio::Command::Pause) {
            self.set_running(&session);
        }
        let start = match command {
            crate::audio::Command::Play => self.play_start.take(),
            _ => None,
        };
        Ok(Handled::Job(JobKind::Transport {
            start,
            context: None,
            backend,
            command,
            project: Box::new(if matches!(command, crate::audio::Command::Pause) {
                Project {
                    session: Default::default(),
                    media: Default::default(),
                }
            } else {
                Project {
                    session,
                    media: self.media.clone(),
                }
            }),
            revision: self.authority.revision,
        }))
    }
    /// Editor Play from a stop starts at the client's cursor, which the GUI
    /// moves locally without sending a seek.
    fn play_from_cursor(
        &mut self,
        peer: &Identity,
        intent: &soundroom_client::models::RemoteEditorIntent,
    ) -> Result<Handled, Error> {
        let cursor = intent.context.cursor_frame.max(0) as u64;
        if let soundroom_engine::authority::Transport::Stopped { frame } = self.authority.transport
        {
            self.play_start = (frame != cursor).then_some(cursor);
        }
        let result = self.transport_job(peer, crate::audio::Command::Play);
        self.play_start = None;
        result
    }
    fn require_paused(&self) -> Result<(), Error> {
        self.require_editable()?;
        if self.backend.as_ref().is_some_and(|b| b.active()) {
            return Err(Error::QueueFull);
        }
        Ok(())
    }
    fn require_editable(&self) -> Result<(), Error> {
        if self.recording_preparing.is_some()
            || self.recording.is_some()
            || self.recording_finalizing
        {
            return Err(Error::Conflict(
                "Finish recording before changing the session".into(),
            ));
        }
        Ok(())
    }
    fn require_transport_owner(&self, peer: &Identity, takeover: bool) -> Result<(), Error> {
        if peer.role != RemoteRole::Operator {
            return Err(protocol("Operator role is required"));
        }
        if !takeover
            && self
                .operator
                .as_ref()
                .is_some_and(|owner| owner != &peer.runtime)
            && (self
                .backend
                .as_ref()
                .is_some_and(|backend| backend.active())
                || self.recording_preparing.is_some()
                || self.recording.is_some()
                || self.recording_finalizing)
        {
            return Err(Error::Conflict(
                "Another operator controls transport; take transport explicitly".into(),
            ));
        }
        Ok(())
    }
    pub(crate) fn close_connections(&self) {
        for peer in self.peers.values() {
            peer.out.close();
        }
    }
    pub fn global(&self) -> Arc<AtomicUsize> {
        self.global.clone()
    }
    pub(crate) fn transport_generation(&self) -> Arc<AtomicU64> {
        self.transport_generation.clone()
    }
    fn invalidate_transport_starts(&mut self) -> Result<(), Error> {
        self.transport_generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |generation| {
                generation.checked_add(1)
            })
            .map_err(|_| protocol("Transport generation exhausted"))?;
        for pending in self
            .pending
            .values()
            .filter(|pending| pending.transport_start)
        {
            pending.cancel.store(true, Ordering::Release);
        }
        if let Some(cancel) = &self.recording_preparing {
            cancel.store(true, Ordering::Release);
        }
        Ok(())
    }
    fn cancel_waveforms(&self, identity: Option<&Identity>) {
        for (key, pending) in &self.pending {
            if pending.waveform
                && identity.map_or(true, |peer| {
                    key.0 == peer.credential && key.1 == peer.runtime
                })
            {
                pending.cancel.store(true, Ordering::Release);
            }
        }
    }
    pub fn join(&mut self, identity: Identity, out: Outbox) -> Result<(), Error> {
        if self.peers.len() >= 16 && !self.peers.contains_key(&identity.runtime) {
            return Err(Error::QueueFull);
        }
        if let Some(old) = self.peers.get(&identity.runtime) {
            if old.identity.credential != identity.credential {
                return Err(protocol("Runtime identity belongs to another credential"));
            }
            old.out.close();
            self.cancel_waveforms(Some(&identity));
        }
        // A new peer learns the device inventory without asking for it.
        let devices = self.device_event();
        let _ = out.send(&devices, false);
        self.peers
            .insert(identity.runtime.clone(), Peer { identity, out });
        self.presence();
        self.history_events();
        Ok(())
    }
    pub fn leave(&mut self, identity: &Identity, out: &Outbox) {
        if self
            .peers
            .get(&identity.runtime)
            .is_some_and(|p| Arc::ptr_eq(&p.out.closed, &out.closed))
        {
            self.peers.remove(&identity.runtime);
            self.cancel_waveforms(Some(identity));
            self.release_write_owner(
                |owner| {
                    owner.runtime == identity.runtime && owner.credential == identity.credential
                },
                soundroom_engine::automation_write::ReleaseReason::Disconnected,
            );
            if self.operator.as_ref() == Some(&identity.runtime) {
                self.operator = None;
            }
            self.presence();
        }
    }
    fn history_events(&self) {
        for peer in self.peers.values() {
            let (undo, redo) = if peer.identity.role == RemoteRole::Viewer {
                (None, None)
            } else {
                self.authority.history_state(&peer.identity.runtime)
            };
            let _=peer.out.send(&json!({"version":3,"kind":"event","event":{"type":"history","canUndo":undo.is_some(),"canRedo":redo.is_some(),"undoName":undo,"redoName":redo}}),false);
        }
    }
    fn presence(&self) {
        self.broadcast(&json!({"version":3,"kind":"event","event":{"type":"presence","peers":self.peer_list()}}),true);
    }
    fn peer_list(&self) -> Vec<RemotePeer> {
        self.peers
            .values()
            .map(|p| RemotePeer {
                id: p.identity.runtime.clone(),
                name: p.identity.name.clone(),
                role: p.identity.role.clone(),
            })
            .collect()
    }
    fn publish_changes(&self, before: Session, old_revision: u64) -> Result<(), Error> {
        if let Some(backend) = &self.backend {
            backend.observe_revision(self.authority.revision);
        }
        if old_revision == self.authority.revision {
            return Ok(());
        }
        self.sync_metronome_grid();
        let input = self.capture_snapshot();
        self.schedule_audio_update(&input);
        self.publication.enqueue(Publication {
            input,
            before: Some(before),
            prepared: None,
            compact: None,
            targets: self
                .peers
                .values()
                .map(|peer| {
                    (
                        format!("{:p}", Arc::as_ptr(&peer.out.closed)),
                        peer.out.clone(),
                    )
                })
                .collect(),
        })
    }
    /// Capture immutable control state only; the bounded shared worker encodes it.
    pub fn queue_snapshot(&self, out: &Outbox) -> Result<(), Error> {
        self.publication.enqueue(Publication {
            input: self.capture_snapshot(),
            before: None,
            prepared: None,
            compact: None,
            targets: BTreeMap::from([(format!("{:p}", Arc::as_ptr(&out.closed)), out.clone())]),
        })
    }
    fn capture_snapshot(&self) -> SnapshotInput {
        let witness = self.witness.lock().ok().and_then(|witness| {
            witness
                .as_ref()
                .filter(|witness| witness.revision == self.authority.revision)
                .cloned()
        });
        let session = match witness {
            Some(witness) => SessionInput::Shared(witness),
            None => SessionInput::Owned(Arc::new(self.authority.session.clone())),
        };
        self.capture_source(
            session,
            self.authority.revision,
            &self.authority.track_revisions,
            &self.media,
        )
    }
    /// The session as heard: during a single-track audition only that track.
    fn audible(&self, session: &Session) -> Session {
        let mut session = session.clone();
        if let Some(solo) = &self.solo_audition {
            for track in &mut session.tracks {
                track.muted = !track.id.eq_ignore_ascii_case(solo);
                track.solo = false;
            }
        }
        session
    }
    fn schedule_audio_update(&self, input: &SnapshotInput) {
        let Some(backend) = &self.backend else { return };
        let session = if self.solo_audition.is_some() {
            SessionInput::Owned(Arc::new(self.audible(input.session.base())))
        } else {
            input.session.clone()
        };
        let source = GraphSource {
            session,
            media: input.media.clone(),
        };
        let next = Arc::new(source.materialize().session);
        let Ok(mut running) = self.running.lock() else {
            return;
        };
        // Parameter, automation, pitch-target and gain/pan edits continue the
        // playing graph (as the in-process engine did) instead of replacing it,
        // which restarted inserts' state on every slider movement.
        let patch = || {
            running
                .as_deref()
                .and_then(|old| soundroom_engine::render::LivePatch::between(old, &next))
        };
        if let (Some(live), Some(canonical)) = (patch(), patch()) {
            if backend.publish_live(
                live,
                canonical,
                self.authority.revision,
                crate::audio::audible_variant(&next),
            ) {
                *running = Some(next);
                return;
            }
        }
        *running = Some(next);
        drop(running);
        backend.schedule_update(source, self.authority.revision);
    }
    /// Prepares a full graph for the current session, without a live patch.
    pub(crate) fn rebuild_audio(&mut self) {
        if let Ok(mut running) = self.running.lock() {
            *running = None;
        }
        self.sync_metronome_grid();
        let input = self.capture_snapshot();
        self.schedule_audio_update(&input);
    }
    /// Keeps the transport click on the project's tempo and meter.
    fn sync_metronome_grid(&self) {
        if let Some(backend) = &self.backend {
            let timing = &self.authority.session.timing;
            backend.set_metronome_grid(Grid::new(
                timing.tempo,
                timing.beats_per_bar,
                timing.beat_unit,
            ));
        }
    }
    /// Records the session a newly started graph plays.
    fn set_running(&self, session: &Session) {
        if let Ok(mut running) = self.running.lock() {
            *running = Some(Arc::new(session.clone()));
        }
    }
    fn capture_snapshot_for(
        &self,
        session: &Session,
        revision: u64,
        versions: &BTreeMap<String, u64>,
        media: &BTreeMap<String, Media>,
    ) -> SnapshotInput {
        self.capture_source(
            SessionInput::Owned(Arc::new(session.clone())),
            revision,
            versions,
            media,
        )
    }
    fn capture_source(
        &self,
        session: SessionInput,
        revision: u64,
        versions: &BTreeMap<String, u64>,
        media: &BTreeMap<String, Media>,
    ) -> SnapshotInput {
        let frame = match self.authority.transport {
            soundroom_engine::authority::Transport::Stopped { frame }
            | soundroom_engine::authority::Transport::Playing { frame }
            | soundroom_engine::authority::Transport::Recording { frame } => frame,
        };
        SnapshotInput {
            session,
            revision,
            versions: versions.clone(),
            media: media.clone(),
            server_id: self.registry.server_id.clone(),
            server_name: self.registry.server_name.clone(),
            project_name: self.project_name.clone(),
            project_id: self.project_id.clone(),
            transport: self.transport_value(
                frame,
                matches!(
                    self.authority.transport,
                    soundroom_engine::authority::Transport::Playing { .. }
                ),
                matches!(
                    self.authority.transport,
                    soundroom_engine::authority::Transport::Recording { .. }
                ),
            ),
            peers: self.peer_list(),
            status: self.audio_status.clone().unwrap_or_else(|| {
                // Shown in the app's status bar: plain words, not internals.
                if self.backend.is_some() {
                    "Ready"
                } else {
                    "Audio output isn't connected"
                }
                .into()
            }),
        }
    }
    fn broadcast(&self, value: &Value, priority: bool) {
        for p in self.peers.values() {
            let _ = p.out.send(value, priority);
        }
    }
    pub fn snapshot(&self) -> Result<Value, Error> {
        self.snapshot_for(
            &self.authority.session,
            self.authority.revision,
            &self.authority.track_revisions,
            &self.media,
        )
    }
    fn snapshot_for(
        &self,
        session: &Session,
        revision: u64,
        versions: &BTreeMap<String, u64>,
        media: &BTreeMap<String, Media>,
    ) -> Result<Value, Error> {
        self.capture_snapshot_for(session, revision, versions, media)
            .value()
    }
    pub fn capabilities() -> Value {
        let mut value = json!({"protocolVersions":[3],"projectVersions":[1,2,3,4,5,6,7,8,9,10,11],"commands":["snapshot","takeTransport","updateTrackControls","setTrackName","setTrackGain","setTrackPan","setTrackMute","setTrackSolo","setTrackArm","addTrack","removeTrack","splitClip","deleteClips","moveClip","replaceTracks","editTracks","editor","undo","redo","listProjects","openProject","saveProject","exportMix","beginUpload","uploadChunk","finishUpload","downloadArtifact","readArtifact","waveform","ping"],"editorOperations":["client.features","session.timing","session.rename","project.package","server.webPairing","server.webClients","server.revokeWebCredential","project.export","jobs.cancel","designer.commit","drums.commit","drums.export","effects.apply","clips.combine","media.cleanup","pitch.analyze","pitch.render","pitch.targets","pitch.apply","pitch.manualApply","effects.preview","drums.preview","drums.stop","designer.preview","designer.stop","designer.configure","designer.noteOn","designer.noteOff","designer.allNotesOff","designer.controller","designer.midiSources","designer.midiConnect","take.preview","take.stop","audition.stop","transport.audition","master.analyze","master.proposal","master.applyProposal","drums.kits","drums.saveKit","drums.deleteKit"],"sampleRate":48000,"deviceBackends":[],"codecs":["wav-pcm","wav-float"],"snapshotPagination":true,"maxFrameBytes":8388608,"maxUploadChunkBytes":262144,"maxUploadBytes":2147483648u64,"maxTimelineFrame":soundroom_engine::model::MAX_FRAME,"features":["compact-control-patches","authored-resource-history","verified-chunk-transfer","paged-snapshot","offline-export","mastering"],"effects":[],"selectionEffects":[],"instruments":[],"designerPresets":[],"drumKits":[],"drumPresets":[]});
        let native: Value = serde_json::from_str(include_str!("../catalog/native-v3.json"))
            .expect("checked native catalog");
        for key in [
            "effects",
            "instruments",
            "designerPresets",
            "drumKits",
            "drumPresets",
        ] {
            value[key] = native["capabilities"][key].clone();
        }
        value
    }
    fn compact_request(&self, author: &str, request: &RemoteRequest) -> Option<CompactRequest> {
        let cache = self.witness.lock().ok()?;
        let witness = cache
            .as_ref()
            .filter(|witness| witness.revision == self.authority.revision)?;
        if matches!(request.command, RemoteCommand::Undo | RemoteCommand::Redo) {
            let redo = matches!(request.command, RemoteCommand::Redo);
            let (id, controls) = self.authority.control_history_target(author, redo)?;
            witness.controls.get(id)?;
            return Some(CompactRequest {
                track_id: id.into(),
                controls,
                label: String::new(),
                gesture: None,
                history: Some(redo),
            });
        }
        let (id, label, gesture) = match &request.command {
            RemoteCommand::SetTrackGain { track_id, .. } => (track_id, "Adjust volume", None),
            RemoteCommand::SetTrackPan { track_id, .. } => (track_id, "Adjust pan", None),
            RemoteCommand::SetTrackMute { track_id, .. } => (track_id, "Mute track", None),
            RemoteCommand::SetTrackSolo { track_id, .. } => (track_id, "Solo track", None),
            RemoteCommand::SetTrackArm { track_id, .. } => (track_id, "Arm track", None),
            RemoteCommand::UpdateTrackControls {
                controls,
                name,
                gesture_id,
            } => {
                let track = witness
                    .base
                    .tracks
                    .iter()
                    .chain(std::iter::once(&witness.base.master_track))
                    .find(|track| track.id == controls.track_id)?;
                if !unchanged_control_content(track, controls).ok()? {
                    return None;
                }
                (&controls.track_id, name.as_str(), gesture_id.clone())
            }
            _ => return None,
        };
        let mut controls = *witness.controls.get(id)?;
        match &request.command {
            RemoteCommand::SetTrackGain { gain, .. } => controls.gain = *gain,
            RemoteCommand::SetTrackPan { pan, .. } => controls.pan = *pan,
            RemoteCommand::SetTrackMute { muted, .. } => controls.muted = *muted,
            RemoteCommand::SetTrackSolo { solo, .. } => controls.solo = *solo,
            RemoteCommand::SetTrackArm { armed, .. } => controls.armed = *armed,
            RemoteCommand::UpdateTrackControls {
                controls: source, ..
            } => {
                controls = MixControls {
                    gain: source.gain,
                    pan: source.pan,
                    muted: source.muted,
                    solo: source.solo,
                    armed: source.armed,
                }
            }
            _ => unreachable!(),
        }
        Some(CompactRequest {
            track_id: id.clone(),
            controls,
            label: label.into(),
            gesture,
            history: None,
        })
    }
    fn apply_compact(
        &mut self,
        peer: &Identity,
        request: &RemoteRequest,
        compact: &CompactRequest,
    ) -> Result<Handled, Error> {
        compact.controls.validate().map_err(engine)?;
        let mut witness = self
            .witness
            .lock()
            .map_err(|_| protocol("Session witness is unavailable"))?;
        let current = witness
            .as_ref()
            .filter(|witness| witness.revision == self.authority.revision)
            .ok_or_else(|| Error::Conflict("Content changed while preparing controls".into()))?;
        let before = *current
            .controls
            .get(&compact.track_id)
            .ok_or_else(|| protocol("Control target unavailable"))?;
        let new_tokens = control_tokens(compact.controls)?;
        let bytes = current
            .bytes
            .checked_sub(control_tokens(before)?)
            .and_then(|bytes| bytes.checked_add(new_tokens))
            .ok_or_else(|| protocol("Invalid size witness"))?;
        let mut versions = self.authority.track_revisions.clone();
        let next = self
            .authority
            .revision
            .saturating_add(1)
            .min(soundroom_engine::model::MAX_FRAME);
        versions.insert(compact.track_id.clone(), next);
        let mut header = self
            .capture_source(
                SessionInput::Owned(Arc::new(Session::default())),
                next,
                &versions,
                &self.media,
            )
            .value()?;
        neutralize_snapshot(&mut header);
        header["session"] = Value::Null;
        let total = encoded_size(&header)?
            .checked_sub(4)
            .and_then(|size| size.checked_add(bytes))
            .ok_or_else(|| protocol("Invalid complete document size"))?;
        if total > validation::MAX_SNAPSHOT_BYTES {
            return Err(protocol("Complete session document exceeds 64 MiB"));
        }
        let expected = Self::expected(request);
        let receipt = match compact.history {
            Some(false) => self.authority.undo(&peer.runtime, role(&peer.role)),
            Some(true) => self.authority.redo(&peer.runtime, role(&peer.role)),
            None => self.authority.edit_controls(
                &peer.runtime,
                role(&peer.role),
                ControlEdit {
                    request_id: &request.id,
                    expected: &expected,
                    track_id: &compact.track_id,
                    controls: compact.controls,
                    label: &compact.label,
                    gesture: compact.gesture.as_deref(),
                },
            ),
        }
        .map_err(engine)?;
        if !receipt.changed.is_empty() {
            let current = witness.as_mut().unwrap();
            current.bytes = bytes;
            current.revision = receipt.revision;
            current
                .controls
                .insert(compact.track_id.clone(), compact.controls);
        }
        if let Some(backend) = &self.backend {
            backend.observe_revision(self.authority.revision);
        }
        Ok(Handled::Payload(None))
    }
    fn expected(request: &RemoteRequest) -> BTreeMap<String, u64> {
        request
            .expected_track_revisions
            .iter()
            .map(|(id, v)| (id.to_ascii_uppercase(), *v as u64))
            .collect()
    }
    fn commit(
        &mut self,
        peer: &Identity,
        request: &RemoteRequest,
        candidate: Session,
    ) -> Result<(), Error> {
        self.require_editable()?;
        if !self.registry.contains(&peer.credential) {
            return Err(protocol(
                "Credential was revoked before the authored commit",
            ));
        }
        if candidate.tracks.len() > 1023 {
            return Err(protocol(
                "Portable control supports at most 1,023 audio tracks plus master",
            ));
        }
        Project {
            session: candidate.clone(),
            media: self.media.clone(),
        }
        .validate_sources()
        .map_err(engine)?;
        // Admit the complete document before changing authority; future fallback must remain loadable.
        let next = self
            .authority
            .revision
            .saturating_add(1)
            .min(soundroom_engine::model::MAX_FRAME);
        let versions = candidate
            .tracks
            .iter()
            .chain(std::iter::once(&candidate.master_track))
            .map(|t| (t.id.clone(), next))
            .collect();
        let mut document = self.snapshot_for(&candidate, next, &versions, &self.media)?;
        neutralize_snapshot(&mut document);
        struct Count(usize);
        impl Write for Count {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                self.0 = self.0.saturating_add(data.len());
                if self.0 > validation::MAX_SNAPSHOT_BYTES {
                    return Err(std::io::Error::other(
                        "Complete session document exceeds 64 MiB",
                    ));
                }
                Ok(data.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        serde_json::to_writer(&mut Count(0), &document)
            .map_err(|_| protocol("Complete session document exceeds 64 MiB"))?;
        drop(document);
        let (label, gesture) = history_action(&request.command);
        if !valid_name(label) || gesture.is_some_and(|g| !valid_name(g)) {
            return Err(protocol("Invalid history action or gesture"));
        }
        let revision = self.authority.revision;
        // An unfenced import is server-placed: the candidate was built from the
        // current session under this lock, so it is fenced to the current state.
        let placed = (matches!(request.command, RemoteCommand::FinishUpload { .. })
            && request.expected_revision.is_none()
            && request.expected_track_revisions.is_empty())
            || master::server_placed(request);
        let (expected_revision, expected) = if placed {
            (Some(revision), self.authority.track_revisions.clone())
        } else {
            (
                request.expected_revision.map(|r| r as u64),
                Self::expected(request),
            )
        };
        self.authority
            .edit_session(
                &peer.runtime,
                role(&peer.role),
                expected_revision,
                &expected,
                candidate,
            )
            .map_err(engine)?;
        if self.authority.revision != revision {
            self.authority
                .label_last_change(&peer.runtime, label, gesture)
                .map_err(engine)?;
        }
        Ok(())
    }
    /// Adds an imported 48 kHz WAV as a clip (new track unless one is named) and
    /// commits it as one authored action. The media file is hard-linked into Media/.
    #[allow(clippy::too_many_arguments)]
    fn place_import(
        &mut self,
        peer: &Identity,
        request: &RemoteRequest,
        mut candidate: Session,
        path: &Path,
        info: soundroom_engine::wave::WaveInfo,
        name: &str,
        track_id: Option<&str>,
        frame: u64,
    ) -> Result<String, Error> {
        let stored = self
            .media
            .values()
            .map(|m| m.info.frames.saturating_mul(m.info.block_align as u64))
            .sum::<u64>();
        if stored + info.frames.saturating_mul(info.block_align as u64) > 8 * 1024 * 1024 * 1024 {
            return Err(protocol("Portable media storage quota exceeded"));
        }
        let asset = new_id()?;
        let destination = self.root.join("Media").join(format!("{asset}.wav"));
        let track = if let Some(id) = track_id {
            Self::track(&mut candidate, id)?
        } else {
            candidate.tracks.push(Track {
                id: new_id()?,
                name: name.into(),
                ..Track::default()
            });
            candidate.tracks.last_mut().unwrap()
        };
        track.clips.push(Clip {
            id: new_id()?,
            asset_id: asset.clone(),
            name: name.into(),
            start: frame,
            offset: 0,
            length: info.frames,
            edge_fade_frames: Some(EDGE_FADE_FRAMES),
            edge_protection_disabled: None,
            cut_fade_in_frames: None,
            cut_fade_out_frames: None,
            fade_in_frames: None,
            fade_out_frames: None,
            fade_reference_offset: None,
            fade_reference_length: None,
            render_envelope: None,
            previous_states: vec![],
            extra: BTreeMap::new(),
        });
        std::fs::hard_link(path, &destination)?;
        self.media.insert(
            asset.clone(),
            Media {
                path: destination.clone(),
                info: info.clone(),
            },
        );
        if let Err(e) = self.commit(peer, request, candidate) {
            self.media.remove(&asset);
            let _ = std::fs::remove_file(destination);
            return Err(e);
        }
        Ok(asset)
    }
    fn track<'a>(session: &'a mut Session, id: &str) -> Result<&'a mut Track, Error> {
        if session.master_track.id.eq_ignore_ascii_case(id) {
            Ok(&mut session.master_track)
        } else {
            session
                .tracks
                .iter_mut()
                .find(|t| t.id.eq_ignore_ascii_case(id))
                .ok_or_else(|| Error::Conflict("Track is unavailable".into()))
        }
    }
    fn remember(&mut self, key: (String, String, String), digest: [u8; 32], reply: Value) {
        let bytes = serde_json::to_vec(&reply).map_or(0, |v| v.len()) + 160;
        self.receipts.push_back(Receipt {
            key: key.clone(),
            digest,
            reply,
            bytes,
        });
        // Once this target has a retained receipt, its cancellation is resolved.
        self.cancelled_requests.remove(&key);
        while self
            .receipts
            .iter()
            .filter(|r| r.key.0 == key.0 && r.key.1 == key.1)
            .count()
            > 256
        {
            let index = self
                .receipts
                .iter()
                .position(|r| r.key.0 == key.0 && r.key.1 == key.1)
                .unwrap();
            self.receipts.remove(index);
        }
        while self.receipts.len() > 4096 {
            self.receipts.pop_front();
        }
        loop {
            let global = self.receipts.iter().map(|r| r.bytes).sum::<usize>();
            let own = self
                .receipts
                .iter()
                .filter(|r| r.key.0 == key.0 && r.key.1 == key.1)
                .map(|r| r.bytes)
                .sum::<usize>();
            if global <= 32 * 1024 * 1024 && own <= 4 * 1024 * 1024 {
                break;
            }
            let record = self.receipts.iter_mut().find(|r| {
                r.reply.get("payload").is_some()
                    && (own <= 4 * 1024 * 1024 || (r.key.0 == key.0 && r.key.1 == key.1))
            });
            let Some(record) = record else {
                break;
            };
            record.reply.as_object_mut().unwrap().remove("payload");
            record.reply["message"]=json!("Original request completed; retained reply payload expired. Refresh state or request a new read-only artifact chunk.");
            record.bytes = serde_json::to_vec(&record.reply).map_or(0, |v| v.len()) + 160;
        }
    }
    fn admits_job(&self, peer: &Identity, critical: bool) -> bool {
        if critical {
            self.pending
                .iter()
                .filter(|(_, pending)| pending.critical)
                .count()
                < 8
                && self
                    .pending
                    .iter()
                    .filter(|(key, pending)| {
                        pending.critical && key.0 == peer.credential && key.1 == peer.runtime
                    })
                    .count()
                    < 2
        } else {
            self.pending
                .values()
                .filter(|pending| !pending.critical && !pending.write && !pending.waveform)
                .count()
                < 4
        }
    }
    fn admits_waveform(&self, peer: &Identity) -> bool {
        self.pending
            .values()
            .filter(|pending| pending.waveform)
            .count()
            < 4
            && self
                .pending
                .iter()
                .filter(|(key, pending)| {
                    pending.waveform && key.0 == peer.credential && key.1 == peer.runtime
                })
                .count()
                < 2
    }
    fn admits_write(&self, peer: &Identity, terminal: bool) -> bool {
        let total = self.pending.values().filter(|p| p.write).count();
        let own = self
            .pending
            .iter()
            .filter(|(key, p)| p.write && key.0 == peer.credential && key.1 == peer.runtime)
            .count();
        if terminal {
            own < writes::OWNER_WRITE_JOBS + writes::OWNER_TERMINAL_RESERVE
                && total < writes::TOTAL_WRITE_JOBS + 16
        } else {
            own < writes::OWNER_WRITE_JOBS && total < writes::TOTAL_WRITE_JOBS
        }
    }
    fn expire(&mut self) {
        self.writes.expire();
        self.uploads
            .retain(|_, u| u.touched.elapsed() < Duration::from_secs(600));
        self.artifacts.retain(|_, a| {
            if a.expires < Instant::now() {
                let _ = std::fs::remove_file(&a.path);
                false
            } else {
                true
            }
        });
    }
    /// Identity comes exclusively from the authenticated channel, never the request body.
    pub fn dispatch(
        shared: &Shared,
        peer: &Identity,
        out: &Outbox,
        value: Value,
    ) -> Result<(), Error> {
        Self::dispatch_validated(shared, peer, out, Self::validate_request(peer, value)?)
    }
    pub(crate) fn validate_request(
        peer: &Identity,
        mut value: Value,
    ) -> Result<ValidatedRequest, Error> {
        if value["version"] != 3 || value["kind"] != "request" {
            return Err(protocol("Expected v3 request"));
        }
        let object = value
            .as_object_mut()
            .ok_or_else(|| protocol("Invalid request"))?;
        object.remove("version");
        object.remove("kind");
        object
            .entry("expectedTrackRevisions")
            .or_insert_with(|| json!({}));
        if let Err(error) = validation::validate_model(&value, "RemoteRequest") {
            if std::env::var_os("SOUNDROOM_DEBUG_CONNECTIONS").is_some() {
                eprintln!(
                    "Soundroom rejected request model: {error}: {}",
                    value["command"]
                );
            }
            return Err(error);
        }
        if !value["clientID"]
            .as_str()
            .is_some_and(|id| id.eq_ignore_ascii_case(&peer.runtime))
        {
            return Err(protocol(
                "Request identity differs from the authenticated peer",
            ));
        }
        canonical_ids(&mut value);
        let digest: [u8; 32] = Sha256::digest(serde_json::to_vec(&value)?).into();
        let request: RemoteRequest = serde_json::from_value(value)?;
        Ok(ValidatedRequest {
            request,
            digest,
            credential: peer.credential.clone(),
            runtime: peer.runtime.clone(),
            role: peer.role.clone(),
            transport_generation: None,
        })
    }
    pub(crate) fn authored_in_flight(&self, peer: &Identity) -> bool {
        self.pending.iter().any(|(key, pending)| {
            pending.authored && key.0 == peer.credential && key.1 == peer.runtime
        })
    }
    pub(crate) fn dispatch_validated(
        shared: &Shared,
        peer: &Identity,
        out: &Outbox,
        prepared: ValidatedRequest,
    ) -> Result<(), Error> {
        if prepared.credential != peer.credential
            || prepared.runtime != peer.runtime
            || prepared.role != peer.role
        {
            return Err(protocol(
                "Validated request differs from authenticated peer",
            ));
        }
        let authored = prepared.authored();
        let ValidatedRequest {
            request,
            digest,
            transport_generation,
            ..
        } = prepared;
        if std::env::var_os("SOUNDROOM_DEBUG_CONNECTIONS").is_some() {
            let kind = serde_json::to_value(&request.command)
                .ok()
                .and_then(|v| {
                    let op = v["intent"]["operation"].as_str().map(str::to_owned);
                    v["type"].as_str().map(|t| match op {
                        Some(op) => format!("{t}:{op}"),
                        None => t.to_owned(),
                    })
                })
                .unwrap_or_default();
            eprintln!(
                "Soundroom request: {kind} expectedRevision={:?} tracks={}",
                request.expected_revision,
                request.expected_track_revisions.len()
            );
        }
        let k = key(peer, &request);
        let mut state = shared
            .lock()
            .map_err(|_| protocol("Service authority is unavailable"))?;
        if !state.registry.authorizes(&peer.credential, &peer.role) {
            return Err(protocol("Credential was revoked or its role changed"));
        }
        if let Some(old) = state.receipts.iter().find(|r| r.key == k) {
            if old.digest != digest {
                return Err(protocol("Request ID was reused with different content"));
            }
            return out.send(&old.reply, true);
        }
        if state.shutdown_requested {
            return out.send(
                &response(
                    &request.id,
                    "rejected",
                    state.authority.revision,
                    "The server is shutting down; reconnect before issuing commands",
                    None,
                ),
                true,
            );
        }
        if let Some(old) = state.pending.get(&k) {
            if old.digest != digest {
                return Err(protocol("In-flight request ID was reused"));
            }
            return out.send(
                &response(
                    &request.id,
                    "busy",
                    state.authority.revision,
                    "Request remains in flight",
                    None,
                ),
                true,
            );
        }
        state.expire();
        let revision = state.authority.revision;
        let changes = matches!(
            request.command,
            RemoteCommand::UpdateTrackControls { .. }
                | RemoteCommand::SetTrackName { .. }
                | RemoteCommand::SetTrackGain { .. }
                | RemoteCommand::SetTrackPan { .. }
                | RemoteCommand::SetTrackMute { .. }
                | RemoteCommand::SetTrackSolo { .. }
                | RemoteCommand::SetTrackArm { .. }
                | RemoteCommand::SetTrackInput { .. }
                | RemoteCommand::AddTrack { .. }
                | RemoteCommand::RemoveTrack { .. }
                | RemoteCommand::SplitClip { .. }
                | RemoteCommand::DeleteClips { .. }
                | RemoteCommand::MoveClip { .. }
                | RemoteCommand::ReplaceTracks { .. }
                | RemoteCommand::EditTracks { .. }
                | RemoteCommand::Undo
                | RemoteCommand::Redo
                | RemoteCommand::FinishUpload { .. }
        ) || matches!(&request.command,RemoteCommand::Editor{intent}
            if matches!(intent.operation.as_str(), "session.timing" | "session.rename" | "master.applyProposal" | "clips.combine"));
        let compact = state.compact_request(&peer.runtime, &request);
        let prepared_edit = compact.is_none() && edits::plain(&request.command);
        let before = if changes && compact.is_none() && !prepared_edit {
            Some(state.authority.session.clone())
        } else {
            None
        };
        let starts_transport = matches!(
            request.command,
            RemoteCommand::Play { .. } | RemoteCommand::Record
        ) || matches!(&request.command, RemoteCommand::Editor { intent }
                if matches!(intent.operation.as_str(), "transport.play" | "transport.toggle" | "transport.record" | "transport.audition"));
        let handled = if state.cancelled_requests.contains(&k) {
            Err(Error::Conflict(
                "Request was cancelled before processing".into(),
            ))
        } else if starts_transport
            && transport_generation.is_some_and(|received| {
                received != state.transport_generation.load(Ordering::Acquire)
            })
        {
            Err(Error::Conflict(
                "Transport start was superseded by Stop".into(),
            ))
        } else if authored && state.require_editable().is_err() {
            Err(Error::Conflict(
                "Finish recording before changing the session".into(),
            ))
        } else if let Some(compact) = &compact {
            state.apply_compact(peer, &request, compact)
        } else if prepared_edit {
            edits::Task::capture(&state, peer).map(|task| {
                Handled::Job(JobKind::Edit {
                    task: Box::new(task),
                })
            })
        } else {
            state.handle(peer, &request)
        };
        match handled {
            Ok(Handled::Job(kind)) => {
                let critical = matches!(
                    &kind,
                    JobKind::FinishRecording { .. }
                        | JobKind::Transport {
                            command: crate::audio::Command::Pause,
                            ..
                        }
                );
                let write = match &kind {
                    JobKind::Write { task } => Some(task.terminal()),
                    _ => None,
                };
                let admitted = match write {
                    Some(terminal) => state.admits_write(peer, terminal),
                    None if matches!(kind, JobKind::Waveform { .. }) => state.admits_waveform(peer),
                    None => state.admits_job(peer, critical),
                };
                if !admitted {
                    if let JobKind::Write { task } = &kind {
                        state.writes.abandon_beginning(task);
                    }
                    if let JobKind::FinishRecording { active, .. } = kind {
                        state.recording = Some(active);
                        state.recording_finalizing = false;
                    }
                    let reply = response(
                        &request.id,
                        "busy",
                        state.authority.revision,
                        "Worker queue is full",
                        None,
                    );
                    return out.send(&reply, true);
                }
                if critical {
                    if let Err(error) = state.invalidate_transport_starts() {
                        if let JobKind::FinishRecording { active, .. } = kind {
                            state.recording = Some(active);
                            state.recording_finalizing = false;
                        }
                        let reply = response(
                            &request.id,
                            "rejected",
                            state.authority.revision,
                            &error.to_string(),
                            None,
                        );
                        state.remember(k, digest, reply.clone());
                        return out.send(&reply, true);
                    }
                    if let JobKind::FinishRecording { active, .. } = &kind {
                        active.request_stop();
                    }
                }
                let transport_start = matches!(
                    &kind,
                    JobKind::Record { .. }
                        | JobKind::Transport {
                            command: crate::audio::Command::Play | crate::audio::Command::Seek(_),
                            ..
                        }
                );
                let cancel = match &kind {
                    JobKind::Process { task } => task
                        .preview
                        .as_ref()
                        .map(|(_, _, cancel)| cancel.clone())
                        .unwrap_or_else(|| Arc::new(AtomicBool::new(false))),
                    _ => Arc::new(AtomicBool::new(false)),
                };
                if matches!(kind, JobKind::Record { .. }) {
                    state.recording_preparing = Some(cancel.clone());
                    state.broadcast_transport();
                }
                state.pending.insert(
                    k,
                    Pending {
                        authored,
                        critical,
                        transport_start,
                        write: write.is_some(),
                        save: matches!(kind, JobKind::Save { .. }),
                        waveform: matches!(kind, JobKind::Waveform { .. }),
                        digest,
                        cancel: cancel.clone(),
                    },
                );
                let job = Job {
                    peer: peer.clone(),
                    request,
                    kind,
                    cancel,
                };
                drop(state);
                let shared = shared.clone();
                let out = out.clone();
                std::thread::spawn(move || run_job(shared, out, job, digest));
                Ok(())
            }
            Ok(Handled::Snapshot) => {
                if out.snapshot_active.load(Ordering::Acquire)
                    || out.snapshot_preparing.load(Ordering::Acquire)
                {
                    // Coalesce: the in-flight transfer plus later deltas already
                    // deliver this state (the initial snapshot is pushed on join).
                    let reply = response(
                        &request.id,
                        "accepted",
                        state.authority.revision,
                        "Snapshot transfer already in progress",
                        None,
                    );
                    state.remember(k, digest, reply.clone());
                    return out.send(&reply, true);
                }
                let reply = response(
                    &request.id,
                    "accepted",
                    state.authority.revision,
                    "Snapshot",
                    None,
                );
                state.remember(k, digest, reply.clone());
                out.send(&reply, true)?;
                state.queue_snapshot(out)
            }
            result => {
                let reply = match result {
                    Ok(Handled::Features(compact)) => {
                        out.compact_controls.store(compact, Ordering::Release);
                        response(
                            &request.id,
                            "accepted",
                            state.authority.revision,
                            "Client features received",
                            None,
                        )
                    }
                    Ok(Handled::Payload(payload)) => response(
                        &request.id,
                        "accepted",
                        state.authority.revision,
                        "",
                        payload,
                    ),
                    Err(Error::Conflict(message)) => response(
                        &request.id,
                        "conflict",
                        state.authority.revision,
                        &message,
                        None,
                    ),
                    Err(Error::QueueFull) => response(
                        &request.id,
                        "busy",
                        state.authority.revision,
                        "Server is busy",
                        None,
                    ),
                    Err(error) => response(
                        &request.id,
                        "rejected",
                        state.authority.revision,
                        &error.to_string(),
                        None,
                    ),
                    _ => unreachable!(),
                };
                if let Some(compact) = &compact {
                    if revision != state.authority.revision {
                        let input = state.capture_snapshot();
                        state.schedule_audio_update(&input);
                        let task = Publication {
                            input,
                            before: None,
                            prepared: None,
                            compact: Some((compact.track_id.clone(), compact.controls)),
                            targets: state
                                .peers
                                .values()
                                .map(|peer| {
                                    (
                                        format!("{:p}", Arc::as_ptr(&peer.out.closed)),
                                        peer.out.clone(),
                                    )
                                })
                                .collect(),
                        };
                        if state.publication.enqueue(task).is_err() {
                            for peer in state.peers.values() {
                                peer.out.close();
                            }
                        }
                        state.history_events();
                    }
                }
                if let Some(before) = before {
                    if state.publish_changes(before, revision).is_err() {
                        for peer in state.peers.values() {
                            peer.out.close();
                        }
                    }
                    if revision != state.authority.revision {
                        state.history_events();
                    }
                }
                state.remember(k, digest, reply.clone());
                out.send(&reply, true)
            }
        }
    }
    fn handle(&mut self, peer: &Identity, r: &RemoteRequest) -> Result<Handled, Error> {
        let readonly = matches!(
            r.command,
            RemoteCommand::Snapshot
                | RemoteCommand::Ping { .. }
                | RemoteCommand::ListProjects
                | RemoteCommand::ReadArtifact { .. }
                | RemoteCommand::DownloadArtifact { .. }
                | RemoteCommand::Waveform { .. }
        ) || matches!(&r.command,RemoteCommand::Editor{intent}
            if matches!(intent.operation.as_str(), "client.features" | "master.proposal" | "sources.status"));
        if peer.role == RemoteRole::Viewer && !readonly {
            return Err(protocol("Editor or operator role is required"));
        }
        let needs_session = matches!(
            r.command,
            RemoteCommand::UpdateTrackControls { .. }
                | RemoteCommand::SetTrackName { .. }
                | RemoteCommand::SetTrackGain { .. }
                | RemoteCommand::SetTrackPan { .. }
                | RemoteCommand::SetTrackMute { .. }
                | RemoteCommand::SetTrackSolo { .. }
                | RemoteCommand::SetTrackArm { .. }
                | RemoteCommand::SetTrackInput { .. }
                | RemoteCommand::AddTrack { .. }
                | RemoteCommand::RemoveTrack { .. }
                | RemoteCommand::SplitClip { .. }
                | RemoteCommand::DeleteClips { .. }
                | RemoteCommand::MoveClip { .. }
                | RemoteCommand::ReplaceTracks { .. }
                | RemoteCommand::EditTracks { .. }
                | RemoteCommand::FinishUpload { .. }
                | RemoteCommand::SaveProject { .. }
                | RemoteCommand::ExportMix { .. }
        );
        let mut candidate = if needs_session {
            self.authority.session.clone()
        } else {
            Session::default()
        };
        match &r.command {
            RemoteCommand::Snapshot => return Ok(Handled::Snapshot),
            RemoteCommand::Ping { client_time } => {
                return Ok(Handled::Payload(Some(
                    json!({"clientTime":client_time,"serverTime":self.started.elapsed().as_secs_f64()}),
                )))
            }
            RemoteCommand::TakeTransport => {
                if peer.role != RemoteRole::Operator {
                    return Err(protocol("Operator role is required"));
                }
                self.operator = Some(peer.runtime.clone());
                return Ok(Handled::Payload(None));
            }
            RemoteCommand::Play { track_only } => {
                if track_only.is_some() {
                    return Err(protocol(
                        "Track-only hardware audition is not qualified yet",
                    ));
                }
                return self.transport_job(peer, crate::audio::Command::Play);
            }
            RemoteCommand::Stop => return self.transport_job(peer, crate::audio::Command::Pause),
            RemoteCommand::Seek { frame } => {
                return self.transport_job(peer, crate::audio::Command::Seek(*frame as u64))
            }
            RemoteCommand::SetLoop { start, end } => {
                self.require_transport_owner(peer, false)?;
                self.require_editable()?;
                if !self.admits_job(peer, false) {
                    return Err(Error::QueueFull);
                }
                let range = match (start, end) {
                    (Some(start), Some(end)) if *start >= 0 && *end >= 0 => Some(
                        soundroom_engine::render::LoopRange::new(*start as u64, *end as u64)
                            .map_err(engine)?,
                    ),
                    (None, None) => None,
                    _ => return Err(protocol("A loop needs a valid start and end")),
                };
                let backend = self.backend.clone().ok_or(Error::QueueFull)?;
                backend.set_loop(range);
                let frame = match self.authority.transport {
                    soundroom_engine::authority::Transport::Playing { frame }
                    | soundroom_engine::authority::Transport::Stopped { frame }
                    | soundroom_engine::authority::Transport::Recording { frame, .. } => frame,
                };
                let playing = matches!(
                    self.authority.transport,
                    soundroom_engine::authority::Transport::Playing { .. }
                );
                let transport = self.transport_value(frame, playing, false);
                self.broadcast(&json!({"version":3,"kind":"event","event":{"type":"transport","transport":transport}}), false);
                // A running transport re-prepares on the new cycle.
                if playing {
                    return self.transport_job(peer, crate::audio::Command::Play);
                }
                return Ok(Handled::Payload(None));
            }
            RemoteCommand::Record => return self.begin_recording(peer, None),
            RemoteCommand::SetDevices {
                input_uid,
                output_uid,
            } => {
                if peer.role != RemoteRole::Operator {
                    return Err(protocol("Operator role is required"));
                }
                self.require_editable()?;
                if self.recording.is_some() || self.recording_finalizing {
                    return Err(Error::Conflict(
                        "Finish recording before changing devices".into(),
                    ));
                }
                #[cfg(target_os = "macos")]
                {
                    let devices =
                        soundroom_engine::core_audio::core_audio_devices().map_err(engine)?;
                    if let Some(uid) = input_uid {
                        if !devices
                            .iter()
                            .any(|d| &d.uid == uid && d.input_channels > 0)
                        {
                            return Err(Error::Conflict("Input device is unavailable".into()));
                        }
                    }
                    if let Some(uid) = output_uid {
                        if !devices
                            .iter()
                            .any(|d| &d.uid == uid && d.output_channels > 0)
                        {
                            return Err(Error::Conflict("Output device is unavailable".into()));
                        }
                        let current = self.output_uid.clone().or_else(|| {
                            devices
                                .iter()
                                .find(|d| d.is_default_output)
                                .map(|d| d.uid.clone())
                        });
                        if current.as_ref() != Some(uid) {
                            if self.backend.as_ref().is_some_and(|b| b.active()) {
                                return Err(Error::Conflict(
                                    "Stop playback before changing the output device".into(),
                                ));
                            }
                            let switch = self.output_switch.as_ref().ok_or_else(|| {
                                protocol("This host cannot change its output device")
                            })?;
                            let (reply, result) = std::sync::mpsc::sync_channel(1);
                            switch
                                .try_send(OutputRequest {
                                    uid: Some(uid.clone()),
                                    reply,
                                })
                                .map_err(|_| Error::QueueFull)?;
                            result
                                .recv_timeout(Duration::from_secs(5))
                                .map_err(|_| protocol("The output device did not respond"))?
                                .map_err(|m| {
                                    Error::Conflict(format!(
                                        "Could not open the output device: {m}"
                                    ))
                                })?;
                            self.output_uid = Some(uid.clone());
                        }
                    }
                }
                #[cfg(not(target_os = "macos"))]
                let _ = output_uid;
                // Both selectors are partial updates: omitted/null input must
                // preserve the microphone when the client changes its output.
                if let Some(uid) = input_uid {
                    self.input_uid = Some(uid.clone());
                }
                let event = self.device_event();
                self.broadcast(&event, false);
                return Ok(Handled::Payload(None));
            }
            RemoteCommand::SetTrackInput {
                track_id,
                device_uid,
                channel,
                stereo,
            } => {
                if *channel < 0 || *channel > 63 {
                    return Err(protocol("Invalid input channel"));
                }
                let track = Self::track(&mut candidate, track_id)?;
                if track.extra.get("inputDeviceUID").and_then(Value::as_str)
                    != device_uid.as_deref()
                {
                    track.extra.remove("inputDeviceName");
                }
                track.extra.insert("inputChannel".into(), json!(channel));
                track.extra.insert("stereoInput".into(), json!(stereo));
                match device_uid {
                    Some(uid) => {
                        track.extra.insert("inputDeviceUID".into(), json!(uid));
                    }
                    None => {
                        track.extra.remove("inputDeviceUID");
                    }
                }
            }
            RemoteCommand::UpdateTrackControls { .. }
            | RemoteCommand::SetTrackName { .. }
            | RemoteCommand::SetTrackGain { .. }
            | RemoteCommand::SetTrackPan { .. }
            | RemoteCommand::SetTrackMute { .. }
            | RemoteCommand::SetTrackSolo { .. }
            | RemoteCommand::SetTrackArm { .. }
            | RemoteCommand::AddTrack { .. }
            | RemoteCommand::RemoveTrack { .. }
            | RemoteCommand::SplitClip { .. }
            | RemoteCommand::DeleteClips { .. }
            | RemoteCommand::MoveClip { .. }
            | RemoteCommand::ReplaceTracks { .. }
            | RemoteCommand::EditTracks { .. } => {
                candidate = edits::apply(candidate, &r.command)?;
            }
            RemoteCommand::Undo => {
                self.authority
                    .undo(&peer.runtime, role(&peer.role))
                    .map_err(engine)?;
                return Ok(Handled::Payload(None));
            }
            RemoteCommand::Redo => {
                self.authority
                    .redo(&peer.runtime, role(&peer.role))
                    .map_err(engine)?;
                return Ok(Handled::Payload(None));
            }
            RemoteCommand::ListProjects => {
                return Ok(Handled::Payload(Some(json!(self.projects()?))))
            }
            RemoteCommand::OpenProject { id } => {
                self.require_paused()?;
                if peer.role != RemoteRole::Operator {
                    return Err(protocol("Operator role is required to open a project"));
                }
                if r.expected_revision != Some(self.authority.revision as i64) {
                    return Err(Error::Conflict("Stale project revision".into()));
                }
                if !safe_filename(id) || !id.ends_with(".soundroom") {
                    return Err(protocol("Invalid project identity"));
                }
                return Ok(Handled::Job(JobKind::Open {
                    path: self.root.join("Projects").join(id),
                    expected: self.authority.revision,
                }));
            }
            RemoteCommand::SaveProject {
                name,
                replace_existing,
                title: requested_title,
            } => {
                if peer.role != RemoteRole::Operator {
                    return Err(protocol("Operator role is required to save a project"));
                }
                self.require_editable()?;
                if self.pending.values().any(|pending| pending.save) {
                    return Err(Error::Conflict(
                        "A project save is already in progress. Wait for it to finish.".into(),
                    ));
                }
                if r.expected_revision
                    .is_some_and(|expected| expected != self.authority.revision as i64)
                {
                    return Err(Error::Conflict("The session changed before saving".into()));
                }
                let title = names::project_title(name)?;
                let path = self
                    .root
                    .join("Projects")
                    .join(format!("{title}.soundroom"));
                candidate.name = match requested_title {
                    Some(title) => names::validated(title)?,
                    None if *replace_existing == Some(true) => candidate.name.clone(),
                    None => title,
                };
                return Ok(Handled::Job(JobKind::Save {
                    project: Box::new(Project {
                        session: candidate,
                        media: self.media.clone(),
                    }),
                    path,
                    expected: self.authority.revision,
                    replace_existing: *replace_existing == Some(true),
                }));
            }
            RemoteCommand::ExportMix { name } => {
                if !safe_filename(name) {
                    return Err(protocol("Invalid export name"));
                }
                return Ok(Handled::Job(JobKind::Export {
                    project: Box::new(Project {
                        session: candidate,
                        media: self.media.clone(),
                    }),
                    path: self
                        .root
                        .join("Artifacts")
                        .join(format!("{}.wav", new_id()?)),
                    name: format!("{}.wav", name.trim_end_matches(".wav")),
                    start: 0,
                    end: self.authority.session.end(),
                    tails: true,
                    stems: None,
                }));
            }
            RemoteCommand::BeginUpload {
                id,
                name,
                bytes,
                sha256,
            } => {
                if !safe_filename(name)
                    || !validation::uuid(id)
                    || !valid_hash(sha256)
                    || self.uploads.contains_key(id)
                    || self.uploads.len() >= 4
                    || self
                        .uploads
                        .values()
                        .filter(|u| u.owner == peer.credential)
                        .count()
                        >= 2
                    || self.uploads.values().map(|u| u.total).sum::<u64>() + *bytes as u64
                        > 4 * 1024 * 1024 * 1024
                {
                    return Err(protocol("Invalid upload or storage quota exceeded"));
                }
                let path = self.root.join("Uploads").join(format!("{id}.part"));
                crate::registry::write_new_private(&path, &[])?;
                self.uploads.insert(
                    id.clone(),
                    Upload {
                        owner: peer.credential.clone(),
                        name: name.clone(),
                        path,
                        total: *bytes as u64,
                        offset: 0,
                        hash: Sha256::new(),
                        expected: sha256.clone(),
                        touched: Instant::now(),
                    },
                );
                return Ok(Handled::Payload(None));
            }
            RemoteCommand::UploadChunk { id, offset, data } => {
                let upload = self
                    .uploads
                    .get_mut(id)
                    .filter(|u| u.owner == peer.credential)
                    .ok_or_else(|| protocol("Upload is unavailable"))?;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|_| protocol("Invalid upload data"))?;
                if bytes.is_empty()
                    || bytes.len() > 262144
                    || *offset as u64 != upload.offset
                    || bytes.len() as u64 > upload.total - upload.offset
                {
                    return Err(Error::Conflict("Upload offset or length is invalid".into()));
                }
                let mut file = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&upload.path)?;
                file.write_all(&bytes)?;
                upload.hash.update(&bytes);
                upload.offset += bytes.len() as u64;
                upload.touched = Instant::now();
                return Ok(Handled::Payload(None));
            }
            RemoteCommand::FinishUpload {
                id,
                track_id,
                frame,
            } => {
                if !self
                    .uploads
                    .get(id)
                    .is_some_and(|u| u.owner == peer.credential)
                {
                    return Err(protocol("Upload is unavailable"));
                }
                let upload = self.uploads.remove(id).unwrap();
                if upload.offset != upload.total
                    || format!("{:x}", upload.hash.clone().finalize()) != upload.expected
                {
                    return Err(protocol("Upload size or SHA-256 does not match"));
                }
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&upload.path)?
                    .sync_all()?;
                #[cfg(target_os = "macos")]
                if upload.name.to_ascii_lowercase().ends_with(".soundroom.zip") {
                    if peer.role != RemoteRole::Operator {
                        return Err(protocol("Operator role is required to open a project"));
                    }
                    self.require_paused()?;
                    let owned = upload.path.with_extension("project.zip");
                    std::fs::hard_link(&upload.path, &owned)?;
                    return Ok(Handled::Job(JobKind::ImportProject {
                        zip: owned,
                        name: upload.name.clone(),
                        expected: self.authority.revision,
                    }));
                }
                let ready = WaveReader::open(&upload.path).ok().filter(|reader| {
                    reader.info.sample_rate == 48000
                        && reader.info.channels <= 2
                        && reader.info.frames > 0
                });
                let Some(reader) = ready else {
                    // Other formats/rates decode on a worker, never under this lock.
                    // The upload record deletes its file on drop; the job owns a link.
                    #[cfg(target_os = "macos")]
                    let owned = upload.path.with_extension("import");
                    #[cfg(target_os = "macos")]
                    std::fs::hard_link(&upload.path, &owned)?;
                    #[cfg(target_os = "macos")]
                    return Ok(Handled::Job(JobKind::Import {
                        path: owned,
                        name: upload.name.clone(),
                        track_id: track_id.clone(),
                        frame: *frame as u64,
                    }));
                    #[cfg(not(target_os = "macos"))]
                    return Err(protocol(
                        "Portable import currently requires 48 kHz mono/stereo WAV",
                    ));
                };
                let asset = self.place_import(
                    peer,
                    r,
                    candidate,
                    &upload.path,
                    reader.info,
                    &upload.name,
                    track_id.as_deref(),
                    *frame as u64,
                )?;
                return Ok(Handled::Payload(Some(json!({"assetID":asset}))));
            }
            RemoteCommand::ReadArtifact { id, offset, bytes }
            | RemoteCommand::DownloadArtifact { id, offset, bytes } => {
                let a = self
                    .artifacts
                    .get(id)
                    .filter(|a| {
                        a.owner == peer.credential
                            && a.runtime.as_ref().map_or(true, |r| *r == peer.runtime)
                            && a.expires > Instant::now()
                    })
                    .ok_or_else(|| protocol("Artifact is unavailable"))?;
                if *bytes <= 0 || *bytes > 262144 || *offset as u64 > a.bytes {
                    return Err(protocol("Invalid artifact chunk range"));
                }
                let count = (*bytes as u64).min(a.bytes - *offset as u64);
                return Ok(Handled::Job(JobKind::ReadArtifact {
                    path: a.path.clone(),
                    offset: *offset as u64,
                    count: count as usize,
                    total: a.bytes,
                    #[cfg(test)]
                    hook: self.artifact_read_hook.clone(),
                }));
            }
            RemoteCommand::Waveform { .. } => {
                return Ok(Handled::Job(JobKind::Waveform {
                    task: waveform::Task::capture(self, r)?,
                }))
            }
            RemoteCommand::Editor { intent } => {
                let mut handled = self.editor(peer, r, intent);
                // The GUI moves its edit cursor locally and adopts the stopped
                // position from the reply context of an editor transport request.
                if let Ok(Handled::Job(JobKind::Transport { context, .. })) = &mut handled {
                    *context = Some(serde_json::to_value(&intent.context)?);
                }
                return handled;
            }
        };
        self.commit(peer, r, candidate)?;
        Ok(Handled::Payload(None))
    }
    fn install_generated(
        &mut self,
        peer: &Identity,
        r: &RemoteRequest,
        value: &crate::synthesis::Rendered,
    ) -> Result<Value, Error> {
        let info = WaveReader::open(&value.path).map_err(engine)?.info;
        let stored = self
            .media
            .values()
            .map(|m| m.info.frames.saturating_mul(m.info.block_align as u64))
            .sum::<u64>();
        if stored + info.frames * info.block_align as u64 > 8 * 1024 * 1024 * 1024 {
            return Err(protocol("Media storage quota exceeded"));
        }
        let mut candidate = self.authority.session.clone();
        let track = if let Some(id) = &value.track {
            Self::track(&mut candidate, id)?
        } else {
            candidate.tracks.push(Track {
                id: new_id()?,
                name: value.name.clone(),
                ..Track::default()
            });
            candidate.tracks.last_mut().unwrap()
        };
        let asset = new_id()?;
        let target = self.root.join("Media").join(format!("{asset}.wav"));
        let clip_id = value.clip.clone().unwrap_or(new_id()?);
        let mut clip = Clip {
            id: clip_id.clone(),
            asset_id: asset.clone(),
            name: value.name.clone(),
            start: value.frame,
            offset: 0,
            length: info.frames,
            edge_fade_frames: Some(EDGE_FADE_FRAMES),
            edge_protection_disabled: None,
            cut_fade_in_frames: None,
            cut_fade_out_frames: None,
            fade_in_frames: None,
            fade_out_frames: None,
            fade_reference_offset: None,
            fade_reference_length: None,
            render_envelope: None,
            previous_states: vec![],
            extra: BTreeMap::new(),
        };
        let mut pattern = value.document.clone();
        normalize_seeds(&mut pattern, false);
        clip.extra.insert(
            if value.designer {
                "soundDesign"
            } else {
                "drumPattern"
            }
            .into(),
            pattern,
        );
        if let Some(id) = &value.clip {
            let previous = track
                .clips
                .iter_mut()
                .find(|c| &c.id == id)
                .ok_or_else(|| Error::Conflict("Instrument clip is unavailable".into()))?;
            let mut state = serde_json::to_value(&*previous)?;
            let object = state.as_object_mut().unwrap();
            object.remove("start");
            object.remove("previousStates");
            object.insert("id".into(), json!(new_id()?));
            object.insert(
                "operation".into(),
                json!(if value.designer {
                    "Edit sound design"
                } else {
                    "Edit drum pattern"
                }),
            );
            let seconds = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| protocol("System clock is invalid"))?
                .as_secs_f64()
                - 978307200.;
            object.insert("createdAt".into(), json!(seconds));
            clip.previous_states = previous.previous_states.clone();
            clip.previous_states
                .insert(0, serde_json::from_value(state)?);
            clip.previous_states.truncate(5);
            clip.start = previous.start;
            clip = if value.designer {
                crate::synthesis::designer_replacement(previous, clip)
            } else {
                crate::synthesis::drum_replacement(previous, clip)
            };
            *previous = clip;
        } else {
            track.clips.push(clip);
        }
        std::fs::hard_link(&value.path, &target)?;
        self.media.insert(
            asset.clone(),
            Media {
                path: target.clone(),
                info,
            },
        );
        if let Err(error) = self.commit(peer, r, candidate) {
            self.media.remove(&asset);
            let _ = std::fs::remove_file(target);
            return Err(error);
        }
        Ok(json!({"assetID":asset,"clipID":clip_id}))
    }
    fn projects(&self) -> Result<Vec<String>, Error> {
        let mut names = vec![];
        for entry in std::fs::read_dir(self.root.join("Projects"))? {
            let e = entry?;
            let name = e.file_name().to_string_lossy().into_owned();
            if e.file_type()?.is_dir()
                && !e.file_type()?.is_symlink()
                && name.ends_with(".soundroom")
                && safe_filename(&name)
            {
                names.push(name);
            }
            if names.len() > 1024 {
                return Err(protocol("Project list exceeds its bound"));
            }
        }
        names.sort();
        Ok(names)
    }
    fn editor(
        &mut self,
        peer: &Identity,
        r: &RemoteRequest,
        intent: &soundroom_client::models::RemoteEditorIntent,
    ) -> Result<Handled, Error> {
        if intent.flags.get("separateSource") == Some(&true)
            && matches!(
                intent.operation.as_str(),
                "effects.apply" | "effects.preview"
            )
        {
            let id = intent
                .context
                .track_id
                .as_ref()
                .ok_or_else(|| protocol("A source track is required"))?;
            let expected = Self::expected(r);
            if expected.get(id) != self.authority.track_revisions.get(id)
                || !expected.contains_key(id)
            {
                return Err(Error::Conflict(
                    "The source changed before separation; reopen the current audio".into(),
                ));
            }
        }
        match intent.operation.as_str() {
            "sources.status" => Ok(Handled::Payload(Some(separation::status(&self.root)))),
            "client.features" => {
                let payload = intent
                    .payload
                    .as_ref()
                    .ok_or_else(|| protocol("Client features payload is required"))?;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(payload)
                    .map_err(|_| protocol("Invalid features payload"))?;
                let features: Vec<String> = serde_json::from_slice(&bytes)?;
                if features.len() > 16
                    || features
                        .iter()
                        .any(|feature| feature != "compact-control-patches")
                {
                    return Err(protocol("Unsupported client features"));
                }
                Ok(Handled::Features(!features.is_empty()))
            }
            "transport.play" => self.play_from_cursor(peer, intent),
            "transport.pause" | "transport.stop" => {
                self.transport_job(peer, crate::audio::Command::Pause)
            }
            "transport.toggle" => {
                if matches!(
                    self.authority.transport,
                    soundroom_engine::authority::Transport::Playing { .. }
                ) {
                    self.transport_job(peer, crate::audio::Command::Pause)
                } else {
                    self.play_from_cursor(peer, intent)
                }
            }
            "drums.kits" => Ok(Handled::Payload(Some(json!(
                soundroom_engine::custom_kits::list()
            )))),
            "drums.saveKit" | "drums.deleteKit" => {
                if peer.role == RemoteRole::Viewer {
                    return Err(protocol("Editor role is required"));
                }
                if intent.operation == "drums.deleteKit" {
                    let id = intent
                        .strings
                        .get("id")
                        .ok_or_else(|| protocol("Kit id is required"))?;
                    soundroom_engine::custom_kits::delete(&id.to_uppercase()).map_err(engine)?;
                    return Ok(Handled::Payload(Some(json!(
                        soundroom_engine::custom_kits::list()
                    ))));
                }
                use base64::Engine as _;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(
                        intent
                            .payload
                            .as_ref()
                            .ok_or_else(|| protocol("Kit design is required"))?,
                    )
                    .map_err(|_| protocol("Invalid kit design"))?;
                let value: Value = serde_json::from_slice(&bytes)?;
                soundroom_client::validation::validate_model(&value, "DrumKitDesign")?;
                let mut design: soundroom_engine::custom_kits::KitDesign =
                    serde_json::from_value(value)?;
                design.id = design.id.to_uppercase();
                design.name = design.name.trim().to_string();
                design.validate().map_err(engine)?;
                Ok(Handled::Job(JobKind::KitExport {
                    design: Box::new(design),
                }))
            }
            "effects.apply" => self.begin_effect(intent),
            "pitch.apply" => self.begin_bake(intent),
            "drums.preview"
            | "designer.preview"
            | "take.preview"
            | "audition.stop"
            | "designer.stop"
            | "drums.stop"
            | "take.stop"
            | "designer.configure"
            | "designer.noteOn"
            | "designer.noteOff"
            | "designer.allNotesOff"
            | "designer.controller"
            | "designer.midiConnect"
            | "designer.midiSources" => self.preview_editor(peer, intent),
            "effects.preview" => {
                self.claim_file_preview(peer, preview::Kind::Effects)?;
                let mut handled = self.begin_effect(intent);
                if let Ok(Handled::Job(JobKind::Process { task })) = &mut handled {
                    let (sequence, cancel) = self.next_preview();
                    task.preview = Some((peer.runtime.clone(), sequence, cancel));
                } else {
                    self.stop_previews();
                }
                handled
            }
            "transport.audition" => {
                let track = intent
                    .strings
                    .get("trackID")
                    .or(intent.context.track_id.as_ref())
                    .cloned();
                if matches!(
                    self.authority.transport,
                    soundroom_engine::authority::Transport::Playing { .. }
                ) {
                    self.transport_job(peer, crate::audio::Command::Pause)
                } else {
                    self.solo_audition = track;
                    self.audition_request = true;
                    let result = self.play_from_cursor(peer, intent);
                    self.audition_request = false;
                    if result.is_err() {
                        self.solo_audition = None;
                    }
                    result
                }
            }
            "pitch.analyze" | "pitch.render" | "pitch.targets" => self.begin_pitch(peer, intent),
            "pitch.manualApply" => self.begin_manual_pitch(intent),
            "clips.combine" => self.begin_combine(peer, intent),
            "designer.commit" | "drums.commit" | "drums.export" => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(
                        intent
                            .payload
                            .as_ref()
                            .ok_or_else(|| protocol("Instrument pattern is required"))?,
                    )
                    .map_err(|_| protocol("Invalid instrument pattern"))?;
                let document: Value = serde_json::from_slice(&bytes)?;
                let designer = intent.operation == "designer.commit";
                validation::validate_model(
                    &document,
                    if designer {
                        "SoundDesignPattern"
                    } else {
                        "DrumPattern"
                    },
                )?;
                if !designer && document["steps"].as_array().is_some_and(|a| a.len() > 4096) {
                    return Err(protocol("Drummer has too many steps"));
                }
                let target =
                    |name: &str, fallback: Option<&String>| -> Result<Option<String>, Error> {
                        let value = intent
                            .strings
                            .get(name)
                            .filter(|s| !s.is_empty())
                            .or(fallback);
                        if value.is_some_and(|s| !validation::uuid(s)) {
                            return Err(protocol("Invalid synthesis target UUID"));
                        }
                        Ok(value.map(|s| s.to_ascii_uppercase()))
                    };
                let track = target("trackID", intent.context.track_id.as_ref())?;
                let clip = target("clipID", intent.context.clip_id.as_ref())?;
                if designer
                    && clip.is_none()
                    && document["notes"].as_array().map_or(true, Vec::is_empty)
                {
                    return Err(protocol("Designer requires notes for a new clip"));
                }
                if clip.is_some() && track.is_none() {
                    return Err(protocol("A clip rewrite requires its track"));
                }
                let frame = *intent
                    .integers
                    .get(if designer { "frame" } else { "start" })
                    .unwrap_or(&intent.context.cursor_frame);
                if frame < 0 || frame as u64 > soundroom_engine::model::MAX_FRAME {
                    return Err(protocol("Invalid instrument timeline position"));
                }
                let export = intent.operation == "drums.export";
                if !export {
                    if let Some(id) = &track {
                        let current =
                            self.authority.track_revisions.get(id).ok_or_else(|| {
                                Error::Conflict("Target track is unavailable".into())
                            })?;
                        if Self::expected(r).get(id) != Some(current) {
                            return Err(Error::Conflict("Stale instrument track".into()));
                        }
                    } else if r.expected_revision != Some(self.authority.revision as i64) {
                        return Err(Error::Conflict("Stale track structure revision".into()));
                    }
                }
                let name = if designer {
                    "Sound design".into()
                } else {
                    document["name"]
                        .as_str()
                        .filter(|s| valid_name(s))
                        .unwrap_or("Drum pattern")
                        .into()
                };
                Ok(Handled::Job(JobKind::Synthesize {
                    task: Box::new(crate::synthesis::Task {
                        document,
                        designer,
                        track,
                        clip,
                        frame: frame as u64,
                        export,
                        path: self
                            .root
                            .join("Artifacts")
                            .join(format!("{}.wav", new_id()?)),
                        name,
                    }),
                }))
            }
            "session.rename" => {
                if r.expected_revision != Some(self.authority.revision as i64) {
                    return Err(Error::Conflict(
                        "The session changed; reopen Rename Session".into(),
                    ));
                }
                let mut candidate = self.authority.session.clone();
                candidate.name = names::validated(
                    intent
                        .strings
                        .get("name")
                        .ok_or_else(|| protocol("A session name is required"))?,
                )?;
                self.commit(peer, r, candidate)?;
                Ok(Handled::Payload(None))
            }
            "session.timing" => {
                if r.expected_revision != Some(self.authority.revision as i64) {
                    return Err(Error::Conflict("Stale tempo/meter revision".into()));
                }
                let mut candidate = self.authority.session.clone();
                if let Some(tempo) = intent.numbers.get("tempo") {
                    candidate.timing.tempo = *tempo;
                }
                if let Some(beats) = intent.integers.get("beatsPerBar") {
                    candidate.timing.beats_per_bar =
                        (*beats).try_into().map_err(|_| protocol("Invalid meter"))?;
                }
                if let Some(unit) = intent.integers.get("beatUnit") {
                    candidate.timing.beat_unit = (*unit)
                        .try_into()
                        .map_err(|_| protocol("Invalid beat unit"))?;
                }
                candidate.version = candidate.version.max(9);
                self.commit(peer, r, candidate)?;
                Ok(Handled::Payload(None))
            }
            "server.webClients" => {
                if peer.role != RemoteRole::Operator {
                    return Err(protocol("Operator role is required"));
                }
                Ok(Handled::Payload(Some(self.registry.clients())))
            }
            "server.webPairing" => {
                if peer.role != RemoteRole::Operator {
                    return Err(protocol("Operator role is required"));
                }
                let name = intent
                    .strings
                    .get("name")
                    .ok_or_else(|| protocol("Approved name is required"))?;
                let role: RemoteRole =
                    serde_json::from_value(json!(intent
                        .strings
                        .get("role")
                        .ok_or_else(|| protocol("Approved role is required"))?))?;
                let seconds = intent
                    .integers
                    .get("expiresInSeconds")
                    .copied()
                    .unwrap_or(120);
                let seconds = u64::try_from(seconds)
                    .map_err(|_| protocol("Invalid pairing code lifetime"))?;
                Ok(Handled::Payload(Some(
                    self.registry.grant_with_lifetime(name, role, seconds)?,
                )))
            }
            "server.revokeWebCredential" => {
                if peer.role != RemoteRole::Operator {
                    return Err(protocol("Operator role is required"));
                }
                let id = intent
                    .strings
                    .get("credentialID")
                    .ok_or_else(|| protocol("Credential ID is required"))?;
                if id.eq_ignore_ascii_case(&peer.credential) {
                    return Err(protocol(
                        "This connection's credential cannot be revoked from Browser access",
                    ));
                }
                if self
                    .protected_credential
                    .as_ref()
                    .is_some_and(|p| p.eq_ignore_ascii_case(id))
                {
                    return Err(protocol("The Soundroom GUI's credential cannot be revoked"));
                }
                self.registry.revoke(id)?;
                self.release_write_owner(
                    |owner| owner.credential.eq_ignore_ascii_case(id),
                    soundroom_engine::automation_write::ReleaseReason::Revoked,
                );
                for (key, job) in &self.pending {
                    if key.0.eq_ignore_ascii_case(id) {
                        job.cancel.store(true, Ordering::Release);
                    }
                }
                for p in self
                    .peers
                    .values()
                    .filter(|p| p.identity.credential.eq_ignore_ascii_case(id))
                {
                    p.out.close();
                }
                Ok(Handled::Payload(None))
            }
            "jobs.cancel" => {
                let id = intent.strings.get("id").map(|s| s.to_ascii_uppercase());
                if id.as_ref().is_some_and(|id| !validation::uuid(id)) {
                    return Err(protocol("Invalid cancellation request ID"));
                }
                let mut count = 0;
                for (key, pending) in &self.pending {
                    let allowed = peer.role == RemoteRole::Operator
                        || (key.0 == peer.credential && key.1 == peer.runtime);
                    if allowed && id.as_ref().map_or(true, |id| &key.2 == id) {
                        pending.cancel.store(true, Ordering::Release);
                        count += 1;
                    }
                }
                if count == 0 {
                    if let Some(id) = id {
                        let key = (peer.credential.clone(), peer.runtime.clone(), id);
                        if self.receipts.iter().any(|receipt| receipt.key == key) {
                            return Err(protocol("The requested job already completed"));
                        }
                        if !self.cancelled_requests.contains(&key)
                            && (self.cancelled_requests.len() >= 4096
                                || self
                                    .cancelled_requests
                                    .iter()
                                    .filter(|key| key.0 == peer.credential && key.1 == peer.runtime)
                                    .count()
                                    >= 128)
                        {
                            return Err(Error::QueueFull);
                        }
                        self.cancelled_requests.insert(key);
                        return Ok(Handled::Payload(None));
                    }
                    return Err(protocol("No owned cancellable job is available"));
                }
                Ok(Handled::Payload(None))
            }
            "transport.record" => self.begin_recording(peer, Some(intent)),
            "transport.finishRecording" => {
                if self.recording.is_none() && self.recording_preparing.is_none() {
                    return Err(Error::Conflict("Nothing is recording".into()));
                }
                self.transport_job(peer, crate::audio::Command::Pause)
            }
            "transport.metronome" => {
                // A monitoring preference, not a transport command: any
                // operator may switch it while another owns the transport.
                if peer.role != RemoteRole::Operator {
                    return Err(protocol("Operator role is required"));
                }
                if let Some(backend) = &self.backend {
                    backend.set_metronome(intent.flags.get("enabled") == Some(&true));
                }
                Ok(Handled::Payload(None))
            }
            "devices.inputVolume" => self.input_volume(peer, intent),
            "recording.settings" => {
                self.require_transport_owner(peer, false)?;
                self.input_monitoring = intent.flags.get("inputMonitoring") == Some(&true);
                if let Some(flag) = self.recording.as_ref().and_then(|r| r.monitoring.as_ref()) {
                    flag.store(self.input_monitoring, Ordering::Release);
                }
                Ok(Handled::Payload(None))
            }
            "meters.subscribe" => {
                let ids: Vec<String> = match &intent.payload {
                    Some(payload) => serde_json::from_slice(
                        &base64::engine::general_purpose::STANDARD
                            .decode(payload)
                            .map_err(|_| protocol("Invalid meter subscription"))?,
                    )?,
                    None => vec![],
                };
                if ids.len() > 32 {
                    return Err(protocol("At most 32 effect meters"));
                }
                if ids.iter().any(|id| !validation::uuid(id)) {
                    return Err(protocol("Invalid effect meter ID"));
                }
                self.effect_meters.insert(peer.runtime.clone(), ids);
                Ok(Handled::Payload(None))
            }
            "media.cleanup" => {
                if peer.role != RemoteRole::Operator {
                    return Err(protocol("Operator role is required"));
                }
                let used = self.authority.referenced_asset_ids();
                let before = self.media.len();
                let unused: Vec<String> = self
                    .media
                    .keys()
                    .filter(|id| !used.contains(&id.to_uppercase()))
                    .cloned()
                    .collect();
                for id in &unused {
                    if let Some(media) = self.media.get(id) {
                        custody::remove(&self.root, &media.path)?;
                        self.media.remove(id);
                    }
                }
                self.autosave_now()?;
                let released = before - self.media.len();
                Ok(Handled::Payload(Some(json!({"released": released}))))
            }
            "devices.refresh" => {
                let event = self.device_event();
                self.broadcast(&event, false);
                Ok(Handled::Payload(None))
            }
            "server.shutdown" => {
                if peer.role != RemoteRole::Operator {
                    return Err(protocol("Only an operator may stop the server"));
                }
                if self.pending.values().any(|p| p.authored) {
                    return Err(Error::Conflict(
                        "Finish pending work before stopping the server".into(),
                    ));
                }
                let stop = self
                    .shutdown
                    .clone()
                    .ok_or_else(|| protocol("This host does not expose process shutdown"))?;
                self.require_paused()?;
                self.autosave_now()?;
                self.shutdown_requested = true;
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(300));
                    stop.store(true, Ordering::Release);
                });
                Ok(Handled::Payload(None))
            }
            "project.new" => {
                if peer.role != RemoteRole::Operator {
                    return Err(protocol("Operator role is required to start a new project"));
                }
                self.require_paused()?;
                if r.expected_revision
                    .is_some_and(|v| v != self.authority.revision as i64)
                {
                    return Err(Error::Conflict("Stale project revision".into()));
                }
                let revision = self
                    .authority
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| Error::Conflict("Revision exhausted".into()))?;
                let project_id = new_id()?;
                let mut authority = Authority::new(Session::default()).map_err(engine)?;
                authority.revision = revision;
                for version in authority.track_revisions.values_mut() {
                    *version = revision;
                }
                self.authority = authority;
                self.project_name = None;
                self.project_id = project_id;
                self.media.clear();
                if let Some(backend) = &self.backend {
                    backend.observe_revision(revision);
                }
                self.history_events();
                let input = self.capture_snapshot();
                self.schedule_audio_update(&input);
                if self
                    .publication
                    .enqueue(Publication {
                        input,
                        before: None,
                        prepared: None,
                        compact: None,
                        targets: self
                            .peers
                            .values()
                            .map(|peer| {
                                (
                                    format!("{:p}", Arc::as_ptr(&peer.out.closed)),
                                    peer.out.clone(),
                                )
                            })
                            .collect(),
                    })
                    .is_err()
                {
                    for peer in self.peers.values() {
                        peer.out.close();
                    }
                }
                Ok(Handled::Payload(None))
            }
            operation if writes::OPERATIONS.contains(&operation) => {
                self.automation_write(peer, r, intent)
            }
            operation if master::OPERATIONS.contains(&operation) => {
                self.master_editor(peer, r, intent)
            }
            #[cfg(target_os = "macos")]
            "project.package" => {
                if peer.role != RemoteRole::Operator {
                    return Err(protocol("Operator role is required to save a project"));
                }
                if let Some(id) = intent.strings.get("release") {
                    let artifact = self
                        .artifacts
                        .get(id)
                        .filter(|artifact| {
                            artifact.owner == peer.credential
                                && artifact
                                    .runtime
                                    .as_ref()
                                    .is_some_and(|runtime| *runtime == peer.runtime)
                        })
                        .ok_or_else(|| protocol("The project download is unavailable"))?;
                    let _ = std::fs::remove_file(&artifact.path);
                    self.artifacts.remove(id);
                    return Ok(Handled::Payload(None));
                }
                self.require_editable()?;
                if r.expected_revision
                    .is_some_and(|revision| revision != self.authority.revision as i64)
                {
                    return Err(Error::Conflict(
                        "The session changed before saving. Retry Save.".into(),
                    ));
                }
                Ok(Handled::Job(JobKind::Package {
                    project: Box::new(Project {
                        session: self.authority.session.clone(),
                        media: self.media.clone(),
                    }),
                }))
            }
            "project.export" => {
                let name = intent
                    .strings
                    .get("name")
                    .map(String::as_str)
                    .unwrap_or("Mix");
                if !safe_filename(name) || intent.strings.get("format").is_some_and(|f| f != "wav")
                {
                    return Err(protocol("Portable export requires WAV and a valid name"));
                }
                let (start, end) = if intent.flags.get("selection") == Some(&true) {
                    (
                        intent
                            .context
                            .range_start
                            .ok_or_else(|| protocol("Selection start is required"))?
                            as u64,
                        intent
                            .context
                            .range_end
                            .ok_or_else(|| protocol("Selection end is required"))?
                            as u64,
                    )
                } else {
                    (0, self.authority.session.end())
                };
                if end <= start || end > self.authority.session.end() {
                    return Err(protocol("Invalid export selection"));
                }
                let stems = if intent.flags.get("tracks") == Some(&true) {
                    let ids = if let Some(payload) = &intent.payload {
                        let bytes = base64::engine::general_purpose::STANDARD
                            .decode(payload)
                            .map_err(|_| protocol("Invalid selected tracks"))?;
                        serde_json::from_slice::<Vec<String>>(&bytes)?
                    } else {
                        self.authority
                            .session
                            .tracks
                            .iter()
                            .map(|t| t.id.clone())
                            .collect()
                    };
                    let canonical = ids
                        .iter()
                        .map(|id| id.to_ascii_uppercase())
                        .collect::<std::collections::BTreeSet<_>>();
                    if ids.is_empty()
                        || ids.len() > 16
                        || canonical.len() != ids.len()
                        || ids.iter().any(|id| {
                            !self
                                .authority
                                .session
                                .tracks
                                .iter()
                                .any(|t| t.id.eq_ignore_ascii_case(id))
                        })
                    {
                        return Err(protocol(
                            "Stem export requires one to sixteen distinct available tracks",
                        ));
                    }
                    Some(canonical.into_iter().collect())
                } else {
                    None
                };
                Ok(Handled::Job(JobKind::Export {
                    project: Box::new(Project {
                        session: self.authority.session.clone(),
                        media: self.media.clone(),
                    }),
                    path: self
                        .root
                        .join("Artifacts")
                        .join(format!("{}.wav", new_id()?)),
                    name: format!("{}.wav", name.trim_end_matches(".wav")),
                    start,
                    end,
                    tails: *intent.flags.get("tails").unwrap_or(&true),
                    stems,
                }))
            }
            _ => Err(protocol(
                "This editor operation is not implemented by the portable service",
            )),
        }
    }
}
enum Handled {
    Payload(Option<Value>),
    Features(bool),
    Snapshot,
    Job(JobKind),
}
fn safe_filename(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && !name.contains(['/', '\\', ':'])
        && !name.chars().any(char::is_control)
        && name.trim() == name
}
fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
#[cfg(test)]
thread_local! { static TEST_HASHED_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
fn hash_file(path: &Path) -> Result<(u64, String), Error> {
    let mut f = File::open(path)?;
    let count = f.metadata()?.len();
    let mut hasher = Sha256::new();
    let mut bytes = [0; 65536];
    loop {
        let n = f.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        hasher.update(&bytes[..n]);
        #[cfg(test)]
        TEST_HASHED_BYTES.with(|count| count.set(count.get() + n as u64));
    }
    Ok((count, format!("{:x}", hasher.finalize())))
}
/// Opens a project directory and installs it as the authoritative session.
fn open_project(
    shared: &Shared,
    peer: &Identity,
    cancel: &AtomicBool,
    path: PathBuf,
    expected: u64,
) -> Result<Option<Value>, Error> {
    let mut project = Project::open(&path).map_err(engine)?;
    names::recover_title(&mut project.session, &path);
    // Another project may already have custody audio under one of this
    // project's asset IDs (a copied project re-rendered, for example).
    let root = shared
        .lock()
        .map_err(|_| protocol("Authority is unavailable"))?
        .root
        .clone();
    let (_, verified_media) = custody::rekey_conflicts(&root, &mut project)?;
    if cancel.load(Ordering::Acquire) {
        return Err(protocol("Job cancelled"));
    }
    let mut authority =
        Authority::new(normalize_loaded_session(project.session)?).map_err(engine)?;
    let state = shared
        .lock()
        .map_err(|_| protocol("Authority is unavailable"))?;
    if !state.registry.contains(&peer.credential) {
        return Err(protocol(
            "Credential was revoked before opening the project",
        ));
    }
    state.require_paused()?;
    if state.authority.revision != expected {
        return Err(Error::Conflict(
            "Session changed while opening project".into(),
        ));
    }
    let revision = state
        .authority
        .revision
        .checked_add(1)
        .filter(|v| *v <= soundroom_engine::model::MAX_FRAME)
        .ok_or_else(|| Error::Conflict("Revision exhausted".into()))?;
    authority.revision = revision;
    for version in authority.track_revisions.values_mut() {
        *version = revision;
    }
    let mut input = state.capture_snapshot_for(
        &authority.session,
        revision,
        &authority.track_revisions,
        &project.media,
    );
    let project_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    let project_id = new_id()?;
    input.project_name = project_name.clone();
    input.project_id = project_id.clone();
    let global = state.global.clone();
    let root = state.root.clone();
    drop(state);
    let media = custody::Staged::load_verified(&root, project.media, verified_media)?;
    // Large model traversal/encoding never holds the serialized control owner.
    let document = SnapshotDocument::prepare(input.value()?, global)?;
    let mut state = shared
        .lock()
        .map_err(|_| protocol("Authority is unavailable"))?;
    if cancel.load(Ordering::Acquire) || !state.registry.contains(&peer.credential) {
        return Err(protocol("Open was cancelled or its credential was revoked"));
    }
    state.require_paused()?;
    if state.authority.revision != expected {
        return Err(Error::Conflict(
            "Session changed while preparing the open document".into(),
        ));
    }
    let media = media.commit(&root)?;
    state.cancel_waveforms(None);
    state.authority = authority;
    state.project_name = project_name.clone();
    state.project_id = project_id.clone();
    if let Some(backend) = &state.backend {
        backend.observe_revision(revision);
    }
    state.media = media;
    let input = state.capture_snapshot();
    state.history_events();
    if state
        .publication
        .enqueue(Publication {
            input,
            before: None,
            prepared: Some(document),
            compact: None,
            targets: state
                .peers
                .values()
                .map(|peer| {
                    (
                        format!("{:p}", Arc::as_ptr(&peer.out.closed)),
                        peer.out.clone(),
                    )
                })
                .collect(),
        })
        .is_err()
    {
        // Authority already committed. A publication failure requires refresh;
        // it must not turn this completed authored request into a rejected receipt.
        for peer in state.peers.values() {
            peer.out.close();
        }
    }
    Ok(Some(
        json!({"projectName":project_name,"projectID":project_id}),
    ))
}
/// Unpacks a ditto-made `.soundroom.zip` into Projects/ under a fresh name.
#[cfg(target_os = "macos")]
fn unpack_project(shared: &Shared, zip: &Path, name: &str) -> Result<PathBuf, Error> {
    struct Cleanup(Vec<PathBuf>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for path in &self.0 {
                let _ = std::fs::remove_file(path);
                let _ = std::fs::remove_dir_all(path);
            }
        }
    }
    let root = shared
        .lock()
        .map_err(|_| protocol("Authority is unavailable"))?
        .root
        .clone();
    let staging = root.join("Artifacts").join(format!("{}.unpack", new_id()?));
    let _cleanup = Cleanup(vec![zip.to_path_buf(), staging.clone()]);
    std::fs::create_dir_all(&staging)?;
    let status = std::process::Command::new("/usr/bin/ditto")
        .args(["-x", "-k"])
        .arg(zip)
        .arg(&staging)
        .status()?;
    if !status.success() {
        return Err(protocol("The project archive could not be unpacked"));
    }
    let mut packages = vec![];
    for entry in std::fs::read_dir(&staging)? {
        let entry = entry?;
        let file = entry.file_name().to_string_lossy().into_owned();
        if file.starts_with("__MACOSX") {
            continue;
        }
        if !entry.file_type()?.is_dir() || !file.ends_with(".soundroom") {
            return Err(protocol("The archive must contain one Soundroom project"));
        }
        packages.push(entry.path());
    }
    let [package] = packages.as_slice() else {
        return Err(protocol("The archive must contain one Soundroom project"));
    };
    // Reject links anywhere in the package; the project must be self-contained.
    let mut pending = vec![package.clone()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                return Err(protocol("Project archives may not contain links"));
            }
            if kind.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    let stem: String = name
        .trim_end_matches(".zip")
        .trim_end_matches(".soundroom")
        .chars()
        .filter(|c| !matches!(c, '/' | ':' | '\\') && !c.is_control())
        .take(100)
        .collect();
    let stem = if stem.trim().is_empty() {
        "Imported project".to_string()
    } else {
        stem
    };
    let projects = root.join("Projects");
    std::fs::create_dir_all(&projects)?;
    let mut target = projects.join(format!("{stem}.soundroom"));
    let mut n = 2;
    while target.exists() {
        target = projects.join(format!("{stem} {n}.soundroom"));
        n += 1;
    }
    std::fs::rename(package, &target)?;
    Ok(target)
}
fn run_job(shared: Shared, out: Outbox, job: Job, digest: [u8; 32]) {
    let starting_record = matches!(job.kind, JobKind::Record { .. });
    let protected_read = matches!(
        job.kind,
        JobKind::Waveform { .. } | JobKind::ReadArtifact { .. }
    );
    let begin = match &job.kind {
        JobKind::Write { task } => task.begin_gesture(),
        _ => None,
    };
    // A transport reply follows the transport event it caused: the GUI adopts
    // the reply's cursor only once it has seen the stop.
    let ordered = matches!(job.kind, JobKind::Transport { .. });
    let last_progress = std::cell::Cell::new(Instant::now());
    let progress = |done: u64, total: u64| {
        if done != total && last_progress.get().elapsed() < Duration::from_millis(250) {
            return;
        }
        last_progress.set(Instant::now());
        let _=out.send(&json!({"version":3,"kind":"event","event":{"type":"job","id":job.request.id,"progress":done as f64/total.max(1) as f64,"message":"Rendering"}}),false);
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Option<Value>, Error> {
        // Once capture has been handed to Finish, it must drain and retain the
        // take even if its requesting peer cancels or disconnects.
        if job.cancel.load(Ordering::Acquire) && !matches!(job.kind, JobKind::FinishRecording { .. }) {
            return Err(protocol("Job cancelled"));
        }
        match job.kind {
            JobKind::Waveform { task } => task.run(&shared, &job.peer, &job.cancel),
            JobKind::Recover { task } => task.run(&shared, &job.peer, &job.request, &job.cancel),
            JobKind::Edit { task } => task.run(&shared, &job.peer, &job.request, &job.cancel),
            JobKind::Transport{backend,command,project,revision,start,context}=>{
                if let Some(frame) = start {
                    backend.execute(crate::audio::Command::Seek(frame), Box::new(Project { session: project.session.clone(), media: project.media.clone() }), revision, job.cancel.clone())?;
                }
                let telemetry=backend.execute(command,project,revision,job.cancel.clone())?;
                let mut state=shared.lock().map_err(|_|protocol("Authority is unavailable"))?;state.audio_telemetry(telemetry);
                Ok(context.map(|mut context| {
                    if !telemetry.playing {
                        context["cursorFrame"] = json!(telemetry.frame);
                    }
                    context
                }))
            }
            JobKind::Write { task } => task.run(&shared, &job.peer, &job.cancel),
            JobKind::Record { plan, backend, project, revision, target, playback, count_in } => {
                let generation = std::sync::atomic::AtomicU64::new(0);
                let result = (|| {
                    backend.execute_recording(crate::audio::Command::Seek(playback), Box::new(Project { session: project.session.clone(), media: project.media.clone() }), revision, job.cancel.clone(), &generation)?;
                    let count_in_seconds = count_in.len() as f64 / 48000.;
                    if plan.metronome {
                        backend.metronome_until_stop();
                    }
                    if !count_in.is_empty() {
                        // The callback starts the clicks and holds the graph for exactly their length.
                        backend.arm_count_in(count_in);
                    }
                    let telemetry = backend.execute_recording(crate::audio::Command::Play, project, revision, job.cancel.clone(), &generation)?;
                    // Capture aligns to callback clock anchors, which resume when the
                    // held graph starts after the count-in.
                    let deadline = Instant::now() + Duration::from_secs_f64(count_in_seconds + 5.);
                    while backend.output_anchor().is_none() && Instant::now() < deadline {
                        if job.cancel.load(Ordering::Acquire) { return Err(protocol("Recording startup was stopped")); }
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    if job.cancel.load(Ordering::Acquire) || backend.output_anchor().is_none()
                        || !backend.telemetry().is_some_and(|t| t.playing) {
                        return Err(protocol("Recording startup has no active callback clock"));
                    }
                    {
                        let state = shared.lock().map_err(|_| protocol("Authority is unavailable"))?;
                        if !state.registry.authorizes(&job.peer.credential, &job.peer.role)
                            || state.authority.revision != revision {
                            return Err(Error::Conflict("Recording source or credential changed during startup".into()));
                        }
                    }
                    let start = target;
                    match record::start(*plan, backend.clone(), start, job.peer.clone(), job.cancel.clone()) {
                        Ok(active) => {
                            let mut state = shared.lock().map_err(|_| protocol("Authority is unavailable"))?;
                            if job.cancel.load(Ordering::Acquire) || !backend.telemetry().is_some_and(|t| t.playing)
                                || !state.registry.authorizes(&job.peer.credential, &job.peer.role)
                                || state.authority.revision != revision || state.recording.is_some() {
                                drop(state);
                                let _ = active.finish();
                                return Err(protocol("Recording startup was stopped or its source changed"));
                            }
                            state.recording_preparing = None;
                            state.recording = Some(active);
                            state.audio_telemetry(backend.telemetry().unwrap_or(telemetry));
                            state.broadcast_transport();
                            Ok(None)
                        }
                        Err(error) => Err(error),
                    }
                })();
                if result.is_err() {
                    // Failed/cancelled startup must also release armed clicks and
                    // leave the callback stopped. The preparation reservation
                    // remains held until this cleanup completes.
                    let _ = backend.pause_if_current(generation.load(Ordering::Acquire));
                }
                result
            }
            JobKind::FinishRecording { active, backend } => {
                record::finish(&shared, active, backend).map(Some)
            }
            #[cfg(target_os = "macos")]
            JobKind::Import { path, name, track_id, frame } => {
                let decoded = path.with_extension("decoded.wav");
                struct Cleanup(Vec<PathBuf>);
                impl Drop for Cleanup {
                    fn drop(&mut self) {
                        for path in &self.0 {
                            let _ = std::fs::remove_file(path);
                        }
                    }
                }
                // AudioToolbox infers some containers (AAC/M4A) from the extension.
                let extension = Path::new(&name)
                    .extension()
                    .and_then(|e| e.to_str())
                    .filter(|e| e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric()))
                    .unwrap_or("audio")
                    .to_ascii_lowercase();
                let hinted = path.with_extension(format!("source.{extension}"));
                let _cleanup = Cleanup(vec![path.clone(), decoded.clone(), hinted.clone()]);
                std::fs::hard_link(&path, &hinted)?;
                soundroom_engine::media_decode::decode_to_wav(&hinted, &decoded, &job.cancel)
                    .map_err(engine)?;
                let info = WaveReader::open(&decoded).map_err(engine)?.info;
                let mut state = shared
                    .lock()
                    .map_err(|_| protocol("Authority is unavailable"))?;
                let before = state.authority.session.clone();
                let revision = state.authority.revision;
                let asset = state.place_import(
                    &job.peer,
                    &job.request,
                    before.clone(),
                    &decoded,
                    info,
                    &name,
                    track_id.as_deref(),
                    frame,
                )?;
                if state.publish_changes(before, revision).is_err() {
                    for peer in state.peers.values() {
                        peer.out.close();
                    }
                }
                if revision != state.authority.revision {
                    state.history_events();
                }
                Ok(Some(json!({"assetID":asset})))
            }
            JobKind::KitExport { design } => {
                let hits = soundroom_engine::custom_kits::export(&design, &job.cancel).map_err(engine)?;
                Ok(Some(json!({"id": design.id, "hits": hits, "kits": soundroom_engine::custom_kits::list()})))
            }
            JobKind::Preview { task } => {
                let (owner, kind) = (task.owner.clone(), task.kind);
                let result = task.render(&job.cancel);
                let mut state = shared.lock().map_err(|_| protocol("Authority is unavailable"))?;
                match result {
                    Ok((task, buffer)) => {
                        state.play_preview(&task, buffer);
                        Ok(None)
                    }
                    Err(error) => {
                        state.preview_failed(&owner, kind);
                        Err(error)
                    }
                }
            }
            JobKind::Pitch { task } => {
                let preview = task.preview.clone();
                let result = task.run(&job.cancel);
                if let (Some(owner), Err(_)) = (&preview, &result) {
                    if let Ok(mut state) = shared.lock() { state.preview_failed(owner, preview::Kind::Pitch); }
                }
                let analysis = result?;
                let mut state = shared.lock().map_err(|_| protocol("Authority is unavailable"))?;
                if let Some((owner, path)) = analysis.audio.clone() {
                    let task = preview::Task {
                        source: preview::Source::File(path),
                        kind: preview::Kind::Pitch,
                        owner: owner.clone(),
                        path: PathBuf::new(),
                        looping: false,
                        tempo: 120.,
                        sequence: 0,
                        superseded: Default::default(),
                    };
                    match task.render(&job.cancel) {
                        Ok((task, buffer)) => state.play_preview(&task, buffer),
                        Err(_) => state.preview_failed(&owner, preview::Kind::Pitch),
                    }
                }
                let before = state.authority.session.clone();
                let revision = state.authority.revision;
                let reply = state.finish_pitch(&job.peer, analysis)?;
                if revision != state.authority.revision {
                    if state.publish_changes(before, revision).is_err() {
                        for peer in state.peers.values() { peer.out.close(); }
                    }
                    state.history_events();
                }
                Ok(Some(reply))
            }
            JobKind::Process { task } if task.preview.is_some() => {
                let (owner, sequence, _) = task.preview.clone().unwrap();
                let result = task.run(&job.cancel, progress);
                let processed = match result {
                    Ok(processed) => processed,
                    Err(error) => {
                        if let Ok(mut state) = shared.lock() {
                            if state.file_preview_current(&owner, preview::Kind::Effects, sequence) {
                                state.preview_failed(&owner, preview::Kind::Effects);
                            }
                        }
                        return Err(error);
                    }
                };
                let task = preview::Task {
                    source: preview::Source::File(processed.path().to_path_buf()),
                    kind: preview::Kind::Effects,
                    owner: owner.clone(),
                    path: PathBuf::new(),
                    looping: false,
                    tempo: 120.,
                    sequence,
                    superseded: Default::default(),
                };
                // WAV decoding/allocation belongs to the worker. Stop/Cancel
                // retain authority access while a large preview is loading.
                let (task, buffer) = match task.render(&job.cancel) {
                    Ok(rendered) => rendered,
                    Err(error) => {
                        if let Ok(mut state) = shared.lock() {
                            if state.file_preview_current(&owner, preview::Kind::Effects, sequence) {
                                state.preview_failed(&owner, preview::Kind::Effects);
                            }
                        }
                        return Err(error);
                    }
                };
                let mut state = shared.lock().map_err(|_| protocol("Authority is unavailable"))?;
                if let Err(error) = processed.preview_allowed(&state, &job.peer, &job.cancel) {
                    if state.file_preview_current(&owner, preview::Kind::Effects, sequence) {
                        state.preview_failed(&owner, preview::Kind::Effects);
                    }
                    return Err(error);
                }
                if !state.file_preview_current(&owner, preview::Kind::Effects, sequence) {
                    return Err(Error::Conflict("The effect preview was stopped or superseded".into()));
                }
                state.play_preview(&task, buffer);
                Ok(None)
            }
            JobKind::Process { task } => {
                let processed = task.run(&job.cancel, progress)?;
                let mut state = shared.lock().map_err(|_| protocol("Authority is unavailable"))?;
                if job.cancel.load(Ordering::Acquire) { return Err(protocol("Processing cancelled before applying")); }
                let before = state.authority.session.clone();
                let revision = state.authority.revision;
                let result = state.commit_processed(&job.peer, &processed);
                processed.discard();
                let context = result?;
                if state.publish_changes(before, revision).is_err() {
                    for peer in state.peers.values() { peer.out.close(); }
                }
                if revision != state.authority.revision { state.history_events(); }
                Ok(Some(context))
            }
            JobKind::Master { task } => {
                task.run(&shared, &job.peer, &job.request, &job.cancel, progress)
            }
            JobKind::Synthesize { task } => {
                struct Cleanup(PathBuf);
                impl Drop for Cleanup {
                    fn drop(&mut self) {
                        let _ = std::fs::remove_file(&self.0);
                    }
                }
                let mut cleanup = Cleanup(task.path.clone());
                let rendered = task.render(&job.cancel)?;
                if job.cancel.load(Ordering::Acquire) {
                    return Err(protocol("Synthesis cancelled"));
                }
                if rendered.export {
                    let (bytes, hash) = hash_file(&rendered.path)?;
                    let id = new_id()?;
                    let mut state = shared
                        .lock()
                        .map_err(|_| protocol("Authority is unavailable"))?;
                    if state.artifacts.len() >= 32
                        || state.artifacts.values().map(|a| a.bytes).sum::<u64>() + bytes
                            > 4 * 1024 * 1024 * 1024
                    {
                        return Err(Error::QueueFull);
                    }
                    state.artifacts.insert(
                        id.clone(),
                        Artifact {
                            owner: job.peer.credential.clone(),
                            runtime: None,
                            path: rendered.path.clone(),
                            bytes,
                            expires: Instant::now() + Duration::from_secs(3600),
                        },
                    );
                    cleanup.0 = PathBuf::new();
                    Ok(Some(
                        json!({"id":id,"name":format!("{}.wav",rendered.name),"bytes":bytes,"sha256":hash}),
                    ))
                } else {
                    let mut state = shared
                        .lock()
                        .map_err(|_| protocol("Authority is unavailable"))?;
                    let before = state.authority.session.clone();
                    let revision = state.authority.revision;
                    let metadata = state.install_generated(&job.peer, &job.request, &rendered)?;
                    if state.publish_changes(before, revision).is_err() {
                        for peer in state.peers.values() { peer.out.close(); }
                    }
                    if revision != state.authority.revision { state.history_events(); }
                    Ok(Some(metadata))
                }
            }
            JobKind::Export {
                project,
                path,
                name,
                start,
                end,
                tails,
                stems,
            } => {
                let stem_mode = stems.is_some();
                let ids = stems
                    .map(|v| v.into_iter().map(Some).collect::<Vec<_>>())
                    .unwrap_or_else(|| vec![None]);
                if end <= start
                    || (end - start).saturating_add(32 * 48000).saturating_mul(8) > 2_147_483_648
                {
                    return Err(protocol("Export exceeds the portable 2 GiB artifact bound"));
                }
                struct Files(Vec<PathBuf>);
                impl Drop for Files {
                    fn drop(&mut self) {
                        for path in &self.0 {
                            let _ = std::fs::remove_file(path);
                        }
                    }
                }
                let mut staged = Files(vec![]);
                let mut artifacts = vec![];
                let mut metadata = vec![];
                for id in ids {
                    let mut session = project.session.clone();
                    let target = if let Some(id) = &id {
                        session = session.isolated_track(id).map_err(engine)?;
                        path.with_file_name(format!(
                            "{}-{id}.wav",
                            path.file_stem().unwrap().to_string_lossy()
                        ))
                    } else {
                        path.clone()
                    };
                    staged.0.push(target.clone());
                    let document = Project {
                        session,
                        media: project.media.clone(),
                    };
                    soundroom_engine::render::export_with_tails(
                        &document,
                        &target,
                        start,
                        end,
                        tails,
                        &job.cancel,
                        progress,
                    )
                    .map_err(engine)?;
                    if job.cancel.load(Ordering::Acquire) {
                        return Err(protocol("Job cancelled"));
                    }
                    let (bytes, hash) = hash_file(&target)?;
                    let identity = new_id()?;
                    let output_name = if let Some(id) = id {
                        format!("{}-{}.wav", name.trim_end_matches(".wav"), &id[..8])
                    } else {
                        name.clone()
                    };
                    metadata.push(
                        json!({"id":identity,"name":output_name,"bytes":bytes,"sha256":hash}),
                    );
                    artifacts.push((
                        identity,
                        Artifact {
                            owner: job.peer.credential.clone(),
                            runtime: None,
                            path: target,
                            bytes,
                            expires: Instant::now() + Duration::from_secs(3600),
                        },
                    ));
                }
                let mut state = shared
                    .lock()
                    .map_err(|_| protocol("Authority is unavailable"))?;
                if state.artifacts.len() + artifacts.len() > 32
                    || state.artifacts.values().map(|a| a.bytes).sum::<u64>()
                        + artifacts.iter().map(|(_, a)| a.bytes).sum::<u64>()
                        > 4 * 1024 * 1024 * 1024
                {
                    return Err(Error::QueueFull);
                }
                for (id, artifact) in artifacts {
                    state.artifacts.insert(id, artifact);
                }
                staged.0.clear();
                Ok(Some(if stem_mode {
                    json!(metadata)
                } else {
                    metadata.remove(0)
                }))
            }
            JobKind::ReadArtifact { path, offset, count, total, #[cfg(test)] hook } => {
                #[cfg(test)]
                if let Some(hook) = hook { hook(); }
                use std::io::{Seek, SeekFrom};
                let mut file = File::open(path)?;
                file.seek(SeekFrom::Start(offset))?;
                let mut data = vec![0; count];
                file.read_exact(&mut data)?;
                let data = base64::engine::general_purpose::STANDARD.encode(data);
                let state = shared.lock().map_err(|_| protocol("Authority is unavailable"))?;
                if !state.registry.authorizes(&job.peer.credential, &job.peer.role) {
                    return Err(protocol("Credential was revoked during download"));
                }
                drop(state);
                Ok(Some(json!({"offset":offset,"data":data,"complete":offset+count as u64==total})))
            }
            JobKind::Save { project, path, expected, replace_existing } => {
                project.save(&path, replace_existing).map_err(engine)?;
                names::saved_title(&shared, &job.peer, &job.request, expected, &project.session.name);
                if let Ok(mut state) = shared.lock() {
                    if state.authority.session == project.session {
                        let name = path.file_name().map(|name| name.to_string_lossy().into_owned());
                        if state.project_name != name {
                            state.project_name = name;
                            for peer in state.peers.values() { let _ = state.queue_snapshot(&peer.out); }
                        }
                    }
                }
                // Once the atomic directory commit succeeds, its receipt remains accepted.
                Ok(Some(json!(path.file_name().unwrap().to_string_lossy())))
            }
            #[cfg(target_os = "macos")]
            JobKind::Package { project } => projects::package(&shared, &job.peer, &project, &job.cancel),
            JobKind::Open { path, expected } => open_project(&shared, &job.peer, &job.cancel, path, expected),
            #[cfg(target_os = "macos")]
            JobKind::ImportProject { zip, name, expected } => {
                let path = unpack_project(&shared, &zip, &name)?;
                open_project(&shared, &job.peer, &job.cancel, path, expected)
            }
        }
    })).unwrap_or_else(|_|Err(protocol("Worker failed; no automatic retry was performed")));
    if let Ok(mut state) = shared.lock() {
        if starting_record
            && state
                .recording_preparing
                .as_ref()
                .is_some_and(|cancel| Arc::ptr_eq(cancel, &job.cancel))
        {
            state.recording_preparing = None;
            state.broadcast_transport();
        }
        if let (Some(gesture), Err(_)) = (&begin, &result) {
            state.writes.abandon_gesture(gesture);
        }
        state.pending.remove(&key(&job.peer, &job.request));
        // Recheck at the final receipt/send boundary too: a revocation can win
        // the authority lock between worker validation and completion.
        let result = if protected_read
            && (job.cancel.load(Ordering::Acquire)
                || !state
                    .registry
                    .authorizes(&job.peer.credential, &job.peer.role))
        {
            Err(protocol("Read cancelled or its credential was revoked"))
        } else {
            result
        };
        let reply = match result {
            Ok(payload) => response(
                &job.request.id,
                "accepted",
                state.authority.revision,
                "",
                payload,
            ),
            Err(Error::Conflict(s)) => response(
                &job.request.id,
                "conflict",
                state.authority.revision,
                &s,
                None,
            ),
            Err(e) => response(
                &job.request.id,
                "rejected",
                state.authority.revision,
                &e.to_string(),
                None,
            ),
        };
        state.remember(key(&job.peer, &job.request), digest, reply.clone());
        let _ = out.send(&reply, !ordered);
    }
}

/// One in-flight input plus one latest pending input; no unbounded encoding job queue.
/// The control owner captures typed immutable state and returns before JSON/model traversal.
#[derive(Clone)]
struct SessionWitness {
    revision: u64,
    base: Arc<Session>,
    controls: BTreeMap<String, MixControls>,
    bytes: usize,
}
#[derive(Clone)]
enum SessionInput {
    Owned(Arc<Session>),
    Shared(SessionWitness),
}
impl SessionInput {
    fn base(&self) -> &Session {
        match self {
            Self::Owned(session) => session,
            Self::Shared(witness) => &witness.base,
        }
    }
    fn value(&self) -> Result<Value, Error> {
        let mut value = wire(self.base())?;
        if let Self::Shared(witness) = self {
            for track in value["tracks"].as_array_mut().into_iter().flatten() {
                Self::patch_value(track, &witness.controls);
            }
            Self::patch_value(&mut value["masterTrack"], &witness.controls);
        }
        Ok(value)
    }
    fn patch_value(track: &mut Value, controls: &BTreeMap<String, MixControls>) {
        if let Some(controls) = track["id"].as_str().and_then(|id| controls.get(id)) {
            track["gain"] = json!(controls.gain);
            track["pan"] = json!(controls.pan);
            track["muted"] = json!(controls.muted);
            track["solo"] = json!(controls.solo);
            track["armed"] = json!(controls.armed);
        }
    }
}
pub(crate) struct GraphSource {
    session: SessionInput,
    media: BTreeMap<String, Media>,
}
impl GraphSource {
    pub(crate) fn materialize(&self) -> Project {
        let mut session = self.session.base().clone();
        if let SessionInput::Shared(witness) = &self.session {
            for track in session
                .tracks
                .iter_mut()
                .chain(std::iter::once(&mut session.master_track))
            {
                if let Some(controls) = witness.controls.get(&track.id) {
                    track.gain = controls.gain;
                    track.pan = controls.pan;
                    track.muted = controls.muted;
                    track.solo = controls.solo;
                    track.armed = controls.armed;
                }
            }
        }
        Project {
            session,
            media: self.media.clone(),
        }
    }
}
fn history_action(command: &RemoteCommand) -> (&str, Option<&str>) {
    match command {
        RemoteCommand::UpdateTrackControls {
            name, gesture_id, ..
        } => (name.as_str(), gesture_id.as_deref()),
        RemoteCommand::ReplaceTracks {
            name, gesture_id, ..
        } => (
            name.as_deref().unwrap_or("Edit tracks"),
            gesture_id.as_deref(),
        ),
        RemoteCommand::EditTracks { edit } => (edit.name.as_str(), edit.gesture_id.as_deref()),
        RemoteCommand::SetTrackName { .. } => ("Rename track", None),
        RemoteCommand::SetTrackGain { .. } => ("Adjust volume", None),
        RemoteCommand::SetTrackPan { .. } => ("Adjust pan", None),
        RemoteCommand::SetTrackMute { .. } => ("Mute track", None),
        RemoteCommand::SetTrackSolo { .. } => ("Solo track", None),
        RemoteCommand::SetTrackArm { .. } => ("Arm track", None),
        RemoteCommand::AddTrack { .. } => ("Add track", None),
        RemoteCommand::RemoveTrack { .. } => ("Remove track", None),
        RemoteCommand::SplitClip { .. } => ("Split clip", None),
        RemoteCommand::MoveClip { .. } => ("Move clip", None),
        RemoteCommand::DeleteClips { .. } => ("Delete clips", None),
        RemoteCommand::FinishUpload { .. } => ("Import audio", None),
        RemoteCommand::SaveProject { .. } => ("Rename session", None),
        RemoteCommand::Editor { intent } => match intent.operation.as_str() {
            "session.timing" => ("Change tempo/meter", None),
            "session.rename" => ("Rename session", None),
            "designer.commit" => ("Render instrument", None),
            "drums.commit" => ("Render drums", None),
            operation => (master::label(operation).unwrap_or("Edit session"), None),
        },
        _ => ("Edit session", None),
    }
}
fn encoded_size(value: &Value) -> Result<usize, Error> {
    struct Count(usize);
    impl Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .ok_or_else(|| std::io::Error::other("Document size overflow"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    serde_json::to_writer(&mut count, value)?;
    Ok(count.0)
}
fn control_tokens(controls: MixControls) -> Result<usize, Error> {
    // Match the actual wire's f32 -> JSON Value -> canonical f64 serializer path.
    [
        &json!(controls.gain),
        &json!(controls.pan),
        &json!(controls.muted),
        &json!(controls.solo),
        &json!(controls.armed),
    ]
    .into_iter()
    .try_fold(0, |total, value| Ok(total + encoded_size(value)?))
}
fn unchanged_control_content(
    track: &Track,
    controls: &soundroom_client::models::RemoteTrackControls,
) -> Result<bool, Error> {
    use soundroom_client::models::{AutomationCurve as Curve, AutomationValueScale as Scale};
    let scale = controls
        .volume_automation_scale
        .as_ref()
        .map(|scale| match scale {
            Scale::Linear => "linear",
            Scale::Decibels => "decibels",
        });
    if serde_json::to_value(&track.routing)? != serde_json::to_value(&controls.routing)?
        || track.name != controls.name
        || i64::from(track.color) != controls.color
        || track.volume_automation_scale.as_deref() != scale
        || track.extra.get("inputDeviceUID").and_then(Value::as_str)
            != controls.input_device_uid.as_deref()
        || track.extra.get("inputDeviceName").and_then(Value::as_str)
            != controls.input_device_name.as_deref()
        || track.extra.get("inputChannel") != Some(&json!(controls.input_channel))
        || track.extra.get("stereoInput") != Some(&json!(controls.stereo_input))
        || track.mix_automation.len() != controls.mix_automation.len()
    {
        return Ok(false);
    }
    for (key, points) in &track.mix_automation {
        let Some(other) = controls.mix_automation.get(key) else {
            return Ok(false);
        };
        if points.len() != other.len() {
            return Ok(false);
        }
        for (point, other) in points.iter().zip(other) {
            let curve = match other.curve.as_ref().unwrap_or(&Curve::Linear) {
                Curve::Linear => "linear",
                Curve::Smooth => "smooth",
                Curve::EaseIn => "easeIn",
                Curve::EaseOut => "easeOut",
                Curve::Hold => "hold",
            };
            if !point.extra.is_empty()
                || i64::try_from(point.frame).ok() != Some(other.frame)
                || point.value != other.value
                || point.curve != curve
                || point.curve_start != other.curve_start.unwrap_or(0.)
                || point.curve_end != other.curve_end.unwrap_or(1.)
            {
                return Ok(false);
            }
        }
    }
    // Effects are independent of mixer automation; strict equality preserves every effect field.
    Ok(wire(&track.realtime_effects)? == serde_json::to_value(&controls.realtime_effects)?)
}
struct CompactRequest {
    track_id: String,
    controls: MixControls,
    label: String,
    gesture: Option<String>,
    history: Option<bool>,
}
struct SnapshotInput {
    session: SessionInput,
    revision: u64,
    versions: BTreeMap<String, u64>,
    media: BTreeMap<String, Media>,
    server_id: String,
    server_name: String,
    project_name: Option<String>,
    project_id: String,
    transport: Value,
    peers: Vec<RemotePeer>,
    status: String,
}
impl SnapshotInput {
    fn prepare_witness(&self, cache: &Mutex<Option<SessionWitness>>) -> Result<(), Error> {
        let SessionInput::Owned(session) = &self.session else {
            return Ok(());
        };
        session.validate().map_err(engine)?;
        let value = wire(session.as_ref())?;
        validation::validate_model(&value, "Session")?;
        let witness = SessionWitness {
            revision: self.revision,
            bytes: encoded_size(&value)?,
            base: session.clone(),
            controls: session
                .tracks
                .iter()
                .chain(std::iter::once(&session.master_track))
                .map(|track| (track.id.clone(), MixControls::from_track(track)))
                .collect(),
        };
        let mut cache = cache
            .lock()
            .map_err(|_| protocol("Session witness is unavailable"))?;
        if cache
            .as_ref()
            .map_or(true, |current| current.revision <= witness.revision)
        {
            *cache = Some(witness);
        }
        Ok(())
    }
    fn control_delta(
        &self,
        id: &str,
        controls: MixControls,
        compact: bool,
    ) -> Result<Vec<u8>, Error> {
        let mut delta = json!({"revision":self.revision,"tracks":[],"controls":[],"removedTrackIDs":[],
            "trackRevisions":{id:self.versions[id]},"metadataUpdated":false,"assets":[]});
        if compact {
            delta["controlPatches"] = json!([{"trackID":id,"gain":controls.gain,"pan":controls.pan,
                "muted":controls.muted,"solo":controls.solo,"armed":controls.armed}]);
        } else {
            let track = self
                .session
                .base()
                .tracks
                .iter()
                .chain(std::iter::once(&self.session.base().master_track))
                .find(|track| track.id == id)
                .ok_or_else(|| protocol("Control target is unavailable"))?;
            let mut value = wire(track)?;
            SessionInput::patch_value(&mut value, &BTreeMap::from([(id.into(), controls)]));
            delta["tracks"] = json!([value]);
        }
        validation::validate_model(&delta, "RemoteSessionDelta")?;
        Ok(serde_json::to_vec(
            &json!({"version":3,"kind":"event","event":{"type":"delta","delta":delta}}),
        )?)
    }
    fn assets(&self) -> Vec<Value> {
        self.media
            .iter()
            .map(|(id, m)| {
                json!({"id":id,"frames":m.info.frames,
            "channels":m.info.channels,"peaks":[],"waveformStartFrame":0})
            })
            .collect()
    }
    fn value(&self) -> Result<Value, Error> {
        let value = json!({"serverID":self.server_id,"serverName":self.server_name,
            "session":self.session.value()?,"revision":self.revision,"trackRevisions":self.versions,
            "transport":self.transport,"devices":[],"inputDeviceID":0,"outputDeviceID":0,
            "assets":self.assets(),"status":self.status,"peers":self.peers,"projectName":self.project_name,"projectID":self.project_id});
        validation::validate_model(&value, "RemoteSnapshot")?;
        Ok(value)
    }
    fn live(&self) -> BTreeMap<&'static str, Value> {
        live_overlays(
            &json!({"transport":self.transport,"status":self.status,"peers":self.peers,
            "devices":[],"inputDeviceID":0,"outputDeviceID":0}),
        )
    }
    fn delta(&self, before: &Session) -> Result<Vec<u8>, Error> {
        let old = before
            .tracks
            .iter()
            .chain(std::iter::once(&before.master_track))
            .map(|t| (&t.id, t))
            .collect::<BTreeMap<_, _>>();
        let tracks = self
            .session
            .base()
            .tracks
            .iter()
            .chain(std::iter::once(&self.session.base().master_track))
            .filter(|t| old.get(&t.id).copied() != Some(*t))
            .map(wire)
            .collect::<Result<Vec<_>, _>>()?;
        let removed = before
            .tracks
            .iter()
            .filter(|t| !self.session.base().tracks.iter().any(|n| n.id == t.id))
            .map(|t| t.id.clone())
            .collect::<Vec<_>>();
        let mut delta = json!({"revision":self.revision,"tracks":tracks,"controls":[],
            "removedTrackIDs":removed,"trackOrder":self.session.base().tracks.iter().map(|t|t.id.clone()).collect::<Vec<_>>(),
            "trackRevisions":self.versions,"name":self.session.base().name,"metadataUpdated":true,
            "timing":self.session.base().timing,"sessionVersion":self.session.base().version,"assets":self.assets()});
        // metadataUpdated: an absent analysis tells clients it was removed (Undo).
        if let Some(analysis) = self
            .session
            .base()
            .extra
            .get(soundroom_engine::mastering::SESSION_KEY)
        {
            delta["masteringAnalysis"] = analysis.clone();
        }
        validation::validate_model(&delta, "RemoteSessionDelta")?;
        Ok(serde_json::to_vec(
            &json!({"version":3,"kind":"event","event":{"type":"delta","delta":delta}}),
        )?)
    }
}
struct Publication {
    input: SnapshotInput,
    before: Option<Session>,
    prepared: Option<Arc<SnapshotDocument>>,
    compact: Option<(String, MixControls)>,
    targets: BTreeMap<String, Outbox>,
}
struct PublicationWorker {
    pending: Arc<Mutex<Option<Publication>>>,
    wake: std::sync::mpsc::SyncSender<()>,
}
impl PublicationWorker {
    fn new(global: Arc<AtomicUsize>, witness: Arc<Mutex<Option<SessionWitness>>>) -> Self {
        Self::with_hook(global, witness, None)
    }
    fn with_hook(
        global: Arc<AtomicUsize>,
        witness: Arc<Mutex<Option<SessionWitness>>>,
        hook: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Self {
        let pending = Arc::new(Mutex::new(None::<Publication>));
        let (wake, receiver) = std::sync::mpsc::sync_channel(1);
        let queue = pending.clone();
        std::thread::spawn(move || {
            while receiver.recv().is_ok() {
                let task = match queue.lock() {
                    Ok(mut pending) => pending.take(),
                    Err(_) => return,
                };
                let Some(mut task) = task else { continue };
                task.targets
                    .retain(|_, out| !out.closed.load(Ordering::Acquire));
                if task.targets.is_empty() {
                    continue;
                }
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || -> Result<(), Error> {
                        if let Some(hook) = &hook {
                            hook();
                        }
                        task.input.prepare_witness(&witness)?;
                        let bounded = |bytes: Vec<u8>| {
                            (bytes.len() <= validation::MAX_FRAME_BYTES).then_some(bytes)
                        };
                        let (delta, legacy) = if let Some((id, controls)) = &task.compact {
                            let compact = bounded(task.input.control_delta(id, *controls, true)?);
                            // A legacy track may be large. Encode it once only if some peers need it.
                            let legacy = if task
                                .targets
                                .values()
                                .any(|out| !out.compact_controls.load(Ordering::Acquire))
                            {
                                bounded(task.input.control_delta(id, *controls, false)?)
                            } else {
                                None
                            };
                            (compact, legacy)
                        } else {
                            (
                                task.before
                                    .as_ref()
                                    .map(|before| task.input.delta(before))
                                    .transpose()?
                                    .and_then(bounded),
                                None,
                            )
                        };
                        let mut document = task.prepared.clone();
                        for out in task.targets.values() {
                            let bytes = if task.compact.is_some()
                                && !out.compact_controls.load(Ordering::Acquire)
                            {
                                legacy.as_ref()
                            } else {
                                delta.as_ref()
                            };
                            let result = match (bytes, out.snapshot_active.load(Ordering::Acquire))
                            {
                                (Some(bytes), false) => out.send_bytes(bytes.clone(), false),
                                _ => {
                                    if document.is_none() {
                                        document = Some(SnapshotDocument::prepare(
                                            task.input.value()?,
                                            global.clone(),
                                        )?);
                                    }
                                    schedule_snapshot(out, document.as_ref().unwrap().clone())
                                }
                            };
                            if result.is_err() {
                                out.close();
                            }
                        }
                        Ok(())
                    },
                ));
                if !matches!(result, Ok(Ok(()))) {
                    for out in task.targets.values() {
                        out.close();
                    }
                }
                if let Ok(pending) = queue.lock() {
                    for (id, out) in &task.targets {
                        if pending
                            .as_ref()
                            .map_or(true, |next| !next.targets.contains_key(id))
                        {
                            out.snapshot_preparing.store(false, Ordering::Release);
                        }
                    }
                } else {
                    return;
                }
            }
        });
        Self { pending, wake }
    }
    fn enqueue(&self, mut task: Publication) -> Result<(), Error> {
        task.targets
            .retain(|_, out| !out.closed.load(Ordering::Acquire));
        if task.targets.is_empty() {
            return Ok(());
        }
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| protocol("Publication worker is unavailable"))?;
        if let Some(mut previous) = pending.take() {
            previous
                .targets
                .retain(|_, out| !out.closed.load(Ordering::Acquire));
            task.targets.extend(previous.targets);
            // Skipping an intermediate authored revision requires an atomic latest snapshot.
            task.before = None;
            task.compact = None;
        }
        if task.targets.len() > 16 {
            for out in task.targets.values() {
                out.close();
            }
            return Err(Error::QueueFull);
        }
        // Seed before handing the source to the worker. A later live event therefore
        // wins even if graph/document preparation takes a long time.
        let overlays = task.input.live();
        for out in task.targets.values() {
            out.seed_live(overlays.clone())?;
        }
        // Set the barrier before exposing the job. Any later authored input is captured
        // behind this one, so its delta cannot overtake the initial snapshot preparation.
        for out in task.targets.values() {
            out.snapshot_preparing.store(true, Ordering::Release);
        }
        *pending = Some(task);
        match self.wake.try_send(()) {
            Ok(()) | Err(std::sync::mpsc::TrySendError::Full(())) => Ok(()),
            Err(std::sync::mpsc::TrySendError::Disconnected(())) => {
                Err(protocol("Publication worker stopped"))
            }
        }
    }
}

fn live_overlays(snapshot: &Value) -> BTreeMap<&'static str, Value> {
    let event = |event: Value| json!({"version":3,"kind":"event","event":event});
    BTreeMap::from([
        (
            "transport",
            event(json!({"type":"transport","transport":snapshot["transport"]})),
        ),
        (
            "status",
            event(json!({"type":"status","message":snapshot["status"]})),
        ),
        (
            "presence",
            event(json!({"type":"presence","peers":snapshot["peers"]})),
        ),
        (
            "devices",
            event(json!({"type":"devices","devices":snapshot["devices"],
            "inputID":snapshot["inputDeviceID"],"outputID":snapshot["outputDeviceID"]})),
        ),
    ])
}
/// Authored admission and shared encoding count this exact metadata-independent
/// shape. Growing status/presence or a later callback clock cannot make an
/// already-admitted authored document exceed the next snapshot's body budget.
fn neutralize_snapshot(snapshot: &mut Value) {
    snapshot["transport"] =
        json!({"frame":0,"isPlaying":false,"isRecording":false,"isPreparing":false,"hostTime":0});
    snapshot["status"] = json!("");
    snapshot["peers"] = json!([]);
    snapshot["devices"] = json!([]);
    snapshot["inputDeviceID"] = json!(0);
    snapshot["outputDeviceID"] = json!(0);
}

/// One encoded, immutable revision can be shared by all peers without charging
/// each peer for another complete 64 MiB document. Queued page frames remain charged.
struct SnapshotDocument {
    bytes: Vec<u8>,
    revision: Value,
    hash: String,
    global: Arc<AtomicUsize>,
}
impl Drop for SnapshotDocument {
    fn drop(&mut self) {
        self.global.fetch_sub(self.bytes.len(), Ordering::AcqRel);
    }
}
impl SnapshotDocument {
    fn prepare(mut snapshot: Value, global: Arc<AtomicUsize>) -> Result<Arc<Self>, Error> {
        neutralize_snapshot(&mut snapshot);
        let revision = snapshot["revision"].clone();
        struct Bounded(Vec<u8>);
        impl Write for Bounded {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                if self.0.len().saturating_add(data.len()) > validation::MAX_SNAPSHOT_BYTES {
                    return Err(std::io::Error::other("Snapshot exceeds its limit"));
                }
                self.0.extend_from_slice(data);
                Ok(data.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut body = Bounded(Vec::new());
        serde_json::to_writer(&mut body, &snapshot)?;
        let bytes = body.0;
        drop(snapshot);
        global
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                v.checked_add(bytes.len())
                    .filter(|n| *n <= GLOBAL_BYTES - GLOBAL_CONTROL_RESERVE)
            })
            .map_err(|_| Error::QueueFull)?;
        let hash = format!("{:x}", Sha256::digest(&bytes));
        Ok(Arc::new(Self {
            bytes,
            revision,
            hash,
            global,
        }))
    }
}
/// Snapshot pages are paced by the bounded channel. Edits during transfer coalesce
/// into one latest replacement snapshot instead of leaking an unbounded delta queue.
pub fn send_snapshot(out: &Outbox, snapshot: Value) -> Result<(), Error> {
    out.seed_live(live_overlays(&snapshot))?;
    let document = SnapshotDocument::prepare(snapshot, out.global.clone())?;
    schedule_snapshot(out, document)
}
fn schedule_snapshot(out: &Outbox, document: Arc<SnapshotDocument>) -> Result<(), Error> {
    if out.closed.load(Ordering::Acquire) {
        return Err(Error::Disconnected);
    }
    let mut pending = out
        .snapshot_pending
        .lock()
        .map_err(|_| protocol("Snapshot scheduler is unavailable"))?;
    if out.snapshot_active.load(Ordering::Acquire) {
        *pending = Some(document);
        return Ok(());
    }
    out.snapshot_active.store(true, Ordering::Release);
    drop(pending);
    let out = out.clone();
    std::thread::spawn(move || {
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || -> Result<(), Error> {
                let mut document = document;
                loop {
                    let id = new_id()?;
                    let deadline = Instant::now() + Duration::from_secs(60);
                    let send = |data: Vec<u8>| -> Result<(), Error> {
                        loop {
                            if out.closed.load(Ordering::Acquire) {
                                return Err(Error::Disconnected);
                            }
                            if Instant::now() > deadline {
                                return Err(Error::Timeout);
                            }
                            if out.bytes.load(Ordering::Acquire) < 2 * 1024 * 1024 {
                                return out.send_bytes(data, false);
                            }
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    };
                    let envelope = |kind: &str| json!({"version":3,"kind":kind,"transferID":id,"revision":document.revision,"totalBytes":document.bytes.len(),"sha256":document.hash});
                    if document.bytes.len() < 512 * 1024 {
                        let mut bytes=b"{\"version\":3,\"kind\":\"event\",\"event\":{\"type\":\"snapshot\",\"snapshot\":".to_vec();
                        bytes.extend_from_slice(&document.bytes);
                        bytes.extend_from_slice(b"}}");
                        send(bytes)?;
                        out.replay_live()?;
                    } else {
                        send(serde_json::to_vec(&envelope("snapshotBegin"))?)?;
                        out.replay_live()?;
                        for (offset, data) in document.bytes.chunks(512 * 1024).enumerate() {
                            send(serde_json::to_vec(
                                &json!({"version":3,"kind":"snapshotPage","transferID":id,"revision":document.revision,"offset":offset*512*1024,"data":base64::engine::general_purpose::STANDARD.encode(data)}),
                            )?)?;
                        }
                        send(serde_json::to_vec(&envelope("snapshotEnd"))?)?;
                    }
                    let mut pending = out
                        .snapshot_pending
                        .lock()
                        .map_err(|_| protocol("Snapshot scheduler is unavailable"))?;
                    if let Some(next) = pending.take() {
                        document = next;
                    } else {
                        out.snapshot_active.store(false, Ordering::Release);
                        return Ok(());
                    }
                }
            },
        ));
        if !matches!(run, Ok(Ok(()))) {
            out.close();
            if let Ok(mut pending) = out.snapshot_pending.lock() {
                *pending = None;
                out.snapshot_active.store(false, Ordering::Release);
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    use std::sync::mpsc::sync_channel;

    fn next_snapshot(inbox: &Inbox) -> Value {
        loop {
            let bytes = inbox.ordinary.recv_timeout(Duration::from_secs(2)).unwrap();
            inbox.received(bytes.len());
            let value: Value = serde_json::from_slice(&bytes).unwrap();
            if value["event"]["type"] == "snapshot" {
                return value["event"]["snapshot"].clone();
            }
            assert!(matches!(
                value["event"]["type"].as_str(),
                Some("transport" | "status" | "devices" | "presence")
            ));
        }
    }
    pub(super) fn control_fixture(points: usize) -> (std::path::PathBuf, Shared, Identity, String) {
        let root = std::env::temp_dir().join(format!("soundroom-compact-{}", new_id().unwrap()));
        private_directory(&root).unwrap();
        let mut registry = Registry::create(&root, "Compact").unwrap();
        let credential = registry.issue("Editor", RemoteRole::Editor).unwrap();
        let identity = Identity {
            runtime: new_id().unwrap(),
            credential: credential["credentialID"].as_str().unwrap().into(),
            name: "Editor".into(),
            role: RemoteRole::Editor,
        };
        let shared = Service::open(&root, None).unwrap();
        let id = new_id().unwrap();
        let mut track = Track {
            id: id.clone(),
            name: "Vox \u{1f3b6} \u{00e9} \"quoted\"\n".into(),
            ..Default::default()
        };
        track.extra.insert("inputChannel".into(), json!(0));
        track.extra.insert("stereoInput".into(), json!(false));
        track.mix_automation.insert(
            "pan".into(),
            (0..points)
                .map(|frame| soundroom_engine::model::AutomationPoint {
                    frame: frame as u64,
                    value: 0.5,
                    curve: "linear".into(),
                    curve_start: 0.,
                    curve_end: 1.,
                    extra: BTreeMap::new(),
                })
                .collect(),
        );
        let mut session = Session {
            name: "Escaped \t \u{1f3b9}".into(),
            ..Default::default()
        };
        session.tracks.push(track);
        let mut state = shared.lock().unwrap();
        state.authority = Authority::new(session).unwrap();
        state
            .capture_snapshot()
            .prepare_witness(&state.witness)
            .unwrap();
        drop(state);
        (root, shared, identity, id)
    }
    fn control_request(
        identity: &Identity,
        id: &str,
        revision: u64,
        command: Value,
    ) -> RemoteRequest {
        serde_json::from_value(json!({"id":new_id().unwrap(),"clientID":identity.runtime,
            "expectedTrackRevisions":{id:revision},"command":command}))
        .unwrap()
    }
    #[test]
    fn scalar_size_witness_matches_canonical_json_and_rolls_back_admission_failure() {
        let (root, shared, identity, id) = control_fixture(16);
        let mut state = shared.lock().unwrap();
        let base = state.witness.lock().unwrap().as_ref().unwrap().base.clone();
        for (gain, pan, muted) in [
            (0.12345679_f32, -0.0, false),
            (1.5, -1., true),
            (1., 0.000000000001, false),
            (0.001, 1., true),
        ] {
            let version = state.authority.track_revisions[&id];
            let request = control_request(
                &identity,
                &id,
                version,
                json!({"type":"setTrackGain","trackID":id,"gain":gain}),
            );
            let mut compact = state.compact_request(&identity.runtime, &request).unwrap();
            compact.controls.pan = pan;
            compact.controls.muted = muted;
            state.apply_compact(&identity, &request, &compact).unwrap();
            let witness = state.witness.lock().unwrap().as_ref().unwrap().clone();
            assert!(
                Arc::ptr_eq(&base, &witness.base),
                "Scalar edits must retain immutable content"
            );
            assert_eq!(
                witness.bytes,
                serde_json::to_vec(&wire(&state.authority.session).unwrap())
                    .unwrap()
                    .len()
            );
            let shared_value = state.capture_snapshot().value().unwrap();
            let full = state.capture_snapshot_for(
                &state.authority.session,
                state.authority.revision,
                &state.authority.track_revisions,
                &state.media,
            );
            // Transport hostTime is captured independently; compare authored model exactly.
            assert_eq!(shared_value["session"], full.value().unwrap()["session"]);
        }
        let version = state.authority.track_revisions[&id];
        let request = control_request(
            &identity,
            &id,
            version,
            json!({"type":"setTrackGain","trackID":id,"gain":0.123456789}),
        );
        let compact = state.compact_request(&identity.runtime, &request).unwrap();
        let before = state.authority.session.clone();
        let history = state
            .authority
            .history_state(&identity.runtime)
            .0
            .unwrap()
            .to_owned();
        state.witness.lock().unwrap().as_mut().unwrap().bytes = validation::MAX_SNAPSHOT_BYTES;
        assert!(state.apply_compact(&identity, &request, &compact).is_err());
        assert_eq!(state.authority.session, before);
        assert_eq!(state.authority.revision, version);
        assert_eq!(
            state.authority.history_state(&identity.runtime).0,
            Some(history.as_str())
        );
        let mut viewer = identity.clone();
        viewer.role = RemoteRole::Viewer;
        state.witness.lock().unwrap().as_mut().unwrap().bytes =
            encoded_size(&wire(&before).unwrap()).unwrap();
        assert!(state.apply_compact(&viewer, &request, &compact).is_err());
        assert_eq!(state.authority.session, before);
        let mut stale = request.clone();
        stale.expected_track_revisions = BTreeMap::from([(id.clone(), 0)]);
        assert!(state.apply_compact(&identity, &stale, &compact).is_err());
        assert_eq!(state.authority.session, before);
        drop(state);
        drop(shared);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn dense_controls_and_authored_undo_reuse_content_and_preserve_gestures() {
        let (root, shared, identity, id) = control_fixture(100000);
        let mut state = shared.lock().unwrap();
        let base = state.witness.lock().unwrap().as_ref().unwrap().base.clone();
        for gain in [0.2, 0.3] {
            let track = &state.authority.session.tracks[0];
            let value = wire(track).unwrap();
            let controls = json!({"trackID":id,"name":track.name,"color":track.color,"gain":gain,"pan":track.pan,
                "muted":track.muted,"solo":track.solo,"armed":track.armed,"inputChannel":0,"stereoInput":false,
                "mixAutomation":value["mixAutomation"],"realtimeEffects":[]});
            let request = control_request(
                &identity,
                &id,
                state.authority.track_revisions[&id],
                json!({"type":"updateTrackControls","controls":controls,"name":"Ride vocal","gestureID":"fader-drag"}),
            );
            let compact = state
                .compact_request(&identity.runtime, &request)
                .expect("Unchanged dense content is eligible");
            state.apply_compact(&identity, &request, &compact).unwrap();
        }
        assert_eq!(
            state.authority.history_state(&identity.runtime).0,
            Some("Ride vocal")
        );
        for (command, gain) in [("undo", 1.), ("redo", 0.3)] {
            let request = control_request(
                &identity,
                &id,
                state.authority.track_revisions[&id],
                json!({"type":command}),
            );
            let compact = state.compact_request(&identity.runtime, &request).unwrap();
            state.apply_compact(&identity, &request, &compact).unwrap();
            assert_eq!(state.authority.session.tracks[0].gain, gain);
            assert_eq!(
                state.authority.session.tracks[0].mix_automation["pan"].len(),
                100000
            );
            assert!(Arc::ptr_eq(
                &base,
                &state.witness.lock().unwrap().as_ref().unwrap().base
            ));
        }
        let mut other = identity.clone();
        other.runtime = new_id().unwrap();
        let request = control_request(
            &other,
            &id,
            state.authority.track_revisions[&id],
            json!({"type":"setTrackPan","trackID":id,"pan":0.8}),
        );
        let compact = state.compact_request(&other.runtime, &request).unwrap();
        state.apply_compact(&other, &request, &compact).unwrap();
        let undo = control_request(
            &identity,
            &id,
            state.authority.track_revisions[&id],
            json!({"type":"undo"}),
        );
        assert!(
            state.compact_request(&identity.runtime, &undo).is_none(),
            "Another author changing the resource fences Undo"
        );
        assert!(state
            .authority
            .undo(&identity.runtime, Role::Editor)
            .is_err());
        drop(state);
        drop(shared);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn negotiated_control_deltas_omit_new_field_for_legacy_peers() {
        let (root, shared, identity, id) = control_fixture(8);
        let global = shared.lock().unwrap().global.clone();
        let (modern, in_modern) = Outbox::pair(global.clone());
        let (legacy, in_legacy) = Outbox::pair(global);
        let credential = shared
            .lock()
            .unwrap()
            .registry
            .issue("Viewer", RemoteRole::Viewer)
            .unwrap();
        let viewer = Identity {
            runtime: new_id().unwrap(),
            name: "Viewer".into(),
            credential: credential["credentialID"].as_str().unwrap().into(),
            role: RemoteRole::Viewer,
        };
        let declare = |features: Vec<&str>| {
            json!({"version":3,"kind":"request","id":new_id().unwrap(),"clientID":viewer.runtime,"command":{"type":"editor","intent":{
            "operation":"client.features","context":{"clipIDs":[],"cursorFrame":0},"integers":{},"numbers":{},"strings":{},"flags":{},
            "payload":base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&features).unwrap())}}})
        };
        Service::dispatch(
            &shared,
            &viewer,
            &modern,
            declare(vec!["compact-control-patches"]),
        )
        .unwrap();
        let bytes = in_modern
            .priority
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        in_modern.received(bytes.len());
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap()["status"],
            "accepted"
        );
        assert!(modern.compact_controls.load(Ordering::Acquire));
        Service::dispatch(&shared, &viewer, &modern, declare(vec!["unknown"])).unwrap();
        let bytes = in_modern
            .priority
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        in_modern.received(bytes.len());
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap()["status"],
            "rejected"
        );
        assert!(modern.compact_controls.load(Ordering::Acquire));
        {
            let mut state = shared.lock().unwrap();
            let request = control_request(
                &identity,
                &id,
                0,
                json!({"type":"setTrackPan","trackID":id,"pan":0.7}),
            );
            let compact = state.compact_request(&identity.runtime, &request).unwrap();
            state.apply_compact(&identity, &request, &compact).unwrap();
            state
                .publication
                .enqueue(Publication {
                    input: state.capture_snapshot(),
                    before: None,
                    prepared: None,
                    compact: Some((id.clone(), compact.controls)),
                    targets: BTreeMap::from([
                        ("modern".into(), modern.clone()),
                        ("legacy".into(), legacy.clone()),
                    ]),
                })
                .unwrap();
        }
        for (inbox, compact) in [(&in_modern, true), (&in_legacy, false)] {
            let bytes = inbox.ordinary.recv_timeout(Duration::from_secs(3)).unwrap();
            inbox.received(bytes.len());
            let frame: Value = serde_json::from_slice(&bytes).unwrap();
            validation::validate_model(&frame["event"]["delta"], "RemoteSessionDelta").unwrap();
            let delta = &frame["event"]["delta"];
            assert_eq!(delta["revision"], 1);
            assert_eq!(delta.get("controlPatches").is_some(), compact);
            assert_eq!(
                delta["tracks"].as_array().unwrap().len(),
                usize::from(!compact)
            );
            if compact {
                assert_eq!(delta["controlPatches"][0]["pan"], json!(0.7_f32));
                assert!(bytes.len() < 2048);
            } else {
                assert_eq!(
                    delta["tracks"][0]["mixAutomation"]["pan"]
                        .as_array()
                        .unwrap()
                        .len(),
                    8
                );
            }
        }
        Service::dispatch(&shared, &viewer, &modern, declare(vec![])).unwrap();
        let bytes = in_modern
            .priority
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        in_modern.received(bytes.len());
        assert!(!modern.compact_controls.load(Ordering::Acquire));
        let (new_connection, _) = Outbox::pair(shared.lock().unwrap().global.clone());
        assert!(!new_connection.compact_controls.load(Ordering::Acquire));
        drop(shared);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn blocked_encoder_keeps_control_available_and_coalesces_latest_for_all_targets() {
        let root = std::env::temp_dir().join(format!("soundroom-encode-{}", new_id().unwrap()));
        private_directory(&root).unwrap();
        Registry::create(&root, "Worker").unwrap();
        let shared = Service::open(&root, None).unwrap();
        let (entered, ready) = sync_channel(1);
        let (release, wait) = sync_channel(1);
        let wait = Mutex::new(wait);
        let calls = AtomicUsize::new(0);
        let hook = Arc::new(move || {
            if calls.fetch_add(1, Ordering::AcqRel) == 0 {
                entered.send(()).unwrap();
                wait.lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
            }
        });
        let global = shared.lock().unwrap().global.clone();
        let (a, in_a) = Outbox::pair(global.clone());
        let (b, in_b) = Outbox::pair(global.clone());
        {
            let mut state = shared.lock().unwrap();
            state.publication =
                PublicationWorker::with_hook(global.clone(), state.witness.clone(), Some(hook));
            state.authority.revision = 1;
            state.authority.session.name = "First".into();
            state.queue_snapshot(&a).unwrap();
        }
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        {
            // This is deterministic: the encoding worker is still blocked at its gate.
            let mut state = shared
                .try_lock()
                .expect("Encoder cannot own the control mutex");
            state.authority.revision = 2;
            state.authority.session.name = "Intermediate".into();
            state.queue_snapshot(&b).unwrap();
            state.authority.revision = 3;
            state.authority.session.name = "Latest".into();
            state.queue_snapshot(&a).unwrap();
            let pending = state.publication.pending.lock().unwrap();
            let pending = pending.as_ref().unwrap();
            assert_eq!(pending.input.revision, 3);
            assert_eq!(pending.targets.len(), 2);
            assert!(pending.before.is_none());
        }
        a.send(
            &json!({"version":3,"kind":"heartbeatReply","id":new_id().unwrap()}),
            true,
        )
        .unwrap();
        let priority = in_a
            .priority
            .recv_timeout(Duration::from_millis(100))
            .unwrap();
        in_a.received(priority.len());
        assert!(in_a.ordinary.try_recv().is_err());
        release.send(()).unwrap();
        let first = next_snapshot(&in_a);
        let latest_a = next_snapshot(&in_a);
        let latest_b = next_snapshot(&in_b);
        assert_eq!(first["revision"], 1);
        assert_eq!(first["session"]["name"], "First");
        assert_eq!(latest_a["revision"], 3);
        assert_eq!(latest_a["session"]["name"], "Latest");
        assert_eq!(latest_a, latest_b);
        while let Ok(bytes) = in_b.ordinary.try_recv() {
            in_b.received(bytes.len());
            let event: Value = serde_json::from_slice(&bytes).unwrap();
            assert!(matches!(
                event["event"]["type"].as_str(),
                Some("transport" | "status" | "presence" | "devices")
            ));
        }
        drop(shared);
        // Closing socket inboxes also releases any final live overlays which
        // arrived after the authored snapshot inspected above.
        drop(in_a);
        drop(in_b);
        let deadline = Instant::now() + Duration::from_secs(2);
        while global.load(Ordering::Acquire) != 0 {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    fn rpc_frame(peer: &Identity, id: &str, command: Value, tracks: Value) -> Value {
        json!({"version":3,"kind":"request","id":id,"clientID":peer.runtime,
            "expectedTrackRevisions":tracks,"command":command})
    }
    fn scoped_cancel_command(target: &str) -> Value {
        json!({"type":"editor","intent":{"operation":"jobs.cancel",
            "context":{"clipIDs":[],"cursorFrame":0},"integers":{},"numbers":{},
            "strings":{"id":target},"flags":{}}})
    }
    fn rpc_reply(inbox: &Inbox, id: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(3);
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
            }
            assert!(Instant::now() < deadline, "RPC receipt timeout");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    #[test]
    fn scoped_cancel_overtakes_validated_admission_without_cancelling_other_work() {
        let (root, shared, editor, track) = control_fixture(0);
        let (out, inbox) = Outbox::pair(shared.lock().unwrap().global.clone());
        let job_id = new_id().unwrap();
        let command = json!({"type":"setTrackName","trackID":track,"name":"Must not commit"});
        // Simulate a decoded request held behind another command while urgent
        // Cancel reaches authority first. No runtime/model is needed to test
        // this ingress invariant.
        let waiting = Service::validate_request(
            &editor,
            rpc_frame(&editor, &job_id, command.clone(), json!({track.clone():0})),
        )
        .unwrap();
        let cancel_id = new_id().unwrap();
        Service::dispatch(
            &shared,
            &editor,
            &out,
            rpc_frame(
                &editor,
                &cancel_id,
                scoped_cancel_command(&job_id),
                json!({}),
            ),
        )
        .unwrap();
        assert_eq!(rpc_reply(&inbox, &cancel_id)["status"], "accepted");
        Service::dispatch_validated(&shared, &editor, &out, waiting).unwrap();
        let cancelled = rpc_reply(&inbox, &job_id);
        assert_eq!(cancelled["status"], "conflict", "{cancelled}");
        assert_eq!(shared.lock().unwrap().authority.revision, 0);
        // A different runtime using the same credential/request ID has its own
        // ownership scope; cancellation cannot affect that client's request.
        let mut other = editor.clone();
        other.runtime = new_id().unwrap();
        Service::dispatch(
            &shared,
            &other,
            &out,
            rpc_frame(&other, &job_id, command, json!({track.clone():0})),
        )
        .unwrap();
        assert_eq!(rpc_reply(&inbox, &job_id)["status"], "accepted");
        assert_eq!(shared.lock().unwrap().authority.revision, 1);
        // The cancelled request's own receipt remains deterministic on retry.
        Service::dispatch(
            &shared,
            &editor,
            &out,
            rpc_frame(
                &editor,
                &job_id,
                json!({"type":"setTrackName","trackID":track,"name":"Must not commit"}),
                json!({track.clone():0}),
            ),
        )
        .unwrap();
        assert_eq!(rpc_reply(&inbox, &job_id), cancelled);
        drop(shared);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn preadmission_cancel_is_bounded_validated_and_consumed() {
        let (root, shared, editor, _) = control_fixture(0);
        let (out, inbox) = Outbox::pair(shared.lock().unwrap().global.clone());
        for index in 0..129 {
            let request_id = new_id().unwrap();
            Service::dispatch(
                &shared,
                &editor,
                &out,
                rpc_frame(
                    &editor,
                    &request_id,
                    scoped_cancel_command(&new_id().unwrap()),
                    json!({}),
                ),
            )
            .unwrap();
            assert_eq!(
                rpc_reply(&inbox, &request_id)["status"],
                if index < 128 { "accepted" } else { "busy" }
            );
        }
        assert_eq!(shared.lock().unwrap().cancelled_requests.len(), 128);
        let malformed = new_id().unwrap();
        Service::dispatch(
            &shared,
            &editor,
            &out,
            rpc_frame(
                &editor,
                &malformed,
                scoped_cancel_command("not-a-uuid"),
                json!({}),
            ),
        )
        .unwrap();
        assert_eq!(rpc_reply(&inbox, &malformed)["status"], "rejected");
        let target = shared
            .lock()
            .unwrap()
            .cancelled_requests
            .iter()
            .next()
            .unwrap()
            .2
            .clone();
        Service::dispatch(
            &shared,
            &editor,
            &out,
            rpc_frame(&editor, &target, json!({"type":"snapshot"}), json!({})),
        )
        .unwrap();
        assert_eq!(rpc_reply(&inbox, &target)["status"], "conflict");
        assert_eq!(shared.lock().unwrap().cancelled_requests.len(), 127);
        drop(shared);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn same_peer_stop_rpc_remains_serviceable_while_full_candidate_worker_is_blocked() {
        let (root, shared, _editor, id) = control_fixture(0);
        let credential = shared
            .lock()
            .unwrap()
            .registry
            .issue("Operator", RemoteRole::Operator)
            .unwrap();
        let operator = Identity {
            runtime: new_id().unwrap(),
            credential: credential["credentialID"].as_str().unwrap().into(),
            name: "Operator".into(),
            role: RemoteRole::Operator,
        };
        let (control, mut host) = soundroom_engine::realtime::channel();
        Service::attach_audio(&shared, control).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let finished = stop.clone();
        // This runner opens no device and does not produce hardware sound.
        let callback = std::thread::spawn(move || {
            let mut left = [0.; 128];
            let mut right = [0.; 128];
            while !finished.load(Ordering::Acquire) {
                host.render(&mut left, &mut right);
                std::thread::sleep(Duration::from_millis(2));
            }
        });
        let (out, inbox) = Outbox::pair(shared.lock().unwrap().global());
        let play = new_id().unwrap();
        Service::dispatch(
            &shared,
            &operator,
            &out,
            rpc_frame(&operator, &play, json!({"type":"play"}), json!({})),
        )
        .unwrap();
        let play_reply = rpc_reply(&inbox, &play);
        assert_eq!(play_reply["status"], "accepted", "{play_reply}");
        let (entered, ready) = sync_channel(1);
        let (release, wait) = sync_channel(1);
        let wait = Mutex::new(wait);
        shared.lock().unwrap().edit_hook = Some(Arc::new(move || {
            entered.send(()).unwrap();
            wait.lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
        }));
        let mut candidate = shared.lock().unwrap().authority.session.tracks[0].clone();
        candidate.name = "Prepared vocal".into();
        candidate.mix_automation.insert(
            "pan".into(),
            (0..100000)
                .map(|frame| soundroom_engine::model::AutomationPoint {
                    frame,
                    value: 0.5,
                    curve: "linear".into(),
                    curve_start: 0.,
                    curve_end: 1.,
                    extra: BTreeMap::new(),
                })
                .collect(),
        );
        let candidate = wire(&candidate).unwrap();
        let edit = new_id().unwrap();
        Service::dispatch(
            &shared,
            &operator,
            &out,
            rpc_frame(
                &operator,
                &edit,
                json!({"type":"replaceTracks","tracks":[candidate],"name":"Prepare vocal"}),
                json!({id.clone():0}),
            ),
        )
        .unwrap();
        ready.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(
            shared.try_lock().is_ok(),
            "Prepared worker cannot own control mutex"
        );
        // Use the same socket outbox, real Service dispatch/ledger and actual
        // callback acknowledgement; testing backend.execute alone is insufficient.
        let stopped = new_id().unwrap();
        Service::dispatch(
            &shared,
            &operator,
            &out,
            rpc_frame(&operator, &stopped, json!({"type":"stop"}), json!({})),
        )
        .unwrap();
        assert_eq!(rpc_reply(&inbox, &stopped)["status"], "accepted");
        assert!(matches!(
            shared.lock().unwrap().authority.transport,
            soundroom_engine::authority::Transport::Stopped { .. }
        ));
        release.send(()).unwrap();
        assert_eq!(rpc_reply(&inbox, &edit)["status"], "accepted");
        let state = shared.lock().unwrap();
        assert_eq!(state.authority.session.tracks[0].name, "Prepared vocal");
        assert_eq!(
            state.authority.session.tracks[0].mix_automation["pan"].len(),
            100000
        );
        assert!(matches!(
            state.authority.transport,
            soundroom_engine::authority::Transport::Stopped { .. }
        ));
        assert_eq!(
            state.authority.history_state(&operator.runtime).0,
            Some("Prepare vocal")
        );
        drop(state);
        // Dense authored state must now become an actual callback-confirmed
        // graph, with compact immutable lanes keeping the memory cap intact.
        let play = new_id().unwrap();
        Service::dispatch(
            &shared,
            &operator,
            &out,
            rpc_frame(&operator, &play, json!({"type":"play"}), json!({})),
        )
        .unwrap();
        let receipt = rpc_reply(&inbox, &play);
        assert_eq!(receipt["status"], "accepted", "{receipt}");
        assert!(
            shared
                .lock()
                .unwrap()
                .backend
                .as_ref()
                .unwrap()
                .inner_telemetry_for_test()
                .unwrap()
                .playing
        );
        let halt = new_id().unwrap();
        Service::dispatch(
            &shared,
            &operator,
            &out,
            rpc_frame(&operator, &halt, json!({"type":"stop"}), json!({})),
        )
        .unwrap();
        let receipt = rpc_reply(&inbox, &halt);
        assert_eq!(receipt["status"], "accepted", "{receipt}");
        stop.store(true, Ordering::Release);
        callback.join().unwrap();
        drop(shared);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn prepared_worker_rechecks_cancellation_and_revoked_role_without_authored_side_effects() {
        for revoke in [false, true] {
            let (root, shared, editor, id) = control_fixture(0);
            let (out, inbox) = Outbox::pair(shared.lock().unwrap().global());
            let (entered, ready) = sync_channel(1);
            let (release, wait) = sync_channel(1);
            let wait = Mutex::new(wait);
            shared.lock().unwrap().edit_hook = Some(Arc::new(move || {
                entered.send(()).unwrap();
                wait.lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
            }));
            let edit = new_id().unwrap();
            Service::dispatch(
                &shared,
                &editor,
                &out,
                rpc_frame(
                    &editor,
                    &edit,
                    json!({"type":"setTrackName","trackID":id,"name":"Never committed"}),
                    json!({id.clone():0}),
                ),
            )
            .unwrap();
            ready.recv_timeout(Duration::from_secs(2)).unwrap();
            let mut state = shared.lock().unwrap();
            if revoke {
                state.registry.revoke(&editor.credential).unwrap();
            } else {
                state.pending[&(
                    editor.credential.clone(),
                    editor.runtime.clone(),
                    edit.to_ascii_uppercase(),
                )]
                    .cancel
                    .store(true, Ordering::Release);
            }
            drop(state);
            release.send(()).unwrap();
            assert_eq!(rpc_reply(&inbox, &edit)["status"], "rejected");
            let state = shared.lock().unwrap();
            assert_eq!(state.authority.revision, 0);
            assert_ne!(state.authority.session.tracks[0].name, "Never committed");
            assert!(state.authority.history_state(&editor.runtime).0.is_none());
            drop(state);
            drop(shared);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn transport_interrupt_jobs_have_independent_peer_and_global_reserves() {
        let (root, shared, editor, _) = control_fixture(0);
        let mut state = shared.lock().unwrap();
        let mut peer = editor.clone();
        peer.runtime = new_id().unwrap();
        let entry = |critical: bool| Pending {
            authored: !critical,
            critical,
            transport_start: false,
            write: false,
            save: false,
            waveform: false,
            digest: [0; 32],
            cancel: Arc::new(AtomicBool::new(false)),
        };
        for _ in 0..4 {
            state.pending.insert(
                (
                    editor.credential.clone(),
                    editor.runtime.clone(),
                    new_id().unwrap(),
                ),
                entry(false),
            );
        }
        assert!(!state.admits_job(&editor, false));
        assert!(state.admits_job(&editor, true));
        for _ in 0..2 {
            state.pending.insert(
                (
                    editor.credential.clone(),
                    editor.runtime.clone(),
                    new_id().unwrap(),
                ),
                entry(true),
            );
        }
        assert!(!state.admits_job(&editor, true));
        assert!(state.admits_job(&peer, true));
        for _ in 0..6 {
            state.pending.insert(
                (
                    peer.credential.clone(),
                    new_id().unwrap(),
                    new_id().unwrap(),
                ),
                entry(true),
            );
        }
        assert!(!state.admits_job(&peer, true));
        state.pending.clear();
        drop(state);
        drop(shared);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn current_role_is_checked_before_cached_receipt_replay() {
        let (root, shared, editor, _) = control_fixture(0);
        let (out, inbox) = Outbox::pair(shared.lock().unwrap().global());
        let request = new_id().unwrap();
        let command = rpc_frame(
            &editor,
            &request,
            json!({"type":"ping","clientTime":0}),
            json!({}),
        );
        Service::dispatch(&shared, &editor, &out, command.clone()).unwrap();
        assert_eq!(rpc_reply(&inbox, &request)["status"], "accepted");
        let mut changed = editor.clone();
        changed.role = RemoteRole::Operator;
        let error = Service::dispatch(&shared, &changed, &out, command).unwrap_err();
        assert!(error.to_string().contains("role changed"));
        assert!(inbox.priority.try_recv().is_err());
        assert_eq!(shared.lock().unwrap().authority.revision, 0);
        drop(shared);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn prepared_worker_rejects_changed_custody_file_before_authority_commit() {
        let (root, shared, editor, id) = control_fixture(0);
        let asset = new_id().unwrap();
        let path = root.join(format!("{asset}.wav"));
        let mut writer = soundroom_engine::wave::WaveWriter::create(&path, 1).unwrap();
        writer.write(&[0.], &[0.]).unwrap();
        writer.finish().unwrap();
        let info = soundroom_engine::wave::WaveReader::open(&path)
            .unwrap()
            .info;
        shared.lock().unwrap().media.insert(
            asset,
            Media {
                path: path.clone(),
                info,
            },
        );
        let (out, inbox) = Outbox::pair(shared.lock().unwrap().global());
        let (entered, ready) = sync_channel(1);
        let (release, wait) = sync_channel(1);
        let wait = Mutex::new(wait);
        shared.lock().unwrap().edit_hook = Some(Arc::new(move || {
            entered.send(()).unwrap();
            wait.lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
        }));
        let edit = new_id().unwrap();
        Service::dispatch(
            &shared,
            &editor,
            &out,
            rpc_frame(
                &editor,
                &edit,
                json!({"type":"setTrackName","trackID":id,"name":"Never committed"}),
                json!({id:0}),
            ),
        )
        .unwrap();
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        // Change the admitted identity/length after full worker validation.
        // Source paths come from custody, never directly from the request.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&[0])
            .unwrap();
        release.send(()).unwrap();
        let reply = rpc_reply(&inbox, &edit);
        assert_eq!(reply["status"], "conflict", "{reply}");
        assert!(reply["message"]
            .as_str()
            .unwrap()
            .contains("Source file changed"));
        let state = shared.lock().unwrap();
        assert_eq!(state.authority.revision, 0);
        assert_ne!(state.authority.session.tracks[0].name, "Never committed");
        assert!(state.authority.history_state(&editor.runtime).0.is_none());
        drop(state);
        drop(shared);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn callback_stop_acknowledgement_does_not_wait_for_control_mutex() {
        let root = std::env::temp_dir().join(format!("soundroom-stop-{}", new_id().unwrap()));
        private_directory(&root).unwrap();
        Registry::create(&root, "Stop").unwrap();
        let shared = Service::open(&root, None).unwrap();
        let (control, mut host) = soundroom_engine::realtime::channel();
        Service::attach_audio(&shared, control).unwrap();
        let backend = shared.lock().unwrap().backend.as_ref().unwrap().clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let callback = std::thread::spawn(move || {
            let mut left = [0.; 64];
            let mut right = [0.; 64];
            while !stopped.load(Ordering::Acquire) {
                host.render(&mut left, &mut right);
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        let play = backend
            .execute(
                crate::audio::Command::Play,
                Box::new(Project {
                    session: Session::default(),
                    media: BTreeMap::new(),
                }),
                0,
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
        assert!(play.playing);
        // Simulate an admitted long authored validation holding authority. A queued Stop
        // must still reach the callback and complete its gain ramp on the driver worker.
        let held = shared.lock().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let paused = backend
            .execute(
                crate::audio::Command::Pause,
                Box::new(Project {
                    session: Session::default(),
                    media: BTreeMap::new(),
                }),
                0,
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
        assert!(!paused.playing);
        drop(held);
        stop.store(true, Ordering::Release);
        callback.join().unwrap();
        drop(shared);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn immutable_body_has_exact_limit_independent_of_live_overlay_growth() {
        let global = Arc::new(AtomicUsize::new(0));
        let mut snapshot = json!({"serverID":new_id().unwrap(),"serverName":"Host",
            "session":{"version":9,"name":"","tracks":[]},"revision":5,"trackRevisions":{},
            "transport":{"frame":0,"isPlaying":false,"isRecording":false,"isPreparing":false,"hostTime":0},
            "devices":[],"inputDeviceID":0,"outputDeviceID":0,"assets":[],"status":"","peers":[]});
        let empty_bytes = encoded_size(&snapshot).unwrap();
        snapshot["session"]["name"] =
            json!("x".repeat(validation::MAX_SNAPSHOT_BYTES - empty_bytes));
        // Admission and encoding use exactly the same neutral body. Even metadata
        // which changed after admission cannot overflow that immutable byte budget.
        snapshot["status"] = json!("\n".repeat(512));
        snapshot["transport"]["frame"] = json!(soundroom_engine::model::MAX_FRAME);
        snapshot["transport"]["isPlaying"] = json!(true);
        snapshot["peers"] = json!([{ "id":new_id().unwrap(),"name":"Owner","role":"operator"}]);
        let document = SnapshotDocument::prepare(snapshot, global.clone()).unwrap();
        assert_eq!(document.bytes.len(), validation::MAX_SNAPSHOT_BYTES);
        assert_eq!(
            global.load(Ordering::Acquire),
            validation::MAX_SNAPSHOT_BYTES
        );
        assert_eq!(document.bytes.iter().filter(|b| **b == b'\n').count(), 0);
        drop(document);
        assert_eq!(global.load(Ordering::Acquire), 0);
    }
    #[test]
    fn oversized_live_event_rejects_without_retaining_or_displacing_latest_metadata() {
        let (out, inbox) = Outbox::pair(Arc::new(AtomicUsize::new(0)));
        out.send(
            &json!({"version":3,"kind":"event","event":{"type":"status","message":"Fresh"}}),
            false,
        )
        .unwrap();
        assert!(out.send(&json!({"version":3,"kind":"event","event":{"type":"status","message":"x".repeat(LIVE_EVENT_BYTES)}}),false).is_err());
        assert_eq!(
            out.live.lock().unwrap()["status"]["event"]["message"],
            "Fresh"
        );
        let first = inbox.ordinary.try_recv().unwrap();
        inbox.received(first.len());
        assert!(inbox.ordinary.try_recv().is_err());
    }
    #[test]
    fn streamed_live_overlays_replay_after_begin_and_never_overtake_pages() {
        use soundroom_client::{models::RemoteSnapshotPage, snapshot::SnapshotAssembler};
        let global = Arc::new(AtomicUsize::new(0));
        let (out, inbox) = Outbox::pair(global.clone());
        let snapshot = json!({"serverID":new_id().unwrap(),"serverName":"Host",
            "session":{"version":9,"name":"Song","tracks":(0..20).map(|_|json!({"id":new_id().unwrap(),"name":"x".repeat(60000)})).collect::<Vec<_>>()},
            "revision":5,"trackRevisions":{},"transport":{"frame":0,"isPlaying":false,"isRecording":false,"isPreparing":false,"hostTime":0},
            "devices":[],"inputDeviceID":0,"outputDeviceID":0,"assets":[],"status":"Captured earlier","peers":[]});
        validation::validate_model(&snapshot, "RemoteSnapshot").unwrap();
        out.send(
            &json!({"version":3,"kind":"event","event":{"type":"status","message":"Before begin"}}),
            true,
        )
        .unwrap();
        let first = inbox.ordinary.recv_timeout(Duration::from_secs(1)).unwrap();
        inbox.received(first.len());
        assert!(inbox.priority.try_recv().is_err());
        send_snapshot(&out, snapshot).unwrap();
        let begin = inbox.ordinary.recv_timeout(Duration::from_secs(2)).unwrap();
        inbox.received(begin.len());
        let begin: RemoteSnapshotPage = serde_json::from_slice(&begin).unwrap();
        assert_eq!(begin.kind, soundroom_client::models::Kind::SnapshotBegin);
        let mut assembler = SnapshotAssembler::default();
        assembler.accept(begin).unwrap();
        // Give the scheduler time to queue the final page. Metadata must stay FIFO
        // even when its active flag has already become false before socket draining.
        let deadline = Instant::now() + Duration::from_secs(2);
        while out.snapshot_active.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        out.send(&json!({"version":3,"kind":"event","event":{"type":"status","message":"After queued end"}}),true).unwrap();
        assert!(inbox.priority.try_recv().is_err());
        let mut statuses = Vec::new();
        let mut installed = None;
        loop {
            let bytes = inbox.ordinary.recv_timeout(Duration::from_secs(2)).unwrap();
            inbox.received(bytes.len());
            let value: Value = serde_json::from_slice(&bytes).unwrap();
            if value["kind"] == "event" {
                if value["event"]["type"] != "status" {
                    continue;
                }
                statuses.push(value["event"]["message"].as_str().unwrap().to_owned());
                if statuses.last().unwrap() == "After queued end" {
                    break;
                }
            } else {
                if let Some(snapshot) = assembler
                    .accept(serde_json::from_value(value).unwrap())
                    .unwrap()
                {
                    installed = Some(snapshot);
                }
            }
        }
        assert_eq!(statuses, vec!["Before begin", "After queued end"]);
        assert_eq!(installed.unwrap().revision, 5);
        drop(out);
        let deadline = Instant::now() + Duration::from_secs(2);
        while global.load(Ordering::Acquire) != 0 {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
}

#[cfg(test)]
mod edge_protection_tests {
    use super::*;
    #[test]
    fn loaded_clips_without_edge_settings_get_the_default_fade() {
        let mut session = Session::default();
        let unset = Clip {
            id: "C".into(),
            asset_id: "A".into(),
            name: "Take".into(),
            start: 0,
            offset: 0,
            length: 1000,
            edge_fade_frames: None,
            edge_protection_disabled: None,
            cut_fade_in_frames: None,
            cut_fade_out_frames: None,
            fade_in_frames: None,
            fade_out_frames: None,
            fade_reference_offset: None,
            fade_reference_length: None,
            render_envelope: None,
            previous_states: vec![],
            extra: BTreeMap::new(),
        };
        let mut explicit = unset.clone();
        explicit.edge_fade_frames = Some(0);
        let mut disabled = unset.clone();
        disabled.edge_protection_disabled = Some(true);
        session.tracks.push(Track {
            clips: vec![unset, explicit, disabled],
            ..Track::default()
        });
        protect_unset_clip_edges(&mut session);
        let clips = &session.tracks[0].clips;
        assert_eq!(
            (
                clips[0].edge_fade_frames,
                clips[0].cut_fade_in_frames,
                clips[0].cut_fade_out_frames
            ),
            (Some(96), Some(96), Some(96))
        );
        assert_eq!(
            clips[1].edge_fade_frames,
            Some(0),
            "An explicit setting is kept"
        );
        assert_eq!(
            clips[2].edge_fade_frames, None,
            "Disabled protection is kept"
        );
    }
}

#[cfg(test)]
mod device_selection_tests {
    use super::*;

    #[test]
    fn omitted_device_selectors_preserve_the_existing_routes() {
        let root = std::env::temp_dir().join(format!("soundroom-device-{}", new_id().unwrap()));
        private_directory(&root).unwrap();
        let mut registry = Registry::create(&root, "Device selection regression").unwrap();
        let credential = registry.issue("Operator", RemoteRole::Operator).unwrap();
        let identity = Identity {
            runtime: new_id().unwrap(),
            credential: credential["credentialID"].as_str().unwrap().into(),
            name: "Operator".into(),
            role: RemoteRole::Operator,
        };
        let shared = Service::open(&root, None).unwrap();
        {
            let mut state = shared.lock().unwrap();
            state.input_uid = Some("saved-microphone".into());
            state.output_uid = Some("saved-speakers".into());
        }
        let (out, inbox) = Outbox::pair(shared.lock().unwrap().global());
        // The wire model maps both omitted selectors and null to None. Neither
        // means reset to the system default. No device is opened by these calls.
        for command in [
            json!({"type":"setDevices","outputUID":null}),
            json!({"type":"setDevices","inputUID":null}),
            json!({"type":"setDevices"}),
        ] {
            let id = new_id().unwrap();
            Service::dispatch(
                &shared,
                &identity,
                &out,
                json!({"version":3,"kind":"request","id":id,"clientID":identity.runtime,
                    "expectedTrackRevisions":{},"command":command}),
            )
            .unwrap();
            let bytes = inbox.priority.recv_timeout(Duration::from_secs(2)).unwrap();
            inbox.received(bytes.len());
            let reply: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(reply["requestID"], id);
            assert_eq!(reply["status"], "accepted", "{reply}");
            let state = shared.lock().unwrap();
            assert_eq!(state.input_uid.as_deref(), Some("saved-microphone"));
            assert_eq!(state.output_uid.as_deref(), Some("saved-speakers"));
        }
        #[cfg(target_os = "macos")]
        {
            let id = new_id().unwrap();
            Service::dispatch(
                &shared,
                &identity,
                &out,
                json!({"version":3,"kind":"request","id":id,"clientID":identity.runtime,
                    "expectedTrackRevisions":{},"command":{"type":"setDevices",
                        "inputUID":"soundroom-unavailable-regression-input"}}),
            )
            .unwrap();
            let bytes = inbox.priority.recv_timeout(Duration::from_secs(2)).unwrap();
            inbox.received(bytes.len());
            let reply: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(reply["requestID"], id);
            assert_eq!(reply["status"], "conflict", "{reply}");
            assert_eq!(reply["message"], "Input device is unavailable");
            let state = shared.lock().unwrap();
            assert_eq!(state.input_uid.as_deref(), Some("saved-microphone"));
            assert_eq!(state.output_uid.as_deref(), Some("saved-speakers"));
        }
        drop(shared);
        std::fs::remove_dir_all(root).unwrap();
    }
}
