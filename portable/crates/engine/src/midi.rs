//! Bounded standard MIDI 0/1 interchange. Full arrangements are inspected before
//! selecting a Designer excerpt; variable tempo and unmatched note-offs reject.
use crate::{
    designer::{Controller, ControllerKind, Note, Pattern},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Part {
    pub id: u32,
    pub name: String,
    pub notes: Vec<Note>,
    pub controllers: Vec<Controller>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Content {
    pub parts: Vec<Part>,
    pub bpm: f64,
    pub beats: f64,
}
fn invalid() -> Error {
    Error::Invalid("Incomplete or unsupported MIDI file; use standard format 0 or 1".into())
}
fn limit() -> Error {
    Error::Invalid("MIDI resource limit exceeded".into())
}
impl Content {
    pub fn pattern(
        &self,
        part_id: Option<u32>,
        start: f64,
        length: Option<f64>,
    ) -> Result<Pattern> {
        let length = length.unwrap_or(self.beats);
        if !start.is_finite()
            || !length.is_finite()
            || start < 0.
            || start >= self.beats
            || length <= 0.
            || length > 64.
            || start + length > self.beats + 0.000001
        {
            return Err(Error::Invalid(
                "Choose a MIDI excerpt of at most 64 beats".into(),
            ));
        }
        let end = start + length;
        let chosen = self
            .parts
            .iter()
            .filter(|p| part_id.is_none() || part_id == Some(p.id))
            .collect::<Vec<_>>();
        let mut notes = vec![];
        for part in &chosen {
            for source in &part.notes {
                let note_end = end.min(source.start_beat + source.duration_beats);
                let note_start = start.max(source.start_beat);
                if note_end > note_start {
                    let mut note = source.clone();
                    note.start_beat = note_start - start;
                    note.duration_beats = note_end - note_start;
                    notes.push(note);
                }
            }
        }
        notes.sort_by(|a, b| {
            a.start_beat
                .total_cmp(&b.start_beat)
                .then(a.pitch.cmp(&b.pitch))
        });
        if notes.is_empty() {
            return Err(Error::Invalid(
                "MIDI excerpt contains no playable notes".into(),
            ));
        }
        if notes.len() > 256 {
            return Err(Error::Invalid(
                "Choose a MIDI excerpt of at most 256 notes".into(),
            ));
        }
        let mut controllers = vec![];
        for part in chosen {
            for kind in [
                ControllerKind::Sustain,
                ControllerKind::Modulation,
                ControllerKind::Expression,
                ControllerKind::PitchBend,
            ] {
                if let Some(c) = part
                    .controllers
                    .iter()
                    .rev()
                    .find(|c| c.kind == kind && c.beat < start)
                {
                    let mut c = c.clone();
                    c.beat = 0.;
                    controllers.push(c);
                }
            }
            controllers.extend(
                part.controllers
                    .iter()
                    .filter(|c| c.beat >= start && c.beat <= end)
                    .map(|c| Controller {
                        kind: c.kind,
                        beat: c.beat - start,
                        value: c.value,
                    }),
            );
        }
        if controllers.len() > 4096 {
            return Err(Error::Invalid(
                "MIDI excerpt contains too many controller events".into(),
            ));
        }
        controllers.sort_by(|a, b| a.beat.total_cmp(&b.beat));
        Ok(Pattern {
            notes,
            bpm: self.bpm,
            beats: length,
            controllers,
            ..Pattern::default()
        })
    }
}
struct Reader<'a> {
    bytes: &'a [u8],
    index: usize,
}
impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, index: 0 }
    }
    fn at_end(&self) -> bool {
        self.index == self.bytes.len()
    }
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        if count > self.bytes.len() - self.index {
            return Err(invalid());
        }
        let start = self.index;
        self.index += count;
        Ok(&self.bytes[start..self.index])
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn uint16(&mut self) -> Result<u32> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]) as u32)
    }
    fn uint32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn variable(&mut self) -> Result<u32> {
        let mut result = 0;
        for _ in 0..4 {
            let byte = self.byte()?;
            result = result << 7 | u32::from(byte & 127);
            if byte & 128 == 0 {
                return Ok(result);
            }
        }
        Err(invalid())
    }
}
#[derive(Clone)]
struct Event {
    tick: u32,
    track: u32,
    channel: u32,
    pitch: u8,
    velocity: u8,
    on: bool,
    order: usize,
}
/// Stable import-local UUIDv8 identity. It is not a security token; connection
/// identities and server-created resource IDs remain authority responsibilities.
fn note_id(seed: u64, event: &Event) -> String {
    let a = seed;
    let b = event.order as u64; // all 17 bits survive the UUID version/variant fields
    format!(
        "{:08X}-{:04X}-8{:03X}-A{:03X}-{:012X}",
        a >> 32,
        (a >> 16) & 65535,
        a & 4095,
        (b >> 48) & 4095,
        b & 0xffffffffffff
    )
}
pub fn inspect(data: &[u8]) -> Result<Content> {
    if data.len() > 4_194_304 {
        return Err(limit());
    }
    let mut reader = Reader::new(data);
    if reader.take(4)? != b"MThd" {
        return Err(invalid());
    }
    let header = reader.uint32()?;
    if header < 6 {
        return Err(invalid());
    }
    let format = reader.uint16()?;
    let track_count = reader.uint16()?;
    let division = reader.uint16()?;
    if format > 1
        || track_count == 0
        || track_count > 256
        || format == 0 && track_count != 1
        || division == 0
    {
        return Err(invalid());
    }
    if division & 0x8000 != 0 {
        return Err(Error::Unsupported(
            "SMPTE-timed MIDI is not supported; export using beats and ticks".into(),
        ));
    }
    reader.take((header - 6) as usize)?;
    let mut tempos = vec![];
    let mut events = vec![];
    let mut controllers: BTreeMap<u32, Vec<Controller>> = BTreeMap::new();
    let mut names = BTreeMap::new();
    let mut last_tick = 0;
    for track in 0..track_count {
        if reader.take(4)? != b"MTrk" {
            return Err(invalid());
        }
        let count = reader.uint32()?;
        let mut chunk = Reader::new(reader.take(count as usize)?);
        let mut tick = 0_u32;
        let mut running = 0;
        let mut ended = false;
        while !chunk.at_end() {
            tick = tick
                .checked_add(chunk.variable()?)
                .filter(|t| *t <= division * 65536)
                .ok_or_else(limit)?;
            last_tick = last_tick.max(tick);
            let mut status = chunk.byte()?;
            let first = if status < 0x80 {
                if !(0x80..0xF0).contains(&running) {
                    return Err(invalid());
                }
                let first = Some(status);
                status = running;
                first
            } else {
                if status < 0xF0 {
                    running = status;
                } else {
                    running = 0;
                }
                None
            };
            if status == 0xFF {
                let kind = chunk.byte()?;
                let length = chunk.variable()?;
                let payload = chunk.take(length as usize)?;
                match kind {
                    0x51 => {
                        if length != 3 {
                            return Err(invalid());
                        }
                        let tempo = u32::from(payload[0]) << 16
                            | u32::from(payload[1]) << 8
                            | u32::from(payload[2]);
                        if tempo == 0 {
                            return Err(invalid());
                        }
                        tempos.push((tick, tempo));
                    }
                    0x03 => {
                        if let Ok(name) = std::str::from_utf8(payload) {
                            if !name.is_empty() {
                                names.insert(track, name.to_owned());
                            }
                        }
                    }
                    0x2F => {
                        if length != 0 || !chunk.at_end() {
                            return Err(invalid());
                        }
                        ended = true;
                        break;
                    }
                    _ => {}
                }
            } else if status == 0xF0 || status == 0xF7 {
                let count = chunk.variable()?;
                chunk.take(count as usize)?;
            } else {
                let command = status & 0xF0;
                if !(0x80..=0xE0).contains(&command) {
                    return Err(invalid());
                }
                let one = if let Some(one) = first {
                    one
                } else {
                    chunk.byte()?
                };
                let two = if command == 0xC0 || command == 0xD0 {
                    0
                } else {
                    chunk.byte()?
                };
                if one >= 128 || two >= 128 {
                    return Err(invalid());
                }
                let channel = u32::from(status & 15);
                if command == 0x90 || command == 0x80 {
                    events.push(Event {
                        tick,
                        track,
                        channel,
                        pitch: one,
                        velocity: two,
                        on: command == 0x90 && two > 0,
                        order: events.len(),
                    });
                    if events.len() > 131072 {
                        return Err(limit());
                    }
                } else if command == 0xB0 || command == 0xE0 {
                    let mut value = f64::from(two) / 127.;
                    let kind = if command == 0xE0 {
                        let raw = u32::from(one) | u32::from(two) << 7;
                        value = if raw <= 8192 {
                            f64::from(raw) / 16384.
                        } else {
                            0.5 + f64::from(raw - 8192) / 16382.
                        };
                        Some(ControllerKind::PitchBend)
                    } else {
                        match one {
                            1 => Some(ControllerKind::Modulation),
                            7 | 11 => Some(ControllerKind::Expression),
                            64 => Some(ControllerKind::Sustain),
                            _ => None,
                        }
                    };
                    if let Some(kind) = kind {
                        let lane = controllers.entry(track * 16 + channel).or_default();
                        lane.push(Controller {
                            kind,
                            beat: f64::from(tick) / f64::from(division),
                            value,
                        });
                        if lane.len() > 65536 {
                            return Err(limit());
                        }
                    }
                }
            }
        }
        if !ended {
            return Err(invalid());
        }
    }
    if !reader.at_end() {
        return Err(invalid());
    }
    tempos.sort_by_key(|t| t.0);
    let tempo = tempos.first().map(|t| t.1).unwrap_or(500000);
    if tempos
        .first()
        .map(|t| t.0 > 0 && tempo != 500000)
        .unwrap_or(false)
        || tempos.iter().any(|t| t.1 != tempo)
    {
        return Err(Error::Unsupported(
            "Designer MIDI import requires constant tempo".into(),
        ));
    }
    let bpm = 60_000_000. / f64::from(tempo);
    if !(30. ..=300.).contains(&bpm) {
        return Err(Error::Unsupported(
            "Designer MIDI tempo must be 30–300 BPM".into(),
        ));
    }
    let seed = data.iter().fold(0xcbf29ce484222325_u64, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x100000001b3)
    });
    let mut active: BTreeMap<(u32, u32, u8), VecDeque<Event>> = BTreeMap::new();
    let mut notes: BTreeMap<u32, Vec<Note>> = BTreeMap::new();
    events.sort_by_key(|e| (e.tick, e.order));
    for event in events {
        let key = (event.track, event.channel, event.pitch);
        if event.on {
            active.entry(key).or_default().push_back(event);
        } else if let Some(start) = active.get_mut(&key).and_then(VecDeque::pop_front) {
            if event.tick > start.tick {
                notes
                    .entry(event.track * 16 + event.channel)
                    .or_default()
                    .push(Note {
                        id: note_id(seed, &start),
                        pitch: i32::from(event.pitch),
                        velocity: f64::from(start.velocity) / 127.,
                        start_beat: f64::from(start.tick) / f64::from(division),
                        duration_beats: f64::from(event.tick - start.tick) / f64::from(division),
                        articulation: None,
                    });
            }
        }
    }
    if active.values().any(|v| !v.is_empty()) {
        return Err(invalid());
    }
    if notes.is_empty() {
        return Err(Error::Invalid("MIDI contains no playable notes".into()));
    }
    let last = notes
        .values()
        .flatten()
        .map(|n| n.start_beat + n.duration_beats)
        .fold(f64::from(last_tick) / f64::from(division), f64::max);
    let parts = notes
        .into_iter()
        .map(|(id, mut notes)| {
            notes.sort_by(|a, b| {
                a.start_beat
                    .total_cmp(&b.start_beat)
                    .then(a.pitch.cmp(&b.pitch))
            });
            let mut controllers = controllers.remove(&id).unwrap_or_default();
            controllers.sort_by(|a, b| a.beat.total_cmp(&b.beat));
            Part {
                id,
                name: format!(
                    "{} · {}",
                    names
                        .get(&(id / 16))
                        .cloned()
                        .unwrap_or_else(|| format!("Track {}", id / 16 + 1)),
                    if id % 16 == 9 {
                        "Drums".into()
                    } else {
                        format!("Channel {}", id % 16 + 1)
                    }
                ),
                notes,
                controllers,
            }
        })
        .collect();
    Ok(Content {
        parts,
        bpm,
        beats: last.max(0.25),
    })
}
fn variable(mut value: u32) -> Vec<u8> {
    let mut result = vec![(value & 127) as u8];
    value >>= 7;
    while value > 0 {
        result.insert(0, (value & 127) as u8 | 128);
        value >>= 7;
    }
    result
}
/// Export MIDI controllers and articulated durations; no instrument program is
/// injected, because Soundroom patches belong to the project.
pub fn encode(p: &Pattern) -> Result<Vec<u8>> {
    let bpm = if p.bpm.is_finite() {
        p.bpm.clamp(30., 300.)
    } else {
        120.
    };
    let beats = if p.beats.is_finite() {
        p.beats.clamp(0.25, 64.)
    } else {
        4.
    };
    let ppq = 480.;
    let tempo = (60_000_000. / bpm).round() as u32;
    let mut track = vec![
        0,
        0xFF,
        0x51,
        3,
        ((tempo >> 16) & 255) as u8,
        ((tempo >> 8) & 255) as u8,
        (tempo & 255) as u8,
    ];
    let mut events = vec![];
    for note in p.performed_notes_up_to(256, false) {
        if !note.start_beat.is_finite()
            || note.start_beat < 0.
            || note.start_beat >= beats
            || !note.duration_beats.is_finite()
            || note.duration_beats <= 0.
            || !note.velocity.is_finite()
            || note.velocity <= 0.
        {
            continue;
        }
        let start = (note.start_beat * ppq).round() as u32;
        let end = ((beats.min(note.start_beat + note.duration_beats) * ppq).round() as u32)
            .max(start + 1);
        let pitch = note.pitch.clamp(0, 127) as u8;
        let velocity = (note.velocity * 127.).round().clamp(1., 127.) as u8;
        events.push((start, 2, [0x90, pitch, velocity]));
        events.push((end, 0, [0x80, pitch, 0]));
    }
    for c in p.controllers.iter().take(4096) {
        if !c.beat.is_finite() || c.beat < 0. || c.beat > beats || !c.value.is_finite() {
            continue;
        }
        let value = c.value.clamp(0., 1.);
        let tick = (c.beat * ppq).round() as u32;
        let bytes = if c.kind == ControllerKind::PitchBend {
            let raw = (if value <= 0.5 {
                value * 16384.
            } else {
                8192. + (value - 0.5) * 16382.
            })
            .round() as u32;
            [0xE0, (raw & 127) as u8, ((raw >> 7) & 127) as u8]
        } else {
            [
                0xB0,
                match c.kind {
                    ControllerKind::Sustain => 64,
                    ControllerKind::Modulation => 1,
                    _ => 11,
                },
                (value * 127.).round() as u8,
            ]
        };
        events.push((tick, 1, bytes));
    }
    events.sort();
    let mut previous = 0;
    for (tick, _, bytes) in events {
        track.extend(variable(tick - previous));
        track.extend(bytes);
        previous = tick;
    }
    track.extend(variable(
        ((beats * ppq).round() as u32).saturating_sub(previous),
    ));
    track.extend([0xFF, 0x2F, 0]);
    let mut output = b"MThd".to_vec();
    output.extend(6_u32.to_be_bytes());
    output.extend([0, 0, 0, 1]);
    output.extend(480_u16.to_be_bytes());
    output.extend(b"MTrk");
    output.extend((track.len() as u32).to_be_bytes());
    output.extend(track);
    Ok(output)
}
