//! Worker-rendered selection processing: `effects.apply` (destructive built-in
//! effect over a track range) and `clips.combine`. The worker renders from an
//! immutable dry document; the commit re-checks the track revision and places
//! one server-authored, undoable action.
use super::{engine, new_id, protocol, Error, Handled, Identity, JobKind, Service};
use base64::Engine;
use serde_json::{json, Value};
use soundroom_client::models::RemoteEditorIntent;
use soundroom_engine::{
    model::{Clip, ClipHistory, FrozenEnvelope, Session, Track},
    project::{Media, Project},
    render::{default_parameters, export, export_with_tails},
    wave::{WaveReader, WaveWriter},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

const RATE: f32 = 48000.;
/// In-memory legacy processors (reverse, normalize) need the whole selection.
const MAX_LEGACY_FRAMES: u64 = 15 * 60 * 48000;
const MAX_RANGE_FRAMES: u64 = 60 * 60 * 48000;
const EFFECTS: &[(&str, &str)] = &[
    ("normalize", "Normalize"),
    ("gain", "Gain"),
    ("fadeIn", "Fade in"),
    ("fadeOut", "Fade out"),
    ("reverse", "Reverse"),
    ("echo", "Echo"),
    ("room", "Room reverb"),
    ("warmth", "Warmth"),
    ("lowPass", "Low-pass filter"),
    ("highPass", "High-pass filter"),
    ("compressor", "Compressor"),
    ("gate", "Gate / Expander"),
    ("utility", "Utility / Alignment"),
    ("guitarRig", "Guitar rig"),
    ("equalizer", "Channel EQ"),
    ("chorus", "Chorus"),
    ("dither", "Dither"),
    ("pitchCorrection", "Pitch correction"),
    ("limiter", "Master limiter"),
    ("resonance", "Resonance control"),
    ("soundPutty", "Sound Putty · Deep 300"),
    ("vocalEnhancer", "Vocal enhancer"),
    ("sourceIsolator", "Source isolator"),
];
const LEGACY_AMOUNT: &[&str] = &[
    "gain",
    "lowPass",
    "highPass",
    "echo",
    "room",
    "warmth",
    "compressor",
];
const NO_REALTIME: &[&str] = &["normalize", "fadeIn", "fadeOut", "reverse"];

pub(super) enum Operation {
    Separate {
        plan: super::separation::Plan,
    },
    Effect {
        kind: String,
        amount: f32,
        parameters: BTreeMap<String, f32>,
        tail: bool,
    },
    Combine {
        clip_ids: Vec<String>,
    },
    /// Render the project's (already wet) track; remove its first `prefix` inserts.
    Bake {
        prefix: usize,
    },
}
pub(super) struct Task {
    project: Project,
    track_id: String,
    start: u64,
    end: u64,
    operation: Operation,
    label: String,
    clip_name: String,
    revision: u64,
    directory: PathBuf,
    /// Audition owner: play the result instead of committing it.
    pub(super) preview: Option<(String, u64, Arc<AtomicBool>)>,
    /// Dry document for the history render (Bake only; other processing reuses its dry pass).
    original: Option<Project>,
}
pub(super) struct Processed {
    task: Task,
    path: PathBuf,
    frames: u64,
    /// Dry render of the replaced span, kept as clip history when the result
    /// does not replace exactly one whole clip.
    original: Option<PathBuf>,
}
impl Drop for Processed {
    fn drop(&mut self) {
        self.discard();
    }
}
impl Processed {
    pub(super) fn preview_allowed(
        &self,
        state: &Service,
        peer: &Identity,
        cancel: &AtomicBool,
    ) -> Result<(), Error> {
        cancelled(cancel)?;
        state.require_editable()?;
        if !state.registry.authorizes(&peer.credential, &peer.role) {
            return Err(protocol("The processing credential or role changed"));
        }
        if state.authority.track_revisions.get(&self.task.track_id) != Some(&self.task.revision) {
            return Err(Error::Conflict(
                "The track changed; the preview was not played".into(),
            ));
        }
        Ok(())
    }
}
struct Cleanup(Vec<PathBuf>);
impl Drop for Cleanup {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}
fn cancelled(cancel: &AtomicBool) -> Result<(), Error> {
    if cancel.load(Ordering::Acquire) {
        Err(protocol("Job cancelled"))
    } else {
        Ok(())
    }
}
fn unattenuated(length: u64) -> FrozenEnvelope {
    FrozenEnvelope {
        reference_length: length,
        reference_offset: 0,
        attack_frames: 0,
        release_frames: 0,
        boundary_attack_frames: 0,
        boundary_release_frames: 0,
        crossfade_in: None,
        crossfade_out: None,
        cut_reference_offset: None,
        cut_reference_length: None,
        additional_boundaries: vec![],
        extra: Default::default(),
    }
}
fn plain_clip(id: String, asset: String, name: String, start: u64, length: u64) -> Clip {
    Clip {
        id,
        asset_id: asset,
        name,
        start,
        offset: 0,
        length,
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
    }
}
/// The Swift `ClipState(clip:operation:)` snapshot: everything but placement.
fn clip_state(clip: &Clip, operation: &str) -> Result<ClipHistory, Error> {
    let mut state = serde_json::to_value(clip)?;
    let object = state
        .as_object_mut()
        .ok_or_else(|| protocol("Invalid clip"))?;
    object.remove("start");
    object.remove("previousStates");
    object.insert("id".into(), json!(new_id()?));
    object.insert("operation".into(), json!(operation));
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| protocol("System clock is invalid"))?
        .as_secs_f64()
        - 978307200.;
    object.insert("createdAt".into(), json!(seconds));
    Ok(serde_json::from_value(state)?)
}
/// `clip.remember(prior, operation:)`: newest first, five states.
/// Swift `Clip.remember(_:operation:)`: the prior's history plus its state,
/// oldest first, keeping the newest five; a no-op change adds nothing.
fn remember(clip: &mut Clip, prior: &Clip, operation: &str) -> Result<(), Error> {
    let state = clip_state(prior, operation)?;
    let comparable = |state: &ClipHistory| -> Result<Value, Error> {
        let mut value = serde_json::to_value(state)?;
        if let Some(object) = value.as_object_mut() {
            object.remove("id");
            object.remove("createdAt");
        }
        Ok(value)
    };
    if comparable(&state)? == comparable(&clip_state(clip, operation)?)? {
        return Ok(());
    }
    let mut states = prior.previous_states.clone();
    states.push(state);
    let excess = states.len().saturating_sub(5);
    states.drain(..excess);
    clip.previous_states = states;
    Ok(())
}
/// Swift `Clip.preserveFadeEnvelope(from:localOffset:)`: a cut piece keeps the
/// original's frozen envelope and fade geometry relative to the whole clip.
fn preserve_fade_envelope(clip: &mut Clip, original: &Clip, local_offset: u64) {
    if let Some(mut frozen) = original.render_envelope.clone() {
        frozen.reference_offset = frozen.reference_offset.saturating_add(local_offset);
        clip.render_envelope = Some(frozen);
    }
    let fades = original.fade_in_frames.unwrap_or(0) > 0
        || original.fade_out_frames.unwrap_or(0) > 0
        || original.edge_fade_frames.unwrap_or(0) > 0;
    if fades && (local_offset != 0 || clip.length != original.length) {
        clip.fade_reference_length =
            Some(original.fade_reference_length.unwrap_or(original.length));
        clip.fade_reference_offset = Some(
            original
                .fade_reference_offset
                .unwrap_or(0)
                .saturating_add(local_offset),
        );
    }
}
/// Resolved (neighbour-aware) frozen envelope for every clip on the track.
fn frozen_envelopes(track: &Track) -> BTreeMap<String, FrozenEnvelope> {
    soundroom_engine::envelope::Envelope::resolve(track)
        .iter()
        .map(|e| (e.clip.id.clone(), e.frozen()))
        .collect()
}
/// `Session.delete(trackID:range:)`: clips are
/// cut at the range edges and the cut sides use the clip's edge fade.
fn delete_range(track: &mut Track, start: u64, end: u64, operation: &str) -> Result<(), Error> {
    let envelopes = frozen_envelopes(track);
    let mut clips = vec![];
    for clip in std::mem::take(&mut track.clips) {
        let clip_end = clip.start + clip.length;
        let mut source = clip.clone();
        if clip.render_envelope.is_some() {
            source.render_envelope = envelopes.get(&clip.id).cloned().or(source.render_envelope);
        }
        if clip.start >= end || clip_end <= start {
            clips.push(clip);
            continue;
        }
        if clip.start < start {
            let mut left = clip.clone();
            left.length = start - clip.start;
            left.cut_fade_out_frames = clip.edge_fade_frames;
            preserve_fade_envelope(&mut left, &source, 0);
            remember(&mut left, &clip, operation)?;
            clips.push(left);
        }
        if clip_end > end {
            let mut right = clip.clone();
            right.id = new_id()?;
            right.start = end;
            right.offset += end - clip.start;
            right.length = clip_end - end;
            right.cut_fade_in_frames = clip.edge_fade_frames;
            preserve_fade_envelope(&mut right, &source, end - clip.start);
            remember(&mut right, &clip, operation)?;
            clips.push(right);
        }
    }
    track.clips = clips;
    Ok(())
}
/// Replaces the selected clips with `clip` (Swift `ClipCombining.prepare`):
/// history from every original, original ordering, and neighbours keep the
/// envelope they had.
fn combine_into(track: &mut Track, mut clip: Clip, clip_ids: &[String]) -> Result<(), Error> {
    let clip_id = clip.id.clone();
    let selected: BTreeSet<&String> = clip_ids.iter().collect();
    let originals: Vec<Clip> = track
        .clips
        .iter()
        .filter(|c| selected.contains(&c.id.to_ascii_uppercase()))
        .cloned()
        .collect();
    if originals.len() != clip_ids.len() {
        return Err(Error::Conflict("A selected clip is unavailable".into()));
    }
    let mut states = vec![];
    for original in &originals {
        states.extend(original.previous_states.iter().cloned());
        states.push(clip_state(original, "Combine clips")?);
    }
    let created = |s: &ClipHistory| {
        s.extra
            .get("createdAt")
            .and_then(Value::as_f64)
            .unwrap_or(0.)
    };
    states.sort_by(|a, b| created(a).total_cmp(&created(b)));
    let excess = states.len().saturating_sub(5);
    states.drain(..excess);
    clip.previous_states = states;
    let before = frozen_envelopes(track);
    let mut replacement = vec![];
    let mut inserted = false;
    for existing in std::mem::take(&mut track.clips) {
        if selected.contains(&existing.id.to_ascii_uppercase()) {
            if !inserted {
                replacement.push(clip.clone());
                inserted = true;
            }
        } else {
            replacement.push(existing);
        }
    }
    track.clips = replacement;
    // Unselected neighbours keep exactly the envelope they had.
    let after = frozen_envelopes(track);
    for existing in &mut track.clips {
        if existing.id == clip_id || selected.contains(&existing.id.to_ascii_uppercase()) {
            continue;
        }
        if let (Some(old), Some(new)) = (before.get(&existing.id), after.get(&existing.id)) {
            if old != new {
                existing.render_envelope = Some(old.clone());
            }
        }
    }
    Ok(())
}
fn read_all(path: &std::path::Path) -> Result<[Vec<f32>; 2], Error> {
    let mut reader = WaveReader::open(path).map_err(engine)?;
    let frames = reader.info.frames as usize;
    let (mut left, mut right) = (vec![0f32; frames], vec![0f32; frames]);
    for start in (0..frames).step_by(8192) {
        let end = (start + 8192).min(frames);
        reader
            .read_stereo(start as u64, &mut left[start..end], &mut right[start..end])
            .map_err(engine)?;
    }
    Ok([left, right])
}
fn write_all(path: &std::path::Path, audio: &[Vec<f32>; 2]) -> Result<(), Error> {
    let mut writer = WaveWriter::create(path, audio[0].len() as u64).map_err(engine)?;
    for (left, right) in audio[0].chunks(8192).zip(audio[1].chunks(8192)) {
        writer.write(left, right).map_err(engine)?;
    }
    writer.finish().map_err(engine)
}
/// `BuiltInEffect.process(_:amount:)`, the legacy one-knob processors.
fn legacy(
    kind: &str,
    amount: f32,
    audio: &mut [Vec<f32>; 2],
    cancel: &AtomicBool,
) -> Result<(), Error> {
    let a = amount.clamp(0., 1.);
    let peak = if kind == "normalize" {
        audio
            .iter()
            .flat_map(|c| c.iter())
            .fold(0f32, |p, v| p.max(v.abs()))
    } else {
        0.
    };
    let normalized = if peak > 0. {
        10f32.powf(-1. / 20.) / peak
    } else {
        1.
    };
    for input in audio.iter_mut() {
        cancelled(cancel)?;
        let n = input.len();
        match kind {
            "reverse" => input.reverse(),
            "normalize" => input.iter_mut().for_each(|v| *v *= normalized),
            "gain" => {
                let gain = 10f32.powf((a * 48. - 24.) / 20.);
                input.iter_mut().for_each(|v| *v *= gain)
            }
            "fadeIn" | "fadeOut" => {
                let denominator = (n.max(2) - 1) as f32;
                for (i, v) in input.iter_mut().enumerate() {
                    let progress = i as f32 / denominator;
                    *v *= if kind == "fadeIn" {
                        progress
                    } else {
                        1. - progress
                    };
                }
            }
            "echo" | "room" => {
                let echo = kind == "echo";
                let delays: &[usize] = if echo {
                    &[12000]
                } else {
                    &[1423, 2137, 3163, 4789]
                };
                let dry = input.clone();
                for &delay in delays {
                    let mut line = vec![0f32; delay];
                    for i in 0..n {
                        let delayed = line[i % delay];
                        line[i % delay] = dry[i] + delayed * if echo { 0.4 } else { 0.65 };
                        input[i] += delayed * a * if echo { 0.65 } else { 0.16 };
                    }
                }
            }
            "warmth" => {
                let drive = 1. + a * 8.;
                input
                    .iter_mut()
                    .for_each(|v| *v = (*v * drive).tanh() / drive.tanh())
            }
            "lowPass" | "highPass" => {
                let hz = if kind == "lowPass" {
                    200. * 80f32.powf(1. - a)
                } else {
                    30. * 100f32.powf(a)
                };
                let coefficient = 1. - (-2. * std::f32::consts::PI * hz / RATE).exp();
                let mut state = 0f32;
                for v in input.iter_mut() {
                    state += coefficient * (*v - state);
                    *v = if kind == "lowPass" { state } else { *v - state };
                }
            }
            "compressor" => {
                let threshold = 10f32.powf((-6. - a * 24.) / 20.);
                let attack = 1. - (-1. / (0.005 * RATE)).exp();
                let release = 1. - (-1. / (0.1 * RATE)).exp();
                let mut envelope = 0f32;
                for v in input.iter_mut() {
                    let level = v.abs();
                    envelope +=
                        if level > envelope { attack } else { release } * (level - envelope);
                    if envelope > threshold {
                        *v *= (threshold / envelope).powf(0.75);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}
impl Task {
    pub(super) fn run(
        self,
        cancel: &AtomicBool,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<Processed, Error> {
        std::fs::create_dir_all(&self.directory)?;
        let id = new_id()?;
        let dry = self.directory.join(format!("{id}.dry.wav"));
        let out = self.directory.join(format!("{id}.wav"));
        let mut cleanup = Cleanup(vec![dry.clone(), out.clone()]);
        export(
            &self.project,
            &dry,
            self.start,
            self.end,
            cancel,
            |done, total| progress(done, total * 2),
        )
        .map_err(engine)?;
        cancelled(cancel)?;
        let frames = self.end - self.start;
        match &self.operation {
            Operation::Separate { plan } => {
                super::separation::run(plan, &dry, &out, frames, cancel, |done, total| {
                    progress(total + done, total * 2)
                })?;
            }
            Operation::Combine { .. } | Operation::Bake { .. } => {
                std::fs::rename(&dry, &out)?;
                if let Some(original) = &self.original {
                    export(original, &dry, self.start, self.end, cancel, |_, _| {})
                        .map_err(engine)?;
                }
            }
            Operation::Effect {
                kind,
                amount,
                parameters,
                tail,
            } => {
                if NO_REALTIME.contains(&kind.as_str())
                    || (parameters.is_empty() && LEGACY_AMOUNT.contains(&kind.as_str()))
                {
                    if frames > MAX_LEGACY_FRAMES {
                        return Err(protocol("Select at most 15 minutes for this effect"));
                    }
                    let mut audio = read_all(&dry)?;
                    legacy(kind, *amount, &mut audio, cancel)?;
                    write_all(&out, &audio)?;
                } else {
                    let info = WaveReader::open(&dry).map_err(engine)?.info;
                    let source = new_id()?;
                    let mut clip = plain_clip(new_id()?, source.clone(), "Dry".into(), 0, frames);
                    clip.edge_fade_frames = Some(0);
                    clip.edge_protection_disabled = Some(true);
                    clip.render_envelope = Some(unattenuated(frames));
                    let session = Session {
                        tracks: vec![Track {
                            id: new_id()?,
                            name: "Process".into(),
                            clips: vec![clip],
                            realtime_effects: vec![soundroom_engine::model::Effect {
                                id: new_id()?,
                                kind: kind.clone(),
                                amount: *amount,
                                enabled: true,
                                automation: vec![],
                                parameters: if parameters.is_empty() {
                                    default_parameters(kind)
                                } else {
                                    parameters.clone()
                                },
                                parameter_automation: BTreeMap::new(),
                                extra: Default::default(),
                            }],
                            ..Track::default()
                        }],
                        ..Session::default()
                    };
                    let project = Project {
                        session,
                        media: BTreeMap::from([(
                            source,
                            Media {
                                path: dry.clone(),
                                info,
                            },
                        )]),
                    };
                    export_with_tails(&project, &out, 0, frames, *tail, cancel, |done, total| {
                        progress(total + done, total * 2)
                    })
                    .map_err(engine)?;
                }
            }
        }
        cancelled(cancel)?;
        let frames = WaveReader::open(&out).map_err(engine)?.info.frames;
        let original = (matches!(
            self.operation,
            Operation::Effect { .. } | Operation::Separate { .. }
        ) || self.original.is_some())
        .then(|| dry.clone());
        cleanup
            .0
            .retain(|p| p != &out && Some(p) != original.as_ref());
        Ok(Processed {
            task: self,
            path: out,
            frames,
            original,
        })
    }
}
impl Processed {
    pub(super) fn path(&self) -> &std::path::Path {
        &self.path
    }
    pub(super) fn discard(&self) {
        let _ = std::fs::remove_file(&self.path);
        if let Some(original) = &self.original {
            let _ = std::fs::remove_file(original);
        }
    }
}
fn payload<T: serde::de::DeserializeOwned>(
    intent: &RemoteEditorIntent,
) -> Result<Option<T>, Error> {
    let Some(payload) = &intent.payload else {
        return Ok(None);
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|_| protocol("Invalid payload"))?;
    Ok(Some(serde_json::from_slice(&bytes)?))
}
impl Service {
    pub(super) fn dry_project(&self, track_id: &str, keep: Option<&BTreeSet<String>>) -> Project {
        let mut session = self.authority.session.clone();
        session
            .tracks
            .retain(|t| t.id.eq_ignore_ascii_case(track_id));
        for track in &mut session.tracks {
            track.muted = false;
            track.solo = false;
            track.gain = 1.;
            track.pan = 0.;
            track.mix_automation.clear();
            track.realtime_effects.clear();
            track.routing = None;
            if let Some(keep) = keep {
                track
                    .clips
                    .retain(|c| keep.contains(&c.id.to_ascii_uppercase()));
            }
        }
        session.master_track = Session::default().master_track;
        Project {
            session,
            media: self.media.clone(),
        }
    }
    fn track_revision(&self, track_id: &str) -> Result<(String, u64), Error> {
        self.authority
            .track_revisions
            .iter()
            .find(|(id, _)| id.eq_ignore_ascii_case(track_id))
            .map(|(id, v)| (id.clone(), *v))
            .ok_or_else(|| Error::Conflict("Choose a track to process.".into()))
    }
    pub(super) fn begin_effect(&mut self, intent: &RemoteEditorIntent) -> Result<Handled, Error> {
        self.require_editable()?;
        let kind = intent
            .strings
            .get("kind")
            .filter(|k| EFFECTS.iter().any(|(id, _)| id == k))
            .ok_or_else(|| protocol("Unknown effect"))?
            .clone();
        let parameters: BTreeMap<String, f32> = payload(intent)?.unwrap_or_default();
        let limit = if kind == "soundPutty" { 916 } else { 64 };
        if parameters.len() > limit || parameters.values().any(|v| !v.is_finite()) {
            return Err(protocol("Invalid effect parameters"));
        }
        let track = intent
            .context
            .track_id
            .as_deref()
            .ok_or_else(|| protocol("Choose a track to process."))?;
        let (start, end) = match (intent.context.range_start, intent.context.range_end) {
            (Some(a), Some(b)) if a >= 0 && b > a => (a as u64, b as u64),
            _ => return Err(protocol("Select audio to process.")),
        };
        if end - start > MAX_RANGE_FRAMES || end > soundroom_engine::model::MAX_FRAME {
            return Err(protocol("Select at most one hour to process"));
        }
        let separation = if intent.flags.get("separateSource") == Some(&true) {
            if end - start > super::separation::MAX_FRAMES {
                return Err(protocol("Select at most 30 minutes for source separation"));
            }
            Some(super::separation::plan(&self.root, intent)?)
        } else {
            None
        };
        let (track_id, revision) = self.track_revision(track)?;
        let name = separation
            .as_ref()
            .map(|plan| plan.label())
            .unwrap_or_else(|| EFFECTS.iter().find(|(id, _)| *id == kind).unwrap().1.into());
        let source = self.source_name(&track_id, start, end);
        Ok(Handled::Job(JobKind::Process {
            task: Box::new(Task {
                project: self.dry_project(&track_id, None),
                track_id,
                start,
                end,
                operation: match separation {
                    Some(plan) => Operation::Separate { plan },
                    None => Operation::Effect {
                        kind: kind.clone(),
                        amount: intent
                            .numbers
                            .get("amount")
                            .copied()
                            .unwrap_or(0.5)
                            .clamp(0., 1.) as f32,
                        parameters,
                        tail: intent.flags.get("tail") == Some(&true),
                    },
                },
                label: name.clone(),
                clip_name: format!("{source} · {name}").chars().take(120).collect(),
                revision,
                directory: self.root.join("Artifacts"),
                preview: None,
                original: None,
            }),
        }))
    }
    /// `pitch.manualApply`: tune one region inside the selection to a fixed note.
    pub(super) fn begin_manual_pitch(
        &mut self,
        intent: &RemoteEditorIntent,
    ) -> Result<Handled, Error> {
        let get = |key: &str| {
            intent
                .integers
                .get(key)
                .copied()
                .filter(|v| *v >= 0)
                .map(|v| v as u64)
                .ok_or_else(|| protocol("Invalid manual tuning range"))
        };
        let (start, end, a, b) = (
            get("start")?,
            get("end")?,
            get("selectionStart")?,
            get("selectionEnd")?,
        );
        let target = intent.integers.get("target").copied().unwrap_or(60);
        if start >= end
            || a >= b
            || start < a
            || end > b
            || !(0..=95).contains(&target)
            || end - start > MAX_RANGE_FRAMES
        {
            return Err(protocol("Invalid manual tuning range"));
        }
        let track = intent
            .strings
            .get("trackID")
            .ok_or_else(|| protocol("Choose a track to tune."))?;
        let mut parameters: BTreeMap<String, f32> = payload(intent)?.unwrap_or_default();
        if parameters.len() > 64 || parameters.values().any(|v| !v.is_finite()) {
            return Err(protocol("Invalid effect parameters"));
        }
        if parameters.is_empty() {
            parameters = default_parameters("pitchCorrection");
        }
        parameters.insert("manualTarget".into(), target as f32);
        let (track_id, revision) = self.track_revision(track)?;
        let name = self.source_name(&track_id, start, end);
        Ok(Handled::Job(JobKind::Process {
            task: Box::new(Task {
                project: self.dry_project(&track_id, None),
                track_id,
                start,
                end,
                operation: Operation::Effect {
                    kind: "pitchCorrection".into(),
                    amount: 0.5,
                    parameters,
                    tail: false,
                },
                label: "Manual pitch tuning".into(),
                clip_name: format!("{name} · Tuned").chars().take(120).collect(),
                revision,
                directory: self.root.join("Artifacts"),
                preview: None,
                original: None,
            }),
        }))
    }
    /// `pitch.apply` (Swift `PitchAudioCommit.plan`): a first edits-only pitch
    /// insert bakes its edited regions; otherwise the full chain bounces the
    /// whole track plus capped tails.
    pub(super) fn begin_bake(&mut self, intent: &RemoteEditorIntent) -> Result<Handled, Error> {
        if self.recording.is_some() {
            return Err(Error::Conflict("Finish recording first".into()));
        }
        let (track, effect) = (
            intent
                .strings
                .get("trackID")
                .ok_or_else(|| protocol("Track is required"))?,
            intent
                .strings
                .get("effectID")
                .ok_or_else(|| protocol("Effect is required"))?,
        );
        let (track_id, revision) = self.track_revision(track)?;
        let source = self
            .authority
            .session
            .tracks
            .iter()
            .find(|t| t.id == track_id)
            .ok_or_else(|| protocol("Track is unavailable"))?;
        let index = source
            .realtime_effects
            .iter()
            .position(|e| e.id.eq_ignore_ascii_case(effect) && e.enabled)
            .ok_or_else(|| protocol("Choose an enabled effect with audio to apply."))?;
        if source
            .routing
            .as_ref()
            .is_some_and(|r| r.sidechain_source_id.is_some())
            && source.realtime_effects[..=index]
                .iter()
                .any(|e| e.kind == "compressor")
        {
            return Err(protocol("Externally sidechained compression stays live. Export this track to render it with its key."));
        }
        let end = source
            .clips
            .iter()
            .map(|c| c.start + c.length)
            .max()
            .unwrap_or(0);
        if end == 0 {
            return Err(protocol("This track has no audio to process."));
        }
        let pitch = &source.realtime_effects[index];
        let edits_only = index == 0
            && pitch.kind == "pitchCorrection"
            && pitch.extra.get("pitchEditsOnly") == Some(&json!(true));
        let (start, stop, prefix) = if edits_only {
            let regions: Vec<soundroom_engine::pitch::Region> = pitch
                .extra
                .get("pitchEdits")
                .map(|v| serde_json::from_value(v.clone()))
                .transpose()?
                .unwrap_or_default();
            let regions: Vec<_> = regions
                .into_iter()
                .filter(|r| r.start_frame < r.end_frame && !r.points.is_empty())
                .collect();
            let first = regions.iter().map(|r| r.start_frame).min();
            let last = regions.iter().map(|r| r.end_frame).max();
            match (first, last) {
                (Some(a), Some(b)) if a < end.min(b) => (a, end.min(b), 1),
                _ => return Err(protocol("Edit pitch targets over audio before applying.")),
            }
        } else {
            let tail = {
                let mut wet = self.dry_project(&track_id, None);
                wet.session.tracks[0].realtime_effects = source.realtime_effects.clone();
                soundroom_engine::render::Renderer::new(&wet)
                    .map(|r| r.tail_frames())
                    .unwrap_or(0)
            };
            (0, end + tail.min(30 * 48000), source.realtime_effects.len())
        };
        if stop - start > MAX_RANGE_FRAMES {
            return Err(protocol("Apply at most one hour of audio"));
        }
        let mut original = self.dry_project(&track_id, None);
        let envelopes = frozen_envelopes(source);
        for clip in &mut original.session.tracks[0].clips {
            clip.render_envelope = envelopes.get(&clip.id).cloned();
        }
        let mut project = Project {
            session: original.session.clone(),
            media: original.media.clone(),
        };
        project.session.tracks[0].realtime_effects = source.realtime_effects[..prefix].to_vec();
        let effect_name = if pitch.kind == "pitchCorrection" {
            "Pitch".to_string()
        } else {
            EFFECTS
                .iter()
                .find(|(id, _)| *id == pitch.kind)
                .map_or("Effects".into(), |(_, n)| n.to_string())
        };
        Ok(Handled::Job(JobKind::Process {
            task: Box::new(Task {
                project,
                track_id,
                start,
                end: stop,
                operation: Operation::Bake { prefix },
                label: format!("Apply {} to audio", effect_name.to_lowercase()),
                clip_name: format!("{} · {effect_name}", source.name)
                    .chars()
                    .take(120)
                    .collect(),
                revision,
                directory: self.root.join("Artifacts"),
                preview: None,
                original: Some(original),
            }),
        }))
    }
    fn source_name(&self, track_id: &str, start: u64, end: u64) -> String {
        self.authority
            .session
            .tracks
            .iter()
            .find(|t| t.id == track_id)
            .map(|t| {
                t.clips
                    .iter()
                    .find(|c| c.start < end && c.start + c.length > start)
                    .map_or(t.name.clone(), |c| c.name.clone())
            })
            .unwrap_or_default()
    }
    pub(super) fn begin_combine(
        &mut self,
        peer: &Identity,
        intent: &RemoteEditorIntent,
    ) -> Result<Handled, Error> {
        if self.recording.is_some() {
            return Err(Error::Conflict("Finish recording first".into()));
        }
        let selected: BTreeSet<String> = intent
            .context
            .clip_ids
            .iter()
            .map(|s| s.to_ascii_uppercase())
            .collect();
        // One track per request: the first track holding two or more selected clips.
        let track = self
            .authority
            .session
            .tracks
            .iter()
            .find(|t| {
                t.clips
                    .iter()
                    .filter(|c| selected.contains(&c.id.to_ascii_uppercase()))
                    .count()
                    >= 2
            })
            .ok_or_else(|| protocol("Select two or more clips on a track to combine them."))?;
        let clips: Vec<&Clip> = track
            .clips
            .iter()
            .filter(|c| selected.contains(&c.id.to_ascii_uppercase()))
            .collect();
        let start = clips.iter().map(|c| c.start).min().unwrap();
        let end = clips.iter().map(|c| c.start + c.length).max().unwrap();
        if end - start > MAX_RANGE_FRAMES {
            return Err(protocol("Combined clips must span at most one hour"));
        }
        let ids: BTreeSet<String> = clips.iter().map(|c| c.id.to_ascii_uppercase()).collect();
        // Contiguous pieces of one source rejoin without rendering when that
        // provably sounds the same.
        let mut ordered: Vec<Clip> = clips.iter().map(|c| (*c).clone()).collect();
        ordered.sort_by_key(|c| c.start);
        let resolved = soundroom_engine::envelope::Envelope::resolve(track);
        let states: Vec<&soundroom_engine::envelope::Envelope> = ordered
            .iter()
            .filter_map(|c| resolved.iter().find(|e| e.clip.id == c.id))
            .collect();
        if let Some(mut joined) =
            soundroom_engine::envelope::Envelope::lossless_join(&ordered, &states)
        {
            joined.id = new_id()?;
            joined.previous_states.clear();
            let track_id = track.id.clone();
            return self.commit_lossless_combine(peer, &track_id, &ids, joined);
        }
        let envelopes = frozen_envelopes(track);
        let (track_id, revision) = self.track_revision(&track.id.clone())?;
        let mut project = self.dry_project(&track_id, Some(&ids));
        for clip in &mut project.session.tracks[0].clips {
            clip.render_envelope = envelopes.get(&clip.id).cloned();
        }
        Ok(Handled::Job(JobKind::Process {
            task: Box::new(Task {
                project,
                track_id,
                start,
                end,
                operation: Operation::Combine {
                    clip_ids: ids.into_iter().collect(),
                },
                label: "Combine clips".into(),
                clip_name: "Combined clips".into(),
                revision,
                directory: self.root.join("Artifacts"),
                preview: None,
                original: None,
            }),
        }))
    }
    fn commit_lossless_combine(
        &mut self,
        peer: &Identity,
        track_id: &str,
        ids: &BTreeSet<String>,
        joined: Clip,
    ) -> Result<Handled, Error> {
        self.require_editable()?;
        let mut candidate = self.authority.session.clone();
        let track = Self::track(&mut candidate, track_id)?;
        let (clip_id, start, length) = (joined.id.clone(), joined.start, joined.length);
        combine_into(track, joined, &ids.iter().cloned().collect::<Vec<_>>())?;
        let revision = self.authority.revision;
        let versions = self.authority.track_revisions.clone();
        self.authority
            .edit_session(
                &peer.runtime,
                super::role(&peer.role),
                Some(revision),
                &versions,
                candidate,
            )
            .map_err(engine)?;
        if self.authority.revision != revision {
            self.authority
                .label_last_change(&peer.runtime, "Combine clips", None)
                .map_err(engine)?;
        }
        Ok(Handled::Payload(Some(json!({
            "trackID": track_id, "clipID": clip_id, "clipIDs": [clip_id],
            "cursorFrame": start, "rangeStart": start, "rangeEnd": start + length,
        }))))
    }
    /// Places the rendered result as one authored action. Returns the editor
    /// context the GUI adopts as its selection.
    pub(super) fn commit_processed(
        &mut self,
        peer: &Identity,
        processed: &Processed,
    ) -> Result<Value, Error> {
        self.require_editable()?;
        if !self.registry.authorizes(&peer.credential, &peer.role) {
            return Err(protocol("The processing credential or role changed"));
        }
        let task = &processed.task;
        if self.authority.track_revisions.get(&task.track_id) != Some(&task.revision) {
            return Err(Error::Conflict(
                "The track changed; the result was not applied".into(),
            ));
        }
        let mut candidate = self.authority.session.clone();
        let track = Self::track(&mut candidate, &task.track_id)?;
        let asset = new_id()?;
        let clip_id = new_id()?;
        let mut clip = plain_clip(
            clip_id.clone(),
            asset.clone(),
            task.clip_name.clone(),
            task.start,
            processed.frames,
        );
        // Swift keeps the dry span as an aligned history clip when the result
        // does not replace exactly one whole clip.
        let mut history: Option<(String, PathBuf)> = None;
        let mut remember_original = |clip: &mut Clip, name: &str| -> Result<(), Error> {
            let Some(path) = &processed.original else {
                return Ok(());
            };
            let source = new_id()?;
            let frames = WaveReader::open(path).map_err(engine)?.info.frames;
            let mut prior = plain_clip(new_id()?, source.clone(), name.into(), task.start, frames);
            prior.edge_fade_frames = Some(0);
            prior.render_envelope = Some(unattenuated(frames));
            remember(clip, &prior, &task.label)?;
            history = Some((source, path.clone()));
            Ok(())
        };
        match &task.operation {
            Operation::Effect { .. } | Operation::Separate { .. } => {
                let overlapping: Vec<Clip> = track
                    .clips
                    .iter()
                    .filter(|c| c.start < task.end && c.start + c.length > task.start)
                    .cloned()
                    .collect();
                clip.edge_fade_frames = Some(
                    overlapping
                        .iter()
                        .filter_map(|c| c.edge_fade_frames)
                        .max()
                        .unwrap_or(super::EDGE_FADE_FRAMES),
                );
                if !overlapping.is_empty()
                    && overlapping
                        .iter()
                        .all(|c| c.edge_protection_disabled == Some(true))
                {
                    clip.edge_protection_disabled = Some(true);
                }
                if let [original] = overlapping.as_slice() {
                    if original.start <= task.start && original.start + original.length >= task.end
                    {
                        let mut prior = original.clone();
                        prior.offset += task.start - original.start;
                        prior.length = (task.end - task.start).max(1);
                        if task.start != original.start
                            || task.end != original.start + original.length
                        {
                            prior.extra.remove("soundDesign");
                        }
                        preserve_fade_envelope(&mut prior, original, task.start - original.start);
                        clip.previous_states = prior.previous_states.clone();
                        remember(&mut clip, &prior, &task.label)?;
                    } else {
                        remember_original(&mut clip, "Original selection")?;
                    }
                } else if !overlapping.is_empty() {
                    remember_original(&mut clip, "Original selection")?;
                }
                delete_range(track, task.start, task.end, &task.label)?;
                track.clips.push(clip);
            }
            Operation::Bake { prefix } => {
                let exact = {
                    let overlapping: Vec<&Clip> = track
                        .clips
                        .iter()
                        .filter(|c| c.start < task.end && c.start + c.length > task.start)
                        .collect();
                    match overlapping.as_slice() {
                        [only]
                            if only.start == task.start && only.start + only.length == task.end =>
                        {
                            Some((*only).clone())
                        }
                        _ => None,
                    }
                };
                if let Some(prior) = exact {
                    clip.previous_states = prior.previous_states.clone();
                    remember(&mut clip, &prior, &task.label)?;
                } else {
                    remember_original(&mut clip, "Original audio")?;
                }
                clip.render_envelope = Some(unattenuated(processed.frames));
                // Freeze resolved envelopes before cutting: the bake already
                // contains them once and new cuts must not add another fade.
                let envelopes = frozen_envelopes(track);
                for existing in &mut track.clips {
                    existing.render_envelope = envelopes.get(&existing.id).cloned();
                }
                delete_range(track, task.start, task.end, &task.label)?;
                for existing in &mut track.clips {
                    existing.cut_fade_in_frames = None;
                    existing.cut_fade_out_frames = None;
                }
                let prefix = (*prefix).min(track.realtime_effects.len());
                track.realtime_effects.drain(..prefix);
                track.clips.push(clip);
            }
            Operation::Combine { clip_ids } => {
                clip.render_envelope = Some(unattenuated(processed.frames));
                combine_into(track, clip, clip_ids)?;
            }
        }
        let destination = self.root.join("Media").join(format!("{asset}.wav"));
        std::fs::hard_link(&processed.path, &destination)?;
        let info = WaveReader::open(&destination).map_err(engine)?.info;
        self.media.insert(
            asset.clone(),
            Media {
                path: destination.clone(),
                info,
            },
        );
        let history = match history {
            Some((source, path)) => {
                let target = self.root.join("Media").join(format!("{source}.wav"));
                std::fs::hard_link(&path, &target)?;
                let info = WaveReader::open(&target).map_err(engine)?.info;
                self.media.insert(
                    source.clone(),
                    Media {
                        path: target.clone(),
                        info,
                    },
                );
                Some((source, target))
            }
            None => None,
        };
        let revision = self.authority.revision;
        let versions = self.authority.track_revisions.clone();
        let result = self
            .authority
            .edit_session(
                &peer.runtime,
                super::role(&peer.role),
                Some(revision),
                &versions,
                candidate,
            )
            .map_err(engine)
            .and_then(|_| {
                if self.authority.revision != revision {
                    self.authority
                        .label_last_change(&peer.runtime, &task.label, None)
                        .map_err(engine)?;
                }
                Ok(())
            });
        if let Err(error) = result {
            self.media.remove(&asset);
            let _ = std::fs::remove_file(destination);
            if let Some((source, target)) = history {
                self.media.remove(&source);
                let _ = std::fs::remove_file(target);
            }
            return Err(error);
        }
        Ok(json!({
            "trackID": task.track_id,
            "clipID": clip_id,
            "clipIDs": [clip_id],
            "cursorFrame": task.start,
            "rangeStart": task.start,
            "rangeEnd": task.start + processed.frames,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn range_delete_keeps_fades_relative_to_the_whole_clip_and_appends_history() {
        let mut clip = plain_clip("C".into(), "A".into(), "Take".into(), 1000, 9000);
        clip.fade_in_frames = Some(2000);
        clip.edge_fade_frames = Some(240);
        let mut track = Track {
            id: "T".into(),
            clips: vec![clip],
            ..Track::default()
        };
        delete_range(&mut track, 4000, 6000, "Reverse").unwrap();
        let (left, right) = (&track.clips[0], &track.clips[1]);
        assert_eq!(
            (left.start, left.length, left.cut_fade_out_frames),
            (1000, 3000, Some(240))
        );
        assert_eq!(
            (left.fade_reference_offset, left.fade_reference_length),
            (Some(0), Some(9000))
        );
        assert_eq!(
            (right.start, right.offset, right.length),
            (6000, 5000, 4000)
        );
        assert_eq!(
            (right.fade_reference_offset, right.fade_reference_length),
            (Some(5000), Some(9000))
        );
        assert_eq!(right.previous_states.len(), 1);
        assert_eq!(right.previous_states[0].length, 9000);
        // Newest state is last and at most five are kept.
        let mut piece = right.clone();
        for n in 0..6 {
            let prior = piece.clone();
            piece.length -= 10;
            remember(&mut piece, &prior, &format!("Edit {n}")).unwrap();
        }
        assert_eq!(piece.previous_states.len(), 5);
        assert_eq!(piece.previous_states.last().unwrap().length, 4000 - 50);
    }
}
