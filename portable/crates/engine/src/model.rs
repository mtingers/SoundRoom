use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
pub const SAMPLE_RATE: u32 = 48_000;
/// Conservative public bound: all frame/revision values must remain exact in JavaScript.
pub const MAX_FRAME: u64 = (1_u64 << 53) - 1;
pub const MAX_TRACKS: usize = 4096;
pub const MAX_CLIPS: usize = 1_000_000;
pub const MASTER_ID: &str = "E3E58100-5125-4E6F-B985-10AA11000001";
pub type Extra = BTreeMap<String, Value>;
pub fn is_id(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
fn valid_id(s: &str) -> Result<()> {
    if is_id(s) {
        Ok(())
    } else {
        Err(Error::Invalid(format!("Invalid UUID: {s}")))
    }
}
fn one() -> f32 {
    1.0
}
fn yes() -> bool {
    true
}
fn version() -> u32 {
    1
}
fn name() -> String {
    "Untitled session".into()
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Timing {
    pub tempo: f64,
    pub beats_per_bar: u32,
    pub beat_unit: u32,
}
impl Default for Timing {
    fn default() -> Self {
        Self {
            tempo: 120.,
            beats_per_bar: 4,
            beat_unit: 4,
        }
    }
}
impl Timing {
    pub fn validate(&self) -> Result<()> {
        if self.tempo.is_finite()
            && (20. ..=300.).contains(&self.tempo)
            && (1..=16).contains(&self.beats_per_bar)
            && [2, 4, 8, 16].contains(&self.beat_unit)
        {
            Ok(())
        } else {
            Err(Error::Invalid("Invalid tempo or meter".into()))
        }
    }
    pub fn frames_per_beat(&self) -> f64 {
        SAMPLE_RATE as f64 * 60. / self.tempo * 4. / self.beat_unit as f64
    }
    pub fn snap(&self, frame: u64, beats: f64) -> u64 {
        let step = self.frames_per_beat() * beats;
        ((frame as f64 / step).round() * step)
            .round()
            .clamp(0., MAX_FRAME as f64) as u64
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    #[serde(default = "version")]
    pub version: u32,
    #[serde(default = "name")]
    pub name: String,
    #[serde(default)]
    pub timing: Timing,
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(default = "master")]
    pub master_track: Track,
    #[serde(flatten)]
    pub extra: Extra,
}
fn master() -> Track {
    Track {
        id: MASTER_ID.into(),
        name: "Master".into(),
        ..Track::default()
    }
}
impl Default for Session {
    fn default() -> Self {
        Self {
            version: 9,
            name: name(),
            timing: Timing::default(),
            tracks: vec![],
            master_track: master(),
            extra: Extra::new(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub color: i32,
    #[serde(default)]
    pub clips: Vec<Clip>,
    #[serde(default = "one")]
    pub gain: f32,
    #[serde(default)]
    pub pan: f32,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub solo: bool,
    #[serde(default)]
    pub armed: bool,
    #[serde(default)]
    pub mix_automation: BTreeMap<String, Vec<AutomationPoint>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_automation_scale: Option<String>,
    #[serde(default)]
    pub realtime_effects: Vec<Effect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<crate::routing::TrackRouting>,
    #[serde(default)]
    pub recording_takes: Vec<Take>,
    #[serde(flatten)]
    pub extra: Extra,
}
impl Default for Track {
    fn default() -> Self {
        Self {
            id: MASTER_ID.into(),
            name: "Audio".into(),
            color: 0,
            clips: vec![],
            gain: 1.,
            pan: 0.,
            muted: false,
            solo: false,
            armed: false,
            mix_automation: BTreeMap::new(),
            volume_automation_scale: None,
            realtime_effects: vec![],
            routing: None,
            recording_takes: vec![],
            extra: Extra::new(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Take {
    #[serde(default)]
    pub clips: Vec<Clip>,
    #[serde(flatten)]
    pub extra: Extra,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Clip {
    pub id: String,
    #[serde(rename = "assetID")]
    pub asset_id: String,
    pub name: String,
    pub start: u64,
    pub offset: u64,
    pub length: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge_fade_frames: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge_protection_disabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut_fade_in_frames: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut_fade_out_frames: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fade_in_frames: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fade_out_frames: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fade_reference_offset: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fade_reference_length: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_envelope: Option<FrozenEnvelope>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub previous_states: Vec<ClipHistory>,
    #[serde(flatten)]
    pub extra: Extra,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipHistory {
    #[serde(rename = "assetID")]
    pub asset_id: String,
    pub offset: u64,
    pub length: u64,
    #[serde(flatten)]
    pub extra: Extra,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrozenEnvelope {
    pub reference_length: u64,
    #[serde(default)]
    pub reference_offset: u64,
    pub attack_frames: u64,
    pub release_frames: u64,
    pub boundary_attack_frames: u64,
    pub boundary_release_frames: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crossfade_in: Option<[u64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crossfade_out: Option<[u64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut_reference_offset: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut_reference_length: Option<u64>,
    #[serde(default)]
    pub additional_boundaries: Vec<Boundary>,
    #[serde(flatten)]
    pub extra: Extra,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Boundary {
    pub start: u64,
    pub length: u64,
    pub attack: u64,
    pub release: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationPoint {
    pub frame: u64,
    pub value: f32,
    #[serde(default = "linear")]
    pub curve: String,
    #[serde(default)]
    pub curve_start: f32,
    #[serde(default = "one")]
    pub curve_end: f32,
    #[serde(flatten)]
    pub extra: Extra,
}
fn linear() -> String {
    "linear".into()
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Effect {
    pub id: String,
    pub kind: String,
    #[serde(default = "half")]
    pub amount: f32,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub automation: Vec<AutomationPoint>,
    #[serde(default)]
    pub parameters: BTreeMap<String, f32>,
    #[serde(default)]
    pub parameter_automation: BTreeMap<String, Vec<AutomationPoint>>,
    #[serde(flatten)]
    pub extra: Extra,
}
fn half() -> f32 {
    0.5
}
impl Session {
    pub fn required_version(&self) -> u32 {
        if self
            .tracks
            .iter()
            .chain(std::iter::once(&self.master_track))
            .any(|t| t.realtime_effects.iter().any(|e| e.kind == "guitarRig"))
        {
            return self.version.max(11);
        }
        if self
            .tracks
            .iter()
            .chain(std::iter::once(&self.master_track))
            .any(|t| {
                t.routing.is_some()
                    || t.realtime_effects.iter().any(|e| {
                        ["gate", "utility"].contains(&e.kind.as_str())
                            || e.kind == "equalizer"
                                && e.parameters
                                    .keys()
                                    .chain(e.parameter_automation.keys())
                                    .any(|k| k.starts_with("band") || k.starts_with("dynamic"))
                    })
            })
        {
            self.version.max(10)
        } else {
            self.version
        }
    }
    pub fn validate(&self) -> Result<()> {
        if !(1..=11).contains(&self.version) {
            return Err(Error::Unsupported(format!(
                "Unsupported project version {} (supported: 1…11)",
                self.version
            )));
        }
        self.timing.validate()?;
        crate::routing::Plan::new(self)?;
        if self.tracks.len() > MAX_TRACKS {
            return Err(Error::Invalid("Too many tracks".into()));
        }
        let mut ids = BTreeSet::new();
        let mut count = 0;
        for t in self
            .tracks
            .iter()
            .chain(std::iter::once(&self.master_track))
        {
            valid_id(&t.id)?;
            if !ids.insert(t.id.to_uppercase()) {
                return Err(Error::Invalid("Duplicate track ID".into()));
            }
            if !t.gain.is_finite()
                || !(0. ..=4.).contains(&t.gain)
                || !t.pan.is_finite()
                || !(-1. ..=1.).contains(&t.pan)
            {
                return Err(Error::Invalid("Invalid track gain/pan".into()));
            }
            if t.realtime_effects.len() > 256 {
                return Err(Error::Invalid("Too many inserts".into()));
            }
            if t.volume_automation_scale
                .as_deref()
                .map(|v| !["linear", "decibels"].contains(&v))
                .unwrap_or(false)
            {
                return Err(Error::Invalid("Unknown volume automation scale".into()));
            }
            for lane in t.mix_automation.values() {
                validate_points(lane)?
            }
            for e in &t.realtime_effects {
                valid_id(&e.id)?;
                if ![
                    "gain",
                    "lowPass",
                    "highPass",
                    "echo",
                    "warmth",
                    "compressor",
                    "guitarRig",
                    "gate",
                    "utility",
                    "room",
                    "equalizer",
                    "chorus",
                    "dither",
                    "pitchCorrection",
                    "limiter",
                    "resonance",
                    "soundPutty",
                    "vocalEnhancer",
                    "sourceIsolator",
                ]
                .contains(&e.kind.as_str())
                {
                    return Err(Error::Unsupported(format!(
                        "Unknown project effect {}",
                        e.kind
                    )));
                }
                if !e.amount.is_finite()
                    || !(0. ..=1.).contains(&e.amount)
                    || e.parameters.values().any(|v| !v.is_finite())
                {
                    return Err(Error::Invalid("Non-finite effect parameter".into()));
                }
                crate::pitch::validate(e)?;
                validate_points(&e.automation)?;
                for lane in e.parameter_automation.values() {
                    validate_points(lane)?
                }
            }
            let mut clips = BTreeSet::new();
            for clip in &t.clips {
                if !clips.insert(clip.id.to_uppercase()) {
                    return Err(Error::Invalid("Duplicate active clip ID".into()));
                }
            }
            for c in t
                .clips
                .iter()
                .chain(t.recording_takes.iter().flat_map(|v| &v.clips))
            {
                count += 1;
                valid_id(&c.id)?;
                valid_id(&c.asset_id)?;
                if c.length == 0
                    || c.start > MAX_FRAME
                    || c.length > MAX_FRAME - c.start
                    || c.offset > MAX_FRAME
                    || c.length > MAX_FRAME - c.offset
                {
                    return Err(Error::Invalid("Invalid clip range".into()));
                }
                if c.previous_states.len() > 5 {
                    return Err(Error::Invalid("Clip history exceeds five states".into()));
                }
                for h in &c.previous_states {
                    valid_id(&h.asset_id)?;
                    if h.length == 0 || h.offset > MAX_FRAME || h.length > MAX_FRAME - h.offset {
                        return Err(Error::Invalid("Invalid history range".into()));
                    }
                }
                for n in [
                    c.edge_fade_frames,
                    c.cut_fade_in_frames,
                    c.cut_fade_out_frames,
                    c.fade_in_frames,
                    c.fade_out_frames,
                    c.fade_reference_offset,
                    c.fade_reference_length,
                ]
                .into_iter()
                .flatten()
                {
                    if n > MAX_FRAME {
                        return Err(Error::Invalid("Invalid fade coordinate".into()));
                    }
                }
                if let Some(e) = &c.render_envelope {
                    if e.reference_length == 0
                        || e.reference_length > MAX_FRAME
                        || e.reference_offset > MAX_FRAME
                        || e.reference_offset > MAX_FRAME - c.length
                    {
                        return Err(Error::Invalid("Invalid frozen envelope".into()));
                    }
                    for n in [
                        Some(e.attack_frames),
                        Some(e.release_frames),
                        Some(e.boundary_attack_frames),
                        Some(e.boundary_release_frames),
                        e.cut_reference_offset,
                        e.cut_reference_length,
                    ]
                    .into_iter()
                    .flatten()
                    {
                        if n > MAX_FRAME {
                            return Err(Error::Invalid("Invalid frozen fade coordinate".into()));
                        }
                    }
                    if e.additional_boundaries.len() > 4096 {
                        return Err(Error::Invalid("Too many frozen boundaries".into()));
                    }
                    for b in &e.additional_boundaries {
                        if b.length == 0
                            || b.start > MAX_FRAME
                            || b.length > MAX_FRAME - b.start
                            || b.attack > MAX_FRAME
                            || b.release > MAX_FRAME
                        {
                            return Err(Error::Invalid("Invalid frozen boundary".into()));
                        }
                    }
                    for r in [e.crossfade_in, e.crossfade_out].into_iter().flatten() {
                        if r[1] < r[0] || r[1] > MAX_FRAME {
                            return Err(Error::Invalid("Invalid frozen crossfade".into()));
                        }
                    }
                }
            }
        }
        if !self.master_track.id.eq_ignore_ascii_case(MASTER_ID)
            || !self.master_track.clips.is_empty()
            || self.master_track.armed
        {
            return Err(Error::Invalid("Invalid master track".into()));
        }
        if count > MAX_CLIPS {
            return Err(Error::Invalid("Too many clips".into()));
        }
        Ok(())
    }
    pub fn end(&self) -> u64 {
        self.tracks
            .iter()
            .flat_map(|t| &t.clips)
            .map(|c| c.start + c.length)
            .max()
            .unwrap_or(0)
    }
    /// Sources heard by the current arrangement, including muted tracks so
    /// they can be unmuted live. Undo history and alternate takes remain in
    /// project custody, but do not occupy playback decoders or resident PCM.
    pub fn playback_asset_ids(&self) -> BTreeSet<String> {
        self.tracks
            .iter()
            .flat_map(|t| &t.clips)
            .map(|c| c.asset_id.to_uppercase())
            .collect()
    }
    pub fn asset_ids(&self) -> BTreeSet<String> {
        self.tracks
            .iter()
            .flat_map(|t| {
                t.clips
                    .iter()
                    .chain(t.recording_takes.iter().flat_map(|v| &v.clips))
            })
            .flat_map(|c| {
                std::iter::once(c.asset_id.to_uppercase())
                    .chain(c.previous_states.iter().map(|h| h.asset_id.to_uppercase()))
            })
            .collect()
    }
}
fn validate_points(points: &[AutomationPoint]) -> Result<()> {
    if points.len() > 1_000_000
        || points.iter().any(|p| {
            p.frame > MAX_FRAME
                || !p.value.is_finite()
                || !(0. ..=1.).contains(&p.value)
                || !p.curve_start.is_finite()
                || !p.curve_end.is_finite()
                || !(0. ..=1.).contains(&p.curve_start)
                || !(p.curve_start..=1.).contains(&p.curve_end)
                || !["linear", "smooth", "easeIn", "easeOut", "hold"].contains(&p.curve.as_str())
        })
    {
        Err(Error::Invalid("Invalid automation lane".into()))
    } else {
        Ok(())
    }
}
