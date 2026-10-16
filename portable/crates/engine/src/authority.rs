//! Control-thread authority. Does not own an audio device or invent playback clock progress.
use crate::{
    model::{Extra, Session, Timing, Track, MAX_FRAME},
    Error, Result,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::atomic::{AtomicU64, Ordering},
};
static AUTHORITY_INSTANCE: AtomicU64 = AtomicU64::new(1);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Viewer,
    Editor,
    Operator,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Receipt {
    pub revision: u64,
    pub changed: Vec<String>,
}
#[derive(Clone, Debug)]
struct History {
    author: String,
    label: String,
    gesture: Option<String>,
    before: BTreeMap<String, Option<Track>>,
    after: BTreeMap<String, Option<Track>>,
    timing: Option<(Timing, Timing)>,
    name: Option<(String, String)>,
    order: Option<(Vec<String>, Vec<String>)>,
    /// Session-level document fields (for example `masteringAnalysis`).
    metadata: Option<(Extra, Extra)>,
    encoded_bytes: usize,
    controls: Option<ControlHistory>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Transport {
    Stopped { frame: u64 },
    Playing { frame: u64 },
    Recording { frame: u64 },
}
#[derive(Clone, Debug, PartialEq)]
pub enum TransportCommand {
    Play,
    Pause,
    Stop,
    Seek(u64),
}
/// Transport changes are requested here and become accepted only after the backend commits them.
/// Device callbacks report position; network/control threads never advance it from wall time.
#[derive(Debug)]
pub struct Authority {
    instance: u64,
    pub session: Session,
    pub revision: u64,
    pub track_revisions: BTreeMap<String, u64>,
    pub transport: Transport,
    history: Vec<History>,
    redo: BTreeMap<String, Vec<History>>,
    receipts: VecDeque<(String, String, String, Receipt)>,
    content_hashes: BTreeMap<String, [u8; 32]>,
}

/// Captured control context with no history or redo copies. Full model traversal,
/// diff/history admission and fingerprints are prepared on a worker. Capturing
/// the session still clones it; immutable-source capture is a separate scaling gate.
pub struct SessionEditContext {
    authority: Authority,
}
/// Private, non-deserializable proof produced only after normal authority validation.
/// A token cannot be applied to another authority or a different source revision.
pub struct PreparedSessionEdit {
    authority: Authority,
    source_revision: u64,
    author: String,
    role: Role,
    receipt: Receipt,
    label: String,
    gesture: Option<String>,
}
impl PreparedSessionEdit {
    pub fn receipt(&self) -> &Receipt {
        &self.receipt
    }
    pub fn session(&self) -> &Session {
        &self.authority.session
    }
}
pub struct SessionEditRequest<'a> {
    pub author: &'a str,
    pub role: Role,
    pub expected_revision: Option<u64>,
    pub expected: &'a BTreeMap<String, u64>,
    pub label: &'a str,
    pub gesture: Option<&'a str>,
}
impl SessionEditContext {
    pub fn session(&self) -> &Session {
        &self.authority.session
    }
    pub fn revision(&self) -> u64 {
        self.authority.revision
    }
    pub fn prepare(
        mut self,
        request: SessionEditRequest<'_>,
        candidate: Session,
    ) -> Result<PreparedSessionEdit> {
        let SessionEditRequest {
            author,
            role,
            expected_revision,
            expected,
            label,
            gesture,
        } = request;
        let source_revision = self.authority.revision;
        let receipt =
            self.authority
                .edit_session(author, role, expected_revision, expected, candidate)?;
        // This fork has at most one new history item; no foreign history is cloned
        // or exposed to worker preparation.
        self.authority.label_last_change(author, label, gesture)?;
        Ok(PreparedSessionEdit {
            authority: self.authority,
            source_revision,
            author: author.into(),
            role,
            receipt,
            label: label.into(),
            gesture: gesture.map(str::to_owned),
        })
    }
}

