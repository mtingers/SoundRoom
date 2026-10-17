//! Kit-designer kits. A design is a name plus a `measured_drums::Shape` per
//! piece. While designing, hits are generated from the measured model; when
//! a design is saved it is exported once into a small stereo sample library
//! (`<kits>/custom/<id>/`: `design.json`, `hits.jsonl`, 16-bit WAVs) that
//! plays as cheaply as the recorded kits.
use crate::{
    drums::{Articulation, Kit, Voice},
    measured_drums::{self, Shape},
    sampled_drums, Error, Result,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex,
    },
};

const RATE: u32 = 48_000;
/// Exported dynamics and alternate strikes per articulation.
const VELOCITIES: [f64; 5] = [0.15, 0.35, 0.55, 0.75, 0.95];
const ROBINS: i32 = 3;
/// Stored files peak here; the hit's `gain` restores its level.
const STORED_PEAK: f32 = 0.89;
pub const MAX_KITS: usize = 64;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KitDesign {
    pub id: String,
    pub name: String,
    /// Shape per piece, keyed by `Voice::name()`; absent pieces are as measured.
    #[serde(default)]
    pub shapes: BTreeMap<String, Shape>,
}
impl KitDesign {
    pub fn validate(&self) -> Result<()> {
        if !sampled_drums::valid_id(&self.id) {
            return Err(Error::Invalid("Kit id must be a UUID".into()));
        }
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
            return Err(Error::Invalid("Kit name must be 1–80 characters".into()));
        }
        if self
            .shapes
            .keys()
            .any(|k| !Voice::ALL.iter().any(|v| v.name() == k))
        {
            return Err(Error::Invalid("Unknown drum piece in kit design".into()));
        }
        Ok(())
    }
    fn shape(&self, voice: Voice) -> Option<Shape> {
        self.shapes.get(voice.name()).map(|s| s.sanitized())
    }
}

/// The pieces and articulations an exported kit contains (rimshots are
/// layered from center + side stick at playback; clap stays modeled).
fn contents() -> Vec<(Voice, Articulation)> {
    let mut list = vec![];
    for voice in Voice::ALL {
        match voice {
            Voice::Clap => {}
            Voice::Snare => list.extend([
                (voice, Articulation::Center),
                (voice, Articulation::SideStick),
                (voice, Articulation::Brush),
            ]),
            Voice::Ride => {
                list.extend([(voice, Articulation::Center), (voice, Articulation::Bell)])
            }
            _ => list.push((voice, Articulation::Center)),
        }
    }
    list
}
fn articulation_name(a: Articulation) -> &'static str {
    match a {
        Articulation::Center => "center",
        Articulation::Rimshot => "rimshot",
        Articulation::SideStick => "sideStick",
        Articulation::Brush => "brush",
        Articulation::Bell => "bell",
    }
}
fn write_wav16(path: &Path, left: &[f32], right: &[f32], scale: f32) -> std::io::Result<()> {
    let frames = left.len().min(right.len());
    let data = frames * 4;
    let mut out = Vec::with_capacity(44 + data);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data as u32).to_le_bytes());
    for i in 0..frames {
        for v in [left[i], right[i]] {
            let q = ((v * scale).clamp(-1., 1.) * 32767.).round() as i16;
            out.extend_from_slice(&q.to_le_bytes());
        }
    }
    std::fs::write(path, out)
}

