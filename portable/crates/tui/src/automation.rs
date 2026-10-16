//! A client-private curve draft. Authoritative edits go through guarded controls.
use crate::{input::Key, render::Canvas};
use soundroom_client::models::*;
use std::collections::BTreeMap;
#[derive(Clone)]
pub enum Target {
    Volume,
    Pan,
    Amount(usize),
    Parameter(usize, String, RemoteParameterMetadata),
}
#[derive(Clone)]
struct Draft {
    points: Vec<AutomationPoint>,
    scale: Option<AutomationValueScale>,
}
pub struct Automation {
    pub track_id: String,
    pub revision: i64,
    pub target: Target,
    pub points: Vec<AutomationPoint>,
    pub selected: usize,
    pub cursor: i64,
    pub value: f32,
    pub start: i64,
    pub span: i64,
    pub step: i64,
    pub status: String,
    baseline: f32,
    decibels: bool,
    controls: RemoteTrackControls,
    undo: Vec<Draft>,
    redo: Vec<Draft>,
    columns: usize,
    rows: usize,
    sample_rate: f64,
    bpm: f64,
}
pub fn controls(track: &Track) -> Result<RemoteTrackControls, String> {
    Ok(RemoteTrackControls {
        track_id: track.id.clone().ok_or("Missing track ID.")?,
        name: track.name.clone().unwrap_or_default(),
        color: track.color.unwrap_or(0),
        gain: track.gain.unwrap_or(1.),
        pan: track.pan.unwrap_or(0.),
        muted: track.muted.unwrap_or(false),
        solo: track.solo.unwrap_or(false),
        armed: track.armed.unwrap_or(false),
        input_device_uid: track.input_device_uid.clone(),
        input_device_name: track.input_device_name.clone(),
        input_channel: track.input_channel.unwrap_or(0),
        stereo_input: track.stereo_input.unwrap_or(false),
        volume_automation_scale: track.volume_automation_scale.clone(),
        mix_automation: track.mix_automation.clone().unwrap_or_default(),
        routing: track.routing.clone(),
        realtime_effects: track.realtime_effects.clone().unwrap_or_default(),
    })
}
impl Automation {
    pub fn open(
        track: &Track,
        revision: i64,
        target: Target,
        cursor: i64,
        sample_rate: f64,
        bpm: f64,
    ) -> Result<Self, String> {
        let controls = controls(track)?;
        let (points, baseline) = match &target {
            Target::Volume => (
                controls
                    .mix_automation
                    .get("volume")
                    .cloned()
                    .unwrap_or_default(),
                (controls.gain / 1.5).clamp(0., 1.),
            ),
            Target::Pan => (
                controls
                    .mix_automation
                    .get("pan")
                    .cloned()
                    .unwrap_or_default(),
                (controls.pan + 1.) / 2.,
            ),
            Target::Amount(i) => {
                let e = controls
                    .realtime_effects
                    .get(*i)
                    .ok_or("Select an insert.")?;
                (
                    e.automation.clone().unwrap_or_default(),
                    e.amount.unwrap_or(0.5),
                )
            }
            Target::Parameter(i, id, meta) => {
                let e = controls
                    .realtime_effects
                    .get(*i)
                    .ok_or("Select an insert.")?;
                let value = e
                    .parameters
                    .as_ref()
                    .and_then(|p| p.get(id))
                    .copied()
                    .unwrap_or(meta.default_value);
                (
                    e.parameter_automation
                        .as_ref()
                        .and_then(|p| p.get(id))
                        .cloned()
                        .unwrap_or_default(),
                    normalize(value, meta),
                )
            }
        };
        let step = (sample_rate * 60. / bpm / 4.).round().max(1.) as i64;
        let span = (sample_rate * 60. / bpm * 16.).round().max(1.) as i64;
        if points.len() > 4096 {
            return Err("This lane exceeds the 4096-point terminal editing limit; use the native or browser editor.".into());
        }
        let mut points = points;
        points.sort_by_key(|p| p.frame);
        let decibels = matches!(target, Target::Volume)
            && controls.volume_automation_scale == Some(AutomationValueScale::Decibels);
        Ok(Self{track_id:controls.track_id.clone(),revision,target,points,selected:0,cursor,value:baseline,start:cursor.saturating_sub(span/4).max(0),span,step,status:"Private curve draft; Enter or :commit applies with the opening track revision. Esc closes.".into(),baseline,decibels,controls,undo:vec![],redo:vec![],columns:65,rows:12,sample_rate,bpm})
    }
    fn capture(&mut self) {
        if self.undo.len() == 100 {
            self.undo.remove(0);
        }
        self.undo.push(Draft {
            points: self.points.clone(),
            scale: self.controls.volume_automation_scale.clone(),
        });
        self.redo.clear();
        // 4096 points × 100 states is at most ~16 MiB of point storage.
    }
    fn restore(&mut self, redo: bool) {
        let old = if redo {
            self.redo.pop()
        } else {
            self.undo.pop()
        };
        if let Some(p) = old {
            let current = Draft {
                points: std::mem::replace(&mut self.points, p.points),
                scale: std::mem::replace(&mut self.controls.volume_automation_scale, p.scale),
            };
            self.decibels = matches!(self.target, Target::Volume)
                && self.controls.volume_automation_scale == Some(AutomationValueScale::Decibels);
            if redo {
                self.undo.push(current)
            } else {
                self.redo.push(current)
            }
            self.selected = self.selected.min(self.points.len().saturating_sub(1));
        }
    }
    fn add(&mut self, frame: i64, value: f32) -> Result<(), String> {
        if !(0..=17_280_000_000).contains(&frame)
            || !value.is_finite()
            || !(0. ..=1.).contains(&value)
        {
            return Err(
                "Point requires nonnegative frame and a value within the lane range.".into(),
            );
        }
        if self.points.len() >= 4096 && !self.points.iter().any(|p| p.frame == frame) {
            return Err("A lane supports 4096 points.".into());
        }
        self.capture();
        self.points.retain(|p| p.frame != frame);
        self.points.push(AutomationPoint {
            frame,
            value,
            curve: Some(AutomationCurve::Smooth),
            curve_start: Some(0.),
            curve_end: Some(1.),
        });
        self.points.sort_by_key(|p| p.frame);
        self.selected = self.points.iter().position(|p| p.frame == frame).unwrap();
        self.cursor = frame;
        self.value = value;
        Ok(())
    }
    fn selected_move(&mut self, frame: i64, value: f32) {
        self.capture();
        let frame = frame.clamp(0, 17_280_000_000);
        if self.selected < self.points.len() {
            let mut p = self.points.remove(self.selected);
            p.frame = frame;
            p.value = value.clamp(0., 1.);
            self.points.retain(|p| p.frame != frame);
            self.points.push(p);
            self.points.sort_by_key(|p| p.frame);
            self.selected = self.points.iter().position(|p| p.frame == frame).unwrap();
        }
        self.cursor = frame;
        self.value = value.clamp(0., 1.);
    }
    pub fn key(&mut self, key: Key) -> Result<bool, String> {
        match key {
            Key::Enter => return Ok(true),
            Key::Char('u') => self.restore(false),
            Key::Ctrl('r') => self.restore(true),
            Key::Char('n') => self.add(self.cursor, self.value)?,
            Key::Delete | Key::Char('d') => {
                if self.selected < self.points.len() {
                    self.capture();
                    self.points.remove(self.selected);
                    self.selected = self.selected.min(self.points.len().saturating_sub(1));
                }
            }
            Key::Char('J') => {
                self.selected = (self.selected + 1).min(self.points.len().saturating_sub(1))
            }
            Key::Char('K') => self.selected = self.selected.saturating_sub(1),
            Key::Char('h')
            | Key::Left
            | Key::Char('l')
            | Key::Right
            | Key::Char('j')
            | Key::Down
            | Key::Char('k')
            | Key::Up => {
                let (f, v) = self
                    .points
                    .get(self.selected)
                    .map_or((self.cursor, self.value), |p| (p.frame, p.value));
                let (f, v) = match key {
                    Key::Char('h') | Key::Left => (f.saturating_sub(self.step), v),
                    Key::Char('l') | Key::Right => (f.saturating_add(self.step), v),
                    Key::Char('j') | Key::Down => (f, (v - 0.01).max(0.)),
                    _ => (f, (v + 0.01).min(1.)),
                };
                self.selected_move(f, v);
            }
            Key::Char('+') | Key::Char('=') => {
                self.span = (self.span as f64 / 1.25).round().max(1.) as i64
            }
            Key::Char('-') => {
                self.span = (self.span as f64 * 1.25).round().min(17_280_000_000.) as i64
            }
            Key::Char('H') => self.start = self.start.saturating_sub(self.span / 4).max(0),
            Key::Char('L') => {
                self.start = self
                    .start
                    .saturating_add(self.span / 4)
                    .min(17_280_000_000 - self.span)
            }
            Key::Mouse {
                button: 0,
                column,
                row,
                released: false,
            } if column >= 10 && row >= 6 && usize::from(row) < 6 + self.rows => {
                let frame = self.start
                    + ((usize::from(column - 10).min(self.columns) as f64 / self.columns as f64)
                        * self.span as f64)
                        .round() as i64;
                let value = (1. - f64::from(row - 6) / self.rows.saturating_sub(1).max(1) as f64)
                    .clamp(0., 1.) as f32;
                self.add(frame, value)?;
            }
            _ => {}
        }
        Ok(false)
    }
    pub fn command(&mut self, words: &[String]) -> Result<bool, String> {
        let arg = |i: usize| words.get(i).ok_or("Missing argument.");
        let number = |i: usize| {
            arg(i)?
                .parse::<f64>()
                .map_err(|_| "Expected a finite number.")
        };
        match words[0].as_str() {
            "commit"=>return Ok(true),
            "point"=>{let frame=self.position_input(arg(1)?)?;self.add(frame,self.normalized_input(number(2)?)?)?;},
            "value"=>{let v=self.normalized_input(number(1)?)?;let frame=self.points.get(self.selected).map_or(self.cursor,|p|p.frame);self.selected_move(frame,v);},
            "curve"|"smooth"=>{let curve:AutomationCurve=if words[0]=="smooth"{AutomationCurve::Smooth}else{serde_json::from_value(serde_json::json!(arg(1)?)).map_err(|_|"Curve: linear, smooth, easeIn, easeOut, hold.")?};self.capture();if words[0]=="smooth"{for p in &mut self.points{p.curve=Some(curve.clone());p.curve_start=Some(0.);p.curve_end=Some(1.);}}else if let Some(p)=self.points.get_mut(self.selected){p.curve=Some(curve);p.curve_start=Some(0.);p.curve_end=Some(1.);}},
            "undo"=>self.restore(false),"redo"=>self.restore(true),
            "clear"=>{self.capture();self.points.clear();self.selected=0;},
            "scale" if matches!(self.target,Target::Volume)=>{let next=Some(match arg(1)?.as_str(){"decibels"=>AutomationValueScale::Decibels,"linear"=>AutomationValueScale::Linear,_=>return Err("Scale: decibels or linear.".into())});self.capture();self.controls.volume_automation_scale=next;self.decibels=self.controls.volume_automation_scale==Some(AutomationValueScale::Decibels);},
            _=>return Err("Automation: :point frame value; :value value; :curve name; :smooth; :scale decibels|linear; :clear; :commit.".into())
        }
        Ok(false)
    }
    fn position_input(&self, text: &str) -> Result<i64, String> {
        let (unit, number) = text.split_once(':').unwrap_or(("frame", text));
        let n = number
            .parse::<f64>()
            .map_err(|_| "Position: frame, beats:number or seconds:number.")?;
        let frame = match unit {
            "frame" if n.fract() == 0. => n,
            "beats" => n * self.sample_rate * 60. / self.bpm,
            "seconds" => n * self.sample_rate,
            _ => return Err("Position: integer frame, beats:number or seconds:number.".into()),
        };
        if !frame.is_finite() || !(0. ..=17_280_000_000.).contains(&frame) {
            return Err("Position is outside the timeline.".into());
        }
        Ok(frame.round() as i64)
    }
    fn normalized_input(&self, value: f64) -> Result<f32, String> {
        if !value.is_finite() {
            return Err("Value must be finite.".into());
        }
        let (min, max, v) = match &self.target {
            Target::Volume => (-80., 20. * 1.5_f64.log10(), 10_f64.powf(value / 20.) / 1.5),
            Target::Pan => (-1., 1., (value + 1.) / 2.),
            Target::Amount(_) => (0., 1., value),
            Target::Parameter(_, _, p) => {
                let scale = if p.unit == "%" { 100. } else { 1. };
                (
                    f64::from(p.minimum) * scale,
                    f64::from(p.maximum) * scale,
                    f64::from(normalize((value / scale) as f32, p)),
                )
            }
        };
        if value < min || value > max {
            return Err(format!("Value range: {min}..{max}."));
        }
        Ok(v as f32)
    }
    pub fn request(&self) -> RemoteCommand {
        let mut c = self.controls.clone();
        match &self.target {
            Target::Volume => {
                c.mix_automation
                    .insert("volume".into(), self.points.clone());
            }
            Target::Pan => {
                c.mix_automation.insert("pan".into(), self.points.clone());
            }
            Target::Amount(i) => c.realtime_effects[*i].automation = Some(self.points.clone()),
            Target::Parameter(i, id, _) => {
                c.realtime_effects[*i]
                    .parameter_automation
                    .get_or_insert_with(BTreeMap::new)
                    .insert(id.clone(), self.points.clone());
            }
        }
        RemoteCommand::UpdateTrackControls {
            controls: c,
            name: "Automation curve".into(),
            gesture_id: None,
        }
    }
    fn label(&self, value: f32) -> String {
        match &self.target {
            Target::Volume => format!("{:.2} dB", 20. * (value * 1.5).max(0.0001).log10()),
            Target::Pan => format!("{:.3} pan", value * 2. - 1.),
            Target::Amount(_) => format!("{:.3}", value),
            Target::Parameter(_, _, m) => format!(
                "{:.3} {}",
                physical(value, m) * if m.unit == "%" { 100. } else { 1. },
                m.unit
            ),
        }
    }
    pub fn draw(&mut self, c: &mut Canvas) {
        for y in 2..c.height.saturating_sub(3) {
            c.fill(0, y, c.width, ' ', 252, 234);
        }
        let title = match &self.target {
            Target::Volume => "Volume",
            Target::Pan => "Pan",
            Target::Amount(_) => "Insert amount",
            Target::Parameter(_, _, m) => &m.name,
        };
        c.text(
            1,
            2,
            &format!(
                "Automation / {title}  original track revision {}",
                self.revision
            ),
            81,
            234,
            true,
        );
        c.text(1,3,"n add; J/K point; h/l time; j/k value; +/- zoom; H/L scroll; Enter applies; Esc closes",245,234,false);
        self.columns = c.width.saturating_sub(11).max(1);
        self.rows = c.height.saturating_sub(12).clamp(1, 25);
        for row in 0..self.rows {
            let value = 1. - row as f32 / self.rows.saturating_sub(1).max(1) as f32;
            c.text(0, 6 + row, &self.label(value), 245, 234, false);
            c.fill(
                10,
                6 + row,
                self.columns,
                if row == self.rows / 2 { '-' } else { '.' },
                238,
                234,
            );
        }
        for x in 0..self.columns {
            let frame =
                self.start + (x as f64 / self.columns as f64 * self.span as f64).round() as i64;
            let value = sample(&self.points, frame, self.baseline, self.decibels);
            let y =
                ((1. - value.clamp(0., 1.)) * self.rows.saturating_sub(1) as f32).round() as usize;
            c.put(10 + x, 6 + y, '*', 81, 234, false);
        }
        for (i, p) in self
            .points
            .iter()
            .enumerate()
            .filter(|(_, p)| p.frame >= self.start && p.frame <= self.start + self.span)
        {
            let x = ((p.frame - self.start) as f64 / self.span as f64 * self.columns as f64).round()
                as usize;
            let y = ((1. - p.value) * self.rows.saturating_sub(1) as f32).round() as usize;
            c.put(
                10 + x,
                6 + y,
                if i == self.selected { '@' } else { 'o' },
                if i == self.selected { 231 } else { 203 },
                234,
                true,
            );
        }
        let (frame, value) = self
            .points
            .get(self.selected)
            .map_or((self.cursor, self.value), |p| (p.frame, p.value));
        c.text(
            1,
            c.height.saturating_sub(6),
            &format!(
                "Point {}/{} frame {}  {}   view {}..{}",
                self.selected + 1,
                self.points.len(),
                frame,
                self.label(value),
                self.start,
                self.start + self.span
            ),
            252,
            234,
            false,
        );
        c.text(
            1,
            c.height.saturating_sub(5),
            ":point beats:4.25 value; seconds:2.5 or integer frame; :smooth; :commit",
            245,
            234,
            false,
        );
        c.text(1, c.height.saturating_sub(4), &self.status, 245, 234, false);
    }
}
/// Upgrade legacy amount-only inserts before their physical parameter lanes are edited.
pub fn editable_effect(source: &RealtimeEffect, meta: &RemoteEffectMetadata) -> RealtimeEffect {
    let mut e = source.clone();
    if e.parameters.as_ref().is_some_and(|p| !p.is_empty()) {
        return e;
    }
    let amount = e.amount.unwrap_or(0.5).clamp(0., 1.);
    let (primary, physical): (&str, fn(f32) -> f32) = match e.kind {
        RealtimeEffectKind::Gain => ("gain", |v| v * 48. - 24.),
        RealtimeEffectKind::LowPass => ("frequency", |v| 200. * 80_f32.powf(1. - v)),
        RealtimeEffectKind::Echo => ("mix", |v| v * 0.65),
        RealtimeEffectKind::Warmth => ("drive", |v| 20. * (1. + v * 8.).log10()),
        _ => ("", |_| 0.),
    };
    let mut values: BTreeMap<_, _> = meta
        .parameters
        .iter()
        .map(|p| (p.id.clone(), p.default_value))
        .collect();
    if !primary.is_empty() {
        values.insert(primary.into(), physical(amount));
    }
    if e.kind == RealtimeEffectKind::Echo {
        values.insert("time".into(), 250.);
        values.insert("feedback".into(), 0.4);
        values.insert("damping".into(), 20000.);
    }
    if e.kind == RealtimeEffectKind::Warmth {
        values.insert("tone".into(), 20000.);
    }
    if e.kind == RealtimeEffectKind::Room {
        values.insert("character".into(), 0.);
    }
    e.parameters = Some(values);
    if let Some(p) = meta.parameters.iter().find(|p| p.id == primary) {
        let lanes = e.parameter_automation.get_or_insert_with(BTreeMap::new);
        if lanes.get(primary).map_or(true, |ps| ps.is_empty()) {
            lanes.insert(
                primary.into(),
                e.automation
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .map(|point| AutomationPoint {
                        value: normalize(physical(point.value), p),
                        ..point.clone()
                    })
                    .collect(),
            );
        }
    }
    e
}
pub fn normalize(v: f32, p: &RemoteParameterMetadata) -> f32 {
    let min = f64::from(p.minimum);
    let max = f64::from(p.maximum);
    if max <= min {
        return 0.;
    }
    let v = f64::from(v).clamp(min, max);
    (if p.logarithmic && min > 0. {
        (v / min).ln() / (max / min).ln()
    } else {
        (v - min) / (max - min)
    }) as f32
}
pub fn physical(v: f32, p: &RemoteParameterMetadata) -> f64 {
    let min = f64::from(p.minimum);
    let max = f64::from(p.maximum);
    let v = f64::from(v.clamp(0., 1.));
    let value = if p.logarithmic && min > 0. {
        min * (max / min).powf(v)
    } else {
        min + (max - min) * v
    };
    if p.integer {
        value.round()
    } else {
        value
    }
}
pub fn fraction(t: f64, curve: &AutomationCurve) -> f64 {
    let t = t.clamp(0., 1.);
    match curve {
        AutomationCurve::Linear => t,
        AutomationCurve::Smooth => t * t * (3. - 2. * t),
        AutomationCurve::EaseIn => t * t,
        AutomationCurve::EaseOut => 1. - (1. - t) * (1. - t),
        AutomationCurve::Hold => {
            if t < 1. {
                0.
            } else {
                1.
            }
        }
    }
}
pub fn sample(points: &[AutomationPoint], frame: i64, baseline: f32, decibels: bool) -> f32 {
    let next = points.partition_point(|p| p.frame <= frame);
    let Some(a) = next.checked_sub(1).and_then(|i| points.get(i)) else {
        return baseline;
    };
    let Some(b) = points.get(next) else {
        return a.value;
    };
    let raw = (frame - a.frame) as f64 / (b.frame - a.frame).max(1) as f64;
    let curve = a.curve.as_ref().unwrap_or(&AutomationCurve::Linear);
    let start = f64::from(a.curve_start.unwrap_or(0.));
    let end = f64::from(a.curve_end.unwrap_or(1.));
    let fa = fraction(start, curve);
    let fb = fraction(end, curve);
    let f = if (fb - fa).abs() > 1e-6 {
        (fraction(start + (end - start) * raw, curve) - fa) / (fb - fa)
    } else {
        raw
    };
    if decibels && f > 0. && f < 1. {
        let a = f64::from(a.value * 1.5).max(0.0001).log10() * 20.;
        let b = f64::from(b.value * 1.5).max(0.0001).log10() * 20.;
        (10_f64.powf((a + (b - a) * f) / 20.) / 1.5).clamp(0., 1.) as f32
    } else {
        (f64::from(a.value) + (f64::from(b.value) - f64::from(a.value)) * f) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn track() -> Track {
        serde_json::from_value(serde_json::json!({"id":"00000000-0000-0000-0000-000000000001","name":"Voice","gain":1,"pan":0,"mixAutomation":{"pan":[{"frame":10,"value":0.2},{"frame":100,"value":0.8}]},"realtimeEffects":[{"id":"00000000-0000-0000-0000-000000000002","kind":"echo","amount":0.5,"enabled":true,"parameters":{"feedback":0.4}}]})).unwrap()
    }
    fn open(target: Target) -> Automation {
        Automation::open(&track(), 9, target, 0, 48000., 120.).unwrap()
    }
    fn point(frame: i64, value: f32, curve: AutomationCurve) -> AutomationPoint {
        AutomationPoint {
            frame,
            value,
            curve: Some(curve),
            curve_start: Some(0.),
            curve_end: Some(1.),
        }
    }
    #[test]
    fn interpolation_scale_is_in_private_undo_history() {
        let mut a = open(Target::Volume);
        a.command(&["scale".into(), "decibels".into()]).unwrap();
        assert!(a.decibels);
        a.restore(false);
        assert!(!a.decibels);
        a.restore(true);
        assert!(a.decibels);
    }
    #[test]
    fn legacy_echo_upgrade_preserves_legacy_mix_and_its_curve() {
        let source:RealtimeEffect=serde_json::from_value(serde_json::json!({"kind":"echo","amount":0.4,"automation":[{"frame":0,"value":0.2,"curve":"smooth"},{"frame":100,"value":0.8}]})).unwrap();
        let parameter = |id: &str, min: f32, max: f32, default: f32| {
            serde_json::from_value(serde_json::json!({"id":id,"name":id,"minimum":min,"maximum":max,"defaultValue":default,"unit":"","logarithmic":false,"integer":false,"automationUnit":"normalized"})).unwrap()
        };
        let meta = RemoteEffectMetadata {
            id: "echo".into(),
            name: "Echo".into(),
            parameters: vec![
                parameter("mix", 0., 1., 0.5),
                parameter("time", 1., 2000., 300.),
                parameter("feedback", 0., 0.95, 0.3),
                parameter("damping", 20., 20000., 8000.),
            ],
            factory_presets: vec![],
            modes: vec![],
        };
        let upgraded = editable_effect(&source, &meta);
        let values = upgraded.parameters.as_ref().unwrap();
        assert!((values["mix"] - 0.26).abs() < 1e-6);
        assert_eq!(values["time"], 250.);
        assert_eq!(values["feedback"], 0.4);
        let lane = &upgraded.parameter_automation.as_ref().unwrap()["mix"];
        assert!((lane[0].value - 0.13).abs() < 1e-6);
        assert_eq!(lane[0].curve, Some(AutomationCurve::Smooth));
        assert_eq!(editable_effect(&upgraded, &meta), upgraded);
    }
    #[test]
    fn positions_use_exact_frames_seconds_or_quarter_beats() {
        let a = open(Target::Volume);
        assert_eq!(a.position_input("beats:4.25").unwrap(), 102000);
        assert_eq!(a.position_input("seconds:2.5").unwrap(), 120000);
        assert_eq!(a.position_input("12345").unwrap(), 12345);
        assert!(a.position_input("frame:0.5").is_err());
        assert!(a.position_input("beats:NaN").is_err());
    }
    #[test]
    fn curves_follow_native_domain_and_split_endpoints() {
        for c in [
            AutomationCurve::Linear,
            AutomationCurve::Smooth,
            AutomationCurve::EaseIn,
            AutomationCurve::EaseOut,
            AutomationCurve::Hold,
        ] {
            let ps = vec![
                point(0, 0.1, c.clone()),
                point(100, 0.9, AutomationCurve::Linear),
            ];
            assert!(
                (sample(&ps, 25, 0., false) - (0.1 + 0.8 * fraction(0.25, &c) as f32)).abs() < 1e-6
            );
            assert_eq!(sample(&ps, -1, 0.4, false), 0.4);
            assert_eq!(sample(&ps, 101, 0., false), 0.9);
        }
        let mut p = point(20, 0.3, AutomationCurve::Smooth);
        p.curve_start = Some(0.2);
        p.curve_end = Some(0.7);
        let ps = vec![p, point(70, 0.7, AutomationCurve::Linear)];
        let expected = 0.3
            + 0.4
                * ((fraction(0.45, &AutomationCurve::Smooth)
                    - fraction(0.2, &AutomationCurve::Smooth))
                    / (fraction(0.7, &AutomationCurve::Smooth)
                        - fraction(0.2, &AutomationCurve::Smooth))) as f32;
        assert!((sample(&ps, 45, 0., false) - expected).abs() < 1e-6);
    }
    #[test]
    fn volume_decibel_interpolation_is_geometric_and_silence_finite() {
        let ps = vec![
            point(0, 1., AutomationCurve::Linear),
            point(100, 0.01, AutomationCurve::Linear),
        ];
        assert!((sample(&ps, 50, 0., true) - 0.1).abs() < 1e-6);
        assert!((sample(&ps, 50, 0., false) - 0.505).abs() < 1e-6);
        let ps = vec![
            point(0, 1., AutomationCurve::Smooth),
            point(100, 0., AutomationCurve::Linear),
        ];
        assert!(sample(&ps, 99, 0., true).is_finite());
        assert_eq!(sample(&ps, 100, 0., true), 0.);
    }
    #[test]
    fn points_are_sorted_unique_and_history_is_local() {
        let mut a = open(Target::Volume);
        a.add(100, 0.7).unwrap();
        a.add(0, 0.2).unwrap();
        a.add(100, 0.4).unwrap();
        assert_eq!(
            a.points.iter().map(|p| p.frame).collect::<Vec<_>>(),
            vec![0, 100]
        );
        a.restore(false);
        assert_eq!(a.points[1].value, 0.7);
        a.restore(true);
        assert_eq!(a.points[1].value, 0.4);
        a.key(Key::Char('h')).unwrap();
        assert_eq!(a.points[a.selected].frame, 0);
        assert_eq!(a.points.len(), 1);
    }
    #[test]
    fn controls_preserve_other_lanes_routing_and_rack() {
        let mut a = open(Target::Volume);
        a.add(20, 0.5).unwrap();
        let RemoteCommand::UpdateTrackControls { controls, .. } = a.request() else {
            panic!()
        };
        assert_eq!(controls.mix_automation["volume"].len(), 1);
        assert_eq!(controls.mix_automation["pan"].len(), 2);
        assert_eq!(
            controls.realtime_effects[0].parameters.as_ref().unwrap()["feedback"],
            0.4
        );
        assert_eq!(a.revision, 9);
    }
    #[test]
    fn numeric_values_are_physical_and_reject_nonfinite() {
        let mut a = open(Target::Volume);
        a.command(&["point".into(), "100".into(), "-6".into()])
            .unwrap();
        assert!((a.points[0].value - (10_f32.powf(-6. / 20.) / 1.5)).abs() < 1e-6);
        assert!(a.command(&["value".into(), "NaN".into()]).is_err());
        let p:RemoteParameterMetadata=serde_json::from_value(serde_json::json!({"id":"time","name":"Time","minimum":20,"maximum":2000,"defaultValue":200,"unit":"ms","logarithmic":true,"integer":false,"automationUnit":"normalized"})).unwrap();
        assert!((normalize(200., &p) - 0.5).abs() < 1e-6);
        assert!((physical(0.5, &p) - 200.).abs() < 1e-6);
    }
    #[test]
    fn chart_is_bounded_and_mouse_uses_visible_time_value() {
        let mut a = open(Target::Pan);
        let mut canvas = Canvas::new(40, 24);
        a.draw(&mut canvas);
        let span = a.span;
        a.key(Key::Mouse {
            button: 0,
            column: 10,
            row: 6,
            released: false,
        })
        .unwrap();
        assert_eq!(a.points[0].value, 1.);
        assert_eq!(a.points[0].frame, 0);
        a.key(Key::Char('+')).unwrap();
        assert!(a.span < span);
        a.draw(&mut Canvas::new(20, 8));
    }
}
