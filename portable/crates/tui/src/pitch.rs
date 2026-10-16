//! Offline analysis and manual target editing with a stable, private chart viewport.
use crate::{
    automation::{controls, sample},
    input::Key,
    render::Canvas,
    worker::EditorResponse,
};
use base64::Engine;
use soundroom_client::{auth::new_id, models::*};
use std::collections::BTreeMap;
#[derive(Clone)]
struct Draft {
    regions: Vec<PitchEditRegion>,
    parameters: Option<BTreeMap<String, f32>>,
    reference: f32,
    manual_targets_changed: bool,
}
#[derive(Default)]
struct Targets {
    curves: Vec<Vec<AutomationPoint>>,
    refs: Vec<(usize, usize)>,
    points: Vec<(i64, usize)>,
    regions: Vec<usize>,
    prefix_end: Vec<i64>,
}
impl Targets {
    fn build(regions: &[PitchEditRegion]) -> Self {
        let mut result = Self::default();
        for (r, region) in regions.iter().enumerate() {
            let points = region.points.as_deref().unwrap_or_default();
            result.curves.push(
                points
                    .iter()
                    .map(|p| AutomationPoint {
                        frame: p.frame,
                        value: p.note,
                        curve: p.curve.clone(),
                        curve_start: p.curve_start,
                        curve_end: p.curve_end,
                    })
                    .collect(),
            );
            for (p, point) in points.iter().enumerate() {
                result.points.push((point.frame, result.refs.len()));
                result.refs.push((r, p));
            }
            result.regions.push(r);
        }
        result.points.sort_by_key(|p| p.0);
        result.regions.sort_by_key(|i| regions[*i].start_frame);
        let mut end = 0;
        for index in &result.regions {
            end = end.max(regions[*index].end_frame);
            result.prefix_end.push(end);
        }
        result
    }
    fn region(&self, regions: &[PitchEditRegion], frame: i64) -> Option<usize> {
        let first = self.prefix_end.partition_point(|end| *end < frame);
        self.regions[first..]
            .iter()
            .copied()
            .take_while(|i| regions[*i].start_frame <= frame)
            .filter(|i| regions[*i].end_frame >= frame)
            .min()
    }
}
pub struct Pitch {
    pub track_id: String,
    pub workspace_id: String,
    pub effect_id: String,
    pub revision: i64,
    pub scope: (i64, i64),
    pub regions: Vec<PitchEditRegion>,
    pub source: Vec<RemotePitchPoint>,
    pub output: Vec<RemotePitchPoint>,
    pub selected: usize,
    pub playing: bool,
    pub dirty: bool,
    manual_targets_changed: bool,
    pub status: String,
    pub busy: bool,
    controls: RemoteTrackControls,
    effect: usize,
    outside: Vec<PitchEditRegion>,
    undo: Vec<Draft>,
    redo: Vec<Draft>,
    start: i64,
    span: i64,
    step: i64,
    center: f32,
    vertical: f32,
    columns: usize,
    rows: usize,
    reference: f32,
    pending_result: Option<EditorResponse>,
    targets: Targets,
    targets_dirty: bool,
}
impl Pitch {
    pub fn open(
        track: &Track,
        effect: usize,
        revision: i64,
        scope: (i64, i64),
        rate: f64,
        bpm: f64,
    ) -> Result<Self, String> {
        if scope.0 < 0 || scope.1 <= scope.0 {
            return Err("Select an audio range to analyze.".into());
        }
        let controls = controls(track)?;
        let e = controls
            .realtime_effects
            .get(effect)
            .ok_or("Add or select a Pitch correction insert first.")?;
        if e.kind != RealtimeEffectKind::PitchCorrection {
            return Err("Select a Pitch correction insert first.".into());
        }
        let effect_id = e.id.clone().ok_or("Missing pitch insert ID.")?;
        let reference = e
            .parameters
            .as_ref()
            .and_then(|p| p.get("reference"))
            .copied()
            .unwrap_or(440.)
            .clamp(400., 480.);
        let existing = e.pitch_edits.as_deref().unwrap_or_default();
        let regions = existing
            .iter()
            .filter_map(|r| slice(r, scope.0, scope.1))
            .collect();
        let outside = existing
            .iter()
            .flat_map(|r| {
                [
                    slice(r, r.start_frame, scope.0),
                    slice(r, scope.1, r.end_frame),
                ]
            })
            .flatten()
            .collect();
        Ok(Self {
            track_id: controls.track_id.clone(),
            workspace_id: new_id().map_err(|e| e.to_string())?,
            effect_id,
            revision,
            scope,
            regions,
            source: vec![],
            output: vec![],
            selected: 0,
            playing: false,
            dirty: false,
            manual_targets_changed: false,
            status: "Analyze selected audio on the server, then edit targets or choose a preset."
                .into(),
            busy: false,
            controls,
            effect,
            outside,
            undo: vec![],
            redo: vec![],
            start: scope.0,
            span: scope.1 - scope.0,
            step: (rate * 60. / bpm / 4.).round().max(1.) as i64,
            center: 60.,
            vertical: 24.,
            columns: 65,
            rows: 12,
            reference,
            pending_result: None,
            targets: Targets::default(),
            targets_dirty: true,
        })
    }
    pub fn request(
        &mut self,
        operation: &str,
        preview: bool,
        original: bool,
        preset: Option<&str>,
    ) -> Result<RemoteCommand, String> {
        if self.busy {
            return Err("Wait for the current pitch result, or :cancel.".into());
        }
        if preview && self.dirty && !original {
            return Err("Use targets before auditioning their output.".into());
        }
        let mut strings = BTreeMap::from([
            ("trackID".into(), self.track_id.clone()),
            ("effectID".into(), self.effect_id.clone()),
        ]);
        strings.insert("workspaceID".into(), self.workspace_id.clone());
        if let Some(preset) = preset {
            if !["Natural", "Tight", "Robotic", "Keep performance"].contains(&preset) {
                return Err("Target preset: Natural, Tight, Robotic, Keep performance.".into());
            }
            strings.insert("preset".into(), preset.into());
        }
        self.busy = true;
        Ok(RemoteCommand::Editor {
            intent: RemoteEditorIntent {
                operation: operation.into(),
                context: RemoteEditorContext {
                    track_id: Some(self.track_id.clone()),
                    clip_id: None,
                    clip_ids: vec![],
                    cursor_frame: self.scope.0,
                    range_start: Some(self.scope.0),
                    range_end: Some(self.scope.1),
                    effect_id: Some(self.effect_id.clone()),
                    automation_target: None,
                },
                integers: BTreeMap::from([
                    ("start".into(), self.scope.0),
                    ("end".into(), self.scope.1),
                ]),
                numbers: BTreeMap::new(),
                strings,
                flags: BTreeMap::from([("preview".into(), preview), ("original".into(), original)]),
                payload: None,
            },
        })
    }
    pub fn response(
        &mut self,
        response: &EditorResponse,
        current_track: Option<&Track>,
        current_revision: Option<i64>,
    ) -> Result<bool, String> {
        let tag = &response.tag;
        if tag.workspace_id.as_deref() != Some(&self.workspace_id)
            || tag.track_id.as_deref() != Some(&self.track_id)
            || tag.effect_id.as_deref() != Some(&self.effect_id)
            || tag.range != Some(self.scope)
            || tag.track_revision != Some(self.revision)
        {
            return Ok(false);
        }
        self.busy = false;
        if response.reply.status != RemoteReplyStatus::Accepted {
            self.status = response.reply.message.clone();
            return Ok(true);
        }
        let authored = tag.operation == "pitch.targets";
        let expected = if authored {
            Some(response.reply.revision)
        } else {
            Some(self.revision)
        };
        if authored && current_revision.is_some_and(|r| r < response.reply.revision) {
            self.pending_result = Some(response.clone());
            self.status = "Waiting for authoritative target metadata.".into();
            return Ok(true);
        }
        if current_revision != expected {
            self.status =
                "Source changed while analysis ran. Reopen this range before using results.".into();
            return Ok(true);
        }
        let payload = response
            .reply
            .payload
            .as_ref()
            .ok_or("Pitch response has no analysis.")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .map_err(|_| "Invalid pitch analysis encoding.")?;
        let mut analysis: RemotePitchAnalysis =
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if analysis.regions.iter().any(|r| {
            r.start_frame < 0
                || r.end_frame <= r.start_frame
                || r.points.as_ref().is_some_and(|ps| {
                    ps.iter().any(|p| {
                        p.frame < r.start_frame
                            || p.frame > r.end_frame
                            || !p.note.is_finite()
                            || !(0. ..=95.).contains(&p.note)
                    })
                })
        }) {
            return Err("Invalid pitch target regions.".into());
        }
        if analysis.points.iter().any(|p| {
            p.frame < self.scope.0
                || p.frame > self.scope.1
                || !p.frequency.is_finite()
                || p.frequency < 0.
                || !p.confidence.is_finite()
                || !(0. ..=1.).contains(&p.confidence)
        }) {
            return Err("Invalid pitch trace values.".into());
        }
        analysis.points.sort_by_key(|p| p.frame);
        if let Some(error) = analysis.error {
            self.status = error;
            return Ok(true);
        }
        if analysis.is_output {
            self.output = analysis.points;
        } else {
            self.source = analysis.points;
            // Center only on explicit new analysis; playback never moves the vertical axis.
            let mut notes: Vec<_> = self
                .source
                .iter()
                .filter(|p| p.frequency > 0. && p.confidence >= 0.6)
                .map(|p| self.note(p.frequency))
                .collect();
            notes.sort_by(f32::total_cmp);
            if let Some(note) = notes.get(notes.len() / 2) {
                self.center = note.round().clamp(0., 95.);
            }
        }
        if !self.dirty || authored {
            self.targets_dirty = true;
            self.regions = analysis
                .regions
                .iter()
                .filter_map(|r| slice(r, self.scope.0, self.scope.1))
                .collect();
            self.selected = 0;
            self.dirty = analysis.targets_are_draft;
            self.manual_targets_changed = analysis.targets_are_draft;
        }
        if authored {
            let track = current_track.ok_or("Pitch source was removed.")?;
            self.controls = controls(track)?;
            self.revision = response.reply.revision;
            self.outside = track
                .realtime_effects
                .as_ref()
                .and_then(|es| es.get(self.effect))
                .and_then(|e| e.pitch_edits.as_ref())
                .map(|rs| {
                    rs.iter()
                        .flat_map(|r| {
                            [
                                slice(r, r.start_frame, self.scope.0),
                                slice(r, self.scope.1, r.end_frame),
                            ]
                        })
                        .flatten()
                        .collect()
                })
                .unwrap_or_default();
        }
        self.status = format!(
            "{} measured points; {} target points. n add; arrows edit; Enter uses targets.",
            self.source.len(),
            self.point_count()
        );
        Ok(true)
    }
    fn note(&self, f: f32) -> f32 {
        69. + 12. * (f / self.reference).log2()
    }
    pub fn failure(&mut self, tag: &crate::worker::ResponseTag, message: &str) -> bool {
        if tag.workspace_id.as_deref() == Some(&self.workspace_id) {
            self.busy = false;
            self.pending_result = None;
            self.status = message.into();
            return true;
        }
        false
    }
    pub fn install_pending(
        &mut self,
        track: Option<&Track>,
        revision: Option<i64>,
    ) -> Result<(), String> {
        if let Some(result) = self.pending_result.take() {
            self.response(&result, track, revision)?;
        }
        Ok(())
    }
    fn point_count(&self) -> usize {
        self.regions
            .iter()
            .map(|r| r.points.as_ref().map_or(0, Vec::len))
            .sum()
    }
    fn points(&self) -> Vec<(usize, usize)> {
        self.regions
            .iter()
            .enumerate()
            .flat_map(|(r, region)| {
                (0..region.points.as_ref().map_or(0, Vec::len)).map(move |p| (r, p))
            })
            .collect()
    }
    fn draft(&self) -> Draft {
        Draft {
            regions: self.regions.clone(),
            parameters: self.controls.realtime_effects[self.effect]
                .parameters
                .clone(),
            reference: self.reference,
            manual_targets_changed: self.manual_targets_changed,
        }
    }
    fn trim_history(&mut self) {
        let bytes = |d: &Draft| {
            d.regions
                .iter()
                .map(|r| r.points.as_ref().map_or(0, Vec::len) * 128 + 128)
                .sum::<usize>()
                + d.parameters.as_ref().map_or(0, |p| p.len() * 128)
        };
        while self.undo.iter().chain(&self.redo).map(bytes).sum::<usize>() > 32 * 1024 * 1024 {
            if !self.undo.is_empty() {
                self.undo.remove(0);
            } else {
                self.redo.remove(0);
            }
        }
    }
    fn capture(&mut self) {
        self.targets_dirty = true;
        if self.undo.len() == 100 {
            self.undo.remove(0);
        }
        self.undo.push(self.draft());
        self.redo.clear();
        self.trim_history();
    }
    fn restore(&mut self, redo: bool) {
        let old = if redo {
            self.redo.pop()
        } else {
            self.undo.pop()
        };
        if let Some(old) = old {
            let current = self.draft();
            self.targets_dirty = true;
            self.regions = old.regions;
            self.controls.realtime_effects[self.effect].parameters = old.parameters;
            self.reference = old.reference;
            self.manual_targets_changed = old.manual_targets_changed;
            if redo {
                self.undo.push(current)
            } else {
                self.redo.push(current)
            }
            self.dirty = true;
            self.selected = self.selected.min(self.point_count().saturating_sub(1));
            self.trim_history();
        }
    }
    pub fn add(&mut self, frame: i64, note: f32) -> Result<(), String> {
        if frame < self.scope.0
            || frame >= self.scope.1
            || !note.is_finite()
            || !(0. ..=95.).contains(&note)
        {
            return Err("Target must be within the selected range and notes 0..95.".into());
        }
        if self.point_count() >= 4096 {
            return Err("Terminal pitch editing supports 4096 target points per range.".into());
        }
        self.capture();
        let index = self
            .regions
            .iter()
            .position(|r| frame >= r.start_frame && frame < r.end_frame)
            .unwrap_or_else(|| {
                let start = self
                    .regions
                    .iter()
                    .filter(|r| r.end_frame <= frame)
                    .map(|r| r.end_frame)
                    .max()
                    .unwrap_or(self.scope.0);
                let end = self
                    .regions
                    .iter()
                    .filter(|r| r.start_frame > frame)
                    .map(|r| r.start_frame)
                    .min()
                    .unwrap_or(self.scope.1);
                self.regions.push(PitchEditRegion {
                    id: new_id().ok(),
                    start_frame: start,
                    end_frame: end,
                    points: Some(vec![]),
                });
                self.regions.len() - 1
            });
        let points = self.regions[index].points.get_or_insert_with(Vec::new);
        points.retain(|p| p.frame != frame);
        points.push(PitchEditPoint {
            id: new_id().ok(),
            frame,
            note,
            curve: Some(AutomationCurve::Smooth),
            curve_start: Some(0.),
            curve_end: Some(1.),
        });
        points.sort_by_key(|p| p.frame);
        self.selected = self
            .points()
            .iter()
            .position(|(r, p)| {
                *r == index && self.regions[*r].points.as_ref().unwrap()[*p].frame == frame
            })
            .unwrap();
        self.dirty = true;
        self.manual_targets_changed = true;
        Ok(())
    }
    pub fn key(&mut self, key: Key) -> Result<bool, String> {
        match key {
            Key::Enter => return Ok(true),
            Key::Char('u') => self.restore(false),
            Key::Ctrl('r') => self.restore(true),
            Key::Char('J') => {
                self.selected = (self.selected + 1).min(self.point_count().saturating_sub(1))
            }
            Key::Char('K') => self.selected = self.selected.saturating_sub(1),
            Key::Char('n') => {
                let frame = self.start + self.span / 2;
                let note = self
                    .source
                    .iter()
                    .min_by_key(|p| p.frame.abs_diff(frame))
                    .filter(|p| p.frequency > 0.)
                    .map_or(self.center, |p| self.note(p.frequency));
                self.add(frame, note.clamp(0., 95.))?;
            }
            Key::Char('+') | Key::Char('=') => {
                self.span = (self.span as f64 / 1.25).round().max(1.) as i64
            }
            Key::Char('-') => {
                self.span =
                    ((self.span as f64 * 1.25).round() as i64).min(self.scope.1 - self.scope.0)
            }
            Key::Char('H') => self.start = (self.start - self.span / 4).max(self.scope.0),
            Key::Char('L') => {
                self.start =
                    (self.start + self.span / 4).min((self.scope.1 - self.span).max(self.scope.0))
            }
            Key::Char('z') => self.vertical = (self.vertical / 1.25).max(2.),
            Key::Char('Z') => self.vertical = (self.vertical * 1.25).min(95.),
            Key::PageUp => self.center = (self.center + 12.).min(95.),
            Key::PageDown => self.center = (self.center - 12.).max(0.),
            Key::Delete | Key::Char('d') => {
                if let Some((r, p)) = self.points().get(self.selected).copied() {
                    self.capture();
                    self.regions[r].points.as_mut().unwrap().remove(p);
                    self.dirty = true;
                    self.manual_targets_changed = true;
                    self.selected = self.selected.min(self.point_count().saturating_sub(1));
                }
            }
            Key::Char('h')
            | Key::Left
            | Key::Char('l')
            | Key::Right
            | Key::Char('j')
            | Key::Down
            | Key::Char('k')
            | Key::Up => {
                if let Some((r, p)) = self.points().get(self.selected).copied() {
                    self.capture();
                    let region = &mut self.regions[r];
                    let points = region.points.as_mut().unwrap();
                    let mut point = points.remove(p);
                    match key {
                        Key::Char('h') | Key::Left => {
                            point.frame = (point.frame - self.step).max(region.start_frame)
                        }
                        Key::Char('l') | Key::Right => {
                            point.frame = (point.frame + self.step).min(region.end_frame)
                        }
                        Key::Char('j') | Key::Down => point.note = (point.note - 0.1).max(0.),
                        _ => point.note = (point.note + 0.1).min(95.),
                    };
                    points.retain(|p| p.frame != point.frame);
                    let moved_frame = point.frame;
                    points.push(point);
                    points.sort_by_key(|p| p.frame);
                    self.selected = self
                        .points()
                        .iter()
                        .position(|(a, b)| {
                            *a == r
                                && self.regions[*a].points.as_ref().unwrap()[*b].frame
                                    == moved_frame
                        })
                        .unwrap_or(0);
                    self.dirty = true;
                    self.manual_targets_changed = true;
                }
            }
            Key::Mouse {
                button: 0,
                column,
                row,
                released: false,
            } if column >= 9 && row >= 6 && usize::from(row) < 6 + self.rows => {
                let frame = self.start
                    + ((f64::from(column - 9) / self.columns as f64) * self.span as f64).round()
                        as i64;
                let note = self.center + self.vertical / 2.
                    - f32::from(row - 6) / self.rows.saturating_sub(1).max(1) as f32
                        * self.vertical;
                self.add(frame, note.clamp(0., 95.))?;
            }
            _ => {}
        }
        Ok(false)
    }
    pub fn command(
        &mut self,
        words: &[String],
        meta: Option<&RemoteEffectMetadata>,
    ) -> Result<bool, String> {
        let arg = |i: usize| words.get(i).ok_or("Missing argument.");
        match words[0].as_str() {
            "parameter" => {
                let parameter = meta.and_then(|m| m.parameters.iter().find(|p| Some(&p.id) == words.get(1))).ok_or("Usage: parameter advertisedParameterID value")?;
                let scale = if parameter.unit == "%" { 100. } else { 1. };
                let value = arg(2)?.parse::<f32>().map_err(|_| "Parameter value must be numeric.")? / scale;
                if !value.is_finite() || value < parameter.minimum || value > parameter.maximum || parameter.integer && value.fract() != 0. {
                    return Err(format!("{} range: {}..{} {}", parameter.name, parameter.minimum * scale, parameter.maximum * scale, parameter.unit));
                }
                self.capture();
                self.controls.realtime_effects[self.effect].parameters.get_or_insert_with(BTreeMap::new).insert(parameter.id.clone(), value);
                if parameter.id == "reference" { self.reference = value; }
                self.dirty = true;
            },
            "processing-preset" => {
                let index = arg(1)?.parse::<usize>().map_err(|_| "Preset index must be an integer.")?;
                let preset = meta.and_then(|m| m.factory_presets.get(index)).ok_or("Unknown processing preset.")?;
                self.capture();
                self.controls.realtime_effects[self.effect].parameters.get_or_insert_with(BTreeMap::new).extend(preset.values.clone());
                if let Some(value) = preset.values.get("reference") { self.reference = *value; }
                self.dirty = true;
            },
            "point"=>{let frame=arg(1)?.parse::<i64>().map_err(|_|"Point position is an exact audio frame.")?;let note=parse_note(arg(2)?)?;self.add(frame,note)?;},
            "smooth"|"curve"=>{let curve=if words[0]=="smooth"{AutomationCurve::Smooth}else{serde_json::from_value(serde_json::json!(arg(1)?)).map_err(|_|"Curve: linear, smooth, easeIn, easeOut, hold.")?};self.capture();let selected=self.points().get(self.selected).copied();for (r,region) in self.regions.iter_mut().enumerate(){for (p,point) in region.points.as_mut().into_iter().flatten().enumerate(){if words[0]=="smooth"||selected==Some((r,p)){point.curve=Some(curve.clone());point.curve_start=Some(0.);point.curve_end=Some(1.);}}}self.dirty=true;self.manual_targets_changed=true;},
            "undo"=>self.restore(false),"redo"=>self.restore(true),"use"|"commit"=>return Ok(true),
            "clear"=>{self.capture();self.regions.clear();self.selected=0;self.dirty=true;self.manual_targets_changed=true;},
            _=>return Err("Pitch: :analyze; :measure; :preset Natural|Tight|Robotic|Keep performance; :point frame C#4; :curve smooth; :parameter id value; :processing-preset index; :use.".into())
        }
        Ok(false)
    }
    pub fn stop_request(&self) -> RemoteCommand {
        RemoteCommand::Editor {
            intent: RemoteEditorIntent {
                operation: "audition.stop".into(),
                context: RemoteEditorContext {
                    track_id: None,
                    clip_id: None,
                    clip_ids: vec![],
                    cursor_frame: 0,
                    range_start: None,
                    range_end: None,
                    effect_id: None,
                    automation_target: None,
                },
                integers: BTreeMap::new(),
                numbers: BTreeMap::new(),
                strings: BTreeMap::from([("kind".into(), "pitch".into())]),
                flags: BTreeMap::new(),
                payload: None,
            },
        }
    }
    pub fn use_targets(&self) -> RemoteCommand {
        let mut controls = self.controls.clone();
        let effect = &mut controls.realtime_effects[self.effect];
        let mut regions = self.outside.clone();
        regions.extend(self.regions.clone());
        regions.sort_by_key(|r| r.start_frame);
        if self.manual_targets_changed {
            effect.pitch_edits = Some(regions);
            effect.pitch_edits_only = Some(true);
        }
        RemoteCommand::UpdateTrackControls {
            controls,
            name: "Pitch targets".into(),
            gesture_id: None,
        }
    }
    pub fn draw(&mut self, c: &mut Canvas) {
        for y in 2..c.height.saturating_sub(3) {
            c.fill(0, y, c.width, ' ', 252, 234);
        }
        c.text(
            1,
            2,
            &format!(
                "Pitch / {}   {}   fixed vertical view",
                self.controls.name,
                if self.busy {
                    "Analyzing"
                } else if self.dirty {
                    "Draft targets"
                } else {
                    "Live targets"
                }
            ),
            81,
            234,
            true,
        );
        c.text(1,3,":analyze; :measure; :preset Natural|Tight|Robotic; :point frame note; :smooth; Enter uses targets",245,234,false);
        c.text(
            1,
            4,
            "J/K point; h/l time; j/k pitch; +/- time zoom; z/Z pitch zoom; PageUp/Down pitch view",
            245,
            234,
            false,
        );
        self.columns = c.width.saturating_sub(10).max(1);
        self.rows = c.height.saturating_sub(12).clamp(1, 25);
        let y_note = |note: f32| {
            ((self.center + self.vertical / 2. - note) / self.vertical
                * self.rows.saturating_sub(1) as f32)
                .round() as isize
        };
        for row in 0..self.rows {
            let note = self.center + self.vertical / 2.
                - row as f32 / self.rows.saturating_sub(1).max(1) as f32 * self.vertical;
            c.text(
                1,
                6 + row,
                &crate::creative::note_name(note.round() as i64),
                245,
                234,
                false,
            );
            c.fill(9, 6 + row, self.columns, '.', 238, 234);
        }
        if self.targets_dirty {
            self.targets = Targets::build(&self.regions);
            self.targets_dirty = false;
        }
        for x in 0..self.columns {
            let a = self.start + (x as f64 / self.columns as f64 * self.span as f64).round() as i64;
            let b = self.start
                + ((x + 1) as f64 / self.columns as f64 * self.span as f64).round() as i64;
            for (trace, color) in [(&self.source, 110), (&self.output, 144)] {
                let first = trace.partition_point(|p| p.frame < a);
                let note = trace[first..]
                    .iter()
                    .take_while(|p| p.frame < b)
                    .filter(|p| p.frequency > 0. && p.confidence >= 0.6)
                    .max_by(|a, b| a.confidence.total_cmp(&b.confidence))
                    .map(|p| self.note(p.frequency));
                if let Some(note) = note {
                    let y = y_note(note);
                    if y >= 0 && (y as usize) < self.rows {
                        c.put(9 + x, 6 + y as usize, '*', color, 234, false);
                    }
                }
            }
            if let Some(i) = self.targets.region(&self.regions, a) {
                let ps = &self.targets.curves[i];
                if let Some(first) = ps.first() {
                    let y = y_note(sample(ps, a, first.value, false));
                    if y >= 0 && (y as usize) < self.rows {
                        c.put(9 + x, 6 + y as usize, '-', 81, 234, false);
                    }
                }
            }
        }
        let first = self.targets.points.partition_point(|p| p.0 < self.start);
        let end = self
            .targets
            .points
            .partition_point(|p| p.0 <= self.start + self.span);
        for (_, i) in &self.targets.points[first..end] {
            let (r, p) = self.targets.refs[*i];
            let point = &self.regions[r].points.as_ref().unwrap()[p];
            {
                let x = ((point.frame - self.start) as f64 / self.span as f64 * self.columns as f64)
                    .round() as usize;
                let y = y_note(point.note);
                if y >= 0 && (y as usize) < self.rows {
                    c.put(
                        9 + x,
                        6 + y as usize,
                        if *i == self.selected { '@' } else { 'o' },
                        203,
                        234,
                        true,
                    );
                }
            }
        }
        if let Some((r, p)) = self.targets.refs.get(self.selected) {
            let point = &self.regions[*r].points.as_ref().unwrap()[*p];
            c.text(
                1,
                c.height.saturating_sub(6),
                &format!(
                    "Target {} frame {} note {:.3} / {}; source cyan, output green, targets blue",
                    self.selected + 1,
                    point.frame,
                    point.note,
                    crate::creative::note_name(point.note.round() as i64)
                ),
                252,
                234,
                false,
            );
        }
        c.text(1, c.height.saturating_sub(4), &self.status, 245, 234, false);
    }
}
pub fn parse_note(text: &str) -> Result<f32, String> {
    if let Ok(note) = text.parse::<f32>() {
        if note.is_finite() && (0. ..=95.).contains(&note) {
            return Ok(note);
        }
        return Err("Pitch target: MIDI note 0..95.".into());
    }
    let bytes = text.as_bytes();
    let first = bytes
        .first()
        .copied()
        .ok_or("Pitch target: note name such as C#4.")?;
    let base = match first.to_ascii_uppercase() {
        b'C' => 0,
        b'D' => 2,
        b'E' => 4,
        b'F' => 5,
        b'G' => 7,
        b'A' => 9,
        b'B' => 11,
        _ => return Err("Pitch target: note name such as C#4.".into()),
    };
    let (offset, accidental) = match bytes.get(1) {
        Some(b'#') => (2, 1),
        Some(b'b') => (2, -1),
        _ => (1, 0),
    };
    let octave = text
        .get(offset..)
        .ok_or("Invalid pitch target.")?
        .parse::<i64>()
        .map_err(|_| "Pitch target needs an octave, such as C4.")?;
    let note = octave
        .checked_add(1)
        .and_then(|o| o.checked_mul(12))
        .and_then(|n| n.checked_add(base + accidental))
        .ok_or("Pitch target octave is out of range.")?;
    if !(0..=95).contains(&note) {
        return Err("Pitch target: MIDI note 0..95.".into());
    }
    Ok(note as f32)
}
pub fn frequency_note(f: f32) -> f32 {
    69. + 12. * (f / 440.).log2()
}
/// Clip a target region without changing its nonlinear transition on either side.
pub fn slice(region: &PitchEditRegion, start: i64, end: i64) -> Option<PitchEditRegion> {
    let start = start.max(region.start_frame);
    let end = end.min(region.end_frame);
    if end <= start {
        return None;
    }
    if start == region.start_frame && end == region.end_frame {
        return Some(region.clone());
    }
    let mut points = region.points.clone().unwrap_or_default();
    if points.is_empty() {
        return None;
    }
    points.sort_by_key(|p| p.frame);
    for frame in [start, end] {
        if points.iter().any(|p| p.frame == frame) {
            continue;
        }
        let next = points.partition_point(|p| p.frame < frame);
        let value = sample(
            &points
                .iter()
                .map(|p| AutomationPoint {
                    frame: p.frame,
                    value: p.note,
                    curve: p.curve.clone(),
                    curve_start: p.curve_start,
                    curve_end: p.curve_end,
                })
                .collect::<Vec<_>>(),
            frame,
            points[0].note,
            false,
        );
        let mut curve = AutomationCurve::Linear;
        let mut cstart = 0.;
        let mut cend = 1.;
        if next > 0 && next < points.len() {
            let bframe = points[next].frame;
            let a = &mut points[next - 1];
            let f = (frame - a.frame) as f32 / (bframe - a.frame) as f32;
            let old = a.curve_end.unwrap_or(1.);
            let middle = a.curve_start.unwrap_or(0.) + (old - a.curve_start.unwrap_or(0.)) * f;
            curve = a.curve.clone().unwrap_or(AutomationCurve::Linear);
            cstart = middle;
            cend = old;
            a.curve_end = Some(middle);
        }
        points.insert(
            next,
            PitchEditPoint {
                id: new_id().ok(),
                frame,
                note: value,
                curve: Some(curve),
                curve_start: Some(cstart),
                curve_end: Some(cend),
            },
        );
    }
    points.retain(|p| p.frame >= start && p.frame <= end);
    Some(PitchEditRegion {
        id: new_id().ok(),
        start_frame: start,
        end_frame: end,
        points: Some(points),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::ResponseTag;
    use std::sync::Arc;
    fn track() -> Track {
        serde_json::from_value(serde_json::json!({"id":"00000000-0000-0000-0000-000000000001","name":"Voice","clips":[],"realtimeEffects":[{"id":"00000000-0000-0000-0000-000000000002","kind":"pitchCorrection","enabled":true,"parameters":{"reference":442},"pitchEdits":[{"startFrame":0,"endFrame":1000,"points":[{"frame":0,"note":55,"curve":"smooth"},{"frame":1000,"note":65}]}]}]})).unwrap()
    }
    fn open() -> Pitch {
        Pitch::open(&track(), 0, 5, (200, 800), 48000., 120.).unwrap()
    }
    fn response(p: &Pitch, op: &str, revision: i64) -> EditorResponse {
        let analysis = serde_json::json!({"points":[{"frame":300,"frequency":442,"confidence":0.9}],"regions":p.regions,"isOutput":false,"targetsAreDraft":false});
        EditorResponse {
            tag: ResponseTag {
                operation: op.into(),
                workspace_id: Some(p.workspace_id.clone()),
                track_id: Some(p.track_id.clone()),
                effect_id: Some(p.effect_id.clone()),
                range: Some(p.scope),
                source_revision: Some(6),
                track_revision: Some(p.revision),
            },
            reply: Arc::new(RemoteReply {
                request_id: "test".into(),
                status: RemoteReplyStatus::Accepted,
                revision,
                message: String::new(),
                payload: Some(
                    base64::engine::general_purpose::STANDARD
                        .encode(serde_json::to_vec(&analysis).unwrap()),
                ),
            }),
        }
    }
    #[test]
    fn nonlinear_pitch_slicing_preserves_curve_at_each_frame() {
        for curve in [
            AutomationCurve::Linear,
            AutomationCurve::Smooth,
            AutomationCurve::EaseIn,
            AutomationCurve::EaseOut,
            AutomationCurve::Hold,
        ] {
            let original = PitchEditRegion {
                id: None,
                start_frame: 0,
                end_frame: 100,
                points: Some(vec![
                    PitchEditPoint {
                        id: None,
                        frame: 0,
                        note: 40.,
                        curve: Some(curve),
                        curve_start: Some(0.1),
                        curve_end: Some(0.9),
                    },
                    PitchEditPoint {
                        id: None,
                        frame: 100,
                        note: 80.,
                        curve: None,
                        curve_start: None,
                        curve_end: None,
                    },
                ]),
            };
            let clipped = slice(&original, 20, 70).unwrap();
            let values = |r: &PitchEditRegion| {
                r.points
                    .as_ref()
                    .unwrap()
                    .iter()
                    .map(|p| AutomationPoint {
                        frame: p.frame,
                        value: p.note,
                        curve: p.curve.clone(),
                        curve_start: p.curve_start,
                        curve_end: p.curve_end,
                    })
                    .collect::<Vec<_>>()
            };
            for frame in 20..=70 {
                assert!(
                    (sample(&values(&original), frame, 40., false)
                        - sample(&values(&clipped), frame, 40., false))
                    .abs()
                        < 0.0001
                );
            }
        }
    }
    #[test]
    fn manual_targets_preserve_regions_outside_reviewed_scope() {
        let mut p = open();
        p.command(&["clear".into()], None).unwrap();
        p.add(300, 60.).unwrap();
        let RemoteCommand::UpdateTrackControls { controls, .. } = p.use_targets() else {
            panic!()
        };
        let regions = controls.realtime_effects[0].pitch_edits.as_ref().unwrap();
        assert_eq!(
            regions
                .iter()
                .map(|r| (r.start_frame, r.end_frame))
                .collect::<Vec<_>>(),
            vec![(0, 200), (200, 800), (800, 1000)]
        );
        assert!(controls.realtime_effects[0].pitch_edits_only.unwrap());
        p.restore(false);
        assert!(p.regions.is_empty());
        p.restore(false);
        assert!(!p.regions.is_empty());
    }
    fn metadata() -> RemoteEffectMetadata {
        serde_json::from_value(serde_json::json!({"id":"pitchCorrection","name":"Pitch correction","parameters":[{"id":"reference","name":"Reference","minimum":400,"maximum":480,"defaultValue":440,"unit":"Hz","logarithmic":false,"integer":false,"automationUnit":"normalized"},{"id":"strength","name":"Strength","minimum":0,"maximum":1,"defaultValue":1,"unit":"%","logarithmic":false,"integer":false,"automationUnit":"normalized"}],"factoryPresets":[{"name":"Natural","values":{"reference":440,"strength":0.6}}],"modes":["realtime"]})).unwrap()
    }
    #[test]
    fn processing_undo_restores_reference_and_does_not_disable_automatic_pitch() {
        let mut source = track();
        source.realtime_effects.as_mut().unwrap()[0].pitch_edits = None;
        let mut p = Pitch::open(&source, 0, 5, (0, 1000), 48000., 120.).unwrap();
        p.command(
            &["parameter".into(), "reference".into(), "450".into()],
            Some(&metadata()),
        )
        .unwrap();
        assert_eq!(p.reference, 450.);
        assert!(!p.manual_targets_changed);
        let RemoteCommand::UpdateTrackControls { controls, .. } = p.use_targets() else {
            panic!()
        };
        assert_eq!(controls.realtime_effects[0].pitch_edits, None);
        assert_ne!(controls.realtime_effects[0].pitch_edits_only, Some(true));
        p.command(&["processing-preset".into(), "0".into()], Some(&metadata()))
            .unwrap();
        assert_eq!(p.reference, 440.);
        p.restore(false);
        assert_eq!(p.reference, 450.);
        p.restore(false);
        assert_eq!(p.reference, 442.);
        assert!(p
            .command(
                &["parameter".into(), "strength".into(), "NaN".into()],
                Some(&metadata())
            )
            .is_err());
        p.command(
            &["parameter".into(), "strength".into(), "65".into()],
            Some(&metadata()),
        )
        .unwrap();
        assert_eq!(
            p.controls.realtime_effects[0].parameters.as_ref().unwrap()["strength"],
            0.65
        );
        p.add(500, 60.).unwrap();
        assert!(p.manual_targets_changed);
        p.restore(false);
        assert!(!p.manual_targets_changed);
    }
    #[test]
    fn target_cache_queries_visible_subset_of_a_large_chart_and_survives_redraw() {
        let mut p = open();
        p.regions[0].points = Some(
            (200..800)
                .map(|frame| PitchEditPoint {
                    id: None,
                    frame,
                    note: 60.,
                    curve: None,
                    curve_start: None,
                    curve_end: None,
                })
                .collect(),
        );
        p.draw(&mut Canvas::new(100, 30));
        let curves = p.targets.curves.as_ptr();
        let refs = p.targets.refs.as_ptr();
        p.start = 400;
        p.span = 20;
        p.draw(&mut Canvas::new(100, 30));
        assert_eq!(curves, p.targets.curves.as_ptr());
        assert_eq!(refs, p.targets.refs.as_ptr());
        let first = p.targets.points.partition_point(|p| p.0 < 400);
        let end = p.targets.points.partition_point(|p| p.0 <= 420);
        assert_eq!(end - first, 21);
        assert_eq!(p.targets.region(&p.regions, 500), Some(0));
        p.add(410, 61.).unwrap();
        assert!(p.targets_dirty);
        p.draw(&mut Canvas::new(100, 30));
        assert!(!p.targets_dirty);
    }
    #[test]
    fn named_notes_and_fractional_targets_have_finite_native_bounds() {
        assert_eq!(parse_note("C#4").unwrap(), 61.);
        assert_eq!(parse_note("Db4").unwrap(), 61.);
        assert_eq!(parse_note("60.5").unwrap(), 60.5);
        for bad in ["NaN", "96", "C99", "C9223372036854775807", "Q4"] {
            assert!(parse_note(bad).is_err());
        }
    }
    #[test]
    fn stale_or_other_workspace_analysis_is_not_installed() {
        let mut p = open();
        let mut result = response(&p, "pitch.analyze", 6);
        result.tag.workspace_id = Some("old".into());
        assert!(!p.response(&result, Some(&track()), Some(5)).unwrap());
        assert!(p.source.is_empty());
        result.tag.workspace_id = Some(p.workspace_id.clone());
        assert!(p.response(&result, Some(&track()), Some(7)).unwrap());
        assert!(p.source.is_empty());
        assert!(p.status.contains("Source changed"));
    }
    #[test]
    fn authoritative_target_receipt_waits_for_metadata_and_respects_reference() {
        let mut p = open();
        let result = response(&p, "pitch.targets", 6);
        p.response(&result, Some(&track()), Some(5)).unwrap();
        assert!(p.source.is_empty());
        assert!(p.pending_result.is_some());
        p.install_pending(Some(&track()), Some(6)).unwrap();
        assert_eq!(p.revision, 6);
        assert_eq!(p.source.len(), 1);
        assert_eq!(p.center, 69.);
        let before = p.center;
        p.source.push(RemotePitchPoint {
            frame: 500,
            frequency: 1000.,
            confidence: 0.7,
        });
        p.draw(&mut Canvas::new(80, 24));
        assert_eq!(p.center, before);
    }
    #[test]
    fn draft_edits_are_retained_when_source_analysis_finishes() {
        let mut p = open();
        let result = response(&p, "pitch.analyze", 6);
        p.add(400, 63.).unwrap();
        p.response(&result, Some(&track()), Some(5)).unwrap();
        assert!(p
            .regions
            .iter()
            .flat_map(|r| r.points.as_ref().unwrap())
            .any(|point| point.frame == 400 && point.note == 63.));
        assert!(p.dirty);
        assert!(p.request("pitch.render", true, false, None).is_err());
    }
    #[test]
    fn chart_and_scope_requests_are_bounded_and_do_not_send_main_transport() {
        let mut p = open();
        let request = p.request("pitch.analyze", false, false, None).unwrap();
        let RemoteCommand::Editor { intent } = request else {
            panic!()
        };
        assert_eq!(intent.integers["start"], 200);
        assert_eq!(intent.integers["end"], 800);
        assert_eq!(intent.strings["workspaceID"], p.workspace_id);
        assert!(p.request("pitch.render", false, false, None).is_err());
        let RemoteCommand::Editor { intent } = p.stop_request() else {
            panic!()
        };
        assert_eq!(intent.operation, "audition.stop");
        assert_eq!(intent.strings["kind"], "pitch");
        p.draw(&mut Canvas::new(20, 8));
        p.key(Key::Char('z')).unwrap();
        assert!(p.vertical < 24.);
    }
}
