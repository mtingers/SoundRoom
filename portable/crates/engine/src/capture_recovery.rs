//! Offline recording recovery. Raw files stay immutable under a service-owned
//! storage lease. Workers stream aligned WAV segments into an exclusive stage;
//! idempotent receipts never grant session edit or recording capability.
use crate::{
    capture_archive::{
        ArchiveEventRead, ArchiveManifest, ArchiveRead, ArchiveStats, CaptureArchiveReader,
        CaptureEvent, CaptureEventReader,
    },
    capture_clock::{CaptureAligner, ClockConfig, ClockObservation, OutputClockAnchor},
    capture_resample::{CaptureResampler, ResampleConfig},
    model::{Clip, Extra, Take, MAX_FRAME},
    wave::{WaveReader, WaveWriter},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use soundroom_platform::PrivateDirectory;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
const MAX_ASSETS: usize = 4096;
const MAX_ISSUES: usize = 8192;
const REPORT_LIMIT: u64 = 16 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryRange {
    pub start: u64,
    pub end: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryRoute {
    pub track_id: String,
    pub channels: Vec<usize>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryPlan {
    pub version: u32,
    pub name: String,
    pub routes: Vec<RecoveryRoute>,
    pub range: Option<RecoveryRange>,
    /// Unwrapped output clock is split into separate timeline takes per pass.
    pub loop_range: Option<RecoveryRange>,
    pub maximum_drift_ppm: f64,
    /// Explicit policy for interrupted/raw-error captures. Artifacts retain
    /// requires_review=true and must not silently become a complete session take.
    pub allow_incomplete: bool,
}
impl RecoveryPlan {
    fn validate(&self, manifest: &ArchiveManifest) -> Result<()> {
        if self.version != 1
            || self.name.is_empty()
            || self.name.len() > 256
            || self.name.contains('\0')
            || !(1..=128).contains(&self.routes.len())
            || !self.maximum_drift_ppm.is_finite()
            || !(10. ..=5000.).contains(&self.maximum_drift_ppm)
        {
            return Err(Error::Invalid("Invalid recording recovery plan".into()));
        }
        let mut ids = BTreeSet::new();
        for route in &self.routes {
            if !valid_id(&route.track_id)
                || !ids.insert(route.track_id.to_uppercase())
                || ![1, 2].contains(&route.channels.len())
                || route.channels.iter().any(|&c| c >= manifest.channels)
            {
                return Err(Error::Invalid(
                    "Invalid or duplicate recovery track/channel route".into(),
                ));
            }
        }
        for range in [self.range, self.loop_range].into_iter().flatten() {
            if range.start >= range.end || range.end > MAX_FRAME {
                return Err(Error::Invalid("Invalid recovery trim/loop range".into()));
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryIssue {
    #[serde(default, skip_serializing_if = "zero_count")]
    pub samples: u64,
    #[serde(default, skip_serializing_if = "zero_count")]
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<CaptureEvent>,
    pub kind: String,
    pub source_frame: Option<u64>,
    pub frames: u64,
    pub detail: String,
    pub requires_review: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveredAsset {
    pub id: String,
    pub clip_id: String,
    pub track_id: String,
    pub relative_path: String,
    pub sha256: String,
    pub frames: u64,
    pub timeline_start: u64,
    pub output_clock_start: u64,
    pub native_segment_start: u64,
    pub epoch: u64,
    pub loop_pass: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveredTrackTake {
    pub track_id: String,
    pub id: String,
    pub epoch: u64,
    pub loop_pass: u64,
    pub take: Take,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryReport {
    pub version: u32,
    pub receipt_id: String,
    pub raw_take_id: String,
    pub source_sha256: String,
    pub plan_sha256: String,
    pub requires_review: bool,
    pub retained_native_frames: u64,
    pub unaligned_native_frames: u64,
    pub trimmed_output_frames: u64,
    pub assets: Vec<RecoveredAsset>,
    pub takes: Vec<RecoveredTrackTake>,
    pub issues: Vec<RecoveryIssue>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum RecoveryOutcome {
    Committed(RecoveryReport),
    AlreadyPresent(RecoveryReport),
}
static STAGE_ID: AtomicU64 = AtomicU64::new(0);
struct Stage<'a> {
    parent: &'a PrivateDirectory,
    name: String,
    directory: Option<PrivateDirectory>,
    audio: Option<PrivateDirectory>,
    files: RefCell<Vec<String>>,
    published: bool,
}
impl<'a> Stage<'a> {
    fn new(parent: &'a PrivateDirectory, name: String) -> Result<Self> {
        let mut stage = Self {
            parent,
            directory: Some(parent.create_dir(&name)?),
            name,
            audio: None,
            files: RefCell::new(Vec::new()),
            published: false,
        };
        stage.audio = Some(stage.directory().create_dir("Audio")?);
        Ok(stage)
    }
    fn directory(&self) -> &PrivateDirectory {
        self.directory.as_ref().unwrap()
    }
    fn audio(&self) -> &PrivateDirectory {
        self.audio.as_ref().unwrap()
    }
    fn new_audio(&self, name: String) -> Result<File> {
        let file = self.audio().create_file(&name)?;
        self.files.borrow_mut().push(name);
        Ok(file)
    }
}
impl Drop for Stage<'_> {
    fn drop(&mut self) {
        if self.published {
            return;
        }
        let Some(directory) = &self.directory else {
            return;
        };
        // A failed sync after rename or an unexpected name replacement must
        // never make cleanup delete committed/foreign artifacts through a handle.
        if self
            .parent
            .open_dir(&self.name)
            .and_then(|d| d.identity())
            .ok()
            != directory.identity().ok()
        {
            return;
        }
        if let Some(audio) = self.audio.take() {
            for name in self.files.borrow().iter() {
                let _ = audio.remove_file(name);
            }
            let _ = directory.remove_dir("Audio", &audio);
            drop(audio);
        }
        for name in ["recovery.json", "recovery.sha256"] {
            let _ = directory.remove_file(name);
        }
        if let Some(directory) = self.directory.take() {
            let _ = self.parent.remove_dir(&self.name, &directory);
        }
    }
}
fn cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(Error::Conflict(
            "Recording recovery cancelled; raw capture retained".into(),
        ))
    } else {
        Ok(())
    }
}
fn valid_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn id(domain: &str, parts: &[&[u8]]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"soundroom.capture.recovery.v1\0");
    hash.update(domain.as_bytes());
    for part in parts {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part);
    }
    let digest = hash.finalize();
    let mut bytes: [u8; 16] = digest[..16].try_into().unwrap();
    bytes[6] = (bytes[6] & 15) | 128;
    bytes[8] = (bytes[8] & 63) | 128;
    let h = hex(&bytes).to_uppercase();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}
fn file_hash(mut file: File, cancel: &AtomicBool) -> Result<String> {
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 8192];
    loop {
        cancelled(cancel)?;
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    Ok(hex(&hash.finalize()))
}
fn source_hash(raw: &PrivateDirectory, cancel: &AtomicBool) -> Result<String> {
    raw.revalidate()?;
    let mut hash = Sha256::new();
    hash.update(b"soundroom.raw.capture.v1\0");
    for name in [
        "manifest.json",
        "native.packets",
        "events.jsonl",
        "completion.json",
    ] {
        hash.update(name.as_bytes());
        let file = match raw.read_file(name) {
            Ok(file) => file,
            Err(error)
                if name == "completion.json" && error.kind() == std::io::ErrorKind::NotFound =>
            {
                hash.update(b"missing");
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        let size = file.metadata()?.len();
        let limit = match name {
            "manifest.json" | "completion.json" => 16384,
            "events.jsonl" => 512 * 1024 * 1024,
            _ => 16 * 1024u64.pow(4),
        };
        if size > limit {
            return Err(Error::Invalid(
                "Raw recovery source size exceeds bound".into(),
            ));
        }
        hash.update(size.to_le_bytes());
        hash.update(file_hash(file, cancel)?.as_bytes());
    }
    Ok(hex(&hash.finalize()))
}
struct OpenAsset {
    metadata: RecoveredAsset,
    writer: WaveWriter,
}
struct Builder<'a> {
    stage: &'a Stage<'a>,
    plan: &'a RecoveryPlan,
    raw_id: &'a str,
    cancel: &'a AtomicBool,
    selected: Vec<usize>,
    maps: Vec<[usize; 2]>,
    open: Vec<OpenAsset>,
    assets: Vec<RecoveredAsset>,
    source_start: u64,
    epoch: u64,
    pass: Option<u64>,
    part: u64,
    trimmed: u64,
}
impl<'a> Builder<'a> {
    fn new(
        stage: &'a Stage<'a>,
        plan: &'a RecoveryPlan,
        raw_id: &'a str,
        cancel: &'a AtomicBool,
    ) -> Self {
        let selected: Vec<usize> = plan
            .routes
            .iter()
            .flat_map(|r| r.channels.iter().copied())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let maps = plan
            .routes
            .iter()
            .map(|r| {
                [
                    selected.iter().position(|&v| v == r.channels[0]).unwrap(),
                    selected
                        .iter()
                        .position(|&v| v == *r.channels.get(1).unwrap_or(&r.channels[0]))
                        .unwrap(),
                ]
            })
            .collect();
        Self {
            stage,
            plan,
            raw_id,
            cancel,
            selected,
            maps,
            open: Vec::new(),
            assets: Vec::new(),
            source_start: 0,
            epoch: 0,
            pass: None,
            part: 0,
            trimmed: 0,
        }
    }
    fn close(&mut self) -> Result<()> {
        for mut asset in self.open.drain(..) {
            cancelled(self.cancel)?;
            asset.metadata.frames = asset.writer.frames();
            asset.writer.finish()?;
            asset.metadata.sha256 = file_hash(
                self.stage
                    .audio()
                    .read_file(format!("{}.wav", asset.metadata.id))?,
                self.cancel,
            )?;
            self.assets.push(asset.metadata);
        }
        self.pass = None;
        self.part += 1;
        Ok(())
    }
    fn segment(&mut self, source: u64, epoch: u64) -> Result<()> {
        self.close()?;
        self.source_start = source;
        self.epoch = epoch;
        Ok(())
    }
    fn push(&mut self, start: i64, channels: &[Vec<f32>], frames: usize) -> Result<()> {
        let mut offset = 0;
        while offset < frames {
            cancelled(self.cancel)?;
            let signed = start + i64::try_from(offset).unwrap();
            if signed < 0 {
                let skip = (signed.unsigned_abs() as usize).min(frames - offset);
                self.trimmed += skip as u64;
                offset += skip;
                continue;
            }
            let clock = signed as u64;
            let (timeline, pass, boundary) =
                if let Some(range) = self.plan.loop_range.filter(|r| clock >= r.start) {
                    let length = range.end - range.start;
                    let within = (clock - range.start) % length;
                    (
                        range.start + within,
                        (clock - range.start) / length + 1,
                        length - within,
                    )
                } else {
                    (clock, 0, MAX_FRAME - clock)
                };
            let count = (frames - offset).min(boundary as usize);
            if count == 0 {
                return Err(Error::Invalid(
                    "Recovery output clock exceeds timeline bound".into(),
                ));
            }
            if self.pass != Some(pass) && !self.open.is_empty() {
                self.close()?;
            }
            let (skip, keep) = if let Some(range) = self.plan.range {
                let first = range.start.saturating_sub(timeline).min(count as u64) as usize;
                let last = range.end.saturating_sub(timeline).min(count as u64) as usize;
                (first, last.saturating_sub(first))
            } else {
                (0, count)
            };
            self.trimmed += (count - keep) as u64;
            if keep != 0 {
                if self.open.is_empty() {
                    if self.assets.len() + self.plan.routes.len() > MAX_ASSETS {
                        return Err(Error::Invalid("Recovered asset count exceeds 4096".into()));
                    }
                    for route in &self.plan.routes {
                        let track = route.track_id.to_uppercase();
                        let routes: Vec<u8> = route.channels.iter().map(|&v| v as u8).collect();
                        let uuid = id(
                            "asset",
                            &[
                                self.raw_id.as_bytes(),
                                track.as_bytes(),
                                &routes,
                                &self.source_start.to_le_bytes(),
                                &self.epoch.to_le_bytes(),
                                &pass.to_le_bytes(),
                                &self.part.to_le_bytes(),
                            ],
                        );
                        let relative = format!("Audio/{uuid}.wav");
                        let writer =
                            WaveWriter::stream_file(self.stage.new_audio(format!("{uuid}.wav"))?)?;
                        self.open.push(OpenAsset {
                            metadata: RecoveredAsset {
                                id: uuid.clone(),
                                clip_id: id("clip", &[uuid.as_bytes()]),
                                track_id: track,
                                relative_path: relative,
                                sha256: String::new(),
                                frames: 0,
                                timeline_start: timeline + skip as u64,
                                output_clock_start: clock + skip as u64,
                                native_segment_start: self.source_start,
                                epoch: self.epoch,
                                loop_pass: pass,
                            },
                            writer,
                        });
                    }
                    self.pass = Some(pass);
                }
                for (asset, map) in self.open.iter_mut().zip(&self.maps) {
                    if asset.metadata.timeline_start + asset.writer.frames()
                        != timeline + skip as u64
                    {
                        return Err(Error::Conflict(
                            "Recovered samples are not contiguous inside a declared segment".into(),
                        ));
                    }
                    asset.writer.write(
                        &channels[map[0]][offset + skip..offset + skip + keep],
                        &channels[map[1]][offset + skip..offset + skip + keep],
                    )?;
                }
            }
            offset += count;
        }
        Ok(())
    }
    fn drain(&mut self, converter: &mut CaptureResampler, output: &mut [Vec<f32>]) -> Result<()> {
        loop {
            cancelled(self.cancel)?;
            let mut refs: Vec<&mut [f32]> = output.iter_mut().map(|v| v.as_mut_slice()).collect();
            let block = converter.drain(&mut refs)?;
            if block.frames == 0 {
                return Ok(());
            }
            self.push(block.output_frame, output, block.frames)?;
        }
    }
}
fn issue(
    issues: &mut Vec<RecoveryIssue>,
    kind: &str,
    source: Option<u64>,
    frames: u64,
    detail: String,
    review: bool,
) -> Result<()> {
    if issues.len() >= MAX_ISSUES {
        return Err(Error::Invalid(
            "Recovery issue count exceeds 8192; raw archive retained".into(),
        ));
    }
    issues.push(RecoveryIssue {
        samples: if kind == "nonfiniteSamples" {
            frames
        } else {
            0
        },
        bytes: if ["packetTail", "eventTail"].contains(&kind) {
            frames
        } else {
            0
        },
        event: None,
        kind: kind.into(),
        source_frame: source,
        frames: if ["nonfiniteSamples", "packetTail", "eventTail"].contains(&kind) {
            0
        } else {
            frames
        },
        detail: detail.chars().take(1024).collect(),
        requires_review: review,
    });
    Ok(())
}
fn zero_count(count: &u64) -> bool {
    *count == 0
}
fn event_position(event: &CaptureEvent) -> u64 {
    match event {
        CaptureEvent::OutputAnchor { source_frame, .. } => source_frame.unwrap_or(0),
        CaptureEvent::QueueOverrun {
            received_frames, ..
        } => *received_frames,
        CaptureEvent::QueueGap { source_frame, .. }
        | CaptureEvent::DeviceGap { source_frame, .. }
        | CaptureEvent::WorkerOverrun { source_frame, .. }
        | CaptureEvent::TimestampOutlier { source_frame }
        | CaptureEvent::ClockReset { source_frame }
        | CaptureEvent::NonfiniteSamples { source_frame, .. }
        | CaptureEvent::DeviceFault { source_frame, .. }
        | CaptureEvent::NativeInputStopped { source_frame, .. } => *source_frame,
    }
}
fn read_event(
    reader: &mut CaptureEventReader,
    issues: &mut Vec<RecoveryIssue>,
) -> Result<Option<CaptureEvent>> {
    match reader.read_event() {
        Ok(ArchiveEventRead::Event(event)) => Ok(Some(event)),
        Ok(ArchiveEventRead::End) => Ok(None),
        Ok(ArchiveEventRead::TruncatedTail { bytes }) => {
            issue(
                issues,
                "eventTail",
                None,
                bytes as u64,
                format!("Interrupted journal: {bytes} bytes"),
                true,
            )?;
            Ok(None)
        }
        Err(error) => {
            issue(issues, "corruptJournal", None, 0, error.to_string(), true)?;
            Ok(None)
        }
    }
}
fn capture_health(issues: &mut Vec<RecoveryIssue>, event: CaptureEvent) -> Result<()> {
    let (kind, frames) = match &event {
        CaptureEvent::NativeInputStopped {
            had_started,
            accepted_frames,
            timestamped_frames,
            rejected_callbacks,
            rejected_frames,
            stop_failures,
            ..
        } => {
            if *had_started
                && accepted_frames == timestamped_frames
                && *rejected_callbacks == 0
                && *rejected_frames == 0
                && *stop_failures == 0
            {
                return Ok(());
            }
            ("nativeInputHealth", *rejected_frames)
        }
        CaptureEvent::QueueOverrun { dropped_frames, .. } => ("queueOverrun", *dropped_frames),
        CaptureEvent::WorkerOverrun { frames, .. } => ("workerOverrun", *frames as u64),
        CaptureEvent::DeviceFault { .. } => ("deviceFault", 0),
        CaptureEvent::ClockReset { .. } => ("recordingClockReset", 0),
        _ => ("captureHealth", 0),
    };
    issue(
        issues,
        kind,
        Some(event_position(&event)),
        frames,
        serde_json::to_string(&event)?,
        true,
    )?;
    issues.last_mut().unwrap().event = Some(event);
    Ok(())
}
fn make_aligner(manifest: &ArchiveManifest, plan: &RecoveryPlan) -> Result<CaptureAligner> {
    let config = |rate| ClockConfig {
        sample_rate: rate,
        host_ticks_per_second: manifest.host_ticks_per_second,
        maximum_drift_ppm: plan.maximum_drift_ppm,
    };
    CaptureAligner::new(
        config(manifest.sample_rate),
        config(48000),
        manifest.calibration_offset_frames,
    )
}
fn takes(assets: &[RecoveredAsset], raw_id: &str, name: &str) -> Vec<RecoveredTrackTake> {
    let mut groups: BTreeMap<(String, u64, u64), Vec<Clip>> = BTreeMap::new();
    for asset in assets {
        groups
            .entry((asset.track_id.clone(), asset.epoch, asset.loop_pass))
            .or_default()
            .push(Clip {
                id: asset.clip_id.clone(),
                asset_id: asset.id.clone(),
                name: name.into(),
                start: asset.timeline_start,
                offset: 0,
                length: asset.frames,
                edge_fade_frames: None,
                edge_protection_disabled: None,
                cut_fade_in_frames: None,
                cut_fade_out_frames: None,
                fade_in_frames: None,
                fade_out_frames: None,
                fade_reference_offset: None,
                fade_reference_length: None,
                render_envelope: None,
                previous_states: Vec::new(),
                extra: Extra::new(),
            });
    }
    groups
        .into_iter()
        .map(|((track, epoch, pass), clips)| {
            let uuid = id(
                "take",
                &[
                    raw_id.as_bytes(),
                    track.as_bytes(),
                    &epoch.to_le_bytes(),
                    &pass.to_le_bytes(),
                ],
            );
            let mut extra = Extra::new();
            extra.insert("id".into(), uuid.clone().into());
            extra.insert("name".into(), name.into());
            extra.insert("rawCaptureID".into(), raw_id.into());
            RecoveredTrackTake {
                track_id: track,
                id: uuid,
                epoch,
                loop_pass: pass,
                take: Take { clips, extra },
            }
        })
        .collect()
}

fn verify_existing(
    target: &PrivateDirectory,
    source: &str,
    plan_hash: &str,
    plan: &RecoveryPlan,
    cancel: &AtomicBool,
) -> Result<RecoveryReport> {
    target.revalidate()?;
    let mut checksum = target.read_file("recovery.sha256")?;
    if checksum.metadata()?.len() != 64 {
        return Err(Error::Invalid("Invalid recovery receipt checksum".into()));
    }
    let mut expected = String::new();
    checksum.read_to_string(&mut expected)?;
    if expected != file_hash(target.read_file("recovery.json")?, cancel)? {
        return Err(Error::Conflict(
            "Committed recovery receipt was changed or corrupted".into(),
        ));
    }
    let file = target.read_file("recovery.json")?;
    if file.metadata()?.len() > REPORT_LIMIT {
        return Err(Error::Invalid("Recovery receipt exceeds bound".into()));
    }
    let mut bytes = Vec::new();
    file.take(REPORT_LIMIT + 1).read_to_end(&mut bytes)?;
    let report: RecoveryReport = serde_json::from_slice(&bytes)?;
    if report.version != 1 || report.source_sha256 != source || report.plan_sha256 != plan_hash {
        return Err(Error::Conflict(
            "Recovery identity reused with different raw content or settings".into(),
        ));
    }
    if !valid_id(&report.raw_take_id)
        || !valid_id(&report.receipt_id)
        || report.assets.len() > MAX_ASSETS
        || report.issues.len() > MAX_ISSUES
        || report.takes != takes(&report.assets, &report.raw_take_id, &plan.name)
    {
        return Err(Error::Invalid("Invalid recovered artifact receipt".into()));
    }
    let routes: BTreeSet<String> = plan
        .routes
        .iter()
        .map(|r| r.track_id.to_uppercase())
        .collect();
    let mut ids = BTreeSet::new();
    let audio = target.open_dir("Audio")?;
    for asset in &report.assets {
        cancelled(cancel)?;
        if !valid_id(&asset.id)
            || !valid_id(&asset.clip_id)
            || !ids.insert(&asset.id)
            || !routes.contains(&asset.track_id)
            || asset.relative_path != format!("Audio/{}.wav", asset.id)
            || asset.frames == 0
            || asset
                .timeline_start
                .checked_add(asset.frames)
                .map_or(true, |v| v > MAX_FRAME)
            || asset
                .output_clock_start
                .checked_add(asset.frames)
                .map_or(true, |v| v > MAX_FRAME)
            || asset.native_segment_start > MAX_FRAME
            || asset.epoch == 0
        {
            return Err(Error::Invalid(
                "Invalid recovered asset identity/path/range".into(),
            ));
        }
        let name = format!("{}.wav", asset.id);
        let expected = file_hash(audio.read_file(&name)?, cancel)?;
        let wave = WaveReader::from_file(audio.read_file(name)?)?;
        if expected != asset.sha256
            || wave.info.frames != asset.frames
            || wave.info.channels != 2
            || wave.info.sample_rate != 48000
        {
            return Err(Error::Conflict(
                "Committed recovered audio was changed or corrupted".into(),
            ));
        }
    }
    Ok(report)
}
/// Recover immutable raw data into a new private, server-chosen destination.
/// The service must hold exclusive storage/recording leases, admit global memory,
/// CPU/disk and validate authored track revisions before committing these takes.
/// An existing identical validated destination returns AlreadyPresent; changed
/// raw/settings/artifacts reject. No session mutation or device operation occurs.
pub fn recover_take(
    raw: &Path,
    target: &Path,
    plan: &RecoveryPlan,
    cancel: &AtomicBool,
) -> Result<RecoveryOutcome> {
    cancelled(cancel)?;
    let raw = PrivateDirectory::open_root(raw)?;
    let parent = PrivateDirectory::open_root(target.parent().ok_or_else(|| {
        Error::Invalid("Recovery target requires an absolute private parent".into())
    })?)?;
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| Error::Invalid("Invalid recovery target name".into()))?;
    recover_take_in(&raw, &parent, name, plan, cancel)
}

/// All I/O is rooted in retained, verified directories. Caller holds the raw
/// immutable lease and serialized authored storage admission; no client path is
/// accepted. Windows uses explicit protected ACL creation for every descendant.
pub fn recover_take_in(
    raw: &PrivateDirectory,
    parent: &PrivateDirectory,
    target: &str,
    plan: &RecoveryPlan,
    cancel: &AtomicBool,
) -> Result<RecoveryOutcome> {
    cancelled(cancel)?;
    let source = source_hash(raw, cancel)?;
    let mut reader = CaptureArchiveReader::from_directory(raw)?;
    let manifest = reader.manifest.clone();
    plan.validate(&manifest)?;
    let plan_hash = hex(&Sha256::digest(serde_json::to_vec(plan)?));
    match parent.open_dir(target) {
        Ok(existing) => {
            let report = verify_existing(&existing, &source, &plan_hash, plan, cancel)?;
            parent.sync()?;
            return Ok(RecoveryOutcome::AlreadyPresent(report));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| Error::Invalid("Cannot name recovery stage before epoch".into()))?
        .as_nanos();
    let mut stage = Stage::new(
        parent,
        format!(
            ".soundroom-recovery-{}-{}-{}",
            std::process::id(),
            STAGE_ID.fetch_add(1, Ordering::Relaxed),
            unique
        ),
    )?;
    let raw_id = manifest.take_id.to_uppercase();
    let mut builder = Builder::new(&stage, plan, &raw_id, cancel);
    let mut issues = Vec::new();
    let mut events = CaptureEventReader::from_directory(raw)?;
    let mut pending = read_event(&mut events, &mut issues)?;
    let mut aligner = make_aligner(&manifest, plan)?;
    let mut last_anchor: Option<OutputClockAnchor> = None;
    let mut converter: Option<CaptureResampler> = None;
    let mut epoch_changed = false;
    let mut native = vec![vec![0.; manifest.maximum_frames]; manifest.channels];
    let mut output = vec![vec![0.; 8192]; builder.selected.len()];
    let mut packets = 0u64;
    let mut retained = 0u64;
    let mut unaligned = 0u64;
    let mut last_native = 0u64;
    loop {
        cancelled(cancel)?;
        let block = {
            let mut refs: Vec<&mut [f32]> = native.iter_mut().map(|v| v.as_mut_slice()).collect();
            reader.read_planar(&mut refs)
        };
        let block = match block {
            Ok(ArchiveRead::Packet(packet)) => packet,
            Ok(ArchiveRead::End) => break,
            Ok(ArchiveRead::TruncatedTail { bytes }) => {
                issue(
                    &mut issues,
                    "packetTail",
                    Some(last_native),
                    bytes,
                    format!("Interrupted native packet: {bytes} bytes"),
                    true,
                )?;
                break;
            }
            Err(error) => {
                issue(
                    &mut issues,
                    "corruptPacket",
                    Some(last_native),
                    0,
                    error.to_string(),
                    true,
                )?;
                break;
            }
        };
        let previous_native = last_native;
        packets += 1;
        retained += block.frames as u64;
        last_native = block.source_frame + block.frames as u64;
        while pending.as_ref().is_some_and(|event| {
            !matches!(event, CaptureEvent::OutputAnchor { .. })
                || event_position(event) <= block.source_frame
        }) {
            let event = pending.take().unwrap();
            match event {
                CaptureEvent::OutputAnchor {
                    source_frame: Some(_), epoch, playback_clock, frames,
                    sample_time_bits, host_time, flags,
                } => {
                    let anchor = OutputClockAnchor {
                        epoch, playback_clock, frames,
                        timing: crate::capture::CaptureTiming {
                            sample_time: f64::from_bits(sample_time_bits), host_time, flags,
                        },
                    };
                    if last_anchor.is_some_and(|old| old.epoch != epoch) {
                        aligner = make_aligner(&manifest, plan)?;
                        epoch_changed = true;
                        issue(&mut issues, "outputEpoch", Some(block.source_frame), 0,
                            "Output seek/pause epoch starts a separate take".into(), true)?;
                    }
                    match aligner.observe_output(anchor) {
                        Ok(ClockObservation::Accepted) => last_anchor = Some(anchor),
                        Ok(ClockObservation::TimestampOutlier) => issue(
                            &mut issues, "outputTimestampOutlier", Some(block.source_frame), 0,
                            "Rejected output timestamp jitter".into(), false,
                        )?,
                        Err(error) => {
                            issue(&mut issues, "outputClock", Some(block.source_frame), 0,
                                error.to_string(), true)?;
                            aligner = make_aligner(&manifest, plan)?;
                            last_anchor = None;
                            epoch_changed = true;
                        }
                    }
                }
                CaptureEvent::OutputAnchor { source_frame: None, .. } => issue(
                    &mut issues, "legacyUnsequencedClock", Some(block.source_frame), 0,
                    "Output anchor has no native worker sequence; retain raw for explicit alignment".into(), true,
                )?,
                CaptureEvent::NonfiniteSamples { .. }
                | CaptureEvent::TimestampOutlier { .. }
                | CaptureEvent::QueueGap { .. }
                | CaptureEvent::DeviceGap { .. } => {}
                other => capture_health(&mut issues, other)?,
            }

            pending = read_event(&mut events, &mut issues)?;
        }
        let invalid = native
            .iter()
            .map(|c| c[..block.frames].iter().filter(|v| !v.is_finite()).count() as u64)
            .sum::<u64>();
        if invalid != 0 {
            issue(
                &mut issues,
                "nonfiniteSamples",
                Some(block.source_frame),
                invalid,
                "Raw bits retained; aligned nonfinite samples replaced with zero".into(),
                true,
            )?;
        }
        let mut alignment = aligner.align_packet(block);
        if let (Err(error), Some(anchor)) = (&alignment, last_anchor) {
            // A native sample clock reset can be remapped from actual retained
            // host time, never from elapsed wall time or a guessed packet count.
            let error = error.to_string();
            aligner = make_aligner(&manifest, plan)?;
            aligner.observe_output(anchor)?;
            alignment = aligner.align_packet(block);
            epoch_changed = true;
            issue(
                &mut issues,
                "inputClockSegment",
                Some(block.source_frame),
                0,
                error,
                true,
            )?;
        }
        let mut alignment = match alignment {
            Ok(value) => value,
            Err(error) => {
                if let Some(mut old) = converter.take() {
                    old.finish();
                    builder.drain(&mut old, &mut output)?;
                    builder.close()?;
                }
                unaligned += block.frames as u64;
                issue(
                    &mut issues,
                    "unalignedPacket",
                    Some(block.source_frame),
                    block.frames as u64,
                    error.to_string(),
                    true,
                )?;
                continue;
            }
        };
        alignment.queue_gap = block.source_frame - previous_native;
        if alignment.queue_gap != 0 || alignment.device_gap != 0 || epoch_changed {
            if let Some(mut old) = converter.take() {
                old.finish();
                builder.drain(&mut old, &mut output)?;
            }
            builder.close()?;
            if alignment.queue_gap != 0 {
                issue(
                    &mut issues,
                    "queueGap",
                    Some(block.source_frame),
                    alignment.queue_gap,
                    "Missing native queue frames; separate clips, no synthetic audio".into(),
                    true,
                )?;
            }
            if alignment.device_gap != 0 {
                issue(
                    &mut issues,
                    "deviceGap",
                    Some(block.source_frame),
                    alignment.device_gap,
                    "Missing device sample-clock frames; separate clips".into(),
                    true,
                )?;
            }
        }
        if converter.is_none() {
            builder.segment(block.source_frame, last_anchor.unwrap().epoch)?;
            converter = Some(CaptureResampler::new(
                ResampleConfig {
                    channels: builder.selected.len(),
                    input_rate: manifest.sample_rate,
                    output_rate: 48000,
                    maximum_drift_ppm: plan.maximum_drift_ppm,
                },
                block.source_frame,
                alignment.output_start,
            )?);
        }
        epoch_changed = false;
        let mut continuous = alignment;
        continuous.queue_gap = 0;
        continuous.device_gap = 0;
        let current = converter.as_mut().unwrap();
        let selected: Vec<&[f32]> = builder
            .selected
            .iter()
            .map(|&i| &native[i][..block.frames])
            .collect();
        if let Err(error) = current
            .follow_packet(block.source_frame, continuous)
            .and_then(|_| current.push(block.source_frame, &selected))
        {
            if let Some(mut old) = converter.take() {
                old.finish();
                builder.drain(&mut old, &mut output)?;
                builder.close()?;
            }
            unaligned += block.frames as u64;
            issue(
                &mut issues,
                "conversionSegment",
                Some(block.source_frame),
                block.frames as u64,
                error.to_string(),
                true,
            )?;
        } else {
            builder.drain(current, &mut output)?;
        }
    }
    if let Some(mut current) = converter.take() {
        current.finish();
        builder.drain(&mut current, &mut output)?;
    }
    builder.close()?;
    while let Some(event) = pending.take() {
        if !matches!(
            event,
            CaptureEvent::OutputAnchor { .. }
                | CaptureEvent::NonfiniteSamples { .. }
                | CaptureEvent::TimestampOutlier { .. }
                | CaptureEvent::QueueGap { .. }
                | CaptureEvent::DeviceGap { .. }
        ) {
            capture_health(&mut issues, event)?;
        }
        pending = read_event(&mut events, &mut issues)?;
    }
    let complete = raw
        .read_file("completion.json")
        .map_err(Error::from)
        .and_then(|f| {
            let mut bytes = Vec::new();
            f.take(16385).read_to_end(&mut bytes)?;
            Ok(serde_json::from_slice::<ArchiveStats>(&bytes)?)
        });
    match complete {
        Ok(stats) if stats.packets == packets && stats.retained_frames == retained => {}
        Ok(_) => issue(
            &mut issues,
            "completionMismatch",
            None,
            0,
            "Completion receipt does not match recovered native prefix".into(),
            true,
        )?,
        Err(_) => issue(
            &mut issues,
            "interruptedCapture",
            None,
            0,
            "Raw capture has no valid completed-write receipt".into(),
            true,
        )?,
    }
    if builder.assets.is_empty() {
        issue(
            &mut issues,
            "noAlignedAudio",
            None,
            0,
            "No received samples could be placed inside the requested timeline region".into(),
            true,
        )?;
    }
    let review = issues.iter().any(|v| v.requires_review);
    if review && !plan.allow_incomplete {
        return Err(Error::Conflict(
            "Capture requires review; incomplete recovery was not authorized".into(),
        ));
    }
    if source_hash(raw, cancel)? != source {
        return Err(Error::Conflict(
            "Raw capture changed during recovery; retry after obtaining an immutable storage lease"
                .into(),
        ));
    }
    let assets = std::mem::take(&mut builder.assets);
    let trimmed = builder.trimmed;
    drop(builder);
    let raw_id = manifest.take_id.to_uppercase();
    let report = RecoveryReport {
        version: 1,
        receipt_id: id("receipt", &[raw_id.as_bytes(), plan_hash.as_bytes()]),
        raw_take_id: raw_id.clone(),
        source_sha256: source,
        plan_sha256: plan_hash,
        requires_review: review,
        retained_native_frames: retained,
        unaligned_native_frames: unaligned,
        trimmed_output_frames: trimmed,
        takes: takes(&assets, &raw_id, &plan.name),
        assets,
        issues,
    };
    let bytes = serde_json::to_vec(&report)?;
    if bytes.len() as u64 > REPORT_LIMIT {
        return Err(Error::Invalid(
            "Recovery receipt exceeds metadata bound".into(),
        ));
    }
    let mut receipt = stage.directory().create_file("recovery.json")?;
    receipt.write_all(&bytes)?;
    receipt.sync_all()?;
    let mut digest = stage.directory().create_file("recovery.sha256")?;
    digest.write_all(hex(&Sha256::digest(&bytes)).as_bytes())?;
    digest.sync_all()?;
    drop(receipt);
    drop(digest);
    stage.audio().sync()?;
    stage.directory().sync()?;
    cancelled(cancel)?;
    match parent.publish(&stage.name, stage.directory(), target) {
        Ok(()) => {
            stage.published = true;
            Ok(RecoveryOutcome::Committed(report))
        }
        Err(error) => {
            if let Ok(existing) = parent.open_dir(target) {
                let report = verify_existing(
                    &existing,
                    &report.source_sha256,
                    &report.plan_sha256,
                    plan,
                    cancel,
                )?;
                parent.sync()?;
                Ok(RecoveryOutcome::AlreadyPresent(report))
            } else {
                Err(error.into())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryAcceptance {
    CompleteOnly,
    /// Server-selected after review; never trust an unauthenticated wire flag.
    ReviewedIncomplete,
}
#[derive(Clone, Debug)]
pub struct PreparedTakeImport {
    /// Only tracks that need additional takes. Existing active clips/mixer,
    /// automation/effects/history are preserved. No authority revision is changed.
    pub tracks: Vec<crate::model::Track>,
    pub asset_ids: BTreeSet<String>,
    pub already_present: Vec<String>,
}
/// Pure resource-scoped preparation for a server-authored take transaction.
/// Caller must validate private artifact custody/digests, asset registration,
/// role, current expected track revisions and recording state before committing.
pub fn prepare_take_import(
    report: &RecoveryReport,
    session: &crate::model::Session,
    acceptance: RecoveryAcceptance,
) -> Result<PreparedTakeImport> {
    if report.version != 1
        || !valid_id(&report.raw_take_id)
        || !valid_id(&report.receipt_id)
        || report.takes.is_empty()
        || report.takes.len() > MAX_ASSETS
        || report.assets.len() > MAX_ASSETS
    {
        return Err(Error::Invalid(
            "Invalid or empty recovered take import".into(),
        ));
    }
    if report.requires_review && acceptance != RecoveryAcceptance::ReviewedIncomplete {
        return Err(Error::Conflict(
            "Incomplete recovered capture requires server-selected review acceptance".into(),
        ));
    }
    let mut assets = BTreeMap::new();
    for asset in &report.assets {
        if !valid_id(&asset.id)
            || asset.frames == 0
            || asset.frames > MAX_FRAME
            || asset.relative_path != format!("Audio/{}.wav", asset.id)
            || asset.sha256.len() != 64
            || !asset.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || assets.insert(asset.id.to_uppercase(), asset).is_some()
        {
            return Err(Error::Invalid("Invalid recovered asset import".into()));
        }
    }
    let mut tracks: BTreeMap<String, crate::model::Track> = BTreeMap::new();
    let mut take_ids = BTreeSet::new();
    let mut needed = BTreeSet::new();
    let mut existing = Vec::new();
    for recovered in &report.takes {
        let track_id = recovered.track_id.to_uppercase();
        if !valid_id(&track_id)
            || !valid_id(&recovered.id)
            || !take_ids.insert(recovered.id.to_uppercase())
            || recovered.take.extra.get("id").and_then(|v| v.as_str())
                != Some(recovered.id.as_str())
            || recovered
                .take
                .extra
                .get("rawCaptureID")
                .and_then(|v| v.as_str())
                != Some(report.raw_take_id.as_str())
            || recovered.take.clips.is_empty()
        {
            return Err(Error::Invalid(
                "Invalid recovered take identity/context".into(),
            ));
        }
        let current = session
            .tracks
            .iter()
            .find(|track| track.id.eq_ignore_ascii_case(&track_id))
            .ok_or_else(|| Error::Conflict("Recovery target track was removed".into()))?;
        let target = tracks
            .entry(track_id.clone())
            .or_insert_with(|| current.clone());
        for clip in &recovered.take.clips {
            let asset = assets.get(&clip.asset_id.to_uppercase()).ok_or_else(|| {
                Error::Invalid("Recovered clip refers to an unregistered artifact".into())
            })?;
            if !asset.track_id.eq_ignore_ascii_case(&track_id)
                || clip.id != asset.clip_id
                || clip.offset != 0
                || clip.start != asset.timeline_start
                || clip.length != asset.frames
            {
                return Err(Error::Invalid(
                    "Recovered clip does not match its verified audio artifact".into(),
                ));
            }
        }
        let matches: Vec<_> = target
            .recording_takes
            .iter()
            .filter(|take| {
                take.extra
                    .get("id")
                    .and_then(|v| v.as_str())
                    .is_some_and(|id| id.eq_ignore_ascii_case(&recovered.id))
            })
            .collect();
        match matches.as_slice() {
            [] => {
                for clip in &recovered.take.clips {
                    needed.insert(clip.asset_id.to_uppercase());
                }
                target.recording_takes.push(recovered.take.clone());
            }
            [take] if **take == recovered.take => existing.push(recovered.id.clone()),
            _ => {
                return Err(Error::Conflict(
                    "Recovered take identity already exists with different content".into(),
                ))
            }
        }
    }
    let mut changed = Vec::new();
    for (id, track) in tracks {
        let original = session
            .tracks
            .iter()
            .find(|t| t.id.eq_ignore_ascii_case(&id))
            .unwrap();
        if track.recording_takes != original.recording_takes {
            let isolated = crate::model::Session {
                tracks: vec![{
                    let mut validation = track.clone();
                    validation.routing = None;
                    validation
                }],
                ..crate::model::Session::default()
            };
            isolated.validate()?;
            changed.push(track);
        }
    }
    Ok(PreparedTakeImport {
        tracks: changed,
        asset_ids: needed,
        already_present: existing,
    })
}

#[cfg(test)]
mod native_health_tests {
    use super::*;
    #[test]
    fn native_loss_cannot_hide_behind_a_completed_raw_packet_archive() {
        for (started, timestamped, rejected, stops) in [
            (true, 64, 0, 0),
            (true, 64, 17, 0),
            (true, 0, 0, 0),
            (true, 64, 0, 1),
            (false, 64, 0, 0),
        ] {
            let event = CaptureEvent::NativeInputStopped {
                source_frame: 64,
                had_started: started,
                callback_count: 1,
                accepted_frames: 64,
                timestamped_frames: timestamped,
                rejected_callbacks: u64::from(rejected != 0),
                rejected_frames: rejected,
                stop_failures: stops,
                last_stop_status: if stops != 0 { -10851 } else { 0 },
            };
            let mut issues = vec![];
            capture_health(&mut issues, event.clone()).unwrap();
            if started && timestamped == 64 && rejected == 0 && stops == 0 {
                assert!(issues.is_empty());
            } else {
                assert_eq!(issues.len(), 1);
                assert_eq!(issues[0].kind, "nativeInputHealth");
                assert!(issues[0].requires_review);
                assert_eq!(issues[0].event, Some(event));
            }
        }
    }
}
