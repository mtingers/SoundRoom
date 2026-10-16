//! Client-private creative drafts; rendering and live voices belong to the host.
use crate::{input::Key, render::Canvas};
use base64::Engine;
use soundroom_client::{auth::new_id, models::*};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Designer,
    Beats,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Focus {
    Notes,
    Sound,
    Keyboard,
}
#[derive(Clone)]
enum Draft {
    Designer(Box<SoundDesignPattern>),
    Beats(DrumPattern),
}
impl Draft {
    fn bytes(&self) -> usize {
        match self {
            Self::Designer(p) => {
                512 + p.notes.as_ref().map_or(0, |ns| ns.len() * 160)
                    + p.controllers.as_ref().map_or(0, |ps| ps.len() * 64)
                    + p.layers.as_ref().map_or(0, |ls| ls.len() * 512)
            }
            Self::Beats(p) => 512 + p.steps.len() * 96 + p.lanes.len() * 192,
        }
    }
}
#[derive(Default)]
struct NoteIndex {
    pitches: BTreeMap<i64, Vec<(f64, f64, usize)>>,
    ends: BTreeMap<i64, Vec<f64>>,
}
impl NoteIndex {
    fn new(notes: &[SoundDesignNote]) -> Self {
        let mut index = Self::default();
        for (i, n) in notes.iter().enumerate() {
            index.pitches.entry(n.pitch).or_default().push((
                n.start_beat,
                n.start_beat + n.duration_beats,
                i,
            ));
        }
        for (pitch, ns) in index.pitches.iter_mut() {
            ns.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)));
            let mut end = f64::NEG_INFINITY;
            index.ends.insert(
                *pitch,
                ns.iter()
                    .map(|n| {
                        end = end.max(n.1);
                        end
                    })
                    .collect(),
            );
        }
        index
    }
    fn visible(&self, pitch: i64, start: f64, end: f64) -> impl Iterator<Item = usize> + '_ {
        let notes = self
            .pitches
            .get(&pitch)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let first = self
            .ends
            .get(&pitch)
            .map_or(0, |es| es.partition_point(|e| *e <= start));
        notes[first..]
            .iter()
            .take_while(move |n| n.0 < end)
            .filter(move |n| n.1 > start)
            .map(|n| n.2)
    }
}
pub struct Creative {
    pub kind: Kind,
    pub sound: Option<SoundDesignPattern>,
    pub drums: Option<DrumPattern>,
    pub focus: Focus,
    pub playing: bool,
    pub preset: usize,
    pub selected: usize,
    pub parameter: usize,
    pub voice: usize,
    pub step: usize,
    pub cursor: f64,
    pub pitch: i64,
    pub octave: i64,
    pub zoom: f64,
    pub scroll: f64,
    pub status: String,
    pub record: bool,
    undo: Vec<Draft>,
    redo: Vec<Draft>,
    offs: BTreeMap<i64, Instant>,
    gesture_dirty: bool,
    viewport_columns: usize,
    catalog: Vec<String>,
    catalog_scroll: usize,
    note_index: NoteIndex,
    index_dirty: bool,
    preview_id: Option<String>,
    workspace_id: String,
    epoch: u64,
    pending_file: Option<String>,
    midi_file: Option<std::sync::Arc<crate::midi::MIDIFile>>,
}
impl Creative {
    pub fn open(
        kind: Kind,
        cap: &RemoteCapabilities,
        clip: Option<&Clip>,
        bpm: f64,
    ) -> Result<Self, String> {
        let sound = if kind == Kind::Designer {
            Some(
                clip.and_then(|c| c.sound_design.clone())
                    .or_else(|| {
                        cap.designer_presets.first().map(|p| {
                            let mut s = p.sound.clone();
                            s.bpm = Some(bpm);
                            s
                        })
                    })
                    .ok_or("This host does not advertise Designer presets.")?,
            )
        } else {
            None
        };
        let drums = if kind == Kind::Beats {
            Some(
                clip.and_then(|c| c.drum_pattern.clone())
                    .or_else(|| {
                        cap.drum_presets.first().map(|p| {
                            let mut d = p.pattern.clone();
                            d.bpm = bpm;
                            d
                        })
                    })
                    .ok_or("This host does not advertise drum presets.")?,
            )
        } else {
            None
        };
        let note_index = NoteIndex::new(
            sound
                .as_ref()
                .and_then(|p| p.notes.as_deref())
                .unwrap_or_default(),
        );
        Ok(Self {
            kind,
            sound,
            drums,
            focus: Focus::Notes,
            playing: false,
            preset: 0,
            selected: 0,
            parameter: 0,
            voice: 0,
            step: 0,
            cursor: 0.,
            pitch: 60,
            octave: 4,
            zoom: 1.,
            scroll: 0.,
            status: "Private draft. Space auditions this workspace; :insert renders to timeline."
                .into(),
            record: false,
            undo: vec![],
            redo: vec![],
            offs: BTreeMap::new(),
            gesture_dirty: false,
            viewport_columns: 65,
            catalog: vec![],
            catalog_scroll: 0,
            note_index,
            index_dirty: false,
            preview_id: None,
            workspace_id: new_id().map_err(|e| e.to_string())?,
            epoch: 0,
            pending_file: None,
            midi_file: None,
        })
    }
    fn trim_history(&mut self) {
        while self
            .undo
            .iter()
            .chain(self.redo.iter())
            .map(Draft::bytes)
            .sum::<usize>()
            > 32 * 1024 * 1024
        {
            if !self.undo.is_empty() {
                self.undo.remove(0);
            } else if !self.redo.is_empty() {
                self.redo.remove(0);
            } else {
                break;
            }
        }
    }
    fn capture(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.index_dirty = true;
        let draft = match self.kind {
            Kind::Designer => self.sound.clone().map(|p| Draft::Designer(Box::new(p))),
            Kind::Beats => self.drums.clone().map(Draft::Beats),
        };
        if let Some(draft) = draft {
            if self.undo.len() == 100 {
                self.undo.remove(0);
            }
            self.undo.push(draft);
            self.redo.clear();
            self.trim_history();
        }
    }
    fn restore(&mut self, redo: bool) {
        let current = match self.kind {
            Kind::Designer => self.sound.clone().map(|p| Draft::Designer(Box::new(p))),
            Kind::Beats => self.drums.clone().map(Draft::Beats),
        };
        let previous = if redo {
            self.redo.pop()
        } else {
            self.undo.pop()
        };
        if let (Some(current), Some(previous)) = (current, previous) {
            if redo {
                self.undo.push(current);
            } else {
                self.redo.push(current);
            }
            match previous {
                Draft::Designer(p) => self.sound = Some(*p),
                Draft::Beats(p) => self.drums = Some(p),
            }
            self.epoch = self.epoch.wrapping_add(1);
            self.gesture_dirty = true;
            self.index_dirty = true;
            self.trim_history();
        }
    }
    fn payload(&self) -> Result<String, String> {
        let value = match self.kind {
            Kind::Designer => {
                serde_json::to_vec(self.sound.as_ref().ok_or("Missing Designer draft.")?)
            }
            Kind::Beats => serde_json::to_vec(self.drums.as_ref().ok_or("Missing drum draft.")?),
        }
        .map_err(|e| e.to_string())?;
        Ok(base64::engine::general_purpose::STANDARD.encode(value))
    }
    pub fn request(
        &self,
        operation: &str,
        context: RemoteEditorContext,
    ) -> Result<RemoteCommand, String> {
        let payload = if matches!(
            operation,
            "designer.preview"
                | "designer.configure"
                | "designer.commit"
                | "designer.noteOn"
                | "drums.preview"
                | "drums.commit"
                | "drums.export"
        ) {
            Some(self.payload()?)
        } else {
            None
        };
        let mut strings = BTreeMap::new();
        if operation == "audition.stop" {
            strings.insert(
                "kind".into(),
                if self.kind == Kind::Designer {
                    "designer"
                } else {
                    "drums"
                }
                .into(),
            );
        }
        if let Some(id) = context.track_id.clone() {
            strings.insert("trackID".into(), id);
        }
        if let Some(id) = context.clip_id.clone() {
            strings.insert("clipID".into(), id);
        }
        Ok(RemoteCommand::Editor {
            intent: RemoteEditorIntent {
                operation: operation.into(),
                context,
                integers: BTreeMap::new(),
                numbers: BTreeMap::new(),
                strings,
                flags: BTreeMap::new(),
                payload,
            },
        })
    }
    fn preview_request(&mut self, context: RemoteEditorContext) -> Result<RemoteCommand, String> {
        let id = new_id().map_err(|e| e.to_string())?;
        let mut command = self.request(
            if self.kind == Kind::Designer {
                "designer.preview"
            } else {
                "drums.preview"
            },
            context,
        )?;
        if let RemoteCommand::Editor { intent } = &mut command {
            intent.strings.insert("workspaceID".into(), id.clone());
        }
        self.preview_id = Some(id);
        Ok(command)
    }
    pub fn preview(&mut self, context: RemoteEditorContext) -> Result<RemoteCommand, String> {
        if self.playing {
            let command = self.request("audition.stop", context)?;
            self.playing = false;
            self.preview_id = None;
            Ok(command)
        } else {
            let command = self.preview_request(context)?;
            self.playing = true;
            Ok(command)
        }
    }
    pub fn failure(&mut self, tag: &crate::worker::ResponseTag, message: &str) {
        if tag.operation == "local.file" && tag.workspace_id.as_ref() == self.pending_file.as_ref()
        {
            self.pending_file = None;
            self.status = message.into();
            return;
        }
        if tag
            .workspace_id
            .as_ref()
            .is_some_and(|id| Some(id) == self.preview_id.as_ref())
        {
            self.preview_id = None;
            self.playing = false;
            self.status = message.into();
        }
    }
    pub fn response(&mut self, response: &crate::worker::EditorResponse) {
        if response.reply.status != RemoteReplyStatus::Accepted {
            self.failure(&response.tag, &response.reply.message);
        }
    }
    pub fn close(&mut self, context: RemoteEditorContext) -> Vec<RemoteCommand> {
        let active = self.playing || !self.offs.is_empty();
        self.playing = false;
        self.preview_id = None;
        self.offs.clear();
        if !active {
            return vec![];
        }
        self.request("audition.stop", context).into_iter().collect()
    }
    pub fn tick(&mut self, context: RemoteEditorContext) -> Vec<RemoteCommand> {
        let mut out = vec![];
        let notes: Vec<_> = self
            .offs
            .iter()
            .filter(|(_, time)| **time <= Instant::now())
            .map(|(pitch, _)| *pitch)
            .collect();
        for pitch in notes {
            self.offs.remove(&pitch);
            if let Ok(RemoteCommand::Editor { mut intent }) =
                self.request("designer.noteOff", context.clone())
            {
                intent.integers.insert("pitch".into(), pitch);
                out.push(RemoteCommand::Editor { intent });
            }
        }
        if self.gesture_dirty && self.playing {
            if let Ok(command) = self.preview_request(context) {
                out.push(command);
            }
        }
        self.gesture_dirty = false;
        out
    }
    pub fn key(
        &mut self,
        key: Key,
        context: RemoteEditorContext,
        operator: bool,
    ) -> Result<Vec<RemoteCommand>, String> {
        let mut out = vec![];
        if !self.catalog.is_empty() {
            match key {
                Key::Char('j') | Key::Down => {
                    self.catalog_scroll =
                        (self.catalog_scroll + 1).min(self.catalog.len().saturating_sub(1))
                }
                Key::Char('k') | Key::Up => {
                    self.catalog_scroll = self.catalog_scroll.saturating_sub(1)
                }
                Key::Tab => self.catalog.clear(),
                _ => {}
            }
            return Ok(out);
        }
        match key {
            Key::Char(' ') => {
                if !operator {
                    return Err("Audition requires an operator credential.".into());
                }
                out.push(self.preview(context)?);
            }
            Key::Tab => {
                self.focus = match self.focus {
                    Focus::Notes => Focus::Sound,
                    Focus::Sound => Focus::Keyboard,
                    Focus::Keyboard => Focus::Notes,
                }
            }
            Key::Char('u') if self.focus != Focus::Keyboard || self.kind == Kind::Beats => {
                self.restore(false)
            }
            Key::Ctrl('r') => self.restore(true),
            Key::Char('+') | Key::Char('=') => self.zoom = (self.zoom * 1.25).min(16.),
            Key::Char('-') => self.zoom = (self.zoom / 1.25).max(0.25),
            Key::Char('z') if self.focus == Focus::Keyboard => {
                self.octave = (self.octave - 1).max(0)
            }
            Key::Char('x') if self.focus == Focus::Keyboard => {
                self.octave = (self.octave + 1).min(8)
            }
            Key::Char('r') if self.focus == Focus::Keyboard => self.record = !self.record,
            Key::Char(key) if self.kind == Kind::Designer && self.focus == Focus::Keyboard => {
                if let Some(index) = "awsedftgyhujk".find(key) {
                    if !operator {
                        return Err("Live keyboard requires an operator credential.".into());
                    }
                    let pitch = (self.octave + 1) * 12 + index as i64;
                    if pitch > 127 {
                        return Ok(out);
                    }
                    let RemoteCommand::Editor { mut intent } =
                        self.request("designer.noteOn", context)?
                    else {
                        unreachable!()
                    };
                    intent.integers.insert("pitch".into(), pitch);
                    intent.numbers.insert("velocity".into(), 0.8);
                    out.push(RemoteCommand::Editor { intent });
                    self.offs
                        .insert(pitch, Instant::now() + Duration::from_millis(300));
                    if self.record {
                        self.add_note(pitch)?;
                        self.cursor = (self.cursor + 0.25)
                            .min(self.sound.as_ref().and_then(|s| s.beats).unwrap_or(4.) - 0.25);
                    }
                }
            }
            Key::Char(key) if self.kind == Kind::Beats && self.focus == Focus::Keyboard => {
                if let Some(index) = "asdfghjkl".find(key) {
                    if !operator {
                        return Err("Drum audition requires an operator credential.".into());
                    }
                    let RemoteCommand::Editor { mut intent } =
                        self.request("drums.preview", context)?
                    else {
                        unreachable!()
                    };
                    intent.strings.insert(
                        "audition".into(),
                        serde_json::to_value(&VOICES[index])
                            .unwrap()
                            .as_str()
                            .unwrap()
                            .into(),
                    );
                    out.push(RemoteCommand::Editor { intent });
                }
            }
            Key::Char('n') if self.kind == Kind::Designer => self.add_note(self.pitch)?,
            Key::Delete | Key::Char('d') => {
                self.capture();
                if self.kind == Kind::Designer {
                    if let Some(notes) = self.sound.as_mut().and_then(|s| s.notes.as_mut()) {
                        if self.selected < notes.len() {
                            notes.remove(self.selected);
                            self.selected = self.selected.min(notes.len().saturating_sub(1));
                        }
                    }
                } else {
                    self.toggle_hit(true);
                }
                self.gesture_dirty = true;
            }
            Key::Enter if self.kind == Kind::Beats => {
                self.capture();
                self.toggle_hit(false);
                self.gesture_dirty = true;
            }
            Key::Char('j')
            | Key::Down
            | Key::Char('k')
            | Key::Up
            | Key::Char('h')
            | Key::Left
            | Key::Char('l')
            | Key::Right => {
                let vertical = matches!(key, Key::Char('j') | Key::Char('k') | Key::Down | Key::Up);
                let direction = if matches!(
                    key,
                    Key::Char('j') | Key::Down | Key::Char('l') | Key::Right
                ) {
                    1.
                } else {
                    -1.
                };
                if self.kind == Kind::Beats && self.focus == Focus::Sound {
                    if vertical {
                        self.parameter = (self.parameter as i64 + direction as i64)
                            .clamp(0, (DRUM_FIELDS.len() - 1) as i64)
                            as usize;
                    } else {
                        let d = DRUM_FIELDS[self.parameter.min(DRUM_FIELDS.len() - 1)];
                        let value = serde_json::to_value(self.drums.as_ref().unwrap()).unwrap()
                            [d.0]
                            .as_f64()
                            .unwrap_or(d.3);
                        let next = if matches!(d.0, "stepsPerBeat" | "variationCycles") {
                            if direction > 0. {
                                value * 2.
                            } else {
                                value / 2.
                            }
                        } else if d.0 == "bars" {
                            value + direction
                        } else {
                            value + direction * (d.2 - d.1) / 100.
                        };
                        self.drum_field(d.0, next.clamp(d.1, d.2))?;
                    }
                } else if self.kind == Kind::Beats {
                    if vertical {
                        self.voice = (self.voice as i64 + direction as i64).clamp(0, 8) as usize;
                    } else {
                        let count = self
                            .drums
                            .as_ref()
                            .map_or(16, |p| p.bars * 4 * p.steps_per_beat);
                        self.step =
                            (self.step as i64 + direction as i64).clamp(0, count - 1) as usize;
                    }
                } else if self.focus == Focus::Sound {
                    if vertical {
                        self.parameter = (self.parameter as i64 + direction as i64)
                            .clamp(0, (SOUND_FIELDS.len() - 1) as i64)
                            as usize;
                    } else {
                        let descriptor = SOUND_FIELDS[self.parameter.min(SOUND_FIELDS.len() - 1)];
                        let current = field_value(
                            &serde_json::to_value(self.sound.as_ref().unwrap()).unwrap(),
                            descriptor.0,
                        )
                        .unwrap_or(descriptor.3);
                        self.sound_field(
                            descriptor.0,
                            (current + direction * (descriptor.2 - descriptor.1) / 100.)
                                .clamp(descriptor.1, descriptor.2),
                        )?;
                    }
                } else {
                    self.capture();
                    let beats = self.sound.as_ref().and_then(|s| s.beats).unwrap_or(4.);
                    let notes = self
                        .sound
                        .as_mut()
                        .unwrap()
                        .notes
                        .get_or_insert_with(Vec::new);
                    if let Some(note) = notes.get_mut(self.selected) {
                        if vertical {
                            note.pitch = (note.pitch - direction as i64).clamp(0, 127);
                            self.pitch = note.pitch;
                        } else {
                            note.start_beat = (note.start_beat + direction * 0.25)
                                .clamp(0., beats - note.duration_beats);
                            self.cursor = note.start_beat;
                        }
                    } else if vertical {
                        self.pitch = (self.pitch - direction as i64).clamp(0, 127);
                    } else {
                        self.cursor = (self.cursor + direction * 0.25).max(0.);
                    }
                    self.gesture_dirty = true;
                }
            }
            Key::Char('[') | Key::Char(']') if self.kind == Kind::Designer => {
                self.capture();
                let delta = if key == Key::Char('[') { -0.25 } else { 0.25 };
                let beats = self.sound.as_ref().and_then(|s| s.beats).unwrap_or(4.);
                if let Some(note) = self
                    .sound
                    .as_mut()
                    .and_then(|s| s.notes.as_mut())
                    .and_then(|n| n.get_mut(self.selected))
                {
                    note.duration_beats =
                        (note.duration_beats + delta).clamp(0.0625, beats - note.start_beat);
                }
                self.gesture_dirty = true;
            }
            Key::Char('J') if self.kind == Kind::Designer && self.focus == Focus::Notes => {
                self.selected = (self.selected + 1).min(
                    self.sound
                        .as_ref()
                        .and_then(|s| s.notes.as_ref())
                        .map_or(0, |n| n.len().saturating_sub(1)),
                )
            }
            Key::Char('K') if self.kind == Kind::Designer && self.focus == Focus::Notes => {
                self.selected = self.selected.saturating_sub(1)
            }
            Key::Mouse {
                button: 0,
                column,
                row,
                released: false,
            } => {
                if self.kind == Kind::Beats
                    && self.focus == Focus::Notes
                    && (6..=14).contains(&row)
                    && column >= 14
                {
                    let start = self.beat_window_start();
                    self.voice = (row - 6) as usize;
                    self.step = usize::from(column - 14) + start;
                    let count = self
                        .drums
                        .as_ref()
                        .map_or(16, |p| p.bars * 4 * p.steps_per_beat)
                        as usize;
                    self.step = self.step.min(count - 1);
                    self.capture();
                    self.toggle_hit(false);
                    self.gesture_dirty = true;
                }
            }
            _ => {}
        }
        Ok(out)
    }
    fn add_note(&mut self, pitch: i64) -> Result<(), String> {
        let pattern = self.sound.as_ref().ok_or("Open Designer.")?;
        if pattern.notes.as_ref().map_or(0, Vec::len) >= 256 {
            return Err(
                "Designer editing is limited to 256 notes; review a shorter MIDI excerpt.".into(),
            );
        }
        let beats = pattern.beats.unwrap_or(4.);
        if self.cursor >= beats {
            return Err("Cursor is beyond the sequence; extend its beats first.".into());
        }
        self.capture();
        let notes = self
            .sound
            .as_mut()
            .unwrap()
            .notes
            .get_or_insert_with(Vec::new);
        notes.push(SoundDesignNote {
            id: new_id().map_err(|e| e.to_string())?,
            pitch,
            velocity: 0.8,
            start_beat: self.cursor,
            duration_beats: 0.25_f64.min(beats - self.cursor),
            articulation: None,
        });
        self.selected = notes.len() - 1;
        self.gesture_dirty = true;
        Ok(())
    }
    fn toggle_hit(&mut self, remove: bool) {
        let voice = VOICES[self.voice.min(8)].clone();
        let p = self.drums.as_mut().unwrap();
        let index = self.step as i64;
        if let Some(position) = p
            .steps
            .iter()
            .position(|s| s.voice == voice && s.index == index)
        {
            p.steps.remove(position);
        } else if !remove {
            p.steps.push(DrumStep {
                voice,
                index,
                velocity: 0.8,
                pan: 0.,
                probability: 1.,
                articulation: None,
            });
        }
    }
    fn sound_field(&mut self, id: &str, value: f64) -> Result<(), String> {
        let descriptor = SOUND_FIELDS
            .iter()
            .find(|p| p.0 == id)
            .ok_or("Unknown sound parameter.")?;
        if !value.is_finite() || value < descriptor.1 || value > descriptor.2 {
            return Err(format!("{id}: {}..{}", descriptor.1, descriptor.2));
        }
        let mut json = serde_json::to_value(self.sound.as_ref().ok_or("Open Designer.")?).unwrap();
        if let Some(key) = id.strip_prefix("organic.") {
            if !json["organic"].is_object() {
                json["organic"] = serde_json::json!({});
            }
            json["organic"][key] = serde_json::json!(value);
        } else {
            json[id] = serde_json::json!(value);
        }
        let next: SoundDesignPattern = serde_json::from_value(json).map_err(|e| e.to_string())?;
        if next.notes.as_ref().is_some_and(|notes| {
            notes
                .iter()
                .any(|n| n.start_beat + n.duration_beats > next.beats.unwrap_or(4.) + 1e-8)
        }) {
            return Err("Sequence duration would hide notes; shorten them first.".into());
        }
        self.capture();
        self.sound = Some(next);
        self.gesture_dirty = true;
        Ok(())
    }
    /// Local file operations use private workspace and draft identities, never server arrival clocks.
    pub fn local_command(
        &mut self,
        words: &[String],
    ) -> Result<Option<crate::localfiles::Request>, String> {
        use crate::localfiles::{Operation, Request};
        let name = words.first().map(String::as_str).unwrap_or("");
        if !matches!(
            name,
            "midi-load"
                | "midi-save"
                | "midi-parts"
                | "midi-excerpt"
                | "preset-load"
                | "preset-save"
        ) {
            return Ok(None);
        }
        if name == "midi-parts" {
            self.show_midi_parts()?;
            return Ok(None);
        }
        if name == "midi-excerpt" {
            let part = words
                .get(1)
                .ok_or("Usage: midi-excerpt partIndex startBeat lengthBeats")?
                .parse::<usize>()
                .map_err(|_| "Part index must be an integer.")?;
            let start = words
                .get(2)
                .ok_or("Specify excerpt start beat.")?
                .parse::<f64>()
                .map_err(|_| "Invalid start beat.")?;
            let length = words
                .get(3)
                .ok_or("Specify excerpt length in beats.")?
                .parse::<f64>()
                .map_err(|_| "Invalid length.")?;
            let next = self
                .midi_file
                .as_ref()
                .ok_or("Load MIDI first.")?
                .excerpt(part, start, length)?;
            if self.sound.is_none() {
                return Err("MIDI excerpts require Designer.".into());
            }
            self.capture();
            let sound = self.sound.as_mut().unwrap();
            sound.notes = next.notes;
            sound.controllers = next.controllers;
            sound.bpm = next.bpm;
            sound.beats = next.beats;
            self.selected = 0;
            self.cursor = 0.;
            self.scroll = 0.;
            self.catalog.clear();
            self.gesture_dirty = true;
            self.status="Excerpt loaded into this private draft. Undo restores the previous sequence; Insert renders on the server.".into();
            return Ok(None);
        }
        if self.pending_file.is_some() {
            return Err("A local file job is pending in this workspace.".into());
        }
        let path = std::path::PathBuf::from(
            words
                .get(1)
                .ok_or("Specify a local file path (quote spaces).")?,
        );
        let operation = match name {
            "midi-load" if self.kind == Kind::Designer => Operation::ReadMIDI(path),
            "midi-save" if self.kind == Kind::Designer => {
                Operation::WriteMIDI(path, Box::new(self.sound.clone().unwrap()))
            }
            "preset-load" if self.kind == Kind::Designer => Operation::ReadSound(path),
            "preset-save" if self.kind == Kind::Designer => {
                Operation::WriteSound(path, Box::new(self.sound.clone().unwrap()))
            }
            "preset-load" => Operation::ReadBeats(path),
            "preset-save" => Operation::WriteBeats(path, Box::new(self.drums.clone().unwrap())),
            _ => return Err("MIDI interchange requires Designer.".into()),
        };
        let id = new_id().map_err(|e| e.to_string())?;
        self.pending_file = Some(id.clone());
        self.status = "Local file job running; transport stays available.".into();
        Ok(Some(Request {
            workspace: self.workspace_id.clone(),
            id,
            epoch: self.epoch,
            operation,
        }))
    }
    fn show_midi_parts(&mut self) -> Result<(), String> {
        let f = self
            .midi_file
            .as_ref()
            .ok_or("Load MIDI with :midi-load local.mid.")?;
        self.catalog_scroll = 0;
        self.catalog=vec![format!("{:.2} BPM / {:.3} beats / {} PPQ. :midi-excerpt index start length (max64 beats /256 notes)",f.bpm,f.beats,f.ppq)];
        self.catalog
            .extend(f.parts.iter().enumerate().map(|(i, p)| {
                format!(
                    "{i}: {} / {} notes / {} controllers",
                    p.name,
                    p.notes.len(),
                    p.controllers.len()
                )
            }));
        self.status =
            "Choose a part and explicit excerpt; no notes have been discarded or inserted.".into();
        Ok(())
    }
    pub fn local_response(&mut self, response: &crate::localfiles::Response) {
        use crate::localfiles::Data;
        if response.workspace != self.workspace_id
            || self.pending_file.as_ref() != Some(&response.id)
        {
            return;
        }
        self.pending_file = None;
        match &response.result {
            Err(e) => self.status = e.clone(),
            Ok(Data::Effects(_)) => {}
            Ok(Data::Saved) => {
                self.status = "Local file saved. Existing files are never overwritten.".into()
            }
            Ok(Data::MIDI(f)) => {
                self.midi_file = Some(f.clone());
                let _ = self.show_midi_parts();
            }
            Ok(Data::Sound(p)) => {
                if response.epoch != self.epoch {
                    self.status="Sound changed during load; preset was not applied. Load again to replace it.".into();
                    return;
                }
                let Some(old) = self.sound.as_ref() else {
                    return;
                };
                let mut next = *p.clone();
                next.notes = old.notes.clone();
                next.controllers = old.controllers.clone();
                next.bpm = old.bpm;
                next.beats = old.beats;
                self.capture();
                self.sound = Some(next);
                self.gesture_dirty = true;
                self.status =
                    "Sound preset loaded; notes, controllers and tempo preserved. Undo available."
                        .into();
            }
            Ok(Data::Beats(p)) => {
                if response.epoch != self.epoch {
                    self.status="Beat changed during load; preset was not applied. Load again to replace it.".into();
                    return;
                }
                self.capture();
                self.drums = Some(*p.clone());
                self.gesture_dirty = true;
                self.status = "Drum pattern loaded into this private draft. Undo available.".into();
            }
        }
    }
    pub fn command(
        &mut self,
        words: &[String],
        cap: &RemoteCapabilities,
        context: RemoteEditorContext,
    ) -> Result<Option<(RemoteCommand, bool)>, String> {
        let arg = |i: usize| words.get(i).ok_or_else(|| format!("Missing argument {i}."));
        let number = |i: usize| {
            arg(i)?
                .parse::<f64>()
                .map_err(|_| "Expected a finite number.".to_string())
        };
        let name = words.first().ok_or("Missing command.")?.as_str();
        if matches!(
            name,
            "presets" | "instruments" | "kits" | "layers" | "controllers" | "help"
        ) {
            self.catalog_scroll = 0;
            self.catalog = match name {
                "presets" if self.kind == Kind::Designer => cap.designer_presets.iter().enumerate().map(|(i,p)|format!("{i}: {} / {}",p.name,p.category)).collect(),
                "presets" => cap.drum_presets.iter().enumerate().map(|(i,p)|format!("{i}: {}",p.name)).collect(),
                "instruments" => cap.instruments.iter().map(|p|format!("{}: {} / {}",p.id,p.name,p.character_label)).collect(),
                "kits" => cap.drum_kits.iter().map(|p|format!("{}: {}",p.id,p.name)).collect(),
                "layers" => self.sound.as_ref().and_then(|p|p.layers.as_ref()).map(|ls|ls.iter().enumerate().map(|(i,l)|format!("{i}: {:?} gain {:.3} pan {:.3} transpose {} velocity {:.2}..{:.2} {}",l.preset,l.gain.unwrap_or(0.5),l.pan.unwrap_or(0.),l.transpose.unwrap_or(0),l.minimum_velocity.unwrap_or(0.),l.maximum_velocity.unwrap_or(1.),if l.enabled.unwrap_or(true){"On"}else{"Off"})).collect()).unwrap_or_default(),
                "controllers" => self.sound.as_ref().and_then(|p|p.controllers.as_ref()).map(|ps|ps.iter().map(|p|format!("{:?} beat {:.3} value {:.3}",p.kind,p.beat,p.value)).collect()).unwrap_or_default(),
                _ => [":presets; :preset index; :instruments; :instrument ID",":sound parameter value; :waveform waveform|secondWaveform sine|triangle|saw|pulse",":layers; :layer-add factoryPresetIndex; :layer-remove index",":layer-set index gain|pan|transpose|minimumVelocity|maximumVelocity|enabled value",":layer-set index toneOverride|characterOverride|attackOverride|releaseOverride|velocityFade|organicSeed value",":controllers; :controller expression|pitchBend|modulation|sustain beat value",":controller-clear kind; :seed unsigned64; :velocity value",":kits; :kit ID; :beat bpm|bars|stepsPerBeat|swing|humanize|masterGain|velocityHumanize|variationCycles value",":lane kit|gain|pan|tuning|damping|muted value",":hit velocity|pan|probability|articulation value",":midi-load local.mid; :midi-parts; :midi-excerpt partIndex startBeat lengthBeats",":midi-save new.mid; :preset-load localPreset.json; :preset-save newPreset.json",":insert; :audition; :undo; :redo; :back"].iter().map(|s|s.to_string()).collect(),
            };
            if self.catalog.is_empty() {
                self.catalog
                    .push("No entries in this draft / host bank.".into());
            }
            self.status = format!("{name}: j/k scroll; Tab or :back returns to editing.");
            return Ok(None);
        }
        self.catalog.clear();
        match name {
            "back" => {}
            "insert" => {
                return Ok(Some((
                    self.request(
                        if self.kind == Kind::Designer {
                            "designer.commit"
                        } else {
                            "drums.commit"
                        },
                        context,
                    )?,
                    false,
                )))
            }
            "audition" => return Ok(Some((self.preview(context)?, true))),
            "sound" => self.sound_field(arg(1)?, number(2)?)?,
            "instrument" => {
                let id = arg(1)?;
                if !cap.instruments.iter().any(|i| &i.id == id) {
                    return Err("Instrument is not advertised by this host.".into());
                }
                let instrument =
                    serde_json::from_value(serde_json::json!(id)).map_err(|e| e.to_string())?;
                self.capture();
                self.sound.as_mut().ok_or("Open Designer.")?.instrument = Some(instrument);
                self.gesture_dirty = true;
            }
            "preset" => {
                let index: usize = arg(1)?
                    .parse()
                    .map_err(|_| "Preset index must be an integer.")?;
                self.capture();
                if self.kind == Kind::Designer {
                    let mut next = cap
                        .designer_presets
                        .get(index)
                        .ok_or("Unknown Designer preset.")?
                        .sound
                        .clone();
                    let old = self.sound.as_ref().unwrap();
                    next.notes = old.notes.clone();
                    next.controllers = old.controllers.clone();
                    next.beats = old.beats;
                    next.bpm = old.bpm;
                    self.sound = Some(next);
                } else {
                    self.drums = Some(
                        cap.drum_presets
                            .get(index)
                            .ok_or("Unknown drum preset.")?
                            .pattern
                            .clone(),
                    );
                }
                self.preset = index;
                self.gesture_dirty = true;
            }
            "velocity" => {
                let value = number(1)?;
                if !value.is_finite() || !(0. ..=1.).contains(&value) {
                    return Err("Velocity range: 0..1.".into());
                }
                self.capture();
                if self.kind == Kind::Designer {
                    let note = self
                        .sound
                        .as_mut()
                        .and_then(|p| p.notes.as_mut())
                        .and_then(|n| n.get_mut(self.selected))
                        .ok_or("Select a note.")?;
                    note.velocity = value;
                } else {
                    let voice = &VOICES[self.voice];
                    let step = self
                        .drums
                        .as_mut()
                        .unwrap()
                        .steps
                        .iter_mut()
                        .find(|s| &s.voice == voice && s.index == self.step as i64)
                        .ok_or("Enable a hit first.")?;
                    step.velocity = value;
                }
                self.gesture_dirty = true;
            }
            "waveform" => {
                let field = arg(1)?;
                if !matches!(field.as_str(), "waveform" | "secondWaveform") {
                    return Err(
                        "Usage: waveform waveform|secondWaveform sine|triangle|saw|pulse".into(),
                    );
                }
                let waveform: SoundDesignWaveform =
                    serde_json::from_value(serde_json::json!(arg(2)?))
                        .map_err(|e| e.to_string())?;
                self.capture();
                let pattern = self.sound.as_mut().ok_or("Open Designer.")?;
                if field == "waveform" {
                    pattern.waveform = Some(waveform)
                } else {
                    pattern.second_waveform = Some(waveform)
                }
                self.gesture_dirty = true;
            }
            "seed" => {
                let seed = arg(1)?
                    .parse::<u64>()
                    .map_err(|_| "Seed must be an exact unsigned 64-bit integer.")?
                    .to_string();
                self.capture();
                let pattern = self.sound.as_mut().ok_or("Open Designer.")?;
                pattern
                    .organic
                    .get_or_insert(OrganicSound {
                        drift: None,
                        timbre: None,
                        breath: None,
                        seed: None,
                    })
                    .seed = Some(seed);
                self.gesture_dirty = true;
            }
            "layer-add" => {
                let index: usize = arg(1)?
                    .parse()
                    .map_err(|_| "Usage: layer-add factoryPresetIndex")?;
                let metadata = cap.designer_presets.get(index).ok_or("Unknown preset.")?;
                let preset: SoundDesignPreset = serde_json::from_value(serde_json::json!(metadata
                    .id
                    .strip_prefix("factory.")
                    .ok_or("Choose a factory instrument preset for this layer.")?))
                .map_err(|e| e.to_string())?;
                if self
                    .sound
                    .as_ref()
                    .ok_or("Open Designer.")?
                    .layers
                    .as_ref()
                    .map_or(0, Vec::len)
                    >= 6
                {
                    return Err("The native designer supports six additional layers.".into());
                }
                let layer:SoundDesignLayer=serde_json::from_value(serde_json::json!({"id":new_id().map_err(|e|e.to_string())?,"preset":preset,"gain":0.5,"pan":0,"transpose":0,"minimumVelocity":0,"maximumVelocity":1,"enabled":true})).map_err(|e|e.to_string())?;
                self.capture();
                self.sound
                    .as_mut()
                    .unwrap()
                    .layers
                    .get_or_insert_with(Vec::new)
                    .push(layer);
                self.gesture_dirty = true;
            }
            "layer-remove" => {
                let index: usize = arg(1)?
                    .parse()
                    .map_err(|_| "Layer index must be an integer.")?;
                if self
                    .sound
                    .as_ref()
                    .and_then(|p| p.layers.as_ref())
                    .map_or(true, |l| index >= l.len())
                {
                    return Err("Unknown layer index.".into());
                }
                self.capture();
                self.sound
                    .as_mut()
                    .unwrap()
                    .layers
                    .as_mut()
                    .unwrap()
                    .remove(index);
                self.gesture_dirty = true;
            }
            "layer-set" => {
                let index: usize = arg(1)?
                    .parse()
                    .map_err(|_| "Layer index must be an integer.")?;
                let field = arg(2)?.as_str();
                let layer = self
                    .sound
                    .as_ref()
                    .and_then(|p| p.layers.as_ref())
                    .and_then(|l| l.get(index))
                    .ok_or("Unknown layer index.")?;
                let mut json = serde_json::to_value(layer).unwrap();
                let value = arg(3)?;
                json[field] = if field == "enabled" {
                    serde_json::json!(value
                        .parse::<bool>()
                        .map_err(|_| "enabled: true or false")?)
                } else if field == "organicSeed" {
                    serde_json::json!(value
                        .parse::<u64>()
                        .map_err(|_| "Seed must be unsigned 64-bit.")?
                        .to_string())
                } else {
                    let value = number(3)?;
                    let (min, max) = match field {
                        "gain" | "minimumVelocity" | "maximumVelocity" | "toneOverride"
                        | "characterOverride" => (0., 1.),
                        "pan" => (-1., 1.),
                        "transpose" => (-24., 24.),
                        "attackOverride" | "releaseOverride" => (0., 2.),
                        "velocityFade" => (0., 0.5),
                        _ => return Err("Unknown layer parameter.".into()),
                    };
                    if !value.is_finite()
                        || value < min
                        || value > max
                        || field == "transpose" && value.fract() != 0.
                    {
                        return Err(format!("{field}: {min}..{max}"));
                    }
                    if field == "transpose" {
                        serde_json::json!(value as i64)
                    } else {
                        serde_json::json!(value)
                    }
                };
                let next: SoundDesignLayer =
                    serde_json::from_value(json).map_err(|e| e.to_string())?;
                if next.minimum_velocity.unwrap_or(0.) > next.maximum_velocity.unwrap_or(1.) {
                    return Err("Minimum velocity cannot exceed maximum velocity.".into());
                }
                self.capture();
                self.sound.as_mut().unwrap().layers.as_mut().unwrap()[index] = next;
                self.gesture_dirty = true;
            }
            "controller" => {
                let kind: SoundDesignControllerKind =
                    serde_json::from_value(serde_json::json!(arg(1)?))
                        .map_err(|e| e.to_string())?;
                let beat = number(2)?;
                let value = number(3)?;
                let pattern = self.sound.as_ref().ok_or("Open Designer.")?;
                if !beat.is_finite()
                    || beat < 0.
                    || beat > pattern.beats.unwrap_or(4.)
                    || !value.is_finite()
                    || !(0. ..=1.).contains(&value)
                {
                    return Err(
                        "Controller beat must be within the sequence; value is 0..1.".into(),
                    );
                }
                if pattern.controllers.as_ref().map_or(0, Vec::len) >= 4096 {
                    return Err("A pattern supports 4096 controller points.".into());
                }
                self.capture();
                let controllers = self
                    .sound
                    .as_mut()
                    .unwrap()
                    .controllers
                    .get_or_insert_with(Vec::new);
                controllers.retain(|p| p.kind != kind || p.beat != beat);
                controllers.push(SoundDesignController { kind, beat, value });
                controllers.sort_by(|a, b| a.beat.total_cmp(&b.beat));
                self.gesture_dirty = true;
            }
            "controller-clear" => {
                let kind: SoundDesignControllerKind =
                    serde_json::from_value(serde_json::json!(arg(1)?))
                        .map_err(|e| e.to_string())?;
                self.capture();
                self.sound
                    .as_mut()
                    .ok_or("Open Designer.")?
                    .controllers
                    .get_or_insert_with(Vec::new)
                    .retain(|p| p.kind != kind);
                self.gesture_dirty = true;
            }
            "lane" => {
                self.lane_field(arg(1)?, arg(2)?, cap)?;
            }
            "hit" => {
                self.hit_field(arg(1)?, arg(2)?, cap)?;
            }
            "beat" => {
                let value = number(2)?;
                self.drum_field(arg(1)?, value)?;
            }
            "kit" => {
                let id = arg(1)?;
                if !cap.drum_kits.iter().any(|k| &k.id == id) {
                    return Err("Kit is not advertised by this host.".into());
                }
                let kit: DrumKit =
                    serde_json::from_value(serde_json::json!(id)).map_err(|e| e.to_string())?;
                self.capture();
                let p = self.drums.as_mut().ok_or("Open Beats.")?;
                p.kit = kit.clone();
                for lane in p.lanes.values_mut() {
                    lane.kit = kit.clone();
                }
                self.gesture_dirty = true;
            }
            "undo" => self.restore(false),
            "redo" => self.restore(true),
            _ => return Err(format!("Unknown creative command: {name}.")),
        }
        Ok(None)
    }
    fn lane_field(
        &mut self,
        id: &str,
        value: &str,
        cap: &RemoteCapabilities,
    ) -> Result<(), String> {
        let voice = serde_json::to_value(&VOICES[self.voice])
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        let p = self.drums.as_ref().ok_or("Open Beats.")?;
        let mut lane = p.lanes.get(&voice).cloned().unwrap_or(DrumLane {
            kit: p.kit.clone(),
            damping: Some(0.5),
            gain: 1.,
            pan: 0.,
            tuning: 0.,
            muted: false,
            custom_kit: None,
            shape: None,
        });
        match id {
            "kit" => {
                if !cap.drum_kits.iter().any(|k| k.id == value) {
                    return Err("Unknown host drum kit.".into());
                }
                lane.kit =
                    serde_json::from_value(serde_json::json!(value)).map_err(|e| e.to_string())?;
            }
            "muted" => lane.muted = value.parse().map_err(|_| "muted: true or false")?,
            _ => {
                let v: f64 = value.parse().map_err(|_| "Lane value must be numeric.")?;
                let (min, max) = match id {
                    "gain" => (0., 1.5),
                    "pan" => (-1., 1.),
                    "tuning" => (-12., 12.),
                    "damping" => (0., 1.),
                    _ => return Err("Unknown lane parameter.".into()),
                };
                if !v.is_finite() || v < min || v > max {
                    return Err(format!("{id}: {min}..{max}"));
                }
                match id {
                    "gain" => lane.gain = v,
                    "pan" => lane.pan = v,
                    "tuning" => lane.tuning = v,
                    _ => lane.damping = Some(v),
                }
            }
        }
        self.capture();
        self.drums.as_mut().unwrap().lanes.insert(voice, lane);
        self.gesture_dirty = true;
        Ok(())
    }
    fn hit_field(&mut self, id: &str, value: &str, cap: &RemoteCapabilities) -> Result<(), String> {
        let pattern = self.drums.as_ref().ok_or("Open Beats.")?;
        let voice = &VOICES[self.voice];
        let index = pattern
            .steps
            .iter()
            .position(|s| &s.voice == voice && s.index == self.step as i64)
            .ok_or("Enable a hit first.")?;
        let mut step = pattern.steps[index].clone();
        if id == "articulation" {
            let kit = serde_json::to_value(&pattern.kit).unwrap();
            let metadata = cap
                .drum_kits
                .iter()
                .find(|k| Some(k.id.as_str()) == kit.as_str())
                .ok_or("Unknown kit metadata.")?;
            let name = serde_json::to_value(voice).unwrap();
            if !metadata
                .articulations
                .get(name.as_str().unwrap())
                .is_some_and(|a| a.iter().any(|a| a == value))
            {
                return Err("This drum voice does not support that articulation.".into());
            }
            step.articulation =
                Some(serde_json::from_value(serde_json::json!(value)).map_err(|e| e.to_string())?);
        } else {
            let v: f64 = value.parse().map_err(|_| "Hit value must be numeric.")?;
            let (min, max) = if id == "pan" { (-1., 1.) } else { (0., 1.) };
            if !v.is_finite() || v < min || v > max {
                return Err(format!("{id}: {min}..{max}"));
            }
            match id {
                "velocity" => step.velocity = v,
                "pan" => step.pan = v,
                "probability" => step.probability = v,
                _ => return Err("Unknown hit parameter.".into()),
            }
        }
        self.capture();
        self.drums.as_mut().unwrap().steps[index] = step;
        self.gesture_dirty = true;
        Ok(())
    }
    fn drum_field(&mut self, id: &str, value: f64) -> Result<(), String> {
        let p = self.drums.as_ref().ok_or("Open Beats.")?;
        let (min, max) = match id {
            "bpm" => (30., 300.),
            "bars" => (1., 8.),
            "stepsPerBeat" => (2., 8.),
            "swing" => (0., 0.7),
            "humanize" => (0., 0.025),
            "masterGain" => (0., 1.),
            "velocityHumanize" => (0., 0.2),
            "variationCycles" => (1., 8.),
            _ => return Err("Unknown beat parameter.".into()),
        };
        if !value.is_finite()
            || value < min
            || value > max
            || matches!(id, "bars" | "stepsPerBeat" | "variationCycles") && value.fract() != 0.
            || id == "stepsPerBeat" && ![2., 4., 8.].contains(&value)
            || id == "variationCycles" && ![1., 2., 4., 8.].contains(&value)
        {
            return Err(format!(
                "{id}: {min}..{max}; grid/cycles use powers of two."
            ));
        }
        let old = p.steps_per_beat;
        let mut json = serde_json::to_value(p).unwrap();
        json[id] = if matches!(id, "bars" | "stepsPerBeat" | "variationCycles") {
            serde_json::json!(value as i64)
        } else {
            serde_json::json!(value)
        };
        let mut next: DrumPattern = serde_json::from_value(json).map_err(|e| e.to_string())?;
        if id == "stepsPerBeat" {
            let mut hits = BTreeMap::new();
            for mut hit in next.steps {
                hit.index = (hit.index as f64 * value / old as f64).round() as i64;
                let key = (serde_json::to_string(&hit.voice).unwrap(), hit.index);
                if hits.get(&key).map_or(true, |previous: &DrumStep| {
                    previous.velocity <= hit.velocity
                }) {
                    hits.insert(key, hit);
                }
            }
            next.steps = hits.into_values().collect();
        }
        next.steps
            .retain(|s| s.index < next.bars * 4 * next.steps_per_beat);
        self.capture();
        self.step = self
            .step
            .min((next.bars * 4 * next.steps_per_beat) as usize - 1);
        self.drums = Some(next);
        self.gesture_dirty = true;
        Ok(())
    }
    fn beat_window_start(&self) -> usize {
        let count = self
            .drums
            .as_ref()
            .map_or(16, |p| p.bars * 4 * p.steps_per_beat) as usize;
        self.step
            .saturating_sub(self.viewport_columns / 2)
            .min(count.saturating_sub(self.viewport_columns))
    }
    pub fn draw(&mut self, c: &mut Canvas) {
        self.viewport_columns = c.width.saturating_sub(15).max(1);
        if self.index_dirty {
            self.note_index = NoteIndex::new(
                self.sound
                    .as_ref()
                    .and_then(|p| p.notes.as_deref())
                    .unwrap_or_default(),
            );
            self.index_dirty = false;
        }
        for y in 2..c.height.saturating_sub(3) {
            c.fill(0, y, c.width, ' ', 252, 234);
        }
        c.text(
            1,
            2,
            &format!(
                "{:?} / {:?}  {}  Tab focus  Space audition  :insert  Esc timeline",
                self.kind,
                self.focus,
                if self.playing { "AUDITION" } else { "Draft" }
            ),
            81,
            234,
            true,
        );
        if !self.catalog.is_empty() {
            for (i, line) in self
                .catalog
                .iter()
                .skip(self.catalog_scroll)
                .take(c.height.saturating_sub(9))
                .enumerate()
            {
                c.text(1, 4 + i, line, 252, 234, false);
            }
            c.text(1, c.height.saturating_sub(4), &self.status, 245, 234, false);
            return;
        }
        if self.kind == Kind::Designer {
            let p = self.sound.as_ref().unwrap();
            let span = (p.beats.unwrap_or(4.) / self.zoom).max(0.25);
            let selected = p
                .notes
                .as_ref()
                .and_then(|ns| ns.get(self.selected))
                .map_or(self.cursor, |n| n.start_beat);
            if selected < self.scroll {
                self.scroll = selected;
            }
            if selected >= self.scroll + span {
                self.scroll = (selected - span * 0.75).max(0.);
            }
        }
        match self.kind {
            Kind::Designer => self.draw_designer(c),
            Kind::Beats => self.draw_beats(c),
        }
        c.text(1, c.height.saturating_sub(4), &self.status, 245, 234, false);
    }
    fn draw_designer(&self, c: &mut Canvas) {
        let Some(p) = self.sound.as_ref() else {
            return;
        };
        c.text(
            1,
            3,
            &format!(
                "Instrument {:?}  {:.1} BPM  {} beats  n add  j/k pitch  h/l time  [/] duration",
                p.instrument,
                p.bpm.unwrap_or(120.),
                p.beats.unwrap_or(4.)
            ),
            252,
            234,
            false,
        );
        if self.focus == Focus::Sound {
            let value = serde_json::to_value(p).unwrap();
            for (i, (id, min, max, default)) in SOUND_FIELDS
                .iter()
                .enumerate()
                .skip(self.parameter.saturating_sub(4))
                .take(c.height.saturating_sub(9))
            {
                c.text(
                    2,
                    5 + i - self.parameter.saturating_sub(4),
                    &format!(
                        "{} {id:<15} {:>8.3}  [{min}..{max}]",
                        if i == self.parameter { ">" } else { " " },
                        field_value(&value, id).unwrap_or(*default)
                    ),
                    if i == self.parameter { 81 } else { 252 },
                    234,
                    false,
                );
            }
            return;
        }
        if self.focus == Focus::Keyboard {
            let mut x = 1;
            for (i, key) in "awsedftgyhujk".chars().enumerate() {
                let pitch = (self.octave + 1) * 12 + i as i64;
                let label = format!("{key}:{} ", note_name(pitch));
                if x + label.len() > c.width {
                    break;
                }
                c.text(
                    x,
                    5,
                    &label,
                    if self.offs.contains_key(&pitch) {
                        203
                    } else {
                        81
                    },
                    234,
                    true,
                );
                x += label.len();
            }
            c.text(
                1,
                7,
                &format!(
                    "z/x octave {}  r {}  taps sound 300 ms on the host",
                    self.octave,
                    if self.record {
                        "Record notes ON"
                    } else {
                        "Record notes OFF"
                    }
                ),
                252,
                234,
                false,
            );
            return;
        }
        let rows = c.height.saturating_sub(10).min(25);
        let center = p
            .notes
            .as_ref()
            .and_then(|n| n.get(self.selected))
            .map_or(self.pitch, |n| n.pitch);
        let top = (center + rows as i64 / 2).min(127);
        let columns = c.width.saturating_sub(10);
        let span = (p.beats.unwrap_or(4.) / self.zoom).max(0.25);
        for row in 0..rows {
            let pitch = top - row as i64;
            c.text(1, 5 + row, &note_name(pitch), 245, 234, false);
            c.fill(
                9,
                5 + row,
                columns,
                if pitch % 12 == 0 { '-' } else { '.' },
                238,
                234,
            );
            if let Some(notes) = p.notes.as_ref() {
                for i in self
                    .note_index
                    .visible(pitch, self.scroll, self.scroll + span)
                {
                    let note = &notes[i];
                    let start = (((note.start_beat - self.scroll) / span * columns as f64).floor()
                        as isize)
                        .max(0) as usize;
                    let end = (((note.start_beat + note.duration_beats - self.scroll) / span
                        * columns as f64)
                        .ceil() as isize)
                        .clamp(0, columns as isize) as usize;
                    c.fill(
                        9 + start,
                        5 + row,
                        end.saturating_sub(start),
                        '=',
                        if i == self.selected { 81 } else { 110 },
                        234,
                    );
                }
            }
        }
        if let Some(n) = p.notes.as_ref().and_then(|n| n.get(self.selected)) {
            c.text(1,c.height.saturating_sub(5),&format!("Note {} {} start {:.3} length {:.3} velocity {:.3}; J/K select; :velocity value",self.selected+1,note_name(n.pitch),n.start_beat,n.duration_beats,n.velocity),252,234,false);
        }
    }
    fn draw_beats(&self, c: &mut Canvas) {
        let Some(p) = self.drums.as_ref() else {
            return;
        };
        c.text(
            1,
            3,
            &format!(
                "{} / {:?}  {} BPM  {} bars  Enter hit  h/l step  j/k drum",
                p.name, p.kit, p.bpm, p.bars
            ),
            252,
            234,
            false,
        );
        c.text(
            1,
            4,
            "Commands :kit ID; :beat swing 0.15; :velocity 0.8; :preset index",
            245,
            234,
            false,
        );
        if self.focus == Focus::Sound {
            let value = serde_json::to_value(p).unwrap();
            for (i, (id, min, max, default)) in DRUM_FIELDS
                .iter()
                .enumerate()
                .skip(self.parameter.saturating_sub(3))
                .take(c.height.saturating_sub(9))
            {
                c.text(
                    2,
                    5 + i - self.parameter.saturating_sub(3),
                    &format!(
                        "{} {id:<18} {:>8.3} [{min}..{max}]",
                        if i == self.parameter { ">" } else { " " },
                        value[id].as_f64().unwrap_or(*default)
                    ),
                    if i == self.parameter { 81 } else { 252 },
                    234,
                    false,
                );
            }
            return;
        }
        if self.focus == Focus::Keyboard {
            for (i, (key, voice)) in "asdfghjkl".chars().zip(VOICES.iter()).enumerate() {
                c.text(2, 6 + i, &format!("{key}: {voice:?}"), 81, 234, true);
            }
            return;
        }
        let count = p.bars * 4 * p.steps_per_beat;
        let visible = c.width.saturating_sub(15);
        let start = self.beat_window_start();
        for (i, voice) in VOICES.iter().enumerate() {
            if 6 + i >= c.height.saturating_sub(5) {
                break;
            }
            let name = serde_json::to_value(voice).unwrap();
            c.text(
                1,
                6 + i,
                &format!(
                    "{} {:<11}",
                    if i == self.voice { ">" } else { " " },
                    name.as_str().unwrap()
                ),
                252,
                234,
                false,
            );
            for step in start..(start + visible).min(count as usize) {
                let hit = p
                    .steps
                    .iter()
                    .find(|s| &s.voice == voice && s.index == step as i64);
                let selected = i == self.voice && step == self.step;
                c.put(
                    14 + step - start,
                    6 + i,
                    hit.map_or(
                        if step as i64 % p.steps_per_beat == 0 {
                            '|'
                        } else {
                            '.'
                        },
                        |h| if h.probability < 1. { '?' } else { 'o' },
                    ),
                    if selected {
                        231
                    } else if hit.is_some() {
                        81
                    } else {
                        240
                    },
                    if selected { 24 } else { 234 },
                    selected,
                );
            }
        }
    }
}
pub const VOICES: [DrumVoice; 9] = [
    DrumVoice::Kick,
    DrumVoice::Snare,
    DrumVoice::ClosedHat,
    DrumVoice::OpenHat,
    DrumVoice::LowTom,
    DrumVoice::HighTom,
    DrumVoice::Clap,
    DrumVoice::Ride,
    DrumVoice::Crash,
];
const SOUND_FIELDS: [(&str, f64, f64, f64); 20] = [
    ("tone", 0., 1., 0.6),
    ("character", 0., 1., 0.5),
    ("resonance", 0., 1., 0.2),
    ("attack", 0., 2., 0.01),
    ("decay", 0., 2., 0.1),
    ("sustain", 0., 1., 0.75),
    ("release", 0., 2., 0.15),
    ("oscillatorMix", 0., 1., 0.),
    ("detune", -50., 50., 0.),
    ("pulseWidth", 0.05, 0.95, 0.5),
    ("subLevel", 0., 1., 0.),
    ("noiseLevel", 0., 1., 0.),
    ("drive", 1., 8., 1.),
    ("baseGain", 0., 1., 1.),
    ("basePan", -1., 1., 0.),
    ("bpm", 30., 300., 120.),
    ("beats", 0.25, 64., 4.),
    ("organic.drift", 0., 1., 0.),
    ("organic.timbre", 0., 1., 0.),
    ("organic.breath", 0., 1., 0.),
];
const DRUM_FIELDS: [(&str, f64, f64, f64); 8] = [
    ("bpm", 30., 300., 120.),
    ("bars", 1., 8., 1.),
    ("stepsPerBeat", 2., 8., 4.),
    ("swing", 0., 0.7, 0.),
    ("humanize", 0., 0.025, 0.),
    ("masterGain", 0., 1., 0.8),
    ("velocityHumanize", 0., 0.2, 0.),
    ("variationCycles", 1., 8., 1.),
];
pub fn note_name(pitch: i64) -> String {
    const NOTES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!(
        "{}{}",
        NOTES[pitch.rem_euclid(12) as usize],
        pitch.div_euclid(12) - 1
    )
}

