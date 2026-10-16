//! Authored candidate preparation outside the serialized control owner.
//! The captured context clones source content only, never foreign author history.
use super::*;
use soundroom_engine::authority::{SessionEditContext, SessionEditRequest};
use std::{fs, sync::atomic::AtomicBool, time::SystemTime};

pub(super) fn plain(command: &RemoteCommand) -> bool {
    matches!(
        command,
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
            | RemoteCommand::EditTracks { .. }
    )
}
pub(super) fn apply(mut candidate: Session, command: &RemoteCommand) -> Result<Session, Error> {
    match command {
        RemoteCommand::SetTrackName { track_id, name } => {
            if !valid_name(name) {
                return Err(protocol("Invalid track name"));
            }
            Service::track(&mut candidate, track_id)?.name = name.clone();
        }
        RemoteCommand::SetTrackGain { track_id, gain } => {
            Service::track(&mut candidate, track_id)?.gain = *gain
        }
        RemoteCommand::SetTrackPan { track_id, pan } => {
            Service::track(&mut candidate, track_id)?.pan = *pan
        }
        RemoteCommand::SetTrackMute { track_id, muted } => {
            Service::track(&mut candidate, track_id)?.muted = *muted
        }
        RemoteCommand::SetTrackSolo { track_id, solo } => {
            Service::track(&mut candidate, track_id)?.solo = *solo
        }
        RemoteCommand::SetTrackArm { track_id, armed } => {
            Service::track(&mut candidate, track_id)?.armed = *armed
        }
        RemoteCommand::UpdateTrackControls { controls, .. } => {
            let t = Service::track(&mut candidate, &controls.track_id)?;
            t.routing = controls
                .routing
                .as_ref()
                .map(|r| serde_json::from_value(serde_json::to_value(r)?))
                .transpose()?;
            t.name = controls.name.clone();
            t.gain = controls.gain;
            t.pan = controls.pan;
            t.color = controls.color as i32;
            t.muted = controls.muted;
            t.solo = controls.solo;
            t.armed = controls.armed;
            t.mix_automation =
                serde_json::from_value(serde_json::to_value(&controls.mix_automation)?)?;
            t.volume_automation_scale = controls.volume_automation_scale.as_ref().map(|s| {
                match s {
                    soundroom_client::models::AutomationValueScale::Linear => "linear",
                    soundroom_client::models::AutomationValueScale::Decibels => "decibels",
                }
                .into()
            });
            let mut effects = serde_json::to_value(&controls.realtime_effects)?;
            normalize_seeds(&mut effects, false);
            t.realtime_effects = serde_json::from_value(effects)?;
            for (key, value) in [
                ("inputDeviceUID", &controls.input_device_uid),
                ("inputDeviceName", &controls.input_device_name),
            ] {
                if let Some(value) = value {
                    t.extra.insert(key.into(), json!(value));
                } else {
                    t.extra.remove(key);
                }
            }
            t.extra
                .insert("inputChannel".into(), json!(controls.input_channel));
            t.extra
                .insert("stereoInput".into(), json!(controls.stereo_input));
        }
        RemoteCommand::AddTrack { name } => {
            if !valid_name(name) {
                return Err(protocol("Invalid track name"));
            }
            candidate.tracks.push(Track {
                id: new_id()?,
                name: name.clone(),
                ..Track::default()
            });
        }
        RemoteCommand::RemoveTrack { track_id } => {
            if !candidate
                .tracks
                .iter()
                .any(|t| t.id.eq_ignore_ascii_case(track_id))
            {
                return Err(Error::Conflict("Track is unavailable".into()));
            }
            candidate
                .tracks
                .retain(|t| !t.id.eq_ignore_ascii_case(track_id));
            for track in &mut candidate.tracks {
                if let Some(r) = &mut track.routing {
                    if r.output_id
                        .as_ref()
                        .is_some_and(|id| id.eq_ignore_ascii_case(track_id))
                    {
                        r.output_id = None;
                    }
                    if r.sidechain_source_id
                        .as_ref()
                        .is_some_and(|id| id.eq_ignore_ascii_case(track_id))
                    {
                        r.sidechain_source_id = None;
                    }
                    r.sends
                        .retain(|send| !send.track_id.eq_ignore_ascii_case(track_id));
                }
            }
        }
        RemoteCommand::MoveClip {
            track_id,
            clip_id,
            start,
        } => {
            let c = Service::track(&mut candidate, track_id)?
                .clips
                .iter_mut()
                .find(|c| c.id.eq_ignore_ascii_case(clip_id))
                .ok_or_else(|| Error::Conflict("Clip is unavailable".into()))?;
            c.start = *start as u64;
        }
        RemoteCommand::DeleteClips { track_id, clip_ids } => {
            let t = Service::track(&mut candidate, track_id)?;
            if clip_ids.is_empty()
                || clip_ids
                    .iter()
                    .any(|id| !t.clips.iter().any(|c| c.id.eq_ignore_ascii_case(id)))
            {
                return Err(Error::Conflict("Selected clip is unavailable".into()));
            }
            t.clips
                .retain(|c| !clip_ids.iter().any(|id| c.id.eq_ignore_ascii_case(id)));
        }
        RemoteCommand::SplitClip {
            track_id,
            clip_id,
            frame,
        } => {
            let t = Service::track(&mut candidate, track_id)?;
            let i = t
                .clips
                .iter()
                .position(|c| c.id.eq_ignore_ascii_case(clip_id))
                .ok_or_else(|| Error::Conflict("Clip is unavailable".into()))?;
            let c = &mut t.clips[i];
            let f = *frame as u64;
            if f <= c.start || f >= c.start + c.length {
                return Err(protocol("Split must be inside the clip"));
            }
            let left = f - c.start;
            let mut right = c.clone();
            right.id = new_id()?;
            right.start = f;
            right.offset += left;
            right.length -= left;
            c.length = left;
            // Persist the original fade coordinate system across both slices.
            let original_length = c.fade_reference_length.unwrap_or(left + right.length);
            let original_offset = c.fade_reference_offset.unwrap_or(c.offset);
            for clip in [&mut *c, &mut right] {
                clip.fade_reference_length = Some(original_length);
                clip.fade_reference_offset = Some(original_offset);
            }
            c.cut_fade_out_frames = Some(64);
            right.cut_fade_in_frames = Some(64);
            t.clips.insert(i + 1, right);
        }
        RemoteCommand::ReplaceTracks { tracks, .. } => {
            for track in tracks {
                let mut value = serde_json::to_value(track)?;
                normalize_seeds(&mut value, false);
                let t: Track = serde_json::from_value(value)?;
                let id = t.id.clone();
                *Service::track(&mut candidate, &id)? = t;
            }
        }
        RemoteCommand::EditTracks { edit } => {
            for track in &edit.tracks {
                let mut value = serde_json::to_value(track)?;
                normalize_seeds(&mut value, false);
                let t: Track = serde_json::from_value(value)?;
                if t.id.eq_ignore_ascii_case(&candidate.master_track.id) {
                    candidate.master_track = t;
                } else if let Some(old) = candidate.tracks.iter_mut().find(|old| old.id == t.id) {
                    *old = t;
                } else {
                    candidate.tracks.push(t);
                }
            }
            for id in &edit.removed_track_ids {
                if !candidate.tracks.iter().any(|t| &t.id == id) {
                    return Err(Error::Conflict("Removed track is unavailable".into()));
                }
                candidate.tracks.retain(|t| &t.id != id);
            }
            if let Some(order) = &edit.track_order {
                if order.len() != candidate.tracks.len() {
                    return Err(protocol("Track order must contain every track"));
                }
                let mut tracks = candidate
                    .tracks
                    .into_iter()
                    .map(|t| (t.id.clone(), t))
                    .collect::<BTreeMap<_, _>>();
                candidate.tracks = order
                    .iter()
                    .map(|id| {
                        tracks
                            .remove(id)
                            .ok_or_else(|| protocol("Track order repeats or omits a track"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
            }
        }
        _ => return Err(protocol("Unsupported prepared authored command")),
    }
    Ok(candidate)
}
pub(super) struct Task {
    context: SessionEditContext,
    media: BTreeMap<String, Media>,
    server_id: String,
    server_name: String,
    #[cfg(test)]
    hook: Option<Arc<dyn Fn() + Send + Sync>>,
}
#[derive(Clone, Debug, PartialEq)]
pub(super) struct AssetStamp {
    length: u64,
    modified: SystemTime,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}
impl AssetStamp {
    /// Matching inode identities prove these names refer to the same bytes.
    /// Size and timestamp alone (the fallback stamp) are insufficient.
    pub(super) fn same_file(&self, other: &Self) -> bool {
        #[cfg(unix)]
        {
            self == other
        }
        #[cfg(not(unix))]
        {
            let _ = other;
            false
        }
    }
}
pub(super) struct TransformAction<'a> {
    pub label: &'a str,
    pub gesture: Option<&'a str>,
    pub expected_revision: Option<u64>,
    pub expected: &'a BTreeMap<String, u64>,
}
pub(super) fn stamp(path: &Path) -> Result<AssetStamp, Error> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(protocol(
            "Authored source is no longer a regular custody file",
        ));
    }
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    Ok(AssetStamp {
        length: metadata.len(),
        modified: metadata.modified()?,
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
    })
}
impl Task {
    pub(super) fn capture(state: &Service, peer: &Identity) -> Result<Self, Error> {
        if peer.role == RemoteRole::Viewer
            || !state.registry.authorizes(&peer.credential, &peer.role)
        {
            return Err(protocol("Current editor or operator role is required"));
        }
        if !state.admits_job(peer, false) {
            return Err(Error::QueueFull);
        }
        Self::capture_reserved(state, peer)
    }
    /// Terminal Write authoring uses reserved capacity: its lease already passed
    /// bounded admission, and refusing here would discard a completed gesture.
    pub(super) fn capture_reserved(state: &Service, peer: &Identity) -> Result<Self, Error> {
        if peer.role == RemoteRole::Viewer
            || !state.registry.authorizes(&peer.credential, &peer.role)
        {
            return Err(protocol("Current editor or operator role is required"));
        }
        Ok(Self {
            context: state.authority.session_edit_context(),
            media: state.media.clone(),
            server_id: state.registry.server_id.clone(),
            server_name: state.registry.server_name.clone(),
            #[cfg(test)]
            hook: state.edit_hook.clone(),
        })
    }
    pub(super) fn run(
        self,
        shared: &Shared,
        peer: &Identity,
        request: &RemoteRequest,
        cancel: &AtomicBool,
    ) -> Result<Option<Value>, Error> {
        let (label, gesture) = history_action(&request.command);
        self.run_transform(
            shared,
            peer,
            cancel,
            TransformAction {
                label,
                gesture,
                expected_revision: request.expected_revision.map(|revision| revision as u64),
                expected: &Service::expected(request),
            },
            |before| apply(before, &request.command),
        )
        .map(|_| None)
    }
    /// Trusted worker-only transformation. Remote requests still enter normal
    /// dispatch; callers cannot select a closure, skip admission or forge tokens.
    pub(super) fn run_transform(
        self,
        shared: &Shared,
        peer: &Identity,
        cancel: &AtomicBool,
        action: TransformAction<'_>,
        transform: impl FnOnce(Session) -> Result<Session, Error>,
    ) -> Result<u64, Error> {
        let TransformAction {
            label,
            gesture,
            expected_revision,
            expected,
        } = action;
        let cancelled = || {
            if cancel.load(Ordering::Acquire) {
                Err(protocol("Authored preparation cancelled"))
            } else {
                Ok(())
            }
        };
        cancelled()?;
        let before = self.context.session().clone();
        let source_revision = self.context.revision();
        let mut candidate = transform(before.clone())?;
        candidate.version = candidate.required_version();
        if candidate.tracks.len() > 1023 {
            return Err(protocol(
                "Portable control supports at most 1,023 audio tracks plus master",
            ));
        }
        let sources = self
            .media
            .iter()
            .map(|(id, media)| Ok((id.clone(), stamp(&media.path)?)))
            .collect::<Result<BTreeMap<_, _>, Error>>()?;
        let project = Project {
            session: candidate,
            media: self.media.clone(),
        };
        project.validate_sources().map_err(engine)?;
        cancelled()?;
        let next = source_revision
            .checked_add(1)
            .filter(|next| *next <= soundroom_engine::model::MAX_FRAME)
            .ok_or_else(|| Error::Conflict("Revision exhausted".into()))?;
        let candidate = project.session;
        let versions = candidate
            .tracks
            .iter()
            .chain(std::iter::once(&candidate.master_track))
            .map(|track| (track.id.clone(), next))
            .collect::<BTreeMap<_, _>>();
        let mut document = json!({"serverID":self.server_id,"serverName":self.server_name,
            "session":wire(&candidate)?,"revision":next,"trackRevisions":versions,
            "transport":{},"status":"","peers":[],"devices":[],"inputDeviceID":0,"outputDeviceID":0,
            "assets":self.media.iter().map(|(id,media)|json!({"id":id,"frames":media.info.frames,"channels":media.info.channels,"peaks":[],"waveformStartFrame":0})).collect::<Vec<_>>()});
        neutralize_snapshot(&mut document);
        validation::validate_model(&document, "RemoteSnapshot")?;
        if encoded_size(&document)? > validation::MAX_SNAPSHOT_BYTES {
            return Err(protocol("Complete session document exceeds 64 MiB"));
        }
        drop(document);
        if !valid_name(label) || gesture.is_some_and(|g| !valid_name(g)) {
            return Err(protocol("Invalid history action or gesture"));
        }
        let prepared = self
            .context
            .prepare(
                SessionEditRequest {
                    author: &peer.runtime,
                    role: role(&peer.role),
                    expected_revision,
                    expected,
                    label,
                    gesture,
                },
                candidate,
            )
            .map_err(engine)?;
        // Materialize the next immutable publication witness on this worker.
        // Its authority revision is installed only after the token commits.
        let base = Arc::new(prepared.session().clone());
        let witness = SessionWitness {
            revision: prepared.receipt().revision,
            bytes: encoded_size(&wire(base.as_ref())?)?,
            controls: base
                .tracks
                .iter()
                .chain(std::iter::once(&base.master_track))
                .map(|track| (track.id.clone(), MixControls::from_track(track)))
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
        if !state.registry.authorizes(&peer.credential, &peer.role) {
            return Err(protocol(
                "Credential or role changed during authored preparation",
            ));
        }
        state.require_editable()?;
        if state.authority.revision != source_revision {
            return Err(Error::Conflict(
                "Session changed during authored preparation".into(),
            ));
        }
        // Custody files cannot be supplied by the request. Recheck actual file
        // identity/length/mtime at admission and after worker validation; content
        // SHA verification belongs to immutable import/recovery custody admission.
        if state.media.len() != self.media.len() {
            return Err(Error::Conflict(
                "Source catalog changed during preparation".into(),
            ));
        }
        for (id, media) in &self.media {
            if state.media.get(id).map(|current| &current.path) != Some(&media.path)
                || stamp(&media.path)? != sources[id]
            {
                return Err(Error::Conflict(
                    "Source file changed during authored preparation".into(),
                ));
            }
        }
        let receipt = state
            .authority
            .commit_prepared_session(&peer.runtime, role(&peer.role), prepared)
            .map_err(engine)?;
        if let Ok(mut cache) = state.witness.lock() {
            // No speculative worker may seed a future revision before admission:
            // that could misidentify another author's accepted session as ours.
            *cache = Some(witness);
        }
        if state.publish_changes(before, source_revision).is_err() {
            // Authored outcome is already committed; publication failure only
            // requires refresh and cannot turn it into a rejected receipt.
            for peer in state.peers.values() {
                peer.out.close();
            }
        }
        if state.authority.revision != source_revision {
            state.history_events();
        }
        Ok(receipt.revision)
    }
}

#[cfg(test)]
mod routing_tests {
    use super::*;
    use soundroom_engine::routing::{TrackRouting, TrackSend};
    fn route(role: &str) -> TrackRouting {
        TrackRouting {
            role: role.into(),
            output_id: None,
            sends: vec![],
            sidechain_source_id: None,
        }
    }
    fn fixture() -> Session {
        let mut session = Session {
            tracks: (0..3)
                .map(|i| Track {
                    id: format!("ABABABAB-0000-4000-8000-{i:012}"),
                    ..Track::default()
                })
                .collect(),
            ..Session::default()
        };
        session.tracks[1].routing = Some(route("bus"));
        session.tracks[2].routing = Some(route("return"));
        let mut r = route("audio");
        r.output_id = Some(session.tracks[1].id.clone());
        r.sidechain_source_id = Some(session.tracks[2].id.clone());
        r.sends = vec![];
        session.tracks[0].routing = Some(r);
        session
    }
    #[test]
    fn control_edits_preserve_routing_and_cannot_be_misclassified_as_scalar() {
        let session = fixture();
        let t = &session.tracks[0];
        let controls: soundroom_client::models::RemoteTrackControls=serde_json::from_value(json!({"trackID":t.id,"name":t.name,"color":t.color,"gain":0.5,"pan":t.pan,"muted":false,"solo":false,"armed":false,"inputChannel":0,"stereoInput":false,"mixAutomation":{},"realtimeEffects":[],"routing":t.routing})).unwrap();
        let result = apply(
            session.clone(),
            &RemoteCommand::UpdateTrackControls {
                controls: controls.clone(),
                name: "Route".into(),
                gesture_id: None,
            },
        )
        .unwrap();
        assert_eq!(result.tracks[0].routing, t.routing);
        result.validate().unwrap();
        let mut cleared = controls;
        cleared.routing = None;
        assert!(!unchanged_control_content(t, &cleared).unwrap());
        let result = apply(
            session,
            &RemoteCommand::UpdateTrackControls {
                controls: cleared,
                name: "Unroute".into(),
                gesture_id: None,
            },
        )
        .unwrap();
        assert!(result.tracks[0].routing.is_none());
    }
    #[test]
    fn deleting_destinations_cleans_outputs_sends_and_keys_and_is_undoable() {
        let mut session = fixture();
        session.tracks[0]
            .routing
            .as_mut()
            .unwrap()
            .sidechain_source_id = None;
        let send = TrackSend {
            track_id: session.tracks[2].id.clone(),
            level: 0.5,
        };
        session.tracks[0].routing.as_mut().unwrap().sends.push(send);
        session.validate().unwrap();
        let result = apply(
            session.clone(),
            &RemoteCommand::RemoveTrack {
                track_id: session.tracks[1].id.clone(),
            },
        )
        .unwrap();
        assert!(result.tracks[0]
            .routing
            .as_ref()
            .unwrap()
            .output_id
            .is_none());
        result.validate().unwrap();
        let result = apply(
            result,
            &RemoteCommand::RemoveTrack {
                track_id: session.tracks[2].id.clone(),
            },
        )
        .unwrap();
        assert!(result.tracks[0].routing.as_ref().unwrap().sends.is_empty());
        result.validate().unwrap();
        let mut authority = Authority::new(session.clone()).unwrap();
        let expected = authority.track_revisions.clone();
        authority
            .edit_session("routing-test", Role::Operator, Some(0), &expected, result)
            .unwrap();
        authority.undo("routing-test", Role::Operator).unwrap();
        assert_eq!(authority.session.tracks, session.tracks);
    }
}
