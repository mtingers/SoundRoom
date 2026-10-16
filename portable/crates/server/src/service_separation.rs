//! Optional local learned source separation, isolated from the authority and
//! audio callback. Requests select an allowlisted model/source, never a path or
//! executable. Runtime installation is a trusted host operation.
use super::{engine, protocol, Error};
use serde_json::{json, Value};
use soundroom_client::models::RemoteEditorIntent;
use soundroom_engine::wave::WaveReader;
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Mutex, TryLockError,
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(all(test, unix))]
#[path = "service_separation_tests.rs"]
mod tests;

const MODELS: [&str; 3] = ["quality", "standard", "extended"];
const SOURCES: [&str; 6] = ["vocals", "drums", "bass", "other", "guitar", "piano"];
pub(super) const MAX_FRAMES: u64 = 30 * 60 * 48000;
const MAX_STATUS_BYTES: u64 = 65536;
const MAX_LINE: usize = 8192;
const MAX_TAIL: usize = 4096;
const TIMEOUT: Duration = Duration::from_secs(3600);
/// Expensive inference is serialized; ordinary job admission bounds waiting
/// workers, while Stop and Cancel retain their existing reserved control class.
static INFERENCE: Mutex<()> = Mutex::new(());

fn weights(model: &str, target: &str) -> &'static str {
    match (model, target) {
        ("standard", _) => "955717e8-8726e21a.th",
        ("extended", _) => "5c90dfd2-34c22ccb.th",
        (_, "vocals") => "04573f0d-f3cf25b2.th",
        (_, "drums") => "f7e0c4bc-ba3fe64a.th",
        (_, "bass") => "d12395a8-e57c48e6.th",
        _ => "92cfc3b6-ef3bcb9c.th",
    }
}
fn small_json(path: &Path) -> Option<Value> {
    let mut file = File::open(path).ok()?;
    if file.metadata().ok()?.len() > MAX_STATUS_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_STATUS_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_STATUS_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}
fn weight_stamp_matches(root: &Path, manifest: &Value, model: &str, target: &str) -> bool {
    let name = weights(model, target);
    let stamp = match std::fs::symlink_metadata(root.join("models").join(name)) {
        Ok(stamp) if stamp.is_file() && !stamp.file_type().is_symlink() => stamp,
        _ => return false,
    };
    let record = &manifest["files"][name];
    let Some(digest) = record["sha256"].as_str() else {
        return false;
    };
    let prefix = &name[9..17];
    let timestamp = stamp
        .modified()
        .ok()
        .and_then(|v| v.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|v| u64::try_from(v.as_nanos()).ok());
    digest.len() == 64
        && digest.bytes().all(|b| b.is_ascii_hexdigit())
        && digest.starts_with(prefix)
        && record["bytes"].as_u64() == Some(stamp.len())
        && record["mtime_ns"].as_u64() == timestamp
        && record["url"].as_str()
            == Some(
                format!("https://dl.fbaipublicfiles.com/demucs/hybrid_transformer/{name}").as_str(),
            )
}

#[derive(Clone, Debug)]
struct Runtime {
    root: PathBuf,
    targets: BTreeMap<String, Vec<String>>,
}
#[derive(Debug)]
pub(super) struct Plan {
    runtime: Runtime,
    model: String,
    target: String,
    operation: String,
}
impl Plan {
    pub(super) fn label(&self) -> String {
        format!(
            "{} {}",
            if self.operation == "remove" {
                "Remove"
            } else {
                "Isolate"
            },
            self.target
        )
    }
}

