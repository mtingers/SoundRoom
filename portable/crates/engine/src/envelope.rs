use crate::model::{Clip, Track};
/// Clip envelopes are resolved on a worker before rendering, matching SoundroomCore/ClipEnvelope.swift.
#[derive(Clone, Debug)]
pub struct Envelope {
    pub clip: Clip,
    pub source_id: String,
    attack: u64,
    release: u64,
    boundary_attack: u64,
    boundary_release: u64,
    cross_in: Option<[i64; 2]>,
    cross_out: Option<[i64; 2]>,
}
fn ramp(frame: u64, frames: u64) -> f32 {
    if frames == 0 {
        1.
    } else {
        (frame as f32 / frames as f32).min(1.)
    }
}
impl Envelope {
    /// Prepared graph projection excludes authored names, history, instrument
    /// recipes and unknown metadata. Source/fade fields retain their exact values.
    pub(crate) fn resolve_playback(track: &Track) -> Vec<Self> {
        let mut envelopes = Self::resolve(track);
        for e in &mut envelopes {
            e.clip.id = String::new();
            e.clip.name = String::new();
            e.clip.asset_id = String::new();
            e.clip.previous_states = Vec::new();
            e.clip.extra.clear();
            if let Some(frozen) = &mut e.clip.render_envelope {
                frozen.extra.clear();
                frozen.additional_boundaries.shrink_to_fit();
            }
        }
        envelopes
    }
    /// The resolved envelope as a frozen render envelope (Swift
    /// `ClipRenderEnvelope(_ envelope:)`). An already frozen clip keeps its
    /// envelope, records its cut window and appends a new boundary once.
    pub fn frozen(&self) -> crate::model::FrozenEnvelope {
        if let Some(existing) = &self.clip.render_envelope {
            let mut frozen = existing.clone();
            frozen.cut_reference_offset = Some(existing.reference_offset);
            frozen.cut_reference_length = Some(self.clip.length);
            if self.boundary_attack > 0 || self.boundary_release > 0 {
                let boundary = crate::model::Boundary {
                    start: existing.reference_offset,
                    length: self.clip.length,
                    attack: self.boundary_attack,
                    release: self.boundary_release,
                };
                if !frozen.additional_boundaries.contains(&boundary) {
                    frozen.additional_boundaries.push(boundary);
                }
            }
            return frozen;
        }
        let start = self.clip.start as i64;
        let relative = |range: Option<[i64; 2]>| {
            range.map(|[a, b]| [(a - start).max(0) as u64, (b - start).max(0) as u64])
        };
        crate::model::FrozenEnvelope {
            reference_length: self.clip.length,
            reference_offset: 0,
            attack_frames: self.attack,
            release_frames: self.release,
            boundary_attack_frames: self.boundary_attack,
            boundary_release_frames: self.boundary_release,
            crossfade_in: relative(self.cross_in),
            crossfade_out: relative(self.cross_out),
            cut_reference_offset: None,
            cut_reference_length: None,
            additional_boundaries: vec![],
            extra: Default::default(),
        }
    }
    /// Swift `ClipCombining.losslessJoin`: rejoin start-ordered pieces of one
    /// source without reading PCM, only when the envelope algebra proves the
    /// joined clip sounds identical. `envelopes` are resolved on their track.
    pub fn lossless_join(clips: &[Clip], envelopes: &[&Self]) -> Option<Clip> {
        let (first, last) = (clips.first()?, clips.last()?);
        if clips.len() != envelopes.len()
            || clips
                .iter()
                .any(|c| c.render_envelope.is_some() || c.asset_id != first.asset_id)
        {
            return None;
        }
        for pair in clips.windows(2) {
            if pair[0].start + pair[0].length != pair[1].start
                || pair[0].offset + pair[0].length != pair[1].offset
            {
                return None;
            }
        }
        let n = clips.len();
        for (i, state) in envelopes.iter().enumerate() {
            if i > 0 && (state.boundary_attack != 0 || state.cross_in.is_some()) {
                return None;
            }
            if i < n - 1 && (state.boundary_release != 0 || state.cross_out.is_some()) {
                return None;
            }
        }
        let (first_end, last_start) = ((first.start + first.length) as i64, last.start as i64);
        if envelopes[0].cross_in.is_some_and(|r| r[1] > first_end)
            || envelopes[n - 1]
                .cross_out
                .is_some_and(|r| r[0] < last_start)
        {
            return None;
        }
        if let Some(length) = first.fade_reference_length {
            let same_position = |c: &Clip| {
                first
                    .fade_reference_offset
                    .unwrap_or(0)
                    .checked_add(c.start - first.start)
                    == Some(c.fade_reference_offset.unwrap_or(0))
            };
            if !clips.iter().all(|c| {
                c.fade_reference_length == Some(length)
                    && same_position(c)
                    && c.fade_in_frames == first.fade_in_frames
                    && c.fade_out_frames == first.fade_out_frames
            }) || !envelopes
                .iter()
                .all(|e| e.attack == envelopes[0].attack && e.release == envelopes[0].release)
            {
                return None;
            }
        } else {
            if !clips.iter().all(|c| {
                c.fade_reference_length.is_none()
                    && c.fade_in_frames.unwrap_or(0) == 0
                    && c.fade_out_frames.unwrap_or(0) == 0
            }) {
                return None;
            }
            for (i, state) in envelopes.iter().enumerate() {
                if (i > 0 && state.attack != 0) || (i < n - 1 && state.release != 0) {
                    return None;
                }
            }
        }
        let mut combined = first.clone();
        combined.length = last.start + last.length - first.start;
        combined.extra.remove("soundDesign");
        combined.extra.remove("drumPattern");
        combined.cut_fade_out_frames = last.cut_fade_out_frames;
        let mut frozen = envelopes[0].frozen();
        let tail = envelopes[n - 1];
        frozen.reference_length = combined.length;
        frozen.release_frames = tail.release;
        frozen.boundary_release_frames = tail.boundary_release;
        let start = first.start as i64;
        frozen.crossfade_out = tail
            .cross_out
            .map(|[a, b]| [(a - start).max(0) as u64, (b - start).max(0) as u64]);
        combined.render_envelope = Some(frozen);
        Some(combined)
    }
    pub fn resolve(track: &Track) -> Vec<Self> {
        let mut sorted = track.clips.clone();
        sorted.sort_by(|a, b| a.start.cmp(&b.start).then(a.id.cmp(&b.id)));
        let mut result: Vec<Self> = sorted
            .iter()
            .map(|c| {
                let protected = c.edge_protection_disabled != Some(true);
                let limit = ((c.fade_reference_length.unwrap_or(c.length).max(1) - 1) / 2).max(1);
                let cut_limit = ((c.length.max(1) - 1) / 2).max(1);
                if let Some(e) = &c.render_envelope {
                    let origin = e.cut_reference_offset.unwrap_or(0);
                    let length = e.cut_reference_length.unwrap_or(e.reference_length);
                    let shift = c.start as i64 - e.reference_offset as i64;
                    Self {
                        clip: c.clone(),
                        source_id: c.asset_id.to_uppercase(),
                        attack: e.attack_frames,
                        release: e.release_frames,
                        boundary_attack: if protected && e.reference_offset != origin {
                            c.cut_fade_in_frames.unwrap_or(0).min(cut_limit)
                        } else {
                            0
                        },
                        boundary_release: if protected
                            && e.reference_offset + c.length != origin + length
                        {
                            c.cut_fade_out_frames.unwrap_or(0).min(cut_limit)
                        } else {
                            0
                        },
                        cross_in: e
                            .crossfade_in
                            .map(|r| [r[0] as i64 + shift, r[1] as i64 + shift]),
                        cross_out: e
                            .crossfade_out
                            .map(|r| [r[0] as i64 + shift, r[1] as i64 + shift]),
                    }
                } else {
                    let edge = if protected {
                        c.edge_fade_frames.unwrap_or(0).min(limit)
                    } else {
                        0
                    };
                    Self {
                        clip: c.clone(),
                        source_id: c.asset_id.to_uppercase(),
                        attack: edge,
                        release: edge,
                        boundary_attack: if protected {
                            c.cut_fade_in_frames.unwrap_or(0).min(cut_limit)
                        } else {
                            0
                        },
                        boundary_release: if protected {
                            c.cut_fade_out_frames.unwrap_or(0).min(cut_limit)
                        } else {
                            0
                        },
                        cross_in: None,
                        cross_out: None,
                    }
                }
            })
            .collect();
        for i in 0..sorted.len().saturating_sub(1) {
            let c = &sorted[i];
            let n = &sorted[i + 1];
            let cyclic = c
                .extra
                .get("drumPattern")
                .filter(|x| !x.is_null())
                .is_some()
                && c.extra.get("drumPattern") == n.extra.get("drumPattern")
                && c.offset == 0
                && n.offset == 0
                && c.length == n.length;
            let contiguous = c.asset_id == n.asset_id && c.offset + c.length == n.offset;
            if c.start + c.length == n.start && (contiguous || cyclic) {
                result[i].boundary_release = 0;
                result[i + 1].boundary_attack = 0;
                if c.fade_reference_length.is_none() && c.render_envelope.is_none() {
                    result[i].release = 0
                }
                if n.fade_reference_length.is_none() && n.render_envelope.is_none() {
                    result[i + 1].attack = 0
                }
            } else if n.start > c.start
                && n.start < c.start + c.length
                && n.start + n.length >= c.start + c.length
                && c.edge_protection_disabled != Some(true)
                && n.edge_protection_disabled != Some(true)
                && c.edge_fade_frames.unwrap_or(0) > 0
                && n.edge_fade_frames.unwrap_or(0) > 0
            {
                let overlap = [n.start as i64, (c.start + c.length) as i64];
                if c.render_envelope.is_none() {
                    result[i].cross_out = Some(overlap);
                    result[i].release = 0;
                    result[i].boundary_release = 0
                }
                if n.render_envelope.is_none() {
                    result[i + 1].cross_in = Some(overlap);
                    result[i + 1].attack = 0;
                    result[i + 1].boundary_attack = 0
                }
            }
        }
        result
    }
    pub fn gain(&self, local: u64) -> f32 {
        let c = &self.clip;
        if local >= c.length {
            return 0.;
        }
        let position = local + c.fade_reference_offset.unwrap_or(0);
        let length = c.fade_reference_length.unwrap_or(c.length).max(1);
        let mut gain = ramp(position, c.fade_in_frames.unwrap_or(0)).min(ramp(
            length.saturating_sub(1 + position),
            c.fade_out_frames.unwrap_or(0),
        ));
        gain = gain
            .min(ramp(position, self.attack))
            .min(ramp(length.saturating_sub(1 + position), self.release));
        if let Some(e) = &c.render_envelope {
            let pos = e.reference_offset + local;
            gain = gain.min(ramp(pos, e.boundary_attack_frames)).min(ramp(
                e.reference_length.saturating_sub(1 + pos),
                e.boundary_release_frames,
            ));
            for b in &e.additional_boundaries {
                gain = gain
                    .min(ramp(pos.saturating_sub(b.start), b.attack))
                    .min(ramp(
                        (b.start + b.length).saturating_sub(1 + pos),
                        b.release,
                    ));
            }
        }
        gain = gain
            .min(ramp(local, self.boundary_attack))
            .min(ramp(c.length - 1 - local, self.boundary_release));
        let frame = (c.start + local) as i64;
        if let Some([lo, hi]) = self.cross_in {
            gain = gain.min(if hi - lo == 1 {
                if frame >= lo {
                    1.
                } else {
                    0.
                }
            } else {
                ((frame - lo) as f32 / (hi - lo - 1) as f32).clamp(0., 1.)
            })
        }
        if let Some([lo, hi]) = self.cross_out {
            gain = gain.min(((hi - 1 - frame) as f32 / (hi - lo - 1).max(1) as f32).clamp(0., 1.))
        }
        gain
    }
}