/// Renders `design` into `<custom root>/<id>/`, replacing an earlier export.
/// Returns the number of hits written.
pub fn export(design: &KitDesign, cancelled: &AtomicBool) -> Result<usize> {
    design.validate()?;
    let root = sampled_drums::custom_root()
        .ok_or_else(|| Error::Invalid("Kit storage is unavailable".into()))?;
    std::fs::create_dir_all(&root)?;
    let target = root.join(&design.id);
    if !target.exists() && list().len() >= MAX_KITS {
        return Err(Error::Invalid(format!("At most {MAX_KITS} saved kits")));
    }
    let staging = root.join(format!(".{}.export", design.id));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let jobs: Vec<(Voice, Articulation, f64, i32)> = contents()
        .into_iter()
        .flat_map(|(v, a)| {
            VELOCITIES
                .into_iter()
                .flat_map(move |vel| (0..ROBINS).map(move |r| (v, a, vel, r)))
        })
        .collect();
    let next = AtomicUsize::new(0);
    let lines = Mutex::new(Vec::new());
    let failure = Mutex::new(None);
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 8);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(&(voice, articulation, velocity, robin)) = jobs.get(index) else { break };
                if cancelled.load(Ordering::Relaxed) {
                    break;
                }
                let result = (|| -> Result<Option<serde_json::Value>> {
                    let Some([mut left, mut right]) = measured_drums::render(
                        Kit::StudioModeled, voice, velocity, robin, articulation, 0.5, design.shape(voice), cancelled,
                    )?
                    else {
                        return Ok(None);
                    };
                    let peak = left.iter().chain(&right).fold(0f32, |m, v| m.max(v.abs()));
                    if peak <= 0. {
                        return Ok(None);
                    }
                    // Trim below -66 dB of the peak, with a 10 ms fade.
                    let floor = peak * 5e-4;
                    let end = left.iter().zip(&right).rposition(|(l, r)| l.abs().max(r.abs()) > floor).map_or(1, |i| i + 1);
                    left.truncate(end);
                    right.truncate(end);
                    let fade = 480.min(end);
                    for k in 0..fade {
                        let w = (0.5 + 0.5 * (std::f64::consts::PI * k as f64 / fade as f64).cos()) as f32;
                        left[end - fade + k] *= w;
                        right[end - fade + k] *= w;
                    }
                    let file = format!("{}/{}/{:02}-{}.wav", voice.name(), articulation_name(articulation), (velocity * 100.).round() as i32, robin);
                    let path = staging.join(&file);
                    std::fs::create_dir_all(path.parent().unwrap())?;
                    write_wav16(&path, &left, &right, STORED_PEAK / peak)?;
                    Ok(Some(serde_json::json!({
                        "voice": voice.name(), "articulation": articulation_name(articulation), "file": file,
                        "power": (peak as f64).powi(2), "gain": peak / STORED_PEAK, "velocity": velocity,
                    })))
                })();
                match result {
                    Ok(Some(line)) => lines.lock().unwrap().push(line),
                    Ok(None) => {}
                    Err(error) => {
                        failure.lock().unwrap().get_or_insert(error);
                        break;
                    }
                }
            });
        }
    });
    if let Some(error) = failure.into_inner().unwrap() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }
    if cancelled.load(Ordering::Relaxed) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(Error::Invalid("Kit export cancelled".into()));
    }
    let mut lines = lines.into_inner().unwrap();
    lines.sort_by(|a, b| a["file"].as_str().cmp(&b["file"].as_str()));
    let count = lines.len();
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(staging.join("hits.jsonl"), text)?;
    std::fs::write(
        staging.join("design.json"),
        serde_json::to_vec_pretty(design)?,
    )?;
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&staging, &target)?;
    sampled_drums::forget_custom(&design.id);
    Ok(count)
}
/// Saved kit designs, by name.
pub fn list() -> Vec<KitDesign> {
    let Some(root) = sampled_drums::custom_root() else {
        return vec![];
    };
    let Ok(entries) = std::fs::read_dir(root) else {
        return vec![];
    };
    let mut kits: Vec<KitDesign> = entries
        .flatten()
        .filter(|e| sampled_drums::valid_id(&e.file_name().to_string_lossy()))
        .filter_map(|e| std::fs::read(e.path().join("design.json")).ok())
        .filter_map(|b| serde_json::from_slice::<KitDesign>(&b).ok())
        .filter(|k| k.validate().is_ok())
        .collect();
    kits.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.id.cmp(&b.id))
    });
    kits
}
/// Removes a saved kit and its exported audio.
pub fn delete(id: &str) -> Result<()> {
    if !sampled_drums::valid_id(id) {
        return Err(Error::Invalid("Kit id must be a UUID".into()));
    }
    let root = sampled_drums::custom_root()
        .ok_or_else(|| Error::Invalid("Kit storage is unavailable".into()))?;
    let path = root.join(id);
    if !path.exists() {
        return Err(Error::Invalid("No saved kit with that id".into()));
    }
    std::fs::remove_dir_all(path)?;
    sampled_drums::forget_custom(id);
    Ok(())
}
