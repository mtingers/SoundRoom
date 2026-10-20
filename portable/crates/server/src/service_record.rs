//! Recording: one capture thread per take owns the input (Core Audio input
//! objects are single-thread), archives every native packet, aligns it to the
//! actual output callback clock, and streams per-track take WAVs. Finishing
//! commits all takes as one server-placed "Record" action.
use super::{engine, new_id, protocol, Error, Identity, Service};
use serde_json::Value;
use soundroom_engine::{
    capture::{capture_channel, CaptureTiming, CaptureWriter},
    capture_archive::{ArchiveManifest, CaptureArchive},
    capture_clock::{CaptureAligner, ClockConfig},
    capture_worker::{route_track, RecordingSourceWorker},
    model::Clip,
    project::Media,
    wave::{WaveReader, WaveWriter},
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{sync_channel, Receiver},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
/// Test-only input clocked by the simulated output callback (same host clock).
pub type SimulatedInput = Arc<Mutex<Option<CaptureWriter>>>;
const SIMULATED_UID: &str = "simulated-input";
const SIMULATED_TICKS: f64 = 1e9;
#[derive(Clone)]
pub(super) enum Device {
    #[cfg(target_os = "macos")]
    Native {
        uid: String,
        channels: usize,
        rate: u32,
        buffer: usize,
    },
    Simulated(SimulatedInput),
}
impl Device {
    fn uid(&self) -> String {
        match self {
            #[cfg(target_os = "macos")]
            Self::Native { uid, .. } => uid.clone(),
            Self::Simulated(_) => SIMULATED_UID.into(),
        }
    }
    pub(super) fn channels(&self) -> usize {
        match self {
            #[cfg(target_os = "macos")]
            Self::Native { channels, .. } => *channels,
            Self::Simulated(_) => 1,
        }
    }
    fn rate(&self) -> u32 {
        match self {
            #[cfg(target_os = "macos")]
            Self::Native { rate, .. } => *rate,
            Self::Simulated(_) => 48000,
        }
    }
    fn maximum_frames(&self) -> usize {
        match self {
            #[cfg(target_os = "macos")]
            Self::Native { buffer, .. } => (*buffer * 2).clamp(512, 8192),
            Self::Simulated(_) => 512,
        }
    }
    fn host_ticks(&self) -> Result<f64, Error> {
        match self {
            #[cfg(target_os = "macos")]
            Self::Native { .. } => {
                soundroom_engine::core_audio::host_ticks_per_second().map_err(engine)
            }
            Self::Simulated(_) => Ok(SIMULATED_TICKS),
        }
    }
}
pub(super) struct Route {
    pub track_id: String,
    pub channels: Vec<usize>,
}
pub(super) struct Plan {
    pub device: Device,
    pub routes: Vec<Route>,
    pub offset_frames: i64,
    pub directory: PathBuf,
    /// Punch-out: takes end here and the recording finishes once input covers it.
    pub end_frame: Option<u64>,
    /// The transport click until the next stop, armed with the count-in.
    pub metronome: bool,
    /// Armed inputs played through the output while recording.
    pub monitor: Option<Monitor>,
}
pub(super) struct MonitorRoute {
    pub channels: Vec<usize>,
    pub gain: f32,
    pub pan: f32,
}
pub(super) struct Monitor {
    pub writer: soundroom_engine::preview::StreamWriter,
    pub enabled: Arc<AtomicBool>,
    pub routes: Vec<MonitorRoute>,
}
/// Mixes native input blocks for the armed tracks and resamples them linearly
/// to 48 kHz into the monitor ring (dropping, never blocking, when it is full).
struct MonitorState {
    monitor: Monitor,
    /// Input frames per 48 kHz output frame: nominal, and as steered (within
    /// ±0.2 %) to hold the ring near `target` while the input and output
    /// device clocks drift apart; otherwise it slowly overfills (dropped
    /// input) or runs dry (gaps), and both click.
    nominal: f64,
    step: f64,
    /// Smoothed ring fill in frames (negative until measured).
    fill: f64,
    position: f64,
    previous: [f32; 2],
    /// Fades monitoring in and out (10 ms) so toggling it never clicks.
    level: f32,
    left: Vec<f32>,
    right: Vec<f32>,
}
impl MonitorState {
    fn new(monitor: Monitor, rate: u32) -> Self {
        Self {
            monitor,
            nominal: rate.max(1) as f64 / 48000.,
            step: rate.max(1) as f64 / 48000.,
            fill: -1.,
            position: 0.,
            previous: [0.; 2],
            level: 0.,
            left: vec![],
            right: vec![],
        }
    }
    fn feed(&mut self, views: &[&[f32]], frames: usize) {
        let enabled = self.monitor.enabled.load(Ordering::Acquire);
        if !enabled && self.level == 0. {
            self.position = 0.;
            self.previous = [0.; 2];
            self.fill = -1.;
            return;
        }
        let queued = self.monitor.writer.queued() as f64;
        self.fill = if self.fill < 0. {
            queued
        } else {
            self.fill + 0.05 * (queued - self.fill)
        };
        let target = (frames as f64 / self.nominal * 1.5).max(256.);
        let error = (self.fill - target) / soundroom_engine::preview::RING as f64;
        self.step = self.nominal * (1. + (error * 0.02).clamp(-0.002, 0.002));
        let mix = |i: usize| -> [f32; 2] {
            let mut out = [0f32; 2];
            for route in &self.monitor.routes {
                let sample = |c: usize| views.get(c).and_then(|v| v.get(i)).copied().unwrap_or(0.);
                let (l, r) = match route.channels.as_slice() {
                    [a, b] => (sample(*a), sample(*b)),
                    [a] => (sample(*a), sample(*a)),
                    _ => (0., 0.),
                };
                let angle = (route.pan.clamp(-1., 1.) + 1.) * std::f32::consts::FRAC_PI_4;
                out[0] += l * route.gain * angle.cos() * std::f32::consts::SQRT_2;
                out[1] += r * route.gain * angle.sin() * std::f32::consts::SQRT_2;
            }
            out
        };
        self.left.clear();
        self.right.clear();
        // `position` is the fractional source index of the next output sample,
        // relative to this block (negative values interpolate from `previous`).
        while self.position < frames as f64 {
            let index = self.position.floor();
            let fraction = (self.position - index) as f32;
            let a = if index < 0. {
                self.previous
            } else {
                mix(index as usize)
            };
            let b = if index + 1. < frames as f64 {
                mix((index + 1.) as usize)
            } else {
                a
            };
            self.left.push(a[0] + (b[0] - a[0]) * fraction);
            self.right.push(a[1] + (b[1] - a[1]) * fraction);
            self.position += self.step;
        }
        self.position -= frames as f64;
        if frames > 0 {
            self.previous = mix(frames - 1);
        }
        let step = if enabled { 1. / 480. } else { -1. / 480. };
        for (l, r) in self.left.iter_mut().zip(&mut self.right) {
            self.level = (self.level + step).clamp(0., 1.);
            *l *= self.level;
            *r *= self.level;
        }
        let _ = self.monitor.writer.write(&self.left, &self.right);
    }
}
pub(super) struct Active {
    pub monitoring: Option<Arc<AtomicBool>>,
    stop: Arc<AtomicBool>,
    punched: Arc<AtomicBool>,
    done: Receiver<Result<Captured, Error>>,
    pub start_frame: u64,
    pub owner: Identity,
}
pub(super) struct Take {
    pub track_id: String,
    pub path: PathBuf,
    pub start: u64,
    pub frames: u64,
}
pub(super) struct Captured {
    pub takes: Vec<Take>,
    pub complete: bool,
}
impl Active {
    pub fn request_stop(&self) {
        if let Some(monitoring) = &self.monitoring {
            monitoring.store(false, Ordering::Release);
        }
        self.stop.store(true, Ordering::Release);
    }
    /// True once a punch recording's input covers the whole punch range.
    pub fn punch_complete(&self) -> bool {
        self.punched.load(Ordering::Acquire)
    }
    /// Stops the input and waits (bounded) for the archived, aligned takes.
    pub fn finish(self) -> Result<Captured, Error> {
        self.request_stop();
        self.done
            .recv_timeout(Duration::from_secs(15))
            .map_err(|_| {
                protocol("Recording did not finish in time; its raw archive is retained")
            })?
    }
}
/// Starts capture; returns once the input is open and running.
pub(super) fn start(
    plan: Plan,
    backend: crate::audio::Backend,
    start_frame: u64,
    owner: Identity,
    stop: Arc<AtomicBool>,
) -> Result<Active, Error> {
    if stop.load(Ordering::Acquire) {
        return Err(protocol("Recording startup was stopped"));
    }
    let monitoring = plan.monitor.as_ref().map(|m| m.enabled.clone());
    let punched = Arc::new(AtomicBool::new(false));
    let punch_flag = punched.clone();
    let (done, receipt) = sync_channel(1);
    let (ready, opened) = sync_channel::<Result<(), String>>(1);
    let flag = stop.clone();
    std::thread::spawn(move || {
        let result = capture(plan, backend, start_frame, flag, punch_flag, &ready);
        if let Err(error) = &result {
            let _ = ready.try_send(Err(error.to_string()));
        }
        let _ = done.send(result);
    });
    match opened.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => Ok(Active {
            monitoring,
            stop,
            punched,
            done: receipt,
            start_frame,
            owner,
        }),
        Ok(Err(message)) => Err(protocol(&message)),
        Err(_) => {
            stop.store(true, Ordering::Release);
            Err(protocol("The input device did not start"))
        }
    }
}
struct TrackTake {
    track_id: String,
    route: Vec<usize>,
    path: PathBuf,
    writer: Option<WaveWriter>,
    start: Option<u64>,
}
fn capture(
    mut plan: Plan,
    backend: crate::audio::Backend,
    start_frame: u64,
    stop: Arc<AtomicBool>,
    punched: Arc<AtomicBool>,
    ready: &std::sync::mpsc::SyncSender<Result<(), String>>,
) -> Result<Captured, Error> {
    if stop.load(Ordering::Acquire) {
        return Err(protocol("Recording startup was stopped"));
    }
    let take_id = new_id()?.to_ascii_uppercase();
    // The archive's private-directory walk rejects symlinked components
    // (e.g. macOS /var -> /private/var), so resolve the parent first.
    let end_frame = plan.end_frame;
    std::fs::create_dir_all(&plan.directory)?;
    let directory = plan.directory.canonicalize()?.join(&take_id);
    std::fs::create_dir_all(&directory)?;
    let channels = plan.device.channels();
    let maximum = plan.device.maximum_frames();
    let ticks = plan.device.host_ticks()?;
    let manifest = ArchiveManifest {
        version: 1,
        take_id: take_id.clone(),
        device_uid: plan.device.uid(),
        channels,
        sample_rate: plan.device.rate(),
        maximum_frames: maximum,
        host_ticks_per_second: ticks,
        calibration_offset_frames: plan.offset_frames,
    };
    let archive =
        CaptureArchive::create(&directory.join("raw"), manifest.clone()).map_err(engine)?;
    let (writer, mut reader) = capture_channel(manifest.capture_config(64)).map_err(engine)?;
    let control = writer.control();
    let clock = |rate| ClockConfig {
        sample_rate: rate,
        host_ticks_per_second: ticks,
        maximum_drift_ppm: 1000.,
    };
    let aligner = CaptureAligner::new(clock(plan.device.rate()), clock(48000), plan.offset_frames)
        .map_err(engine)?;
    let mut worker =
        RecordingSourceWorker::with_capture(archive, aligner, 2000., &control).map_err(engine)?;
    let mut tracks = plan
        .routes
        .iter()
        .map(|route| {
            let path = directory.join(format!("{}.wav", route.track_id));
            Ok(TrackTake {
                track_id: route.track_id.clone(),
                route: route.channels.clone(),
                writer: Some(WaveWriter::stream(&path).map_err(engine)?),
                path,
                start: None,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    #[cfg(target_os = "macos")]
    let mut native = None;
    if stop.load(Ordering::Acquire) {
        return Err(protocol("Recording startup was stopped"));
    }
    match &plan.device {
        #[cfg(target_os = "macos")]
        Device::Native { uid, .. } => {
            let mut input = soundroom_engine::core_audio::CoreAudioInput::open_device(writer, uid)
                .map_err(engine)?;
            input.start().map_err(engine)?;
            native = Some(input);
        }
        Device::Simulated(slot) => {
            *slot
                .lock()
                .map_err(|_| protocol("Simulated input unavailable"))? = Some(writer);
        }
    }
    let _ = ready.try_send(Ok(()));
    let mut input = vec![vec![0f32; maximum]; channels];
    let mut out = vec![vec![0f32; 4096]; channels];
    let mut last_anchor = None;
    let rate = plan.device.rate();
    let mut monitor = plan.monitor.take().map(|m| MonitorState::new(m, rate));
    let ingest = |reader: &mut soundroom_engine::capture::CaptureReader,
                  worker: &mut RecordingSourceWorker,
                  input: &mut Vec<Vec<f32>>,
                  monitor: &mut Option<MonitorState>,
                  interruptible: bool|
     -> Result<(), Error> {
        loop {
            if interruptible && stop.load(Ordering::Acquire) {
                return Ok(());
            }
            let block = {
                let mut views: Vec<&mut [f32]> =
                    input.iter_mut().map(|v| v.as_mut_slice()).collect();
                reader.read_planar(&mut views).map_err(engine)?
            };
            let Some(block) = block else { return Ok(()) };
            let views: Vec<&[f32]> = input.iter().map(|v| &v[..block.frames]).collect();
            if let Some(monitor) = monitor.as_mut() {
                monitor.feed(&views, block.frames);
            }
            // Alignment errors suspend publication; the raw packet stays archived.
            if let Err(error) = worker.ingest(block, &views) {
                debug(format_args!("ingest: {error}"));
            }
        }
    };
    loop {
        // Seal the producer before draining its remaining packets. Draining a
        // still-running input can keep replenishing work and postpone Stop.
        if stop.load(Ordering::Acquire) {
            break;
        }
        if let Some(anchor) = backend.output_anchor() {
            if last_anchor != Some(anchor) {
                if let Err(error) = worker.observe_output(anchor) {
                    debug(format_args!("anchor: {error}"));
                }
                last_anchor = Some(anchor);
            }
        }
        ingest(&mut reader, &mut worker, &mut input, &mut monitor, true)?;
        if stop.load(Ordering::Acquire) {
            break;
        }
        if let Err(error) = worker.observe_capture_health(reader.stats()) {
            debug(format_args!("health: {error}"));
        }
        publish(
            &mut worker,
            &mut out,
            &mut tracks,
            start_frame,
            end_frame,
            &punched,
        )?;
        std::thread::sleep(Duration::from_millis(2));
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    #[cfg(target_os = "macos")]
    let stopped = match native.as_mut() {
        Some(input) => loop {
            match input.stop_and_seal().map_err(engine)? {
                Some(stopped) => break Some(stopped),
                None if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(1)),
                None => {
                    return Err(protocol(
                        "The input did not stop; its raw archive is retained",
                    ))
                }
            }
        },
        None => None,
    };
    if let Device::Simulated(slot) = &plan.device {
        if let Ok(mut slot) = slot.lock() {
            slot.take();
        }
        control.seal();
    }
    let receipt = loop {
        ingest(&mut reader, &mut worker, &mut input, &mut monitor, false)?;
        if let Some(receipt) = reader.seal_receipt() {
            break receipt;
        }
        if Instant::now() > deadline {
            return Err(protocol(
                "The input queue did not drain; its raw archive is retained",
            ));
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    worker.finish_conversion();
    publish(
        &mut worker,
        &mut out,
        &mut tracks,
        start_frame,
        end_frame,
        &punched,
    )?;
    let suspended = worker.suspended();
    #[cfg(target_os = "macos")]
    let complete = match stopped {
        Some(stopped) => worker
            .finish_core_audio_capture(stopped, receipt)
            .map_err(engine)?
            .is_complete(),
        None => {
            worker.finish_archive_sealed(receipt).map_err(engine)?;
            !suspended
        }
    };
    #[cfg(not(target_os = "macos"))]
    let complete = {
        worker.finish_archive_sealed(receipt).map_err(engine)?;
        !suspended
    };
    let mut takes = vec![];
    for mut track in tracks {
        let writer = track.writer.take().unwrap();
        let frames = writer.frames();
        match track.start.filter(|_| frames > 0) {
            Some(start) => {
                writer.finish().map_err(engine)?;
                takes.push(Take {
                    track_id: track.track_id,
                    path: track.path,
                    start,
                    frames,
                });
            }
            None => {
                drop(writer);
                let _ = std::fs::remove_file(&track.path);
            }
        }
    }
    Ok(Captured { takes, complete })
}
/// Recording diagnostics on stderr (server.log) with SOUNDROOM_DEBUG_RECORDING=1.
fn debug(message: std::fmt::Arguments) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var_os("SOUNDROOM_DEBUG_RECORDING").is_some()) {
        eprintln!("Soundroom recording: {message}");
    }
}
/// Writes aligned output (session frames) into each armed track's take,
/// trimming anything before the recording start.
fn publish(
    worker: &mut RecordingSourceWorker,
    out: &mut [Vec<f32>],
    tracks: &mut [TrackTake],
    start_frame: u64,
    end_frame: Option<u64>,
    punched: &AtomicBool,
) -> Result<(), Error> {
    let mut left = vec![0f32; 4096];
    let mut right = vec![0f32; 4096];
    loop {
        let block = {
            let mut views: Vec<&mut [f32]> = out.iter_mut().map(|v| v.as_mut_slice()).collect();
            worker.drain(&mut views).map_err(engine)?
        };
        let Some(block) = block.filter(|b| b.frames > 0) else {
            return Ok(());
        };
        let skip = (start_frame as i64 - block.output_frame).clamp(0, block.frames as i64) as usize;
        if skip == block.frames {
            continue;
        }
        let first = (block.output_frame + skip as i64) as u64;
        let mut frames = block.frames - skip;
        if let Some(end) = end_frame {
            if first + frames as u64 >= end {
                punched.store(true, Ordering::Release);
            }
            frames = frames.min(end.saturating_sub(first) as usize);
            if frames == 0 {
                continue;
            }
        }
        let views: Vec<&[f32]> = out.iter().map(|v| &v[skip..skip + frames]).collect();
        for track in tracks.iter_mut() {
            route_track(
                &views,
                &track.route,
                &mut left[..frames],
                &mut right[..frames],
            )
            .map_err(engine)?;
            track.start.get_or_insert(first);
            if let Some(writer) = track.writer.as_mut() {
                writer
                    .write(&left[..frames], &right[..frames])
                    .map_err(engine)?;
            }
        }
    }
}
/// Pushes one simulated input packet from the simulated output callback.
pub fn push_simulated(slot: &SimulatedInput, phase: &mut u64, host_time: u64, frames: usize) {
    let Ok(mut slot) = slot.try_lock() else {
        return;
    };
    let Some(writer) = slot.as_mut() else { return };
    let mut tone = [0f32; 512];
    for value in tone.iter_mut().take(frames) {
        *value = ((*phase as f64) * 2. * std::f64::consts::PI * 440. / 48000.).sin() as f32 * 0.25;
        *phase += 1;
    }
    let sample_time = (*phase - frames as u64) as f64;
    writer.push(
        &[&tone[..frames]],
        CaptureTiming {
            sample_time,
            host_time,
            flags: 3,
        },
    );
}
impl Service {
    pub fn simulated_input(&mut self) -> SimulatedInput {
        self.simulated_input
            .get_or_insert_with(|| Arc::new(Mutex::new(None)))
            .clone()
    }
    /// Every armed track must resolve to an available input. This host opens one
    /// input clock; mixed-device capture requires a future aggregate adapter.
    pub(super) fn recording_device(&self) -> Result<Device, Error> {
        if let Some(slot) = &self.simulated_input {
            selected_input_uid(
                &self.authority.session,
                self.input_uid.as_deref(),
                Some(SIMULATED_UID),
                &[(SIMULATED_UID.to_owned(), 1)],
            )?;
            return Ok(Device::Simulated(slot.clone()));
        }
        #[cfg(target_os = "macos")]
        {
            let devices = soundroom_engine::core_audio::core_audio_devices().map_err(engine)?;
            let available = devices
                .iter()
                .map(|device| (device.uid.clone(), device.input_channels as usize))
                .collect::<Vec<_>>();
            let uid = selected_input_uid(
                &self.authority.session,
                self.input_uid.as_deref(),
                devices
                    .iter()
                    .find(|device| device.is_default_input)
                    .map(|device| device.uid.as_str()),
                &available,
            )?;
            let device = devices
                .iter()
                .find(|d| d.uid == uid)
                .filter(|d| d.input_channels > 0)
                .ok_or_else(|| {
                    Error::Conflict("The selected input device is unavailable".into())
                })?;
            Ok(Device::Native {
                uid: device.uid.clone(),
                channels: device.input_channels as usize,
                rate: device.nominal_sample_rate.round() as u32,
                buffer: device.buffer_frames as usize,
            })
        }
        #[cfg(not(target_os = "macos"))]
        Err(protocol(
            "Recording needs the macOS input backend in this build",
        ))
    }
    /// Commits all takes as one server-placed authored action.
    pub(super) fn commit_takes(
        &mut self,
        peer: &Identity,
        captured: Captured,
    ) -> Result<usize, Error> {
        let mut candidate = self.authority.session.clone();
        let mut added = vec![];
        let result = (|| {
            for take in &captured.takes {
                let Some(track) = candidate
                    .tracks
                    .iter_mut()
                    .find(|t| t.id.eq_ignore_ascii_case(&take.track_id))
                else {
                    continue;
                };
                let asset = new_id()?;
                let destination = self.root.join("Media").join(format!("{asset}.wav"));
                std::fs::hard_link(&take.path, &destination)?;
                added.push(asset.clone());
                let info = WaveReader::open(&destination).map_err(engine)?.info;
                track.clips.push(Clip {
                    id: new_id()?,
                    asset_id: asset.clone(),
                    name: format!("{} take", track.name),
                    start: take.start,
                    offset: 0,
                    length: info.frames.min(take.frames),
                    edge_fade_frames: Some(super::EDGE_FADE_FRAMES),
                    edge_protection_disabled: None,
                    cut_fade_in_frames: None,
                    cut_fade_out_frames: None,
                    fade_in_frames: None,
                    fade_out_frames: None,
                    fade_reference_offset: None,
                    fade_reference_length: None,
                    render_envelope: None,
                    previous_states: vec![],
                    extra: BTreeMap::new(),
                });
                self.media.insert(
                    asset,
                    Media {
                        path: destination,
                        info,
                    },
                );
            }
            let revision = self.authority.revision;
            let versions = self.authority.track_revisions.clone();
            self.authority
                .edit_session(
                    &peer.runtime,
                    super::role(&peer.role),
                    Some(revision),
                    &versions,
                    candidate,
                )
                .map_err(engine)?;
            if self.authority.revision != revision {
                self.authority
                    .label_last_change(&peer.runtime, "Record", None)
                    .map_err(engine)?;
            }
            Ok(added.len())
        })();
        if result.is_err() {
            for asset in &added {
                if let Some(media) = self.media.remove(asset) {
                    let _ = std::fs::remove_file(media.path);
                }
            }
        }
        result
    }
}

/// Pure route admission; no microphone or system device state is changed here.
fn selected_input_uid(
    session: &soundroom_engine::model::Session,
    session_uid: Option<&str>,
    default_uid: Option<&str>,
    available: &[(String, usize)],
) -> Result<String, Error> {
    let mut selected: Option<&str> = None;
    for track in session.tracks.iter().filter(|track| track.armed) {
        let uid = match track.extra.get("inputDeviceUID") {
            Some(Value::String(uid)) if !uid.is_empty() => uid.as_str(),
            None | Some(Value::Null) => session_uid.or(default_uid).ok_or_else(|| {
                Error::Conflict(format!("{}: the session input is unavailable", track.name))
            })?,
            _ => {
                return Err(Error::Conflict(format!(
                    "{}: invalid saved input device",
                    track.name
                )))
            }
        };
        let channels = available
            .iter()
            .find(|(id, channels)| id == uid && *channels > 0)
            .map(|(_, channels)| *channels)
            .ok_or_else(|| {
                Error::Conflict(format!(
                    "{}: the selected input device is unavailable; choose its input source again",
                    track.name
                ))
            })?;
        let channel = match track.extra.get("inputChannel") {
            None | Some(Value::Null) => 0,
            Some(value) => value
                .as_u64()
                .filter(|channel| *channel < 64)
                .ok_or_else(|| Error::Conflict(format!("{}: invalid input channel", track.name)))?
                as usize,
        };
        let stereo = match track.extra.get("stereoInput") {
            None | Some(Value::Null) => false,
            Some(value) => value
                .as_bool()
                .ok_or_else(|| Error::Conflict(format!("{}: invalid stereo input", track.name)))?,
        };
        if channels > 64 || channel >= channels || (stereo && channel + 1 >= channels) {
            return Err(Error::Conflict(format!(
                "{}: the selected input channels are unavailable",
                track.name
            )));
        }
        if selected.is_some_and(|selected| selected != uid) {
            return Err(Error::Conflict("This Rust host records one input device at a time. Assign every armed track to the same microphone or interface.".into()));
        }
        selected = Some(uid);
    }
    selected
        .map(str::to_owned)
        .ok_or_else(|| Error::Conflict("Arm a track to record".into()))
}

/// Stops capture, stops the transport and commits the takes. Used by an
/// explicit Stop/Finish and by punch-out completion.
pub(super) fn finish(
    shared: &super::Shared,
    active: Active,
    backend: crate::audio::Backend,
) -> Result<serde_json::Value, Error> {
    struct Finalizing(super::Shared);
    impl Drop for Finalizing {
        fn drop(&mut self) {
            if let Ok(mut state) = self.0.lock() {
                state.recording_finalizing = false;
                state.broadcast_transport();
            }
        }
    }
    let _finalizing = Finalizing(shared.clone());
    let owner = active.owner.clone();
    active.request_stop();
    let telemetry = backend.execute(
        crate::audio::Command::Pause,
        Box::new(soundroom_engine::project::Project {
            session: Default::default(),
            media: Default::default(),
        }),
        0,
        Arc::new(AtomicBool::new(false)),
    );
    // Publish the actual callback-confirmed stop before disk/archive work.
    {
        let mut state = shared
            .lock()
            .map_err(|_| protocol("Authority is unavailable"))?;
        if let Ok(telemetry) = &telemetry {
            state.audio_telemetry(*telemetry);
            state.broadcast_transport();
        }
        state.status_event("Finishing take…");
    }
    let captured = active.finish()?;
    let complete = captured.complete;
    let mut state = shared
        .lock()
        .map_err(|_| protocol("Authority is unavailable"))?;
    // A failed Pause must not discard the take: commit it, then report.
    if let Err(error) = &telemetry {
        state.status_event(&format!("Transport did not confirm stop: {error}"));
    }
    let before = state.authority.session.clone();
    let revision = state.authority.revision;
    let takes = state.commit_takes(&owner, captured)?;
    if state.publish_changes(before, revision).is_err() {
        state.close_connections();
    }
    if revision != state.authority.revision {
        state.history_events();
    }
    state.broadcast_transport();
    Ok(serde_json::json!({"takes": takes, "complete": complete}))
}
impl Service {
    /// Reads, or with `volume` sets, an input device's hardware level (the
    /// System Settings input volume). `uid` defaults to the session input.
    /// The reply is `{uid, volume}`; volume is null when the device has none.
    pub(super) fn input_volume(
        &mut self,
        peer: &Identity,
        intent: &soundroom_client::models::RemoteEditorIntent,
    ) -> Result<super::Handled, Error> {
        let volume = intent.numbers.get("volume").copied();
        if volume.is_some() && peer.role != soundroom_client::models::RemoteRole::Operator {
            return Err(protocol("Operator role is required"));
        }
        #[cfg(target_os = "macos")]
        {
            use soundroom_engine::core_audio;
            // Empty is the system default input.
            let uid = intent
                .strings
                .get("uid")
                .filter(|uid| !uid.is_empty())
                .or(self.input_uid.as_ref())
                .map_or("", String::as_str);
            let (uid, level) = match volume {
                Some(volume) => core_audio::set_input_volume(uid, volume as f32),
                None => core_audio::input_volume(uid),
            }
            .map_err(engine)?;
            Ok(super::Handled::Payload(Some(
                serde_json::json!({"uid": uid, "volume": level}),
            )))
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (volume, &self.input_uid);
            Err(protocol("This host cannot adjust input volume"))
        }
    }
    /// Commits a punch recording whose input covers its range, on its own
    /// thread so the audio driver never blocks.
    pub(crate) fn finish_completed_punch(&mut self, shared: super::Shared) {
        if !self.recording.as_ref().is_some_and(Active::punch_complete) {
            return;
        }
        let (Some(backend), Some(active)) = (self.backend.clone(), self.recording.take()) else {
            return;
        };
        active.request_stop();
        self.recording_finalizing = true;
        std::thread::spawn(move || {
            if let Err(error) = finish(&shared, active, backend) {
                if let Ok(state) = shared.lock() {
                    state.status_event(&error.to_string());
                }
            }
        });
    }
}

#[cfg(test)]
mod monitor_tests {
    use super::*;
    #[test]
    fn saved_input_uids_are_honored_and_missing_or_mixed_routes_are_rejected() {
        use soundroom_engine::model::{Session, Track};
        let available = vec![("default".into(), 1), ("interface".into(), 4)];
        let mut session = Session::default();
        let mut track = Track {
            id: new_id().unwrap(),
            armed: true,
            ..Track::default()
        };
        track
            .extra
            .insert("inputDeviceUID".into(), serde_json::json!("interface"));
        track
            .extra
            .insert("inputChannel".into(), serde_json::json!(2));
        track
            .extra
            .insert("stereoInput".into(), serde_json::json!(true));
        session.tracks.push(track);
        assert_eq!(
            selected_input_uid(&session, Some("default"), Some("default"), &available).unwrap(),
            "interface"
        );
        session.tracks[0]
            .extra
            .insert("inputDeviceUID".into(), serde_json::json!("missing"));
        assert!(
            selected_input_uid(&session, Some("default"), Some("default"), &available).is_err()
        );
        session.tracks[0]
            .extra
            .insert("inputDeviceUID".into(), serde_json::json!("interface"));
        session.tracks.push(Track {
            id: new_id().unwrap(),
            armed: true,
            ..Track::default()
        });
        assert!(
            selected_input_uid(&session, Some("default"), Some("default"), &available).is_err()
        );
        assert_eq!(
            selected_input_uid(&session, Some("interface"), Some("default"), &available).unwrap(),
            "interface"
        );
        session.tracks[0]
            .extra
            .insert("inputChannel".into(), serde_json::json!(3));
        assert!(
            selected_input_uid(&session, Some("interface"), Some("default"), &available).is_err()
        );
        session.tracks[0]
            .extra
            .insert("inputChannel".into(), serde_json::json!(-1));
        assert!(
            selected_input_uid(&session, Some("interface"), Some("default"), &available).is_err()
        );
    }
    #[test]
    fn recording_stop_is_published_before_a_blocked_archive_finishes() {
        for fail in [false, true] {
            let (root, shared, mut owner, _) = super::super::publication_tests::control_fixture(0);
            let credential = shared
                .lock()
                .unwrap()
                .registry
                .issue("Operator", soundroom_client::models::RemoteRole::Operator)
                .unwrap();
            owner.credential = credential["credentialID"].as_str().unwrap().into();
            owner.name = "Operator".into();
            owner.role = soundroom_client::models::RemoteRole::Operator;
            let (control, mut host) = soundroom_engine::realtime::channel();
            Service::attach_audio(&shared, control).unwrap();
            let running = Arc::new(AtomicBool::new(true));
            let callback_running = running.clone();
            let callback = std::thread::spawn(move || {
                while callback_running.load(Ordering::Acquire) {
                    host.render(&mut [0.; 128], &mut [0.; 128]);
                    std::thread::sleep(Duration::from_millis(2));
                }
            });
            let (out, inbox) = super::super::Outbox::pair(shared.lock().unwrap().global.clone());
            shared
                .lock()
                .unwrap()
                .join(owner.clone(), out.clone())
                .unwrap();
            let (release, receipt) = sync_channel(1);
            let stop = Arc::new(AtomicBool::new(false));
            let monitoring = Arc::new(AtomicBool::new(true));
            let active = Active {
                monitoring: Some(monitoring.clone()),
                stop: stop.clone(),
                punched: Arc::new(AtomicBool::new(false)),
                done: receipt,
                start_frame: 0,
                owner: owner.clone(),
            };
            shared.lock().unwrap().recording = Some(active);
            let request_id = new_id().unwrap();
            Service::dispatch(
                &shared,
                &owner,
                &out,
                serde_json::json!({
                    "version":3,"kind":"request","id":request_id,"clientID":owner.runtime,
                    "expectedTrackRevisions":{},"command":{"type":"stop"}
                }),
            )
            .unwrap();
            assert!(
                stop.load(Ordering::Acquire),
                "Admission must signal capture immediately"
            );
            assert!(
                !monitoring.load(Ordering::Acquire),
                "Stop must silence input monitoring before archive work"
            );
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let bytes = inbox
                    .ordinary
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .unwrap();
                inbox.received(bytes.len());
                let event: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                if event["event"]["type"] == "transport" {
                    let transport = &event["event"]["transport"];
                    if transport["isPlaying"] == false
                        && transport["isRecording"] == false
                        && transport["isPreparing"] == true
                    {
                        break;
                    }
                }
            }
            assert!(
                shared.lock().unwrap().pending.contains_key(&(
                    owner.credential.clone(),
                    owner.runtime.clone(),
                    request_id.clone()
                )),
                "Stop must be visible while archive receipt is blocked"
            );
            {
                let mut state = shared.lock().unwrap();
                assert!(
                    state.begin_recording(&owner, None).is_err(),
                    "A new take cannot overlap finalization"
                );
            }
            release
                .send(if fail {
                    Err(protocol("Injected archive failure"))
                } else {
                    Ok(Captured {
                        takes: vec![],
                        complete: true,
                    })
                })
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            let reply = loop {
                let bytes = inbox
                    .priority
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .unwrap();
                inbox.received(bytes.len());
                let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                if value["kind"] == "reply" && value["requestID"] == request_id {
                    break value;
                }
            };
            assert_eq!(reply["status"], if fail { "rejected" } else { "accepted" });
            assert!(
                !shared.lock().unwrap().recording_finalizing,
                "Success and failure both release busy state"
            );
            running.store(false, Ordering::Release);
            callback.join().unwrap();
            drop(shared);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn monitor_holds_its_ring_level_when_device_clocks_drift() {
        for drift in [0.999, 1.001] {
            let (control, mut mixer) = soundroom_engine::preview::channel();
            let mut state = MonitorState::new(
                Monitor {
                    writer: control.monitor_writer().unwrap(),
                    enabled: Arc::new(AtomicBool::new(true)),
                    routes: vec![MonitorRoute {
                        channels: vec![0],
                        gain: 1.,
                        pan: 0.,
                    }],
                },
                44_100,
            );
            let block = vec![0.5f32; 441];
            let (mut left, mut right) = (vec![0f32; 1024], vec![0f32; 1024]);
            let mut owed = 0.;
            // Two minutes of 10 ms input blocks, read by an output whose clock
            // runs 0.1 % fast or slow.
            for i in 0..12_000 {
                state.feed(&[&block], 441);
                owed += 480. * drift;
                let n = owed as usize;
                owed -= n as f64;
                let available = state.monitor.writer.queued();
                if i > 1000 {
                    assert!(available >= n, "ran dry at {i} ({drift})");
                    assert!(available < 3000, "overfilling at {i} ({drift})");
                }
                mixer.mix(&mut left[..n], &mut right[..n]);
            }
            control.reclaim();
        }
    }
    #[test]
    fn monitor_resamples_native_input_to_48k_and_respects_the_switch() {
        let (control, _mixer) = soundroom_engine::preview::channel();
        let enabled = Arc::new(AtomicBool::new(true));
        let writer = control.monitor_writer().unwrap();
        let mut state = MonitorState::new(
            Monitor {
                writer,
                enabled: enabled.clone(),
                routes: vec![MonitorRoute {
                    channels: vec![0],
                    gain: 1.,
                    pan: 0.,
                }],
            },
            44_100,
        );
        let block = vec![0.5f32; 441];
        for _ in 0..4 {
            state.feed(&[&block], 441);
        }
        let queued = state.monitor.writer.queued();
        // Nominal 1920, within the ±0.2 % drift steering.
        assert!((1914..=1926).contains(&queued), "{queued}");
        enabled.store(false, Ordering::Release);
        // Switching off fades out over the next 10 ms, then stops writing.
        state.feed(&[&block], 441);
        let faded = state.monitor.writer.queued();
        assert!(faded > queued);
        assert!(state.level == 0.);
        state.feed(&[&block], 441);
        assert_eq!(state.monitor.writer.queued(), faded);
    }
}
