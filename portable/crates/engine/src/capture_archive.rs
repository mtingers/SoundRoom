//! Recoverable native packet archive. Worker-owned files preserve every Float32
//! bit and timestamp before routing or conversion. Complete CRC-checked packets
//! survive a partial final write; recovery never treats that tail as audio.
use crate::{
    capture::{CaptureConfig, CaptureTiming, CapturedBlock},
    model::MAX_FRAME,
    Error, Result,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Write},
    path::Path,
};
const HEADER: usize = 32;
const PACKET: usize = 48;
const MAX_EVENTS: u64 = 1_000_000;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArchiveManifest {
    pub version: u32,
    /// Server-assigned immutable identity; recovery must use idempotent take IDs.
    pub take_id: String,
    pub device_uid: String,
    pub channels: usize,
    pub sample_rate: u32,
    pub maximum_frames: usize,
    pub host_ticks_per_second: f64,
    pub calibration_offset_frames: i64,
}
impl ArchiveManifest {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1
            || self.take_id.len() != 36
            || !self.take_id.bytes().enumerate().all(|(i, b)| {
                if [8, 13, 18, 23].contains(&i) {
                    b == b'-'
                } else {
                    b.is_ascii_hexdigit()
                }
            })
            || self.device_uid.is_empty()
            || self.device_uid.len() > 4096
            || self.device_uid.contains('\0')
            || !(1..=64).contains(&self.channels)
            || !(8000..=384000).contains(&self.sample_rate)
            || !(1..=8192).contains(&self.maximum_frames)
            || !self.host_ticks_per_second.is_finite()
            || !(1000. ..=1e12).contains(&self.host_ticks_per_second)
            || self.calibration_offset_frames.unsigned_abs() > 48000 * 30
        {
            return Err(Error::Invalid("Invalid native archive manifest".into()));
        }
        Ok(())
    }
    pub fn capture_config(&self, blocks: usize) -> CaptureConfig {
        CaptureConfig {
            channels: self.channels,
            maximum_frames: self.maximum_frames,
            sample_rate: self.sample_rate,
            blocks,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum CaptureEvent {
    QueueOverrun {
        received_frames: u64,
        dropped_frames: u64,
    },
    WorkerOverrun {
        source_frame: u64,
        frames: usize,
    },
    OutputAnchor {
        /// Worker-consumed native exclusive end at observation. Older archives
        /// without this sequence require explicit legacy alignment review.
        #[serde(default)]
        source_frame: Option<u64>,
        epoch: u64,
        playback_clock: u64,
        frames: usize,
        sample_time_bits: u64,
        host_time: u64,
        flags: u32,
    },
    QueueGap {
        source_frame: u64,
        frames: u64,
    },
    DeviceGap {
        source_frame: u64,
        frames: u64,
    },
    TimestampOutlier {
        source_frame: u64,
    },
    ClockReset {
        source_frame: u64,
    },
    NonfiniteSamples {
        source_frame: u64,
        samples: u64,
    },
    DeviceFault {
        source_frame: u64,
        status: i32,
    },
    /// Final aggregate native health. source_frame is the worker observation
    /// boundary, not the placement of rejected callbacks in the PCM timeline.
    NativeInputStopped {
        source_frame: u64,
        had_started: bool,
        callback_count: u64,
        accepted_frames: u64,
        timestamped_frames: u64,
        rejected_callbacks: u64,
        rejected_frames: u64,
        stop_failures: u64,
        last_stop_status: i32,
    },
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArchiveStats {
    pub packets: u64,
    pub retained_frames: u64,
    pub queue_gap_frames: u64,
    pub invalid_samples: u64,
    pub events: u64,
}
pub struct CaptureArchive {
    directory: soundroom_platform::PrivateDirectory,
    manifest: ArchiveManifest,
    metadata: File,
    hashes: [Sha256; 4],
    file: File,
    events: File,
    completion: File,
    bytes: Vec<u8>,
    next: u64,
    stats: ArchiveStats,
    failed: bool,
}
/// Immutable seal of a worker-written entry. Service custody must compare both
/// held identity/length and its independently verified content digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureArtifactSeal {
    pub name: &'static str,
    pub identity: soundroom_platform::FileIdentity,
    pub bytes: u64,
    pub sha256: [u8; 32],
}
pub(crate) struct FinishedCaptureArchive {
    pub directory: soundroom_platform::PrivateDirectory,
    pub stats: ArchiveStats,
    pub artifacts: [CaptureArtifactSeal; 4],
}
impl CaptureArchive {
    /// Destination must not exist. Service chooses a private storage-root path;
    /// no client path is accepted here and raw data is never automatically deleted.
    pub fn create(path: &Path, manifest: ArchiveManifest) -> Result<Self> {
        manifest.validate()?;
        Self::create_directory(
            soundroom_platform::PrivateDirectory::create_root(path)?,
            manifest,
        )
    }
    /// Service-owned root capability and one server-chosen child name.
    pub fn create_in(
        parent: &soundroom_platform::PrivateDirectory,
        name: &str,
        manifest: ArchiveManifest,
    ) -> Result<Self> {
        manifest.validate()?;
        Self::create_directory(parent.create_dir(name)?, manifest)
    }
    fn create_directory(
        directory: soundroom_platform::PrivateDirectory,
        manifest: ArchiveManifest,
    ) -> Result<Self> {
        let mut metadata = directory.create_file("manifest.json")?;
        let manifest_bytes = serde_json::to_vec(&manifest)?;
        metadata.write_all(&manifest_bytes)?;
        metadata.sync_all()?;
        let mut file = directory.create_file("native.packets")?;
        let mut header = [0u8; HEADER];
        header[..8].copy_from_slice(b"SRCPACK1");
        header[8..12].copy_from_slice(&(manifest.channels as u32).to_le_bytes());
        header[12..16].copy_from_slice(&manifest.sample_rate.to_le_bytes());
        header[16..20].copy_from_slice(&(manifest.maximum_frames as u32).to_le_bytes());
        let crc = checksum(&header[..28]);
        header[28..].copy_from_slice(&crc.to_le_bytes());
        file.write_all(&header)?;
        file.sync_all()?;
        let mut hashes: [Sha256; 4] = std::array::from_fn(|_| Sha256::new());
        hashes[0].update(&manifest_bytes);
        hashes[1].update(header);
        let events = directory.create_file("events.jsonl")?;
        let completion = directory.create_file("completion.json")?;
        directory.sync()?;
        let capacity = PACKET + manifest.channels * manifest.maximum_frames * 4;
        Ok(Self {
            directory,
            manifest,
            metadata,
            hashes,
            file,
            events,
            completion,
            bytes: vec![0; capacity],
            next: 0,
            stats: ArchiveStats::default(),
            failed: false,
        })
    }
    pub fn manifest(&self) -> &ArchiveManifest {
        &self.manifest
    }
    pub fn stats(&self) -> ArchiveStats {
        self.stats
    }
    pub fn append(&mut self, block: CapturedBlock, channels: &[&[f32]]) -> Result<()> {
        if self.failed {
            return Err(Error::Conflict(
                "Native archive has a failed partial write; retain for recovery".into(),
            ));
        }
        if channels.len() != self.manifest.channels
            || !(1..=self.manifest.maximum_frames).contains(&block.frames)
            || channels.iter().any(|c| c.len() != block.frames)
            || block.source_frame < self.next
            || block
                .source_frame
                .checked_add(block.frames as u64)
                .map_or(true, |n| n > MAX_FRAME)
        {
            return Err(Error::Invalid(
                "Invalid or overlapping raw archive packet".into(),
            ));
        }
        let size = PACKET + channels.len() * block.frames * 4;
        let bytes = &mut self.bytes[..size];
        bytes[..PACKET].fill(0);
        bytes[..4].copy_from_slice(b"PACK");
        bytes[4..8].copy_from_slice(&(block.frames as u32).to_le_bytes());
        bytes[8..16].copy_from_slice(&block.source_frame.to_le_bytes());
        bytes[16..24].copy_from_slice(&block.timing.sample_time.to_bits().to_le_bytes());
        bytes[24..32].copy_from_slice(&block.timing.host_time.to_le_bytes());
        bytes[32..36].copy_from_slice(&block.timing.flags.to_le_bytes());
        let mut invalid = 0;
        for (channel, audio) in channels.iter().enumerate() {
            for (frame, &sample) in audio.iter().enumerate() {
                let offset = PACKET + (channel * block.frames + frame) * 4;
                bytes[offset..offset + 4].copy_from_slice(&sample.to_bits().to_le_bytes());
                invalid += u64::from(!sample.is_finite());
            }
        }
        let crc = checksum(&bytes[PACKET..]);
        bytes[36..40].copy_from_slice(&crc.to_le_bytes());
        let crc = checksum(&bytes[..40]);
        bytes[40..44].copy_from_slice(&crc.to_le_bytes());
        if let Err(error) = self.file.write_all(bytes) {
            self.failed = true;
            return Err(error.into());
        }
        self.hashes[1].update(bytes);
        self.stats.packets += 1;
        self.stats.retained_frames += block.frames as u64;
        self.stats.invalid_samples += invalid;
        if block.source_frame > self.next {
            let gap = block.source_frame - self.next;
            self.stats.queue_gap_frames += gap;
            self.event(CaptureEvent::QueueGap {
                source_frame: self.next,
                frames: gap,
            })?;
        }
        if invalid != 0 {
            self.event(CaptureEvent::NonfiniteSamples {
                source_frame: block.source_frame,
                samples: invalid,
            })?;
        }
        self.next = block.source_frame + block.frames as u64;
        Ok(())
    }
    pub fn event(&mut self, event: CaptureEvent) -> Result<()> {
        if self.failed || self.stats.events >= MAX_EVENTS {
            return Err(Error::Conflict(
                "Native archive event journal is failed or full".into(),
            ));
        }
        let mut bytes = serde_json::to_vec(&event)?;
        bytes.push(b'\n');
        if let Err(error) = self.events.write_all(&bytes) {
            self.failed = true;
            return Err(error.into());
        }
        self.hashes[2].update(&bytes);
        self.stats.events += 1;
        Ok(())
    }
    /// Call from worker on a bounded interval. A successful sync is the durable
    /// prefix boundary; raw files survive cancellation and unclean shutdown.
    pub fn sync(&mut self) -> Result<()> {
        if self.failed {
            return Err(Error::Conflict(
                "Cannot checkpoint a failed native archive".into(),
            ));
        }
        if let Err(error) = self.file.sync_all().and_then(|_| self.events.sync_all()) {
            self.failed = true;
            return Err(error.into());
        }
        Ok(())
    }
    pub fn finish(self) -> Result<ArchiveStats> {
        Ok(self.finish_custody()?.stats)
    }
    pub(crate) fn finish_custody(mut self) -> Result<FinishedCaptureArchive> {
        self.sync()?;
        let completion_bytes = serde_json::to_vec(&self.stats)?;
        self.completion.write_all(&completion_bytes)?;
        self.hashes[3].update(&completion_bytes);
        self.completion.sync_all()?;
        self.directory.sync()?;
        let names = [
            "manifest.json",
            "native.packets",
            "events.jsonl",
            "completion.json",
        ];
        let files = [&self.metadata, &self.file, &self.events, &self.completion];
        let mut seals = Vec::with_capacity(4);
        for ((name, file), hash) in names.into_iter().zip(files).zip(self.hashes.iter()) {
            let identity = soundroom_platform::file_identity(file)?;
            let named = self.directory.read_file(name)?;
            if soundroom_platform::file_identity(&named)? != identity
                || named.metadata()?.len() != file.metadata()?.len()
            {
                return Err(Error::Conflict(
                    "Capture archive entry changed before durable custody transition".into(),
                ));
            }
            seals.push(CaptureArtifactSeal {
                name,
                identity,
                bytes: file.metadata()?.len(),
                sha256: hash.clone().finalize().into(),
            });
        }
        let artifacts = seals
            .try_into()
            .map_err(|_| Error::Invalid("Capture artifact seal count".into()))?;
        let Self {
            directory,
            stats,
            file,
            events,
            completion,
            metadata,
            ..
        } = self;
        // No mutable archive file handles survive the custody transition.
        drop(metadata);
        drop(file);
        drop(events);
        drop(completion);
        Ok(FinishedCaptureArchive {
            directory,
            stats,
            artifacts,
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ArchiveRead {
    Packet(CapturedBlock),
    End,
    /// Bytes in an incomplete final header/PCM payload; not interpreted as audio.
    TruncatedTail {
        bytes: u64,
    },
}
pub struct CaptureArchiveReader {
    pub manifest: ArchiveManifest,
    file: File,
    bytes: Vec<u8>,
    remaining: u64,
    next: u64,
    done: bool,
}
impl CaptureArchiveReader {
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_files(
            File::open(path.join("manifest.json"))?,
            File::open(path.join("native.packets"))?,
        )
    }
    pub fn from_directory(directory: &soundroom_platform::PrivateDirectory) -> Result<Self> {
        Self::from_files(
            directory.read_file("manifest.json")?,
            directory.read_file("native.packets")?,
        )
    }
    fn from_files(mut metadata: File, mut file: File) -> Result<Self> {
        if metadata.metadata()?.len() > 16384 {
            return Err(Error::Invalid("Raw archive metadata exceeds bound".into()));
        }
        let mut json = Vec::new();
        metadata.read_to_end(&mut json)?;
        let manifest: ArchiveManifest = serde_json::from_slice(&json)?;
        manifest.validate()?;
        let length = file.metadata()?.len();
        if !(HEADER as u64..=16 * 1024u64.pow(4)).contains(&length) {
            return Err(Error::Invalid("Raw archive file size exceeds bound".into()));
        }
        let mut header = [0; HEADER];
        file.read_exact(&mut header)?;
        if &header[..8] != b"SRCPACK1"
            || read32(&header, 8) as usize != manifest.channels
            || read32(&header, 12) != manifest.sample_rate
            || read32(&header, 16) as usize != manifest.maximum_frames
            || read32(&header, 28) != checksum(&header[..28])
            || header[20..28] != [0; 8]
        {
            return Err(Error::Invalid(
                "Raw archive header does not match manifest".into(),
            ));
        }
        let capacity = manifest.channels * manifest.maximum_frames * 4;
        Ok(Self {
            manifest,
            file,
            bytes: vec![0; capacity],
            remaining: length - HEADER as u64,
            next: 0,
            done: false,
        })
    }
    pub fn read_planar(&mut self, channels: &mut [&mut [f32]]) -> Result<ArchiveRead> {
        if channels.len() != self.manifest.channels
            || channels
                .iter()
                .any(|c| c.len() < self.manifest.maximum_frames)
        {
            return Err(Error::Invalid(
                "Native recovery buffers do not match manifest".into(),
            ));
        }
        if self.done || self.remaining == 0 {
            self.done = true;
            return Ok(ArchiveRead::End);
        }
        if self.remaining < PACKET as u64 {
            self.done = true;
            return Ok(ArchiveRead::TruncatedTail {
                bytes: self.remaining,
            });
        }
        self.done = true; // IO or validation failures permanently stop this scan.
        let mut header = [0u8; PACKET];
        self.file.read_exact(&mut header)?;
        self.remaining -= PACKET as u64;
        let frames = read32(&header, 4) as usize;
        let source_frame = read64(&header, 8);
        self.done = true; // Any validation/IO error permanently stops this scan.
        if &header[..4] != b"PACK"
            || read32(&header, 40) != checksum(&header[..40])
            || header[44..] != [0; 4]
            || !(1..=self.manifest.maximum_frames).contains(&frames)
            || source_frame < self.next
            || source_frame
                .checked_add(frames as u64)
                .map_or(true, |n| n > MAX_FRAME)
        {
            return Err(Error::Invalid(
                "Corrupt native archive packet header or sequence".into(),
            ));
        }
        let size = frames * self.manifest.channels * 4;
        if self.remaining < size as u64 {
            return Ok(ArchiveRead::TruncatedTail {
                bytes: PACKET as u64 + self.remaining,
            });
        }
        let bytes = &mut self.bytes[..size];
        self.file.read_exact(bytes)?;
        self.remaining -= size as u64;
        if read32(&header, 36) != checksum(bytes) {
            return Err(Error::Invalid("Corrupt native capture PCM checksum".into()));
        }
        for (channel, audio) in channels.iter_mut().enumerate() {
            for (frame, sample) in audio[..frames].iter_mut().enumerate() {
                *sample = f32::from_bits(read32(bytes, (channel * frames + frame) * 4));
            }
        }
        self.next = source_frame + frames as u64;
        self.done = false;
        Ok(ArchiveRead::Packet(CapturedBlock {
            source_frame,
            frames,
            timing: CaptureTiming {
                sample_time: f64::from_bits(read64(&header, 16)),
                host_time: read64(&header, 24),
                flags: read32(&header, 32),
            },
        }))
    }
}
fn read32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}
fn read64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}
fn checksum(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArchiveEventRead {
    Event(CaptureEvent),
    End,
    TruncatedTail { bytes: usize },
}
/// Bounded streaming event recovery. Incomplete final JSON is an explicit tail;
/// corrupt complete events stop recovery rather than accepting partial fields.
pub struct CaptureEventReader {
    file: BufReader<File>,
    bytes: Vec<u8>,
    count: u64,
    done: bool,
}
impl CaptureEventReader {
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_file(File::open(path.join("events.jsonl"))?)
    }
    pub fn from_directory(directory: &soundroom_platform::PrivateDirectory) -> Result<Self> {
        Self::from_file(directory.read_file("events.jsonl")?)
    }
    fn from_file(file: File) -> Result<Self> {
        if file.metadata()?.len() > 512 * 1024 * 1024 {
            return Err(Error::Invalid("Capture event journal exceeds bound".into()));
        }
        Ok(Self {
            file: BufReader::with_capacity(16384, file),
            bytes: Vec::with_capacity(16385),
            count: 0,
            done: false,
        })
    }
    pub fn read_event(&mut self) -> Result<ArchiveEventRead> {
        if self.done {
            return Ok(ArchiveEventRead::End);
        }
        self.done = true;
        self.bytes.clear();
        (&mut self.file)
            .take(16385)
            .read_until(b'\n', &mut self.bytes)?;
        if self.bytes.is_empty() {
            return Ok(ArchiveEventRead::End);
        }
        if self.bytes.len() > 16384 || self.count >= MAX_EVENTS {
            return Err(Error::Invalid(
                "Capture event count/size exceeds bound".into(),
            ));
        }
        if self.bytes.last() != Some(&b'\n') {
            return Ok(ArchiveEventRead::TruncatedTail {
                bytes: self.bytes.len(),
            });
        }
        let event: CaptureEvent = serde_json::from_slice(&self.bytes)?;
        self.count += 1;
        self.done = false;
        Ok(ArchiveEventRead::Event(event))
    }
}
