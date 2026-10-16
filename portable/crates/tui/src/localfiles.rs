//! Client-local interchange actor. No local paths or file contents go to the server until Insert.
use crate::midi;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use soundroom_client::{auth::new_id, models::*, validation};
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
    thread,
};
pub const MAX_PRESET: usize = 512 * 1024;
pub const MAX_EFFECT_LIBRARY: usize = 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedEffectPreset {
    pub id: String,
    pub name: String,
    pub kind: RealtimeEffectKind,
    pub parameters: std::collections::BTreeMap<String, f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_amount: Option<f32>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EffectPresetDocument {
    version: i64,
    presets: Vec<SavedEffectPreset>,
}
pub fn effect_presets(
    bytes: &[u8],
    metadata: &[RemoteEffectMetadata],
) -> Result<Vec<SavedEffectPreset>, String> {
    if bytes.len() > MAX_EFFECT_LIBRARY {
        return Err("Effect preset libraries are limited to 1 MiB.".into());
    }
    let document: EffectPresetDocument =
        serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if document.version != 1 {
        return Err("Unsupported effect library version.".into());
    }
    if document.presets.is_empty() || document.presets.len() > 128 {
        return Err("Effect libraries support 1–128 presets.".into());
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut names = std::collections::BTreeSet::new();
    for p in &document.presets {
        let kind = serde_json::to_value(&p.kind).map_err(|e| e.to_string())?;
        let meta = metadata
            .iter()
            .find(|m| Some(m.id.as_str()) == kind.as_str())
            .ok_or("Effect is not advertised by this host.")?;
        if !validation::uuid(&p.id)
            || !ids.insert(p.id.to_ascii_uppercase())
            || p.name.trim().is_empty()
            || p.name.chars().count() > 80
            || p.name.chars().any(char::is_control)
            || !names.insert((meta.id.clone(), p.name.trim().to_lowercase()))
        {
            return Err(
                "Effect presets need unique IDs and names of 1–80 characters on one line.".into(),
            );
        }
        if p.legacy_amount
            .is_some_and(|n| !n.is_finite() || !(0. ..=1.).contains(&n))
        {
            return Err("Legacy amount must be 0..1.".into());
        }
        if p.parameters.len() > 64 {
            return Err("Effect preset parameter budget exceeded.".into());
        }
        for (key, n) in &p.parameters {
            let parameter = meta
                .parameters
                .iter()
                .find(|m| &m.id == key)
                .ok_or("Unknown effect preset parameter.")?;
            if !n.is_finite()
                || *n < parameter.minimum
                || *n > parameter.maximum
                || ((parameter.integer || parameter.choices.is_some()) && n.fract() != 0.)
            {
                return Err(format!(
                    "{} must be {}..{} {}.",
                    parameter.name, parameter.minimum, parameter.maximum, parameter.unit
                ));
            }
        }
    }
    Ok(document.presets)
}
pub fn encode_effects(
    presets: &[SavedEffectPreset],
    metadata: &[RemoteEffectMetadata],
) -> Result<Vec<u8>, String> {
    let b = serde_json::to_vec_pretty(&EffectPresetDocument {
        version: 1,
        presets: presets.to_vec(),
    })
    .map_err(|e| e.to_string())?;
    effect_presets(&b, metadata)?;
    Ok(b)
}

#[derive(Clone)]
pub enum Operation {
    ReadMIDI(PathBuf),
    WriteMIDI(PathBuf, Box<SoundDesignPattern>),
    ReadSound(PathBuf),
    WriteSound(PathBuf, Box<SoundDesignPattern>),
    ReadBeats(PathBuf),
    WriteBeats(PathBuf, Box<DrumPattern>),
    ReadEffects(PathBuf, Vec<RemoteEffectMetadata>),
    WriteEffects(PathBuf, Vec<SavedEffectPreset>, Vec<RemoteEffectMetadata>),
}
#[derive(Clone)]
pub struct Request {
    pub workspace: String,
    pub id: String,
    pub epoch: u64,
    pub operation: Operation,
}
impl Request {
    pub fn bytes(&self) -> usize {
        match &self.operation {
            Operation::WriteMIDI(_, p) | Operation::WriteSound(_, p) => {
                512 + p.notes.as_ref().map_or(0, |n| n.len() * 160)
                    + p.controllers.as_ref().map_or(0, |n| n.len() * 64)
                    + p.layers.as_ref().map_or(0, |n| n.len() * 512)
            }
            Operation::WriteBeats(_, p) => 512 + p.steps.len() * 96 + p.lanes.len() * 192,
            Operation::WriteEffects(_, ps, meta) => {
                4096 + ps
                    .iter()
                    .map(|p| {
                        512 + p.name.len()
                            + p.parameters.keys().map(|k| k.len() + 64).sum::<usize>()
                    })
                    .sum::<usize>()
                    + metadata_bytes(meta)
            }
            Operation::ReadEffects(_, meta) => 4096 + metadata_bytes(meta),
            _ => 4096,
        }
    }
}
fn metadata_bytes(meta: &[RemoteEffectMetadata]) -> usize {
    meta.iter()
        .map(|m| {
            256 + m.id.len()
                + m.name.len()
                + m.parameters
                    .iter()
                    .map(|p| {
                        256 + p.id.len()
                            + p.name.len()
                            + p.unit.len()
                            + p.choices
                                .as_ref()
                                .map_or(0, |c| c.iter().map(|s| 32 + s.len()).sum::<usize>())
                    })
                    .sum::<usize>()
                + m.factory_presets
                    .iter()
                    .map(|p| {
                        256 + p.name.len() + p.values.keys().map(|k| 64 + k.len()).sum::<usize>()
                    })
                    .sum::<usize>()
        })
        .sum()
}
#[derive(Clone)]
pub enum Data {
    MIDI(Arc<midi::MIDIFile>),
    Sound(Box<SoundDesignPattern>),
    Beats(Box<DrumPattern>),
    Saved,
    Effects(Arc<Vec<SavedEffectPreset>>),
}
#[derive(Clone)]
pub struct Response {
    pub workspace: String,
    pub id: String,
    pub epoch: u64,
    pub result: Result<Data, String>,
}
pub struct Actor {
    send: SyncSender<Request>,
    receive: Receiver<Response>,
}
impl Default for Actor {
    fn default() -> Self {
        let (send, requests) = mpsc::sync_channel::<Request>(1);
        let (results, receive) = mpsc::sync_channel(1);
        thread::spawn(move || {
            while let Ok(r) = requests.recv() {
                let result = execute(&r.operation);
                if results
                    .send(Response {
                        workspace: r.workspace,
                        id: r.id,
                        epoch: r.epoch,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self { send, receive }
    }
}
impl Actor {
    pub fn submit(&self, r: Request) -> Result<(), Response> {
        self.send.try_send(r).map_err(|e| {
            let r = match e {
                mpsc::TrySendError::Full(r) | mpsc::TrySendError::Disconnected(r) => r,
            };
            Response {
                workspace: r.workspace,
                id: r.id,
                epoch: r.epoch,
                result: Err("Local file worker is busy; retry when it finishes.".into()),
            }
        })
    }
    pub fn poll(&self) -> Option<Response> {
        self.receive.try_recv().ok()
    }
}
fn read(path: &Path, max: usize) -> Result<Vec<u8>, String> {
    if path.as_os_str().len() > 4096 {
        return Err("Local path exceeds 4096 bytes.".into());
    }
    if !std::fs::metadata(path)
        .map_err(|e| e.to_string())?
        .is_file()
    {
        return Err("Choose a regular file.".into());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let f = options.open(path).map_err(|e| e.to_string())?;
    let m = f.metadata().map_err(|e| e.to_string())?;
    if !m.is_file() || m.len() == 0 || m.len() > max as u64 {
        return Err(format!("Choose a regular file between 1 and {max} bytes."));
    }
    let mut b = Vec::with_capacity(m.len() as usize);
    f.take(max as u64 + 1)
        .read_to_end(&mut b)
        .map_err(|e| e.to_string())?;
    if b.len() > max {
        return Err("File grew beyond its size limit.".into());
    }
    Ok(b)
}
/// Same-directory temporary file + no-clobber link: failures never overwrite an existing file.
fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if path.as_os_str().len() > 4096 {
        return Err("Local path exceeds 4096 bytes.".into());
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temp = parent.join(format!(
        ".soundroom-{}.partial",
        new_id().map_err(|e| e.to_string())?
    ));
    let result = (|| {
        let mut f = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)
            .map_err(|e| e.to_string())?;
        f.write_all(bytes).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        drop(f);
        std::fs::hard_link(&temp, path)
            .map_err(|e| format!("Could not install file (choose an unused filename): {e}"))
    })();
    let _ = std::fs::remove_file(temp);
    result
}
fn seeds(value: &mut Value, native: bool) -> Result<(), String> {
    match value {
        Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                if (key == "seed" || key == "organicSeed") && !value.is_null() {
                    let n = if let Some(s) = value.as_str() {
                        s.parse::<u64>().ok().filter(|n| n.to_string() == s)
                    } else {
                        value.as_u64()
                    }
                    .ok_or("Seed must be an exact unsigned 64-bit integer.")?;
                    *value = if native {
                        Value::Number(n.into())
                    } else {
                        Value::String(n.to_string())
                    };
                } else {
                    seeds(value, native)?
                }
            }
        }
        Value::Array(a) => {
            for value in a {
                seeds(value, native)?
            }
        }
        _ => {}
    }
    Ok(())
}
fn range(value: &Value, key: &str, min: f64, max: f64) -> Result<(), String> {
    if let Some(v) = value.get(key).filter(|v| !v.is_null()) {
        if v.as_f64()
            .filter(|n| n.is_finite() && *n >= min && *n <= max)
            .is_none()
        {
            return Err(format!("{key} must be {min}..{max}."));
        }
    }
    Ok(())
}
pub fn sound_preset(bytes: &[u8]) -> Result<SoundDesignPattern, String> {
    if bytes.len() > MAX_PRESET {
        return Err("Sound presets are limited to 512 KiB.".into());
    }
    let mut value: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    seeds(&mut value, false)?;
    validation::validate_model(&value, "DesignerSoundPreset").map_err(|e| e.to_string())?;
    if value.get("version").and_then(Value::as_i64) != Some(1) {
        return Err("Unsupported sound preset version.".into());
    }
    let s = value.get("sound").ok_or("Missing preset sound.")?;
    for key in [
        "tone",
        "character",
        "resonance",
        "sustain",
        "oscillatorMix",
        "subLevel",
        "noiseLevel",
        "baseGain",
    ] {
        range(s, key, 0., 1.)?
    }
    for key in ["attack", "release", "decay"] {
        range(s, key, 0., 2.)?
    }
    range(s, "fineTune", -50., 50.)?;
    range(s, "detune", -50., 50.)?;
    range(s, "pulseWidth", 0.05, 0.95)?;
    range(s, "drive", 1., 8.)?;
    range(s, "basePan", -1., 1.)?;
    if let Some(ls) = s.get("layers").and_then(Value::as_array) {
        if ls.len() > 6 {
            return Err("Sounds support at most six layers.".into());
        }
        for l in ls {
            for key in [
                "gain",
                "minimumVelocity",
                "maximumVelocity",
                "toneOverride",
                "characterOverride",
                "sustainOverride",
            ] {
                range(l, key, 0., 1.)?
            }
            for key in ["attackOverride", "releaseOverride", "decayOverride"] {
                range(l, key, 0., 2.)?
            }
            range(l, "fineTune", -50., 50.)?;
            range(l, "delay", 0., 0.5)?;
            range(l, "lowestPitch", 0., 127.)?;
            range(l, "highestPitch", 0., 127.)?;
            range(l, "velocityFade", 0., 0.5)?;
            range(l, "pan", -1., 1.)?;
            range(l, "transpose", -24., 24.)?;
            if l.get("minimumVelocity")
                .and_then(Value::as_f64)
                .unwrap_or(0.)
                > l.get("maximumVelocity")
                    .and_then(Value::as_f64)
                    .unwrap_or(1.)
            {
                return Err("Layer velocity range is reversed.".into());
            }
        }
    }
    if let Some(o) = s.get("organic") {
        for key in ["drift", "timbre", "breath"] {
            range(o, key, 0., 1.)?
        }
    }
    let mut p: SoundDesignPattern = serde_json::from_value(s.clone()).map_err(|e| e.to_string())?;
    p.notes = Some(vec![]);
    p.controllers = Some(vec![]);
    p.bpm = Some(120.);
    p.beats = Some(4.);
    Ok(p)
}
pub fn encode_sound(pattern: &SoundDesignPattern) -> Result<Vec<u8>, String> {
    let mut p = pattern.clone();
    p.notes = Some(vec![]);
    p.controllers = Some(vec![]);
    p.bpm = Some(120.);
    p.beats = Some(4.);
    let mut v = serde_json::to_value(DesignerSoundPreset {
        version: 1,
        sound: p,
    })
    .map_err(|e| e.to_string())?;
    let check = serde_json::to_vec(&v).map_err(|e| e.to_string())?;
    sound_preset(&check)?;
    seeds(&mut v, true)?;
    let bytes = serde_json::to_vec_pretty(&v).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_PRESET {
        return Err("Sound preset exceeds 512 KiB.".into());
    }
    Ok(bytes)
}
pub fn beats_preset(bytes: &[u8]) -> Result<DrumPattern, String> {
    if bytes.len() > MAX_PRESET {
        return Err("Drum presets are limited to 512 KiB.".into());
    }
    let v: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    validation::validate_model(&v, "DrumPattern").map_err(|e| e.to_string())?;
    let p: DrumPattern = serde_json::from_value(v).map_err(|e| e.to_string())?;
    if !(30. ..=300.).contains(&p.bpm)
        || !(1..=8).contains(&p.bars)
        || ![2, 4, 8].contains(&p.steps_per_beat)
        || !(0. ..=0.7).contains(&p.swing)
        || !(0. ..=0.025).contains(&p.humanize)
        || !(0. ..=1.).contains(&p.master_gain)
        || p.steps.len() > 4096
        || p.lanes.len() > 16
        || p.variation_cycles
            .is_some_and(|n| ![1, 2, 4, 8].contains(&n))
        || p.velocity_humanize
            .is_some_and(|n| !(0. ..=0.2).contains(&n))
    {
        return Err("Drum preset timing or dynamics are outside supported ranges.".into());
    }
    let count = p.bars * 4 * p.steps_per_beat;
    let mut keys = std::collections::BTreeSet::new();
    for s in &p.steps {
        if !keys.insert((format!("{:?}", s.voice), s.index)) {
            return Err("Duplicate drum hit at the same voice and step.".into());
        }
        if s.index < 0
            || s.index >= count
            || !(0. ..=1.).contains(&s.velocity)
            || !(-1. ..=1.).contains(&s.pan)
            || !(0. ..=1.).contains(&s.probability)
        {
            return Err("Drum hit is outside the pattern or supported dynamics.".into());
        }
    }
    for (key, l) in &p.lanes {
        serde_json::from_value::<DrumVoice>(Value::String(key.clone()))
            .map_err(|_| "Unknown drum lane voice.")?;
        if !(0. ..=1.5).contains(&l.gain)
            || !(-1. ..=1.).contains(&l.pan)
            || !(-12. ..=12.).contains(&l.tuning)
            || l.damping.is_some_and(|n| !(0. ..=1.).contains(&n))
        {
            return Err("Drum lane is outside supported ranges.".into());
        }
    }
    Ok(p)
}
fn execute(op: &Operation) -> Result<Data, String> {
    match op {
        Operation::ReadMIDI(path) => Ok(Data::MIDI(Arc::new(midi::parse(&read(
            path,
            midi::MAX_FILE,
        )?)?))),
        Operation::WriteMIDI(path, p) => {
            write(path, &midi::encode(p)?)?;
            Ok(Data::Saved)
        }
        Operation::ReadSound(path) => Ok(Data::Sound(Box::new(sound_preset(&read(
            path, MAX_PRESET,
        )?)?))),
        Operation::WriteSound(path, p) => {
            write(path, &encode_sound(p)?)?;
            Ok(Data::Saved)
        }
        Operation::ReadBeats(path) => Ok(Data::Beats(Box::new(beats_preset(&read(
            path, MAX_PRESET,
        )?)?))),
        Operation::ReadEffects(path, meta) => Ok(Data::Effects(Arc::new(effect_presets(
            &read(path, MAX_EFFECT_LIBRARY)?,
            meta,
        )?))),
        Operation::WriteEffects(path, presets, meta) => {
            write(path, &encode_effects(presets, meta)?)?;
            Ok(Data::Saved)
        }
        Operation::WriteBeats(path, p) => {
            let b = serde_json::to_vec_pretty(p).map_err(|e| e.to_string())?;
            beats_preset(&b)?;
            write(path, &b)?;
            Ok(Data::Saved)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sound() -> SoundDesignPattern {
        serde_json::from_value(serde_json::json!({"notes":[{"id":new_id().unwrap(),"pitch":60,"velocity":0.8,"startBeat":0,"durationBeats":1}],"organic":{"seed":"18446744073709551615","drift":0.2},"layers":[{"organicSeed":"18446744073709551614"}],"tone":0.4})).unwrap()
    }
    #[test]
    fn sound_preset_preserves_native_uint64_and_removes_song_data() {
        let b = encode_sound(&sound()).unwrap();
        let text = String::from_utf8(b.clone()).unwrap();
        assert!(text.contains("18446744073709551615"));
        assert!(!text.contains("\"18446744073709551615\""));
        let p = sound_preset(&b).unwrap();
        assert_eq!(
            p.organic.unwrap().seed.as_deref(),
            Some("18446744073709551615")
        );
        assert_eq!(
            p.layers.unwrap()[0].organic_seed.as_deref(),
            Some("18446744073709551614")
        );
        assert!(p.notes.unwrap().is_empty());
        assert_eq!(p.bpm, Some(120.));
    }
    #[test]
    fn malformed_presets_reject_version_unknown_fields_seed_overflow_and_controls() {
        for text in [
            r#"{"version":2,"sound":{}}"#,
            r#"{"version":1,"sound":{"tone":2}}"#,
            r#"{"version":1,"sound":{"paidPlugin":true}}"#,
            r#"{"version":1,"sound":{"organic":{"seed":18446744073709551616}}}"#,
            r#"{"version":1,"sound":{"layers":[{"minimumVelocity":0.9,"maximumVelocity":0.1}]}}"#,
        ] {
            assert!(sound_preset(text.as_bytes()).is_err(), "{text}");
        }
        assert!(sound_preset(&vec![b' '; MAX_PRESET + 1]).is_err());
    }
    #[test]
    fn drums_reject_hidden_duplicate_hits_and_preserve_articulations() {
        let mut v = serde_json::json!({"name":"Custom","kit":"acoustic","bpm":120,"bars":1,"stepsPerBeat":4,"swing":0,"humanize":0.01,"masterGain":0.8,"variationCycles":4,"steps":[{"voice":"snare","index":4,"velocity":0.6,"pan":0,"probability":0.8,"articulation":"rimshot"}],"lanes":{}});
        let p = beats_preset(&serde_json::to_vec(&v).unwrap()).unwrap();
        assert_eq!(p.steps[0].articulation, Some(DrumArticulation::Rimshot));
        v["steps"][0]["index"] = 32.into();
        assert!(beats_preset(&serde_json::to_vec(&v).unwrap()).is_err());
        v["steps"][0]["index"] = 4.into();
        let hit = v["steps"][0].clone();
        v["steps"].as_array_mut().unwrap().push(hit);
        assert!(beats_preset(&serde_json::to_vec(&v).unwrap()).is_err());
        v["steps"].as_array_mut().unwrap().pop();
        v["variationCycles"] = 3.into();
        assert!(beats_preset(&serde_json::to_vec(&v).unwrap()).is_err());
    }
    #[test]
    fn local_saves_never_overwrite_and_cleanup_temporary_files() {
        let parent =
            std::env::temp_dir().join(format!("soundroom-local-test-{}", new_id().unwrap()));
        std::fs::create_dir(&parent).unwrap();
        let path = parent.join("preset.json");
        write(&path, b"original").unwrap();
        assert!(write(&path, b"new").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 1);
        assert!(read(&parent, MAX_PRESET).is_err());
        assert!(read(&path, 2).is_err());
        std::fs::remove_dir_all(parent).unwrap();
    }
    #[test]
    fn actor_correlates_bounded_background_jobs_without_host_commands() {
        let a = Actor::default();
        let r = Request {
            workspace: "workspace".into(),
            id: "job".into(),
            epoch: 3,
            operation: Operation::ReadMIDI(PathBuf::from("/no/soundroom/midi/here.mid")),
        };
        assert!(a.submit(r).is_ok());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if let Some(r) = a.poll() {
                assert_eq!(r.workspace, "workspace");
                assert_eq!(r.id, "job");
                assert_eq!(r.epoch, 3);
                assert!(r.result.is_err());
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    fn gain_metadata() -> RemoteEffectMetadata {
        serde_json::from_value(serde_json::json!({"id":"gain","name":"Gain","parameters":[{"id":"gain","name":"Gain","minimum":-24,"maximum":24,"defaultValue":0,"unit":"dB","logarithmic":false,"integer":false,"automationUnit":"normalized"}],"factoryPresets":[],"modes":["realtime"]})).unwrap()
    }
    fn gain_preset() -> SavedEffectPreset {
        SavedEffectPreset {
            id: new_id().unwrap(),
            name: "Quiet".into(),
            kind: RealtimeEffectKind::Gain,
            parameters: std::collections::BTreeMap::from([("gain".into(), -6.)]),
            legacy_amount: Some(0.375),
        }
    }
    #[test]
    fn effect_libraries_match_native_document_and_keep_legacy_processor_amount() {
        let p = gain_preset();
        let bytes = encode_effects(std::slice::from_ref(&p), &[gain_metadata()]).unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(v["presets"][0]["legacyAmount"], 0.375);
        assert!(v.get("values").is_none());
        assert_eq!(effect_presets(&bytes, &[gain_metadata()]).unwrap(), vec![p]);
    }
    #[test]
    fn effect_libraries_reject_unknown_fields_duplicate_ids_names_and_domain_values() {
        let meta = gain_metadata();
        let original = serde_json::to_value(EffectPresetDocument {
            version: 1,
            presets: vec![gain_preset()],
        })
        .unwrap();
        for field in ["gain", "unknown"] {
            let mut v = original.clone();
            v["presets"][0]["parameters"][field] = 25.into();
            assert!(effect_presets(
                &serde_json::to_vec(&v).unwrap(),
                std::slice::from_ref(&meta)
            )
            .is_err());
        }
        let mut v = original.clone();
        v["presets"][0]["legacyAmount"] = 2.into();
        assert!(effect_presets(
            &serde_json::to_vec(&v).unwrap(),
            std::slice::from_ref(&meta)
        )
        .is_err());
        let mut v = original.clone();
        let p = v["presets"][0].clone();
        v["presets"].as_array_mut().unwrap().push(p);
        assert!(effect_presets(
            &serde_json::to_vec(&v).unwrap(),
            std::slice::from_ref(&meta)
        )
        .is_err());
        let mut v = original.clone();
        v["presets"][0]["automation"] = serde_json::json!([]);
        assert!(effect_presets(&serde_json::to_vec(&v).unwrap(), &[meta]).is_err());
        assert!(effect_presets(&serde_json::to_vec(&original).unwrap(), &[]).is_err());
        assert!(effect_presets(&vec![0; MAX_EFFECT_LIBRARY + 1], &[]).is_err());
    }
}
