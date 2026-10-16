//! Renders an arrangement (a session whose clips point at instrument or drum
//! patterns) into a Soundroom project and a mixed WAV, with the same engine
//! the app uses: every clip keeps its editable pattern, and its audio is the
//! pattern rendered with the installed recorded instruments and kits.
//!
//! render_arrangement <arrangement.json> <out.soundroom> <mix.wav> [--kits <Kits root>]
//!
//! arrangement.json: {"session": <Session JSON>, "assets": [{"id", "name",
//! "pattern" | "drums"}], "tailSeconds": 4}. Clip lengths are set from the
//! rendered audio; a clip's "trim" (frames) shortens it instead.
use soundroom_engine::{
    designer, drums, project::Project, render::Renderer, sampled_drums, sampled_instruments,
    wave::{WaveReader, WaveWriter},
};
use std::{path::PathBuf, sync::atomic::AtomicBool};

fn write_wav(path: &std::path::Path, left: &[f32], right: &[f32]) -> soundroom_engine::Result<()> {
    let mut writer = WaveWriter::create(path, left.len() as u64)?;
    for (l, r) in left.chunks(8192).zip(right.chunks(8192)) {
        writer.write(l, r)?;
    }
    writer.finish()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let (plan_path, project_path, mix_path) = (
        PathBuf::from(&args[1]),
        PathBuf::from(&args[2]),
        PathBuf::from(&args[3]),
    );
    let kits = args
        .iter()
        .position(|a| a == "--kits")
        .map(|i| PathBuf::from(&args[i + 1]))
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap())
                .join("Library/Application Support/Soundroom/RustServer/Kits")
        });
    sampled_drums::set_kit_roots(vec![kits]);
    // Missing libraries would silently fall back to the modeled voices.
    for instrument in designer::Instrument::ALL {
        if instrument.recording().is_some() && !sampled_instruments::available(instrument) {
            eprintln!("warning: {instrument:?} recording not installed; it will be modeled");
        }
    }
    if !sampled_drums::available() {
        eprintln!("warning: recorded drum kit not installed; drums will be modeled");
    }
    let plan: serde_json::Value = serde_json::from_slice(&std::fs::read(&plan_path)?)?;
    let audio_dir = project_path.join("Audio");
    std::fs::create_dir_all(&audio_dir)?;
    let assets = plan["assets"].as_array().ok_or("assets missing")?;
    let cancel = AtomicBool::new(false);
    // Render the sources in parallel; each is independent.
    let next = std::sync::atomic::AtomicUsize::new(0);
    let lengths = std::sync::Mutex::new(std::collections::BTreeMap::new());
    let failures = std::sync::Mutex::new(vec![]);
    let started = std::time::Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..std::thread::available_parallelism().map_or(4, |n| n.get()) {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(job) = assets.get(index) else { break };
                let id = job["id"].as_str().unwrap().to_owned();
                let result = (|| -> Result<(Vec<f32>, Vec<f32>), Box<dyn std::error::Error>> {
                    Ok(if let Some(path) = job.get("audioPath").and_then(|p| p.as_str()) {
                        // Explicit research import: Python adapters resample first.
                        // Use the ordinary decoder and inserts, without model fallback.
                        let mut reader = WaveReader::open(std::path::Path::new(path))?;
                        if reader.info.sample_rate != 48000 || reader.info.channels != 2 || reader.info.frames > 48000 * 60 {
                            return Err("Research source must be stereo 48 kHz, at most one minute".into());
                        }
                        let mut left = vec![0.; reader.info.frames as usize];
                        let mut right = vec![0.; left.len()];
                        for start in (0..left.len()).step_by(8192) {
                            let end = (start + 8192).min(left.len());
                            reader.read_stereo(start as u64, &mut left[start..end], &mut right[start..end])?;
                        }
                        (left, right)
                    } else if let Some(drum) = job.get("drums") {
                        let pattern: drums::Pattern = serde_json::from_value(drum.clone())?;
                        let [l, r] = pattern.render(1, true, &cancel)?;
                        (l, r)
                    } else {
                        let pattern: designer::Pattern = serde_json::from_value(job["pattern"].clone())?;
                        let mut audio = pattern.render(48000., &cancel)?;
                        let left = audio.remove(0);
                        let right = audio.pop().unwrap_or_else(|| left.clone());
                        (left, right)
                    })
                })();
                match result {
                    Ok((left, right)) => {
                        let peak = left.iter().chain(&right).fold(0f32, |m, v| m.max(v.abs()));
                        if !peak.is_finite() || peak >= 1. {
                            failures.lock().unwrap().push(format!("{id}: peak {peak}"));
                        }
                        if let Err(e) = write_wav(&audio_dir.join(format!("{id}.wav")), &left, &right) {
                            failures.lock().unwrap().push(format!("{id}: {e}"));
                        }
                        lengths.lock().unwrap().insert(id, left.len() as u64);
                    }
                    Err(e) => failures.lock().unwrap().push(format!("{id} ({}): {e}", job["name"])),
                }
            });
        }
    });
    let failures = failures.into_inner().unwrap();
    if !failures.is_empty() {
        return Err(failures.join("\n").into());
    }
    let lengths = lengths.into_inner().unwrap();
    println!("Rendered {} sources in {:.1}s", lengths.len(), started.elapsed().as_secs_f64());
    let mut session = plan["session"].clone();
    let mut end = 0u64;
    for track in session["tracks"].as_array_mut().unwrap() {
        for clip in track["clips"].as_array_mut().unwrap() {
            let length = lengths[clip["assetID"].as_str().unwrap()];
            let length = clip.get("trim").and_then(|t| t.as_u64()).map_or(length, |t| t.min(length));
            clip.as_object_mut().unwrap().remove("trim");
            clip["length"] = serde_json::json!(length);
            end = end.max(clip["start"].as_u64().unwrap() + length);
        }
    }
    std::fs::write(project_path.join("session.json"), serde_json::to_vec_pretty(&session)?)?;
    let project = Project::open(&project_path)?;
    let frames = end + (plan["tailSeconds"].as_f64().unwrap_or(4.) * 48000.) as u64;
    let mut renderer = Renderer::new(&project)?;
    let (mut left, mut right) = (vec![0f32; frames as usize], vec![0f32; frames as usize]);
    for start in (0..frames as usize).step_by(8192) {
        let n = (frames as usize - start).min(8192);
        renderer.render(&mut left[start..start + n], &mut right[start..start + n])?;
    }
    write_wav(&mix_path, &left, &right)?;
    let peak = left.iter().chain(&right).fold(0f32, |m, v| m.max(v.abs()));
    let rms = (left.iter().chain(&right).map(|v| (*v as f64).powi(2)).sum::<f64>() / (2 * frames) as f64).sqrt();
    println!(
        "Mix: {:.1}s, peak {:.2} dBFS, RMS {:.1} dBFS",
        frames as f64 / 48000.,
        20. * (peak as f64).log10(),
        20. * rms.log10()
    );
    // --stems <dir>: each track alone through its own inserts (no master
    // chain), for balancing.
    if let Some(dir) = args.iter().position(|a| a == "--stems").map(|i| PathBuf::from(&args[i + 1])) {
        std::fs::create_dir_all(&dir)?;
        let count = session["tracks"].as_array().unwrap().len();
        for index in 0..count {
            let mut solo = session.clone();
            solo["masterTrack"]["realtimeEffects"] = serde_json::json!([]);
            solo["masterTrack"]["gain"] = serde_json::json!(1.0);
            for (i, track) in solo["tracks"].as_array_mut().unwrap().iter_mut().enumerate() {
                track["muted"] = serde_json::json!(i != index);
            }
            std::fs::write(project_path.join("session.json"), serde_json::to_vec(&solo)?)?;
            let stem = Project::open(&project_path)?;
            let mut renderer = Renderer::new(&stem)?;
            for start in (0..frames as usize).step_by(8192) {
                let n = (frames as usize - start).min(8192);
                renderer.render(&mut left[start..start + n], &mut right[start..start + n])?;
            }
            let path = dir.join(format!("{index:02}.wav"));
            let _ = std::fs::remove_file(&path);
            write_wav(&path, &left, &right)?;
        }
        std::fs::write(project_path.join("session.json"), serde_json::to_vec_pretty(&session)?)?;
    }
    Ok(())
}
