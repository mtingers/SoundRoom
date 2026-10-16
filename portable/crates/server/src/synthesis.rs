//! Worker-only native instrument rendering; no client audio or audio-device access.
use crate::Error;
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};
pub(crate) struct Task {
    pub document: Value,
    pub designer: bool,
    pub track: Option<String>,
    pub clip: Option<String>,
    pub frame: u64,
    pub export: bool,
    pub path: PathBuf,
    pub name: String,
}
pub(crate) struct Rendered {
    pub document: Value,
    pub designer: bool,
    pub track: Option<String>,
    pub clip: Option<String>,
    pub frame: u64,
    pub export: bool,
    pub path: PathBuf,
    pub name: String,
}
impl Task {
    pub fn render(self, cancel: &AtomicBool) -> Result<Rendered, Error> {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::Protocol("Synthesis cancelled".into()));
        }
        let mut document = self.document.clone();
        crate::service::normalize_seeds(&mut document, false);
        let audio = if self.designer {
            let pattern: soundroom_engine::designer::Pattern = serde_json::from_value(document)?;
            pattern
                .render(48000., cancel)
                .map_err(|e| Error::Protocol(e.to_string()))?
        } else {
            let pattern: soundroom_engine::drums::Pattern = serde_json::from_value(document)?;
            let audio = pattern
                .render(1, self.export, cancel)
                .map_err(|e| Error::Protocol(e.to_string()))?;
            Vec::from(audio)
        };
        let count = audio.first().map_or(0, Vec::len);
        if count == 0
            || audio.is_empty()
            || audio.len() > 2
            || audio.iter().any(|c| c.len() != count)
        {
            return Err(Error::Protocol("Instrument produced invalid audio".into()));
        }
        let mut writer = soundroom_engine::wave::WaveWriter::create(&self.path, count as u64)
            .map_err(|e| Error::Protocol(e.to_string()))?;
        let result = (|| -> Result<(), Error> {
            for (i, left) in audio[0].chunks(8192).enumerate() {
                if cancel.load(Ordering::Acquire) {
                    return Err(Error::Protocol("Synthesis cancelled".into()));
                }
                writer
                    .write(
                        left,
                        if audio.len() == 1 {
                            left
                        } else {
                            &audio[1][i * 8192..i * 8192 + left.len()]
                        },
                    )
                    .map_err(|e| Error::Protocol(e.to_string()))?;
            }
            writer
                .finish()
                .map_err(|e| Error::Protocol(e.to_string()))?;
            Ok(())
        })();
        if let Err(error) = result {
            let _ = std::fs::remove_file(&self.path);
            return Err(error);
        }
        Ok(Rendered {
            document: self.document,
            designer: self.designer,
            track: self.track,
            clip: self.clip,
            frame: self.frame,
            export: self.export,
            path: self.path,
            name: self.name,
        })
    }
}

/// Keep the timeline window of a sliced drum source when replacing its PCM.
/// Whole loops (including older versions with a render tail) follow pattern length.
pub(crate) fn drum_replacement(
    previous: &soundroom_engine::model::Clip,
    rendered: soundroom_engine::model::Clip,
) -> soundroom_engine::model::Clip {
    let prior = previous
        .extra
        .get("drumPattern")
        .filter(|p| !p.is_null())
        .or_else(|| {
            previous
                .previous_states
                .iter()
                .filter(|s| {
                    s.asset_id == previous.asset_id
                        && previous.offset >= s.offset
                        && previous.offset - s.offset <= s.length
                        && previous.length <= s.length - (previous.offset - s.offset)
                        && s.extra.get("drumPattern").is_some_and(|p| !p.is_null())
                })
                .max_by(|a, b| {
                    let time = |s: &soundroom_engine::model::ClipHistory| {
                        s.extra
                            .get("createdAt")
                            .and_then(Value::as_f64)
                            .unwrap_or(0.)
                    };
                    time(a).total_cmp(&time(b))
                })
                .and_then(|s| s.extra.get("drumPattern"))
        });
    let whole = prior
        .and_then(|p| serde_json::from_value::<soundroom_engine::drums::Pattern>(p.clone()).ok())
        .map_or(true, |p| {
            previous.offset == 0
                && previous.length
                    >= (p.duration() * 48000.).round().max(1.) as u64
                        * p.variation_cycles.unwrap_or(1).clamp(1, 8) as u64
        });
    let mut result = previous.clone();
    if whole {
        result.offset = 0;
        result.length = rendered.length;
    } else {
        result.offset = previous.offset.min(rendered.length.saturating_sub(1));
        result.length = previous.length.min(rendered.length - result.offset);
    }
    if result.offset != previous.offset || result.length != previous.length {
        result.fade_reference_offset = None;
        result.fade_reference_length = None;
        result.render_envelope = None;
    }
    result.asset_id = rendered.asset_id;
    result.previous_states = rendered.previous_states;
    result.extra.remove("soundDesign");
    result
        .extra
        .insert("drumPattern".into(), rendered.extra["drumPattern"].clone());
    result
}

/// Preserve the timeline window and mix/fade controls of a sliced instrument.
pub(crate) fn designer_replacement(
    previous: &soundroom_engine::model::Clip,
    rendered: soundroom_engine::model::Clip,
) -> soundroom_engine::model::Clip {
    let prior = previous
        .extra
        .get("soundDesign")
        .filter(|p| !p.is_null())
        .or_else(|| {
            previous
                .previous_states
                .iter()
                .filter(|s| {
                    s.asset_id == previous.asset_id
                        && previous.offset >= s.offset
                        && previous.offset - s.offset <= s.length
                        && previous.length <= s.length - (previous.offset - s.offset)
                        && s.extra.get("soundDesign").is_some_and(|p| !p.is_null())
                })
                .max_by(|a, b| {
                    let time = |s: &soundroom_engine::model::ClipHistory| {
                        s.extra
                            .get("createdAt")
                            .and_then(Value::as_f64)
                            .unwrap_or(0.)
                    };
                    time(a).total_cmp(&time(b))
                })
                .and_then(|s| s.extra.get("soundDesign"))
        });
    let whole = prior.map_or(true, |p| {
        let tempo = p["bpm"]
            .as_f64()
            .filter(|v| v.is_finite())
            .unwrap_or(120.)
            .clamp(30., 300.);
        let beats = p["beats"]
            .as_f64()
            .filter(|v| v.is_finite())
            .unwrap_or(4.)
            .clamp(0.25, 64.);
        previous.offset == 0
            && previous.length >= (beats * 60. / tempo * 48000.).round().max(1.) as u64
    });
    let mut result = previous.clone();
    if whole {
        result.offset = 0;
        result.length = rendered.length;
    } else {
        result.offset = previous.offset.min(rendered.length.saturating_sub(1));
        result.length = previous.length.min(rendered.length - result.offset);
    }
    if result.offset != previous.offset || result.length != previous.length {
        result.fade_reference_offset = None;
        result.fade_reference_length = None;
        result.render_envelope = None;
    }
    result.asset_id = rendered.asset_id;
    result.previous_states = rendered.previous_states;
    result.extra.remove("drumPattern");
    result
        .extra
        .insert("soundDesign".into(), rendered.extra["soundDesign"].clone());
    result
}