/// These scalars cannot change clip/media, automation, effect, MIDI, or timing invariants.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct MixControls {
    pub gain: f32,
    pub pan: f32,
    pub muted: bool,
    pub solo: bool,
    pub armed: bool,
}
impl MixControls {
    pub fn from_track(track: &Track) -> Self {
        Self {
            gain: track.gain,
            pan: track.pan,
            muted: track.muted,
            solo: track.solo,
            armed: track.armed,
        }
    }
    pub fn validate(&self) -> Result<()> {
        if !self.gain.is_finite()
            || !(0. ..=4.).contains(&self.gain)
            || !self.pan.is_finite()
            || !(-1. ..=1.).contains(&self.pan)
        {
            return Err(Error::Invalid("Invalid gain/pan control".into()));
        }
        Ok(())
    }
    fn apply(self, track: &mut Track) {
        track.gain = self.gain;
        track.pan = self.pan;
        track.muted = self.muted;
        track.solo = self.solo;
        track.armed = self.armed;
    }
}
pub struct ControlEdit<'a> {
    pub request_id: &'a str,
    pub expected: &'a BTreeMap<String, u64>,
    pub track_id: &'a str,
    pub controls: MixControls,
    pub label: &'a str,
    pub gesture: Option<&'a str>,
}
#[derive(Clone, Debug)]
struct ControlHistory {
    track_id: String,
    before: MixControls,
    after: MixControls,
    content_hash: [u8; 32],
}
/// Borrow all non-scalar content. Hashing is performed when content is validated/changed,
/// never while applying subsequent scalar controls or checking their compact history.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TrackContent<'a> {
    id: &'a str,
    name: &'a str,
    color: i32,
    clips: &'a [crate::model::Clip],
    mix_automation: &'a BTreeMap<String, Vec<crate::model::AutomationPoint>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    volume_automation_scale: &'a Option<String>,
    realtime_effects: &'a [crate::model::Effect],
    #[serde(skip_serializing_if = "Option::is_none")]
    routing: &'a Option<crate::routing::TrackRouting>,
    recording_takes: &'a [crate::model::Take],
    #[serde(flatten)]
    extra: &'a crate::model::Extra,
}
fn track_content(track: &Track) -> TrackContent<'_> {
    TrackContent {
        id: &track.id,
        name: &track.name,
        color: track.color,
        clips: &track.clips,
        mix_automation: &track.mix_automation,
        volume_automation_scale: &track.volume_automation_scale,
        realtime_effects: &track.realtime_effects,
        routing: &track.routing,
        recording_takes: &track.recording_takes,
        extra: &track.extra,
    }
}
fn content_fingerprint(track: &Track) -> Result<[u8; 32]> {
    struct Hash(Sha256);
    impl std::io::Write for Hash {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut hash = Hash(Sha256::new());
    serde_json::to_writer(&mut hash, &track_content(track))?;
    Ok(hash.0.finalize().into())
}
impl Authority {
    pub fn new(mut session: Session) -> Result<Self> {
        session.version = session.required_version();
        session.validate()?;
        let track_revisions = session
            .tracks
            .iter()
            .chain(std::iter::once(&session.master_track))
            .map(|t| (t.id.clone(), 0))
            .collect();
        let content_hashes = session
            .tracks
            .iter()
            .chain(std::iter::once(&session.master_track))
            .map(|track| Ok((track.id.clone(), content_fingerprint(track)?)))
            .collect::<Result<_>>()?;
        let instance = AUTHORITY_INSTANCE
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            })
            .map_err(|_| Error::Conflict("Authority identity exhausted".into()))?;
        Ok(Self {
            instance,
            session,
            revision: 0,
            track_revisions,
            transport: Transport::Stopped { frame: 0 },
            history: vec![],
            redo: BTreeMap::new(),
            receipts: VecDeque::new(),
            content_hashes,
        })
    }
    /// Every asset the current session, Undo or Redo can still reach.
    pub fn referenced_asset_ids(&self) -> std::collections::BTreeSet<String> {
        let mut ids = self.session.asset_ids();
        let tracks = self
            .history
            .iter()
            .chain(self.redo.values().flatten())
            .flat_map(|h| h.before.values().chain(h.after.values()))
            .flatten();
        for track in tracks {
            let session = Session {
                tracks: vec![track.clone()],
                ..Session::default()
            };
            ids.extend(session.asset_ids());
        }
        ids
    }
    pub fn session_edit_context(&self) -> SessionEditContext {
        SessionEditContext {
            authority: Self {
                instance: self.instance,
                session: self.session.clone(),
                revision: self.revision,
                track_revisions: self.track_revisions.clone(),
                transport: self.transport.clone(),
                history: vec![],
                redo: BTreeMap::new(),
                receipts: VecDeque::new(),
                content_hashes: self.content_hashes.clone(),
            },
        }
    }
    /// Commit only already-validated authored state. Transport remains the actual
    /// backend state, which may have advanced or stopped during preparation.
    pub fn commit_prepared_session(
        &mut self,
        author: &str,
        role: Role,
        mut prepared: PreparedSessionEdit,
    ) -> Result<Receipt> {
        if role == Role::Viewer || prepared.role != role || prepared.author != author {
            return Err(Error::Conflict("Prepared edit author/role changed".into()));
        }
        if self.instance != prepared.authority.instance || self.revision != prepared.source_revision
        {
            return Err(Error::Conflict(
                "Session changed during authored preparation".into(),
            ));
        }
        if matches!(self.transport, Transport::Recording { .. }) {
            return Err(Error::Conflict(
                "Session edits are blocked during recording".into(),
            ));
        }
        if prepared.receipt.changed.is_empty() {
            return Ok(prepared.receipt);
        }
        self.history.append(&mut prepared.authority.history);
        self.redo.remove(author);
        self.session = prepared.authority.session;
        self.track_revisions = prepared.authority.track_revisions;
        self.content_hashes = prepared.authority.content_hashes;
        self.revision = prepared.authority.revision;
        // Label validation already succeeded on the preparation fork. Existing
        // gesture coalescing remains best-effort after this accepted commit; a
        // serialization failure cannot turn a committed edit into rejection.
        let _ = self.label_last_change(author, &prepared.label, prepared.gesture.as_deref());
        self.trim_history();
        Ok(prepared.receipt)
    }
    fn next_revision(&self) -> Result<u64> {
        if self.revision < MAX_FRAME {
            Ok(self.revision + 1)
        } else {
            Err(Error::Conflict(
                "Revision exhausted; reopen a new authority epoch".into(),
            ))
        }
    }
    fn control_track(&self, id: &str) -> Option<&Track> {
        if id == self.session.master_track.id {
            Some(&self.session.master_track)
        } else {
            self.session.tracks.iter().find(|track| track.id == id)
        }
    }
    fn control_track_mut(&mut self, id: &str) -> Result<&mut Track> {
        if id == self.session.master_track.id {
            Ok(&mut self.session.master_track)
        } else {
            self.session
                .tracks
                .iter_mut()
                .find(|track| track.id == id)
                .ok_or_else(|| Error::Conflict("Track is unavailable".into()))
        }
    }
    fn updated_hashes(
        &self,
        candidate: &Session,
        changed: &[String],
    ) -> Result<BTreeMap<String, [u8; 32]>> {
        let mut hashes = self.content_hashes.clone();
        for id in changed {
            let track = if id == &candidate.master_track.id {
                Some(&candidate.master_track)
            } else {
                candidate.tracks.iter().find(|track| &track.id == id)
            };
            if let Some(track) = track {
                hashes.insert(id.clone(), content_fingerprint(track)?);
            } else {
                hashes.remove(id);
            }
        }
        Ok(hashes)
    }
    fn controls_match(&self, controls: &ControlHistory, redo: bool) -> bool {
        self.content_hashes.get(&controls.track_id) == Some(&controls.content_hash)
            && self.control_track(&controls.track_id).is_some_and(|track| {
                MixControls::from_track(track)
                    == if redo {
                        controls.before
                    } else {
                        controls.after
                    }
            })
    }
    /// Compact authored edit against an already validated resource. Scalar checks preserve
    /// the full session's invariants; content changes still use complete candidate validation.
    pub fn edit_controls(
        &mut self,
        author: &str,
        role: Role,
        edit: ControlEdit<'_>,
    ) -> Result<Receipt> {
        let ControlEdit {
            request_id,
            expected,
            track_id,
            controls,
            label,
            gesture,
        } = edit;
        if role == Role::Viewer {
            return Err(Error::Conflict("Viewer cannot edit".into()));
        }
        if author.is_empty()
            || author.len() > 128
            || request_id.is_empty()
            || request_id.len() > 128
            || label.is_empty()
            || label.len() > 128
            || label.chars().any(char::is_control)
            || gesture
                .is_some_and(|g| g.is_empty() || g.len() > 128 || g.chars().any(char::is_control))
        {
            return Err(Error::Invalid("Invalid identity/history metadata".into()));
        }
        controls.validate()?;
        let fingerprint = serde_json::to_string(&(expected, track_id, controls, label, gesture))?;
        if let Some((_, _, previous, receipt)) = self
            .receipts
            .iter()
            .find(|(a, r, _, _)| a == author && r == request_id)
        {
            if previous != &fingerprint {
                return Err(Error::Conflict(
                    "Request ID reused for another command".into(),
                ));
            }
            return Ok(receipt.clone());
        }
        let current = self
            .track_revisions
            .get(track_id)
            .ok_or_else(|| Error::Conflict("Track unavailable".into()))?;
        if expected.get(track_id) != Some(current) {
            return Err(Error::Conflict("Stale track controls".into()));
        }
        let before = MixControls::from_track(
            self.control_track(track_id)
                .ok_or_else(|| Error::Conflict("Track unavailable".into()))?,
        );
        let receipt = if before == controls {
            Receipt {
                revision: self.revision,
                changed: vec![],
            }
        } else {
            let revision = self.next_revision()?;
            let content_hash = match self.content_hashes.get(track_id) {
                Some(hash) => *hash,
                None => content_fingerprint(self.control_track(track_id).unwrap())?,
            };
            self.content_hashes.insert(track_id.into(), content_hash);
            controls.apply(self.control_track_mut(track_id)?);
            self.track_revisions.insert(track_id.into(), revision);
            self.revision = revision;
            self.history.push(History {
                author: author.into(),
                label: label.into(),
                gesture: gesture.map(str::to_owned),
                before: BTreeMap::new(),
                after: BTreeMap::new(),
                timing: None,
                name: None,
                order: None,
                metadata: None,
                encoded_bytes: 512,
                controls: Some(ControlHistory {
                    track_id: track_id.into(),
                    before,
                    after: controls,
                    content_hash,
                }),
            });
            self.redo.remove(author);
            self.label_last_change(author, label, gesture)?;
            self.trim_history();
            Receipt {
                revision,
                changed: vec![track_id.into()],
            }
        };
        self.receipts.push_back((
            author.into(),
            request_id.into(),
            fingerprint,
            receipt.clone(),
        ));
        while self.receipts.len() > 4096
            || self
                .receipts
                .iter()
                .map(|(a, r, f, _)| a.len() + r.len() + f.len() + 128)
                .sum::<usize>()
                > 16 * 1024 * 1024
        {
            self.receipts.pop_front();
        }
        Ok(receipt)
    }
    pub fn edit_tracks(
        &mut self,
        author: &str,
        role: Role,
        request_id: &str,
        expected: &BTreeMap<String, u64>,
        tracks: Vec<Track>,
    ) -> Result<Receipt> {
        if role == Role::Viewer {
            return Err(Error::Conflict("Viewer cannot edit".into()));
        }
        if author.len() > 128 || request_id.len() > 128 {
            return Err(Error::Invalid("Identity/request ID exceeds bound".into()));
        }
        let fingerprint = serde_json::to_string(&(expected, &tracks))?;
        if fingerprint.len() > 8 * 1024 * 1024 {
            return Err(Error::Invalid(
                "Track edit exceeds 8 MiB; use incremental commands when available".into(),
            ));
        }
        if let Some((_, _, old, receipt)) = self
            .receipts
            .iter()
            .find(|(a, r, _, _)| a == author && r == request_id)
        {
            if old != &fingerprint {
                return Err(Error::Conflict(
                    "Request ID reused for another command".into(),
                ));
            }
            return Ok(receipt.clone());
        }
        if tracks.is_empty() {
            return Err(Error::Invalid("Empty track edit".into()));
        }
        let mut before = BTreeMap::new();
        let mut after = BTreeMap::new();
        let mut candidate = self.session.clone();
        for track in tracks {
            let current = self
                .track_revisions
                .get(&track.id)
                .ok_or_else(|| Error::Conflict("Track was removed or is unavailable".into()))?;
            if expected.get(&track.id) != Some(current) {
                return Err(Error::Conflict(format!("Stale track {}", track.id)));
            }
            if after.contains_key(&track.id) {
                return Err(Error::Invalid("Track repeated in edit".into()));
            }
            let destination = if candidate.master_track.id == track.id {
                &mut candidate.master_track
            } else {
                candidate
                    .tracks
                    .iter_mut()
                    .find(|t| t.id == track.id)
                    .ok_or_else(|| Error::Conflict("Track is unavailable".into()))?
            };
            before.insert(track.id.clone(), Some(destination.clone()));
            after.insert(track.id.clone(), Some(track.clone()));
            *destination = track;
        }
        candidate.version = candidate.required_version();
        candidate.validate()?;
        let encoded_bytes = serde_json::to_vec(&before)?.len() + serde_json::to_vec(&after)?.len();
        if encoded_bytes > 32 * 1024 * 1024 {
            return Err(Error::Invalid(
                "Track history exceeds 32 MiB edit budget".into(),
            ));
        }
        let revision = self.next_revision()?;
        let changed = after.keys().cloned().collect::<Vec<_>>();
        let hashes = self.updated_hashes(&candidate, &changed)?;
        for id in &changed {
            self.track_revisions.insert(id.clone(), revision);
        }
        self.session = candidate;
        self.content_hashes = hashes;
        self.revision = revision;
        self.history.push(History {
            author: author.into(),
            label: "Edit tracks".into(),
            gesture: None,
            before,
            after,
            timing: None,
            name: None,
            order: None,
            metadata: None,
            encoded_bytes,
            controls: None,
        });
        self.redo.remove(author);
        self.trim_history();
        let receipt = Receipt { revision, changed };
        self.receipts.push_back((
            author.into(),
            request_id.into(),
            fingerprint,
            receipt.clone(),
        ));
        while self.receipts.len() > 4096
            || self
                .receipts
                .iter()
                .map(|(a, r, f, _)| a.len() + r.len() + f.len() + 128)
                .sum::<usize>()
                > 16 * 1024 * 1024
        {
            self.receipts.pop_front();
        }
        Ok(receipt)
    }
    fn trim_history(&mut self) {
        // Serialized metadata budget is an upper-level bound, not a heap-allocation measurement.
        while self.history.len() > 256
            || self
                .history
                .iter()
                .chain(self.redo.values().flatten())
                .map(|h| h.encoded_bytes)
                .sum::<usize>()
                > 32 * 1024 * 1024
        {
            if !self.history.is_empty() {
                self.history.remove(0);
            } else if let Some(key) = self.redo.keys().next().cloned() {
                self.redo.remove(&key);
            } else {
                break;
            }
        }
    }
    pub fn set_timing(
        &mut self,
        author: &str,
        role: Role,
        expected: u64,
        timing: Timing,
    ) -> Result<Receipt> {
        if role == Role::Viewer {
            return Err(Error::Conflict("Viewer cannot edit tempo".into()));
        }
        if matches!(self.transport, Transport::Recording { .. }) {
            return Err(Error::Conflict(
                "Tempo cannot change during recording".into(),
            ));
        }
        if expected != self.revision {
            return Err(Error::Conflict("Stale session revision".into()));
        }
        timing.validate()?;
        let revision = self.next_revision()?;
        let before = self.session.timing.clone();
        self.session.timing = timing.clone();
        if timing != Timing::default() {
            self.session.version = self.session.version.max(9)
        }
        self.revision = revision;
        self.history.push(History {
            author: author.into(),
            label: "Edit timing".into(),
            gesture: None,
            before: BTreeMap::new(),
            after: BTreeMap::new(),
            timing: Some((before, timing)),
            name: None,
            order: None,
            metadata: None,
            encoded_bytes: 128,
            controls: None,
        });
        self.redo.remove(author);
        self.trim_history();
        Ok(Receipt {
            revision,
            changed: vec!["timing".into()],
        })
    }
    /// Labels/coalesces only the just-authored, adjacent and unchanged scope.
    /// Request IDs and resource revisions remain distinct even for one Undo gesture.
    pub fn label_last_change(
        &mut self,
        author: &str,
        label: &str,
        gesture: Option<&str>,
    ) -> Result<()> {
        if label.is_empty()
            || label.len() > 128
            || label.chars().any(char::is_control)
            || gesture
                .is_some_and(|g| g.is_empty() || g.len() > 128 || g.chars().any(char::is_control))
        {
            return Err(Error::Invalid("Invalid history action/gesture".into()));
        }
        let Some(last) = self.history.last_mut().filter(|h| h.author == author) else {
            return Ok(());
        };
        last.label = label.into();
        last.gesture = gesture.map(str::to_owned);
        let n = self.history.len();
        if n < 2 {
            return Ok(());
        }
        let (previous, current) = (&self.history[n - 2], &self.history[n - 1]);
        if let (Some(previous_controls), Some(current_controls)) =
            (&previous.controls, &current.controls)
        {
            if previous.author == author
                && current.gesture.is_some()
                && previous.gesture == current.gesture
                && previous.label == current.label
                && previous_controls.track_id == current_controls.track_id
                && previous_controls.content_hash == current_controls.content_hash
                && previous_controls.after == current_controls.before
            {
                let after = current_controls.after;
                self.history.pop();
                self.history
                    .last_mut()
                    .unwrap()
                    .controls
                    .as_mut()
                    .unwrap()
                    .after = after;
                self.trim_history();
            }
            return Ok(());
        }
        if previous.controls.is_some() || current.controls.is_some() {
            return Ok(());
        }
        if previous.author != author
            || current.gesture.is_none()
            || previous.gesture != current.gesture
            || previous.label != current.label
            || previous.after != current.before
            || previous.timing.is_some()
            || current.timing.is_some()
            || previous.name.is_some()
            || current.name.is_some()
            || previous.order.is_some()
            || current.order.is_some()
            || previous.metadata.is_some()
            || current.metadata.is_some()
        {
            return Ok(());
        }
        let bytes =
            serde_json::to_vec(&previous.before)?.len() + serde_json::to_vec(&current.after)?.len();
        if bytes > 32 * 1024 * 1024 {
            return Ok(());
        }
        let current = self.history.pop().unwrap();
        let previous = self.history.last_mut().unwrap();
        previous.after = current.after;
        previous.encoded_bytes = bytes;
        self.trim_history();
        Ok(())
    }
    /// Available authored history is private per runtime and checked against current resources.
    pub fn history_state(&self, author: &str) -> (Option<&str>, Option<&str>) {
        let undo = self
            .history
            .iter()
            .rfind(|h| h.author == author)
            .filter(|h| self.history_matches(h, false))
            .map(|h| h.label.as_str());
        let redo = self
            .redo
            .get(author)
            .and_then(|h| h.last())
            .filter(|h| self.history_matches(h, true))
            .map(|h| h.label.as_str());
        (undo, redo)
    }
    /// Returns only a currently applicable scalar history target. No content clone or mutation.
    pub fn control_history_target(&self, author: &str, redo: bool) -> Option<(&str, MixControls)> {
        let history = if redo {
            self.redo.get(author)?.last()?
        } else {
            self.history
                .iter()
                .rfind(|history| history.author == author)?
        };
        if !self.history_matches(history, redo) {
            return None;
        }
        let controls = history.controls.as_ref()?;
        Some((
            &controls.track_id,
            if redo {
                controls.after
            } else {
                controls.before
            },
        ))
    }
    fn history_matches(&self, h: &History, redo: bool) -> bool {
        if let Some(controls) = &h.controls {
            return self.controls_match(controls, redo);
        }
        let expected = if redo { &h.before } else { &h.after };
        expected.iter().all(|(id, value)| {
            let current = if id.eq_ignore_ascii_case(crate::model::MASTER_ID) {
                Some(&self.session.master_track)
            } else {
                self.session.tracks.iter().find(|t| &t.id == id)
            };
            current == value.as_ref()
        }) && h.timing.as_ref().map_or(true, |(before, after)| {
            self.session.timing == if redo { before.clone() } else { after.clone() }
        }) && h.name.as_ref().map_or(true, |(before, after)| {
            &self.session.name == if redo { before } else { after }
        }) && h.order.as_ref().map_or(true, |(before, after)| {
            self.session
                .tracks
                .iter()
                .map(|t| &t.id)
                .eq(if redo { before } else { after }.iter())
        }) && h.metadata.as_ref().map_or(true, |(before, after)| {
            &self.session.extra == if redo { before } else { after }
        })
    }
    pub fn undo(&mut self, author: &str, role: Role) -> Result<Receipt> {
        if role == Role::Viewer {
            return Err(Error::Conflict("Viewer cannot undo".into()));
        }
        let i = self
            .history
            .iter()
            .rposition(|h| h.author == author)
            .ok_or_else(|| Error::Conflict("No authored change to undo".into()))?;
        let h = self.history[i].clone();
        let receipt = self.apply_history(&h, false)?;
        self.history.remove(i);
        self.redo.entry(author.into()).or_default().push(h);
        Ok(receipt)
    }
    pub fn redo(&mut self, author: &str, role: Role) -> Result<Receipt> {
        if role == Role::Viewer {
            return Err(Error::Conflict("Viewer cannot redo".into()));
        }
        let h = self
            .redo
            .get(author)
            .and_then(|v| v.last())
            .cloned()
            .ok_or_else(|| Error::Conflict("No authored change to redo".into()))?;
        let receipt = self.apply_history(&h, true)?;
        self.redo.get_mut(author).unwrap().pop();
        self.history.push(h);
        Ok(receipt)
    }
    fn apply_history(&mut self, h: &History, redo: bool) -> Result<Receipt> {
        if let Some(controls) = &h.controls {
            if !self.controls_match(controls, redo) {
                return Err(Error::Conflict(
                    "Another client changed this resource; compact history would overwrite it"
                        .into(),
                ));
            }
            let revision = self.next_revision()?;
            let replacement = if redo {
                controls.after
            } else {
                controls.before
            };
            replacement.apply(self.control_track_mut(&controls.track_id)?);
            self.track_revisions
                .insert(controls.track_id.clone(), revision);
            self.revision = revision;
            return Ok(Receipt {
                revision,
                changed: vec![controls.track_id.clone()],
            });
        }
        let (expected, replacement) = if redo {
            (&h.before, &h.after)
        } else {
            (&h.after, &h.before)
        };
        let mut candidate = self.session.clone();
        if let Some((before, after)) = &h.order {
            let (expected, replacement) = if redo {
                (before, after)
            } else {
                (after, before)
            };
            if candidate.tracks.iter().map(|t| &t.id).ne(expected.iter()) {
                return Err(Error::Conflict(
                    "Another client changed track structure/order".into(),
                ));
            }
            // Apply resource replacements first, then restore this exact checked order.
            let _ = replacement;
        }
        for (id, expected) in expected {
            let current = if id.eq_ignore_ascii_case(crate::model::MASTER_ID) {
                Some(&candidate.master_track)
            } else {
                candidate.tracks.iter().find(|t| &t.id == id)
            };
            if current != expected.as_ref() {
                return Err(Error::Conflict(
                    "Another client changed this resource; history would overwrite it".into(),
                ));
            }
            match &replacement[id] {
                Some(track) if id.eq_ignore_ascii_case(crate::model::MASTER_ID) => {
                    candidate.master_track = track.clone()
                }
                Some(track) => {
                    if let Some(current) = candidate.tracks.iter_mut().find(|t| &t.id == id) {
                        *current = track.clone();
                    } else {
                        candidate.tracks.push(track.clone());
                    }
                }
                None => candidate.tracks.retain(|t| &t.id != id),
            }
        }
        if let Some((before, after)) = &h.order {
            let replacement = if redo { after } else { before };
            let mut tracks = candidate
                .tracks
                .into_iter()
                .map(|t| (t.id.clone(), t))
                .collect::<BTreeMap<_, _>>();
            candidate.tracks = replacement
                .iter()
                .map(|id| tracks.remove(id).expect("checked history membership"))
                .collect();
        }
        if let Some((before, after)) = &h.name {
            let (expected, replacement) = if redo {
                (before, after)
            } else {
                (after, before)
            };
            if &candidate.name != expected {
                return Err(Error::Conflict("Another client renamed the session".into()));
            }
            candidate.name = replacement.clone();
        }
        if let Some((before, after)) = &h.timing {
            if matches!(self.transport, Transport::Recording { .. }) {
                return Err(Error::Conflict(
                    "Tempo history cannot change during recording".into(),
                ));
            }
            let (expected, replacement) = if redo {
                (before, after)
            } else {
                (after, before)
            };
            if &candidate.timing != expected {
                return Err(Error::Conflict("Another client changed tempo".into()));
            }
            candidate.timing = replacement.clone();
        }
        if let Some((before, after)) = &h.metadata {
            let (expected, replacement) = if redo {
                (before, after)
            } else {
                (after, before)
            };
            if &candidate.extra != expected {
                return Err(Error::Conflict(
                    "Another client changed session metadata".into(),
                ));
            }
            candidate.extra = replacement.clone();
        }
        candidate.version = candidate.required_version();
        candidate.validate()?;
        let revision = self.next_revision()?;
        let mut changed = replacement.keys().cloned().collect::<Vec<_>>();
        let hashes = self.updated_hashes(&candidate, &changed)?;
        for id in &changed {
            if replacement[id].is_some() {
                self.track_revisions.insert(id.clone(), revision);
            } else {
                self.track_revisions.remove(id);
            }
        }
        if h.timing.is_some() {
            changed.push("timing".into())
        }
        if h.name.is_some() {
            changed.push("name".into())
        }
        if h.order.is_some() {
            changed.push("order".into())
        }
        if h.metadata.is_some() {
            changed.push("metadata".into())
        }
        self.session = candidate;
        self.content_hashes = hashes;
        self.revision = revision;
        Ok(Receipt { revision, changed })
    }
    /// Commits a prevalidated candidate with resource-scoped history. Structural changes
    /// additionally require the observed session revision so unseen additions cannot be lost.
    pub fn edit_session(
        &mut self,
        author: &str,
        role: Role,
        expected_revision: Option<u64>,
        expected: &BTreeMap<String, u64>,
        mut candidate: Session,
    ) -> Result<Receipt> {
        if role == Role::Viewer {
            return Err(Error::Conflict("Viewer cannot edit".into()));
        }
        if author.is_empty() || author.len() > 128 {
            return Err(Error::Invalid("Invalid author".into()));
        }
        if matches!(self.transport, Transport::Recording { .. }) {
            return Err(Error::Conflict(
                "Session edits are blocked during recording".into(),
            ));
        }
        candidate.version = candidate.required_version();
        candidate.validate()?;
        let old_order = self
            .session
            .tracks
            .iter()
            .map(|t| t.id.clone())
            .collect::<Vec<_>>();
        let new_order = candidate
            .tracks
            .iter()
            .map(|t| t.id.clone())
            .collect::<Vec<_>>();
        let structural = old_order != new_order
            || self.session.name != candidate.name
            || self.session.timing != candidate.timing;
        // Structural changes need a whole-session fence: the session revision,
        // or (as native clients send, with none given) the current version of
        // every existing track. An explicit stale revision always conflicts.
        let every_track_fenced = expected_revision.is_none()
            && old_order.iter().all(|id| {
                expected.get(id).is_some() && expected.get(id) == self.track_revisions.get(id)
            });
        let metadata = self.session.extra != candidate.extra;
        if (structural || metadata)
            && expected_revision != Some(self.revision)
            && !every_track_fenced
        {
            return Err(Error::Conflict("Stale session structure revision".into()));
        }
        let old = self
            .session
            .tracks
            .iter()
            .chain(std::iter::once(&self.session.master_track))
            .map(|t| (t.id.clone(), t.clone()))
            .collect::<BTreeMap<_, _>>();
        let new = candidate
            .tracks
            .iter()
            .chain(std::iter::once(&candidate.master_track))
            .map(|t| (t.id.clone(), t.clone()))
            .collect::<BTreeMap<_, _>>();
        let ids = old
            .keys()
            .chain(new.keys())
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let mut before = BTreeMap::new();
        let mut after = BTreeMap::new();
        for id in ids {
            if old.get(&id) == new.get(&id) {
                continue;
            }
            if old.contains_key(&id) && expected.get(&id) != self.track_revisions.get(&id) {
                return Err(Error::Conflict(format!("Stale resource {id}")));
            }
            before.insert(id.clone(), old.get(&id).cloned());
            after.insert(id.clone(), new.get(&id).cloned());
        }
        if before.is_empty() && !structural && !metadata {
            return Ok(Receipt {
                revision: self.revision,
                changed: vec![],
            });
        }
        let metadata = metadata.then(|| (self.session.extra.clone(), candidate.extra.clone()));
        let bytes = serde_json::to_vec(&before)?.len()
            + serde_json::to_vec(&after)?.len()
            + metadata
                .as_ref()
                .map_or(Ok(0), |m| serde_json::to_vec(m).map(|v| v.len()))?;
        if bytes > 32 * 1024 * 1024 {
            return Err(Error::Invalid("History exceeds metadata budget".into()));
        }
        let revision = self.next_revision()?;
        let timing = if self.session.timing != candidate.timing {
            Some((self.session.timing.clone(), candidate.timing.clone()))
        } else {
            None
        };
        let name = if self.session.name != candidate.name {
            Some((self.session.name.clone(), candidate.name.clone()))
        } else {
            None
        };
        let order = if old_order != new_order {
            Some((old_order, new_order))
        } else {
            None
        };
        let mut changed = after.keys().cloned().collect::<Vec<_>>();
        let hashes = self.updated_hashes(&candidate, &changed)?;
        for id in &changed {
            if after[id].is_some() {
                self.track_revisions.insert(id.clone(), revision);
            } else {
                self.track_revisions.remove(id);
            }
        }
        if timing.is_some() {
            changed.push("timing".into());
        }
        if name.is_some() {
            changed.push("name".into());
        }
        if order.is_some() {
            changed.push("order".into());
        }
        if metadata.is_some() {
            changed.push("metadata".into());
        }
        self.history.push(History {
            author: author.into(),
            label: "Edit tracks".into(),
            gesture: None,
            before,
            after,
            timing,
            name,
            order,
            metadata,
            encoded_bytes: bytes + 1024,
            controls: None,
        });
        self.redo.remove(author);
        self.trim_history();
        self.session = candidate;
        self.content_hashes = hashes;
        self.revision = revision;
        Ok(Receipt { revision, changed })
    }
    pub fn validate_transport(&self, role: Role, command: &TransportCommand) -> Result<()> {
        if role != Role::Operator {
            return Err(Error::Conflict(
                "Operator role required for transport".into(),
            ));
        }
        if let TransportCommand::Seek(frame) = command {
            if *frame > MAX_FRAME {
                return Err(Error::Invalid("Seek exceeds timeline bound".into()));
            }
            if matches!(self.transport, Transport::Recording { .. }) {
                return Err(Error::Conflict("Cannot seek during recording".into()));
            }
        }
        Ok(())
    }
    pub fn backend_committed_transport(&mut self, transport: Transport) -> Result<()> {
        let frame = match transport {
            Transport::Stopped { frame }
            | Transport::Playing { frame }
            | Transport::Recording { frame } => frame,
        };
        if frame > MAX_FRAME {
            return Err(Error::Invalid(
                "Backend position exceeds timeline bound".into(),
            ));
        }
        self.transport = transport;
        Ok(())
    }
}

#[cfg(test)]
mod compact_fingerprint_tests {
    use super::*;
    #[test]
    fn fingerprint_covers_every_non_control_serialized_track_field() {
        for scale in [None, Some("decibels".into())] {
            let mut track = Track {
                volume_automation_scale: scale,
                ..Default::default()
            };
            track.extra.insert(
                "futureData".into(),
                serde_json::json!({"meaning":"preserved"}),
            );
            let mut expected = serde_json::to_value(&track).unwrap();
            for name in ["gain", "pan", "muted", "solo", "armed"] {
                expected.as_object_mut().unwrap().remove(name);
            }
            assert_eq!(
                serde_json::to_value(track_content(&track)).unwrap(),
                expected
            );
            let before = content_fingerprint(&track).unwrap();
            track.gain = 0.5;
            track.pan = 0.2;
            track.muted = true;
            track.solo = true;
            track.armed = true;
            assert_eq!(before, content_fingerprint(&track).unwrap());
            track.extra.insert(
                "futureData".into(),
                serde_json::json!({"meaning":"changed"}),
            );
            assert_ne!(before, content_fingerprint(&track).unwrap());
        }
    }
}
