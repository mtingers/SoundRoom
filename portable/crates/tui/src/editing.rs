use soundroom_client::{auth::new_id, models::*};
use std::collections::BTreeSet;
#[derive(Clone)]
pub struct Clipboard {
    pub server_id: String,
    pub source_track: String,
    pub clips: BTreeSet<String>,
    pub range: Option<(i64, i64)>,
    pub frozen: Option<Vec<Clip>>,
}
impl Clipboard {
    pub fn freeze(&mut self, source: &Track) -> Result<(), String> {
        let copied: Vec<_> = source
            .clips
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .filter(|c| self.clips.contains(&c.id))
            .collect();
        clipboard_bound(&copied)?;
        self.frozen = Some(copied.into_iter().cloned().collect());
        Ok(())
    }
    pub fn paste(
        &self,
        snapshot: &RemoteSnapshot,
        destination: &str,
        frame: i64,
        maximum: i64,
    ) -> Result<Track, String> {
        if self.server_id != snapshot.server_id {
            return Err("Clipboard belongs to another audio host.".into());
        }
        let tracks = snapshot.session.tracks.as_deref().unwrap_or(&[]);
        let source = tracks
            .iter()
            .find(|t| t.id.as_deref() == Some(&self.source_track))
            .ok_or("Copied source track was removed.")?;
        let mut target = tracks
            .iter()
            .find(|t| t.id.as_deref() == Some(destination))
            .cloned()
            .ok_or("Destination track was removed.")?;
        let copied: Vec<_> = self
            .frozen
            .as_deref()
            .or(source.clips.as_deref())
            /* the cut clipboard retains immutable source states after removal */
            .unwrap_or(&[])
            .iter()
            .filter(|c| self.clips.contains(&c.id))
            .collect();
        if copied.len() != self.clips.len() || copied.is_empty() {
            return Err("A copied clip was removed; copy the current clips again.".into());
        }
        let origin = self
            .range
            .map_or_else(|| copied.iter().map(|c| c.start).min().unwrap(), |r| r.0);
        let destination_clips = target.clips.get_or_insert_with(Vec::new);
        if copied.len() + destination_clips.len() > 4096 {
            return Err("Paste would exceed 4096 clips on one track.".into());
        }
        for clip in copied {
            let mut next = if let Some((lo, hi)) = self.range {
                let Some(part) = fragment(clip, lo, hi, false)? else {
                    continue;
                };
                part
            } else {
                clip.clone()
            };
            next.id = new_id().map_err(|e| e.to_string())?;
            next.start = frame
                .checked_add(next.start - origin)
                .ok_or("Paste exceeds the timeline.")?;
            if next.start < 0
                || next
                    .start
                    .checked_add(next.length)
                    .map_or(true, |end| end > maximum)
            {
                return Err("Paste exceeds the timeline.".into());
            }
            destination_clips.push(next);
        }
        Ok(target)
    }
}

struct ClipboardCounter(usize);
impl std::io::Write for ClipboardCounter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.checked_add(bytes.len()).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "Clipboard too large")
        })?;
        if self.0 > 32 * 1024 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Clipboard limited to 32 MiB",
            ));
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn clipboard_bound<T: serde::Serialize>(value: &T) -> Result<(), String> {
    serde_json::to_writer(ClipboardCounter(0), value)
        .map_err(|_| "This clipboard exceeds its 32 MiB memory limit.".into())
}