fn candidates(server_root: &Path) -> Vec<PathBuf> {
    if let Some(path) = std::env::var_os("SOUNDROOM_SEPARATION_RUNTIME") {
        return vec![PathBuf::from(path)];
    }
    let mut paths = vec![server_root.join("SeparationRuntime")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(contents) = exe.parent().and_then(Path::parent) {
            paths.push(contents.join("Resources/SeparationRuntime"));
            if let Some(build) = contents.parent().and_then(Path::parent) {
                paths.push(build.join("SeparationRuntime"));
            }
        }
        // Development helper binaries live under portable/target/release.
        for parent in exe.ancestors().take(8) {
            if parent.join(".git").exists() {
                paths.push(parent.join("build/SeparationRuntime"));
            }
        }
    }
    paths
}
fn load(root: PathBuf) -> Option<Runtime> {
    if !root.join(".venv/bin/python").is_file()
        || !root.join("worker.py").is_file()
        || !root.join("models").is_dir()
    {
        return None;
    }
    let value = small_json(&root.join("ready.json"))?;
    if value["schema"] != 1 || value["sampleRate"] != 48000 || value["runtimeReady"] != true {
        return None;
    }
    let manifest = small_json(&root.join("models/manifest.json"))?;
    if manifest["schema"] != 1
        || manifest["provider"] != "https://github.com/facebookresearch/demucs"
    {
        return None;
    }
    let mut targets = BTreeMap::new();
    for model in MODELS {
        let entry = &value["models"][model];
        if entry["available"] != true {
            continue;
        }
        let available: Vec<String> = entry["targets"]
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .filter(|target| {
                SOURCES.contains(target)
                    && (model == "extended" || !["guitar", "piano"].contains(target))
            })
            .filter(|target| weight_stamp_matches(&root, &manifest, model, target))
            .map(str::to_string)
            .collect();
        let expected = if model == "extended" {
            &SOURCES[..]
        } else {
            &SOURCES[..4]
        };
        if expected
            .iter()
            .all(|source| available.iter().any(|name| name == source))
        {
            targets.insert(model.into(), available);
        }
    }
    if targets.is_empty() {
        None
    } else {
        Some(Runtime { root, targets })
    }
}
fn discover(root: &Path) -> Option<Runtime> {
    candidates(root).into_iter().find_map(load)
}
pub(super) fn status(root: &Path) -> Value {
    match discover(root) {
        Some(runtime) => {
            json!({"available":true,"models":runtime.targets.keys().collect::<Vec<_>>(),
            "targets":runtime.targets,"message":"Local source separation models are ready."})
        }
        None => json!({"available":false,"models":[],"targets":{},
            "message":"Local source separation needs its optional model installation. Realtime spectral cleanup is available now."}),
    }
}
pub(super) fn plan(root: &Path, intent: &RemoteEditorIntent) -> Result<Plan, Error> {
    if intent.strings.get("kind").map(String::as_str) != Some("sourceIsolator") {
        return Err(protocol("Source separation requires Source isolator"));
    }
    let get = |key: &str| {
        intent
            .strings
            .get(key)
            .map(String::as_str)
            .ok_or_else(|| protocol("Choose a source, separation model, and operation"))
    };
    let model = get("separationModel")?;
    let target = get("separationTarget")?;
    let operation = get("separationOperation")?;
    if !MODELS.contains(&model)
        || !SOURCES.contains(&target)
        || !["extract", "remove"].contains(&operation)
    {
        return Err(protocol("Invalid source separation settings"));
    }
    if model != "extended" && ["guitar", "piano"].contains(&target) {
        return Err(protocol(
            "Guitar and piano separation require the extended model",
        ));
    }
    let runtime = discover(root).ok_or_else(|| {
        protocol("Install the local source separation models before using learned separation")
    })?;
    if !runtime
        .targets
        .get(model)
        .is_some_and(|targets| targets.iter().any(|name| name == target))
    {
        return Err(protocol(
            "The selected source separation model is not installed",
        ));
    }
    Ok(Plan {
        runtime,
        model: model.into(),
        target: target.into(),
        operation: operation.into(),
    })
}

fn cancelled(cancel: &AtomicBool) -> Result<(), Error> {
    if cancel.load(Ordering::Acquire) {
        Err(protocol("Source separation cancelled"))
    } else {
        Ok(())
    }
}
struct ChildGuard(Child, bool);
impl ChildGuard {
    fn stop(&mut self) {
        if self.1 {
            return;
        }
        self.1 = true;
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.stop();
    }
}
struct Logs {
    tail: Vec<u8>,
    error: Option<String>,
}
/// Drain continuously even if the child emits unbounded logs. Lines and retained
/// error tails are bounded; progress drops when the consumer already has newer work.
fn drain(
    mut reader: impl Read + Send + 'static,
    progress: mpsc::SyncSender<f64>,
) -> thread::JoinHandle<Logs> {
    thread::spawn(move || {
        let mut chunk = [0_u8; 4096];
        let mut line = Vec::with_capacity(MAX_LINE);
        let mut tail = Vec::with_capacity(MAX_TAIL);
        let mut overflow = false;
        let mut error = None;
        while let Ok(count) = reader.read(&mut chunk) {
            if count == 0 {
                break;
            }
            let keep = (tail.len() + count).saturating_sub(MAX_TAIL);
            if keep > 0 {
                tail.drain(..keep.min(tail.len()));
            }
            tail.extend_from_slice(&chunk[..count]);
            for byte in &chunk[..count] {
                if *byte == b'\n' {
                    if !overflow {
                        if let Ok(value) = serde_json::from_slice::<Value>(&line) {
                            if value["type"] == "progress" {
                                if let Some(amount) =
                                    value["progress"].as_f64().filter(|v| v.is_finite())
                                {
                                    let _ = progress.try_send(amount.clamp(0., 1.));
                                }
                            } else if value["type"] == "error" {
                                error = value["message"].as_str().map(|v| {
                                    v.chars().filter(|c| !c.is_control()).take(512).collect()
                                });
                            }
                        }
                    }
                    line.clear();
                    overflow = false;
                } else if line.len() < MAX_LINE {
                    line.push(*byte);
                } else {
                    overflow = true;
                }
            }
        }
        Logs { tail, error }
    })
}

