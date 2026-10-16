use crate::{
    input::Key,
    render::Canvas,
    worker::{Shared, Work},
};
use soundroom_client::{models::*, state::ViewState};
use std::{collections::BTreeMap, sync::Arc};
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Panel {
    Arrange,
    Effects,
    EffectPresets,
    Routing,
    Help,
    Projects,
    Clip,
    Files,
}
pub struct App {
    pub view: ViewState,
    pub row: usize,
    pub top: usize,
    pub beats: bool,
    pub snap: bool,
    pub follow: bool,
    pub range: Option<(i64, i64)>,
    pub panel: Panel,
    pub effect: usize,
    pub parameter: usize,
    pub command: Option<String>,
    pub status: String,
    pub quit: bool,
    pub shared: Shared,
    pub width: usize,
    pub height: usize,
    pending_revision: Option<i64>,
    uncertain_pending: bool,
    indices: Vec<crate::timeline::ClipIndex>,
    assets: BTreeMap<String, usize>,
    projects: Vec<String>,
    count: u32,
    prefix: Option<char>,
    clipboard: Option<crate::editing::Clipboard>,
    track_clipboard: Option<crate::editing::TrackClipboard>,
    pub creative: Option<crate::creative::Creative>,
    pub automation: Option<crate::automation::Automation>,
    pub pitch: Option<crate::pitch::Pitch>,
    pub master: Option<crate::master::Master>,
    creative_context: Option<RemoteEditorContext>,
    creative_revision: Option<i64>,
    effect_library: Vec<crate::localfiles::SavedEffectPreset>,
    pending_effect_file: Option<String>,
    pending_saved_preset: Option<crate::localfiles::SavedEffectPreset>,
    preset_row: usize,
}
impl App {
    pub fn new() -> Self {
        Self {
            view: ViewState::default(),
            row: 0,
            top: 0,
            beats: true,
            snap: true,
            follow: false,
            range: None,
            panel: Panel::Arrange,
            effect: 0,
            parameter: 0,
            command: None,
            status: "Press ? for shortcuts, : for commands.".into(),
            quit: false,
            shared: Shared::default(),
            width: 100,
            height: 30,
            pending_revision: None,
            uncertain_pending: false,
            indices: vec![],
            assets: BTreeMap::new(),
            projects: vec![],
            count: 0,
            prefix: None,
            clipboard: None,
            track_clipboard: None,
            creative: None,
            automation: None,
            pitch: None,
            master: None,
            creative_context: None,
            creative_revision: None,
            effect_library: vec![],
            pending_effect_file: None,
            pending_saved_preset: None,
            preset_row: 0,
        }
    }
    pub fn install(&mut self, shared: Shared) {
        let changed = match (&self.shared.snapshot, &shared.snapshot) {
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            (None, None) => false,
            _ => true,
        };
        for event in &shared.events {
            if let RemoteEvent::Audition {
                kind,
                owner_id,
                playing,
                preparing,
                ..
            } = event
            {
                if let Some(creative) = self.creative.as_mut() {
                    let expected = if creative.kind == crate::creative::Kind::Designer {
                        "designer"
                    } else {
                        "drums"
                    };
                    if kind == expected {
                        creative.playing = (*playing || *preparing)
                            && owner_id.as_ref() == shared.client_id.as_ref();
                    }
                }
            }
            if let RemoteEvent::Audition {
                kind,
                owner_id,
                playing,
                preparing,
                ..
            } = event
            {
                if kind == "pitch" {
                    if let Some(pitch) = self.pitch.as_mut() {
                        pitch.playing = (*playing || *preparing)
                            && owner_id.as_ref() == shared.client_id.as_ref();
                    }
                }
            }
            if let RemoteEvent::Projects { projects } = event {
                self.projects = projects.clone();
                self.panel = Panel::Projects;
            }
        }
        if shared
            .transfer
            .as_ref()
            .is_some_and(|t| t.active && t.importing)
            && self.pending_revision.is_none()
        {
            self.pending_revision = Some(-1);
        }
        for reply in &shared.mutation_receipts {
            if reply.status != RemoteReplyStatus::Accepted {
                self.pending_revision = None;
            } else {
                self.pending_revision = Some(reply.revision);
            }
        }
        for failure in &shared.failures {
            self.work_failed(failure);
        }
        if self.uncertain_pending && shared.fresh_snapshot && shared.connected {
            self.pending_revision = None;
            self.uncertain_pending = false;
        }
        self.shared = shared;
        if let Some(pitch) = self.pitch.as_mut() {
            let track = self
                .shared
                .snapshot
                .as_ref()
                .and_then(|s| s.session.tracks.as_ref())
                .and_then(|ts| ts.iter().find(|t| t.id.as_deref() == Some(&pitch.track_id)));
            let revision = self
                .shared
                .snapshot
                .as_ref()
                .and_then(|s| s.track_revisions.get(&pitch.track_id))
                .copied();
            if let Err(error) = pitch.install_pending(track, revision) {
                pitch.busy = false;
                pitch.status = error;
            }
            for response in &self.shared.responses {
                if let Err(error) = pitch.response(response, track, revision) {
                    pitch.busy = false;
                    pitch.status = error;
                }
            }
        }
        if let (Some(master), Some(snapshot)) =
            (self.master.as_mut(), self.shared.snapshot.as_ref())
        {
            let metadata = self
                .shared
                .capabilities
                .as_ref()
                .map_or(&[][..], |c| c.effects.as_slice());
            if let Err(error) = master.install(snapshot) {
                master.busy = false;
                master.status = error;
            }
            for response in &self.shared.responses {
                if let Err(error) = master.response(response, snapshot, metadata) {
                    master.busy = false;
                    master.status = error;
                }
            }
        }
        if let Some(creative) = self.creative.as_mut() {
            for response in &self.shared.responses {
                creative.response(response);
            }
            for response in &self.shared.local_files {
                creative.local_response(response);
            }
        }
        for response in self.shared.local_files.clone() {
            if response.workspace == "effect-library"
                && self.pending_effect_file.as_ref() == Some(&response.id)
            {
                self.pending_effect_file = None;
                match response.result {
                    Err(e) => {
                        self.pending_saved_preset = None;
                        self.status = e
                    }
                    Ok(crate::localfiles::Data::Saved) => {
                        if let Some(p) = self.pending_saved_preset.take() {
                            self.effect_library.push(p);
                        }
                        self.status = "Effect preset file saved locally; :fx-custom-list opens the private library.".into()
                    }
                    Ok(crate::localfiles::Data::Effects(presets)) => {
                        if self.effect_library.len() + presets.len() > 128 {
                            self.status="Private effect library would exceed 128 entries; remove entries first.".into();
                            continue;
                        }
                        let mut candidate = self.effect_library.clone();
                        for incoming in presets.iter() {
                            let mut p = incoming.clone();
                            let base = p.name.trim().to_string();
                            let mut name = base.clone();
                            let mut suffix = 2;
                            while candidate.iter().any(|s| {
                                s.kind == p.kind && s.name.to_lowercase() == name.to_lowercase()
                            }) {
                                name = format!(
                                    "{} ({suffix})",
                                    base.chars().take(68).collect::<String>()
                                );
                                suffix += 1;
                            }
                            p.name = name;
                            p.id = match soundroom_client::auth::new_id() {
                                Ok(id) => id,
                                Err(e) => {
                                    self.status = e.to_string();
                                    break;
                                }
                            };
                            candidate.push(p);
                        }
                        if candidate.len() != self.effect_library.len() + presets.len() {
                            continue;
                        }
                        self.effect_library = candidate;
                        self.panel = Panel::EffectPresets;
                        self.preset_row = 0;
                        self.status=format!("{} presets loaded privately. :fx-custom index applies a reviewed preset.",presets.len());
                    }
                    _ => {}
                }
            }
        }
        if self.master.as_ref().is_some_and(|m| m.applied) {
            self.master = None;
        }
        if self.pending_revision.is_some_and(|revision| {
            revision >= 0
                && self
                    .shared
                    .snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.revision >= revision)
        }) {
            self.pending_revision = None;
        }
        if !changed {
            return;
        }
        self.indices = self
            .tracks()
            .iter()
            .map(|t| crate::timeline::ClipIndex::new(t.clips.as_deref().unwrap_or(&[])))
            .collect();
        self.assets = self
            .shared
            .snapshot
            .as_ref()
            .map(|s| {
                s.assets
                    .iter()
                    .enumerate()
                    .map(|(i, a)| (a.id.clone(), i))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(index) = self.view.selected_track.as_ref().and_then(|id| {
            self.tracks().iter().position(|track| {
                track
                    .id
                    .as_ref()
                    .is_some_and(|other| other.eq_ignore_ascii_case(id))
            })
        }) {
            self.row = index;
        }
        let n = self.tracks().len();
        if n > 0 {
            self.row = self.row.min(n - 1);
            self.view.selected_track = self.track().and_then(|t| t.id.clone());
        } else {
            self.row = 0;
            self.view.selected_track = None;
        }
        self.top = self.top.min(self.row);
        self.view.selected_clips.retain(|id| {
            self.shared.snapshot.as_ref().is_some_and(|s| {
                s.session.tracks.as_ref().is_some_and(|ts| {
                    ts.iter().any(|t| {
                        t.clips
                            .as_ref()
                            .is_some_and(|cs| cs.iter().any(|c| &c.id == id))
                    })
                })
            })
        });
    }
    pub fn tracks(&self) -> &[Track] {
        self.shared
            .snapshot
            .as_ref()
            .and_then(|s| s.session.tracks.as_deref())
            .unwrap_or(&[])
    }
    pub fn track(&self) -> Option<&Track> {
        self.tracks().get(self.row)
    }
    pub fn sample_rate(&self) -> f64 {
        self.shared
            .capabilities
            .as_ref()
            .map_or(48000., |c| c.sample_rate)
    }
    pub fn bpm(&self) -> f64 {
        self.shared
            .snapshot
            .as_ref()
            .and_then(|s| s.session.timing.as_ref())
            .map_or(120., |t| t.tempo)
    }
    pub fn step(&self) -> i64 {
        if self.beats {
            (self.sample_rate() * 60. / self.bpm() / 4.).round() as i64
        } else {
            (self.sample_rate() / 10.).round() as i64
        }
        .max(1)
    }
    pub fn frames_per_column(&self) -> f64 {
        self.sample_rate() * 0.2 / self.view.zoom.clamp(0.01, 1000.)
    }
    pub fn key(&mut self, key: Key) -> Vec<Work> {
        let mut out = vec![];
        if let Some(line) = self.command.as_mut() {
            match key {
                Key::Escape => self.command = None,
                Key::Enter => {
                    let text = self.command.take().unwrap();
                    match self.parse_command(&text) {
                        Ok(work) => out.extend(work),
                        Err(e) => self.status = e,
                    }
                }
                Key::Backspace => {
                    line.pop();
                }
                Key::Char(c) => {
                    if line.len() < 4096 {
                        line.push(c);
                    }
                }
                Key::Paste(s) => {
                    if line.len() + s.len() <= 4096 {
                        line.push_str(&s);
                    }
                }
                Key::Ctrl('c') => self.command = None,
                _ => {}
            }
            return out;
        }
        if self.master.is_some() {
            match key {
                Key::Char(':') => {
                    self.command = Some(String::new());
                    return out;
                }
                Key::Escape | Key::Ctrl('c') | Key::Char('t') => {
                    if !self.master.as_mut().unwrap().close_nested() {
                        self.master = None;
                    }
                    return out;
                }
                Key::Char('q') => {
                    self.master = None;
                    self.quit = true;
                    return out;
                }
                Key::Char(' ') => {
                    let playing = self.shared.transport.as_ref().map_or_else(
                        || {
                            self.shared
                                .snapshot
                                .as_ref()
                                .is_some_and(|s| s.transport.is_playing)
                        },
                        |(t, _)| t.is_playing,
                    );
                    self.push(
                        self.editor(if playing {
                            "transport.pause"
                        } else {
                            "transport.play"
                        }),
                        true,
                        &mut out,
                    );
                    return out;
                }
                _ => {}
            }
            let cap = self.shared.capabilities.clone();
            let metadata = cap.as_ref().map_or(&[][..], |c| c.effects.as_slice());
            match self.master.as_mut().unwrap().key(key, metadata) {
                Ok(true) => {
                    if let Err(error) = self.commit_master(&mut out) {
                        self.status = error;
                    }
                }
                Ok(false) => {}
                Err(error) => self.status = error,
            }
            return out;
        }
        if self.pitch.is_some() {
            match key {
                Key::Char(':') => {
                    self.command = Some(String::new());
                    return out;
                }
                Key::Escape | Key::Ctrl('c') | Key::Char('t') => {
                    self.close_pitch(&mut out);
                    return out;
                }
                Key::Char('q') => {
                    self.close_pitch(&mut out);
                    self.quit = true;
                    return out;
                }
                Key::Char(' ') => {
                    match self.pitch_operation("pitch.render", true, false, None, &mut out) {
                        Ok(()) => {}
                        Err(e) => self.status = e,
                    };
                    return out;
                }
                _ => {}
            }
            match self.pitch.as_mut().unwrap().key(key) {
                Ok(true) => self.commit_pitch(&mut out),
                Ok(false) => {}
                Err(e) => self.status = e,
            }
            return out;
        }
        if self.automation.is_some() {
            match key {
                Key::Char(':') => {
                    self.command = Some(String::new());
                    return out;
                }
                Key::Escape | Key::Ctrl('c') | Key::Char('t') => {
                    self.automation = None;
                    return out;
                }
                Key::Char('q') => {
                    self.quit = true;
                    return out;
                }
                Key::Char(' ') => {
                    let operation = if self
                        .shared
                        .transport
                        .as_ref()
                        .is_some_and(|(t, _)| t.is_playing)
                    {
                        "transport.pause"
                    } else {
                        "transport.play"
                    };
                    self.push(self.editor(operation), true, &mut out);
                    return out;
                }
                _ => {}
            }
            match self.automation.as_mut().unwrap().key(key) {
                Ok(true) => self.commit_automation(&mut out),
                Ok(false) => {}
                Err(e) => self.status = e,
            }
            return out;
        }
        if self.creative.is_some() {
            if key == Key::Char('q')
                && self
                    .creative
                    .as_ref()
                    .is_some_and(|c| c.focus != crate::creative::Focus::Keyboard)
            {
                self.quit = true;
            }
            if key == Key::Char(':') {
                self.command = Some(String::new());
                return out;
            }
            let context = self.creative_context.clone().unwrap();
            if matches!(key, Key::Escape | Key::Ctrl('c') | Key::Char('T'))
                || key == Key::Char('t')
                    && self
                        .creative
                        .as_ref()
                        .is_some_and(|c| c.focus != crate::creative::Focus::Keyboard)
                || self.quit
            {
                let commands = self.creative.as_mut().unwrap().close(context);
                self.creative = None;
                for command in commands {
                    self.push(command, true, &mut out);
                }
                return out;
            }
            let operator =
                matches!(self.shared.role, Some(RemoteRole::Operator)) && self.shared.connected;
            match self.creative.as_mut().unwrap().key(key, context, operator) {
                Ok(commands) => {
                    for command in commands {
                        self.push(command, true, &mut out);
                    }
                }
                Err(error) => self.status = error,
            }
            return out;
        }
        if let Key::Char(digit @ '0'..='9') = key {
            if digit != '0' || self.count > 0 {
                self.count = (self.count * 10 + digit.to_digit(10).unwrap()).min(9999);
                return out;
            }
        }
        let count = self.count.max(1);
        self.count = 0;
        if self.panel == Panel::EffectPresets
            && !matches!(
                key,
                Key::Char(':') | Key::Char(' ') | Key::Char('q') | Key::Ctrl('c')
            )
        {
            self.prefix = None;
            let last = self.effect_library.len().saturating_sub(1);
            match key {
                Key::Escape | Key::Char('t') | Key::Char('e') | Key::Tab => {
                    self.panel = Panel::Effects
                }
                Key::Char('j') | Key::Down => {
                    self.preset_row = self.preset_row.saturating_add(count as usize).min(last)
                }
                Key::Char('k') | Key::Up => {
                    self.preset_row = self.preset_row.saturating_sub(count as usize)
                }
                Key::PageDown => {
                    self.preset_row = self
                        .preset_row
                        .saturating_add(self.height.saturating_sub(9))
                        .min(last)
                }
                Key::PageUp => {
                    self.preset_row = self
                        .preset_row
                        .saturating_sub(self.height.saturating_sub(9))
                }
                Key::Home | Key::Char('g') => self.preset_row = 0,
                Key::End | Key::Char('G') => self.preset_row = last,
                Key::Enter => {
                    if let Err(e) = self.effect_library_command(
                        &["fx-custom".into(), self.preset_row.to_string()],
                        &mut out,
                    ) {
                        self.status = e;
                    }
                    return out;
                }
                Key::Mouse { button: 64, .. } => {
                    self.preset_row = self.preset_row.saturating_sub(3)
                }
                Key::Mouse { button: 65, .. } => {
                    self.preset_row = self.preset_row.saturating_add(3).min(last)
                }
                Key::Mouse {
                    button: 0,
                    column,
                    row,
                    released: false,
                } => {
                    let start = self
                        .preset_row
                        .saturating_sub(self.height.saturating_sub(10) / 2);
                    if usize::from(column) >= self.width.saturating_sub(40.min(self.width / 2)) + 2
                        && row >= 5
                    {
                        self.preset_row = start.saturating_add(usize::from(row - 5)).min(last)
                    }
                }
                _ => {}
            }
            if let Some(p) = self.effect_library.get(self.preset_row) {
                self.status = format!(
                    "{} / {:?}{}",
                    p.name,
                    p.kind,
                    if p.legacy_amount.is_some() {
                        " / legacy processor"
                    } else {
                        ""
                    }
                );
            }
            return out;
        }
        if let Some(prefix) = self.prefix.take() {
            if prefix == 'g' && key == Key::Char('g') {
                self.set_cursor(0);
                self.view.horizontal_frame = 0;
                return out;
            }
            if prefix == 'd' && key == Key::Char('d') {
                self.delete_selection(&mut out);
                return out;
            }
        }
        match key {
            Key::Char(':') => self.command = Some(String::new()),
            Key::Char('?') => {
                self.panel = if self.panel == Panel::Help {
                    Panel::Arrange
                } else {
                    Panel::Help
                }
            }
            Key::Char('q') | Key::Ctrl('c') => self.quit = true,
            Key::Char('j') | Key::Down => self.move_row(count as isize),
            Key::Char('k') | Key::Up => self.move_row(-(count as isize)),
            Key::PageDown => self.move_row((self.height.saturating_sub(8) / 2) as isize),
            Key::PageUp => self.move_row(-((self.height.saturating_sub(8) / 2) as isize)),
            Key::Char('g') | Key::Char('d') => {
                if let Key::Char(ch) = key {
                    self.prefix = Some(ch);
                }
            }
            Key::Home => {
                self.set_cursor(0);
                self.view.horizontal_frame = 0;
            }
            Key::Char('G') | Key::End => {
                let end = self
                    .tracks()
                    .iter()
                    .flat_map(|t| t.clips.as_deref().unwrap_or(&[]))
                    .map(|c| c.start + c.length)
                    .max()
                    .unwrap_or(0);
                self.view.cursor_frame = end;
                self.ensure_cursor_visible();
            }
            Key::Char('w') | Key::Char('b') => self.edge(key == Key::Char('w'), count),
            Key::Char('y') => self.copy_selection(),
            Key::Char('p') => {
                if let Err(error) = self.paste_clipboard(&mut out) {
                    self.status = error;
                }
            }
            Key::Char('h') | Key::Left => {
                if self.panel == Panel::Effects {
                    self.adjust_parameter(-1., &mut out);
                } else {
                    self.move_cursor(-(count as i64));
                }
            }
            Key::Char('l') | Key::Right => {
                if self.panel == Panel::Effects {
                    self.adjust_parameter(1., &mut out);
                } else {
                    self.move_cursor(count as i64);
                }
            }
            Key::Char('H') => self.scroll(-1),
            Key::Char('L') => self.scroll(1),
            Key::Char('+') | Key::Char('=') => self.zoom(1.25),
            Key::Char('-') => self.zoom(0.8),
            Key::Char('B') if self.panel != Panel::Effects => self.beats = !self.beats,
            Key::Char('n') => self.snap = !self.snap,
            Key::Char('f') => self.panel = Panel::Effects,
            Key::Char('t') => self.panel = Panel::Arrange,
            Key::Char('F') => self.fit(),
            Key::Char('v') => {
                self.range = Some((self.view.cursor_frame, self.view.cursor_frame));
                self.status = "Range: h/l or click the ruler; v resets its anchor.".into();
            }
            Key::Escape => {
                self.range = None;
                self.view.selected_clips.clear();
                self.panel = Panel::Arrange;
            }
            Key::Char('[') | Key::Char(']') => self.nudge_clip(
                if key == Key::Char('[') {
                    -(count as i64)
                } else {
                    count as i64
                },
                &mut out,
            ),
            Key::Tab => {
                self.panel = match self.panel {
                    Panel::Arrange => Panel::Effects,
                    Panel::Effects => Panel::Routing,
                    _ => Panel::Arrange,
                };
            }
            Key::Char('e') => self.panel = Panel::Effects,
            Key::Char('i') => self.panel = Panel::Routing,
            Key::Char(' ') => {
                let playing = self
                    .shared
                    .transport
                    .as_ref()
                    .is_some_and(|(t, _)| t.is_playing);
                let command = if playing {
                    self.editor("transport.pause")
                } else {
                    self.editor("transport.play")
                };
                self.push(command, true, &mut out);
            }
            Key::Char('0') => {
                self.view.cursor_frame = 0;
                self.view.horizontal_frame = 0;
            }
            Key::Char('R') => self.push(self.editor("transport.record"), true, &mut out),

            Key::Char('P') => self.push(RemoteCommand::TakeTransport, true, &mut out),
            Key::Char('m') | Key::Char('s') | Key::Char('a') => {
                if let Some(t) = self.track() {
                    if let Some(id) = t.id.clone() {
                        let command = match key {
                            Key::Char('m') => RemoteCommand::SetTrackMute {
                                track_id: id,
                                muted: !t.muted.unwrap_or(false),
                            },
                            Key::Char('s') => RemoteCommand::SetTrackSolo {
                                track_id: id,
                                solo: !t.solo.unwrap_or(false),
                            },
                            _ => RemoteCommand::SetTrackArm {
                                track_id: id,
                                armed: !t.armed.unwrap_or(false),
                            },
                        };
                        self.push(command, false, &mut out);
                    }
                }
            }
            Key::Char('S') | Key::Char('x') => {
                if let Some(c) = self.selected_clip() {
                    if let Some(id) = self.track().and_then(|t| t.id.clone()) {
                        self.push(
                            RemoteCommand::SplitClip {
                                track_id: id,
                                clip_id: c.id.clone(),
                                frame: self.view.cursor_frame,
                            },
                            false,
                            &mut out,
                        );
                    }
                }
            }
            Key::Enter => self.push(
                RemoteCommand::Seek {
                    frame: self.view.cursor_frame,
                },
                true,
                &mut out,
            ),
            Key::Char('C') => self.push(self.editor("clips.combine"), false, &mut out),
            Key::Char('r') => {
                let recording = self
                    .shared
                    .transport
                    .as_ref()
                    .is_some_and(|(t, _)| t.is_recording);
                self.push(
                    self.editor(if recording {
                        "transport.finishRecording"
                    } else {
                        "transport.record"
                    }),
                    true,
                    &mut out,
                );
            }
            Key::Delete => {
                if let Some(id) = self.track().and_then(|t| t.id.clone()) {
                    self.push(
                        RemoteCommand::DeleteClips {
                            track_id: id,
                            clip_ids: self.selected_ids().into_iter().collect(),
                        },
                        false,
                        &mut out,
                    );
                }
            }
            Key::Char('u') => self.push(RemoteCommand::Undo, false, &mut out),
            Key::Ctrl('r') => self.push(RemoteCommand::Redo, false, &mut out),
            Key::Char('J') if self.panel == Panel::Effects => {
                let count = self
                    .selected_effect_metadata()
                    .map_or(0, |m| m.parameters.len());
                self.parameter = self
                    .parameter
                    .saturating_add(1)
                    .min(count.saturating_sub(1));
            }
            Key::Char('K') if self.panel == Panel::Effects => {
                self.parameter = self.parameter.saturating_sub(1)
            }
            Key::Char('E') => {
                let count = self
                    .track()
                    .and_then(|t| t.realtime_effects.as_ref())
                    .map_or(0, Vec::len);
                self.effect = self.effect.saturating_add(1).min(count.saturating_sub(1));
                self.parameter = 0;
            }
            Key::Char('B') => {
                self.effect = self.effect.saturating_sub(1);
                self.parameter = 0;
            }
            Key::Mouse { button: 64, .. } => self.move_row(-2),
            Key::Mouse { button: 65, .. } => self.move_row(2),
            Key::Mouse {
                button: button @ (0 | 2),
                column,
                row,
                released: false,
            } => {
                let row = row as usize;
                let x = column as usize;
                if row >= 4 && row < self.height.saturating_sub(4) {
                    let index = self.top + (row - 4) / 2;
                    if index < self.tracks().len() {
                        self.row = index;
                    }
                    let right = self.width.saturating_sub(if self.panel == Panel::Arrange {
                        0
                    } else {
                        40.min(self.width / 2)
                    });
                    if x >= 34 && x <= right {
                        let frame = (self.view.horizontal_frame as f64
                            + (x - 34) as f64 * self.frames_per_column())
                        .round() as i64;
                        self.set_cursor(frame);
                        self.follow = false;
                        if let Some(c) = self
                            .track()
                            .and_then(|t| t.clips.as_ref())
                            .and_then(|cs| {
                                cs.iter()
                                    .find(|c| frame >= c.start && frame < c.start + c.length)
                            })
                            .map(|c| c.id.clone())
                        {
                            self.view.selected_clips.clear();
                            self.view.selected_clips.insert(c);
                            if button == 2 {
                                self.panel = Panel::Clip;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        self.view.selected_track = self.track().and_then(|t| t.id.clone());
        let visible = self.height.saturating_sub(8) / 2;
        if self.row < self.top {
            self.top = self.row;
        } else if visible > 0 && self.row >= self.top + visible {
            self.top = self.row + 1 - visible;
        }
        out
    }
    fn set_cursor(&mut self, frame: i64) {
        let f = frame.max(0);
        self.view.cursor_frame = if self.snap && self.beats {
            let division = self.sample_rate() * 60. / self.bpm() / 4.;
            ((f as f64 / division).round() * division).round() as i64
        } else if self.snap {
            ((f as f64 / self.step() as f64).round() * self.step() as f64).round() as i64
        } else {
            f
        };
        if let Some((start, _)) = self.range {
            self.range = Some((start, self.view.cursor_frame));
        }
    }
    fn move_cursor(&mut self, direction: i64) {
        self.follow = false;
        if self.beats && self.snap {
            let division = self.sample_rate() * 60. / self.bpm() / 4.;
            let boundary = (self.view.cursor_frame as f64 / division).round() + direction as f64;
            self.set_cursor((boundary.max(0.) * division).round() as i64);
        } else {
            self.set_cursor(
                self.view
                    .cursor_frame
                    .saturating_add(direction * self.step()),
            );
        }
        let columns = self.width.saturating_sub(34);
        let end = self
            .view
            .horizontal_frame
            .saturating_add((columns as f64 * self.frames_per_column()) as i64);
        if self.view.cursor_frame < self.view.horizontal_frame {
            self.view.horizontal_frame = self.view.cursor_frame;
        } else if self.view.cursor_frame > end {
            self.view.horizontal_frame = self
                .view
                .cursor_frame
                .saturating_sub((columns as f64 * self.frames_per_column() / 2.) as i64);
        }
    }
    fn move_row(&mut self, delta: isize) {
        self.row = self
            .row
            .saturating_add_signed(delta)
            .min(self.tracks().len().saturating_sub(1));
    }
    fn scroll(&mut self, direction: i64) {
        let delta = (self.width.saturating_sub(34) as f64 * self.frames_per_column() / 2.) as i64;
        self.view.horizontal_frame = self
            .view
            .horizontal_frame
            .saturating_add(direction * delta)
            .max(0);
        self.follow = false;
    }
    fn zoom(&mut self, factor: f64) {
        let anchor = self.view.cursor_frame;
        let relative = (anchor - self.view.horizontal_frame) as f64 / self.frames_per_column();
        self.view.zoom = (self.view.zoom * factor).clamp(0.01, 1000.);
        self.view.horizontal_frame =
            (anchor as f64 - relative * self.frames_per_column()).max(0.) as i64;
    }
    fn ensure_cursor_visible(&mut self) {
        let columns = self.width.saturating_sub(34);
        let span = (columns as f64 * self.frames_per_column()) as i64;
        if self.view.cursor_frame < self.view.horizontal_frame
            || self.view.cursor_frame > self.view.horizontal_frame + span
        {
            self.view.horizontal_frame = (self.view.cursor_frame - span / 2).max(0);
        }
        self.follow = false;
    }
    fn fit(&mut self) {
        let end = self
            .tracks()
            .iter()
            .flat_map(|t| t.clips.as_deref().unwrap_or(&[]))
            .map(|c| c.start + c.length)
            .max()
            .unwrap_or(self.sample_rate() as i64);
        self.view.zoom = (self.sample_rate() * 0.2 * self.width.saturating_sub(34) as f64
            / end.max(1) as f64)
            .clamp(0.01, 1000.);
        self.view.horizontal_frame = 0;
        self.follow = false;
    }
    fn edge(&mut self, forward: bool, count: u32) {
        let mut edges: Vec<_> = self
            .track()
            .into_iter()
            .flat_map(|t| t.clips.as_deref().unwrap_or(&[]))
            .flat_map(|c| [c.start, c.start + c.length])
            .collect();
        edges.sort_unstable();
        edges.dedup();
        for _ in 0..count {
            self.view.cursor_frame = if forward {
                edges
                    .iter()
                    .copied()
                    .find(|e| *e > self.view.cursor_frame)
                    .unwrap_or_else(|| edges.last().copied().unwrap_or(0))
            } else {
                edges
                    .iter()
                    .rev()
                    .copied()
                    .find(|e| *e < self.view.cursor_frame)
                    .unwrap_or(0)
            };
        }
        self.ensure_cursor_visible();
        if let Some((a, _)) = self.range {
            self.range = Some((a, self.view.cursor_frame));
        }
    }
    fn selected_ids(&self) -> std::collections::BTreeSet<String> {
        if !self.view.selected_clips.is_empty() {
            return self.view.selected_clips.clone();
        }
        let range = self.range.map(|(a, b)| (a.min(b), a.max(b)));
        self.track()
            .into_iter()
            .flat_map(|t| t.clips.as_deref().unwrap_or(&[]))
            .filter(|c| {
                range.map_or(
                    self.view.cursor_frame >= c.start
                        && self.view.cursor_frame < c.start + c.length,
                    |(a, b)| c.start < b && c.start + c.length > a,
                )
            })
            .map(|c| c.id.clone())
            .collect()
    }
    fn copy_selection(&mut self) {
        let ids = self.selected_ids();
        let Some(snapshot) = self.shared.snapshot.as_ref() else {
            return;
        };
        let Some(track) = self.track().and_then(|t| t.id.clone()) else {
            return;
        };
        if ids.is_empty() {
            self.status = "Place the cursor on a clip or select a range.".into();
            return;
        }
        self.status = format!("Copied {} clip reference(s) in this client.", ids.len());
        self.track_clipboard = None;
        self.clipboard = Some(crate::editing::Clipboard {
            server_id: snapshot.server_id.clone(),
            source_track: track,
            clips: ids,
            frozen: None,
            range: self
                .range
                .filter(|_| self.view.selected_clips.len() <= 1)
                .map(|(a, b)| (a.min(b), a.max(b)))
                .filter(|(a, b)| b > a),
        });
    }
    fn paste_clipboard(&mut self, out: &mut Vec<Work>) -> Result<(), String> {
        if self.track_clipboard.is_some() {
            return self.paste_track(out);
        }
        let clipboard = self
            .clipboard
            .clone()
            .ok_or("Clipboard is empty; y copies clips.")?;
        let snapshot = self
            .shared
            .snapshot
            .as_ref()
            .ok_or("Waiting for metadata.")?;
        let id = self
            .track()
            .and_then(|t| t.id.as_ref())
            .ok_or("Select a destination track.")?;
        let maximum = self
            .shared
            .capabilities
            .as_ref()
            .map_or(9_007_199_254_740_991, |c| c.max_timeline_frame);
        let track = clipboard.paste(snapshot, id, self.view.cursor_frame, maximum)?;
        let source_revision = snapshot
            .track_revisions
            .get(&clipboard.source_track.to_ascii_uppercase())
            .copied();
        self.push(
            RemoteCommand::EditTracks {
                edit: RemoteTrackEdit {
                    tracks: vec![track],
                    removed_track_ids: vec![],
                    track_order: None,
                    name: "Paste clips".into(),
                    gesture_id: None,
                },
            },
            false,
            out,
        );
        if let (Some(revision), Some(Work::Command { tracks, .. })) =
            (source_revision, out.last_mut())
        {
            tracks.insert(clipboard.source_track, revision);
        }
        Ok(())
    }
    fn transfer_command(&mut self, words: &[String], out: &mut Vec<Work>) -> Result<(), String> {
        let command = words[0].as_str();
        if command == "files" {
            self.panel = Panel::Files;
            return Ok(());
        }
        if command == "transfer" {
            self.status = self
                .shared
                .transfer
                .as_ref()
                .map_or("No local transfer.".into(), |v| {
                    format!(
                        "{}: {} {}/{} bytes{}",
                        v.label,
                        v.phase,
                        v.completed,
                        v.total,
                        v.request_id
                            .as_ref()
                            .map_or(String::new(), |id| format!("; request {id}"))
                    )
                });
            return Ok(());
        }
        if command == "transfer-cancel" {
            out.push(Work::CancelTransfer);
            return Ok(());
        }
        if !self.shared.connected {
            return Err("Connect and synchronize before transferring files.".into());
        }
        if self.shared.transfer.as_ref().is_some_and(|t| t.active) {
            return Err("A local transfer is active; :transfer-cancel stops it.".into());
        }
        let cap = self
            .shared
            .capabilities
            .as_ref()
            .ok_or("Waiting for host transfer capabilities.")?;
        let required = match command {
            "import" => &["beginUpload", "uploadChunk", "finishUpload"][..],
            "export" | "stems" => &["downloadArtifact"][..],
            _ => &["downloadArtifact"][..],
        };
        if required
            .iter()
            .any(|id| !cap.commands.iter().any(|c| c == id))
        {
            return Err("This host does not advertise the requested transfer workflow.".into());
        }
        if command != "download" && self.shared.role == Some(RemoteRole::Viewer) {
            return Err("Import and export require editor or operator permission.".into());
        }
        let arg = |i| {
            words.get(i).ok_or("Usage: import localPath [new]; export localPath [mixName]; download artifactNumber localPath.")
        };
        let request = match command {
            "import" => {
                let path = std::path::PathBuf::from(arg(1)?);
                let snapshot = self
                    .shared
                    .snapshot
                    .as_ref()
                    .ok_or("Waiting for project metadata.")?;
                let track_id = match words.get(2).map(String::as_str) {
                    None => self.view.selected_track.clone(),
                    Some("new") => None,
                    _ => return Err("Usage: import localPath [new]".into()),
                };
                let tracks = track_id
                    .as_ref()
                    .map(|id| {
                        snapshot
                            .track_revisions
                            .iter()
                            .find(|(key, _)| key.eq_ignore_ascii_case(id))
                            .map(|(_, v)| (id.clone(), *v))
                            .ok_or("Refresh the target track metadata.")
                    })
                    .transpose()?
                    .into_iter()
                    .collect();
                crate::transfers::TransferRequest::Import {
                    path,
                    track_id,
                    frame: self.view.cursor_frame,
                    revision: snapshot.revision,
                    tracks,
                    maximum_bytes: cap.max_upload_bytes.max(0) as u64,
                    chunk_bytes: cap.max_upload_chunk_bytes.max(0) as usize,
                }
            }
            "export" | "stems" => {
                let stems = command == "stems";
                let path = if stems {
                    None
                } else {
                    Some(std::path::PathBuf::from(arg(1)?))
                };
                let format = if stems {
                    arg(1)?.to_ascii_lowercase()
                } else {
                    path.as_ref()
                        .unwrap()
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("wav")
                        .to_ascii_lowercase()
                };
                let format = if format == "aif" {
                    "aiff".to_string()
                } else {
                    format
                };
                if !["wav", "aiff", "m4a"].contains(&format.as_str())
                    || !cap.codecs.iter().any(|c| {
                        c.eq_ignore_ascii_case(&format)
                            || (format == "wav" && c.to_ascii_lowercase().starts_with("wav-"))
                    })
                {
                    return Err(format!(
                        "Host does not advertise {format} export. Available codecs: {}.",
                        cap.codecs.join(", ")
                    ));
                }
                let scope = if stems {
                    words.get(2).map(String::as_str).unwrap_or("all")
                } else {
                    "mix"
                };
                let options = words
                    .iter()
                    .skip(if stems { 3 } else { 2 })
                    .collect::<Vec<_>>();
                let selection = options.iter().any(|s| s.as_str() == "selection");
                let tails = !options.iter().any(|s| s.as_str() == "no-tails");
                let names = options
                    .iter()
                    .filter(|s| !matches!(s.as_str(), "selection" | "no-tails"))
                    .collect::<Vec<_>>();
                if names.len() > 1 {
                    return Err("Quote the export name. Options: selection, no-tails.".into());
                }
                let name = names
                    .first()
                    .map(|s| (*s).to_string())
                    .or_else(|| {
                        self.shared
                            .snapshot
                            .as_ref()
                            .and_then(|s| s.session.name.clone())
                    })
                    .unwrap_or_else(|| "Mix".into());
                let recipe = stems || format != "wav" || selection || !tails;
                if recipe
                    && (!cap.commands.iter().any(|c| c == "editor")
                        || !cap.editor_operations.iter().any(|c| c == "project.export"))
                {
                    return Err("Host does not advertise export recipes (project.export).".into());
                }
                if !recipe {
                    if !cap.commands.iter().any(|c| c == "exportMix") {
                        return Err("Host does not advertise mix export.".into());
                    }
                    crate::transfers::TransferRequest::Export {
                        path: path.unwrap(),
                        name,
                    }
                } else {
                    let mut context = self.context();
                    context.clip_id = None;
                    context.clip_ids.clear();
                    context.effect_id = None;
                    if selection {
                        let (a, b) = self
                            .range
                            .map(|(a, b)| (a.min(b), a.max(b)))
                            .filter(|(a, b)| b > a)
                            .ok_or("Select an export range first (v then h/l).")?;
                        context.range_start = Some(a);
                        context.range_end = Some(b);
                        if context.track_id.is_none() {
                            return Err("Selection export needs a selected track context.".into());
                        }
                    } else {
                        context.range_start = None;
                        context.range_end = None;
                    }
                    let tracks = if stems {
                        Some(match scope {
                            "track" => vec![context.track_id.clone().ok_or("Select a track.")?],
                            "all" => self
                                .shared
                                .snapshot
                                .as_ref()
                                .and_then(|s| s.session.tracks.as_ref())
                                .map(|ts| ts.iter().filter_map(|t| t.id.clone()).collect())
                                .unwrap_or_default(),
                            _ => return Err("Stem scope must be all or track.".into()),
                        })
                    } else {
                        None
                    };
                    if tracks
                        .as_ref()
                        .is_some_and(|ts| ts.is_empty() || ts.len() > 16)
                    {
                        return Err(
                            "Stem exports support 1–16 tracks; choose track for larger sessions."
                                .into(),
                        );
                    }
                    crate::transfers::TransferRequest::ProjectExport {
                        path,
                        name,
                        format,
                        context,
                        tails,
                        tracks,
                    }
                }
            }
            _ => {
                let raw = arg(1)?;
                let artifact = raw
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .and_then(|n| self.shared.artifacts.iter().rev().nth(n))
                    .or_else(|| {
                        self.shared
                            .artifacts
                            .iter()
                            .find(|a| a.id.eq_ignore_ascii_case(raw))
                    })
                    .cloned()
                    .ok_or("Artifact not found; :files lists recent verified export manifests.")?;
                crate::transfers::TransferRequest::Download {
                    path: std::path::PathBuf::from(arg(2)?),
                    artifact,
                }
            }
        };
        out.push(Work::Transfer(request));
        self.status = "Local transfer queued; playback controls remain available.".into();
        Ok(())
    }
    fn maximum_frame(&self) -> i64 {
        self.shared
            .capabilities
            .as_ref()
            .map_or(17_280_000_000, |c| c.max_timeline_frame)
    }
    fn commit_arrangement(&mut self, track: Track, name: &str, out: &mut Vec<Work>) {
        self.push(
            RemoteCommand::EditTracks {
                edit: RemoteTrackEdit {
                    tracks: vec![track],
                    removed_track_ids: vec![],
                    track_order: None,
                    name: name.into(),
                    gesture_id: None,
                },
            },
            false,
            out,
        );
    }
    fn paste_track(&mut self, out: &mut Vec<Work>) -> Result<(), String> {
        let copy = self.track_clipboard.clone().ok_or("Copy a track first.")?;
        let snapshot = self
            .shared
            .snapshot
            .as_ref()
            .ok_or("Waiting for metadata.")?;
        let track = copy.paste(
            snapshot,
            self.view.cursor_frame,
            self.maximum_frame(),
            self.shared
                .capabilities
                .as_ref()
                .map_or(&[][..], |c| c.effects.as_slice()),
        )?;
        let new_id = track.id.clone().ok_or("Missing pasted track identity.")?;
        let mut order: Vec<_> = self.tracks().iter().filter_map(|t| t.id.clone()).collect();
        let insert = order
            .iter()
            .position(|id| Some(id) == self.view.selected_track.as_ref())
            .map_or(order.len(), |i| i + 1);
        order.insert(insert, new_id);
        self.push(
            RemoteCommand::EditTracks {
                edit: RemoteTrackEdit {
                    tracks: vec![track],
                    removed_track_ids: vec![],
                    track_order: Some(order),
                    name: if copy.automation {
                        "Paste track + automation"
                    } else {
                        "Paste track"
                    }
                    .into(),
                    gesture_id: None,
                },
            },
            false,
            out,
        );
        if let Some(Work::Command { tracks, .. }) = out.last_mut() {
            tracks.insert(
                copy.source.id.ok_or("Missing source identity.")?,
                copy.revision,
            );
        }
        Ok(())
    }
    fn arrange_command(&mut self, words: &[String], out: &mut Vec<Work>) -> Result<(), String> {
        let command = words[0].as_str();
        let arg = |i: usize| {
            words
                .get(i)
                .map(String::as_str)
                .ok_or("Missing argument; :clip shows editing commands.")
        };
        if command == "clip" || command == "history" {
            self.panel = Panel::Clip;
            return Ok(());
        }
        let original = self.track().cloned().ok_or("Select a track.")?;
        if command == "copy-track" {
            let automation = match words.get(1).map(String::as_str) {
                None => false,
                Some("automation") => true,
                _ => return Err("Usage: copy-track [automation]".into()),
            };
            let snapshot = self
                .shared
                .snapshot
                .as_ref()
                .ok_or("Waiting for metadata.")?;
            self.track_clipboard = Some(crate::editing::TrackClipboard::copy(
                snapshot, &original, automation,
            )?);
            self.clipboard = None;
            self.status = if automation {
                "Track and automation copied; p pastes at the cursor."
            } else {
                "Track copied; p pastes at the cursor."
            }
            .into();
            return Ok(());
        }
        if command == "cut" {
            if self.selected_ids().is_empty() {
                return Err("Select audio to cut.".into());
            }
            self.copy_selection();
            if self.clipboard.is_none() {
                return Err("Select audio to cut.".into());
            }
            if let Some(copy) = self.clipboard.as_mut() {
                copy.freeze(&original)?;
            }
            if let Some((a, b)) = self.range.filter(|_| self.view.selected_clips.len() <= 1) {
                self.commit_arrangement(
                    crate::editing::delete_range(&original, a.min(b), a.max(b))?,
                    "Cut audio",
                    out,
                );
            } else {
                self.delete_selection(out);
            }
            return Ok(());
        }
        let maximum = self.maximum_frame();
        let rate = self.sample_rate();
        let bpm = self.bpm();
        let position = |text: &str| crate::editing::position(text, rate, bpm, maximum);
        if command == "trim-range" {
            let (a, b) = self
                .range
                .map(|(a, b)| (a.min(b), a.max(b)))
                .filter(|(a, b)| b > a)
                .ok_or("Select a nonempty range with v and h/l.")?;
            let ids = self.selected_ids();
            let mut next = original.clone();
            let mut clips = vec![];
            for clip in original.clips.as_deref().unwrap_or(&[]) {
                if ids.contains(&clip.id) {
                    if let Some(part) = crate::editing::fragment(clip, a, b, false)? {
                        clips.push(part);
                    }
                } else {
                    clips.push(clip.clone());
                }
            }
            next.clips = Some(clips);
            self.commit_arrangement(next, "Trim selected clips to range", out);
            return Ok(());
        }
        if command == "align" {
            let ids = self.selected_ids();
            let origin = original
                .clips
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .filter(|c| ids.contains(&c.id))
                .map(|c| c.start)
                .min()
                .ok_or("Select audio to align.")?;
            let shift = self.view.cursor_frame - origin;
            let mut next = original.clone();
            for c in next
                .clips
                .as_mut()
                .into_iter()
                .flatten()
                .filter(|c| ids.contains(&c.id))
            {
                c.start += shift;
                if c.start + c.length > maximum {
                    return Err("Aligned audio exceeds the timeline.".into());
                }
            }
            self.commit_arrangement(next, "Align selected clips", out);
            return Ok(());
        }
        let clip = self
            .selected_clip()
            .cloned()
            .ok_or("Place the cursor on a clip.")?;
        let mut next = original.clone();
        if command == "restore" || command == "history-track" {
            let requested = arg(1)?;
            let states = clip.previous_states.as_deref().unwrap_or(&[]);
            let state = requested
                .parse::<usize>()
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| states.iter().rev().nth(i))
                .or_else(|| states.iter().find(|s| s.id.eq_ignore_ascii_case(requested)))
                .ok_or("History item not found; :history lists newest first.")?;
            let restored = crate::editing::from_history(&clip, state, command == "history-track")?;
            if restored.start + restored.length > maximum {
                return Err("History state exceeds the timeline.".into());
            }
            if command == "history-track" {
                let mut track = crate::editing::new_track(
                    format!("{} · {}", state.name, state.operation),
                    original.color.unwrap_or(0),
                )?;
                track.clips = Some(vec![restored]);
                self.commit_arrangement(track, "Create track from clip history", out);
            } else {
                let clips = next.clips.as_mut().ok_or("Clip was removed.")?;
                *clips
                    .iter_mut()
                    .find(|c| c.id == clip.id)
                    .ok_or("Clip was removed.")? = restored;
                self.commit_arrangement(next, "Restore clip state", out);
            }
            return Ok(());
        }
        let source_frames = self
            .shared
            .snapshot
            .as_ref()
            .and_then(|s| s.assets.iter().find(|a| a.id == clip.asset_id))
            .map(|a| a.frames);
        let replacement = match command {
            "trim" => crate::editing::trim(
                &clip,
                source_frames.ok_or("Refresh the clip's source metadata before trimming.")?,
                position(arg(1)?)?,
                position(arg(2)?)?,
            )?,
            "slip" => {
                let offset = position(arg(1)?)?;
                if offset + clip.length
                    > source_frames.ok_or("Refresh the clip's source metadata before slipping.")?
                {
                    return Err("Source offset exceeds the audio file.".into());
                }
                let mut changed = clip.clone();
                changed.offset = offset;
                changed.sound_design = None;
                changed.drum_pattern = None;
                changed.render_envelope = None;
                changed.fade_reference_offset = None;
                changed.fade_reference_length = None;
                changed
            }
            "fade" => {
                let frames = position(arg(2)?)?;
                if frames > clip.length {
                    return Err("Fade duration must fit inside the clip.".into());
                }
                let mut changed = clip.clone();
                match arg(1)? {
                    "in" => changed.fade_in_frames = Some(frames),
                    "out" => changed.fade_out_frames = Some(frames),
                    _ => return Err("Usage: fade in|out ms:5".into()),
                };
                changed.render_envelope = None;
                changed.fade_reference_offset = None;
                changed.fade_reference_length = None;
                changed
            }
            "protect" => {
                let mut changed = clip.clone();
                changed.edge_protection_disabled = Some(match arg(1)? {
                    "on" => false,
                    "off" => true,
                    _ => return Err("Usage: protect on|off".into()),
                });
                changed.edge_fade_frames = Some(
                    changed
                        .edge_fade_frames
                        .unwrap_or((rate * 0.005).round() as i64),
                );
                changed.render_envelope = None;
                changed.fade_reference_offset = None;
                changed.fade_reference_length = None;
                changed
            }
            "crossfade" => {
                let frames = position(arg(1)?)?;
                if frames < 1 {
                    return Err("Crossfade must contain at least one audio frame.".into());
                }
                let incoming = original
                    .clips
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .filter(|c| c.start > clip.start)
                    .min_by_key(|c| c.start)
                    .ok_or("No following clip on this track.")?;
                if incoming.start > clip.start + clip.length || frames > incoming.length {
                    return Err("Crossfade requires touching/overlapping clips and a duration inside the following clip.".into());
                }
                let mut changed = crate::editing::trim(
                    &clip,
                    source_frames.ok_or("Refresh the source metadata before crossfading.")?,
                    clip.start,
                    incoming.start + frames,
                )?;
                changed.edge_protection_disabled = Some(false);
                changed.edge_fade_frames = Some(
                    changed
                        .edge_fade_frames
                        .unwrap_or((rate * 0.005).round() as i64)
                        .max(1),
                );
                let target = next
                    .clips
                    .as_mut()
                    .and_then(|cs| cs.iter_mut().find(|c| c.id == incoming.id))
                    .ok_or("Following clip was removed.")?;
                target.edge_protection_disabled = Some(false);
                target.edge_fade_frames = Some(
                    target
                        .edge_fade_frames
                        .unwrap_or((rate * 0.005).round() as i64)
                        .max(1),
                );
                target.render_envelope = None;
                target.fade_reference_offset = None;
                target.fade_reference_length = None;
                changed
            }
            _ => return Err("Unknown clip command.".into()),
        };
        if replacement.start + replacement.length > maximum {
            return Err("Edit exceeds the timeline.".into());
        }
        *next
            .clips
            .as_mut()
            .and_then(|cs| cs.iter_mut().find(|c| c.id == clip.id))
            .ok_or("Clip was removed.")? = replacement;
        self.commit_arrangement(next, &format!("{} clip", command), out);
        Ok(())
    }
    fn delete_selection(&mut self, out: &mut Vec<Work>) {
        let ids = self.selected_ids();
        if let Some(track_id) = self.track().and_then(|t| t.id.clone()) {
            if !ids.is_empty() {
                self.push(
                    RemoteCommand::DeleteClips {
                        track_id,
                        clip_ids: ids.into_iter().collect(),
                    },
                    false,
                    out,
                );
            }
        }
    }
    fn nudge_clip(&mut self, count: i64, out: &mut Vec<Work>) {
        let Some(clip) = self.selected_clip().cloned() else {
            return;
        };
        let Some(track_id) = self.track().and_then(|t| t.id.clone()) else {
            return;
        };
        self.push(
            RemoteCommand::MoveClip {
                track_id,
                clip_id: clip.id,
                start: clip.start.saturating_add(self.step() * count).max(0),
            },
            false,
            out,
        );
    }
    fn selected_clip(&self) -> Option<&Clip> {
        self.track().and_then(|t| t.clips.as_ref()).and_then(|cs| {
            cs.iter()
                .find(|c| self.view.selected_clips.contains(&c.id))
                .or_else(|| {
                    cs.iter().find(|c| {
                        self.view.cursor_frame >= c.start
                            && self.view.cursor_frame < c.start + c.length
                    })
                })
        })
    }
    pub fn context(&self) -> RemoteEditorContext {
        let range = self.range.map(|(a, b)| (a.min(b), a.max(b)));
        RemoteEditorContext {
            track_id: self.track().and_then(|t| t.id.clone()),
            clip_id: self.selected_clip().map(|c| c.id.clone()),
            clip_ids: self.selected_ids().into_iter().collect(),
            cursor_frame: self.view.cursor_frame,
            range_start: range.map(|r| r.0),
            range_end: range.map(|r| r.1),
            effect_id: self
                .track()
                .and_then(|t| t.realtime_effects.as_ref())
                .and_then(|es| es.get(self.effect))
                .and_then(|e| e.id.clone()),
            automation_target: None,
        }
    }
    pub fn editor(&self, operation: &str) -> RemoteCommand {
        let mut context = self.context();
        if operation.starts_with("transport.") {
            context.clip_ids.clear();
            context.clip_id = None;
            context.range_start = None;
            context.range_end = None;
            context.effect_id = None;
        }
        let mut strings = BTreeMap::new();
        if operation.starts_with("pitch.") || operation.starts_with("master.") {
            if let Some(id) = context.track_id.clone() {
                strings.insert("trackID".into(), id);
            }
            if let Some(id) = context.effect_id.clone() {
                strings.insert("effectID".into(), id);
            }
        }
        RemoteCommand::Editor {
            intent: RemoteEditorIntent {
                operation: operation.into(),
                context,
                integers: BTreeMap::new(),
                numbers: BTreeMap::new(),
                strings,
                flags: BTreeMap::new(),
                payload: None,
            },
        }
    }
    fn push(&mut self, command: RemoteCommand, operator: bool, out: &mut Vec<Work>) {
        let operator_role = matches!(self.shared.role, Some(RemoteRole::Operator));
        if !self.shared.connected {
            self.status = "Disconnected: no command sent.".into();
            return;
        }
        if operator && !operator_role {
            self.status = "Playback and recording require an operator credential.".into();
            return;
        }
        let readonly = matches!(
            command,
            RemoteCommand::Snapshot | RemoteCommand::ListProjects
        ) || matches!(&command, RemoteCommand::Editor{intent} if intent.operation=="master.proposal");
        let cancel =
            matches!(&command,RemoteCommand::Editor{intent} if intent.operation=="jobs.cancel");
        if !operator && !readonly && matches!(self.shared.role, None | Some(RemoteRole::Viewer)) {
            self.status = "This client has read-only access.".into();
            return;
        }
        if let Some(cap) = self.shared.capabilities.as_ref() {
            let json = serde_json::to_value(&command).unwrap();
            let tag = json["type"].as_str().unwrap();
            if !cap.commands.iter().any(|c| c == tag) {
                self.status = format!("Host does not support {tag}.");
                return;
            }
            if let RemoteCommand::Editor { intent } = &command {
                if !cap.editor_operations.contains(&intent.operation) {
                    self.status = format!("Host does not support {}.", intent.operation);
                    return;
                }
            }
        }
        if !operator && !readonly && !cancel && self.pending_revision.is_some() {
            self.status = "Waiting for the previous edit receipt and metadata.".into();
            return;
        }
        if !operator && !readonly && !cancel {
            self.pending_revision = Some(-1);
        }
        let revision = if readonly || cancel {
            None
        } else {
            self.shared.snapshot.as_ref().map(|s| s.revision)
        };
        let tracks = if readonly || cancel {
            BTreeMap::new()
        } else {
            self.track()
                .and_then(|t| t.id.as_ref())
                .and_then(|id| {
                    self.shared
                        .snapshot
                        .as_ref()
                        .and_then(|s| s.track_revisions.get(&id.to_ascii_uppercase()))
                        .map(|r| (id.clone(), *r))
                })
                .into_iter()
                .collect()
        };
        out.push(Work::Command {
            command: Box::new(command),
            revision,
            tracks,
            authored: !operator && !readonly && !cancel,
        });
        self.status = "Queued; waiting for authoritative receipt.".into();
    }
    pub fn work_failed(&mut self, failure: &crate::worker::WorkFailure) {
        if failure.authored {
            if failure.uncertain {
                self.pending_revision = Some(-1);
                self.uncertain_pending = true;
            } else {
                self.pending_revision = None;
            }
        }
        if let Some(tag) = &failure.tag {
            if tag.operation == "local.file"
                && tag.workspace_id.as_ref() == self.pending_effect_file.as_ref()
            {
                self.pending_effect_file = None;
                self.pending_saved_preset = None;
            }
            if let Some(creative) = self.creative.as_mut() {
                creative.failure(tag, &failure.message);
            }
            if let Some(pitch) = self.pitch.as_mut() {
                pitch.failure(tag, &failure.message);
            }
            if let Some(master) = self.master.as_mut() {
                master.failure(tag, &failure.message);
            }
        }
        self.status = failure.message.clone();
    }
    pub fn cancel_pending(&mut self) {
        self.pending_revision = None;
    }
    fn selected_effect_metadata(&self) -> Option<&RemoteEffectMetadata> {
        let effect = self.track()?.realtime_effects.as_ref()?.get(self.effect)?;
        let kind = serde_json::to_value(&effect.kind).ok()?;
        self.shared
            .capabilities
            .as_ref()?
            .effects
            .iter()
            .find(|m| Some(m.id.as_str()) == kind.as_str())
    }
    fn effect_library_command(
        &mut self,
        words: &[String],
        out: &mut Vec<Work>,
    ) -> Result<(), String> {
        use crate::localfiles::{Operation, Request, SavedEffectPreset};
        let command = words[0].as_str();
        if command == "fx-custom-list" {
            self.panel = Panel::EffectPresets;
            self.status = format!(
                "{} private presets; :fx-custom zeroBasedIndex applies one.",
                self.effect_library.len()
            );
            return Ok(());
        }
        if command == "fx-custom-remove" {
            let i = words
                .get(1)
                .ok_or("Specify custom preset index.")?
                .parse::<usize>()
                .map_err(|_| "Index must be an integer.")?;
            if i >= self.effect_library.len() {
                return Err("Unknown custom preset index.".into());
            }
            self.effect_library.remove(i);
            self.preset_row = self
                .preset_row
                .min(self.effect_library.len().saturating_sub(1));
            self.status =
                "Preset removed from this private library; original files retained.".into();
            return Ok(());
        }
        let cap = self
            .shared
            .capabilities
            .as_ref()
            .ok_or("Waiting for host effect metadata.")?;
        if command == "fx-custom" {
            let i = words
                .get(1)
                .ok_or("Specify custom preset index.")?
                .parse::<usize>()
                .map_err(|_| "Index must be an integer.")?;
            let preset = self
                .effect_library
                .get(i)
                .ok_or("Unknown custom preset index.")?
                .clone();
            let mut track = self.track().cloned().ok_or("Select a track.")?;
            let effect = track
                .realtime_effects
                .as_mut()
                .and_then(|es| es.get_mut(self.effect))
                .ok_or("Select an insert.")?;
            if effect.kind != preset.kind {
                return Err("Preset kind does not match the selected insert.".into());
            }
            let kind = serde_json::to_value(&effect.kind).map_err(|e| e.to_string())?;
            let meta = cap
                .effects
                .iter()
                .find(|m| Some(m.id.as_str()) == kind.as_str())
                .ok_or("Host effect metadata is unavailable.")?;
            crate::localfiles::encode_effects(std::slice::from_ref(&preset), &cap.effects)?;
            if let Some(amount) = preset.legacy_amount {
                effect.amount = Some(amount);
                effect.parameters = Some(BTreeMap::new());
            } else {
                *effect = crate::automation::editable_effect(effect, meta);
                effect.parameters = Some(
                    meta.parameters
                        .iter()
                        .map(|p| {
                            (
                                p.id.clone(),
                                *preset.parameters.get(&p.id).unwrap_or(&p.default_value),
                            )
                        })
                        .collect(),
                );
            }
            self.push(
                RemoteCommand::UpdateTrackControls {
                    controls: crate::automation::controls(&track)?,
                    name: format!("Preset {}", preset.name),
                    gesture_id: None,
                },
                false,
                out,
            );
            return Ok(());
        }
        if self.pending_effect_file.is_some() {
            return Err("An effect preset file job is pending.".into());
        }
        let path = std::path::PathBuf::from(
            words
                .get(1)
                .ok_or("Specify a local preset file (quote spaces).")?,
        );
        let metadata = cap.effects.clone();
        let id = soundroom_client::auth::new_id().map_err(|e| e.to_string())?;
        let operation = match command {
            "fx-presets-load" => Operation::ReadEffects(path, metadata),
            "fx-presets-save" => {
                Operation::WriteEffects(path, self.effect_library.clone(), metadata)
            }
            "fx-preset-save" => {
                let effect = self
                    .track()
                    .and_then(|t| t.realtime_effects.as_ref())
                    .and_then(|es| es.get(self.effect))
                    .ok_or("Select an insert.")?;
                let meta = self
                    .selected_effect_metadata()
                    .ok_or("Host insert metadata is unavailable.")?;
                let name = words
                    .get(2)
                    .ok_or("Usage: fx-preset-save local.json \"Preset name\"")?
                    .trim()
                    .to_string();
                let legacy = effect.parameters.as_ref().map_or(true, |p| p.is_empty())
                    && effect
                        .parameter_automation
                        .as_ref()
                        .map_or(true, |p| p.is_empty());
                let preset = SavedEffectPreset {
                    id: soundroom_client::auth::new_id().map_err(|e| e.to_string())?,
                    name,
                    kind: effect.kind.clone(),
                    parameters: meta
                        .parameters
                        .iter()
                        .map(|p| {
                            (
                                p.id.clone(),
                                effect
                                    .parameters
                                    .as_ref()
                                    .and_then(|ps| ps.get(&p.id))
                                    .copied()
                                    .unwrap_or(p.default_value),
                            )
                        })
                        .collect(),
                    legacy_amount: legacy.then_some(effect.amount.unwrap_or(0.5)),
                };
                crate::localfiles::encode_effects(std::slice::from_ref(&preset), &metadata)?;
                if self.effect_library.len() == 128
                    || self.effect_library.iter().any(|p| {
                        p.kind == preset.kind && p.name.to_lowercase() == preset.name.to_lowercase()
                    })
                {
                    return Err(
                        "Choose a unique preset name; the private library supports 128 entries."
                            .into(),
                    );
                }
                self.pending_saved_preset = Some(preset.clone());
                Operation::WriteEffects(path, vec![preset], metadata)
            }
            _ => return Err("Unknown effect library command.".into()),
        };
        self.pending_effect_file = Some(id.clone());
        out.push(Work::LocalFile(Request {
            workspace: "effect-library".into(),
            id,
            epoch: 0,
            operation,
        }));
        self.status = "Local effect preset file job queued; host transport stays available.".into();
        Ok(())
    }
    fn effect_command(
        &mut self,
        command: &str,
        words: &[String],
        out: &mut Vec<Work>,
    ) -> Result<(), String> {
        let track = self.track().cloned().ok_or("Select a track.")?;
        let mut effects = track.realtime_effects.clone().unwrap_or_default();
        let capabilities = self
            .shared
            .capabilities
            .clone()
            .ok_or("Waiting for effect metadata.")?;
        if command == "fx-add" {
            let id = words.get(1).ok_or("Usage: fx-add effectID")?;
            let metadata = capabilities
                .effects
                .iter()
                .find(|m| &m.id == id)
                .ok_or("Unknown host effect ID.")?;
            let effect:RealtimeEffect=serde_json::from_value(serde_json::json!({"id":soundroom_client::auth::new_id().map_err(|e|e.to_string())?,"kind":id,"enabled":true,"amount":0.5,"parameters":metadata.parameters.iter().map(|p|(p.id.clone(),p.default_value)).collect::<BTreeMap<_,_>>(),"automation":[],"parameterAutomation":{}})).map_err(|e|e.to_string())?;
            effects.push(effect);
            self.effect = effects.len() - 1;
            self.parameter = 0;
        } else {
            let effect = effects
                .get_mut(self.effect)
                .ok_or("Select an insert first.")?;
            let kind = serde_json::to_value(&effect.kind).unwrap();
            let metadata = capabilities
                .effects
                .iter()
                .find(|m| Some(m.id.as_str()) == kind.as_str())
                .ok_or("Unknown effect metadata.")?;
            match command {
                "fx-remove" => {
                    effects.remove(self.effect);
                    self.effect = self.effect.min(effects.len().saturating_sub(1));
                }
                "fx-toggle" => effect.enabled = Some(!effect.enabled.unwrap_or(true)),
                "fx-preset" => {
                    let index: usize = words
                        .get(1)
                        .ok_or("Usage: fx-preset zeroBasedIndex")?
                        .parse()
                        .map_err(|_| "Preset index must be an integer.")?;
                    let preset = metadata
                        .factory_presets
                        .get(index)
                        .ok_or("Unknown factory preset.")?;
                    effect
                        .parameters
                        .get_or_insert_with(BTreeMap::new)
                        .extend(preset.values.clone());
                }
                "fx-set" => {
                    let id = words.get(1).ok_or("Usage: fx-set parameterID value")?;
                    let parameter = metadata
                        .parameters
                        .iter()
                        .find(|p| &p.id == id)
                        .ok_or("Unknown parameter ID.")?;
                    let scale = if parameter.unit == "%" { 100. } else { 1. };
                    let value: f32 = words
                        .get(2)
                        .ok_or("Missing value.")?
                        .parse::<f32>()
                        .map_err(|_| "Value must be numeric.")?
                        / scale;
                    if !value.is_finite()
                        || value < parameter.minimum
                        || value > parameter.maximum
                        || parameter.integer && value.fract() != 0.
                    {
                        return Err(format!(
                            "{} range: {}..{} {}",
                            parameter.name,
                            parameter.minimum * scale,
                            parameter.maximum * scale,
                            parameter.unit
                        ));
                    }
                    effect
                        .parameters
                        .get_or_insert_with(BTreeMap::new)
                        .insert(id.clone(), value);
                }
                _ => unreachable!(),
            }
        }
        self.controls(track, effects, out);
        Ok(())
    }
    fn adjust_parameter(&mut self, direction: f32, out: &mut Vec<Work>) {
        let Some(track) = self.track().cloned() else {
            return;
        };
        let Some(mut effects) = track.realtime_effects.clone() else {
            return;
        };
        let Some(effect) = effects.get_mut(self.effect) else {
            return;
        };
        let kind = serde_json::to_value(&effect.kind).unwrap();
        let Some(metadata) = self.shared.capabilities.as_ref().and_then(|c| {
            c.effects
                .iter()
                .find(|e| Some(e.id.as_str()) == kind.as_str())
        }) else {
            return;
        };
        let Some(parameter) = metadata.parameters.get(self.parameter) else {
            return;
        };
        let parameters = effect.parameters.get_or_insert_with(BTreeMap::new);
        let value = parameters
            .get(&parameter.id)
            .copied()
            .unwrap_or(parameter.default_value);
        let new = if parameter.logarithmic && parameter.minimum > 0. {
            value * (parameter.maximum / parameter.minimum).powf(direction / 100.)
        } else {
            value + direction * (parameter.maximum - parameter.minimum) / 100.
        };
        parameters.insert(
            parameter.id.clone(),
            if parameter.integer { new.round() } else { new }
                .clamp(parameter.minimum, parameter.maximum),
        );
        self.controls(track, effects, out);
    }
    fn controls(&mut self, track: Track, effects: Vec<RealtimeEffect>, out: &mut Vec<Work>) {
        let Some(id) = track.id else {
            return;
        };
        self.push(
            RemoteCommand::UpdateTrackControls {
                controls: RemoteTrackControls {
                    track_id: id,
                    name: track.name.unwrap_or_default(),
                    color: track.color.unwrap_or(0),
                    gain: track.gain.unwrap_or(1.),
                    pan: track.pan.unwrap_or(0.),
                    muted: track.muted.unwrap_or(false),
                    solo: track.solo.unwrap_or(false),
                    armed: track.armed.unwrap_or(false),
                    input_device_uid: track.input_device_uid,
                    input_device_name: track.input_device_name,
                    input_channel: track.input_channel.unwrap_or(0),
                    stereo_input: track.stereo_input.unwrap_or(false),
                    volume_automation_scale: track.volume_automation_scale,
                    mix_automation: track.mix_automation.unwrap_or_default(),
                    routing: track.routing,
                    realtime_effects: effects,
                },
                name: "Effect parameter".into(),
                gesture_id: None,
            },
            false,
            out,
        );
    }
    pub fn parse_command(&mut self, line: &str) -> Result<Vec<Work>, String> {
        let words = split_words(line)?;
        let Some(command) = words.first().map(String::as_str) else {
            return Ok(vec![]);
        };
        let arg = |i: usize| {
            words
                .get(i)
                .cloned()
                .ok_or_else(|| format!("Missing argument {i}."))
        };
        let id = || {
            self.track()
                .and_then(|t| t.id.clone())
                .ok_or("Select a track.".to_string())
        };
        let mut out = vec![];
        if matches!(command, "designer" | "beats") {
            self.open_creative(if command == "designer" {
                crate::creative::Kind::Designer
            } else {
                crate::creative::Kind::Beats
            })?;
            return Ok(out);
        }
        if command == "master" {
            if self.creative.is_some()
                || self.automation.is_some()
                || self.pitch.is_some()
                || self.master.is_some()
            {
                return Err("Close the current workspace first.".into());
            }
            let cap = self
                .shared
                .capabilities
                .as_ref()
                .ok_or("Waiting for host capabilities.")?;
            if !cap.features.iter().any(|f| f == "mastering") {
                return Err("Host does not advertise mastering.".into());
            }
            let snapshot = self
                .shared
                .snapshot
                .as_ref()
                .ok_or("Waiting for the server session.")?;
            self.master = Some(crate::master::Master::open(snapshot, self.sample_rate())?);
            return Ok(out);
        }
        if self.master.is_some() {
            match command {
                "q" | "quit" => {
                    self.master = None;
                    self.quit = true;
                }
                "analyze" | "proposal" => self.master_operation(
                    if command == "analyze" {
                        "master.analyze"
                    } else {
                        "master.proposal"
                    },
                    &mut out,
                )?,
                "cancel" => self.push(self.editor("jobs.cancel"), false, &mut out),
                "play" | "pause" => self.push(
                    self.editor(if command == "play" {
                        "transport.play"
                    } else {
                        "transport.pause"
                    }),
                    true,
                    &mut out,
                ),
                _ => {
                    let cap = self.shared.capabilities.clone();
                    let metadata = cap.as_ref().map_or(&[][..], |c| c.effects.as_slice());
                    if self.master.as_mut().unwrap().command(&words, metadata)? {
                        self.commit_master(&mut out)?;
                    }
                }
            }
            return Ok(out);
        }
        if command == "pitch" {
            if self.creative.is_some()
                || self.automation.is_some()
                || self.pitch.is_some()
                || self.master.is_some()
            {
                return Err("Close the current workspace first.".into());
            }
            let cap = self
                .shared
                .capabilities
                .as_ref()
                .ok_or("Waiting for capabilities.")?;
            if !cap.features.iter().any(|f| f == "pitch") {
                return Err("Host does not advertise pitch editing.".into());
            }
            let track = self.track().ok_or("Select an audio track.")?;
            let id = track.id.as_ref().ok_or("Missing track ID.")?;
            let scope = self
                .range
                .map(|(a, b)| (a.min(b), a.max(b)))
                .or_else(|| self.selected_clip().map(|c| (c.start, c.start + c.length)))
                .ok_or("Select a clip or audio range first.")?;
            let revision = self
                .shared
                .snapshot
                .as_ref()
                .and_then(|s| s.track_revisions.get(id))
                .copied()
                .ok_or("Missing track revision.")?;
            self.pitch = Some(crate::pitch::Pitch::open(
                track,
                self.effect,
                revision,
                scope,
                self.sample_rate(),
                self.bpm(),
            )?);
            return Ok(out);
        }
        if self.pitch.is_some() {
            match command {
                "q" | "quit" => {
                    self.close_pitch(&mut out);
                    self.quit = true;
                }
                "analyze" => self.pitch_operation("pitch.analyze", false, false, None, &mut out)?,
                "measure" => self.pitch_operation("pitch.render", false, false, None, &mut out)?,
                "audition" | "original" => self.pitch_operation(
                    "pitch.render",
                    true,
                    command == "original",
                    None,
                    &mut out,
                )?,
                "preset" => {
                    let name = words.iter().skip(1).cloned().collect::<Vec<_>>().join(" ");
                    self.pitch_operation("pitch.targets", false, false, Some(&name), &mut out)?;
                }
                "cancel" => {
                    self.push(self.editor("jobs.cancel"), false, &mut out);
                }
                "apply" => {
                    if self.pitch.as_ref().unwrap().dirty {
                        return Err("Use targets before applying the chain to audio.".into());
                    }
                    self.pitch_operation("pitch.apply", false, false, None, &mut out)?;
                    if !out.is_empty() {
                        self.close_pitch(&mut out);
                    }
                }
                _ => {
                    let cap = self.shared.capabilities.clone();
                    let meta = cap
                        .as_ref()
                        .and_then(|c| c.effects.iter().find(|e| e.id == "pitchCorrection"));
                    if self.pitch.as_mut().unwrap().command(&words, meta)? {
                        self.commit_pitch(&mut out);
                    }
                }
            }
            return Ok(out);
        }
        if command == "automation" {
            if self.creative.is_some()
                || self.automation.is_some()
                || self.pitch.is_some()
                || self.master.is_some()
            {
                return Err("Close the current workspace first.".into());
            }
            let cap = self
                .shared
                .capabilities
                .as_ref()
                .ok_or("Waiting for host capabilities.")?;
            if !cap.features.iter().any(|f| f == "automation") {
                return Err("Host does not advertise automation.".into());
            }
            let target = match words.get(1).map(String::as_str).unwrap_or("volume") {
                "volume" => crate::automation::Target::Volume,
                "pan" => crate::automation::Target::Pan,
                "amount" => crate::automation::Target::Amount(self.effect),
                "fx" => {
                    let meta = self.selected_effect_metadata().ok_or("Select an insert.")?;
                    let id = words.get(2).ok_or("Usage: automation fx parameterID")?;
                    let p = meta
                        .parameters
                        .iter()
                        .find(|p| &p.id == id)
                        .ok_or("Unknown insert parameter.")?;
                    crate::automation::Target::Parameter(self.effect, id.clone(), p.clone())
                }
                _ => return Err("Automation: volume, pan, amount, fx parameterID.".into()),
            };
            let mut track = self.track().cloned().ok_or("Select a track.")?;
            if let crate::automation::Target::Parameter(i, _, _) = &target {
                if let Some(meta) = self.selected_effect_metadata() {
                    let effects = track.realtime_effects.as_mut().ok_or("Select an insert.")?;
                    effects[*i] = crate::automation::editable_effect(&effects[*i], meta);
                }
            }
            let id = track.id.as_ref().ok_or("Missing track ID.")?;
            let revision = self
                .shared
                .snapshot
                .as_ref()
                .and_then(|s| s.track_revisions.get(id))
                .copied()
                .ok_or("Missing track revision.")?;
            self.automation = Some(crate::automation::Automation::open(
                &track,
                revision,
                target,
                self.view.cursor_frame,
                self.sample_rate(),
                self.bpm(),
            )?);
            return Ok(out);
        }
        if self.automation.is_some() && !matches!(command, "q" | "quit") {
            let commit = self.automation.as_mut().unwrap().command(&words)?;
            if commit {
                self.commit_automation(&mut out);
            }
            return Ok(out);
        }
        if matches!(command, "q" | "quit") {
            if let Some(creative) = self.creative.as_mut() {
                let context = self.creative_context.clone().unwrap();
                let commands = creative.close(context);
                for command in commands {
                    self.push(command, true, &mut out);
                }
            }
            self.quit = true;
            return Ok(out);
        }
        if self.creative.is_some() {
            if matches!(
                command,
                "midi-load"
                    | "midi-save"
                    | "midi-parts"
                    | "midi-excerpt"
                    | "preset-load"
                    | "preset-save"
            ) {
                if let Some(request) = self.creative.as_mut().unwrap().local_command(&words)? {
                    out.push(Work::LocalFile(request));
                }
                return Ok(out);
            }
            if command == "audition" && !matches!(self.shared.role, Some(RemoteRole::Operator)) {
                return Err("Audition requires an operator credential.".into());
            }
            let cap = self
                .shared
                .capabilities
                .clone()
                .ok_or("Waiting for capabilities.")?;
            let context = self.creative_context.clone().unwrap();
            let action = self
                .creative
                .as_mut()
                .unwrap()
                .command(&words, &cap, context)?;
            if let Some((command, operator)) = action {
                self.push(command, operator, &mut out);
                if !operator {
                    if let Some(Work::Command { tracks, .. }) = out.last_mut() {
                        if let Some(context) = self.creative_context.as_ref() {
                            if context.clip_id.is_some() {
                                if let (Some(id), Some(revision)) =
                                    (context.track_id.clone(), self.creative_revision)
                                {
                                    tracks.insert(id, revision);
                                }
                            }
                        }
                    }
                }
            }
            return Ok(out);
        }
        if matches!(
            command,
            "import" | "export" | "stems" | "download" | "transfer-cancel" | "transfer" | "files"
        ) {
            self.transfer_command(&words, &mut out)?;
            return Ok(out);
        }
        if matches!(
            command,
            "clip"
                | "history"
                | "restore"
                | "history-track"
                | "trim"
                | "trim-range"
                | "slip"
                | "fade"
                | "protect"
                | "crossfade"
                | "align"
                | "copy-track"
                | "cut"
        ) {
            self.arrange_command(&words, &mut out)?;
            return Ok(out);
        }
        if matches!(
            command,
            "fx-presets-load"
                | "fx-presets-save"
                | "fx-preset-save"
                | "fx-custom"
                | "fx-custom-list"
                | "fx-custom-remove"
        ) {
            self.effect_library_command(&words, &mut out)?;
            return Ok(out);
        }
        let action = match command {
            "q" | "quit" => {
                self.quit = true;
                return Ok(out);
            }
            "reconnect" => return Ok(vec![Work::Reconnect]),
            "refresh" => RemoteCommand::Snapshot,
            "play" => self.editor("transport.play"),
            "play-track" => {
                let RemoteCommand::Editor { mut intent } = self.editor("transport.play") else {
                    unreachable!()
                };
                intent.strings.insert("trackID".into(), id()?);
                RemoteCommand::Editor { intent }
            }
            "follow" => {
                self.follow = !self.follow;
                return Ok(out);
            }
            "fit" => {
                self.fit();
                return Ok(out);
            }
            "timebase" => {
                self.beats = match arg(1)?.as_str() {
                    "beats" => true,
                    "time" => false,
                    _ => return Err("Usage: timebase time|beats".into()),
                };
                return Ok(out);
            }
            "copy" => {
                self.copy_selection();
                return Ok(out);
            }
            "paste" => {
                self.paste_clipboard(&mut out)?;
                return Ok(out);
            }
            "pause" => self.editor("transport.pause"),
            "stop" => RemoteCommand::Stop,
            "record" => self.editor("transport.record"),
            "finish" => self.editor("transport.finishRecording"),
            "claim" => RemoteCommand::TakeTransport,
            "undo" => RemoteCommand::Undo,
            "redo" => RemoteCommand::Redo,
            "add" => RemoteCommand::AddTrack { name: arg(1)? },
            "rename" => RemoteCommand::SetTrackName {
                track_id: id()?,
                name: arg(1)?,
            },
            "gain" => {
                let db: f32 = arg(1)?.parse().map_err(|_| "Gain must be dB.")?;
                if !db.is_finite() || !(-90. ..=12.).contains(&db) {
                    return Err("Gain range: -90 to +12 dB.".into());
                }
                RemoteCommand::SetTrackGain {
                    track_id: id()?,
                    gain: 10_f32.powf(db / 20.),
                }
            }
            "pan" => {
                let pan: f32 = arg(1)?.parse().map_err(|_| "Pan must be -1 to 1.")?;
                if !pan.is_finite() || !(-1. ..=1.).contains(&pan) {
                    return Err("Pan must be -1 to 1.".into());
                }
                RemoteCommand::SetTrackPan {
                    track_id: id()?,
                    pan,
                }
            }
            "route" => {
                let channel: i64 = arg(2)?
                    .parse()
                    .map_err(|_| "Channel must be a zero-based integer.")?;
                let uid = arg(1)?;
                let device = self
                    .shared
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.devices.iter().find(|d| d.uid == uid))
                    .ok_or("Unknown host input UID.")?;
                let stereo = words.get(3).is_some_and(|s| s == "stereo");
                if channel < 0 || channel + if stereo { 2 } else { 1 } > device.input_channels {
                    return Err("Input channels exceed this host device.".into());
                }
                RemoteCommand::SetTrackInput {
                    track_id: id()?,
                    device_uid: Some(uid),
                    channel,
                    stereo,
                }
            }
            "save" => RemoteCommand::SaveProject { name: arg(1)?, replace_existing: None, title: None },
            "projects" => RemoteCommand::ListProjects,
            "open" => {
                let raw = arg(1)?;
                let id = if self.projects.contains(&raw) {
                    raw
                } else {
                    raw.parse::<usize>()
                        .ok()
                        .and_then(|i| i.checked_sub(1))
                        .and_then(|i| self.projects.get(i))
                        .cloned()
                        .unwrap_or(raw)
                };
                RemoteCommand::OpenProject { id }
            }
            "devices" => {
                self.panel = Panel::Routing;
                return Ok(out);
            }
            "output" => {
                let uid = arg(1)?;
                let snapshot = self
                    .shared
                    .snapshot
                    .as_ref()
                    .ok_or("Waiting for host devices.")?;
                if !snapshot
                    .devices
                    .iter()
                    .any(|d| d.uid == uid && d.output_channels > 0)
                {
                    return Err("Choose a host output UID from :devices.".into());
                }
                let input = snapshot
                    .devices
                    .iter()
                    .find(|d| d.id == snapshot.input_device_id)
                    .map(|d| d.uid.clone());
                RemoteCommand::SetDevices {
                    input_uid: input,
                    output_uid: Some(uid),
                }
            }
            "loop" => {
                let range = self
                    .range
                    .map(|(a, b)| (a.min(b), a.max(b)))
                    .filter(|(a, b)| b > a)
                    .ok_or("Select a range with v then h/l.")?;
                RemoteCommand::SetLoop {
                    start: Some(range.0),
                    end: Some(range.1),
                }
            }
            "unloop" => RemoteCommand::SetLoop {
                start: None,
                end: None,
            },
            "combine" => self.editor("clips.combine"),
            "apply" => {
                if self.context().effect_id.is_none() {
                    return Err("Select a live insert first.".into());
                }
                self.editor("pitch.apply")
            }
            "select" => {
                let (a, b) = self
                    .range
                    .map(|(a, b)| (a.min(b), a.max(b)))
                    .ok_or("Choose a range first.")?;
                let ids = self
                    .track()
                    .and_then(|t| t.clips.as_ref())
                    .map(|clips| {
                        clips
                            .iter()
                            .filter(|clip| clip.start < b && clip.start + clip.length > a)
                            .map(|clip| clip.id.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                self.view.selected_clips = ids;
                return Ok(out);
            }
            "all" => {
                self.view.selected_clips = self
                    .track()
                    .and_then(|t| t.clips.as_ref())
                    .map(|clips| clips.iter().map(|c| c.id.clone()).collect())
                    .unwrap_or_default();
                return Ok(out);
            }
            "fx-add" | "fx-set" | "fx-preset" | "fx-remove" | "fx-toggle" => {
                self.effect_command(command, &words, &mut out)?;
                return Ok(out);
            }
            _ => return Err(format!("Unknown command: {command}. ? lists commands.")),
        };
        let operator = matches!(
            command,
            "play"
                | "play-track"
                | "pause"
                | "stop"
                | "record"
                | "finish"
                | "claim"
                | "loop"
                | "unloop"
                | "output"
        );
        self.push(action, operator, &mut out);
        Ok(out)
    }
    fn master_operation(&mut self, operation: &str, out: &mut Vec<Work>) -> Result<(), String> {
        let snapshot = self
            .shared
            .snapshot
            .clone()
            .ok_or("Waiting for server metadata.")?;
        let request = self
            .master
            .as_mut()
            .unwrap()
            .request(operation, &snapshot)?;
        self.push(request, false, out);
        if out.is_empty() {
            self.master.as_mut().unwrap().busy = false;
        }
        Ok(())
    }
    fn commit_master(&mut self, out: &mut Vec<Work>) -> Result<(), String> {
        let snapshot = self
            .shared
            .snapshot
            .clone()
            .ok_or("Waiting for server metadata.")?;
        let cap = self.shared.capabilities.clone();
        let metadata = cap.as_ref().map_or(&[][..], |c| c.effects.as_slice());
        let master = self.master.as_mut().unwrap();
        let request = master.apply_request(&snapshot, metadata)?;
        let id = master.master_id.clone();
        let revision = master.revision;
        self.push(request, false, out);
        if let Some(Work::Command { tracks, .. }) = out.last_mut() {
            tracks.clear();
            tracks.insert(id, revision);
        } else {
            self.master.as_mut().unwrap().busy = false;
        }
        Ok(())
    }
    fn pitch_operation(
        &mut self,
        operation: &str,
        preview: bool,
        original: bool,
        preset: Option<&str>,
        out: &mut Vec<Work>,
    ) -> Result<(), String> {
        if preview && !matches!(self.shared.role, Some(RemoteRole::Operator)) {
            return Err("Audition requires an operator credential.".into());
        }
        if preview && !original && self.pitch.as_ref().is_some_and(|p| p.playing) {
            let request = self.pitch.as_ref().unwrap().stop_request();
            self.push(request, true, out);
            return Ok(());
        }
        let pitch = self.pitch.as_mut().unwrap();
        let id = pitch.track_id.clone();
        let revision = pitch.revision;
        let request = pitch.request(operation, preview, original, preset)?;
        self.push(request, preview, out);
        if let Some(Work::Command { tracks, .. }) = out.last_mut() {
            tracks.clear();
            tracks.insert(id, revision);
        } else {
            self.pitch.as_mut().unwrap().busy = false;
        }
        Ok(())
    }
    fn commit_pitch(&mut self, out: &mut Vec<Work>) {
        let pitch = self.pitch.as_ref().unwrap();
        let stop = (pitch.playing || pitch.busy).then(|| pitch.stop_request());
        let request = pitch.use_targets();
        let id = pitch.track_id.clone();
        let revision = pitch.revision;
        self.push(request, false, out);
        if let Some(Work::Command { tracks, .. }) = out.last_mut() {
            tracks.clear();
            tracks.insert(id, revision);
            self.pitch = None;
            if let Some(stop) = stop {
                self.push(stop, true, out);
            }
        }
    }
    fn close_pitch(&mut self, out: &mut Vec<Work>) {
        if let Some(pitch) = self.pitch.take() {
            if pitch.playing || pitch.busy {
                self.push(pitch.stop_request(), true, out);
            }
        }
    }
    fn commit_automation(&mut self, out: &mut Vec<Work>) {
        let a = self.automation.as_ref().unwrap();
        let command = a.request();
        let id = a.track_id.clone();
        let revision = a.revision;
        self.push(command, false, out);
        if let Some(Work::Command { tracks, .. }) = out.last_mut() {
            tracks.clear();
            tracks.insert(id, revision);
            self.automation = None;
        }
    }
    fn open_creative(&mut self, kind: crate::creative::Kind) -> Result<(), String> {
        if self.creative.is_some()
            || self.automation.is_some()
            || self.pitch.is_some()
            || self.master.is_some()
        {
            return Err(
                "Close the current creative workspace with t before opening another.".into(),
            );
        }
        let cap = self
            .shared
            .capabilities
            .as_ref()
            .ok_or("Waiting for capabilities.")?;
        let feature = if kind == crate::creative::Kind::Designer {
            "designer"
        } else {
            "beats"
        };
        if !cap.features.iter().any(|f| f == feature) {
            return Err(format!("This host does not advertise {feature}."));
        }
        let clip = self.selected_clip();
        let creative = crate::creative::Creative::open(kind, cap, clip, self.bpm())?;
        let mut context = self.context();
        let editable = clip.is_some_and(|c| {
            if kind == crate::creative::Kind::Designer {
                c.sound_design.is_some()
            } else {
                c.drum_pattern.is_some()
            }
        });
        if !editable {
            context.clip_id = None;
        }
        context.clip_ids.clear();
        context.range_start = None;
        context.range_end = None;
        context.effect_id = None;
        context.automation_target = None;
        self.creative_revision = context
            .track_id
            .as_ref()
            .and_then(|id| {
                self.shared
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.track_revisions.get(&id.to_ascii_uppercase()))
            })
            .copied();
        self.creative = Some(creative);
        self.creative_context = Some(context);
        Ok(())
    }
    pub fn tick(&mut self) -> Vec<Work> {
        let mut out = vec![];
        if let (Some(creative), Some(context)) =
            (self.creative.as_mut(), self.creative_context.clone())
        {
            for command in creative.tick(context) {
                self.push(command, true, &mut out);
            }
        }
        out
    }
    fn workspace_footer(&self, c: &mut Canvas, hint: &str) {
        let footer = c.height.saturating_sub(3);
        c.fill(0, footer, c.width, ' ', 252, 236);
        c.text(1, footer, &self.shared.status, 252, 236, false);
        c.text(1, footer + 1, &self.status, 244, 234, false);
        c.text(
            1,
            footer + 2,
            &self
                .command
                .as_ref()
                .map_or(hint.to_owned(), |s| format!(":{s}")),
            245,
            234,
            false,
        );
    }
    pub fn draw(&mut self, width: usize, height: usize) -> Canvas {
        self.width = width;
        self.height = height;
        let mut c = Canvas::new(width, height);
        let role = match self.shared.role {
            Some(RemoteRole::Operator) => "Operator",
            Some(RemoteRole::Editor) => "Editor",
            Some(RemoteRole::Viewer) => "Viewer",
            None => "Authenticating",
        };
        let snapshot = self.shared.snapshot.clone();
        let server = snapshot
            .as_ref()
            .map_or("Connecting", |s| s.server_name.as_str());
        c.fill(0, 0, width, ' ', 252, 236);
        c.text(
            1,
            0,
            &format!(
                "SOUNDROOM  {server}  {}  {role}",
                if self.shared.connected {
                    "CONNECTED"
                } else {
                    "OFFLINE"
                }
            ),
            if self.shared.connected { 120 } else { 209 },
            236,
            true,
        );
        let cursor = self.time(self.view.cursor_frame);
        let transport = self.shared.transport.as_ref().map(|(t, received)| {
            let frame = if t.is_playing && received.elapsed().as_millis() < 500 {
                t.frame + (received.elapsed().as_secs_f64() * self.sample_rate()) as i64
            } else {
                t.frame
            };
            (frame, t.is_playing, t.is_recording)
        });
        c.text(
            1,
            1,
            &format!(
                "{}  Cursor {}  {} {}  Zoom {:.2}  {}",
                transport.map_or("Stopped", |(_, playing, recording)| if recording {
                    "RECORDING"
                } else if playing {
                    "Playing"
                } else {
                    "Paused"
                }),
                cursor,
                if self.beats { "Beats" } else { "Time" },
                if self.snap { "Snap" } else { "Free" },
                self.view.zoom,
                if self.follow {
                    "Follow"
                } else {
                    "Private view"
                }
            ),
            252,
            234,
            false,
        );
        c.text(
            1,
            2,
            &format!(
                "Tracks  {} BPM   {:?}   Space play/pause  Tab panel  ? help",
                self.bpm(),
                self.panel
            ),
            245,
            234,
            false,
        );
        if let Some(transfer) = self.shared.transfer.as_ref().filter(|t| t.active) {
            let progress = if transfer.total > 0 {
                format!(
                    "{:.1}% {:.1}/{:.1} MiB",
                    transfer.completed as f64 / transfer.total as f64 * 100.,
                    transfer.completed as f64 / 1048576.,
                    transfer.total as f64 / 1048576.
                )
            } else {
                "Preparing".into()
            };
            c.fill(0, 2, width, ' ', 252, 234);
            c.text(
                1,
                2,
                &format!(
                    "{} · {} · {}  :transfer-cancel",
                    transfer.label, transfer.phase, progress
                ),
                81,
                234,
                false,
            );
        }
        if let Some(master) = self.master.as_mut() {
            let stale = self
                .shared
                .snapshot
                .as_ref()
                .map_or(true, |s| master.stale(s));
            let metadata = self
                .shared
                .capabilities
                .as_ref()
                .map_or(&[][..], |c| c.effects.as_slice());
            master.draw(&mut c, metadata, stale);
            self.workspace_footer(
                &mut c,
                "Space mix playback; Enter uses proposal; Esc closes; : commands",
            );
            return c;
        }
        if let Some(pitch) = self.pitch.as_mut() {
            pitch.draw(&mut c);
            self.workspace_footer(
                &mut c,
                "Space Pitch audition; Enter uses targets; Esc closes; : commands",
            );
            return c;
        }
        if let Some(automation) = self.automation.as_mut() {
            automation.draw(&mut c);
            self.workspace_footer(
                &mut c,
                "Space mix playback; Enter commits; Esc discards; : commands",
            );
            return c;
        }
        if let Some(creative) = self.creative.as_mut() {
            creative.draw(&mut c);
            let footer = height.saturating_sub(3);
            c.fill(0, footer, width, ' ', 252, 236);
            c.text(1, footer, &self.shared.status, 252, 236, false);
            c.text(1, footer + 1, &self.status, 244, 234, false);
            c.text(1,footer+2,&self.command.as_ref().map_or("Tab focus; Space owns audition; t timeline; :preset index; :sound parameter value; :insert".into(),|s|format!(":{s}")),245,234,false);
            return c;
        }
        let inspector = if self.panel == Panel::Arrange {
            0
        } else {
            40.min(width / 2)
        };
        let right = width.saturating_sub(inspector);
        let timeline = right.saturating_sub(34);
        let start = self.view.horizontal_frame;
        let frame_per = self.frames_per_column();
        if self.follow {
            if let Some((frame, true, _)) = transport {
                if frame < start || frame >= start + (timeline as f64 * frame_per) as i64 {
                    self.view.horizontal_frame =
                        (frame - (timeline as f64 * frame_per * 0.25) as i64).max(0);
                }
            }
        }
        let start = self.view.horizontal_frame;
        c.text(1, 3, "Track / M S A / Gain / Pan", 244, 234, true);
        for x in (0..timeline).step_by(12) {
            c.text(
                34 + x,
                3,
                &self.time(start + (x as f64 * frame_per) as i64),
                244,
                234,
                false,
            );
        }
        let rows = height.saturating_sub(8) / 2;
        for (index, track) in self.tracks().iter().enumerate().skip(self.top).take(rows) {
            let y = 4 + (index - self.top) * 2;
            let selected = index == self.row;
            let bg = if selected { 238 } else { 234 };
            c.fill(0, y, right, ' ', 252, bg);
            c.fill(0, y + 1, right, ' ', 252, bg);
            let gain = track.gain.unwrap_or(1.).max(0.00001);
            c.text(
                1,
                y,
                &format!(
                    "{} {:<15.15} {}{}{} {:>5.1} {:>4.1}",
                    if selected { ">" } else { " " },
                    track.name.as_deref().unwrap_or("Track"),
                    if track.muted.unwrap_or(false) {
                        "M"
                    } else {
                        "-"
                    },
                    if track.solo.unwrap_or(false) {
                        "S"
                    } else {
                        "-"
                    },
                    if track.armed.unwrap_or(false) {
                        "A"
                    } else {
                        "-"
                    },
                    20. * gain.log10(),
                    track.pan.unwrap_or(0.)
                ),
                252,
                bg,
                selected,
            );
            let end = start + (timeline as f64 * frame_per) as i64;
            if let Some(clips) = track.clips.as_ref() {
                let Some(indexed) = self.indices.get(index) else {
                    continue;
                };
                for clip in indexed.visible(clips, start, end) {
                    let left = (((clip.start - start) as f64 / frame_per).floor() as isize).max(0)
                        as usize;
                    let last = (((clip.start + clip.length - start) as f64 / frame_per).ceil()
                        as isize)
                        .clamp(0, timeline as isize) as usize;
                    let color = if self.view.selected_clips.contains(&clip.id) {
                        81
                    } else {
                        65 + (track.color.unwrap_or(0).rem_euclid(6) as u8) * 12
                    };
                    c.fill(34 + left, y + 1, last.saturating_sub(left), '=', color, bg);
                    if let Some(asset) = self
                        .assets
                        .get(&clip.asset_id)
                        .and_then(|i| snapshot.as_ref().and_then(|s| s.assets.get(*i)))
                    {
                        for column in left..last {
                            let frame =
                                (start + (column as f64 * frame_per) as i64).max(clip.start);
                            let source = clip.offset + (frame - clip.start);
                            let end = clip.offset
                                + ((start + ((column + 1) as f64 * frame_per) as i64)
                                    .min(clip.start + clip.length)
                                    - clip.start);
                            if let Some(peak) = crate::timeline::peak(asset, source, end) {
                                let symbols = b" .:-=+*#@";
                                c.put(
                                    34 + column,
                                    y + 1,
                                    symbols[(peak * 8.).round() as usize] as char,
                                    color,
                                    bg,
                                    false,
                                );
                            }
                        }
                    }

                    c.text(
                        34 + left,
                        y,
                        &clip
                            .name
                            .chars()
                            .take(last.saturating_sub(left))
                            .collect::<String>(),
                        color,
                        bg,
                        false,
                    );
                }
            }
            if let Some((a, b)) = self.range {
                for x in 0..timeline {
                    let frame = start + (x as f64 * frame_per) as i64;
                    if frame >= a.min(b) && frame < a.max(b) {
                        let cell = &mut c.cells[(y + 1) * c.width + 34 + x];
                        cell.bg = 24;
                    }
                }
            }
            let cursor = ((self.view.cursor_frame - start) as f64 / frame_per).floor() as isize;
            if cursor >= 0 && (cursor as usize) < timeline {
                c.put(34 + cursor as usize, y, '|', 231, bg, true);
                c.put(34 + cursor as usize, y + 1, '|', 231, bg, true);
            }
            if let Some((frame, true, _)) = transport {
                let p = ((frame - start) as f64 / frame_per) as isize;
                if p >= 0 && (p as usize) < timeline {
                    c.put(34 + p as usize, y + 1, '|', 203, bg, true);
                }
            }
        }
        if inspector > 0 {
            for y in 2..height.saturating_sub(3) {
                c.put(right, y, '|', 240, 234, false);
            }
            self.draw_panel(&mut c, right + 2, inspector.saturating_sub(3));
        }
        let footer = height.saturating_sub(3);
        c.fill(0, footer, width, ' ', 252, 236);
        c.text(1, footer, &self.shared.status, 252, 236, false);
        c.text(1, footer + 1, &self.status, 244, 234, false);
        c.text(1,footer+2,&self.command.as_ref().map_or("j/k tracks  h/l cursor  w/b edges  y/p copy/paste  dd delete  x split  v range  +/- zoom  : command".into(),|s|format!(":{s}")),if self.command.is_some(){81}else{245},234,false);
        c
    }
    pub fn time(&self, frame: i64) -> String {
        if self.beats {
            let timing = self
                .shared
                .snapshot
                .as_ref()
                .and_then(|s| s.session.timing.as_ref());
            let meter = timing.map_or(4, |t| t.beats_per_bar).max(1);
            let beat_unit = timing.map_or(4, |t| t.beat_unit).max(1);
            let ticks =
                (frame.max(0) as f64 / self.sample_rate() * self.bpm() / 60. * 960.).round() as i64;
            let ticks_per_beat = 3840 / beat_unit;
            let ticks_per_bar = meter * ticks_per_beat;
            format!(
                "{}.{}.{:03}",
                ticks / ticks_per_bar + 1,
                (ticks % ticks_per_bar) / ticks_per_beat + 1,
                ticks % ticks_per_beat
            )
        } else {
            let sec = frame as f64 / self.sample_rate();
            format!("{}:{:05.2}", (sec / 60.) as i64, sec % 60.)
        }
    }
    fn draw_panel(&self, c: &mut Canvas, x: usize, width: usize) {
        let mut lines = vec![];
        match self.panel {
            Panel::Help => {
                lines.extend(
                    [
                        "PRIVATE NAVIGATION",
                        "Counts: 3j; 8l; gg/G start/end",
                        "H/L scroll; +/- zoom",
                        "B beats/time; n snap; :follow",
                        "v range; w/b edges; [/] nudge",
                        "SHARED CONTROLS",
                        "Space pause/play; :play-track",
                        "m mute; s solo; a arm",
                        "x/S split; dd/Delete clips",
                        "u undo; Ctrl-R redo",
                        "r/R record; :finish; P claim",
                        "COMMANDS (quote names)",
                        ":add \"Track name\"",
                        ":rename \"New name\"",
                        ":gain -6; :pan -0.2",
                        ":route UID channel [stereo]",
                        ":loop; :unloop; :combine",
                        ":apply (bake inserts; Undo retained)",
                        ":select range clips; :all clips",
                        ":clip; :trim start end; :trim-range",
                        ":history; :restore number",
                        ":history-track number",
                        ":copy-track [automation]; p paste",
                        ":cut; :slip source; :align",
                        ":fade in|out ms:5; :crossfade ms:20",
                        ":fx-add ID; :fx-remove; :fx-toggle",
                        ":fx-set parameterID value",
                        ":fx-preset zeroBasedIndex",
                        ":save \"Project name\"",
                        ":projects; :open number or ID",
                        ":files; :import localPath [new]",
                        ":export local.wav|aiff [name] [selection] [no-tails]",
                        ":stems wav|aiff all|track [selection] [no-tails]",
                        ":download number localPath",
                        ":transfer-cancel; :transfer",
                        ":refresh; :reconnect; :quit",
                    ]
                    .map(str::to_owned),
                );
            }
            Panel::Routing => {
                lines.push("HOST AUDIO ROUTING".into());
                if let Some(s) = self.shared.snapshot.as_ref() {
                    for d in &s.devices {
                        lines.push(format!(
                            "{} (in {} / out {})",
                            d.name, d.input_channels, d.output_channels
                        ));
                        lines.push(d.uid.clone());
                    }
                }
                if let Some(t) = self.track() {
                    lines.push(format!("Track: {}", t.name.as_deref().unwrap_or("")));
                    lines.push(format!(
                        "Input: {}",
                        t.input_device_name.as_deref().unwrap_or("Default")
                    ));
                    lines.push(format!(
                        "Channel {} {}",
                        t.input_channel.unwrap_or(0),
                        if t.stereo_input.unwrap_or(false) {
                            "stereo"
                        } else {
                            "mono"
                        }
                    ));
                }
                lines.push(":route UID 0 [stereo]".into());
                lines.push(":output UID (operator; stop hardware first)".into());
            }
            Panel::Effects => {
                lines.push("REALTIME INSERTS".into());
                if !self.effect_library.is_empty() {
                    lines.push(format!(
                        ":fx-custom-list ({} saved presets)",
                        self.effect_library.len()
                    ));
                }
                lines.push(":fx-preset-save file \"Name\"".into());
                lines.push(":fx-presets-load file; :fx-custom-list".into());
                lines.push("E/B insert; J/K parameter".into());
                lines.push("h/l value; :apply rewrites audio".into());
                if self
                    .track()
                    .and_then(|t| t.realtime_effects.as_ref())
                    .map_or(true, |e| e.is_empty())
                {
                    lines.push("Add with :fx-add ID".into());
                    if let Some(cap) = self.shared.capabilities.as_ref() {
                        for m in &cap.effects {
                            lines.push(format!("{}: {}", m.id, m.name));
                        }
                    }
                }
                if let Some(track) = self.track() {
                    if let Some(effects) = track.realtime_effects.as_ref() {
                        for (i, e) in effects
                            .iter()
                            .enumerate()
                            .skip(self.effect.saturating_sub(2))
                            .take(5)
                        {
                            let kind = serde_json::to_value(&e.kind).unwrap();
                            let meta = self.shared.capabilities.as_ref().and_then(|c| {
                                c.effects
                                    .iter()
                                    .find(|m| Some(m.id.as_str()) == kind.as_str())
                            });
                            lines.push(format!(
                                "{} {} {}",
                                if i == self.effect { ">" } else { " " },
                                if e.enabled.unwrap_or(true) {
                                    "On"
                                } else {
                                    "Off"
                                },
                                meta.map_or("Insert", |m| m.name.as_str())
                            ));
                            if i == self.effect {
                                if let Some(meta) = meta {
                                    for (j, p) in meta
                                        .parameters
                                        .iter()
                                        .enumerate()
                                        .skip(self.parameter.saturating_sub(3))
                                        .take(c.height.saturating_sub(13))
                                    {
                                        let value = e
                                            .parameters
                                            .as_ref()
                                            .and_then(|ps| ps.get(&p.id))
                                            .copied()
                                            .unwrap_or(p.default_value);
                                        lines.push(format!(
                                            " {} {} ({}): {:.3} {}",
                                            if j == self.parameter { ">" } else { " " },
                                            p.name,
                                            p.id,
                                            if p.unit == "%" { value * 100. } else { value },
                                            p.unit
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Panel::EffectPresets => {
                lines.push("MY EFFECT PRESETS".into());
                lines.push("j/k browse · Enter applies · t inserts".into());
                let start = self
                    .preset_row
                    .saturating_sub(c.height.saturating_sub(10) / 2);
                for (i, p) in self
                    .effect_library
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(c.height.saturating_sub(9))
                {
                    lines.push(format!(
                        "{}{i}: {} / {:?}{}",
                        if i == self.preset_row { ">" } else { " " },
                        p.name,
                        p.kind,
                        if p.legacy_amount.is_some() {
                            " (legacy)"
                        } else {
                            ""
                        }
                    ));
                }
                if self.effect_library.is_empty() {
                    lines.push(":fx-presets-load file.json".into());
                }
            }
            Panel::Projects => {
                lines.push("HOST PROJECTS".into());
                for (i, id) in self.projects.iter().enumerate() {
                    lines.push(format!("{}: {id}", i + 1));
                }
                lines.push(":open number or ID".into());
                lines.push(":save \"Project name\"".into());
            }
            Panel::Files => {
                lines.push("LOCAL TRANSFERS / HOST EXPORTS".into());
                if let Some(v) = self.shared.transfer.as_ref() {
                    lines.push(v.label.clone());
                    lines.push(v.phase.clone());
                    lines.push(format!(
                        "{:.1}/{:.1} MiB",
                        v.completed as f64 / 1048576.,
                        v.total as f64 / 1048576.
                    ));
                    if let Some(id) = v.request_id.as_ref() {
                        lines.push(format!("Request {id}"));
                    }
                }
                lines.push(":import localPath [new]".into());
                lines.push(":export local.wav|aiff [name] [selection] [no-tails]".into());
                lines.push(":stems wav|aiff all|track [selection] [no-tails]".into());
                lines.push(":transfer-cancel".into());
                lines.push("RECENT EXPORTS (1 = latest)".into());
                for (i, a) in self.shared.artifacts.iter().rev().enumerate() {
                    lines.push(format!(
                        "{} {} ({:.1} MiB)",
                        i + 1,
                        a.name,
                        a.bytes as f64 / 1048576.
                    ));
                }
                lines.push(":download number localPath".into());
            }
            Panel::Clip => {
                lines.push("CLIP / SOURCE".into());
                if let Some(clip) = self.selected_clip() {
                    lines.push(clip.name.clone());
                    lines.push(format!("Start {}", self.time(clip.start)));
                    lines.push(format!("End   {}", self.time(clip.start + clip.length)));
                    lines.push(format!("Source offset {} frames", clip.offset));
                    lines.push(format!(
                        "Fade in/out {:.1}/{:.1} ms",
                        clip.fade_in_frames.unwrap_or(0) as f64 / self.sample_rate() * 1000.,
                        clip.fade_out_frames.unwrap_or(0) as f64 / self.sample_rate() * 1000.
                    ));
                    lines.push(format!(
                        "Edge protection {}",
                        if clip.edge_protection_disabled == Some(true) {
                            "OFF"
                        } else {
                            "ON"
                        }
                    ));
                    lines.push(":trim start end; :trim-range".into());
                    lines.push(":slip source; :align".into());
                    lines.push(":fade in|out ms:5".into());
                    lines.push(":crossfade ms:20; :protect on".into());
                    lines.push("HISTORY (1 = latest)".into());
                    for (i, state) in clip
                        .previous_states
                        .as_deref()
                        .unwrap_or(&[])
                        .iter()
                        .rev()
                        .enumerate()
                    {
                        lines.push(format!(
                            "{} Before {} ({:.2}s)",
                            i + 1,
                            state.operation,
                            state.length as f64 / self.sample_rate()
                        ));
                    }
                    lines.push(":restore number".into());
                    lines.push(":history-track number".into());
                } else {
                    lines.push("Place cursor on a clip.".into());
                }
            }
            Panel::Arrange => {}
        }
        for (i, line) in lines.iter().take(c.height.saturating_sub(7)).enumerate() {
            c.text(
                x,
                3 + i,
                &line.chars().take(width).collect::<String>(),
                if i == 0 { 81 } else { 252 },
                234,
                i == 0,
            );
        }
    }
}
pub fn split_words(text: &str) -> Result<Vec<String>, String> {
    let mut words = vec![];
    let mut word = String::new();
    let mut quote = false;
    let mut escaped = false;
    for c in text.chars() {
        if escaped {
            word.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            quote = !quote;
        } else if c.is_whitespace() && !quote {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            word.push(c);
        }
    }
    if quote || escaped {
        return Err("Unclosed quote or escape.".into());
    }
    if !word.is_empty() {
        words.push(word);
    }
    Ok(words)
}
impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_navigation_never_sends_commands() {
        let mut app = App::new();
        for key in [
            Key::Char('l'),
            Key::Char('+'),
            Key::Char('v'),
            Key::Char('L'),
            Key::Char('B'),
            Key::Char('f'),
        ] {
            assert!(app.key(key).is_empty());
        }
        assert!(app.view.cursor_frame > 0);
        assert!(app.range.is_some());
    }
    #[test]
    fn command_mode_blocks_global_shortcuts() {
        let mut app = App::new();
        app.key(Key::Char(':'));
        app.key(Key::Char('q'));
        app.key(Key::Char(' '));
        assert!(!app.quit);
        assert_eq!(app.command.as_deref(), Some("q "));
    }
    #[test]
    fn readonly_cannot_send_playback_or_edits() {
        let mut app = App::new();
        app.shared.connected = true;
        app.shared.role = Some(RemoteRole::Viewer);
        assert!(app.key(Key::Char(' ')).is_empty());
        assert!(app.parse_command("add track").unwrap().is_empty());
    }
    #[test]
    fn quoted_names_are_data_not_shell() {
        assert_eq!(
            split_words("save \"Dark $(echo hi)\"").unwrap(),
            vec!["save", "Dark $(echo hi)"]
        );
        assert!(split_words("save \"oops").is_err());
    }
    #[test]
    fn drawing_is_bounded_at_minimum_terminal_size() {
        let mut app = App::new();
        assert_eq!(app.draw(20, 8).cells.len(), 160);
    }
}