/// Exact-frame positions remain independent of the current time/beat display.
pub fn position(text: &str, rate: f64, bpm: f64, maximum: i64) -> Result<i64, String> {
    let (unit, value) = text.split_once(':').unwrap_or(("frame", text));
    let number = value
        .parse::<f64>()
        .map_err(|_| "Position: integer frame, seconds:number or beats:number.")?;
    let frame = match unit {
        "frame" if number.fract() == 0. => number,
        "seconds" => number * rate,
        "ms" => number * rate / 1000.,
        "beats" => number * rate * 60. / bpm,
        _ => return Err("Position: integer frame, seconds:number or beats:number.".into()),
    };
    if !frame.is_finite() || !(0. ..=maximum as f64).contains(&frame) {
        return Err("Position exceeds this host's timeline.".into());
    }
    Ok(frame.round() as i64)
}
fn reset_envelope(clip: &mut Clip) {
    clip.render_envelope = None;
    clip.fade_reference_offset = None;
    clip.fade_reference_length = None;
}
pub fn trim(clip: &Clip, source_frames: i64, start: i64, end: i64) -> Result<Clip, String> {
    if start < 0
        || end <= start
        || start < clip.start - clip.offset
        || end - start > source_frames - (clip.offset + start - clip.start)
    {
        return Err("Trim must retain audio within its source file.".into());
    }
    let mut next = clip.clone();
    next.start = start;
    next.offset += start - clip.start;
    next.length = end - start;
    if next.start != clip.start || next.length != clip.length {
        next.sound_design = None;
        next.drum_pattern = None;
        next.fade_in_frames = next.fade_in_frames.map(|n| n.min(next.length));
        next.fade_out_frames = next.fade_out_frames.map(|n| n.min(next.length));
        reset_envelope(&mut next);
    }
    Ok(next)
}
/// Keep the original envelope's source coordinates when cutting out a fragment.
pub fn fragment(clip: &Clip, lo: i64, hi: i64, new_identity: bool) -> Result<Option<Clip>, String> {
    let start = clip.start.max(lo);
    let end = (clip.start + clip.length).min(hi);
    if end <= start {
        return Ok(None);
    }
    let mut next = clip.clone();
    if new_identity {
        next.id = new_id().map_err(|e| e.to_string())?;
    }
    let delta = start - clip.start;
    next.start = start;
    next.offset += delta;
    next.length = end - start;
    if delta != 0 || next.length != clip.length {
        if let Some(envelope) = next.render_envelope.as_mut() {
            envelope.reference_offset = Some(envelope.reference_offset.unwrap_or(0) + delta);
        }
        if [
            clip.fade_in_frames,
            clip.fade_out_frames,
            clip.edge_fade_frames,
        ]
        .iter()
        .any(|v| v.unwrap_or(0) > 0)
        {
            next.fade_reference_length = Some(clip.fade_reference_length.unwrap_or(clip.length));
            next.fade_reference_offset = Some(clip.fade_reference_offset.unwrap_or(0) + delta);
        }
        if delta > 0 {
            next.cut_fade_in_frames = clip.edge_fade_frames;
        }
        if end != clip.start + clip.length {
            next.cut_fade_out_frames = clip.edge_fade_frames;
        }
        next.sound_design = None;
        next.drum_pattern = None;
    }
    Ok(Some(next))
}
pub fn delete_range(track: &Track, lo: i64, hi: i64) -> Result<Track, String> {
    if lo < 0 || hi <= lo {
        return Err("Choose a nonempty range.".into());
    }
    let mut next = track.clone();
    let mut clips = vec![];
    for clip in track.clips.as_deref().unwrap_or(&[]) {
        if clip.start >= hi || clip.start + clip.length <= lo {
            clips.push(clip.clone());
            continue;
        }
        if let Some(part) = fragment(clip, clip.start, lo, false)? {
            clips.push(part);
        }
        if let Some(part) = fragment(clip, hi, clip.start + clip.length, true)? {
            clips.push(part);
        }
    }
    if clips.len() > 4096 {
        return Err("Cut would exceed 4096 clips on one track.".into());
    }
    next.clips = Some(clips);
    Ok(next)
}
pub fn from_history(clip: &Clip, state: &ClipState, new_identity: bool) -> Result<Clip, String> {
    let mut next = clip.clone();
    next.asset_id = state.asset_id.clone();
    next.name = state.name.clone();
    next.offset = state.offset;
    next.length = state.length;
    next.sound_design = state.sound_design.clone();
    next.drum_pattern = state.drum_pattern.clone();
    next.render_envelope = state.render_envelope.clone();
    next.fade_in_frames = state.fade_in_frames;
    next.fade_out_frames = state.fade_out_frames;
    next.fade_reference_offset = state.fade_reference_offset;
    next.fade_reference_length = state.fade_reference_length;
    next.edge_fade_frames = state.edge_fade_frames;
    next.edge_protection_disabled = state.edge_protection_disabled;
    next.cut_fade_in_frames = state.cut_fade_in_frames;
    next.cut_fade_out_frames = state.cut_fade_out_frames;
    if new_identity {
        next.id = new_id().map_err(|e| e.to_string())?;
        next.previous_states = None;
    }
    Ok(next)
}
pub fn new_track(name: String, color: i64) -> Result<Track, String> {
    serde_json::from_value(serde_json::json!({"id":new_id().map_err(|e|e.to_string())?,"name":name,"color":color,"clips":[],"recordingTakes":[],"gain":1,"pan":0,"armed":false,"muted":false,"solo":false,"realtimeEffects":[],"mixAutomation":{}})).map_err(|e|e.to_string())
}
/// Splitting the curve domain preserves nonlinear and dB interpolation at frame zero.
pub fn relocate_curve(
    points: &[AutomationPoint],
    shift: i64,
    baseline: f32,
    decibels: bool,
    maximum: i64,
) -> Result<Vec<AutomationPoint>, String> {
    let mut retained = points.to_vec();
    retained.sort_by_key(|p| p.frame);
    if shift < 0
        && retained.iter().any(|p| p.frame < -shift)
        && !retained.iter().any(|p| p.frame == -shift)
    {
        let boundary = -shift;
        let value = crate::automation::sample(&retained, boundary, baseline, decibels);
        let next = retained.partition_point(|p| p.frame < boundary);
        let mut point = AutomationPoint {
            frame: boundary,
            value,
            curve: Some(AutomationCurve::Linear),
            curve_start: Some(0.),
            curve_end: Some(1.),
        };
        if next > 0 && next < retained.len() {
            let next_frame = retained[next].frame;
            let a = &mut retained[next - 1];
            let start = a.curve_start.unwrap_or(0.);
            let end = a.curve_end.unwrap_or(1.);
            let middle =
                start + (end - start) * (boundary - a.frame) as f32 / (next_frame - a.frame) as f32;
            point.curve = a.curve.clone();
            point.curve_start = Some(middle);
            point.curve_end = Some(end);
            a.curve_end = Some(middle);
        }
        retained.insert(next, point);
    }
    let mut result = vec![];
    for mut p in retained {
        p.frame = p
            .frame
            .checked_add(shift)
            .ok_or("Automation exceeds the timeline.")?;
        if p.frame < 0 {
            continue;
        }
        if p.frame > maximum {
            return Err("Pasted automation exceeds the timeline.".into());
        }
        result.push(p);
    }
    Ok(result)
}
#[derive(Clone)]
pub struct TrackClipboard {
    pub server_id: String,
    pub source: Track,
    pub revision: i64,
    pub automation: bool,
}
impl TrackClipboard {
    pub fn copy(
        snapshot: &RemoteSnapshot,
        source: &Track,
        automation: bool,
    ) -> Result<Self, String> {
        clipboard_bound(source)?;
        let id = source.id.as_ref().ok_or("Missing track identity.")?;
        let revision = snapshot
            .track_revisions
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(id))
            .map(|(_, v)| *v)
            .ok_or("Refresh the source track before copying.")?;
        let mut source = source.clone();
        if !automation {
            source.mix_automation = None;
            for effect in source.realtime_effects.as_mut().into_iter().flatten() {
                effect.automation = None;
                effect.parameter_automation = None;
                effect.pitch_edits = None;
                effect.pitch_edits_only = Some(false);
            }
        }
        Ok(Self {
            server_id: snapshot.server_id.clone(),
            source,
            revision,
            automation,
        })
    }
    pub fn paste(
        &self,
        snapshot: &RemoteSnapshot,
        frame: i64,
        maximum: i64,
        metadata: &[RemoteEffectMetadata],
    ) -> Result<Track, String> {
        if snapshot.server_id != self.server_id {
            return Err("Clipboard belongs to another audio host.".into());
        }
        let source_id = self
            .source
            .id
            .as_ref()
            .ok_or("Missing source track identity.")?;
        if !snapshot
            .session
            .tracks
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .any(|t| t.id.as_deref() == Some(source_id))
        {
            return Err("Copied source track was removed.".into());
        }
        let source_revision = snapshot
            .track_revisions
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(source_id))
            .map(|(_, v)| *v);
        if source_revision != Some(self.revision) {
            return Err("Source track changed; copy its current state again.".into());
        }
        let origin = self
            .source
            .clips
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(|c| c.start)
            .min()
            .unwrap_or(0);
        let shift = frame
            .checked_sub(origin)
            .ok_or("Paste exceeds the timeline.")?;
        let mut target = self.source.clone();
        target.id = Some(new_id().map_err(|e| e.to_string())?);
        target.armed = Some(false);
        let base = format!("{} copy", self.source.name.as_deref().unwrap_or("Track"));
        let mut name = base.clone();
        let mut suffix = 2;
        while snapshot
            .session
            .tracks
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .any(|t| t.name.as_deref() == Some(&name))
        {
            name = format!("{base} {suffix}");
            suffix += 1;
        }
        target.name = Some(name);
        target.clips = Some(relocate_clips(
            self.source.clips.as_deref().unwrap_or(&[]),
            shift,
            maximum,
        )?);
        let active = self
            .source
            .recording_takes
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .position(|t| Some(&t.id) == self.source.active_take_id.as_ref());
        for take in target.recording_takes.as_mut().into_iter().flatten() {
            take.id = new_id().map_err(|e| e.to_string())?;
            take.clips = relocate_clips(&take.clips, shift, maximum)?;
        }
        target.active_take_id = active.and_then(|i| {
            target
                .recording_takes
                .as_ref()
                .and_then(|ts| ts.get(i))
                .map(|t| t.id.clone())
        });
        for (key, points) in target
            .mix_automation
            .as_mut()
            .into_iter()
            .flat_map(|map| map.iter_mut())
        {
            let baseline = if key == "volume" {
                (target.gain.unwrap_or(1.) / 1.5).clamp(0., 1.)
            } else {
                (target.pan.unwrap_or(0.) + 1.) / 2.
            };
            *points = relocate_curve(
                points,
                shift,
                baseline,
                key == "volume"
                    && target.volume_automation_scale == Some(AutomationValueScale::Decibels),
                maximum,
            )?;
        }
        for effect in target.realtime_effects.as_mut().into_iter().flatten() {
            effect.id = Some(new_id().map_err(|e| e.to_string())?);
            if let Some(points) = effect.automation.as_mut() {
                *points =
                    relocate_curve(points, shift, effect.amount.unwrap_or(0.5), false, maximum)?;
            }
            let kind = serde_json::to_value(&effect.kind).map_err(|e| e.to_string())?;
            for (key, points) in effect
                .parameter_automation
                .as_mut()
                .into_iter()
                .flat_map(|map| map.iter_mut())
            {
                let descriptor = metadata
                    .iter()
                    .find(|m| Some(m.id.as_str()) == kind.as_str())
                    .and_then(|m| m.parameters.iter().find(|p| &p.id == key));
                let baseline = descriptor.map_or(0.5, |p| {
                    crate::automation::normalize(
                        effect
                            .parameters
                            .as_ref()
                            .and_then(|ps| ps.get(key))
                            .copied()
                            .unwrap_or(p.default_value),
                        p,
                    )
                });
                *points = relocate_curve(points, shift, baseline, false, maximum)?;
            }
            if let Some(regions) = effect.pitch_edits.as_mut() {
                regions.retain(|r| r.end_frame + shift > 0);
                for region in regions {
                    region.id = Some(new_id().map_err(|e| e.to_string())?);
                    region.start_frame = (region.start_frame + shift).max(0);
                    region.end_frame += shift;
                    if region.end_frame > maximum {
                        return Err("Pasted pitch edits exceed the timeline.".into());
                    }
                    let original = region.points.as_deref().unwrap_or(&[]);
                    let points: Vec<_> = original
                        .iter()
                        .map(|p| AutomationPoint {
                            frame: p.frame,
                            value: p.note,
                            curve: p.curve.clone(),
                            curve_start: p.curve_start,
                            curve_end: p.curve_end,
                        })
                        .collect();
                    region.points = Some(
                        relocate_curve(
                            &points,
                            shift,
                            points.first().map_or(60., |p| p.value),
                            false,
                            maximum,
                        )?
                        .into_iter()
                        .map(|p| {
                            Ok(PitchEditPoint {
                                id: Some(new_id().map_err(|e| e.to_string())?),
                                frame: p.frame,
                                note: p.value,
                                curve: p.curve,
                                curve_start: p.curve_start,
                                curve_end: p.curve_end,
                            })
                        })
                        .collect::<Result<_, String>>()?,
                    );
                }
            }
            if let Some(ranges) = effect.baked_ranges.as_mut() {
                ranges.retain(|r| r.end_frame + shift > 0);
                for r in ranges {
                    r.start_frame = (r.start_frame + shift).max(0);
                    r.end_frame += shift;
                    if r.end_frame > maximum {
                        return Err("Pasted baked ranges exceed the timeline.".into());
                    }
                }
            }
        }
        Ok(target)
    }
}
fn relocate_clips(clips: &[Clip], shift: i64, maximum: i64) -> Result<Vec<Clip>, String> {
    let mut result = vec![];
    for c in clips {
        let mut moved = c.clone();
        moved.id = new_id().map_err(|e| e.to_string())?;
        moved.start = c
            .start
            .checked_add(shift)
            .ok_or("Paste exceeds the timeline.")?;
        if moved.start + moved.length > maximum {
            return Err("Pasted audio exceeds the timeline.".into());
        }
        if let Some(part) = fragment(&moved, 0, moved.start + moved.length, false)? {
            result.push(part);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::sample;
    fn clip() -> Clip {
        serde_json::from_value(serde_json::json!({"id":"00000000-0000-0000-0000-000000000004","assetID":"00000000-0000-0000-0000-000000000005","name":"Verse","start":200,"offset":100,"length":400,"edgeFadeFrames":5,"fadeInFrames":30,"renderEnvelope":{"referenceOffset":7,"referenceLength":700},"soundDesign":{"instrument":"studioPiano","notes":[]}})).unwrap()
    }
    fn snapshot() -> RemoteSnapshot {
        serde_json::from_value(serde_json::json!({"serverID":"00000000-0000-0000-0000-000000000001","serverName":"Host","revision":8,"trackRevisions":{"00000000-0000-0000-0000-000000000002":7},"session":{"tracks":[]},"transport":{"frame":0,"isPlaying":false,"isRecording":false,"isPreparing":false,"hostTime":0},"devices":[],"inputDeviceID":0,"outputDeviceID":0,"assets":[],"status":"Ready","projectName":"Song","peers":[]})).unwrap()
    }
    fn source() -> Track {
        let mut t = new_track("Voice".into(), 0).unwrap();
        t.id = Some("00000000-0000-0000-0000-000000000002".into());
        t.clips = Some(vec![clip()]);
        t.armed = Some(true);
        t.volume_automation_scale = Some(AutomationValueScale::Decibels);
        let points:Vec<AutomationPoint>=serde_json::from_value(serde_json::json!([{"frame":100,"value":0.1,"curve":"smooth","curveStart":0.15,"curveEnd":0.85},{"frame":500,"value":0.9}])).unwrap();
        t.mix_automation = Some(std::collections::BTreeMap::from([
            ("volume".into(), points.clone()),
            ("pan".into(), points.clone()),
        ]));
        t.realtime_effects=Some(vec![serde_json::from_value(serde_json::json!({"id":"00000000-0000-0000-0000-000000000006","kind":"pitchCorrection","pitchEditsOnly":true,"amount":0.5,"automation":points,"parameterAutomation":{"mix":points},"pitchEdits":[{"id":"00000000-0000-0000-0000-000000000007","startFrame":100,"endFrame":550,"points":[{"id":"00000000-0000-0000-0000-000000000008","frame":100,"note":60,"curve":"easeIn","curveStart":0.1,"curveEnd":0.95},{"id":"00000000-0000-0000-0000-000000000009","frame":500,"note":72}]}],"bakedRanges":[{"startFrame":100,"endFrame":550}]})).unwrap()]);
        t.recording_takes = Some(vec![RecordingTake {
            id: "00000000-0000-0000-0000-000000000010".into(),
            name: "Take 1".into(),
            clips: vec![clip()],
        }]);
        t.active_take_id = t.recording_takes.as_ref().map(|t| t[0].id.clone());
        t
    }
    #[test]
    fn trim_and_fragments_keep_source_audio_and_retire_generated_editing_correctly() {
        let c = clip();
        let part = fragment(&c, 250, 450, false).unwrap().unwrap();
        assert_eq!((part.start, part.offset, part.length), (250, 150, 200));
        assert_eq!(part.render_envelope.unwrap().reference_offset, Some(57));
        assert_eq!(
            (part.fade_reference_offset, part.fade_reference_length),
            (Some(50), Some(400))
        );
        assert!(part.sound_design.is_none());
        assert_eq!(part.cut_fade_in_frames, Some(5));
        assert_eq!(part.cut_fade_out_frames, Some(5));
        let extended = trim(&c, 1000, 150, 750).unwrap();
        assert_eq!((extended.offset, extended.length), (50, 600));
        assert!(extended.render_envelope.is_none());
        assert!(trim(&c, 1000, 99, 300).is_err());
        assert!(trim(&c, 1000, 200, 2000).is_err());
        let untouched = trim(&c, 1000, 200, 600).unwrap();
        assert_eq!(untouched, c);
    }
    #[test]
    fn cutting_a_range_keeps_two_audio_fragments_with_independent_ids_and_envelopes() {
        let mut t = new_track("Voice".into(), 0).unwrap();
        t.clips = Some(vec![clip()]);
        let cut = delete_range(&t, 300, 400).unwrap();
        let clips = cut.clips.unwrap();
        assert_eq!(clips.len(), 2);
        assert_eq!(
            (clips[0].start, clips[0].offset, clips[0].length),
            (200, 100, 100)
        );
        assert_eq!(
            (clips[1].start, clips[1].offset, clips[1].length),
            (400, 300, 200)
        );
        assert_eq!(clips[0].id, clip().id);
        assert_ne!(clips[1].id, clip().id);
        assert_eq!(
            clips[1].render_envelope.as_ref().unwrap().reference_offset,
            Some(207)
        );
        assert_eq!(t.clips.unwrap()[0], clip());
    }
    #[test]
    fn track_copy_preserves_every_frame_of_nonlinear_volume_pan_and_effect_automation() {
        let source = source();
        let mut s = snapshot();
        s.session.tracks = Some(vec![source.clone()]);
        let copy = TrackClipboard::copy(&s, &source, true).unwrap();
        let pasted = copy.paste(&s, 0, 1000, &[]).unwrap();
        for key in ["volume", "pan"] {
            let original = &source.mix_automation.as_ref().unwrap()[key];
            let moved = &pasted.mix_automation.as_ref().unwrap()[key];
            for frame in 0..350 {
                assert!(
                    (sample(original, frame + 200, 0.5, key == "volume")
                        - sample(moved, frame, 0.5, key == "volume"))
                    .abs()
                        < 0.00001,
                    "{key} frame {frame}"
                );
            }
            assert_eq!(moved[0].frame, 0);
            assert!(moved[0].curve_start.unwrap() > 0.15);
        }
        let original = &source.realtime_effects.as_ref().unwrap()[0];
        let moved = &pasted.realtime_effects.as_ref().unwrap()[0];
        for frame in 0..350 {
            assert!(
                (sample(
                    original.automation.as_ref().unwrap(),
                    frame + 200,
                    0.5,
                    false
                ) - sample(moved.automation.as_ref().unwrap(), frame, 0.5, false))
                .abs()
                    < 0.00001
            );
        }
        let curves = |effect: &RealtimeEffect| {
            effect.pitch_edits.as_ref().unwrap()[0]
                .points
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
        for frame in 0..350 {
            assert!(
                (sample(&curves(original), frame + 200, 60., false)
                    - sample(&curves(moved), frame, 60., false))
                .abs()
                    < 0.00002
            );
        }
        assert_ne!(pasted.id, source.id);
        assert_ne!(moved.id, original.id);
        assert_eq!(pasted.armed, Some(false));
        assert_ne!(
            pasted.recording_takes.as_ref().unwrap()[0].id,
            source.recording_takes.as_ref().unwrap()[0].id
        );
        assert_eq!(
            pasted.active_take_id,
            Some(pasted.recording_takes.as_ref().unwrap()[0].id.clone())
        );
        assert_eq!(
            (
                moved.baked_ranges.as_ref().unwrap()[0].start_frame,
                moved.baked_ranges.as_ref().unwrap()[0].end_frame
            ),
            (0, 350)
        );
        assert!(copy.paste(&s, 999, 1000, &[]).is_err());
        s.track_revisions.insert(source.id.clone().unwrap(), 8);
        assert!(copy.paste(&s, 0, 1000, &[]).is_err());
    }
    #[test]
    fn copying_without_automation_keeps_baked_audio_protection_and_static_sound() {
        let source = source();
        let mut s = snapshot();
        s.session.tracks = Some(vec![source.clone()]);
        let pasted = TrackClipboard::copy(&s, &source, false)
            .unwrap()
            .paste(&s, 0, 1000, &[])
            .unwrap();
        let effect = &pasted.realtime_effects.as_ref().unwrap()[0];
        assert!(effect.automation.is_none());
        assert!(effect.parameter_automation.is_none());
        assert!(effect.pitch_edits.is_none());
        assert_eq!(effect.pitch_edits_only, Some(false));
        assert_eq!(
            effect.amount,
            source.realtime_effects.as_ref().unwrap()[0].amount
        );
        assert_eq!(
            (
                effect.baked_ranges.as_ref().unwrap()[0].start_frame,
                effect.baked_ranges.as_ref().unwrap()[0].end_frame
            ),
            (0, 350)
        );
    }
    #[test]
    fn restoring_history_retains_timeline_placement_and_replaces_all_audio_provenance() {
        let state:ClipState=serde_json::from_value(serde_json::json!({"id":"00000000-0000-0000-0000-000000000011","operation":"Reverb","createdAt":0,"assetID":"00000000-0000-0000-0000-000000000012","name":"Original","offset":20,"length":500,"edgeProtectionDisabled":true,"cutFadeOutFrames":4})).unwrap();
        let restored = from_history(&clip(), &state, false).unwrap();
        assert_eq!(restored.start, 200);
        assert_eq!(restored.asset_id, state.asset_id);
        assert_eq!(restored.offset, 20);
        assert_eq!(restored.length, 500);
        assert!(restored.render_envelope.is_none());
        assert!(restored.fade_in_frames.is_none());
        assert_eq!(restored.edge_protection_disabled, Some(true));
        let new = from_history(&clip(), &state, true).unwrap();
        assert_ne!(new.id, clip().id);
        assert!(new.previous_states.is_none());
    }
    #[test]
    fn position_units_are_precise_bounded_and_do_not_depend_on_display_mode() {
        assert_eq!(
            position("beats:4.25", 48000., 123., 1_000_000).unwrap(),
            99512
        );
        assert_eq!(position("ms:5", 48000., 123., 1_000_000).unwrap(), 240);
        assert!(position("NaN", 48000., 123., 1_000_000).is_err());
        assert!(position("1.25", 48000., 123., 1_000_000).is_err());
        assert!(position("seconds:99", 48000., 123., 1_000_000).is_err());
    }
}
