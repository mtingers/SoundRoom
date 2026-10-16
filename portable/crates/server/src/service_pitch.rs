//! Pitch editor analysis: `pitch.analyze` (input contour + draft targets),
//! `pitch.render` (measured output) and `pitch.targets` (seed a preset curve and
//! author it on the track's edits-only pitch insert).
use super::{engine, new_id, protocol, Error, Handled, Identity, JobKind, Service};
use serde_json::{json, Value};
use soundroom_client::models::RemoteEditorIntent;
use soundroom_engine::{
    model::Effect,
    pitch::{analyze_wave, ContourPoint, Region},
    pitch_targets::{allowed_pitch_classes, seed, slice, Preset},
    project::Project,
    render::{default_parameters, export},
    wave::WaveReader,
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

const MAX_FRAMES: u64 = 30 * 60 * 48000;

pub(super) enum Mode {
    Analyze,
    Measure { original: bool },
    Targets(Preset),
}
pub(super) struct Task {
    project: Project,
    track_id: String,
    effect_id: Option<String>,
    start: u64,
    end: u64,
    mode: Mode,
    /// The effect's existing targets, sliced to the scope.
    existing: Vec<Region>,
    parameters: BTreeMap<String, f32>,
    revision: u64,
    path: PathBuf,
    /// Audition owner: keep the rendered file and play it.
    pub(super) preview: Option<String>,
}
pub(super) struct Analysis {
    task: Task,
    /// The rendered audition (preview requests only).
    pub(super) audio: Option<(String, PathBuf)>,
    contour: Vec<ContourPoint>,
}
fn effect_regions(effect: &Effect) -> Vec<Region> {
    effect
        .extra
        .get("pitchEdits")
        .and_then(|v| serde_json::from_value::<Vec<Region>>(v.clone()).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.start_frame < r.end_frame && !r.points.is_empty())
        .collect()
}
fn identified(mut regions: Vec<Region>) -> Result<Vec<Region>, Error> {
    for region in &mut regions {
        if region.id.is_empty() {
            region.id = new_id()?;
        }
        for point in &mut region.points {
            if point.id.is_empty() {
                point.id = new_id()?;
            }
        }
    }
    Ok(regions)
}
impl Task {
    pub(super) fn run(self, cancel: &AtomicBool) -> Result<Analysis, Error> {
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut cleanup = Cleanup(self.path.clone());
        export(
            &self.project,
            &self.path,
            self.start,
            self.end,
            cancel,
            |_, _| {},
        )
        .map_err(engine)?;
        let mut reader = WaveReader::open(&self.path).map_err(engine)?;
        let contour = analyze_wave(&mut reader, 65_536, cancel)
            .map_err(engine)?
            .into_iter()
            .map(|p| ContourPoint {
                frame: p.frame + self.start,
                ..p
            })
            .collect();
        if cancel.load(Ordering::Acquire) {
            return Err(protocol("Job cancelled"));
        }
        let audio = self.preview.clone().map(|owner| {
            cleanup.0 = PathBuf::new();
            (owner, self.path.clone())
        });
        Ok(Analysis {
            task: self,
            audio,
            contour,
        })
    }
}
impl Service {
    pub(super) fn begin_pitch(
        &mut self,
        peer: &Identity,
        intent: &RemoteEditorIntent,
    ) -> Result<Handled, Error> {
        if self.recording.is_some() {
            return Err(Error::Conflict("Finish recording first".into()));
        }
        let track = intent
            .strings
            .get("trackID")
            .or(intent.context.track_id.as_ref())
            .ok_or_else(|| protocol("Choose a track to analyze"))?;
        let (track_id, revision) = self
            .authority
            .track_revisions
            .iter()
            .find(|(id, _)| id.eq_ignore_ascii_case(track))
            .map(|(id, v)| (id.clone(), *v))
            .ok_or_else(|| Error::Conflict("Track is unavailable".into()))?;
        let source = self
            .authority
            .session
            .tracks
            .iter()
            .find(|t| t.id == track_id)
            .ok_or_else(|| protocol("Track is unavailable"))?
            .clone();
        let requested = intent
            .strings
            .get("effectID")
            .or(intent.context.effect_id.as_ref());
        let index = requested.and_then(|id| {
            source
                .realtime_effects
                .iter()
                .position(|e| e.id.eq_ignore_ascii_case(id) && e.kind == "pitchCorrection")
        });
        let (start, end) = match (intent.integers.get("start"), intent.integers.get("end")) {
            (Some(a), Some(b)) if *a >= 0 && b > a => (*a as u64, *b as u64),
            (None, None) => match (intent.context.range_start, intent.context.range_end) {
                (Some(a), Some(b)) if a >= 0 && b > a => (a as u64, b as u64),
                _ => (
                    0,
                    source
                        .clips
                        .iter()
                        .map(|c| c.start + c.length)
                        .max()
                        .unwrap_or(0),
                ),
            },
            _ => return Err(protocol("Invalid pitch range")),
        };
        if start >= end || end - start > MAX_FRAMES {
            return Err(protocol("Analyze at most 30 minutes of audio"));
        }
        let preview =
            intent.operation == "pitch.render" && intent.flags.get("preview") == Some(&true);
        if preview {
            self.claim_file_preview(peer, super::preview::Kind::Pitch)?;
        }
        let mode = match intent.operation.as_str() {
            "pitch.analyze" => Mode::Analyze,
            "pitch.render" => Mode::Measure {
                original: intent.flags.get("original") == Some(&true),
            },
            _ => Mode::Targets(
                intent
                    .strings
                    .get("preset")
                    .and_then(|p| Preset::named(p))
                    .ok_or_else(|| protocol("Unknown pitch preset"))?,
            ),
        };
        // Input bus: inserts before the pitch stage; measured output includes it.
        let prefix = match (&mode, index) {
            (Mode::Measure { original: false }, Some(i)) => i + 1,
            (_, Some(i)) => i,
            (_, None) => 0,
        };
        let keyed = source
            .routing
            .as_ref()
            .is_some_and(|r| r.sidechain_source_id.is_some())
            && source.realtime_effects[..prefix]
                .iter()
                .any(|e| e.kind == "compressor");
        let mut project = if keyed {
            Project {
                session: self
                    .authority
                    .session
                    .isolated_track(&track_id)
                    .map_err(engine)?,
                media: self.media.clone(),
            }
        } else {
            self.dry_project(&track_id, None)
        };
        project.session.tracks[0].gain = 1.;
        project.session.tracks[0].pan = 0.;
        project.session.tracks[0].mix_automation.clear();
        project.session.tracks[0].realtime_effects = source.realtime_effects[..prefix].to_vec();
        let effect = index.map(|i| &source.realtime_effects[i]);
        let mut parameters = effect.map(|e| e.parameters.clone()).unwrap_or_default();
        if parameters.is_empty() {
            parameters = default_parameters("pitchCorrection");
        }
        let existing = effect
            .map(|e| {
                effect_regions(e)
                    .iter()
                    .filter_map(|r| slice(r, start, end))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Handled::Job(JobKind::Pitch {
            task: Box::new(Task {
                project,
                track_id,
                effect_id: effect.map(|e| e.id.clone()),
                start,
                end,
                mode,
                existing,
                parameters,
                revision,
                path: self
                    .root
                    .join("Artifacts")
                    .join(format!("{}.pitch.wav", new_id()?)),
                preview: preview.then(|| peer.runtime.clone()),
            }),
        }))
    }
    /// Shapes the reply; `pitch.targets` also authors the seeded curve.
    pub(super) fn finish_pitch(
        &mut self,
        peer: &Identity,
        analysis: Analysis,
    ) -> Result<Value, Error> {
        self.require_editable()?;
        let Analysis { task, contour, .. } = analysis;
        let points: Vec<Value> = contour
            .iter()
            .map(
                |p| json!({"frame": p.frame, "frequency": p.frequency, "confidence": p.confidence}),
            )
            .collect();
        let voiced = contour.iter().any(ContourPoint::is_voiced);
        let reference = task
            .parameters
            .get("reference")
            .copied()
            .filter(|r| (420. ..=460.).contains(r))
            .unwrap_or(440.);
        let mask = allowed_pitch_classes(&task.parameters);
        let (regions, draft, output, error) = match task.mode {
            Mode::Measure { original } => (task.existing.clone(), false, !original, None),
            Mode::Analyze => {
                let error = (!voiced)
                    .then(|| "No stable single pitch detected in this selection.".to_string());
                if task.existing.is_empty() {
                    let seeded = identified(seed(
                        &contour,
                        (task.start, task.end),
                        reference,
                        Preset::Performance,
                        5.,
                        4095,
                    ))?;
                    let draft = !seeded.is_empty();
                    (seeded, draft, false, error)
                } else {
                    (task.existing.clone(), false, false, error)
                }
            }
            Mode::Targets(preset) => {
                if !voiced {
                    return Err(protocol("No stable pitch was detected in this range."));
                }
                let clean = identified(seed(
                    &contour,
                    (task.start, task.end),
                    reference,
                    preset,
                    5.,
                    mask,
                ))?;
                self.author_targets(peer, &task, &clean, preset)?;
                (clean, false, false, None)
            }
        };
        Ok(json!({
            "points": points,
            "regions": regions,
            "isOutput": output,
            "targetsAreDraft": draft,
            "error": error,
        }))
    }
    fn author_targets(
        &mut self,
        peer: &Identity,
        task: &Task,
        clean: &[Region],
        preset: Preset,
    ) -> Result<(), Error> {
        if self.authority.track_revisions.get(&task.track_id) != Some(&task.revision) {
            return Err(Error::Conflict(
                "The track changed while preparing targets; try again".into(),
            ));
        }
        let mut candidate = self.authority.session.clone();
        let track = Self::track(&mut candidate, &task.track_id)?;
        let existing = task
            .effect_id
            .as_ref()
            .and_then(|id| track.realtime_effects.iter().position(|e| &e.id == id));
        let index = match existing {
            Some(i) => i,
            None => {
                let mut extra = BTreeMap::new();
                extra.insert("pitchEditsOnly".to_string(), json!(true));
                track.realtime_effects.insert(
                    0,
                    Effect {
                        id: new_id()?,
                        kind: "pitchCorrection".into(),
                        amount: 0.5,
                        enabled: true,
                        automation: vec![],
                        parameters: task.parameters.clone(),
                        parameter_automation: BTreeMap::new(),
                        extra: extra.into_iter().collect(),
                    },
                );
                0
            }
        };
        let effect = &mut track.realtime_effects[index];
        let mut retained = vec![];
        for region in effect_regions(effect) {
            if region.end_frame <= task.start || region.start_frame >= task.end {
                retained.push(region);
                continue;
            }
            if region.start_frame < task.start {
                if let Some(part) = slice(&region, region.start_frame, task.start) {
                    retained.push(part);
                }
            }
            if region.end_frame > task.end {
                if let Some(mut part) = slice(&region, task.end, region.end_frame) {
                    if region.start_frame < task.start {
                        part.id = new_id()?;
                    }
                    retained.push(part);
                }
            }
        }
        retained.extend(clean.iter().cloned());
        retained.sort_by_key(|r| r.start_frame);
        let retained = identified(retained)?;
        effect
            .extra
            .insert("pitchEdits".into(), serde_json::to_value(&retained)?);
        let label = format!(
            "Pitch curve preset · {}",
            match preset {
                Preset::Natural => "Natural",
                Preset::Tight => "Tight",
                Preset::Robotic => "Robotic",
                Preset::Performance => "Keep performance",
            }
        );
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
                .label_last_change(&peer.runtime, &label, None)
                .map_err(engine)?;
        }
        Ok(())
    }
}
