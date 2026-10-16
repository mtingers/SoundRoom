//! One receipt-driven transfer per client. Disk/hash work never blocks control polling.
use base64::Engine;
use sha2::{Digest, Sha256};
use soundroom_client::{auth::new_id, models::*, validation};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{Read, Seek, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
pub const CHUNK: usize = 262144;
#[derive(Clone)]
pub enum TransferRequest {
    Import {
        path: PathBuf,
        track_id: Option<String>,
        frame: i64,
        revision: i64,
        tracks: BTreeMap<String, i64>,
        maximum_bytes: u64,
        chunk_bytes: usize,
    },
    Export {
        path: PathBuf,
        name: String,
    },
    ProjectExport {
        path: Option<PathBuf>,
        name: String,
        format: String,
        context: RemoteEditorContext,
        tails: bool,
        tracks: Option<Vec<String>>,
    },
    Download {
        path: PathBuf,
        artifact: RemoteArtifact,
    },
}
#[derive(Clone, Debug)]
pub struct TransferView {
    pub label: String,
    pub phase: String,
    pub completed: u64,
    pub total: u64,
    pub active: bool,
    pub importing: bool,
    pub request_id: Option<String>,
}
#[derive(Clone, Copy, PartialEq, Debug)]
enum Stage {
    Preparing,
    Begin,
    Reading,
    Chunk,
    Finish,
    Export,
    Opening,
    Download,
    Writing,
    Installing,
}
pub struct Action {
    pub command: RemoteCommand,
    pub revision: Option<i64>,
    pub tracks: BTreeMap<String, i64>,
    pub authored: bool,
    pub timeout: Duration,
}
struct Waiting {
    id: String,
    stage: Stage,
    deadline: Instant,
}
struct Job {
    request: TransferRequest,
    id: String,
    stage: Stage,
    waiting: Option<Waiting>,
    io: DiskActor,
    bytes: u64,
    offset: u64,
    name: String,
    digest: String,
    chunk: usize,
}
#[derive(Default)]
pub struct Transfers {
    job: Option<Job>,
    pub view: Option<TransferView>,
    pub failure: Option<(bool, bool, String)>,
    pub receipt: Option<RemoteReply>,
    pub artifacts: Vec<RemoteArtifact>,
}
impl Transfers {
    pub fn start(&mut self, request: TransferRequest) -> Result<(), String> {
        if self.job.is_some() {
            return Err("A local transfer is already active; :transfer-cancel stops it.".into());
        }
        let path = match &request {
            TransferRequest::Import { path, .. }
            | TransferRequest::Export { path, .. }
            | TransferRequest::Download { path, .. } => path,
            TransferRequest::ProjectExport { path, .. } => path
                .as_deref()
                .unwrap_or(std::path::Path::new("Host stems")),
        };
        if path.as_os_str().len() > 4096 {
            return Err("Local file path exceeds 4096 bytes.".into());
        }
        let id = new_id().map_err(|e| e.to_string())?;
        let disk = DiskActor::new()?;
        let (stage, bytes, name, digest, chunk) = match &request {
            TransferRequest::Import {
                maximum_bytes,
                chunk_bytes,
                ..
            } => {
                if *maximum_bytes == 0 || *chunk_bytes == 0 {
                    return Err("Host upload limits are unavailable.".into());
                }
                disk.send(DiskWork::Prepare {
                    path: path.to_path_buf(),
                    maximum: (*maximum_bytes).min(2_147_483_648),
                })?;
                (
                    Stage::Preparing,
                    0,
                    String::new(),
                    String::new(),
                    (*chunk_bytes).min(CHUNK),
                )
            }
            TransferRequest::Export { name, .. } => {
                validate_name(name)?;
                (Stage::Export, 0, name.clone(), String::new(), CHUNK)
            }
            TransferRequest::ProjectExport {
                name,
                format,
                context,
                tracks,
                path,
                ..
            } => {
                validate_name(name)?;
                if !["wav", "aiff", "m4a"].contains(&format.as_str()) {
                    return Err("Unsupported export format.".into());
                }
                if let Some(ids) = tracks {
                    let unique: std::collections::BTreeSet<_> =
                        ids.iter().map(|s| s.to_ascii_uppercase()).collect();
                    if path.is_some()
                        || ids.is_empty()
                        || ids.len() > 16
                        || unique.len() != ids.len()
                        || ids.iter().any(|s| !validation::uuid(s))
                    {
                        return Err("Stem export requires one to sixteen distinct tracks and produces host manifests.".into());
                    }
                } else if path.is_none() {
                    return Err("Mix export requires a local destination.".into());
                }
                if context
                    .range_start
                    .zip(context.range_end)
                    .is_some_and(|(a, b)| a < 0 || b <= a)
                    || context.range_start.is_some() != context.range_end.is_some()
                {
                    return Err("Invalid export selection.".into());
                }
                (Stage::Export, 0, name.clone(), String::new(), CHUNK)
            }
            TransferRequest::Download { artifact, .. } => {
                validate_artifact(artifact)?;
                disk.send(DiskWork::Open {
                    path: path.to_path_buf(),
                    artifact: artifact.clone(),
                })?;
                (
                    Stage::Opening,
                    artifact.bytes as u64,
                    artifact.name.clone(),
                    artifact.sha256.clone(),
                    CHUNK,
                )
            }
        };
        self.view = Some(TransferView {
            label: path
                .file_name()
                .map_or_else(|| "Local transfer".into(), |s| s.to_string_lossy().into()),
            phase: if stage == Stage::Preparing {
                "Checking source"
            } else if stage == Stage::Export {
                "Rendering on host"
            } else {
                "Opening destination"
            }
            .into(),
            completed: 0,
            total: bytes,
            active: true,
            importing: false,
            request_id: None,
        });
        self.failure = None;
        self.receipt = None;
        self.job = Some(Job {
            request,
            id,
            stage,
            waiting: None,
            io: disk,
            bytes,
            offset: 0,
            name,
            digest,
            chunk,
        });
        Ok(())
    }
    pub fn next(&mut self, allow_finish: bool) -> Result<Option<Action>, String> {
        let Some(job) = self.job.as_mut() else {
            return Ok(None);
        };
        if let Some(wait) = &job.waiting {
            if Instant::now() >= wait.deadline {
                let id = wait.id.clone();
                let authored = wait.stage == Stage::Finish;
                self.fail(authored,true,format!("Outcome uncertain for transfer request {id}; refresh before retrying. No request is replayed."));
            }
            return Ok(None);
        }
        let result = match job.io.results.try_recv() {
            Ok(value) => Some(value),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.fail(false, false, "Local file worker stopped.".into());
                return Ok(None);
            }
        };
        if let Some(result) = result {
            match result {
                Err(error) => {
                    self.fail(false, false, error);
                    return Ok(None);
                }
                Ok(DiskResult::Prepared {
                    name,
                    bytes,
                    digest,
                }) => {
                    job.name = name;
                    job.bytes = bytes;
                    job.digest = digest;
                    job.stage = Stage::Begin;
                }
                Ok(DiskResult::Read(bytes)) => {
                    if bytes.is_empty()
                        || bytes.len() > job.chunk
                        || job.offset + bytes.len() as u64 > job.bytes
                    {
                        self.fail(
                            false,
                            false,
                            "Import source changed during transfer.".into(),
                        );
                        return Ok(None);
                    }
                    job.stage = Stage::Chunk;
                    let action = Action {
                        command: RemoteCommand::UploadChunk {
                            id: job.id.clone(),
                            offset: job.offset as i64,
                            data: base64::engine::general_purpose::STANDARD.encode(&bytes),
                        },
                        revision: None,
                        tracks: BTreeMap::new(),
                        authored: false,
                        timeout: Duration::from_secs(30),
                    };
                    job.offset += bytes.len() as u64;
                    return Ok(Some(action));
                }
                Ok(DiskResult::Opened) => {
                    job.stage = Stage::Download;
                }
                Ok(DiskResult::Written(bytes)) => {
                    job.offset += bytes as u64;
                    job.stage = if job.offset == job.bytes {
                        Stage::Installing
                    } else {
                        Stage::Download
                    };
                }
                Ok(DiskResult::Installed) => {
                    self.complete("Verified download saved without replacing an existing file.");
                    return Ok(None);
                }
            }
        }
        if let Some(view) = self.view.as_mut() {
            view.completed = job.offset;
            view.total = job.bytes;
        }
        match job.stage {
            Stage::Begin => Ok(Some(Action {
                command: RemoteCommand::BeginUpload {
                    id: job.id.clone(),
                    name: job.name.clone(),
                    bytes: job.bytes as i64,
                    sha256: job.digest.clone(),
                },
                revision: None,
                tracks: BTreeMap::new(),
                authored: false,
                timeout: Duration::from_secs(30),
            })),
            Stage::Export => Ok(Some(Action {
                command: match &job.request {
                    TransferRequest::ProjectExport {
                        format,
                        context,
                        tails,
                        tracks,
                        ..
                    } => RemoteCommand::Editor {
                        intent: RemoteEditorIntent {
                            operation: "project.export".into(),
                            context: context.clone(),
                            integers: BTreeMap::new(),
                            numbers: BTreeMap::new(),
                            strings: BTreeMap::from([
                                ("name".into(), job.name.clone()),
                                ("format".into(), format.clone()),
                            ]),
                            flags: BTreeMap::from([
                                ("tails".into(), *tails),
                                ("selection".into(), context.range_start.is_some()),
                                ("tracks".into(), tracks.is_some()),
                            ]),
                            payload: tracks
                                .as_ref()
                                .map(|ids| {
                                    serde_json::to_vec(ids).map(|b| {
                                        base64::engine::general_purpose::STANDARD.encode(b)
                                    })
                                })
                                .transpose()
                                .map_err(|e| e.to_string())?,
                        },
                    },
                    _ => RemoteCommand::ExportMix {
                        name: job.name.clone(),
                    },
                },
                revision: None,
                tracks: BTreeMap::new(),
                authored: false,
                timeout: Duration::from_secs(600),
            })),
            Stage::Download => Ok(Some(Action {
                command: RemoteCommand::DownloadArtifact {
                    id: job.id.clone(),
                    offset: job.offset as i64,
                    bytes: job.chunk.min((job.bytes - job.offset) as usize) as i64,
                },
                revision: None,
                tracks: BTreeMap::new(),
                authored: false,
                timeout: Duration::from_secs(30),
            })),
            Stage::Finish if !allow_finish => Ok(None),
            Stage::Finish => {
                let TransferRequest::Import {
                    track_id,
                    frame,
                    revision,
                    tracks,
                    ..
                } = &job.request
                else {
                    return Err("Invalid transfer phase.".into());
                };
                Ok(Some(Action {
                    command: RemoteCommand::FinishUpload {
                        id: job.id.clone(),
                        track_id: track_id.clone(),
                        frame: *frame,
                    },
                    revision: track_id.is_none().then_some(*revision),
                    tracks: tracks.clone(),
                    authored: true,
                    timeout: Duration::from_secs(600),
                }))
            }
            Stage::Installing => {
                job.io.send(DiskWork::Install)?;
                job.stage = Stage::Writing;
                self.phase("Verifying and saving");
                Ok(None)
            }
            _ => Ok(None),
        }
    }
    pub fn sent(&mut self, id: String, timeout: Duration) {
        if let Some(job) = self.job.as_mut() {
            let stage = job.stage;
            job.waiting = Some(Waiting {
                id: id.clone(),
                stage,
                deadline: Instant::now() + timeout,
            });
            if let Some(v) = self.view.as_mut() {
                v.request_id = Some(id);
                v.importing = stage == Stage::Finish;
                v.phase = match stage {
                    Stage::Begin => "Starting upload",
                    Stage::Chunk => "Uploading",
                    Stage::Finish => "Importing on host",
                    Stage::Export => "Rendering on host",
                    Stage::Download => "Downloading",
                    _ => "Working",
                }
                .into();
            }
        }
    }
    pub fn response(&mut self, reply: &RemoteReply) -> bool {
        let Some(job) = self.job.as_mut() else {
            return false;
        };
        if job.waiting.as_ref().map(|w| w.id.as_str()) != Some(reply.request_id.as_str()) {
            return false;
        }
        let stage = job.waiting.take().unwrap().stage;
        if reply.status != RemoteReplyStatus::Accepted {
            self.fail(stage == Stage::Finish, false, reply.message.clone());
            return true;
        }
        if stage == Stage::Finish {
            self.receipt = Some(reply.clone());
            self.complete("Audio imported on the host.");
            return true;
        }
        if stage == Stage::Export
            && matches!(
                &job.request,
                TransferRequest::ProjectExport {
                    tracks: Some(_),
                    ..
                }
            )
        {
            let result = (|| -> Result<Vec<RemoteArtifact>, String> {
                let artifacts: Vec<RemoteArtifact> = decode_payload(reply, "[RemoteArtifact]")?;
                let expected = if let TransferRequest::ProjectExport {
                    tracks: Some(ids), ..
                } = &job.request
                {
                    ids.len()
                } else {
                    0
                };
                if artifacts.is_empty() || artifacts.len() > 16 || artifacts.len() != expected {
                    return Err("Host returned an invalid stem manifest count.".into());
                }
                let mut ids = std::collections::BTreeSet::new();
                for a in &artifacts {
                    validate_artifact(a)?;
                    if !ids.insert(a.id.to_ascii_uppercase()) {
                        return Err("Host returned duplicate stem artifact IDs.".into());
                    }
                }
                Ok(artifacts)
            })();
            match result {
                Ok(artifacts) => {
                    let count = artifacts.len();
                    for a in artifacts {
                        if self.artifacts.len() == 16 {
                            self.artifacts.remove(0);
                        }
                        self.artifacts.push(a);
                    }
                    self.complete(&format!("{count} stems rendered on host. :files lists manifests; :download saves each verified file."));
                }
                Err(e) => self.fail(false, false, e),
            }
            return true;
        }
        let result = (|| -> Result<(), String> {
            match stage {
                Stage::Begin | Stage::Chunk => {
                    if job.offset == job.bytes {
                        job.stage = Stage::Finish;
                    } else {
                        job.io.send(DiskWork::Read {
                            offset: job.offset,
                            bytes: job.chunk.min((job.bytes - job.offset) as usize),
                        })?;
                        job.stage = Stage::Reading;
                    }
                }
                Stage::Export => {
                    let artifact: RemoteArtifact = decode_payload(reply, "RemoteArtifact")?;
                    validate_artifact(&artifact)?;
                    if self.artifacts.len() == 16 {
                        self.artifacts.remove(0);
                    }
                    self.artifacts.push(artifact.clone());
                    let path = match &job.request {
                        TransferRequest::Export { path, .. } => path.clone(),
                        TransferRequest::ProjectExport {
                            path: Some(path), ..
                        } => path.clone(),
                        _ => return Err("Invalid export state.".into()),
                    };
                    job.io.send(DiskWork::Open {
                        path,
                        artifact: artifact.clone(),
                    })?;
                    job.id = artifact.id;
                    job.name = artifact.name;
                    job.bytes = artifact.bytes as u64;
                    job.digest = artifact.sha256;
                    job.stage = Stage::Opening;
                }
                Stage::Download => {
                    let chunk: RemoteArtifactChunk = decode_payload(reply, "RemoteArtifactChunk")?;
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(&chunk.data)
                        .map_err(|_| "Invalid artifact bytes.")?;
                    let wanted = job.chunk.min((job.bytes - job.offset) as usize);
                    if chunk.offset != job.offset as i64
                        || bytes.is_empty()
                        || bytes.len() > wanted
                        || chunk.complete != (job.offset + bytes.len() as u64 == job.bytes)
                    {
                        return Err("Artifact offset or completion flag does not match.".into());
                    }
                    job.io.send(DiskWork::Write {
                        offset: job.offset,
                        bytes,
                    })?;
                    job.stage = Stage::Writing;
                }
                _ => return Err("Unexpected transfer receipt.".into()),
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.fail(stage == Stage::Finish, false, error);
        }
        true
    }
    pub fn pending_id(&self) -> Option<&str> {
        self.job.as_ref()?.waiting.as_ref().map(|w| w.id.as_str())
    }
    pub fn pending_authored(&self) -> bool {
        self.job
            .as_ref()
            .and_then(|j| j.waiting.as_ref())
            .is_some_and(|w| w.stage == Stage::Finish)
    }
    pub fn disconnect(&mut self, uncertain: &[String]) {
        if let Some(job) = self.job.as_ref() {
            let wait = job.waiting.as_ref();
            let sent = wait.is_some_and(|w| uncertain.contains(&w.id));
            let authored = wait.is_some_and(|w| w.stage == Stage::Finish);
            let message = wait.map_or_else(
                || "Transfer stopped before sending the next request.".into(),
                |w| {
                    if sent {
                        format!(
                            "Outcome uncertain for transfer request {}; refresh before retrying.",
                            w.id
                        )
                    } else {
                        "Transfer stopped; its pending request was not sent.".into()
                    }
                },
            );
            self.fail(authored, sent, message);
        }
    }
    pub fn cancel(&mut self, queued_canceled: bool) {
        if let Some(job) = self.job.as_ref() {
            let authored = job
                .waiting
                .as_ref()
                .is_some_and(|w| w.stage == Stage::Finish);
            let uncertain = !queued_canceled
                && job
                    .waiting
                    .as_ref()
                    .is_some_and(|w| matches!(w.stage, Stage::Finish | Stage::Export));
            let message = if uncertain {
                format!("Local transfer canceled; host request {} may still finish. Refresh before retrying.",job.waiting.as_ref().unwrap().id)
            } else {
                "Local transfer canceled; staged uploads expire on the host.".into()
            };
            self.fail(authored, uncertain, message);
        }
    }
    pub fn reject(&mut self, authored: bool, error: String) {
        self.fail(
            authored,
            false,
            format!("Transfer request was not sent: {error}"),
        );
    }
    fn phase(&mut self, phase: &str) {
        if let Some(v) = self.view.as_mut() {
            v.phase = phase.into();
        }
    }
    fn complete(&mut self, message: &str) {
        self.job = None;
        if let Some(v) = self.view.as_mut() {
            v.active = false;
            v.importing = false;
            v.completed = v.total;
            v.phase = message.into();
        }
    }
    fn fail(&mut self, authored: bool, uncertain: bool, message: String) {
        let message: String = message.chars().take(4096).collect();
        self.job = None;
        if let Some(v) = self.view.as_mut() {
            v.active = false;
            v.importing = false;
            v.phase = message.clone();
        }
        self.failure = Some((authored, uncertain, message));
    }
}
fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
        return Err(
            "Transfer filename must contain 1–256 UTF-8 bytes without control characters.".into(),
        );
    }
    Ok(())
}
fn validate_artifact(artifact: &RemoteArtifact) -> Result<(), String> {
    validation::validate_model(
        &serde_json::to_value(artifact).map_err(|e| e.to_string())?,
        "RemoteArtifact",
    )
    .map_err(|e| e.to_string())?;
    if artifact.bytes <= 0 || artifact.bytes > 2_147_483_648 {
        return Err("Artifact must contain 1 byte to 2 GiB.".into());
    }
    if artifact.sha256.len() != 64
        || !artifact
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("Artifact requires a lowercase SHA-256 digest.".into());
    }
    validate_name(&artifact.name)
}
fn decode_payload<T: serde::de::DeserializeOwned>(
    reply: &RemoteReply,
    model: &str,
) -> Result<T, String> {
    let raw = reply
        .payload
        .as_deref()
        .ok_or("Missing transfer payload.")?;
    if raw.len() > 2 * CHUNK {
        return Err("Transfer payload exceeds its byte limit.".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw)
        .map_err(|_| "Invalid transfer payload.")?;
    validation::decode_model(&bytes, model).map_err(|e| e.to_string())
}

enum DiskWork {
    Prepare {
        path: PathBuf,
        maximum: u64,
    },
    Read {
        offset: u64,
        bytes: usize,
    },
    Open {
        path: PathBuf,
        artifact: RemoteArtifact,
    },
    Write {
        offset: u64,
        bytes: Vec<u8>,
    },
    Install,
}
enum DiskResult {
    Prepared {
        name: String,
        bytes: u64,
        digest: String,
    },
    Read(Vec<u8>),
    Opened,
    Written(usize),
    Installed,
}
struct DiskActor {
    sender: SyncSender<DiskWork>,
    results: Receiver<Result<DiskResult, String>>,
    cancel: Arc<AtomicBool>,
}
static DISK_ACTORS: AtomicUsize = AtomicUsize::new(0);
struct DiskPermit;
impl Drop for DiskPermit {
    fn drop(&mut self) {
        DISK_ACTORS.fetch_sub(1, Ordering::Relaxed);
    }
}
impl DiskActor {
    fn new() -> Result<Self, String> {
        DISK_ACTORS
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                (n < 16).then_some(n + 1)
            })
            .map_err(|_| "Canceled file workers are still stopping; try again when they finish.")?;
        let permit = DiskPermit;
        let (sender, rx) = mpsc::sync_channel(2);
        let (tx, results) = mpsc::sync_channel(2);
        let cancel = Arc::new(AtomicBool::new(false));
        let stopped = cancel.clone();
        thread::Builder::new()
            .name("soundroom-files".into())
            .spawn(move || {
                let _permit = permit;
                let mut files = DiskFiles::default();
                while !stopped.load(Ordering::Relaxed) {
                    let Ok(work) = rx.recv() else {
                        break;
                    };
                    let result = files.run(work, &stopped);
                    if tx.send(result).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| format!("Cannot start local file worker: {e}"))?;
        Ok(Self {
            sender,
            results,
            cancel,
        })
    }
    fn send(&self, work: DiskWork) -> Result<(), String> {
        self.sender
            .try_send(work)
            .map_err(|_| "Local file queue is full or closed.".into())
    }
}
impl Drop for DiskActor {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
#[derive(Default)]
struct DiskFiles {
    source: Option<File>,
    source_bytes: u64,
    source_modified: Option<std::time::SystemTime>,
    download: Option<DownloadFile>,
}
struct DownloadFile {
    file: Option<File>,
    path: PathBuf,
    destination: PathBuf,
    artifact: RemoteArtifact,
    digest: Sha256,
    offset: u64,
}
impl Drop for DownloadFile {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
    }
}
impl DiskFiles {
    fn run(&mut self, work: DiskWork, cancel: &AtomicBool) -> Result<DiskResult, String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Transfer canceled.".into());
        }
        match work {
            DiskWork::Prepare { path, maximum } => {
                let mut file = File::open(&path).map_err(|e| format!("Cannot open import: {e}"))?;
                let metadata = file.metadata().map_err(|e| e.to_string())?;
                if !metadata.is_file() || metadata.len() == 0 || metadata.len() > maximum {
                    return Err(format!(
                        "Import requires a regular file of 1 byte to {} MiB.",
                        maximum / 1024 / 1024
                    ));
                }
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or("Import filename is not UTF-8.")?
                    .to_owned();
                validate_name(&name)?;
                let mut digest = Sha256::new();
                let mut buffer = [0u8; 65536];
                let mut total = 0u64;
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        return Err("Transfer canceled.".into());
                    }
                    let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
                    if n == 0 {
                        break;
                    }
                    total += n as u64;
                    if total > metadata.len() {
                        return Err("Import file changed during hashing.".into());
                    }
                    digest.update(&buffer[..n]);
                }
                if total != metadata.len() {
                    return Err("Import file changed during hashing.".into());
                }
                file.rewind().map_err(|e| e.to_string())?;
                self.source_bytes = metadata.len();
                self.source_modified = metadata.modified().ok();
                self.source = Some(file);
                Ok(DiskResult::Prepared {
                    name,
                    bytes: total,
                    digest: hex(digest.finalize()),
                })
            }
            DiskWork::Read { offset, bytes } => {
                if bytes == 0 || bytes > CHUNK {
                    return Err("Invalid local read size.".into());
                }
                let file = self.source.as_mut().ok_or("Missing import source.")?;
                if file.stream_position().map_err(|e| e.to_string())? != offset {
                    return Err("Import offset mismatch.".into());
                }
                let mut buffer = vec![0u8; bytes];
                file.read_exact(&mut buffer)
                    .map_err(|_| "Import file changed during upload.")?;
                if offset + bytes as u64 == self.source_bytes {
                    let m = file.metadata().map_err(|e| e.to_string())?;
                    if m.len() != self.source_bytes || m.modified().ok() != self.source_modified {
                        return Err("Import file changed during upload.".into());
                    }
                }
                Ok(DiskResult::Read(buffer))
            }
            DiskWork::Open { path, artifact } => {
                validate_artifact(&artifact)?;
                if path.exists() {
                    return Err(
                        "Download destination already exists; choose a new filename.".into(),
                    );
                }
                let parent = path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or_else(|| std::path::Path::new("."));
                let temporary = parent.join(format!(
                    ".soundroom-download-{}.partial",
                    new_id().map_err(|e| e.to_string())?
                ));
                let file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temporary)
                    .map_err(|e| format!("Cannot create download: {e}"))?;
                self.download = Some(DownloadFile {
                    file: Some(file),
                    path: temporary,
                    destination: path,
                    artifact,
                    digest: Sha256::new(),
                    offset: 0,
                });
                Ok(DiskResult::Opened)
            }
            DiskWork::Write { offset, bytes } => {
                let download = self
                    .download
                    .as_mut()
                    .ok_or("Missing download destination.")?;
                if offset != download.offset
                    || bytes.is_empty()
                    || bytes.len() > CHUNK
                    || offset + bytes.len() as u64 > download.artifact.bytes as u64
                {
                    return Err("Download offset or length mismatch.".into());
                }
                download
                    .file
                    .as_mut()
                    .ok_or("Missing download file.")?
                    .write_all(&bytes)
                    .map_err(|e| format!("Cannot write download: {e}"))?;
                download.digest.update(&bytes);
                download.offset += bytes.len() as u64;
                Ok(DiskResult::Written(bytes.len()))
            }
            DiskWork::Install => {
                let mut download = self
                    .download
                    .take()
                    .ok_or("Missing download destination.")?;
                if download.offset != download.artifact.bytes as u64
                    || hex(download.digest.clone().finalize()) != download.artifact.sha256
                {
                    return Err(
                        "Artifact SHA-256 or length does not match; partial download discarded."
                            .into(),
                    );
                }
                download
                    .file
                    .as_mut()
                    .ok_or("Missing download file.")?
                    .sync_all()
                    .map_err(|e| format!("Cannot sync download: {e}"))?;
                if cancel.load(Ordering::Relaxed) {
                    return Err("Transfer canceled before installation.".into());
                }
                drop(download.file.take());
                std::fs::hard_link(&download.path, &download.destination).map_err(|e| {
                    format!("Cannot install download without replacing existing files: {e}")
                })?;
                std::fs::remove_file(&download.path).map_err(|e| e.to_string())?;
                Ok(DiskResult::Installed)
            }
        }
    }
}
fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let p =
                std::env::temp_dir().join(format!("soundroom-tui-transfer-{}", new_id().unwrap()));
            std::fs::create_dir(&p).unwrap();
            Self(p)
        }
        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
        fn clean_partial(&self) {
            let end = Instant::now() + Duration::from_secs(5);
            while std::fs::read_dir(&self.0).unwrap().any(|e| {
                e.unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".partial")
            }) {
                assert!(Instant::now() < end, "partial file was not removed");
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn next(t: &mut Transfers) -> Action {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(action) = t.next(true).unwrap() {
                return action;
            }
            assert!(t.failure.is_none(), "{:?}", t.failure);
            assert!(Instant::now() < end, "transfer stalled");
            thread::sleep(Duration::from_millis(1));
        }
    }
    fn receipt(t: &mut Transfers, status: RemoteReplyStatus, payload: Option<String>) {
        let id = t.pending_id().unwrap().to_owned();
        assert!(t.response(&RemoteReply {
            request_id: id,
            status,
            revision: 9,
            message: "accepted".into(),
            payload
        }));
    }
    fn sent(t: &mut Transfers, action: &Action) {
        t.sent(new_id().unwrap(), action.timeout);
    }
    fn payload(value: impl serde::Serialize) -> String {
        base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&value).unwrap())
    }
    fn artifact(data: &[u8]) -> RemoteArtifact {
        RemoteArtifact {
            id: new_id().unwrap(),
            name: "Mix.wav".into(),
            bytes: data.len() as i64,
            sha256: hex(Sha256::digest(data)),
        }
    }
    fn import(path: PathBuf) -> TransferRequest {
        TransferRequest::Import {
            path,
            track_id: Some("00000000-0000-0000-0000-000000000002".into()),
            frame: 48000,
            revision: 8,
            tracks: BTreeMap::from([("00000000-0000-0000-0000-000000000002".into(), 7)]),
            maximum_bytes: 2_147_483_648,
            chunk_bytes: CHUNK,
        }
    }
    fn settle(t: &mut Transfers) {
        let end = Instant::now() + Duration::from_secs(5);
        while t.view.as_ref().is_some_and(|v| v.active) {
            let action = t.next(true).unwrap();
            assert!(
                action.is_none(),
                "unexpected request after final download chunk"
            );
            assert!(Instant::now() < end);
            thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn uploads_hash_bounded_chunks_wait_for_each_receipt_and_preserve_target_revision() {
        let dir = Directory::new();
        let data = vec![7u8; CHUNK + 13];
        let path = dir.file("Voice.wav");
        std::fs::write(&path, &data).unwrap();
        let mut t = Transfers::default();
        t.start(import(path)).unwrap();
        let action = next(&mut t);
        assert!(
            matches!(&action.command,RemoteCommand::BeginUpload{bytes,sha256,..} if *bytes==data.len() as i64&&sha256==&hex(Sha256::digest(&data)))
        );
        sent(&mut t, &action);
        assert!(t.next(true).unwrap().is_none());
        receipt(&mut t, RemoteReplyStatus::Accepted, None);
        for (offset, size) in [(0, CHUNK), (CHUNK, 13)] {
            let action = next(&mut t);
            let RemoteCommand::UploadChunk {
                offset: actual,
                data,
                ..
            } = &action.command
            else {
                panic!()
            };
            assert_eq!(*actual, offset as i64);
            assert_eq!(
                base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .unwrap()
                    .len(),
                size
            );
            assert!(!action.authored);
            sent(&mut t, &action);
            receipt(&mut t, RemoteReplyStatus::Accepted, None);
        }
        assert!(t.next(false).unwrap().is_none());
        let action = next(&mut t);
        assert!(action.authored);
        assert_eq!(action.tracks.values().copied().collect::<Vec<_>>(), vec![7]);
        assert_eq!(action.revision, None);
        assert!(matches!(
            action.command,
            RemoteCommand::FinishUpload { frame: 48000, .. }
        ));
        sent(&mut t, &action);
        receipt(&mut t, RemoteReplyStatus::Accepted, None);
        assert_eq!(t.receipt.as_ref().unwrap().revision, 9);
        assert!(!t.view.as_ref().unwrap().active);
    }
    #[test]
    fn source_change_rejected_before_finish_and_new_track_uses_opening_session_revision() {
        let dir = Directory::new();
        let path = dir.file("Voice.wav");
        std::fs::write(&path, [1u8; 64]).unwrap();
        let mut t = Transfers::default();
        let mut request = import(path.clone());
        if let TransferRequest::Import {
            track_id, tracks, ..
        } = &mut request
        {
            *track_id = None;
            tracks.clear();
        }
        t.start(request).unwrap();
        let action = next(&mut t);
        sent(&mut t, &action);
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&[2]).unwrap();
        drop(file);
        receipt(&mut t, RemoteReplyStatus::Accepted, None);
        settle(&mut t);
        assert!(t.failure.as_ref().unwrap().2.contains("changed"));
        assert!(t.receipt.is_none());
        std::fs::write(&path, [1u8; 64]).unwrap();
        let mut request = import(path);
        if let TransferRequest::Import {
            track_id, tracks, ..
        } = &mut request
        {
            *track_id = None;
            tracks.clear();
        }
        t.start(request).unwrap();
        for _ in 0..2 {
            let action = next(&mut t);
            sent(&mut t, &action);
            receipt(&mut t, RemoteReplyStatus::Accepted, None);
        }
        let finish = next(&mut t);
        assert_eq!(finish.revision, Some(8));
        assert!(finish.tracks.is_empty());
    }
    #[test]
    fn downloads_validate_offsets_completion_and_hash_before_atomic_install() {
        let dir = Directory::new();
        let path = dir.file("Mix.wav");
        let data = vec![9u8; CHUNK + 27];
        let mut t = Transfers::default();
        t.start(TransferRequest::Download {
            path: path.clone(),
            artifact: artifact(&data),
        })
        .unwrap();
        for (offset, length) in [(0, CHUNK), (CHUNK, 27)] {
            let action = next(&mut t);
            assert!(
                matches!(action.command,RemoteCommand::DownloadArtifact{offset:o,bytes:b,..} if o==offset as i64&&b==length as i64)
            );
            sent(&mut t, &action);
            let chunk = RemoteArtifactChunk {
                offset: offset as i64,
                data: base64::engine::general_purpose::STANDARD
                    .encode(&data[offset..offset + length]),
                complete: offset + length == data.len(),
            };
            receipt(&mut t, RemoteReplyStatus::Accepted, Some(payload(chunk)));
        }
        settle(&mut t);
        assert!(t.failure.is_none());
        assert_eq!(std::fs::read(&path).unwrap(), data);
        dir.clean_partial();
    }
    #[test]
    fn hash_failure_cancellation_and_destination_race_leave_existing_audio_untouched() {
        for mode in ["hash", "cancel", "race", "offset", "completion"] {
            let dir = Directory::new();
            let path = dir.file("Mix.wav");
            let data = vec![4u8; 31];
            let mut art = artifact(&data);
            if mode == "hash" {
                art.sha256 = "0".repeat(64);
            }
            let mut t = Transfers::default();
            t.start(TransferRequest::Download {
                path: path.clone(),
                artifact: art,
            })
            .unwrap();
            let action = next(&mut t);
            sent(&mut t, &action);
            if mode == "cancel" {
                t.cancel(false);
            } else {
                if mode == "race" {
                    std::fs::write(&path, b"Existing audio").unwrap();
                }
                let chunk = RemoteArtifactChunk {
                    offset: if mode == "offset" { 1 } else { 0 },
                    data: base64::engine::general_purpose::STANDARD.encode(&data),
                    complete: mode != "completion",
                };
                receipt(&mut t, RemoteReplyStatus::Accepted, Some(payload(chunk)));
                settle(&mut t);
            }
            assert!(t.failure.is_some(), "{mode}");
            dir.clean_partial();
            if mode == "race" {
                assert_eq!(std::fs::read(&path).unwrap(), b"Existing audio");
            } else {
                assert!(!path.exists(), "{mode}");
            }
        }
    }
    #[test]
    fn uncertain_finish_retains_request_identity_and_never_replays_or_accepts_late_receipts() {
        let dir = Directory::new();
        let path = dir.file("Voice.wav");
        std::fs::write(&path, [1u8; 20]).unwrap();
        let mut t = Transfers::default();
        t.start(import(path)).unwrap();
        for _ in 0..2 {
            let action = next(&mut t);
            sent(&mut t, &action);
            receipt(&mut t, RemoteReplyStatus::Accepted, None);
        }
        let finish = next(&mut t);
        assert!(finish.authored);
        let id = new_id().unwrap();
        t.sent(id.clone(), Duration::ZERO);
        assert!(t.next(true).unwrap().is_none());
        assert_eq!(t.failure.as_ref().map(|f| (f.0, f.1)), Some((true, true)));
        assert_eq!(
            t.view.as_ref().unwrap().request_id.as_deref(),
            Some(id.as_str())
        );
        assert!(!t.response(&RemoteReply {
            request_id: id,
            status: RemoteReplyStatus::Accepted,
            revision: 9,
            message: "Late".into(),
            payload: None
        }));
        assert!(t.next(true).unwrap().is_none());
        assert!(t.receipt.is_none());
    }
    #[test]
    fn export_manifest_download_and_rejected_reply_are_explicitly_correlated() {
        let dir = Directory::new();
        let path = dir.file("Mix.wav");
        let mut t = Transfers::default();
        t.start(TransferRequest::Export {
            path,
            name: "Song".into(),
        })
        .unwrap();
        let action = next(&mut t);
        assert!(matches!(action.command, RemoteCommand::ExportMix { .. }));
        assert_eq!(action.timeout, Duration::from_secs(600));
        sent(&mut t, &action);
        let unrelated = RemoteReply {
            request_id: new_id().unwrap(),
            status: RemoteReplyStatus::Accepted,
            revision: 9,
            message: String::new(),
            payload: None,
        };
        assert!(!t.response(&unrelated));
        let data = [8u8; 10];
        receipt(
            &mut t,
            RemoteReplyStatus::Accepted,
            Some(payload(artifact(&data))),
        );
        let action = next(&mut t);
        assert!(matches!(
            action.command,
            RemoteCommand::DownloadArtifact { .. }
        ));
        sent(&mut t, &action);
        receipt(&mut t, RemoteReplyStatus::Rejected, None);
        assert!(t.failure.is_some());
        dir.clean_partial();
    }
    fn recipe(path: Option<PathBuf>, tracks: Option<Vec<String>>) -> TransferRequest {
        TransferRequest::ProjectExport {
            path,
            name: "Song".into(),
            format: "aiff".into(),
            tails: false,
            tracks,
            context: RemoteEditorContext {
                track_id: Some("00000000-0000-0000-0000-000000000002".into()),
                clip_id: None,
                clip_ids: vec![],
                cursor_frame: 48000,
                range_start: Some(48000),
                range_end: Some(96000),
                effect_id: None,
                automation_target: None,
            },
        }
    }
    #[test]
    fn export_recipe_keeps_range_format_tails_and_large_job_timeout() {
        let dir = Directory::new();
        let mut t = Transfers::default();
        t.start(recipe(Some(dir.file("Mix.aiff")), None)).unwrap();
        let a = next(&mut t);
        let RemoteCommand::Editor { intent } = a.command.clone() else {
            panic!()
        };
        assert_eq!(intent.operation, "project.export");
        assert_eq!(intent.strings["format"], "aiff");
        assert_eq!(intent.context.range_start, Some(48000));
        assert_eq!(intent.context.range_end, Some(96000));
        assert!(!intent.flags["tails"]);
        assert!(intent.flags["selection"]);
        assert!(!intent.flags["tracks"]);
        assert_eq!(a.timeout, Duration::from_secs(600));
        assert!(!a.authored);
        assert!(a.revision.is_none());
        sent(&mut t, &a);
        receipt(
            &mut t,
            RemoteReplyStatus::Accepted,
            Some(payload(artifact(&[1; 12]))),
        );
        assert!(matches!(
            next(&mut t).command,
            RemoteCommand::DownloadArtifact { .. }
        ));
        t.cancel(true);
        dir.clean_partial();
    }
    #[test]
    fn stems_validate_complete_manifest_set_atomically_then_offer_verified_downloads() {
        let ids = vec![new_id().unwrap(), new_id().unwrap()];
        let mut t = Transfers::default();
        t.start(recipe(None, Some(ids.clone()))).unwrap();
        let a = next(&mut t);
        let RemoteCommand::Editor { intent } = a.command.clone() else {
            panic!()
        };
        assert!(intent.flags["tracks"]);
        let decoded: Vec<String> = serde_json::from_slice(
            &base64::engine::general_purpose::STANDARD
                .decode(intent.payload.unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(decoded, ids);
        sent(&mut t, &a);
        let one = artifact(&[1; 8]);
        let mut bad = artifact(&[2; 8]);
        bad.sha256 = "bad".into();
        receipt(
            &mut t,
            RemoteReplyStatus::Accepted,
            Some(payload(vec![one.clone(), bad])),
        );
        assert!(t.failure.is_some());
        assert!(t.artifacts.is_empty());
        t.start(recipe(None, Some(ids))).unwrap();
        let a = next(&mut t);
        sent(&mut t, &a);
        let two = artifact(&[2; 8]);
        receipt(
            &mut t,
            RemoteReplyStatus::Accepted,
            Some(payload(vec![one, two])),
        );
        assert_eq!(t.artifacts.len(), 2);
        assert!(t.failure.is_none());
        assert!(!t.view.as_ref().unwrap().active);
        assert!(t.view.as_ref().unwrap().phase.contains("2 stems"));
        assert!(t.next(true).unwrap().is_none());
    }
}
