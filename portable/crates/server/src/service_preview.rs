//! Instrument/drum/take/effect/pitch auditions and live designer notes.
//! Buffers are rendered on job workers and played on preview voice 0 after the
//! transport graph; live notes run a `LiveInstrument` on a dedicated thread that
//! stays a few milliseconds ahead of the callback through the preview ring.
//! One client owns the audition lease at a time.
use super::{engine, protocol, Error, Handled, Identity, JobKind, Service};
use base64::Engine;
use serde_json::{json, Value};
use soundroom_client::models::{RemoteEditorIntent, RemoteRole};
use soundroom_engine::{
    designer::{self, ControllerKind, LiveInstrument, PreparedVoice},
    drums,
    preview::{Buffer, PreviewControl, StreamWriter},
    project::Project,
    render::export,
    wave::WaveReader,
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{sync_channel, RecvTimeoutError, SyncSender, TryRecvError},
        Arc,
    },
    time::{Duration, Instant},
};

const MAX_PREVIEW_FRAMES: u64 = 10 * 60 * 48000;
const AHEAD: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Designer,
    Drums,
    Take,
    Pitch,
    Effects,
}
impl Kind {
    const ALL: [Kind; 5] = [
        Kind::Designer,
        Kind::Drums,
        Kind::Take,
        Kind::Pitch,
        Kind::Effects,
    ];
    fn name(self) -> &'static str {
        match self {
            Kind::Designer => "designer",
            Kind::Drums => "drums",
            Kind::Take => "take",
            Kind::Pitch => "pitch",
            Kind::Effects => "effects",
        }
    }
    fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }
}
enum Live {
    On {
        pitch: i32,
        velocity: f64,
        pattern: Box<designer::Pattern>,
        beat: f64,
    },
    Off(i32),
    AllOff,
    Controller(ControllerKind, f64),
}
#[derive(Default)]
pub(super) struct Previews {
    owner: Option<(String, Kind)>,
    preparing: Option<Kind>,
    /// Start time and beats per second of a looping instrument preview.
    started: Option<(Instant, f64)>,
    last: BTreeMap<&'static str, Value>,
    live: Option<SyncSender<Live>>,
    pattern: Option<designer::Pattern>,
    /// Latest rendered-preview request; a slower, older render that finishes
    /// after a newer one must not replace it (e.g. switching kits quickly).
    sequence: u64,
    /// The in-flight render's stop flag, raised when it is superseded.
    in_flight: Option<Arc<AtomicBool>>,
    #[cfg(target_os = "macos")]
    midi: Option<soundroom_engine::core_midi::MidiInput>,
}
pub(super) enum Source {
    /// A loop, or one hit of a voice at a velocity.
    Drums(Box<drums::Pattern>, Option<(drums::Voice, f64)>),
    Designer(Box<designer::Pattern>),
    Render {
        project: Box<Project>,
        start: u64,
        end: u64,
    },
    /// A file already rendered by another job (effect/pitch preview).
    File(PathBuf),
}
pub(super) struct Task {
    pub(super) source: Source,
    pub(super) kind: Kind,
    pub(super) owner: String,
    pub(super) path: PathBuf,
    pub(super) looping: bool,
    pub(super) tempo: f64,
    /// `Previews::sequence` at request time; 0 for file previews.
    pub(super) sequence: u64,
    /// Set when a newer request replaces this render.
    pub(super) superseded: Arc<AtomicBool>,
}
fn stereo(mut channels: Vec<Vec<f32>>) -> Result<(Vec<f32>, Vec<f32>), Error> {
    match channels.len() {
        1 => {
            let left = channels.pop().unwrap();
            Ok((left.clone(), left))
        }
        2 => {
            let right = channels.pop().unwrap();
            Ok((channels.pop().unwrap(), right))
        }
        _ => Err(protocol("Preview produced invalid audio")),
    }
}
/// A rendered selection starts and ends mid-waveform (and may loop): the
/// same short edge fade as clip edges keeps it from clicking.
fn fade_edges((mut left, mut right): (Vec<f32>, Vec<f32>)) -> (Vec<f32>, Vec<f32>) {
    let frames = (super::EDGE_FADE_FRAMES as usize).min(left.len() / 2);
    let n = left.len();
    for k in 0..frames {
        let w = (0.5 - 0.5 * (std::f32::consts::PI * (k as f32 + 0.5) / frames as f32).cos()).clamp(0., 1.);
        for channel in [&mut left, &mut right] {
            channel[k] *= w;
            channel[n - 1 - k] *= w;
        }
    }
    (left, right)
}
pub(super) fn load(path: &std::path::Path) -> Result<(Vec<f32>, Vec<f32>), Error> {
    load_cancellable(path, &AtomicBool::new(false))
}
fn load_cancellable(path: &std::path::Path, cancel: &AtomicBool) -> Result<(Vec<f32>, Vec<f32>), Error> {
    if cancel.load(Ordering::Acquire) { return Err(protocol("Preview cancelled")); }
    let mut reader = WaveReader::open(path).map_err(engine)?;
    let frames = reader.info.frames.min(MAX_PREVIEW_FRAMES) as usize;
    let (mut left, mut right) = (vec![0f32; frames], vec![0f32; frames]);
    for start in (0..frames).step_by(8192) {
        if cancel.load(Ordering::Acquire) { return Err(protocol("Preview cancelled")); }
        let end = (start + 8192).min(frames);
        reader
            .read_stereo(start as u64, &mut left[start..end], &mut right[start..end])
            .map_err(engine)?;
    }
    Ok((left, right))
}
impl Task {
    pub(super) fn render(self, cancel: &AtomicBool) -> Result<(Self, Buffer), Error> {
        // A newer preview request supersedes this one: stop work early.
        let stop: &AtomicBool = &self.superseded;
        if cancel.load(Ordering::Acquire) {
            self.superseded.store(true, Ordering::Release);
        }
        let mut intro = 0;
        let (left, right) = match &self.source {
            Source::Drums(pattern, Some((voice, velocity))) => {
                // The piece as the pattern plays it: its lane's kit, shape,
                // tuning, gain and pan (recorded and modeled kits included).
                let [left, right] = pattern.audition(*voice, *velocity, stop).map_err(engine)?;
                (left, right)
            }
            Source::Drums(pattern, None) => {
                // A clean first pass, then the seamless loop (whose start
                // carries the previous pass's tails).
                let [mut left, mut right] = pattern.render_first_pass(1, stop).map_err(engine)?;
                intro = left.len();
                let [loop_left, loop_right] = pattern.render(1, false, stop).map_err(engine)?;
                left.extend(loop_left);
                right.extend(loop_right);
                (left, right)
            }
            Source::Designer(pattern) => stereo(pattern.render_loop(stop).map_err(engine)?)?,
            Source::Render {
                project,
                start,
                end,
            } => {
                if let Some(parent) = self.path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                export(project, &self.path, *start, *end, cancel, |_, _| {}).map_err(engine)?;
                let audio = load(&self.path);
                let _ = std::fs::remove_file(&self.path);
                fade_edges(audio?)
            }
            Source::File(path) => {
                let audio = load_cancellable(path, cancel);
                let _ = std::fs::remove_file(path);
                fade_edges(audio?)
            }
        };
        if cancel.load(Ordering::Acquire) {
            return Err(protocol("Preview cancelled"));
        }
        let looping = self.looping;
        let (mut left, mut right) = (left, right);
        if looping {
            // Tail folding can sum across the loop boundary. One stereo gain
            // keeps the waveform and pan instead of clipping (Swift's loop prep).
            let peak = left
                .iter()
                .chain(&right)
                .filter(|v| v.is_finite())
                .fold(0f32, |m, v| m.max(v.abs()));
            if peak > 0.98 {
                let gain = 0.98 / peak;
                for v in left.iter_mut().chain(right.iter_mut()) {
                    *v = if v.is_finite() { *v * gain } else { 0. };
                }
            }
        }
        Ok((
            self,
            Buffer {
                left,
                right,
                looping,
                gain: 1.,
                loop_start: intro,
                ..Default::default()
            },
        ))
    }
}
fn spawn_live(writer: StreamWriter) -> SyncSender<Live> {
    let (tx, rx) = sync_channel::<Live>(256);
    std::thread::Builder::new()
        .name("soundroom-live-instrument".into())
        .spawn(move || {
            let mut instrument = LiveInstrument::new(32);
            let (mut left, mut right) = (vec![0f32; 256], vec![0f32; 256]);
            let handle = |instrument: &mut LiveInstrument, command: Live| match command {
                Live::On {
                    pitch,
                    velocity,
                    pattern,
                    beat,
                } => {
                    let stereo = !pattern.layers.is_empty()
                        || pattern.base_gain != 1.
                        || pattern.base_pan != 0.;
                    instrument.add_voice(PreparedVoice::new(
                        designer::Voice::new(pitch, velocity, &pattern, 48000., beat),
                        pitch,
                        pattern.base_gain,
                        pattern.base_pan,
                        stereo,
                    ));
                    for layer in pattern.layers.iter().take(6).filter(|l| {
                        l.enabled
                            && l.accepts(pitch)
                            && velocity >= l.minimum_velocity
                            && velocity <= l.maximum_velocity
                    }) {
                        let source = layer.instrument(&pattern);
                        instrument.add_voice(PreparedVoice::new(
                            designer::Voice::new(
                                (pitch + layer.transpose).clamp(0, 127),
                                velocity * layer.weight(velocity),
                                &source,
                                48000.,
                                beat,
                            )
                            .with_delay(layer.onset_delay()),
                            pitch,
                            layer.gain,
                            layer.pan,
                            true,
                        ));
                    }
                }
                Live::Off(pitch) => instrument.note_off(pitch),
                Live::AllOff => instrument.all_notes_off(),
                Live::Controller(kind, value) => instrument.controller(kind, value),
            };
            loop {
                loop {
                    match rx.try_recv() {
                        Ok(command) => handle(&mut instrument, command),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return,
                    }
                }
                if instrument.active_voice_count() == 0 {
                    instrument.collect_finished();
                    match rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(command) => handle(&mut instrument, command),
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                    continue;
                }
                if writer.queued() < AHEAD {
                    let _ = instrument.render_stereo(&mut left, &mut right);
                    let mut offset = 0;
                    while offset < left.len() {
                        offset += writer.write(&left[offset..], &right[offset..]);
                        if offset < left.len() {
                            std::thread::sleep(Duration::from_millis(1));
                        }
                    }
                    instrument.collect_finished();
                } else {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        })
        .expect("live instrument thread");
    tx
}
fn decode<T: serde::de::DeserializeOwned>(
    intent: &RemoteEditorIntent,
    model: &str,
) -> Result<T, Error> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(
            intent
                .payload
                .as_ref()
                .ok_or_else(|| protocol("Preview payload is required"))?,
        )
        .map_err(|_| protocol("Invalid preview payload"))?;
    let document: Value = serde_json::from_slice(&bytes)?;
    soundroom_client::validation::validate_model(&document, model)?;
    let mut document = document;
    super::normalize_seeds(&mut document, false);
    Ok(serde_json::from_value(document)?)
}
impl Service {
    fn preview_control(&self) -> Result<PreviewControl, Error> {
        self.backend
            .as_ref()
            .map(|b| b.preview())
            .ok_or_else(|| protocol("This host has no audio output"))
    }
    /// Acquires the audition lease, stopping a different kind first.
    fn claim_preview(&mut self, peer: &Identity, kind: Kind) -> Result<PreviewControl, Error> {
        if self.recording.is_some() {
            return Err(Error::Conflict("Finish recording first".into()));
        }
        let control = self.preview_control()?;
        if let Some((owner, current)) = &self.previews.owner {
            if owner != &peer.runtime {
                return Err(Error::Conflict(
                    "Another client owns the current instrument audition.".into(),
                ));
            }
            if *current != kind {
                self.stop_previews();
            }
        }
        self.previews.owner = Some((peer.runtime.clone(), kind));
        Ok(control)
    }
    /// Starts a new rendered-preview request: supersedes the one in flight.
    pub(super) fn next_preview(&mut self) -> (u64, Arc<AtomicBool>) {
        if let Some(previous) = self.previews.in_flight.take() {
            previous.store(true, Ordering::Release);
        }
        self.previews.sequence += 1;
        let flag = Arc::new(AtomicBool::new(false));
        self.previews.in_flight = Some(flag.clone());
        (self.previews.sequence, flag)
    }
    pub(super) fn stop_previews(&mut self) {
        if let Some(in_flight) = self.previews.in_flight.take() {
            in_flight.store(true, Ordering::Release);
        }
        if let Ok(control) = self.preview_control() {
            control.release_all();
        }
        if let Some(live) = &self.previews.live {
            let _ = live.try_send(Live::AllOff);
        }
        self.previews.owner = None;
        self.previews.preparing = None;
        self.previews.started = None;
        #[cfg(target_os = "macos")]
        if let Some(midi) = self.previews.midi.as_mut() {
            let _ = midi.connect(0);
        }
    }
    /// Server MIDI sources as (id, name); opens CoreMIDI on first use.
    fn midi_sources(&mut self) -> Result<Vec<Value>, Error> {
        #[cfg(target_os = "macos")]
        {
            if self.previews.midi.is_none() {
                let this = self
                    .this
                    .clone()
                    .ok_or_else(|| protocol("MIDI needs the audio host"))?;
                let (sender, bytes) = sync_channel::<Vec<u8>>(1024);
                self.previews.midi =
                    Some(soundroom_engine::core_midi::MidiInput::new(sender).map_err(engine)?);
                std::thread::Builder::new()
                    .name("soundroom-midi".into())
                    .spawn(move || {
                        let mut parser = soundroom_engine::core_midi::Parser::default();
                        for packet in bytes {
                            let events = parser.receive(&packet);
                            let Some(shared) = this.upgrade() else { return };
                            let Ok(mut state) = shared.lock() else { return };
                            for event in events {
                                state.midi_event(event);
                            }
                        }
                    })?;
            }
            Ok(self
                .previews
                .midi
                .as_ref()
                .unwrap()
                .sources()
                .into_iter()
                .map(|(id, name)| json!({"id": id, "name": name}))
                .collect())
        }
        #[cfg(not(target_os = "macos"))]
        Ok(vec![])
    }
    /// A parsed event from the connected source: play it and mirror it to the
    /// lease owner, as the Swift host does.
    #[cfg(target_os = "macos")]
    fn midi_event(&mut self, event: soundroom_engine::core_midi::Event) {
        use soundroom_engine::core_midi::Event;
        let Some((owner, Kind::Designer)) = self.previews.owner.clone() else {
            return;
        };
        let (command, message) = match event {
            Event::Note {
                pitch,
                velocity,
                on,
            } => (
                if on {
                    self.previews.pattern.clone().map(|pattern| Live::On {
                        pitch,
                        velocity: velocity as f64 / 127.,
                        pattern: Box::new(pattern),
                        beat: 0.,
                    })
                } else {
                    Some(Live::Off(pitch))
                },
                json!({"type":"midiNote","pitch":pitch,"velocity":velocity,"on":on}),
            ),
            Event::Controller(kind, value) => (
                Some(Live::Controller(kind, value)),
                json!({"type":"midiController","kind":kind,"value":value}),
            ),
            Event::AllNotesOff => (Some(Live::AllOff), json!({"type":"midiAllNotesOff"})),
        };
        if let Some(command) = command {
            if let Ok(live) = self.live() {
                let _ = live.try_send(command);
            }
        }
        if let Some(peer) = self.peers.get(&owner) {
            let _ = peer
                .out
                .send(&json!({"version":3,"kind":"event","event":message}), false);
        }
    }
    /// A job-rendered buffer starts only if its lease is still current.
    pub(super) fn play_preview(&mut self, task: &Task, mut buffer: Buffer) {
        if self.previews.owner.as_ref() != Some(&(task.owner.clone(), task.kind))
            || (task.sequence != 0 && task.sequence != self.previews.sequence)
        {
            return;
        }
        self.previews.preparing = None;
        if let Ok(control) = self.preview_control() {
            // A live edit of a playing loop continues at the same beat with a
            // short crossfade, as the in-process designer did, instead of
            // restarting the loop on every slider or note change.
            if buffer.looping && task.tempo > 0. {
                buffer.continuous = control.playing(0);
                if buffer.continuous && buffer.loop_start > 0 {
                    // Already playing: continue in the loop, skip the intro.
                    buffer.left.drain(..buffer.loop_start);
                    buffer.right.drain(..buffer.loop_start);
                    buffer.loop_start = 0;
                }
                let looped = buffer.left.len().saturating_sub(buffer.loop_start);
                buffer.beats = looped as f64 / 48_000. * task.tempo / 60.;
            }
            control.play(0, buffer);
            self.previews.started = Some((Instant::now(), task.tempo / 60.));
        }
    }
    pub(super) fn preview_failed(&mut self, owner: &str, kind: Kind) {
        if self.previews.owner.as_ref() == Some(&(owner.to_string(), kind)) {
            self.previews.preparing = None;
        }
    }
    /// Prepares an effect/pitch preview job that plays its rendered file.
    pub(super) fn claim_file_preview(&mut self, peer: &Identity, kind: Kind) -> Result<(), Error> {
        self.claim_preview(peer, kind)?;
        self.previews.preparing = Some(kind);
        Ok(())
    }
    pub(super) fn file_preview_current(&self, owner: &str, kind: Kind, sequence: u64) -> bool {
        self.previews.owner.as_ref() == Some(&(owner.to_string(), kind))
            && self.previews.sequence == sequence
    }
    pub(crate) fn publish_auditions(&mut self) {
        let control = self.preview_control().ok();
        let voice = control.as_ref().is_some_and(|c| c.playing(0));
        for kind in Kind::ALL {
            let owned = self.previews.owner.as_ref().filter(|(_, k)| *k == kind);
            let playing = owned.is_some() && voice;
            // The voice's own position wraps with the loop buffer, so a looping
            // preview never reports a beat past its pattern.
            let seconds = control
                .as_ref()
                .map_or(0., |c| c.position(0) as f64 / 48_000.);
            let position = match (kind, playing, self.previews.started) {
                (Kind::Designer, true, Some((_, beats))) => seconds * beats,
                (Kind::Drums, true, Some(_)) => seconds,
                _ => 0.,
            };
            let state = json!({"type":"audition","kind":kind.name(),"ownerID":owned.map(|(o, _)| o.clone()),
                "playing":playing,"preparing":self.previews.preparing == Some(kind),"position":position});
            let last = self.previews.last.get(kind.name());
            let transition = last.map_or(true, |last| {
                last["playing"] != state["playing"]
                    || last["preparing"] != state["preparing"]
                    || last["ownerID"] != state["ownerID"]
            });
            let moved = playing && last.is_some_and(|last| last["position"] != state["position"]);
            if transition || moved {
                self.previews.last.insert(kind.name(), state.clone());
                let event = json!({"version":3,"kind":"event","event":state});
                for peer in self.peers.values() {
                    // Position-only updates may be skipped for a slow reader.
                    let _ = if transition {
                        peer.out.send(&event, false)
                    } else {
                        peer.out.send_lossy(&event)
                    };
                }
            }
        }
    }
    fn live(&mut self) -> Result<&SyncSender<Live>, Error> {
        if self.previews.live.is_none() {
            let writer = self
                .preview_control()?
                .stream_writer()
                .ok_or_else(|| protocol("Live instrument is unavailable"))?;
            self.previews.live = Some(spawn_live(writer));
        }
        Ok(self.previews.live.as_ref().unwrap())
    }
    fn owns(&self, peer: &Identity, kind: Kind) -> bool {
        self.previews.owner.as_ref() == Some(&(peer.runtime.clone(), kind))
    }
    pub(super) fn preview_editor(
        &mut self,
        peer: &Identity,
        intent: &RemoteEditorIntent,
    ) -> Result<Handled, Error> {
        let path = self
            .root
            .join("Artifacts")
            .join(format!("{}.preview.wav", super::new_id()?));
        let job = |source, kind: Kind, looping, tempo, (sequence, superseded): (u64, Arc<AtomicBool>)| {
            Ok(Handled::Job(JobKind::Preview {
                task: Box::new(Task {
                    superseded,
                    source,
                    kind,
                    owner: peer.runtime.clone(),
                    path,
                    looping,
                    tempo,
                    sequence,
                }),
            }))
        };
        match intent.operation.as_str() {
            "drums.preview" => {
                let pattern: drums::Pattern = decode(intent, "DrumPattern")?;
                if pattern.steps.len() > 4096 {
                    return Err(protocol("Drummer has too many steps"));
                }
                let velocity = intent.numbers.get("velocity").copied().unwrap_or(0.9);
                if !(0. ..=1.).contains(&velocity) {
                    return Err(protocol("Audition velocity must be 0…1"));
                }
                let audition = match intent.strings.get("audition") {
                    Some(name) => Some((
                        serde_json::from_value::<drums::Voice>(json!(name))
                            .map_err(|_| protocol("Unknown drum voice"))?,
                        velocity,
                    )),
                    None => None,
                };
                self.claim_preview(peer, Kind::Drums)?;
                self.previews.preparing = Some(Kind::Drums);
                let tempo = pattern.bpm;
                let ticket = self.next_preview();
                job(
                    Source::Drums(Box::new(pattern), audition),
                    Kind::Drums,
                    audition.is_none(),
                    tempo,
                    ticket,
                )
            }
            "designer.preview" => {
                let pattern: designer::Pattern = decode(intent, "SoundDesignPattern")?;
                if pattern.notes.is_empty() {
                    return Err(protocol("Designer requires notes"));
                }
                self.claim_preview(peer, Kind::Designer)?;
                self.previews.preparing = Some(Kind::Designer);
                self.previews.pattern = Some(pattern.clone());
                let tempo = pattern.bpm;
                let ticket = self.next_preview();
                job(
                    Source::Designer(Box::new(pattern)),
                    Kind::Designer,
                    true,
                    tempo,
                    ticket,
                )
            }
            "take.preview" => {
                if self.owns(peer, Kind::Take) && self.preview_control()?.playing(0) {
                    self.stop_previews();
                    return Ok(Handled::Payload(None));
                }
                let take_id = intent
                    .strings
                    .get("takeID")
                    .ok_or_else(|| protocol("Choose a take to audition"))?;
                let (track, take) = self
                    .authority
                    .session
                    .tracks
                    .iter()
                    .find_map(|t| {
                        t.recording_takes
                            .iter()
                            .find(|take| {
                                take.extra
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .is_some_and(|id| id.eq_ignore_ascii_case(take_id))
                            })
                            .map(|take| (t.clone(), take.clone()))
                    })
                    .ok_or_else(|| protocol("Take is unavailable"))?;
                let mut project = self.dry_project(&track.id, None);
                project.session.tracks[0].clips = take.clips.clone();
                let end = take
                    .clips
                    .iter()
                    .map(|c| c.start + c.length)
                    .max()
                    .unwrap_or(0);
                let start = take.clips.iter().map(|c| c.start).min().unwrap_or(0);
                let (start, end) = match (intent.integers.get("start"), intent.integers.get("end"))
                {
                    (Some(a), Some(b)) if *a >= 0 && b > a => (*a as u64, *b as u64),
                    _ => (start, end),
                };
                if end <= start || end - start > MAX_PREVIEW_FRAMES {
                    return Err(protocol("Invalid take audition range"));
                }
                self.claim_preview(peer, Kind::Take)?;
                self.previews.preparing = Some(Kind::Take);
                let ticket = self.next_preview();
                job(
                    Source::Render {
                        project: Box::new(project),
                        start,
                        end,
                    },
                    Kind::Take,
                    false,
                    120.,
                    ticket,
                )
            }
            "audition.stop" | "designer.stop" | "drums.stop" | "take.stop" => {
                let kind = if intent.operation == "audition.stop" {
                    match intent.strings.get("kind") {
                        Some(name) => {
                            Some(Kind::parse(name).ok_or_else(|| protocol("Unknown audition"))?)
                        }
                        None => None,
                    }
                } else {
                    Kind::parse(intent.operation.split('.').next().unwrap())
                };
                if let Some((owner, current)) = &self.previews.owner {
                    if owner != &peer.runtime && peer.role != RemoteRole::Operator {
                        return Err(Error::Conflict("Another client owns this audition.".into()));
                    }
                    if kind.map_or(true, |k| k == *current) {
                        self.stop_previews();
                    }
                }
                Ok(Handled::Payload(None))
            }
            "designer.configure" => {
                if !self.owns(peer, Kind::Designer) {
                    return Err(protocol("Connect the server MIDI source first."));
                }
                self.previews.pattern = Some(decode(intent, "SoundDesignPattern")?);
                Ok(Handled::Payload(None))
            }
            "designer.noteOn" => {
                let pitch = intent.integers.get("pitch").copied().unwrap_or(-1);
                if !(0..=127).contains(&pitch) {
                    return Err(protocol("Invalid note"));
                }
                let pattern: designer::Pattern = match intent.payload {
                    Some(_) => decode(intent, "SoundDesignPattern")?,
                    None => self
                        .previews
                        .pattern
                        .clone()
                        .ok_or_else(|| protocol("Instrument pattern is required"))?,
                };
                self.claim_preview(peer, Kind::Designer)?;
                self.previews.pattern = Some(pattern.clone());
                let velocity = intent
                    .numbers
                    .get("velocity")
                    .copied()
                    .unwrap_or(0.8)
                    .clamp(0., 1.);
                let beat = intent.numbers.get("beat").copied().unwrap_or(0.);
                self.live()?
                    .try_send(Live::On {
                        pitch: pitch as i32,
                        velocity,
                        pattern: Box::new(pattern),
                        beat,
                    })
                    .map_err(|_| Error::QueueFull)?;
                Ok(Handled::Payload(None))
            }
            "designer.noteOff" | "designer.allNotesOff" | "designer.controller" => {
                if !self.owns(peer, Kind::Designer) {
                    return Ok(Handled::Payload(None));
                }
                let command = match intent.operation.as_str() {
                    "designer.noteOff" => {
                        Live::Off(intent.integers.get("pitch").copied().unwrap_or(0) as i32)
                    }
                    "designer.allNotesOff" => Live::AllOff,
                    _ => Live::Controller(
                        serde_json::from_value(json!(intent.strings.get("kind")))
                            .map_err(|_| protocol("Unknown controller"))?,
                        intent
                            .numbers
                            .get("value")
                            .copied()
                            .unwrap_or(0.)
                            .clamp(0., 1.),
                    ),
                };
                self.live()?
                    .try_send(command)
                    .map_err(|_| Error::QueueFull)?;
                Ok(Handled::Payload(None))
            }
            "designer.midiSources" | "designer.midiConnect" => {
                let sources = self.midi_sources()?;
                if intent.operation == "designer.midiConnect" {
                    let source = intent.integers.get("sourceID").copied().unwrap_or(0);
                    if source < 0
                        || (source != 0 && !sources.iter().any(|s| s["id"] == json!(source)))
                    {
                        return Err(protocol("MIDI source is unavailable"));
                    }
                    self.claim_preview(peer, Kind::Designer)?;
                    if intent.payload.is_some() {
                        self.previews.pattern = Some(decode(intent, "SoundDesignPattern")?);
                    }
                    #[cfg(target_os = "macos")]
                    if let Some(midi) = self.previews.midi.as_mut() {
                        midi.connect(source as u32).map_err(engine)?;
                    }
                }
                if let Some(p) = self.peers.get(&peer.runtime) {
                    let _ = p.out.send(&json!({"version":3,"kind":"event","event":{"type":"midiSources","sources":sources}}), false);
                }
                Ok(Handled::Payload(Some(json!(sources))))
            }
            _ => Err(protocol("Unsupported preview operation")),
        }
    }
}