pub(super) fn run(
    plan: &Plan,
    input: &Path,
    output: &Path,
    frames: u64,
    cancel: &AtomicBool,
    progress: impl FnMut(u64, u64),
) -> Result<(), Error> {
    run_timed(plan, input, output, frames, cancel, progress, TIMEOUT)
}
fn run_timed(
    plan: &Plan,
    input: &Path,
    output: &Path,
    frames: u64,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
    timeout: Duration,
) -> Result<(), Error> {
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let name = output
        .file_name()
        .ok_or_else(|| protocol("Invalid source separation output"))?
        .to_string_lossy();
    let _scratch = Scratch(output.with_file_name(format!(".{name}.separation.tmp")));
    let deadline = Instant::now() + timeout;
    let _permit = loop {
        cancelled(cancel)?;
        if Instant::now() >= deadline {
            return Err(protocol("Source separation timed out while waiting"));
        }
        match INFERENCE.try_lock() {
            Ok(permit) => break permit,
            Err(TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(25)),
            Err(TryLockError::Poisoned(_)) => {
                return Err(protocol("Source separation is unavailable"))
            }
        }
    };
    cancelled(cancel)?;
    let root = &plan.runtime.root;
    let mut command = Command::new(root.join(".venv/bin/python"));
    command
        .arg("-I")
        .arg(root.join("worker.py"))
        .arg("--input")
        .arg(input)
        .arg("--output")
        .arg(output)
        .arg("--target")
        .arg(&plan.target)
        .arg("--model")
        .arg(&plan.model)
        .arg("--operation")
        .arg(&plan.operation)
        .arg("--models-dir")
        .arg(root.join("models"))
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = ChildGuard(
        command
            .spawn()
            .map_err(|_| protocol("Could not start the local source separation runtime"))?,
        false,
    );
    let (sender, receiver) = mpsc::sync_channel(16);
    let stdout = drain(
        child
            .0
            .stdout
            .take()
            .ok_or_else(|| protocol("Source separation output is unavailable"))?,
        sender.clone(),
    );
    let stderr = drain(
        child
            .0
            .stderr
            .take()
            .ok_or_else(|| protocol("Source separation log is unavailable"))?,
        sender,
    );
    let result = loop {
        while let Ok(amount) = receiver.try_recv() {
            progress((amount * frames as f64) as u64, frames);
        }
        if cancel.load(Ordering::Acquire) {
            break Err(protocol("Source separation cancelled"));
        }
        if Instant::now() >= deadline {
            break Err(protocol("Source separation timed out"));
        }
        match child.0.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(_) => break Err(protocol("Source separation worker became unavailable")),
        }
    };
    // Kill any descendants still holding the log pipes and reap the direct child
    // before joining drainers. No helper survives cancellation or failed jobs.
    child.stop();
    let stdout = stdout.join().unwrap_or(Logs {
        tail: vec![],
        error: None,
    });
    let stderr = stderr.join().unwrap_or(Logs {
        tail: vec![],
        error: None,
    });
    let status = result?;
    cancelled(cancel)?;
    if !status.success() {
        let message = stdout.error.or(stderr.error).unwrap_or_else(|| {
            let tail = String::from_utf8_lossy(&stderr.tail);
            tail.lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("The local model worker failed")
                .chars()
                .filter(|c| !c.is_control())
                .take(512)
                .collect()
        });
        return Err(protocol(&format!("Source separation failed: {message}")));
    }
    validate_output(output, frames, cancel)?;
    progress(frames, frames);
    Ok(())
}
fn validate_output(output: &Path, frames: u64, cancel: &AtomicBool) -> Result<(), Error> {
    let mut file =
        File::open(output).map_err(|_| protocol("Source separation did not produce audio"))?;
    let info = WaveReader::from_file(file.try_clone()?)
        .map_err(engine)?
        .info;
    if info.channels != 2
        || info.sample_rate != 48000
        || !info.float
        || info.bits != 32
        || info.frames != frames
    {
        return Err(protocol(
            "Source separation returned incompatible or unaligned audio",
        ));
    }
    file.seek(SeekFrom::Start(info.data_offset))?;
    let mut bytes = [0_u8; 8192 * 8];
    let mut remaining = frames;
    while remaining > 0 {
        cancelled(cancel)?;
        let count = remaining.min(8192) as usize;
        file.read_exact(&mut bytes[..count * 8])?;
        if bytes[..count * 8].chunks_exact(4).any(|sample| {
            let value = f32::from_le_bytes(sample.try_into().unwrap());
            !value.is_finite() || value.abs() > 16.
        }) {
            return Err(protocol("Source separation returned invalid audio samples"));
        }
        remaining -= count as u64;
    }
    Ok(())
}
