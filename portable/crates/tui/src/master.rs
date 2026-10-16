//! Native mix measurements and a private, manually editable mastering proposal.
use crate::{
    automation::{Automation, Target},
    input::Key,
    render::Canvas,
    worker::EditorResponse,
};
use base64::Engine;
use soundroom_client::{auth::new_id, models::*, validation::validate_model};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Chart,
    Rack,
}
#[derive(Default)]
struct Chart {
    key: (i64, i64, usize, usize),
    cells: Vec<(usize, usize, char, u8)>,
}
pub struct Master {
    pub master_id: String,
    pub revision: i64,
    pub report: Option<MasteringAnalysis>,
    pub proposed: Vec<RealtimeEffect>,
    pub busy: bool,
    pub applied: bool,
    pub status: String,
    pub automation: Option<Automation>,
    pub target: f64,
    pub intensity: f64,
    source: Vec<(String, i64)>,
    pending_source: Option<Vec<(String, i64)>>,
    pending_revision: Option<i64>,
    nonce: Option<String>,
    awaiting_analysis: Option<i64>,
    effect: usize,
    parameter: usize,
    section: usize,
    focus: Focus,
    start: i64,
    span: i64,
    columns: usize,
    rows: usize,
    rate: f64,
    bpm: f64,
    chart: Chart,
    chart_dirty: bool,
    undo: Vec<Vec<RealtimeEffect>>,
    redo: Vec<Vec<RealtimeEffect>>,
}
impl Master {
    pub fn open(snapshot: &RemoteSnapshot, rate: f64) -> Result<Self, String> {
        let track = snapshot
            .session
            .master_track
            .as_ref()
            .ok_or("This host has no Master track.")?;
        let id = track.id.as_ref().ok_or("Missing Master track ID.")?;
        let revision = *snapshot
            .track_revisions
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(id))
            .map(|(_, v)| v)
            .ok_or("Missing Master track revision.")?;
        let report = snapshot.session.mastering_analysis.clone();
        if let Some(r) = &report {
            validate_analysis(r)?;
        }
        let span = report
            .as_ref()
            .map_or((rate * 30.) as i64, |r| r.analyzed_frames)
            .max(1);
        Ok(Self {
            master_id: id.clone(),
            revision,
            report,
            proposed: vec![],
            busy: false,
            applied: false,
            status:
                "Analyze the mix, request a native proposal, then tune each control before Use."
                    .into(),
            automation: None,
            target: -14.,
            intensity: 0.65,
            source: source(snapshot),
            pending_source: None,
            pending_revision: None,
            nonce: None,
            awaiting_analysis: None,
            effect: 0,
            parameter: 0,
            section: 0,
            focus: Focus::Chart,
            start: 0,
            span,
            columns: 80,
            rows: 10,
            rate,
            bpm: snapshot.session.timing.as_ref().map_or(120., |t| t.tempo),
            chart: Chart::default(),
            chart_dirty: true,
            undo: vec![],
            redo: vec![],
        })
    }
    pub fn stale(&self, snapshot: &RemoteSnapshot) -> bool {
        self.source != source(snapshot)
    }
    pub fn request(
        &mut self,
        operation: &str,
        snapshot: &RemoteSnapshot,
    ) -> Result<RemoteCommand, String> {
        if self.busy || self.awaiting_analysis.is_some() {
            return Err("Wait for the current Master operation or cancel it explicitly.".into());
        }
        if operation == "master.proposal" && self.report.is_none() {
            return Err("Analyze the mix before requesting a proposal.".into());
        }
        if operation == "master.proposal" && self.stale(snapshot) {
            return Err("Mix changed. Analyze again before requesting a proposal.".into());
        }
        self.pending_source = Some(source(snapshot));
        self.pending_revision = Some(
            *snapshot
                .track_revisions
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(&self.master_id))
                .map(|(_, v)| v)
                .ok_or("Master track was removed.")?,
        );
        let nonce = new_id().map_err(|e| e.to_string())?;
        self.nonce = Some(nonce.clone());
        self.busy = true;
        self.status = if operation == "master.analyze" {
            "Measuring the full mix on the hardware server…"
        } else {
            "Preparing a bounded native proposal…"
        }
        .into();
        Ok(editor(
            operation,
            None,
            BTreeMap::from([("workspaceID".into(), nonce)]),
            BTreeMap::from([
                ("targetLUFS".into(), self.target),
                ("intensity".into(), self.intensity),
            ]),
            None,
        ))
    }
    pub fn response(
        &mut self,
        response: &EditorResponse,
        snapshot: &RemoteSnapshot,
        metadata: &[RemoteEffectMetadata],
    ) -> Result<bool, String> {
        if self.nonce.is_none()
            || self.nonce.as_ref() != response.tag.workspace_id.as_ref()
            || !matches!(
                response.tag.operation.as_str(),
                "master.proposal" | "master.analyze" | "master.applyProposal"
            )
        {
            return Ok(false);
        }
        self.busy = false;
        if response.reply.status != RemoteReplyStatus::Accepted {
            self.status = response.reply.message.clone();
            self.nonce = None;
            self.pending_source = None;
            self.pending_revision = None;
            return Ok(true);
        }
        if response.tag.operation == "master.applyProposal" {
            self.applied = true;
            self.nonce = None;
            self.status = "Native Master proposal applied.".into();
            return Ok(true);
        }
        if self.pending_source.as_ref() != Some(&source(snapshot)) {
            self.status =
                "The mix changed during analysis. Analyze again before using a proposal.".into();
            self.nonce = None;
            return Ok(true);
        }
        if response.tag.operation == "master.analyze" {
            self.awaiting_analysis = Some(response.reply.revision);
            self.install(snapshot)?;
            return Ok(true);
        }
        let payload = response
            .reply
            .payload
            .as_ref()
            .ok_or("Master proposal has no processing rack.")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .map_err(|_| "Invalid proposal encoding.")?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err("Master proposal exceeds the control frame budget.".into());
        }
        let json: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        validate_model(&json, "[RealtimeEffect]").map_err(|e| e.to_string())?;
        let effects: Vec<RealtimeEffect> =
            serde_json::from_value(json).map_err(|e| e.to_string())?;
        validate_rack(&effects, metadata)?;
        if let Some(source) = self.pending_source.take() {
            self.source = source;
        }
        if let Some(revision) = self.pending_revision.take() {
            self.revision = revision;
        }
        self.proposed = effects;
        self.effect = 0;
        self.parameter = 0;
        self.undo.clear();
        self.redo.clear();
        self.nonce = None;
        self.status="Private native proposal. Parameters and smooth section automation remain editable. Use replaces the Master rack; server Undo retains it.".into();
        Ok(true)
    }
    pub fn install(&mut self, snapshot: &RemoteSnapshot) -> Result<(), String> {
        if let Some(revision) = self.awaiting_analysis {
            if snapshot.revision < revision {
                self.status = "Waiting for measured Master metadata.".into();
                return Ok(());
            }
            self.awaiting_analysis = None;
            self.nonce = None;
            if self.pending_source.as_ref() != Some(&source(snapshot)) {
                self.pending_source = None;
                self.status =
                    "The mix changed before measured metadata arrived. Analyze again.".into();
                return Ok(());
            }
            let report =
                snapshot.session.mastering_analysis.clone().ok_or(
                    "Analysis completed without measured metadata. Refresh or analyze again.",
                )?;
            validate_analysis(&report)?;
            if let Some(source) = self.pending_source.take() {
                self.source = source;
            }
            if let Some(revision) = self.pending_revision.take() {
                self.revision = revision;
            }
            self.start = 0;
            self.span = report.analyzed_frames.max(1);
            self.section = 0;
            self.report = Some(report);
            self.chart_dirty = true;
            self.proposed.clear();
            self.undo.clear();
            self.redo.clear();
            self.status="Measured loudness, true peak, crest and tonal balance. :proposal prepares a conservative native rack.".into();
        }
        Ok(())
    }
    pub fn use_proposal(
        &self,
        snapshot: &RemoteSnapshot,
        metadata: &[RemoteEffectMetadata],
    ) -> Result<RemoteCommand, String> {
        if self.busy || self.awaiting_analysis.is_some() {
            return Err("Wait for Master measurements before using a proposal.".into());
        }
        if self.stale(snapshot) {
            return Err("Mix changed. Analyze again before applying this proposal.".into());
        }
        if self.report.as_ref().map_or(true, |r| r.true_peak_db < -70.) || self.proposed.is_empty()
        {
            return Err("Analyze audible audio and request a proposal first.".into());
        }
        validate_rack(&self.proposed, metadata)?;
        let bytes = serde_json::to_vec(&self.proposed).map_err(|e| e.to_string())?;
        if bytes.len() > 6 * 1024 * 1024 {
            return Err("Proposal exceeds the bounded request budget.".into());
        }
        Ok(editor(
            "master.applyProposal",
            Some(self.master_id.clone()),
            BTreeMap::new(),
            BTreeMap::new(),
            Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
        ))
    }
    pub fn apply_request(
        &mut self,
        snapshot: &RemoteSnapshot,
        metadata: &[RemoteEffectMetadata],
    ) -> Result<RemoteCommand, String> {
        let mut request = self.use_proposal(snapshot, metadata)?;
        let nonce = new_id().map_err(|e| e.to_string())?;
        if let RemoteCommand::Editor { intent } = &mut request {
            intent.strings.insert("workspaceID".into(), nonce.clone());
        }
        self.nonce = Some(nonce);
        self.pending_source = Some(source(snapshot));
        self.busy = true;
        self.status =
            "Applying the native proposal; the draft stays open until its receipt.".into();
        Ok(request)
    }
    pub fn failure(&mut self, tag: &crate::worker::ResponseTag, message: &str) -> bool {
        if self.nonce.is_some() && self.nonce.as_ref() == tag.workspace_id.as_ref() {
            self.busy = false;
            self.awaiting_analysis = None;
            self.nonce = None;
            self.pending_source = None;
            self.pending_revision = None;
            self.status = message.into();
            return true;
        }
        false
    }
    fn meta<'a>(&self, metadata: &'a [RemoteEffectMetadata]) -> Option<&'a RemoteEffectMetadata> {
        let effect = self.proposed.get(self.effect)?;
        let kind = serde_json::to_value(&effect.kind).ok()?;
        metadata
            .iter()
            .find(|m| Some(m.id.as_str()) == kind.as_str())
    }
    fn capture(&mut self) {
        if self.undo.len() == 100 {
            self.undo.remove(0);
        }
        self.undo.push(self.proposed.clone());
        self.redo.clear();
        self.trim_history();
    }
    fn trim_history(&mut self) {
        let size = |rack: &Vec<RealtimeEffect>| {
            rack.iter()
                .map(|e| {
                    512 + e.parameters.as_ref().map_or(0, |p| p.len() * 128)
                        + e.parameter_automation
                            .as_ref()
                            .map_or(0, |p| p.values().map(|ps| ps.len() * 64).sum::<usize>())
                })
                .sum::<usize>()
        };
        while self.undo.iter().chain(&self.redo).map(size).sum::<usize>() > 32 * 1024 * 1024 {
            if !self.undo.is_empty() {
                self.undo.remove(0);
            } else {
                self.redo.remove(0);
            }
        }
    }
    fn restore(&mut self, redo: bool) {
        let previous = if redo {
            self.redo.pop()
        } else {
            self.undo.pop()
        };
        if let Some(previous) = previous {
            let current = std::mem::replace(&mut self.proposed, previous);
            if redo {
                self.undo.push(current);
            } else {
                self.redo.push(current);
            }
            self.trim_history();
        }
    }
    pub fn close_nested(&mut self) -> bool {
        self.automation.take().is_some()
    }
    fn use_curve(&mut self) {
        if let Some(automation) = self.automation.take() {
            let RemoteCommand::UpdateTrackControls { controls, .. } = automation.request() else {
                return;
            };
            self.capture();
            self.proposed = controls.realtime_effects;
            self.status =
                "Private proposal curve updated. Enter uses the complete proposal.".into();
        }
    }
    pub fn key(&mut self, key: Key, metadata: &[RemoteEffectMetadata]) -> Result<bool, String> {
        if let Some(automation) = self.automation.as_mut() {
            if automation.key(key)? {
                self.use_curve();
            }
            return Ok(false);
        }
        if (self.busy || self.awaiting_analysis.is_some())
            && matches!(
                key,
                Key::Char('h' | 'l' | 'm' | 'u') | Key::Ctrl('r') | Key::Enter
            )
        {
            return Err("Wait for the current Master operation before changing the draft.".into());
        }
        match key {
            Key::Enter => return Ok(true),
            Key::Tab => {
                self.focus = if self.focus == Focus::Chart {
                    Focus::Rack
                } else {
                    Focus::Chart
                }
            }
            Key::Char('u') => self.restore(false),
            Key::Ctrl('r') => self.restore(true),
            Key::Char('+') => {
                self.span = (self.span as f64 / 1.5).round().max(1.) as i64;
                self.chart_dirty = true;
            }
            Key::Char('-') => {
                self.span = (self.span as f64 * 1.5).round().min(
                    self.report
                        .as_ref()
                        .map_or(17_280_000_000., |r| r.analyzed_frames.max(1) as f64),
                ) as i64;
                self.chart_dirty = true;
            }
            Key::Char('H') => {
                self.start = self.start.saturating_sub(self.span / 4).max(0);
                self.chart_dirty = true;
            }
            Key::Char('L') => {
                self.start = self.start.saturating_add(self.span / 4).min(
                    self.report
                        .as_ref()
                        .map_or(0, |r| (r.analyzed_frames - self.span).max(0)),
                );
                self.chart_dirty = true;
            }
            Key::Char('J') => {
                self.effect = (self.effect + 1).min(self.proposed.len().saturating_sub(1));
                self.parameter = 0;
            }
            Key::Char('K') => {
                self.effect = self.effect.saturating_sub(1);
                self.parameter = 0;
            }
            Key::Char('j' | 'k') => {
                let down = matches!(key, Key::Char('j'));
                if self.focus == Focus::Chart {
                    let count = self.report.as_ref().map_or(0, |r| r.sections.len());
                    self.section = if down {
                        (self.section + 1).min(count.saturating_sub(1))
                    } else {
                        self.section.saturating_sub(1)
                    };
                } else {
                    let count = self.meta(metadata).map_or(0, |m| m.parameters.len());
                    self.parameter = if down {
                        (self.parameter + 1).min(count.saturating_sub(1))
                    } else {
                        self.parameter.saturating_sub(1)
                    };
                }
            }
            Key::Char('h' | 'l') if self.focus == Focus::Rack => {
                let p = self
                    .meta(metadata)
                    .and_then(|m| m.parameters.get(self.parameter))
                    .cloned()
                    .ok_or("Select a proposed effect parameter.")?;
                let old = self.proposed[self.effect]
                    .parameters
                    .as_ref()
                    .and_then(|ps| ps.get(&p.id))
                    .copied()
                    .unwrap_or(p.default_value);
                let direction = if matches!(key, Key::Char('l')) {
                    1.
                } else {
                    -1.
                };
                let mut value = if p.integer {
                    old + direction
                } else if p.logarithmic && p.minimum > 0. {
                    old * ((p.maximum / p.minimum).ln() / 100. * direction).exp()
                } else {
                    old + (p.maximum - p.minimum) / 100. * direction
                };
                value = value.clamp(p.minimum, p.maximum);
                self.capture();
                self.proposed[self.effect]
                    .parameters
                    .get_or_insert_with(BTreeMap::new)
                    .insert(p.id, value);
            }
            Key::Char('m') => {
                if self.proposed.get(self.effect).is_some() {
                    self.capture();
                    let e = &mut self.proposed[self.effect];
                    e.enabled = Some(!e.enabled.unwrap_or(true));
                }
            }
            Key::Mouse {
                button: 0,
                column,
                row,
                released: false,
            } if usize::from(row) >= 6 && usize::from(row) < 6 + self.rows => {
                if let Some(report) = &self.report {
                    let frame = self.start
                        + ((usize::from(column).saturating_sub(10) as f64
                            / self.columns.max(1) as f64)
                            * self.span as f64) as i64;
                    self.section = report
                        .sections
                        .partition_point(|s| s.end <= frame)
                        .min(report.sections.len().saturating_sub(1));
                }
            }
            _ => {}
        }
        Ok(false)
    }
    pub fn command(
        &mut self,
        words: &[String],
        metadata: &[RemoteEffectMetadata],
    ) -> Result<bool, String> {
        if let Some(automation) = self.automation.as_mut() {
            if automation.command(words)? {
                self.use_curve();
            }
            return Ok(false);
        }
        if (self.busy || self.awaiting_analysis.is_some())
            && words.first().map(String::as_str) != Some("rack")
        {
            return Err("Wait for the current Master operation before changing the draft.".into());
        }
        let arg = |i: usize| words.get(i).ok_or("Missing argument.");
        match words[0].as_str(){
            "target"=>{let value=arg(1)?.parse::<f64>().map_err(|_|"Loudness aim must be numeric.")?;if !value.is_finite()||!(-24. ..= -8.).contains(&value){return Err("Loudness aim range: -24..-8 LUFS.".into());}self.target=value;},
            "intensity"=>{let value=arg(1)?.parse::<f64>().map_err(|_|"Proposal amount must be numeric.")?;if !value.is_finite()||!(0. ..=1.).contains(&value){return Err("Proposal amount range: 0..1.".into());}self.intensity=value;},
            "effect"=>{let index=arg(1)?.parse::<usize>().map_err(|_|"Effect index must be an integer.")?;if index>=self.proposed.len(){return Err("Unknown proposal effect.".into());}self.effect=index;self.parameter=0;},
            "parameter"=>{let meta=self.meta(metadata).ok_or("Select a proposed effect first.")?;let p=meta.parameters.iter().find(|p|Some(&p.id)==words.get(1)).cloned().ok_or("Unknown advertised parameter ID.")?;let scale=if p.unit=="%"{100.}else{1.};let value=arg(2)?.parse::<f32>().map_err(|_|"Value must be numeric.")?/scale;if !value.is_finite()||value<p.minimum||value>p.maximum||p.integer&&value.fract()!=0.{return Err(format!("{} range: {}..{} {}",p.name,p.minimum*scale,p.maximum*scale,p.unit));}self.capture();self.proposed[self.effect].parameters.get_or_insert_with(BTreeMap::new).insert(p.id,value);},
            "processing-preset" => {
                let index=arg(1)?.parse::<usize>().map_err(|_|"Preset index must be an integer.")?;
                let preset=self.meta(metadata).and_then(|m|m.factory_presets.get(index)).cloned().ok_or("Unknown advertised processing preset.")?;
                self.capture();self.proposed[self.effect].parameters.get_or_insert_with(BTreeMap::new).extend(preset.values);
            },
            "rack" => { self.status=self.proposed.iter().enumerate().map(|(i,e)|{let kind=serde_json::to_value(&e.kind).unwrap();format!("{} {} ({})",i,kind.as_str().unwrap_or("insert"),if e.enabled.unwrap_or(true){"on"}else{"off"})}).collect::<Vec<_>>().join(" / "); },
            "toggle"=>{self.key(Key::Char('m'),metadata)?;},"undo"=>self.restore(false),"redo"=>self.restore(true),"use"|"commit"=>return Ok(true),
            "automation"=>{let p=self.meta(metadata).and_then(|m|m.parameters.iter().find(|p|Some(&p.id)==words.get(1))).cloned().ok_or("Usage: automation parameterID on a proposed effect.")?;let track:Track=serde_json::from_value(serde_json::json!({"id":self.master_id,"name":"Master proposal","gain":1,"pan":0,"clips":[],"realtimeEffects":self.proposed})).map_err(|e|e.to_string())?;self.automation=Some(Automation::open(&track,self.revision,Target::Parameter(self.effect,p.id.clone(),p),self.start,self.rate,self.bpm)?);},
            _=>return Err("Master: :analyze; :proposal; :target -14; :intensity 0.65; :effect index; :parameter ID value; :automation ID; :use.".into())
        }
        Ok(false)
    }
    fn chart(&mut self, width: usize, height: usize) {
        let key = (self.start, self.span, width, height);
        if !self.chart_dirty && self.chart.key == key {
            return;
        }
        self.chart = Chart { key, cells: vec![] };
        self.chart_dirty = false;
        let Some(report) = &self.report else {
            return;
        };
        for x in 0..width {
            let a = self.start + (x as f64 / width.max(1) as f64 * self.span as f64) as i64;
            let b = self.start + ((x + 1) as f64 / width.max(1) as f64 * self.span as f64) as i64;
            let first = report.sections.partition_point(|s| s.end < a);
            let sections = &report.sections[first..];
            let visible: Vec<_> = sections.iter().take_while(|s| s.start <= b).collect();
            if visible.is_empty() {
                continue;
            }
            let peak = visible.iter().map(|s| s.true_peak).fold(-120., f32::max);
            let power = visible
                .iter()
                .map(|s| 10_f64.powf(f64::from(s.loudness) / 10.))
                .sum::<f64>()
                / visible.len() as f64;
            let loudness = (10. * power.max(1e-12).log10()) as f32;
            for (level, ch, color) in [(loudness, '*', 179), (peak, '+', 114)] {
                let y = (-level.clamp(-60., 0.) / 60. * height.saturating_sub(1) as f32).round()
                    as usize;
                self.chart.cells.push((x, y, ch, color));
            }
        }
    }
    pub fn draw(&mut self, c: &mut Canvas, metadata: &[RemoteEffectMetadata], stale: bool) {
        if let Some(automation) = self.automation.as_mut() {
            automation.draw(c);
            return;
        }
        for y in 2..c.height.saturating_sub(3) {
            c.fill(0, y, c.width, ' ', 252, 234);
        }
        c.text(
            1,
            2,
            if stale {
                "MASTER / MIX CHANGED — analyze again"
            } else if self.busy {
                "MASTER / Measuring on server"
            } else {
                "MASTER / Native analysis and private proposal"
            },
            179,
            234,
            true,
        );
        c.text(1,3,&format!("Aim {:.1} LUFS / amount {:.0}% / :analyze :proposal :use / Space mix / Tab chart or rack",self.target,self.intensity*100.),245,234,false);
        if let Some(report) = &self.report {
            c.text(1,4,&format!("Integrated {:.1} LUFS   true peak {:.1} dBTP   crest {:.1} dB   bass {:.0}% / air {:.0}%",report.integrated_lufs,report.true_peak_db,report.crest_db,report.bass_fraction*100.,report.air_fraction*100.),252,234,true);
        }
        self.columns = c.width.saturating_sub(10).max(1);
        self.rows = (c.height / 3).clamp(1, 12);
        self.chart(self.columns, self.rows);
        for row in 0..self.rows {
            let level = -60. * row as f32 / self.rows.saturating_sub(1).max(1) as f32;
            c.text(1, 6 + row, &format!("{:4.0}", level), 245, 234, false);
            c.fill(9, 6 + row, self.columns, '.', 238, 234);
        }
        for &(x, y, ch, color) in &self.chart.cells {
            c.put(9 + x, 6 + y, ch, color, 234, true);
        }
        let base = 7 + self.rows;
        if let Some(section) = self
            .report
            .as_ref()
            .and_then(|r| r.sections.get(self.section))
        {
            c.text(
                1,
                base,
                &format!(
                    "Section {}  {:.2}–{:.2}s / {:.1} LUFS / {:.1} dBTP / resonance {:.2} dB",
                    self.section + 1,
                    section.start as f64 / self.rate,
                    section.end as f64 / self.rate,
                    section.loudness,
                    section.true_peak,
                    section.resonance
                ),
                252,
                234,
                false,
            );
        }
        c.text(
            1,
            base + 1,
            if self.focus == Focus::Chart {
                "CHART: j/k section; +/- zoom; H/L scroll; * loudness / + true peak"
            } else {
                "RACK: J/K insert; j/k parameter; h/l value; m enable; :automation ID"
            },
            245,
            234,
            false,
        );
        if let Some(meta) = self.meta(metadata) {
            let effect = &self.proposed[self.effect];
            c.text(
                1,
                base + 2,
                &format!(
                    "Insert {} / {} / {}",
                    self.effect,
                    meta.name,
                    if effect.enabled.unwrap_or(true) {
                        "Enabled"
                    } else {
                        "Bypassed"
                    }
                ),
                81,
                234,
                true,
            );
            for (line, p) in meta
                .parameters
                .iter()
                .enumerate()
                .skip(self.parameter.saturating_sub(2))
                .take(c.height.saturating_sub(base + 7).min(8))
            {
                let value = effect
                    .parameters
                    .as_ref()
                    .and_then(|ps| ps.get(&p.id))
                    .copied()
                    .unwrap_or(p.default_value);
                let scale = if p.unit == "%" { 100. } else { 1. };
                let points = effect
                    .parameter_automation
                    .as_ref()
                    .and_then(|ps| ps.get(&p.id))
                    .map_or(0, Vec::len);
                c.text(
                    1,
                    base + 3 + line - self.parameter.saturating_sub(2),
                    &format!(
                        "{} {} {:.4} {} / {} curve points",
                        if line == self.parameter { ">" } else { " " },
                        p.name,
                        value * scale,
                        p.unit,
                        points
                    ),
                    252,
                    234,
                    line == self.parameter,
                );
            }
        }
        c.text(1, c.height.saturating_sub(4), &self.status, 245, 234, false);
    }
}
fn source(snapshot: &RemoteSnapshot) -> Vec<(String, i64)> {
    snapshot
        .session
        .tracks
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(|t| t.id.as_ref())
        .map(|id| {
            (
                id.clone(),
                snapshot
                    .track_revisions
                    .get(id)
                    .or_else(|| snapshot.track_revisions.get(&id.to_ascii_uppercase()))
                    .copied()
                    .unwrap_or(-1),
            )
        })
        .collect()
}
fn editor(
    operation: &str,
    track_id: Option<String>,
    strings: BTreeMap<String, String>,
    numbers: BTreeMap<String, f64>,
    payload: Option<String>,
) -> RemoteCommand {
    RemoteCommand::Editor {
        intent: RemoteEditorIntent {
            operation: operation.into(),
            context: RemoteEditorContext {
                track_id,
                clip_id: None,
                clip_ids: vec![],
                cursor_frame: 0,
                range_start: None,
                range_end: None,
                effect_id: None,
                automation_target: None,
            },
            integers: BTreeMap::new(),
            numbers,
            strings,
            flags: BTreeMap::new(),
            payload,
        },
    }
}
fn validate_analysis(report: &MasteringAnalysis) -> Result<(), String> {
    if report.analyzed_frames < 0
        || report.sections.len() > 200000
        || !report.integrated_lufs.is_finite()
        || !report.true_peak_db.is_finite()
        || !report.crest_db.is_finite()
        || !report.bass_fraction.is_finite()
        || !(0. ..=1.).contains(&report.bass_fraction)
        || !report.air_fraction.is_finite()
        || !(0. ..=1.).contains(&report.air_fraction)
        || report.sections.iter().any(|s| {
            s.start < 0
                || s.end <= s.start
                || s.end > report.analyzed_frames
                || !s.loudness.is_finite()
                || !s.true_peak.is_finite()
                || !s.resonance.is_finite()
        })
        || report.sections.windows(2).any(|ss| ss[0].end > ss[1].start)
    {
        return Err("Invalid measured Master section data.".into());
    }
    Ok(())
}
fn validate_rack(
    effects: &[RealtimeEffect],
    metadata: &[RemoteEffectMetadata],
) -> Result<(), String> {
    if effects.len() > 64 {
        return Err("Master proposal exceeds 64 inserts.".into());
    }
    for e in effects {
        let kind = serde_json::to_value(&e.kind).map_err(|e| e.to_string())?;
        let meta = metadata
            .iter()
            .find(|m| Some(m.id.as_str()) == kind.as_str())
            .ok_or("Proposal contains an effect not advertised by this host.")?;
        if let Some(parameters) = &e.parameters {
            for (id, value) in parameters {
                let p = meta
                    .parameters
                    .iter()
                    .find(|p| &p.id == id)
                    .ok_or("Proposal contains an unknown effect parameter.")?;
                if !value.is_finite()
                    || *value < p.minimum
                    || *value > p.maximum
                    || p.integer && value.fract() != 0.
                {
                    return Err(format!("{} is outside its native range.", p.name));
                }
            }
        }
        if let Some(curves) = &e.parameter_automation {
            for (id, points) in curves {
                if !meta.parameters.iter().any(|p| &p.id == id) {
                    return Err("Proposal contains an unknown automation parameter.".into());
                }
                if points.len() > 200000
                    || points.windows(2).any(|ps| ps[0].frame >= ps[1].frame)
                    || points.iter().any(|p| {
                        p.frame < 0
                            || !p.value.is_finite()
                            || !(0. ..=1.).contains(&p.value)
                            || p.curve_start
                                .is_some_and(|v| !v.is_finite() || !(0. ..=1.).contains(&v))
                            || p.curve_end
                                .is_some_and(|v| !v.is_finite() || !(0. ..=1.).contains(&v))
                    })
                {
                    return Err("Proposal contains an invalid normalized automation curve.".into());
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::ResponseTag;
    use std::sync::Arc;
    fn snapshot() -> RemoteSnapshot {
        serde_json::from_value(serde_json::json!({"serverID":"00000000-0000-0000-0000-000000000001","serverName":"Studio","revision":5,"trackRevisions":{"00000000-0000-0000-0000-000000000002":4,"00000000-0000-0000-0000-000000000003":3},"session":{"version":9,"name":"Song","tracks":[{"id":"00000000-0000-0000-0000-000000000002","name":"Voice"}],"masterTrack":{"id":"00000000-0000-0000-0000-000000000003","name":"Master"},"masteringAnalysis":{"integratedLUFS":-19,"truePeakDB":-3,"crestDB":12,"bassFraction":0.4,"airFraction":0.2,"analyzedFrames":96000,"sections":[{"start":0,"end":48000,"loudness":-24,"truePeak":-6,"resonance":2},{"start":48000,"end":96000,"loudness":-16,"truePeak":-3,"resonance":1}],"sourceFingerprint":"nativechecked"}},"transport":{"frame":0,"isPlaying":false,"isRecording":false,"isPreparing":false,"hostTime":0},"assets":[],"devices":[],"inputDeviceID":0,"outputDeviceID":0,"peers":[],"status":"Ready"})).unwrap()
    }
    fn metadata() -> Vec<RemoteEffectMetadata> {
        serde_json::from_value(serde_json::json!([{"id":"gain","name":"Gain","parameters":[{"id":"gain","name":"Gain","minimum":-24,"maximum":24,"defaultValue":0,"unit":"dB","integer":false,"logarithmic":false,"automationUnit":"normalized"}],"factoryPresets":[{"name":"Gentle","values":{"gain":1.5}}],"modes":["realtime"]}])).unwrap()
    }
    fn effects() -> Vec<RealtimeEffect> {
        serde_json::from_value(serde_json::json!([{"id":"00000000-0000-0000-0000-000000000004","kind":"gain","enabled":true,"parameters":{"gain":3},"parameterAutomation":{"gain":[{"frame":0,"value":0.5,"curve":"smooth"},{"frame":96000,"value":0.7}]}}])).unwrap()
    }
    fn response(master: &Master, operation: &str, revision: i64) -> EditorResponse {
        EditorResponse {
            tag: ResponseTag {
                operation: operation.into(),
                workspace_id: master.nonce.clone(),
                track_id: None,
                effect_id: None,
                range: None,
                source_revision: Some(5),
                track_revision: None,
            },
            reply: Arc::new(RemoteReply {
                request_id: "00000000-0000-0000-0000-000000000009".into(),
                status: RemoteReplyStatus::Accepted,
                revision,
                message: String::new(),
                payload: Some(
                    base64::engine::general_purpose::STANDARD
                        .encode(serde_json::to_vec(&effects()).unwrap()),
                ),
            }),
        }
    }
    fn open() -> Master {
        Master::open(&snapshot(), 48000.).unwrap()
    }
    #[test]
    fn proposal_uses_native_payload_and_private_parameters_preserve_automation() {
        let mut master = open();
        let snapshot = snapshot();
        master.request("master.proposal", &snapshot).unwrap();
        let result = response(&master, "master.proposal", 5);
        assert!(master.response(&result, &snapshot, &metadata()).unwrap());
        let curves = master.proposed[0].parameter_automation.clone();
        master
            .command(
                &["parameter".into(), "gain".into(), "2.25".into()],
                &metadata(),
            )
            .unwrap();
        assert_eq!(
            master.proposed[0].parameters.as_ref().unwrap()["gain"],
            2.25
        );
        assert_eq!(master.proposed[0].parameter_automation, curves);
        master.restore(false);
        assert_eq!(master.proposed[0].parameters.as_ref().unwrap()["gain"], 3.);
        let RemoteCommand::Editor { intent } = master.use_proposal(&snapshot, &metadata()).unwrap()
        else {
            panic!()
        };
        assert_eq!(intent.operation, "master.applyProposal");
        assert_eq!(intent.context.track_id, Some(master.master_id.clone()));
        let decoded: Vec<RealtimeEffect> = serde_json::from_slice(
            &base64::engine::general_purpose::STANDARD
                .decode(intent.payload.unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(decoded, master.proposed);
    }
    #[test]
    fn queued_query_rejection_keeps_previous_master_binding_and_accepted_apply_closes_with_newer_metadata(
    ) {
        let mut master = open();
        master.proposed = effects();
        let original = master.proposed.clone();
        let mut snapshot = snapshot();
        snapshot.track_revisions.insert(master.master_id.clone(), 9);
        master.request("master.proposal", &snapshot).unwrap();
        assert_eq!(master.revision, 3);
        let failed = response(&master, "master.proposal", 9);
        assert!(master.failure(&failed.tag, "Not sent: queue full"));
        assert_eq!(master.revision, 3);
        assert_eq!(master.proposed, original);
        assert!(!master.busy);
        master.apply_request(&snapshot, &metadata()).unwrap();
        let accepted = response(&master, "master.applyProposal", 9);
        snapshot.revision = 10;
        snapshot
            .track_revisions
            .insert("00000000-0000-0000-0000-000000000002".into(), 10);
        assert!(master.response(&accepted, &snapshot, &metadata()).unwrap());
        assert!(master.applied);
        assert!(!master.busy);
    }
    #[test]
    fn apply_conflict_retains_private_proposal_and_processing_history() {
        let mut master = open();
        master.proposed = effects();
        let snapshot = snapshot();
        master
            .command(
                &["parameter".into(), "gain".into(), "2".into()],
                &metadata(),
            )
            .unwrap();
        master.apply_request(&snapshot, &metadata()).unwrap();
        assert!(master
            .command(
                &["parameter".into(), "gain".into(), "4".into()],
                &metadata()
            )
            .is_err());
        let mut result = response(&master, "master.applyProposal", 6);
        Arc::make_mut(&mut result.reply).status = RemoteReplyStatus::Conflict;
        Arc::make_mut(&mut result.reply).message = "Master changed".into();
        master.response(&result, &snapshot, &metadata()).unwrap();
        assert!(!master.applied);
        assert!(!master.busy);
        assert_eq!(master.proposed[0].parameters.as_ref().unwrap()["gain"], 2.);
        master.restore(false);
        assert_eq!(master.proposed[0].parameters.as_ref().unwrap()["gain"], 3.);
    }
    #[test]
    fn different_query_or_changed_mix_cannot_adopt_a_proposal() {
        let mut master = open();
        let mut snapshot = snapshot();
        master.request("master.proposal", &snapshot).unwrap();
        let mut result = response(&master, "master.proposal", 5);
        result.tag.workspace_id = Some("old-workspace".into());
        assert!(!master.response(&result, &snapshot, &metadata()).unwrap());
        assert!(master.proposed.is_empty());
        result.tag.workspace_id = master.nonce.clone();
        snapshot
            .track_revisions
            .insert("00000000-0000-0000-0000-000000000002".into(), 6);
        assert!(master.response(&result, &snapshot, &metadata()).unwrap());
        assert!(master.proposed.is_empty());
        assert!(master.status.contains("mix changed"));
    }
    #[test]
    fn fresh_analysis_can_replace_a_stale_mix_but_failed_query_cannot_revalidate_old_proposal() {
        let mut master = open();
        master.proposed = effects();
        let mut snapshot = snapshot();
        snapshot
            .track_revisions
            .insert("00000000-0000-0000-0000-000000000002".into(), 6);
        assert!(master.stale(&snapshot));
        assert!(master.request("master.proposal", &snapshot).is_err());
        master.request("master.analyze", &snapshot).unwrap();
        let mut result = response(&master, "master.analyze", 6);
        Arc::make_mut(&mut result.reply).status = RemoteReplyStatus::Rejected;
        master.response(&result, &snapshot, &metadata()).unwrap();
        assert!(master.stale(&snapshot));
        assert!(master.use_proposal(&snapshot, &metadata()).is_err());
        master.request("master.analyze", &snapshot).unwrap();
        let result = response(&master, "master.analyze", 6);
        master.response(&result, &snapshot, &metadata()).unwrap();
        snapshot.revision = 6;
        master.install(&snapshot).unwrap();
        assert!(!master.stale(&snapshot));
        assert!(master.proposed.is_empty());
        let mut unrelated = result;
        unrelated.tag.workspace_id = None;
        assert!(!master.response(&unrelated, &snapshot, &metadata()).unwrap());
    }
    #[test]
    fn analysis_receipt_waits_for_measured_metadata() {
        let mut master = open();
        master.report = None;
        let mut snapshot = snapshot();
        master.request("master.analyze", &snapshot).unwrap();
        let result = response(&master, "master.analyze", 6);
        master.response(&result, &snapshot, &metadata()).unwrap();
        assert!(master.report.is_none());
        assert!(master.request("master.proposal", &snapshot).is_err());
        snapshot.revision = 6;
        master.install(&snapshot).unwrap();
        assert_eq!(master.report.as_ref().unwrap().integrated_lufs, -19.);
        assert!(master.awaiting_analysis.is_none());
        assert!(master.proposed.is_empty());
    }
    #[test]
    fn nested_proposal_curves_commit_privately_and_undo_retains_prior_curve() {
        let mut master = open();
        master.proposed = effects();
        let original = master.proposed.clone();
        master
            .command(&["automation".into(), "gain".into()], &metadata())
            .unwrap();
        master
            .command(&["point".into(), "48000".into(), "6".into()], &metadata())
            .unwrap();
        assert!(!master.command(&["commit".into()], &metadata()).unwrap());
        assert!(master.automation.is_none());
        assert_eq!(
            master.proposed[0].parameter_automation.as_ref().unwrap()["gain"].len(),
            3
        );
        master.restore(false);
        assert_eq!(master.proposed, original);
        master.restore(true);
        assert_eq!(
            master.proposed[0].parameter_automation.as_ref().unwrap()["gain"].len(),
            3
        );
    }
    #[test]
    fn silent_or_malformed_measurements_and_invalid_parameters_do_not_apply() {
        let mut master = open();
        master.proposed = effects();
        master.report.as_mut().unwrap().true_peak_db = -80.;
        assert!(master.use_proposal(&snapshot(), &metadata()).is_err());
        for bad in ["NaN", "25", "-25"] {
            assert!(master
                .command(
                    &["parameter".into(), "gain".into(), bad.into()],
                    &metadata()
                )
                .is_err());
        }
        let mut analysis = master.report.unwrap();
        analysis.sections[0].end = 100000;
        assert!(validate_analysis(&analysis).is_err());
        let mut invalid = effects();
        invalid[0]
            .parameters
            .as_mut()
            .unwrap()
            .insert("unknown".into(), 1.);
        assert!(validate_rack(&invalid, &metadata()).is_err());
    }
    #[test]
    fn chart_cache_survives_transport_redraw_and_rebuilds_only_for_view_changes() {
        let mut master = open();
        master.draw(&mut Canvas::new(100, 30), &metadata(), false);
        let ptr = master.chart.cells.as_ptr();
        master.draw(&mut Canvas::new(100, 30), &metadata(), false);
        assert_eq!(ptr, master.chart.cells.as_ptr());
        assert!(master.chart.cells.iter().any(|(_, _, ch, _)| *ch == '+'));
        master.key(Key::Char('+'), &metadata()).unwrap();
        assert!(master.chart_dirty);
        master.draw(&mut Canvas::new(100, 30), &metadata(), false);
        assert!(!master.chart_dirty);
        master.draw(&mut Canvas::new(20, 8), &metadata(), false);
    }
}
