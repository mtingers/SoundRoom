//! One worker per physical input. Each native packet is archived before clock
//! validation. Track routes share that source; independent device clocks never
//! share a resampler. Only completed aligned segments may become session takes.
use crate::{
    capture::CapturedBlock,
    capture_archive::{ArchiveStats, CaptureArchive, CaptureEvent},
    capture_clock::{CaptureAligner, ClockObservation, OutputClockAnchor, PacketAlignment},
    capture_resample::{CaptureResampler, ResampleConfig, ResampledBlock},
    Error, Result,
};
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CaptureIngest {
    Aligned(PacketAlignment),
    /// The native packet is safely retained; no aligned audio is fabricated.
    RetainedOnly,
}
pub struct RecordingSourceWorker {
    archive: CaptureArchive,
    aligner: CaptureAligner,
    resampler: Option<CaptureResampler>,
    suspended: bool,
    last_frame: u64,
    maximum_drift_ppm: f64,
    last_output: Option<OutputClockAnchor>,
    dropped: u64,
    faults: u64,
    capture_scope: Option<u64>,
}
/// Host-only, nonforgeable native Stop + drained queue + durable raw completion
/// proof. Incomplete custody is retained for reviewed recovery, never a clean take.
#[cfg(target_os = "macos")]
pub struct CoreAudioCaptureCustody {
    archive: crate::capture_archive::FinishedCaptureArchive,
    native: crate::core_audio::CoreAudioInputStopped,
    complete: bool,
}
#[cfg(target_os = "macos")]
impl CoreAudioCaptureCustody {
    pub fn capture_scope_id(&self) -> u64 {
        self.native.capture_scope_id()
    }
    pub fn artifacts(&self) -> &[crate::capture_archive::CaptureArtifactSeal; 4] {
        &self.archive.artifacts
    }
    pub fn is_complete(&self) -> bool {
        self.complete
    }
    pub fn stats(&self) -> ArchiveStats {
        self.archive.stats
    }
    pub fn native_health(&self) -> crate::core_audio::CoreAudioInputHealth {
        self.native.health()
    }
    pub fn directory(&self) -> &soundroom_platform::PrivateDirectory {
        &self.archive.directory
    }
    /// Service must inspect the opaque proof first; directory-only custody is
    /// insufficient to claim a complete live recording.
    pub fn into_directory(self) -> soundroom_platform::PrivateDirectory {
        self.archive.directory
    }
}
impl RecordingSourceWorker {
    pub fn new(
        archive: CaptureArchive,
        aligner: CaptureAligner,
        maximum_drift_ppm: f64,
    ) -> Result<Self> {
        if !maximum_drift_ppm.is_finite() || !(10. ..=5000.).contains(&maximum_drift_ppm) {
            return Err(Error::Invalid(
                "Invalid recording worker drift bound".into(),
            ));
        }
        let (input, output) = aligner.configs();
        if archive.manifest().sample_rate != input.sample_rate
            || archive.manifest().host_ticks_per_second != input.host_ticks_per_second
            || maximum_drift_ppm < input.maximum_drift_ppm.max(output.maximum_drift_ppm)
        {
            return Err(Error::Invalid(
                "Archive and alignment clock configuration differ".into(),
            ));
        }
        Ok(Self {
            archive,
            aligner,
            resampler: None,
            suspended: false,
            last_frame: 0,
            maximum_drift_ppm,
            last_output: None,
            dropped: 0,
            faults: 0,
            capture_scope: None,
        })
    }
    /// Live source ownership binds final drain receipts to this exact producer
    /// queue. The offline/recovery constructor remains available separately.
    pub fn with_capture(
        archive: CaptureArchive,
        aligner: CaptureAligner,
        maximum_drift_ppm: f64,
        control: &crate::capture::CaptureControl,
    ) -> Result<Self> {
        let config = control.config();
        let manifest = archive.manifest();
        if manifest.channels != config.channels
            || manifest.maximum_frames != config.maximum_frames
            || manifest.sample_rate != config.sample_rate
            || control.is_sealing()
        {
            return Err(Error::Conflict(
                "Live archive must bind an open capture queue with matching native format".into(),
            ));
        }
        let mut worker = Self::new(archive, aligner, maximum_drift_ppm)?;
        worker.capture_scope = Some(control.scope_id());
        Ok(worker)
    }
    pub fn observe_output(&mut self, anchor: OutputClockAnchor) -> Result<ClockObservation> {
        if self.suspended {
            return Err(Error::Conflict(
                "Recording alignment suspended; native archive remains open".into(),
            ));
        }
        if self.last_output != Some(anchor) {
            self.archive.event(CaptureEvent::OutputAnchor {
                source_frame: Some(self.last_frame),
                epoch: anchor.epoch,
                playback_clock: anchor.playback_clock,
                frames: anchor.frames,
                sample_time_bits: anchor.timing.sample_time.to_bits(),
                host_time: anchor.timing.host_time,
                flags: anchor.timing.flags,
            })?;
            self.last_output = Some(anchor);
        }
        let result = self.aligner.observe_output(anchor);
        if result.is_err() {
            self.suspended = true;
            self.archive.event(CaptureEvent::ClockReset {
                source_frame: self.last_frame,
            })?;
        }
        result
    }
    pub fn ingest(&mut self, block: CapturedBlock, channels: &[&[f32]]) -> Result<CaptureIngest> {
        self.archive.append(block, channels)?;
        self.last_frame = block.source_frame + block.frames as u64;
        if self.suspended {
            return Ok(CaptureIngest::RetainedOnly);
        }
        let alignment = match self.aligner.align_packet(block) {
            Ok(value) => value,
            Err(error) => {
                self.suspended = true;
                self.archive.event(CaptureEvent::ClockReset {
                    source_frame: block.source_frame,
                })?;
                return Err(error);
            }
        };
        if alignment.device_gap != 0 {
            self.archive.event(CaptureEvent::DeviceGap {
                source_frame: block.source_frame,
                frames: alignment.device_gap,
            })?;
        }
        if alignment.clock_observation == ClockObservation::TimestampOutlier {
            self.archive.event(CaptureEvent::TimestampOutlier {
                source_frame: block.source_frame,
            })?;
        }
        if alignment.queue_gap != 0 || alignment.device_gap != 0 {
            // Existing filter keeps its received prefix for final draining. A
            // service may recover both raw segments, but cannot call this a
            // complete uninterrupted take or conceal the gap with silence.
            self.suspended = true;
            return Ok(CaptureIngest::RetainedOnly);
        }
        if self.resampler.is_none() {
            let manifest = self.archive.manifest();
            self.resampler = Some(CaptureResampler::new(
                ResampleConfig {
                    channels: manifest.channels,
                    input_rate: manifest.sample_rate,
                    output_rate: 48000,
                    maximum_drift_ppm: self.maximum_drift_ppm,
                },
                block.source_frame,
                alignment.output_start,
            )?);
        }
        let resampler = self.resampler.as_mut().unwrap();
        if resampler.remaining_capacity() < block.frames {
            self.suspended = true;
            self.archive.event(CaptureEvent::WorkerOverrun {
                source_frame: block.source_frame,
                frames: block.frames,
            })?;
            return Err(Error::Conflict(
                "Aligned worker output ring overrun; native packet retained".into(),
            ));
        }
        if let Err(error) = resampler
            .follow_packet(block.source_frame, alignment)
            .and_then(|_| resampler.push(block.source_frame, channels))
        {
            self.suspended = true;
            self.archive.event(CaptureEvent::ClockReset {
                source_frame: block.source_frame,
            })?;
            return Err(error);
        }
        Ok(CaptureIngest::Aligned(alignment))
    }
    /// Includes overflow at the final packet where a following source-frame gap
    /// never arrives. Counters are advisory snapshots; raw packet sequence remains
    /// authoritative. Any loss/error suspends aligned publication, not retention.
    pub fn observe_capture_health(&mut self, stats: crate::capture::CaptureStats) -> Result<()> {
        if stats.dropped_frames > self.dropped {
            self.archive.event(CaptureEvent::QueueOverrun {
                received_frames: stats.received_frames,
                dropped_frames: stats.dropped_frames,
            })?;
            self.dropped = stats.dropped_frames;
            self.suspended = true;
        }
        if stats.device_faults > self.faults {
            self.archive.event(CaptureEvent::DeviceFault {
                source_frame: stats.received_frames,
                status: stats.last_device_error,
            })?;
            self.faults = stats.device_faults;
            self.suspended = true;
        }
        Ok(())
    }
    pub fn suspended(&self) -> bool {
        self.suspended
    }
    pub fn archive_stats(&self) -> ArchiveStats {
        self.archive.stats()
    }
    pub fn sync(&mut self) -> Result<()> {
        self.archive.sync()
    }
    pub fn finish_conversion(&mut self) {
        if let Some(resampler) = &mut self.resampler {
            resampler.finish();
        }
    }
    pub fn drain(&mut self, channels: &mut [&mut [f32]]) -> Result<Option<ResampledBlock>> {
        self.resampler
            .as_mut()
            .map(|s| s.drain(channels))
            .transpose()
    }
    /// Incomplete/suspended takes still return raw archive evidence. Authority
    /// must not promote them to a complete recording based only on this result.
    pub fn finish_archive(self) -> Result<ArchiveStats> {
        self.archive.finish()
    }
    /// Complete host-only custody boundary. The native receipt cannot be forged
    /// from client data or a producer seal alone. Native rejected callbacks have
    /// unknown placement; aggregate evidence is retained without inventing PCM.
    #[cfg(target_os = "macos")]
    pub fn finish_core_audio_capture(
        mut self,
        stopped: crate::core_audio::CoreAudioInputStopped,
        drained: crate::capture::CaptureSealReceipt,
    ) -> Result<CoreAudioCaptureCustody> {
        if !stopped.matches_capture(&drained)
            || !stopped.matches_device_uid(&self.archive.manifest().device_uid)
            || !stopped.matches_host_ticks_per_second(self.archive.manifest().host_ticks_per_second)
        {
            return Err(Error::Conflict(
                "Native Stop proof belongs to another capture queue/final health".into(),
            ));
        }
        self.validate_sealed(&drained)?;
        let health = stopped.health();
        if health.running
            || !health.closing
            || !health.quiescent
            || health.accepted_frames != self.archive.stats().retained_frames
            || health.timestamped_frames > health.accepted_frames
        {
            return Err(Error::Conflict(
                "Native Stop, queue and raw accepted-frame evidence disagree".into(),
            ));
        }
        self.observe_capture_health(drained.stats())?;
        self.archive.event(CaptureEvent::NativeInputStopped {
            source_frame: self.last_frame,
            had_started: stopped.had_started(),
            callback_count: health.callback_count,
            accepted_frames: health.accepted_frames,
            timestamped_frames: health.timestamped_frames,
            rejected_callbacks: health.rejected_callbacks,
            rejected_frames: health.rejected_frames,
            stop_failures: health.stop_failures,
            last_stop_status: health.last_stop_status,
        })?;
        let raw = self.archive.stats();
        let complete = stopped.had_started()
            && !self.suspended
            && health.rejected_callbacks == 0
            && health.rejected_frames == 0
            && health.stop_failures == 0
            && health.timestamped_frames == health.accepted_frames
            && raw.invalid_samples == 0
            && raw.queue_gap_frames == 0
            && health.capture.dropped_frames == 0
            && health.capture.device_faults == 0;
        let archive = self.archive.finish_custody()?;
        Ok(CoreAudioCaptureCustody {
            archive,
            native: stopped,
            complete,
        })
    }
    fn validate_sealed(&self, receipt: &crate::capture::CaptureSealReceipt) -> Result<()> {
        let stats = receipt.stats();
        let manifest = self.archive.manifest();
        let archive = self.archive.stats();
        if self.capture_scope != Some(receipt.scope_id)
            || receipt.config.channels != manifest.channels
            || receipt.config.sample_rate != manifest.sample_rate
            || receipt.config.maximum_frames != manifest.maximum_frames
            || stats.received_frames.checked_sub(stats.dropped_frames)
                != Some(archive.retained_frames)
            || stats.invalid_samples != archive.invalid_samples
        {
            return Err(Error::Conflict(
                "Sealed capture receipt does not match the bound queue and archived PCM prefix"
                    .into(),
            ));
        }
        Ok(())
    }
    /// Final live raw retention boundary, distinct from hardware Stop and authored
    /// take acceptance. Every accepted packet must have been archived, and final
    /// drops/faults are journaled even if no following packet reveals the gap.
    pub fn finish_archive_sealed(
        mut self,
        receipt: crate::capture::CaptureSealReceipt,
    ) -> Result<ArchiveStats> {
        self.validate_sealed(&receipt)?;
        let stats = receipt.stats();
        self.observe_capture_health(stats)?;
        self.archive.finish()
    }
}
/// Apply one track's native-channel route to an already aligned source block.
/// Mono duplicates to both channels; stereo keeps channel order. No implicit mix
/// or normalization is performed, and an invalid route changes no output.
pub fn route_track(
    channels: &[&[f32]],
    route: &[usize],
    left: &mut [f32],
    right: &mut [f32],
) -> Result<()> {
    if ![1, 2].contains(&route.len())
        || left.len() != right.len()
        || route
            .iter()
            .any(|&i| channels.get(i).map_or(true, |c| c.len() != left.len()))
    {
        return Err(Error::Invalid(
            "Invalid recording mono/stereo channel route".into(),
        ));
    }
    left.copy_from_slice(channels[route[0]]);
    right.copy_from_slice(channels[*route.get(1).unwrap_or(&route[0])]);
    Ok(())
}
