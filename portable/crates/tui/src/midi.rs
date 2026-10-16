//! Bounded Standard MIDI File interchange; this module never opens MIDI/audio devices.
use soundroom_client::{auth::new_id, models::*};
use std::collections::{BTreeMap, VecDeque};
pub const MAX_FILE: usize = 4 * 1024 * 1024;
#[derive(Clone, Debug)]
pub struct MIDINote {
    pub pitch: i64,
    pub velocity: f64,
    pub start: f64,
    pub duration: f64,
}
#[derive(Clone, Debug)]
pub struct MIDIControl {
    pub kind: SoundDesignControllerKind,
    pub beat: f64,
    pub value: f64,
}
#[derive(Clone, Debug)]
pub struct MIDIPart {
    pub name: String,
    pub track: usize,
    pub channel: u8,
    pub notes: Vec<MIDINote>,
    pub controllers: Vec<MIDIControl>,
}
#[derive(Clone, Debug)]
pub struct MIDIFile {
    pub parts: Vec<MIDIPart>,
    pub bpm: f64,
    pub beats: f64,
    pub ppq: u16,
}
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let end = self
            .at
            .checked_add(count)
            .filter(|n| *n <= self.bytes.len())
            .ok_or("MIDI file is truncated.")?;
        let part = &self.bytes[self.at..end];
        self.at = end;
        Ok(part)
    }
    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, String> {
        let p = self.take(2)?;
        Ok(u16::from_be_bytes([p[0], p[1]]))
    }
    fn u32(&mut self) -> Result<u32, String> {
        let p = self.take(4)?;
        Ok(u32::from_be_bytes([p[0], p[1], p[2], p[3]]))
    }
    fn vlq(&mut self) -> Result<u32, String> {
        let mut value = 0u32;
        for _ in 0..4 {
            let b = self.byte()?;
            value = (value << 7) | u32::from(b & 127);
            if b & 128 == 0 {
                return Ok(value);
            }
        }
        Err("MIDI variable-length value exceeds four bytes.".into())
    }
    fn data(&mut self) -> Result<u8, String> {
        let b = self.byte()?;
        if b > 127 {
            Err("MIDI data byte has an unexpected status bit.".into())
        } else {
            Ok(b)
        }
    }
}
#[derive(Default)]
struct PartBuilder {
    notes: Vec<MIDINote>,
    controllers: Vec<MIDIControl>,
    pending: BTreeMap<u8, VecDeque<(u64, u8)>>,
}
impl PartBuilder {
    fn off(&mut self, pitch: u8, tick: u64, ppq: u16) {
        if let Some((start, velocity)) = self.pending.get_mut(&pitch).and_then(VecDeque::pop_front)
        {
            if tick > start {
                self.notes.push(MIDINote {
                    pitch: i64::from(pitch),
                    velocity: f64::from(velocity) / 127.,
                    start: start as f64 / f64::from(ppq),
                    duration: (tick - start) as f64 / f64::from(ppq),
                });
            }
        }
    }
    fn controller(&mut self, kind: SoundDesignControllerKind, tick: u64, value: f64, ppq: u16) {
        self.controllers.push(MIDIControl {
            kind,
            beat: tick as f64 / f64::from(ppq),
            value,
        });
    }
}
pub fn parse(bytes: &[u8]) -> Result<MIDIFile, String> {
    if bytes.len() > MAX_FILE {
        return Err("MIDI file is limited to 4 MiB.".into());
    }
    let mut r = Reader { bytes, at: 0 };
    if r.take(4)? != b"MThd" {
        return Err("Choose a Standard MIDI File (.mid or .midi).".into());
    }
    let header = r.u32()? as usize;
    if header < 6 {
        return Err("Invalid MIDI header length.".into());
    }
    let h = r.take(header)?;
    let mut h = Reader { bytes: h, at: 0 };
    let format = h.u16()?;
    let tracks = h.u16()?;
    let ppq = h.u16()?;
    if format > 1 || tracks == 0 || tracks > 256 || (format == 0 && tracks != 1) {
        return Err("MIDI supports type 0/1 with 1–256 tracks; independent type 2 patterns are unsupported.".into());
    }
    if ppq == 0 || ppq & 0x8000 != 0 {
        return Err("MIDI requires PPQ timing; SMPTE timecode files need conversion first.".into());
    }
    let mut parts = vec![];
    let mut bpm = 120.;
    let mut tempo_tick = u64::MAX;
    let mut tempo_micros: Option<u32> = None;
    let mut total_ticks = 0;
    let mut events = 0usize;
    let mut notes = 0usize;
    for track in 0..usize::from(tracks) {
        loop {
            let kind = r.take(4)?;
            let length = r.u32()? as usize;
            let chunk = r.take(length)?;
            if kind != b"MTrk" {
                continue;
            }
            let mut tr = Reader {
                bytes: chunk,
                at: 0,
            };
            let mut tick = 0u64;
            let mut status = None;
            let mut ended = false;
            let mut name = format!("Track {}", track + 1);
            let mut channels: BTreeMap<u8, PartBuilder> = BTreeMap::new();
            while tr.at < tr.bytes.len() {
                events += 1;
                if events > 200_000 {
                    return Err("MIDI exceeds 200,000 events.".into());
                }
                tick = tick
                    .checked_add(u64::from(tr.vlq()?))
                    .ok_or("MIDI timeline overflow.")?;
                if tick > u64::from(ppq) * 65_536 {
                    return Err("MIDI timeline exceeds 65,536 beats.".into());
                }
                total_ticks = total_ticks.max(tick);
                let first = tr.byte()?;
                let mut first_data = None;
                let event = if first & 128 != 0 {
                    first
                } else {
                    first_data = Some(first);
                    status.ok_or("MIDI running status has no previous voice event.")?
                };
                if event == 0xFF {
                    status = None;
                    let kind = tr.byte()?;
                    let len = tr.vlq()? as usize;
                    let data = tr.take(len)?;
                    match kind {
                        0x03 => {
                            name = String::from_utf8_lossy(data)
                                .chars()
                                .filter(|c| !c.is_control())
                                .take(128)
                                .collect()
                        }
                        0x51 if data.len() == 3 => {
                            let micros = (u32::from(data[0]) << 16)
                                | (u32::from(data[1]) << 8)
                                | u32::from(data[2]);
                            if micros == 0 {
                                return Err("MIDI tempo cannot be zero.".into());
                            }
                            if tempo_micros.is_some_and(|old| old != micros) {
                                return Err("MIDI changes tempo; Designer uses one tempo. Choose a constant-tempo MIDI file.".into());
                            }
                            tempo_micros = Some(micros);
                            tempo_tick = tempo_tick.min(tick);
                            bpm = 60_000_000. / f64::from(micros);
                        }
                        0x51 => return Err("Invalid MIDI tempo event length.".into()),
                        0x2F => {
                            if !data.is_empty() || tr.at != tr.bytes.len() {
                                return Err("Invalid MIDI end-of-track marker.".into());
                            }
                            ended = true;
                            break;
                        }
                        _ => {}
                    }
                    continue;
                }
                if event == 0xF0 || event == 0xF7 {
                    status = None;
                    let len = tr.vlq()? as usize;
                    tr.take(len)?;
                    continue;
                }
                if !(0x80..=0xEF).contains(&event) {
                    return Err("Unsupported system message in MIDI track.".into());
                }
                status = Some(event);
                let a = match first_data {
                    Some(a) => a,
                    None => tr.data()?,
                };
                let message = event & 0xF0;
                let b = if message == 0xC0 || message == 0xD0 {
                    0
                } else {
                    tr.data()?
                };
                let channel = event & 15;
                let part = channels.entry(channel).or_default();
                match message {
                    0x90 if b > 0 => {
                        notes += 1;
                        if notes > 100_000 {
                            return Err("MIDI exceeds 100,000 note starts.".into());
                        }
                        part.pending.entry(a).or_default().push_back((tick, b));
                    }
                    0x80 | 0x90 => part.off(a, tick, ppq),
                    0xB0 => match a {
                        1 => part.controller(
                            SoundDesignControllerKind::Modulation,
                            tick,
                            f64::from(b) / 127.,
                            ppq,
                        ),
                        7 | 11 => part.controller(
                            SoundDesignControllerKind::Expression,
                            tick,
                            f64::from(b) / 127.,
                            ppq,
                        ),
                        64 => part.controller(
                            SoundDesignControllerKind::Sustain,
                            tick,
                            if b >= 64 { 1. } else { 0. },
                            ppq,
                        ),
                        _ => {}
                    },
                    0xE0 => {
                        let raw = u16::from(a) | (u16::from(b) << 7);
                        part.controller(
                            SoundDesignControllerKind::PitchBend,
                            tick,
                            if raw <= 8192 {
                                f64::from(raw) / 16384.
                            } else {
                                0.5 + f64::from(raw - 8192) / 16382.
                            },
                            ppq,
                        );
                    }
                    _ => {}
                }
            }
            if !ended {
                return Err("MIDI track has no end-of-track marker.".into());
            }
            for (channel, mut builder) in channels {
                if builder.notes.is_empty() && builder.controllers.is_empty() {
                    continue;
                }
                builder
                    .notes
                    .sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
                builder
                    .controllers
                    .sort_by(|a, b| a.beat.total_cmp(&b.beat));
                parts.push(MIDIPart {
                    name: format!(
                        "{} · channel {}{}",
                        if name.is_empty() { "Unnamed" } else { &name },
                        channel + 1,
                        if channel == 9 { " (drums)" } else { "" }
                    ),
                    track,
                    channel,
                    notes: builder.notes,
                    controllers: builder.controllers,
                });
            }
            break;
        }
    }
    if r.at != r.bytes.len() {
        return Err("MIDI file has trailing data.".into());
    }
    if tempo_tick != u64::MAX && tempo_tick > 0 && tempo_micros != Some(500000) {
        return Err("MIDI changes from its initial tempo; Designer uses one tempo.".into());
    }
    if !(30. ..=300.).contains(&bpm) {
        return Err("MIDI tempo must be 30–300 BPM.".into());
    }
    if parts.is_empty() {
        return Err("This MIDI file has no supported notes or controllers.".into());
    }
    Ok(MIDIFile {
        parts,
        bpm,
        beats: total_ticks as f64 / f64::from(ppq),
        ppq,
    })
}
impl MIDIFile {
    /// Excerpts are explicit and never silently truncate a dense MIDI arrangement.
    pub fn excerpt(
        &self,
        index: usize,
        start: f64,
        length: f64,
    ) -> Result<SoundDesignPattern, String> {
        if !start.is_finite()
            || !length.is_finite()
            || start < 0.
            || !(0.25..=64.).contains(&length)
        {
            return Err("MIDI excerpt: nonnegative start and 0.25–64 beats.".into());
        }
        let part = self
            .parts
            .get(index)
            .ok_or("MIDI part index is outside this file.")?;
        let end = start + length;
        if start >= self.beats || end > self.beats + 0.000001 {
            return Err("Choose an excerpt within the MIDI arrangement.".into());
        }
        let mut notes = vec![];
        for note in part
            .notes
            .iter()
            .filter(|n| n.start < end && n.start + n.duration > start)
        {
            if notes.len() == 256 {
                return Err(
                    "This excerpt exceeds 256 notes; shorten the range or choose another part."
                        .into(),
                );
            }
            let lo = note.start.max(start);
            let hi = (note.start + note.duration).min(end);
            notes.push(SoundDesignNote {
                id: new_id().map_err(|e| e.to_string())?,
                pitch: note.pitch,
                velocity: note.velocity,
                start_beat: lo - start,
                duration_beats: hi - lo,
                articulation: None,
            });
        }
        if notes.is_empty() {
            return Err("This excerpt has no playable notes; choose another part or range.".into());
        }
        let mut controls = vec![];
        for kind in [
            SoundDesignControllerKind::Expression,
            SoundDesignControllerKind::Modulation,
            SoundDesignControllerKind::Sustain,
            SoundDesignControllerKind::PitchBend,
        ] {
            if let Some(before) = part
                .controllers
                .iter()
                .rfind(|p| p.kind == kind && p.beat < start)
            {
                controls.push(SoundDesignController {
                    kind: kind.clone(),
                    beat: 0.,
                    value: before.value,
                });
            }
            for p in part
                .controllers
                .iter()
                .filter(|p| p.kind == kind && p.beat >= start && p.beat < end)
            {
                if controls.len() == 4096 {
                    return Err(
                        "This excerpt exceeds 4096 controller points; shorten the range.".into(),
                    );
                }
                controls.push(SoundDesignController {
                    kind: kind.clone(),
                    beat: p.beat - start,
                    value: p.value,
                });
            }
        }
        controls.sort_by(|a, b| a.beat.total_cmp(&b.beat));
        serde_json::from_value(
            serde_json::json!({"beats":length,"bpm":self.bpm,"notes":notes,"controllers":controls}),
        )
        .map_err(|e| e.to_string())
    }
}
fn vlq(value: u32, out: &mut Vec<u8>) {
    let mut bytes = [0u8; 4];
    let mut i = 3;
    bytes[i] = (value & 127) as u8;
    let mut rest = value >> 7;
    while rest > 0 {
        i -= 1;
        bytes[i] = ((rest & 127) as u8) | 128;
        rest >>= 7;
    }
    out.extend_from_slice(&bytes[i..]);
}
pub fn encode(pattern: &SoundDesignPattern) -> Result<Vec<u8>, String> {
    let bpm = pattern.bpm.unwrap_or(120.);
    if !bpm.is_finite() || !(30. ..=300.).contains(&bpm) {
        return Err("MIDI tempo must be 30–300 BPM.".into());
    }
    let notes = pattern.notes.as_deref().unwrap_or(&[]);
    let controllers = pattern.controllers.as_deref().unwrap_or(&[]);
    if notes.len() > 100_000 || controllers.len() > 65_536 {
        return Err("MIDI export exceeds its bounded note/controller budget.".into());
    }
    let ppq = 960u16;
    let tick = |beat: f64| -> Result<u32, String> {
        let n = (beat * f64::from(ppq)).round();
        if !n.is_finite() || !(0. ..=65_536. * f64::from(ppq)).contains(&n) {
            return Err("MIDI event is outside the export timeline.".into());
        }
        Ok(n as u32)
    };
    let mut events: Vec<(u32, u8, Vec<u8>)> = vec![];
    for note in pattern.notes.as_deref().unwrap_or(&[]) {
        if !(0..=127).contains(&note.pitch)
            || !note.velocity.is_finite()
            || !(0. ..=1.).contains(&note.velocity)
            || !note.duration_beats.is_finite()
            || note.duration_beats <= 0.
        {
            return Err("Invalid MIDI note.".into());
        }
        if note.velocity == 0. {
            continue;
        }
        let start = tick(note.start_beat)?;
        let end = tick(note.start_beat + note.duration_beats)?.max(start.saturating_add(1));
        events.push((
            start,
            2,
            vec![
                0x90,
                note.pitch as u8,
                (note.velocity * 127.).round().clamp(1., 127.) as u8,
            ],
        ));
        events.push((end, 0, vec![0x80, note.pitch as u8, 0]));
    }
    for p in pattern.controllers.as_deref().unwrap_or(&[]) {
        if !p.value.is_finite() || !(0. ..=1.).contains(&p.value) {
            return Err("Invalid MIDI controller value.".into());
        }
        let bytes = match p.kind {
            SoundDesignControllerKind::PitchBend => {
                let value = if p.value <= 0.5 {
                    (p.value * 16384.).round() as u16
                } else {
                    (8192. + (p.value - 0.5) * 16382.).round() as u16
                };
                vec![0xE0, (value & 127) as u8, ((value >> 7) & 127) as u8]
            }
            _ => vec![
                0xB0,
                match p.kind {
                    SoundDesignControllerKind::Expression => 11,
                    SoundDesignControllerKind::Modulation => 1,
                    _ => 64,
                },
                if p.kind == SoundDesignControllerKind::Sustain {
                    if p.value >= 0.5 {
                        127
                    } else {
                        0
                    }
                } else {
                    (p.value * 127.).round() as u8
                },
            ],
        };
        events.push((tick(p.beat)?, 1, bytes));
    }
    if events.len() > 199_998 {
        return Err("MIDI export exceeds 200,000 events including tempo/end markers.".into());
    }
    events.sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
    let micros = (60_000_000. / bpm).round() as u32;
    let mut track = vec![
        0,
        0xFF,
        0x51,
        3,
        (micros >> 16) as u8,
        (micros >> 8) as u8,
        micros as u8,
    ];
    let mut position = 0;
    for (at, _, bytes) in events {
        vlq(at - position, &mut track);
        track.extend_from_slice(&bytes);
        position = at;
        if track.len() > MAX_FILE {
            return Err("MIDI export exceeds 4 MiB.".into());
        }
    }
    let end = tick(pattern.beats.unwrap_or(4.))?.max(position);
    vlq(end - position, &mut track);
    track.extend_from_slice(&[0xFF, 0x2F, 0]);
    let mut out = b"MThd".to_vec();
    out.extend_from_slice(&6u32.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&ppq.to_be_bytes());
    out.extend_from_slice(b"MTrk");
    out.extend_from_slice(&(track.len() as u32).to_be_bytes());
    out.extend_from_slice(&track);
    if out.len() > MAX_FILE {
        return Err("MIDI export exceeds 4 MiB.".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn smf(events: &[u8]) -> Vec<u8> {
        let mut out = b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk".to_vec();
        out.extend_from_slice(&(events.len() as u32).to_be_bytes());
        out.extend_from_slice(events);
        out
    }
    fn pattern() -> SoundDesignPattern {
        serde_json::from_value(serde_json::json!({"bpm":120,"beats":4,"notes":[{"id":new_id().unwrap(),"pitch":60,"velocity":0.8,"startBeat":0,"durationBeats":1}],"controllers":[{"kind":"pitchBend","beat":0,"value":0.5},{"kind":"expression","beat":0.5,"value":1}]})).unwrap()
    }
    #[test]
    fn export_roundtrip_exact_pitch_center_and_sequence() {
        let p = pattern();
        let b = encode(&p).unwrap();
        let file = parse(&b).unwrap();
        assert_eq!(file.parts.len(), 1);
        assert_eq!(file.beats, 4.);
        assert_eq!(file.parts[0].notes[0].pitch, 60);
        assert_eq!(file.parts[0].notes[0].duration, 1.);
        assert_eq!(file.parts[0].controllers[0].value, 0.5);
        let excerpt = file.excerpt(0, 0., 4.).unwrap();
        assert_eq!(excerpt.notes.as_ref().unwrap()[0].start_beat, 0.);
        assert_eq!(excerpt.bpm, Some(120.));
        assert_eq!(encode(&p).unwrap(), b);
    }
    #[test]
    fn running_status_zero_velocity_and_overlap_fifo() {
        let file = parse(&smf(&[
            0, 0x90, 60, 100, 0, 60, 90, 0x83, 0x60, 60, 0, 0x83, 0x60, 0x80, 60, 0, 0, 0xff, 0x2f,
            0,
        ]))
        .unwrap();
        let ns = &file.parts[0].notes;
        assert_eq!(ns.len(), 2);
        assert_eq!(ns[0].duration, 1.);
        assert_eq!(ns[1].duration, 2.);
        assert!(ns[0].velocity > ns[1].velocity);
    }
    #[test]
    fn excerpt_clips_crossing_notes_and_carries_controller_state() {
        let mut p = pattern();
        p.notes.as_mut().unwrap()[0].duration_beats = 4.;
        let f = parse(&encode(&p).unwrap()).unwrap();
        let x = f.excerpt(0, 1., 2.).unwrap();
        let n = &x.notes.as_ref().unwrap()[0];
        assert_eq!(n.start_beat, 0.);
        assert_eq!(n.duration_beats, 2.);
        assert!(x.controllers.unwrap().iter().all(|c| c.beat == 0.));
        assert!(f.excerpt(0, 3., 2.).is_err());
        assert!(f.excerpt(1, 0., 1.).is_err());
    }
    #[test]
    fn malformed_and_unsupported_files_reject_without_partial_patterns() {
        for bytes in [
            vec![0, 60, 100, 0, 0xff, 0x2f, 0],
            vec![0, 0x90, 60, 0x90],
            vec![0, 0xff, 0x2f, 0, 0],
            vec![0, 0x90, 60, 100],
        ] {
            assert!(parse(&smf(&bytes)).is_err());
        }
        let mut b = encode(&pattern()).unwrap();
        b[12] = 0x80;
        assert!(parse(&b).is_err());
        let mut b = encode(&pattern()).unwrap();
        b[9] = 2;
        assert!(parse(&b).is_err());
        assert!(parse(&vec![0; MAX_FILE + 1]).is_err());
    }
    #[test]
    fn variable_tempo_rejects_and_density_requires_explicit_shorter_range() {
        assert!(parse(&smf(&[
            0, 0xff, 0x51, 3, 7, 0xa1, 0x20, 0x83, 0x60, 0xff, 0x51, 3, 6, 0x1a, 0x80, 0, 0xff,
            0x2f, 0
        ]))
        .is_err());
        let mut p = pattern();
        p.notes = Some(
            (0..257)
                .map(|_| p.notes.as_ref().unwrap()[0].clone())
                .collect(),
        );
        let f = parse(&encode(&p).unwrap()).unwrap();
        assert!(f.excerpt(0, 0., 4.).is_err());
        p.bpm = Some(29.);
        assert!(encode(&p).is_err());
    }
}
