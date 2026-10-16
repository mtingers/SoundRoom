//! Mastering editor operations, ported from the Swift host: `master.analyze`
//! measures the full mix on a worker and stores the analysis as one authored
//! action; `master.proposal` returns the conservative rack for a still-current
//! analysis; `master.applyProposal` replaces the Master rack as one action.
use super::{engine, new_id, protocol, Error, Handled, Identity, JobKind, Service, Shared};
use base64::Engine;
use serde_json::Value;
use soundroom_client::{
    models::{RemoteCommand, RemoteEditorIntent, RemoteRequest},
    validation,
};
use soundroom_engine::{
    mastering::{self, MasteringAnalysis},
    model::Effect,
    project::Project,
};
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) const OPERATIONS: [&str; 3] =
    ["master.analyze", "master.proposal", "master.applyProposal"];

/// History labels, matching the native host's Undo names.
pub(super) fn label(operation: &str) -> Option<&'static str> {
    match operation {
        "master.analyze" => Some("Analyze master mix"),
        "master.applyProposal" => Some("Apply mastering proposal"),
        _ => None,
    }
}

/// Analysis and application are checked against the session under the lock
/// when committed, so an unfenced request is server-placed on that state.
pub(super) fn server_placed(request: &RemoteRequest) -> bool {
    let RemoteCommand::Editor { intent } = &request.command else {
        return false;
    };
    let unfenced =
        request.expected_revision.is_none() && request.expected_track_revisions.is_empty();
    intent.operation == "master.analyze" || (intent.operation == "master.applyProposal" && unfenced)
}

/// Worker-owned immutable document and media used for one full-mix analysis.
pub(super) struct Task {
    project: Box<Project>,
}
impl Task {
    pub(super) fn run(
        self,
        shared: &Shared,
        peer: &Identity,
        request: &RemoteRequest,
        cancel: &AtomicBool,
        progress: impl FnMut(u64, u64),
    ) -> Result<Option<Value>, Error> {
        let analysis = mastering::analyze(&self.project, cancel, progress).map_err(engine)?;
        if cancel.load(Ordering::Acquire) {
            return Err(protocol("Master analysis cancelled."));
        }
        let mut state = shared
            .lock()
            .map_err(|_| protocol("Authority is unavailable"))?;
        let before = state.authority.session.clone();
        let revision = state.authority.revision;
        if before != self.project.session {
            return Err(Error::Conflict("Mix changed; analyze again.".into()));
        }
        let mut candidate = before.clone();
        let value = serde_json::to_value(&analysis)?;
        validation::validate_model(&value, "MasteringAnalysis")?;
        candidate.extra.insert(mastering::SESSION_KEY.into(), value);
        candidate.version = candidate.version.max(7);
        state.commit(peer, request, candidate)?;
        if state.publish_changes(before, revision).is_err() {
            for peer in state.peers.values() {
                peer.out.close();
            }
        }
        if revision != state.authority.revision {
            state.history_events();
        }
        Ok(None)
    }
}

impl Service {
    pub(super) fn master_editor(
        &mut self,
        peer: &Identity,
        r: &RemoteRequest,
        intent: &RemoteEditorIntent,
    ) -> Result<Handled, Error> {
        match intent.operation.as_str() {
            "master.analyze" => {
                if self.recording.is_some() {
                    return Err(Error::Conflict(
                        "Finish recording before analyzing the mix.".into(),
                    ));
                }
                if self.authority.session.end() == 0 {
                    return Err(protocol("Add audio before analyzing the mix."));
                }
                Ok(Handled::Job(JobKind::Master {
                    task: Box::new(Task {
                        project: Box::new(Project {
                            session: self.authority.session.clone(),
                            media: self.media.clone(),
                        }),
                    }),
                }))
            }
            "master.proposal" => {
                let analysis = self.current_analysis()?;
                let target = intent.numbers.get("targetLUFS").copied().unwrap_or(-14.);
                let intensity = intent.numbers.get("intensity").copied().unwrap_or(0.65);
                if !(-24. ..=-8.).contains(&target) || !(0. ..=1.).contains(&intensity) {
                    return Err(protocol(
                        "Target loudness must be -24…-8 LUFS and intensity 0…1.",
                    ));
                }
                let effects = analysis
                    .proposal(target as f32, intensity as f32, || {
                        new_id().map_err(|e| soundroom_engine::Error::Invalid(e.to_string()))
                    })
                    .map_err(engine)?;
                Ok(Handled::Payload(Some(serde_json::to_value(effects)?)))
            }
            "master.applyProposal" => {
                self.current_analysis()?;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(
                        intent
                            .payload
                            .as_ref()
                            .ok_or_else(|| protocol("A mastering proposal is required"))?,
                    )
                    .map_err(|_| protocol("Invalid mastering proposal"))?;
                let value: Value = serde_json::from_slice(&bytes)?;
                validation::validate_model(&value, "[RealtimeEffect]")?;
                let effects: Vec<Effect> = serde_json::from_value(value)?;
                let mut candidate = self.authority.session.clone();
                candidate.master_track.realtime_effects = effects;
                candidate.validate().map_err(engine)?;
                self.commit(peer, r, candidate)?;
                Ok(Handled::Payload(None))
            }
            _ => Err(protocol("Unsupported mastering operation")),
        }
    }
    /// The stored analysis, only while it still describes the current mix.
    fn current_analysis(&self) -> Result<MasteringAnalysis, Error> {
        let session = &self.authority.session;
        let analysis = MasteringAnalysis::stored(session)
            .ok_or_else(|| protocol("Analyze the mix before requesting a mastering proposal."))?;
        if !analysis.matches(session) {
            return Err(protocol(
                "Mix changed; analyze again before using a proposal.",
            ));
        }
        Ok(analysis)
    }
}