fn field_value(json: &serde_json::Value, id: &str) -> Option<f64> {
    if let Some(field) = id.strip_prefix("organic.") {
        json["organic"][field].as_f64()
    } else {
        json[id].as_f64()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cap() -> RemoteCapabilities {
        serde_json::from_value(serde_json::json!({
            "protocolVersions":[3],"projectVersions":[9],"commands":["editor"],"editorOperations":["audition.stop","designer.preview","designer.stop","designer.noteOn","designer.noteOff","designer.allNotesOff","designer.commit","drums.preview","drums.stop","drums.commit"],
            "sampleRate":48000,"deviceBackends":["CoreAudio"],"codecs":["wav"],"snapshotPagination":true,"maxFrameBytes":8388608,"maxUploadChunkBytes":262144,"maxUploadBytes":2147483648_i64,"maxTimelineFrame":17280000000_i64,"features":["designer","beats"],"effects":[],"selectionEffects":[],
            "instruments":[{"id":"studioPiano","name":"Studio piano","characterLabel":"Body"}],
            "designerPresets":[{"id":"factory.Studio piano","name":"Studio piano","category":"Keys","sound":{"instrument":"studioPiano","beats":4,"bpm":120,"notes":[{"id":"00000000-0000-0000-0000-000000000001","pitch":60,"velocity":0.8,"startBeat":0,"durationBeats":1}]}}],
            "drumKits":[{"id":"acoustic","name":"Acoustic","voices":["kick"],"articulations":{"kick":["normal"]}}],
            "drumPresets":[{"id":"basic","name":"Basic","pattern":{"name":"Basic","kit":"acoustic","bpm":120,"bars":2,"stepsPerBeat":4,"swing":0,"humanize":0,"masterGain":0.8,"steps":[{"voice":"kick","index":2,"velocity":0.8,"pan":0,"probability":1}],"lanes":{}}}]
        })).unwrap()
    }
    fn context() -> RemoteEditorContext {
        serde_json::from_value(serde_json::json!({"trackID":"00000000-0000-0000-0000-000000000002","clipID":"00000000-0000-0000-0000-000000000003","clipIDs":[],"cursorFrame":24000})).unwrap()
    }
    fn open(kind: Kind) -> Creative {
        Creative::open(kind, &cap(), None, 120.).unwrap()
    }
    fn intent(command: &RemoteCommand) -> &RemoteEditorIntent {
        match command {
            RemoteCommand::Editor { intent } => intent,
            _ => panic!("Expected workspace editor operation"),
        }
    }
    fn cmd(c: &mut Creative, text: &str) -> Result<Option<(RemoteCommand, bool)>, String> {
        c.command(&crate::app::split_words(text).unwrap(), &cap(), context())
    }
    fn preview_tag(command: &RemoteCommand) -> crate::worker::ResponseTag {
        let intent = intent(command);
        crate::worker::ResponseTag {
            operation: intent.operation.clone(),
            workspace_id: intent.strings.get("workspaceID").cloned(),
            track_id: None,
            effect_id: None,
            range: None,
            source_revision: None,
            track_revision: None,
        }
    }
    #[test]
    fn rejected_preview_is_retryable_and_older_receipts_cannot_stop_a_new_preview() {
        for kind in [Kind::Designer, Kind::Beats] {
            let mut c = open(kind);
            let old = preview_tag(&c.preview(context()).unwrap());
            assert!(c.playing);
            c.response(&crate::worker::EditorResponse {
                tag: old.clone(),
                reply: std::sync::Arc::new(RemoteReply {
                    request_id: "old".into(),
                    status: RemoteReplyStatus::Rejected,
                    revision: 1,
                    message: "Another client owns this audition.".into(),
                    payload: None,
                }),
            });
            assert!(!c.playing);
            assert!(c.status.contains("owns"));
            let current = preview_tag(&c.preview(context()).unwrap());
            assert_ne!(old.workspace_id, current.workspace_id);
            c.failure(&old, "Late failure");
            assert!(c.playing);
            c.failure(&current, "Not sent: queue full");
            assert!(!c.playing);
            assert!(c.preview(context()).is_ok());
            assert!(c.playing);
        }
    }
    #[test]
    fn live_update_failure_and_closed_workspace_are_correlated_to_the_latest_preview() {
        let mut c = open(Kind::Designer);
        let old = preview_tag(&c.preview(context()).unwrap());
        c.gesture_dirty = true;
        let update = c.tick(context());
        let latest = preview_tag(&update[0]);
        assert_ne!(old.workspace_id, latest.workspace_id);
        c.failure(&old, "Old render failed");
        assert!(c.playing);
        c.close(context());
        c.failure(&latest, "Late render failed");
        assert_ne!(c.status, "Late render failed");
        assert!(!c.playing);
    }
    #[test]
    fn large_note_index_skips_invisible_notes_and_retains_overlaps() {
        let notes: Vec<_> = (0..100000)
            .map(|i| SoundDesignNote {
                id: format!("{i}"),
                pitch: 60,
                velocity: 0.8,
                start_beat: i as f64,
                duration_beats: 0.5,
                articulation: None,
            })
            .collect();
        let index = NoteIndex::new(&notes);
        assert_eq!(index.visible(60, 99990., 100000.).count(), 10);
        assert_eq!(index.visible(61, 99990., 100000.).count(), 0);
        let notes = vec![
            SoundDesignNote {
                id: "long".into(),
                pitch: 60,
                velocity: 1.,
                start_beat: 0.,
                duration_beats: 100.,
                articulation: None,
            },
            SoundDesignNote {
                id: "short".into(),
                pitch: 60,
                velocity: 1.,
                start_beat: 2.,
                duration_beats: 1.,
                articulation: None,
            },
        ];
        assert_eq!(
            NoteIndex::new(&notes)
                .visible(60, 50., 51.)
                .collect::<Vec<_>>(),
            vec![0]
        );
    }
    #[test]
    fn workspace_space_and_live_edits_never_touch_main_transport() {
        for (kind, operation) in [
            (Kind::Designer, "designer.preview"),
            (Kind::Beats, "drums.preview"),
        ] {
            let mut c = open(kind);
            let requests = c.key(Key::Char(' '), context(), true).unwrap();
            assert_eq!(intent(&requests[0]).operation, operation);
            assert!(c.playing);
            if kind == Kind::Designer {
                cmd(&mut c, "sound tone 0.7").unwrap();
            } else {
                cmd(&mut c, "beat swing 0.2").unwrap();
            }
            let update = c.tick(context());
            assert_eq!(update.len(), 1);
            assert_eq!(intent(&update[0]).operation, operation);
            assert!(c.playing);
            assert!(c.tick(context()).is_empty());
            assert!(c
                .close(context())
                .iter()
                .all(|r| !intent(r).operation.starts_with("transport.")));
        }
    }
    #[test]
    fn audition_role_is_checked_before_state_changes() {
        let mut c = open(Kind::Designer);
        assert!(c.key(Key::Char(' '), context(), false).is_err());
        assert!(!c.playing);
        c.focus = Focus::Keyboard;
        assert!(c.key(Key::Char('a'), context(), false).is_err());
        assert!(c.offs.is_empty());
    }
    #[test]
    fn live_keyboard_has_bounded_note_off_and_records_local_notes() {
        let mut c = open(Kind::Designer);
        c.focus = Focus::Keyboard;
        c.record = true;
        let before = c.sound.as_ref().unwrap().notes.as_ref().unwrap().len();
        let r = c.key(Key::Char('a'), context(), true).unwrap();
        assert_eq!(intent(&r[0]).operation, "designer.noteOn");
        assert_eq!(intent(&r[0]).integers["pitch"], 60);
        assert_eq!(
            c.sound.as_ref().unwrap().notes.as_ref().unwrap().len(),
            before + 1
        );
        c.offs.insert(60, Instant::now() - Duration::from_millis(1));
        let r = c.tick(context());
        assert_eq!(intent(&r[0]).operation, "designer.noteOff");
        assert!(c.offs.is_empty());
        assert!(c.tick(context()).is_empty());
    }
    #[test]
    fn typed_commit_preserves_exact_seed_and_native_identifiers() {
        let mut c = open(Kind::Designer);
        cmd(&mut c, "seed 18446744073709551615").unwrap();
        cmd(&mut c, "layer-add 0").unwrap();
        cmd(&mut c, "layer-set 0 organicSeed 18446744073709551614").unwrap();
        assert!(cmd(&mut c, "layer-set 0 minimumVelocity 0.9").is_ok());
        assert!(cmd(&mut c, "layer-set 0 maximumVelocity 0.2").is_err());
        let r = cmd(&mut c, "insert").unwrap().unwrap().0;
        let i = intent(&r);
        assert_eq!(i.strings["trackID"], context().track_id.unwrap());
        assert_eq!(i.strings["clipID"], context().clip_id.unwrap());
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(i.payload.as_ref().unwrap())
            .unwrap();
        let decoded: SoundDesignPattern = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            decoded.organic.unwrap().seed.as_deref(),
            Some("18446744073709551615")
        );
        assert_eq!(
            decoded.layers.unwrap()[0].organic_seed.as_deref(),
            Some("18446744073709551614")
        );
    }
    #[test]
    fn controller_edits_replace_points_and_undo_privately() {
        let mut c = open(Kind::Designer);
        cmd(&mut c, "controller expression 2 0.7").unwrap();
        cmd(&mut c, "controller expression 2 0.3").unwrap();
        assert_eq!(
            c.sound
                .as_ref()
                .unwrap()
                .controllers
                .as_ref()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            c.sound.as_ref().unwrap().controllers.as_ref().unwrap()[0].value,
            0.3
        );
        cmd(&mut c, "undo").unwrap();
        assert_eq!(
            c.sound.as_ref().unwrap().controllers.as_ref().unwrap()[0].value,
            0.7
        );
        assert!(cmd(&mut c, "controller expression 5 0.2").is_err());
        assert!(cmd(&mut c, "sound beats 0.5").is_err());
    }
    #[test]
    fn beat_grid_remap_keeps_strongest_hit_and_crops_hidden_hits() {
        let mut c = open(Kind::Beats);
        let pattern = c.drums.as_mut().unwrap();
        pattern.steps.push(DrumStep {
            voice: DrumVoice::Kick,
            index: 1,
            velocity: 0.5,
            pan: 0.,
            probability: 1.,
            articulation: None,
        });
        pattern.steps.push(DrumStep {
            voice: DrumVoice::Snare,
            index: 30,
            velocity: 0.7,
            pan: 0.,
            probability: 1.,
            articulation: None,
        });
        cmd(&mut c, "beat stepsPerBeat 2").unwrap();
        assert_eq!(
            c.drums
                .as_ref()
                .unwrap()
                .steps
                .iter()
                .filter(|h| h.voice == DrumVoice::Kick)
                .count(),
            1
        );
        assert_eq!(
            c.drums
                .as_ref()
                .unwrap()
                .steps
                .iter()
                .find(|h| h.voice == DrumVoice::Kick)
                .unwrap()
                .velocity,
            0.8
        );
        cmd(&mut c, "beat bars 1").unwrap();
        assert!(c.drums.as_ref().unwrap().steps.iter().all(|h| h.index < 8));
        assert!(cmd(&mut c, "beat variationCycles 3").is_err());
        assert!(cmd(&mut c, "lane tuning 13").is_err());
        c.step = 1;
        assert!(cmd(&mut c, "hit articulation rimshot").is_err());
        cmd(&mut c, "hit probability 0.5").unwrap();
    }
    #[test]
    fn mouse_uses_rendered_scrolled_beat_window() {
        let mut c = open(Kind::Beats);
        cmd(&mut c, "beat bars 8").unwrap();
        c.step = 100;
        let mut canvas = Canvas::new(40, 24);
        c.draw(&mut canvas);
        let start = c.beat_window_start();
        assert!(start > 0);
        c.key(
            Key::Mouse {
                button: 0,
                column: 16,
                row: 6,
                released: false,
            },
            context(),
            false,
        )
        .unwrap();
        assert_eq!(c.step, start + 2);
        assert!(c
            .drums
            .as_ref()
            .unwrap()
            .steps
            .iter()
            .any(|h| h.voice == DrumVoice::Kick && h.index == c.step as i64));
    }
    #[test]
    fn capability_library_and_parameter_focus_are_readable_and_local() {
        let mut c = open(Kind::Beats);
        assert!(cmd(&mut c, "kits").unwrap().is_none());
        assert!(c.catalog[0].contains("Acoustic"));
        c.key(Key::Tab, context(), false).unwrap();
        c.key(Key::Tab, context(), false).unwrap();
        assert_eq!(c.focus, Focus::Sound);
        assert!(c.key(Key::Char('l'), context(), false).unwrap().is_empty());
        assert!(c.drums.as_ref().unwrap().bpm > 120.);
        let mut canvas = Canvas::new(40, 24);
        c.draw(&mut canvas);
        assert_eq!(canvas.cells.len(), 960);
    }
    fn file_request(c: &mut Creative, text: &str) -> crate::localfiles::Request {
        c.local_command(
            &text
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
        )
        .unwrap()
        .unwrap()
    }
    #[test]
    fn local_preset_receipt_preserves_sequence_and_undo_and_ignores_closed_workspace() {
        let mut c = open(Kind::Designer);
        let old = c.sound.clone().unwrap();
        let r = file_request(&mut c, "preset-load local.json");
        let mut next = old.clone();
        next.tone = Some(0.2);
        next.notes = Some(vec![]);
        next.controllers = Some(vec![]);
        next.bpm = Some(88.);
        let response = crate::localfiles::Response {
            workspace: r.workspace,
            id: r.id,
            epoch: r.epoch,
            result: Ok(crate::localfiles::Data::Sound(Box::new(next))),
        };
        let mut other = open(Kind::Designer);
        other.local_response(&response);
        assert_eq!(other.sound, Some(old.clone()));
        c.local_response(&response);
        let applied = c.sound.as_ref().unwrap();
        assert_eq!(applied.tone, Some(0.2));
        assert_eq!(applied.notes, old.notes);
        assert_eq!(applied.controllers, old.controllers);
        assert_eq!(applied.bpm, old.bpm);
        cmd(&mut c, "undo").unwrap();
        assert_eq!(c.sound, Some(old));
    }
    #[test]
    fn local_preset_pending_edit_or_unsent_job_cannot_overwrite_new_sound() {
        let mut c = open(Kind::Designer);
        let r = file_request(&mut c, "preset-load local.json");
        let p = c.sound.clone().unwrap();
        cmd(&mut c, "sound tone 0.3").unwrap();
        c.local_response(&crate::localfiles::Response {
            workspace: r.workspace,
            id: r.id,
            epoch: r.epoch,
            result: Ok(crate::localfiles::Data::Sound(Box::new(p))),
        });
        assert_eq!(c.sound.as_ref().unwrap().tone, Some(0.3));
        assert!(c.status.contains("not applied"));
        let r = file_request(&mut c, "preset-load local.json");
        let w = crate::worker::Work::LocalFile(r);
        let failure = w.failure(false, "Local queue full".into());
        c.failure(failure.tag.as_ref().unwrap(), &failure.message);
        assert!(c.pending_file.is_none());
        assert_eq!(c.status, "Local queue full");
    }
    #[test]
    fn midi_load_only_lists_parts_and_explicit_excerpt_is_private_undoable() {
        let mut c = open(Kind::Designer);
        let old = c.sound.clone().unwrap();
        let r = file_request(&mut c, "midi-load local.mid");
        let mut input = old.clone();
        input.beats = Some(64.);
        input.notes.as_mut().unwrap()[0].start_beat = 32.;
        let file = crate::midi::parse(&crate::midi::encode(&input).unwrap()).unwrap();
        c.local_response(&crate::localfiles::Response {
            workspace: r.workspace,
            id: r.id,
            epoch: r.epoch,
            result: Ok(crate::localfiles::Data::MIDI(std::sync::Arc::new(file))),
        });
        assert_eq!(c.sound, Some(old.clone()));
        assert!(c.catalog[0].contains("64.000 beats"));
        assert!(c
            .local_command(&["midi-excerpt", "0", "32", "4"].map(str::to_owned))
            .unwrap()
            .is_none());
        assert_eq!(
            c.sound.as_ref().unwrap().notes.as_ref().unwrap()[0].start_beat,
            0.
        );
        assert_eq!(c.sound.as_ref().unwrap().instrument, old.instrument);
        cmd(&mut c, "undo").unwrap();
        assert_eq!(c.sound, Some(old));
        assert!(open(Kind::Beats)
            .local_command(&["midi-load", "local.mid"].map(str::to_owned))
            .is_err());
    }
}
