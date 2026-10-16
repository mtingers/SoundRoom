//! Host-only recovered-take authority. No remote operation or Record capability
//! is exposed here. Callers supply sealed server custody, never client paths.
use super::*;
use soundroom_engine::{
    authority::{SessionEditContext, SessionEditRequest},
    capture_archive::ArchiveManifest,
    capture_recovery::{
        prepare_take_import, recover_take_in, RecoveryAcceptance, RecoveryOutcome, RecoveryPlan,
    },
};
use soundroom_platform::{file_identity, FileIdentity, PrivateDirectory};
use std::{collections::BTreeSet, io::Seek, sync::Weak, time::SystemTime};
const RAW_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const RECOVERED_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const SOURCE_FILES: [&str; 4] = [
    "manifest.json",
    "native.packets",
    "events.jsonl",
    "completion.json",
];
#[derive(Default)]
pub(super) struct State {
    raw: BTreeMap<String, Arc<Raw>>,
    retained: BTreeMap<String, u64>,
    reserved: u64,
    reserved_folders: usize,
    assets: BTreeMap<String, Arc<AssetLease>>,
}
struct Raw {
    directory: PrivateDirectory,
    manifest: ArchiveManifest,
    bytes: u64,
    fingerprints: BTreeMap<String, Fingerprint>,
    busy: AtomicBool,
}
#[derive(Clone, PartialEq)]
struct Stamp {
    identity: FileIdentity,
    bytes: u64,
    modified: SystemTime,
}
#[derive(Clone, PartialEq)]
struct Fingerprint {
    stamp: Stamp,
    sha256: String,
}
struct AssetLease {
    _directory: Arc<PrivateDirectory>,
    file: File,
    stamp: Stamp,
}
fn stamp(file: &File) -> Result<Stamp, Error> {
    let m = file.metadata()?;
    Ok(Stamp {
        identity: file_identity(file)?,
        bytes: m.len(),
        modified: m.modified()?,
    })
}
fn fingerprint(mut file: File, cancel: &AtomicBool) -> Result<Fingerprint, Error> {
    let before = stamp(&file)?;
    file.rewind()?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(protocol("Recovery cancelled"));
        }
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    if stamp(&file)? != before {
        return Err(Error::Conflict(
            "Custody content changed during fingerprinting".into(),
        ));
    }
    Ok(Fingerprint {
        stamp: before,
        sha256: format!("{:x}", hash.finalize()),
    })
}
fn target_name(request: &Request) -> Result<String, Error> {
    Ok(format!(
        "take-{:x}",
        Sha256::digest(serde_json::to_vec(&(&request.source_id, &request.plan))?)
    ))
}
fn verify_raw(raw: &Raw, cancel: &AtomicBool, hash: bool) -> Result<(), Error> {
    for name in SOURCE_FILES {
        match (raw.fingerprints.get(name), raw.directory.read_file(name)) {
            (Some(expected), Ok(file)) => {
                let matches = if hash {
                    fingerprint(file, cancel)? == *expected
                } else {
                    stamp(&file)? == expected.stamp
                };
                if !matches {
                    return Err(Error::Conflict("Raw custody changed after sealing".into()));
                }
            }
            (None, Err(e))
                if name == "completion.json" && e.kind() == std::io::ErrorKind::NotFound => {}
            _ => {
                return Err(Error::Conflict(
                    "Raw custody file presence changed after sealing".into(),
                ))
            }
        }
    }
    Ok(())
}
/// Host-selected routes and reviewed policy. Future wire exposure must first
/// define a canonical operation; this DTO is intentionally outside wire models.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub id: String,
    pub source_id: String,
    pub plan: RecoveryPlan,
    pub expected_track_revisions: BTreeMap<String, u64>,
    pub reviewed_incomplete: bool,
}
impl Service {
    /// Register a sealed archive supplied by the trusted hardware/recovery owner.
    /// Call on a worker. The caller relinquishes write authority before calling;
    /// immutable content is SHA checked again during recovery. Registration alone
    /// does not authorize recording or edit the session.
    pub fn adopt_raw_capture(
        shared: &Shared,
        directory: PrivateDirectory,
        cancel: &AtomicBool,
    ) -> Result<String, Error> {
        let manifest_file = directory.read_file("manifest.json")?;
        if manifest_file.metadata()?.len() > 16384 {
            return Err(protocol("Archive manifest exceeds 16 KiB"));
        }
        let manifest: ArchiveManifest = serde_json::from_reader(manifest_file.take(16385))?;
        manifest.validate().map_err(engine)?;
        let mut fingerprints = BTreeMap::new();
        let mut bytes = 0u64;
        // Inspect every length before hashing any potentially large source.
        for name in SOURCE_FILES {
            let file = match directory.read_file(name) {
                Ok(file) => file,
                Err(e) if name == "completion.json" && e.kind() == std::io::ErrorKind::NotFound => {
                    continue
                }
                Err(e) => return Err(e.into()),
            };
            bytes = bytes
                .checked_add(file.metadata()?.len())
                .filter(|n| *n <= RAW_BYTES)
                .ok_or(Error::QueueFull)?;
        }
        for name in SOURCE_FILES {
            match directory.read_file(name) {
                Ok(file) => {
                    fingerprints.insert(name.into(), fingerprint(file, cancel)?);
                }
                Err(e) if name == "completion.json" && e.kind() == std::io::ErrorKind::NotFound => {
                }
                Err(e) => return Err(e.into()),
            }
        }
        if cancel.load(Ordering::Acquire) {
            return Err(protocol("Capture custody admission cancelled"));
        }
        if fingerprints
            .get("manifest.json")
            .is_some_and(|f| f.stamp.bytes > 16384)
        {
            return Err(protocol("Archive manifest grew beyond 16 KiB"));
        }
        if fingerprints
            .values()
            .try_fold(0u64, |n, f| n.checked_add(f.stamp.bytes))
            != Some(bytes)
        {
            return Err(Error::Conflict(
                "Capture source changed during admission".into(),
            ));
        }
        directory.revalidate()?;
        let mut state = shared
            .lock()
            .map_err(|_| protocol("Authority is unavailable"))?;
        if let Some((id, old)) = state
            .recovery
            .raw
            .iter()
            .find(|(_, raw)| raw.manifest.take_id.eq_ignore_ascii_case(&manifest.take_id))
        {
            if old.fingerprints != fingerprints {
                return Err(Error::Conflict(
                    "Raw take identity reused with changed custody".into(),
                ));
            }
            return Ok(id.clone());
        }
        if state.recovery.raw.len() >= 32
            || state
                .recovery
                .raw
                .values()
                .map(|r| r.bytes)
                .sum::<u64>()
                .checked_add(bytes)
                .map_or(true, |n| n > RAW_BYTES)
        {
            return Err(Error::QueueFull);
        }
        let id = new_id()?;
        state.recovery.raw.insert(
            id.clone(),
            Arc::new(Raw {
                directory,
                manifest,
                bytes,
                fingerprints,
                busy: AtomicBool::new(false),
            }),
        );
        Ok(id)
    }
    /// Start one bounded authored recovery worker. Identity/role comes from the
    /// authenticated host channel, never from Request. Standard request receipts,
    /// cancellation and per-author FIFO use the existing service ledger.
    pub fn submit_recovery(
        shared: &Shared,
        peer: &Identity,
        out: &Outbox,
        mut request: Request,
    ) -> Result<(), Error> {
        if !validation::uuid(&request.id) || !validation::uuid(&request.source_id) {
            return Err(protocol("Recovery IDs must be UUIDs"));
        }
        request.id = request.id.to_uppercase();
        request.source_id = request.source_id.to_uppercase();
        for route in &mut request.plan.routes {
            route.track_id = route.track_id.to_uppercase();
        }
        let original_targets = request.expected_track_revisions.len();
        request.expected_track_revisions = request
            .expected_track_revisions
            .into_iter()
            .map(|(id, v)| (id.to_uppercase(), v))
            .collect();
        if original_targets != request.expected_track_revisions.len() {
            return Err(protocol("Duplicate case-insensitive recovery targets"));
        }
        let bytes = serde_json::to_vec(&request)?;
        if bytes.len() > 65536 {
            return Err(protocol("Recovery request exceeds 64 KiB"));
        }
        let mut hash = Sha256::new();
        hash.update(b"soundroom-host-recovery-v1\0");
        hash.update(bytes);
        let digest: [u8; 32] = hash.finalize().into();
        let wire_request = RemoteRequest {
            id: request.id.clone(),
            client_id: peer.runtime.clone(),
            expected_revision: None,
            expected_track_revisions: request
                .expected_track_revisions
                .iter()
                .map(|(id, v)| (id.clone(), *v as i64))
                .collect(),
            command: RemoteCommand::Ping { client_time: 0. },
        };
        let mut state = shared
            .lock()
            .map_err(|_| protocol("Authority is unavailable"))?;
        if peer.role != RemoteRole::Operator
            || !state.registry.authorizes(&peer.credential, &peer.role)
        {
            return Err(protocol("Current operator role is required for recovery"));
        }
        let k = key(peer, &wire_request);
        if let Some(old) = state.receipts.iter().find(|r| r.key == k) {
            if old.digest != digest {
                return Err(protocol(
                    "Recovery request ID reused with different content",
                ));
            }
            return out.send(&old.reply, true);
        }
        if let Some(old) = state.pending.get(&k) {
            if old.digest != digest {
                return Err(protocol("Recovery in-flight ID reused"));
            }
            return out.send(
                &response(
                    &request.id,
                    "busy",
                    state.authority.revision,
                    "Recovery remains in flight",
                    None,
                ),
                true,
            );
        }
        if state.authored_in_flight(peer) || !state.admits_job(peer, false) {
            return Err(Error::QueueFull);
        }
        let raw = state
            .recovery
            .raw
            .get(&request.source_id)
            .cloned()
            .ok_or_else(|| Error::Conflict("Server-owned raw take is unavailable".into()))?;
        let targets = request
            .plan
            .routes
            .iter()
            .map(|r| r.track_id.clone())
            .collect::<BTreeSet<_>>();
        if targets.is_empty()
            || targets.len() != request.plan.routes.len()
            || targets.len() > 32
            || targets != request.expected_track_revisions.keys().cloned().collect()
            || request
                .expected_track_revisions
                .values()
                .any(|v| *v > soundroom_engine::model::MAX_FRAME)
        {
            return Err(protocol(
                "Every recovery route needs one safe target version",
            ));
        }
        if request.reviewed_incomplete && !request.plan.allow_incomplete {
            return Err(protocol(
                "Reviewed incomplete recovery needs explicit incomplete policy",
            ));
        }
        for (id, expected) in &request.expected_track_revisions {
            if state.authority.track_revisions.get(id) != Some(expected) {
                return Err(Error::Conflict("Recovery target track changed".into()));
            }
        }
        // Worst-case native->48k expansion: <=8x at the minimum supported rate
        // including 5000ppm drift. Stereo output is <=2/channels input bytes.
        // Header reserve covers the engine's 4096 segment cap before any writing.
        let native = raw.fingerprints["native.packets"].stamp.bytes;
        let estimate = native
            .checked_mul(16)
            .and_then(|n| n.checked_mul(targets.len() as u64))
            .map(|n| n / (raw.manifest.channels as u64))
            .and_then(|n| n.checked_add(4096 * 128 + 16 * 1024 * 1024 + 64))
            .ok_or(Error::QueueFull)?;
        let target = target_name(&request)?;
        let new_folder = !state.recovery.retained.contains_key(&target);
        let retained = state.recovery.retained.values().sum::<u64>();
        let media_bytes = state
            .media
            .values()
            .try_fold(0u64, |n, m| {
                n.checked_add(
                    m.info
                        .frames
                        .checked_mul(m.info.block_align as u64)?
                        .checked_add(128)?,
                )
            })
            .ok_or(Error::QueueFull)?;
        if (new_folder && state.recovery.retained.len() + state.recovery.reserved_folders >= 32)
            || retained
                .checked_add(state.recovery.reserved)
                .and_then(|n| n.checked_add(estimate))
                .map_or(true, |n| n > RECOVERED_BYTES)
            || media_bytes
                .checked_add(estimate)
                .map_or(true, |n| n > RECOVERED_BYTES)
        {
            return Err(Error::QueueFull);
        }

        let root = PrivateDirectory::open_root(&state.root)?;
        let artifacts = match root.open_dir("Recovered") {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => root.create_dir("Recovered")?,
            Err(e) => return Err(e.into()),
        };
        if raw.busy.swap(true, Ordering::AcqRel) {
            return Err(Error::QueueFull);
        }
        state.recovery.reserved += estimate;
        if new_folder {
            state.recovery.reserved_folders += 1;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let task = Task {
            context: state.authority.session_edit_context(),
            media: state.media.clone(),
            request,
            raw: raw.clone(),
            artifacts,
            artifact_path: state.root.join("Recovered"),
            server_id: state.registry.server_id.clone(),
            server_name: state.registry.server_name.clone(),
            lease: Reservation {
                service: Arc::downgrade(shared),
                bytes: estimate,
                raw: raw.clone(),
                new_folder,
            },
            #[cfg(test)]
            hook: state.edit_hook.clone(),
        };
        state.pending.insert(
            k,
            Pending {
                save: false,
                waveform: false,
                authored: true,
                critical: false,
                transport_start: false,
                write: false,
                digest,
                cancel: cancel.clone(),
            },
        );
        let job = Job {
            peer: peer.clone(),
            request: wire_request,
            kind: JobKind::Recover {
                task: Box::new(task),
            },
            cancel,
        };
        drop(state);
        let shared = shared.clone();
        let out = out.clone();
        std::thread::spawn(move || run_job(shared, out, job, digest));
        Ok(())
    }
}
struct Reservation {
    service: Weak<Mutex<Service>>,
    bytes: u64,
    raw: Arc<Raw>,
    new_folder: bool,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.raw.busy.store(false, Ordering::Release);
        if let Some(service) = self.service.upgrade() {
            if let Ok(mut state) = service.lock() {
                state.recovery.reserved = state.recovery.reserved.saturating_sub(self.bytes);
                if self.new_folder {
                    state.recovery.reserved_folders =
                        state.recovery.reserved_folders.saturating_sub(1);
                }
            }
        }
    }
}
pub(super) struct Task {
    context: SessionEditContext,
    media: BTreeMap<String, Media>,
    request: Request,
    raw: Arc<Raw>,
    artifacts: PrivateDirectory,
    artifact_path: PathBuf,
    server_id: String,
    server_name: String,
    lease: Reservation,
    #[cfg(test)]
    hook: Option<Arc<dyn Fn() + Send + Sync>>,
}
impl Task {
    pub(super) fn run(
        self,
        shared: &Shared,
        peer: &Identity,
        _wire: &RemoteRequest,
        cancel: &AtomicBool,
    ) -> Result<Option<Value>, Error> {
        let cancelled = || {
            if cancel.load(Ordering::Acquire) {
                Err(protocol("Recovery cancelled"))
            } else {
                Ok(())
            }
        };
        cancelled()?;
        self.raw.directory.revalidate()?;
        verify_raw(&self.raw, cancel, true)?;
        let target = target_name(&self.request)?;
        let new_accounting = {
            let mut state = shared
                .lock()
                .map_err(|_| protocol("Authority is unavailable"))?;
            if state.recovery.retained.contains_key(&target) {
                false
            } else {
                state
                    .recovery
                    .retained
                    .insert(target.clone(), self.lease.bytes);
                true
            }
        };
        let outcome = recover_take_in(
            &self.raw.directory,
            &self.artifacts,
            &target,
            &self.request.plan,
            cancel,
        )
        .map_err(engine);
        let report = match outcome {
            Ok(RecoveryOutcome::Committed(report) | RecoveryOutcome::AlreadyPresent(report)) => {
                report
            }
            Err(error) => {
                // Failed stage cleanup remains charged unless absence is proved.
                // This also prevents panic/verification failures after publication
                // from turning retained disk artifacts into unaccounted orphans.
                if new_accounting
                    && self
                        .artifacts
                        .open_dir(&target)
                        .err()
                        .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
                {
                    if let Ok(mut state) = shared.lock() {
                        state.recovery.retained.remove(&target);
                    }
                }
                return Err(error);
            }
        };
        let directory = Arc::new(self.artifacts.open_dir(&target)?);
        let audio = Arc::new(directory.open_dir("Audio")?);
        let mut additions = BTreeMap::new();
        let mut leases = BTreeMap::new();
        let mut actual = directory
            .read_file("recovery.json")?
            .metadata()?
            .len()
            .checked_add(directory.read_file("recovery.sha256")?.metadata()?.len())
            .filter(|n| *n <= self.lease.bytes)
            .ok_or(Error::QueueFull)?;
        for asset in &report.assets {
            let name = format!("{}.wav", asset.id);
            if asset.relative_path != format!("Audio/{name}") {
                return Err(protocol(
                    "Recovery artifact path differs from server layout",
                ));
            }
            let file = audio.read_file(&name)?;
            let fingerprint = fingerprint(file.try_clone()?, cancel)?;
            if fingerprint.sha256 != asset.sha256 {
                return Err(Error::Conflict(
                    "Recovered artifact fingerprint changed".into(),
                ));
            }
            let info = WaveReader::from_file(file.try_clone()?)
                .map_err(engine)?
                .info;
            if info.sample_rate != 48000
                || info.channels != 2
                || !info.float
                || info.bits != 32
                || info.frames != asset.frames
            {
                return Err(protocol(
                    "Recovered artifact format/length differs from receipt",
                ));
            }
            actual = actual
                .checked_add(fingerprint.stamp.bytes)
                .filter(|n| *n <= self.lease.bytes)
                .ok_or(Error::QueueFull)?;
            additions.insert(
                asset.id.clone(),
                Media {
                    path: self.artifact_path.join(&target).join("Audio").join(name),
                    info,
                },
            );
            leases.insert(
                asset.id.clone(),
                Arc::new(AssetLease {
                    _directory: audio.clone(),
                    file,
                    stamp: fingerprint.stamp,
                }),
            );
        }
        // Persist storage accounting even when operator review/authority later
        // rejects: retained artifacts remain available for deliberate review.
        {
            let mut state = shared
                .lock()
                .map_err(|_| protocol("Authority is unavailable"))?;
            state.recovery.retained.insert(target.clone(), actual);
        }
        // Provenance belongs to the held recovery receipt, not the language-
        // neutral RecordingTake grammar (id/name/clips). Rehydrate only exact
        // canonical matches in a private helper context; strip the one known
        // recovery annotation before complete snapshot validation.
        let mut import_context = self.context.session().clone();
        let recovered_ids = report
            .takes
            .iter()
            .map(|take| take.id.clone())
            .collect::<BTreeSet<_>>();
        for recovered in &report.takes {
            if let Some(track) = import_context
                .tracks
                .iter_mut()
                .find(|t| t.id == recovered.track_id)
            {
                for take in &mut track.recording_takes {
                    if take.extra.get("id").and_then(Value::as_str) == Some(&recovered.id) {
                        let mut expected = recovered.take.clone();
                        expected.extra.remove("rawCaptureID");
                        if *take != expected {
                            return Err(Error::Conflict(
                                "Canonical recovered take identity changed".into(),
                            ));
                        }
                        take.extra
                            .insert("rawCaptureID".into(), json!(report.raw_take_id));
                    }
                }
            }
        }
        let imported = prepare_take_import(
            &report,
            &import_context,
            if self.request.reviewed_incomplete {
                RecoveryAcceptance::ReviewedIncomplete
            } else {
                RecoveryAcceptance::CompleteOnly
            },
        )
        .map_err(engine)?;
        let before = self.context.session().clone();
        let source_revision = self.context.revision();
        let mut candidate = before.clone();
        for mut track in imported.tracks {
            for take in &mut track.recording_takes {
                if take
                    .extra
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| recovered_ids.contains(id))
                {
                    take.extra.remove("rawCaptureID");
                }
            }
            let id = track.id.clone();
            *Service::track(&mut candidate, &id)? = track;
        }
        let mut media = self.media.clone();
        for (id, asset) in &additions {
            if media
                .get(id)
                .is_some_and(|old| old.path != asset.path || old.info.frames != asset.info.frames)
            {
                return Err(Error::Conflict(
                    "Recovered asset identity already belongs to different catalog content".into(),
                ));
            }
            media.insert(id.clone(), asset.clone());
        }
        let source_stamps = self
            .media
            .iter()
            .map(|(id, media)| Ok((id.clone(), edits::stamp(&media.path)?)))
            .collect::<Result<BTreeMap<_, _>, Error>>()?;
        let project = Project {
            session: candidate,
            media: media.clone(),
        };
        project.validate_sources().map_err(engine)?;
        let next = source_revision
            .checked_add(1)
            .filter(|n| *n <= soundroom_engine::model::MAX_FRAME)
            .ok_or_else(|| Error::Conflict("Revision exhausted".into()))?;
        let candidate = project.session;
        let mut document = json!({"serverID":self.server_id,"serverName":self.server_name,"session":wire(&candidate)?,"revision":next,
            "trackRevisions":candidate.tracks.iter().chain(std::iter::once(&candidate.master_track)).map(|t|(t.id.clone(),next)).collect::<BTreeMap<_,_>>(),
            "transport":{},"status":"","peers":[],"devices":[],"inputDeviceID":0,"outputDeviceID":0,
            "assets":media.iter().map(|(id,m)|json!({"id":id,"frames":m.info.frames,"channels":m.info.channels,"peaks":[],"waveformStartFrame":0})).collect::<Vec<_>>()});
        neutralize_snapshot(&mut document);
        validation::validate_model(&document, "RemoteSnapshot")?;
        if encoded_size(&document)? > validation::MAX_SNAPSHOT_BYTES {
            return Err(protocol("Recovered session exceeds 64 MiB"));
        }
        drop(document);
        // Task has Drop, so consume an independently captured context below.
        let context = self.context;
        let prepared = context
            .prepare(
                SessionEditRequest {
                    author: &peer.runtime,
                    role: role(&peer.role),
                    expected_revision: Some(source_revision),
                    expected: &self.request.expected_track_revisions,
                    label: "Recover recording",
                    gesture: None,
                },
                candidate,
            )
            .map_err(engine)?;
        let base = Arc::new(prepared.session().clone());
        let witness = SessionWitness {
            revision: prepared.receipt().revision,
            bytes: encoded_size(&wire(base.as_ref())?)?,
            controls: base
                .tracks
                .iter()
                .chain(std::iter::once(&base.master_track))
                .map(|t| (t.id.clone(), MixControls::from_track(t)))
                .collect(),
            base,
        };
        #[cfg(test)]
        if let Some(hook) = &self.hook {
            hook();
        }
        cancelled()?;
        let mut state = shared
            .lock()
            .map_err(|_| protocol("Authority is unavailable"))?;
        cancelled()?;
        if peer.role != RemoteRole::Operator
            || !state.registry.authorizes(&peer.credential, &peer.role)
        {
            return Err(protocol("Operator credential changed during recovery"));
        }
        state.require_editable()?;
        if state.authority.revision != source_revision
            || state.media.len() != self.media.len()
            || self
                .media
                .iter()
                .any(|(id, m)| state.media.get(id).map(|v| &v.path) != Some(&m.path))
        {
            return Err(Error::Conflict(
                "Session or source catalog changed during recovery".into(),
            ));
        }
        for (id, media) in &self.media {
            if edits::stamp(&media.path)? != source_stamps[id] {
                return Err(Error::Conflict(
                    "Existing source custody changed during recovery".into(),
                ));
            }
        }
        self.raw.directory.revalidate()?;
        directory.revalidate()?;
        audio.revalidate()?;
        verify_raw(&self.raw, cancel, false)?;
        for (id, lease) in &leases {
            if stamp(&lease.file)? != lease.stamp
                || stamp(&audio.read_file(format!("{id}.wav"))?)? != lease.stamp
            {
                return Err(Error::Conflict(
                    "Recovered custody changed before commit".into(),
                ));
            }
        }
        // The authority token commits before infallible map replacement. Any
        // rejection leaves catalog/history/versions untouched; publication/graph
        // failure after this point does not reject an accepted authored edit.
        state
            .authority
            .commit_prepared_session(&peer.runtime, role(&peer.role), prepared)
            .map_err(engine)?;
        state.media = media;
        state.recovery.assets.extend(leases);
        if let Ok(mut cache) = state.witness.lock() {
            *cache = Some(witness);
        }
        if state.publish_changes(before, source_revision).is_err() {
            for p in state.peers.values() {
                p.out.close();
            }
        }
        if state.authority.revision != source_revision {
            state.history_events();
        }
        Ok(Some(
            json!({"receiptID":report.receipt_id,"rawTakeID":report.raw_take_id,"sourceID":self.request.source_id,"requiresReview":report.requires_review,"assets":report.assets.len(),"takes":report.takes.len()}),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundroom_engine::{
        capture::{CaptureTiming, CapturedBlock},
        capture_archive::{CaptureArchive, CaptureEvent},
    };
    use std::sync::mpsc::sync_channel;
    struct Fixture {
        root: PathBuf,
        service: Shared,
        peer: Identity,
        out: Outbox,
        inbox: Inbox,
        source: String,
        track: String,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn fixture(complete: bool) -> Fixture {
        fixture_with_completion(complete, false)
    }
    fn fixture_with_completion(complete: bool, missing: bool) -> Fixture {
        let (root, service, _, track) = super::super::publication_tests::control_fixture(0);
        let canonical = std::fs::canonicalize(&root).unwrap();
        service.lock().unwrap().root = canonical.clone();
        let custody = PrivateDirectory::open_root(&canonical).unwrap();
        let mut archive = CaptureArchive::create_in(
            &custody,
            "RawCapture",
            ArchiveManifest {
                version: 1,
                take_id: new_id().unwrap(),
                device_uid: "pure-simulated-only".into(),
                channels: 2,
                sample_rate: 48000,
                maximum_frames: 512,
                host_ticks_per_second: 1e9,
                calibration_offset_frames: 0,
            },
        )
        .unwrap();
        for packet in 0..8u64 {
            let frame = packet * 512;
            let host = 1_000_000_000_000_000_000 + (frame as f64 * 1e9 / 48000.).round() as u64;
            archive
                .event(CaptureEvent::OutputAnchor {
                    source_frame: Some(frame),
                    epoch: 1,
                    playback_clock: 40000 + frame,
                    frames: 512,
                    sample_time_bits: (100000. + frame as f64).to_bits(),
                    host_time: host,
                    flags: 3,
                })
                .unwrap();
            let left = vec![0.125; 512];
            let right = vec![-0.0625; 512];
            archive
                .append(
                    CapturedBlock {
                        source_frame: frame,
                        frames: 512,
                        timing: CaptureTiming {
                            sample_time: 500000. + frame as f64,
                            host_time: host,
                            flags: 3,
                        },
                    },
                    &[&left, &right],
                )
                .unwrap();
        }
        if complete {
            archive.finish().unwrap();
        } else {
            drop(archive);
        }
        if missing {
            custody
                .open_dir("RawCapture")
                .unwrap()
                .remove_file("completion.json")
                .unwrap();
        }
        let source = Service::adopt_raw_capture(
            &service,
            custody.open_dir("RawCapture").unwrap(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let credential = service
            .lock()
            .unwrap()
            .registry
            .issue("Recovery operator", RemoteRole::Operator)
            .unwrap();
        let peer = Identity {
            runtime: new_id().unwrap(),
            credential: credential["credentialID"].as_str().unwrap().into(),
            name: "Recovery operator".into(),
            role: RemoteRole::Operator,
        };
        let (out, inbox) = Outbox::pair(service.lock().unwrap().global());
        Fixture {
            root,
            service,
            peer,
            out,
            inbox,
            source,
            track,
        }
    }
    impl Fixture {
        fn request(&self, incomplete: bool, reviewed: bool) -> Request {
            Request {
                id: new_id().unwrap(),
                source_id: self.source.clone(),
                plan: RecoveryPlan {
                    version: 1,
                    name: "Recovered vocal".into(),
                    routes: vec![soundroom_engine::capture_recovery::RecoveryRoute {
                        track_id: self.track.clone(),
                        channels: vec![0, 1],
                    }],
                    range: None,
                    loop_range: None,
                    maximum_drift_ppm: 1000.,
                    allow_incomplete: incomplete,
                },
                expected_track_revisions: BTreeMap::from([(
                    self.track.clone(),
                    self.service.lock().unwrap().authority.track_revisions[&self.track],
                )]),
                reviewed_incomplete: reviewed,
            }
        }
        fn reply(&self, id: &str) -> Value {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Ok(bytes) = self.inbox.priority.try_recv() {
                    self.inbox.received(bytes.len());
                    let v: Value = serde_json::from_slice(&bytes).unwrap();
                    if v["kind"] == "reply" && v["requestID"] == id {
                        return v;
                    }
                }
                assert!(Instant::now() < deadline, "Recovery receipt timeout");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        fn submit(&self, request: Request) -> Value {
            let id = request.id.clone();
            Service::submit_recovery(&self.service, &self.peer, &self.out, request).unwrap();
            self.reply(&id)
        }
    }
    #[test]
    fn recovered_take_commits_catalog_history_and_exact_retry_without_reexecution() {
        let f = fixture(true);
        let request = f.request(false, false);
        let receipt = f.submit(request.clone());
        assert_eq!(receipt["status"], "accepted", "{receipt}");
        let report = receipt["payload"].clone();
        assert_eq!(receipt["revision"], 1);
        {
            let state = f.service.lock().unwrap();
            assert_eq!(state.media.len(), 1);
            assert_eq!(state.recovery.assets.len(), 1);
            assert_eq!(state.authority.session.tracks[0].recording_takes.len(), 1);
            assert!(state.authority.session.tracks[0].clips.is_empty());
            assert_eq!(
                state.authority.history_state(&f.peer.runtime).0,
                Some("Recover recording")
            );
            assert_eq!(state.recovery.reserved, 0);
            assert!(!state.recovery.raw[&f.source].busy.load(Ordering::Acquire));
        }
        assert_eq!(f.submit(request.clone()), receipt);
        let mut changed = request;
        changed.reviewed_incomplete = true;
        assert!(Service::submit_recovery(&f.service, &f.peer, &f.out, changed).is_err());
        let second = f.submit(f.request(false, false));
        assert_eq!(second["status"], "accepted", "{second}");
        assert_eq!(second["revision"], 1);
        assert_eq!(second["payload"], report);
        let mut state = f.service.lock().unwrap();
        state
            .authority
            .undo(&f.peer.runtime, Role::Operator)
            .unwrap();
        assert!(state.authority.session.tracks[0].recording_takes.is_empty());
        state
            .authority
            .redo(&f.peer.runtime, Role::Operator)
            .unwrap();
        assert_eq!(state.authority.session.tracks[0].recording_takes.len(), 1);
        assert_eq!(state.media.len(), 1);
        let project = Project {
            session: state.authority.session.clone(),
            media: state.media.clone(),
        };
        drop(state);
        let saved = f.root.join("Recovered.soundroom");
        project.save_new(&saved).unwrap();
        let reopened = Project::open(&saved).unwrap();
        assert_eq!(reopened.session.tracks[0].recording_takes.len(), 1);
        assert!(!serde_json::to_string(&reopened.session)
            .unwrap()
            .contains("rawCaptureID"));
    }
    #[test]
    fn incomplete_take_retains_review_artifacts_but_requires_operator_acceptance() {
        let f = fixture(false);
        let receipt = f.submit(f.request(true, false));
        assert_eq!(receipt["status"], "conflict", "{receipt}");
        assert!(receipt["message"]
            .as_str()
            .unwrap()
            .contains("review acceptance"));
        {
            let state = f.service.lock().unwrap();
            assert_eq!(state.authority.revision, 0);
            assert!(state.media.is_empty());
            assert_eq!(state.recovery.retained.len(), 1);
            assert_eq!(state.recovery.reserved, 0);
        }
        let receipt = f.submit(f.request(true, true));
        assert_eq!(receipt["status"], "accepted", "{receipt}");
        let payload: Value = serde_json::from_slice(
            &base64::engine::general_purpose::STANDARD
                .decode(receipt["payload"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(payload["requiresReview"], true);
        let mut viewer = f.peer.clone();
        viewer.role = RemoteRole::Viewer;
        assert!(
            Service::submit_recovery(&f.service, &viewer, &f.out, f.request(true, true)).is_err()
        );
    }
    #[test]
    fn missing_completion_is_recoverable_only_after_review() {
        let f = fixture_with_completion(false, true);
        let receipt = f.submit(f.request(true, false));
        assert_eq!(receipt["status"], "conflict", "{receipt}");
        let receipt = f.submit(f.request(true, true));
        assert_eq!(receipt["status"], "accepted", "{receipt}");
    }
    #[test]
    fn stale_role_cancel_or_custody_change_rolls_back_media_and_authorship() {
        for failure in ["stale", "revoked", "cancelled", "raw", "artifact"] {
            let f = fixture(true);
            let request = f.request(false, false);
            let id = request.id.clone();
            let (entered, ready) = sync_channel(1);
            let (release, wait) = sync_channel(1);
            let wait = Mutex::new(wait);
            f.service.lock().unwrap().edit_hook = Some(Arc::new(move || {
                entered.send(()).unwrap();
                wait.lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
            }));
            Service::submit_recovery(&f.service, &f.peer, &f.out, request).unwrap();
            ready.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(f.service.try_lock().is_ok());
            let artifact_target = {
                let mut probe = f.request(false, false);
                probe.source_id = f.source.clone();
                target_name(&probe).unwrap()
            };
            {
                let mut state = f.service.lock().unwrap();
                match failure {
                    "stale" => {
                        state.authority.revision += 1;
                    }
                    "revoked" => {
                        state.registry.revoke(&f.peer.credential).unwrap();
                    }
                    "cancelled" => {
                        state
                            .pending
                            .values()
                            .next()
                            .unwrap()
                            .cancel
                            .store(true, Ordering::Release);
                    }
                    "artifact" => {
                        let directory = f
                            .root
                            .join("Recovered")
                            .join(artifact_target.clone())
                            .join("Audio");
                        let path = std::fs::read_dir(&directory)
                            .unwrap()
                            .next()
                            .unwrap()
                            .unwrap()
                            .path();
                        let old = directory.join("previous.wav");
                        std::fs::rename(&path, &old).unwrap();
                        std::fs::copy(&old, &path).unwrap();
                    }
                    "raw" => {
                        let file = std::fs::OpenOptions::new()
                            .write(true)
                            .open(f.root.join("RawCapture/native.packets"))
                            .unwrap();
                        file.set_len(file.metadata().unwrap().len() - 4).unwrap();
                    }
                    _ => unreachable!(),
                }
            }
            release.send(()).unwrap();
            let receipt = f.reply(&id);
            assert_ne!(receipt["status"], "accepted", "{failure}: {receipt}");
            let state = f.service.lock().unwrap();
            assert!(state.media.is_empty(), "{failure}");
            assert!(state.recovery.assets.is_empty());
            assert!(state.authority.session.tracks[0].recording_takes.is_empty());
            assert_eq!(state.authority.history_state(&f.peer.runtime).0, None);
            assert_eq!(state.recovery.reserved, 0);
            assert!(!state.recovery.raw[&f.source].busy.load(Ordering::Acquire));
        }
    }
    #[test]
    fn sealed_source_sha_change_and_nonprivate_alias_are_rejected() {
        let f = fixture(true);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(f.root.join("RawCapture/native.packets"))
            .unwrap();
        file.set_len(file.metadata().unwrap().len() - 4).unwrap();
        let receipt = f.submit(f.request(false, false));
        assert_eq!(receipt["status"], "conflict", "{receipt}");
        let state = f.service.lock().unwrap();
        assert!(state.media.is_empty());
        assert!(state.recovery.retained.is_empty());
        assert_eq!(state.recovery.reserved, 0);
        drop(state);
        assert!(PrivateDirectory::open_root(&f.root.join("RawCapture/../RawCapture")).is_err());
    }
    #[test]
    fn prewrite_storage_reservation_and_target_versions_reject_without_orphans() {
        let f = fixture(true);
        let request = f.request(false, false);
        f.service.lock().unwrap().recovery.reserved = RECOVERED_BYTES;
        assert!(Service::submit_recovery(&f.service, &f.peer, &f.out, request).is_err());
        assert!(!f.root.join("Recovered").exists());
        let mut state = f.service.lock().unwrap();
        state.recovery.reserved = 0;
        drop(state);
        let mut request = f.request(false, false);
        request.expected_track_revisions.clear();
        assert!(Service::submit_recovery(&f.service, &f.peer, &f.out, request).is_err());
        assert!(!f.root.join("Recovered").exists());
    }
}
