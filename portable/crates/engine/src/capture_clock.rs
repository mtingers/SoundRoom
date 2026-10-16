//! Worker-side mapping between actual input/output device timestamp domains.
//! No wall clock or network timing is an audio clock. Missing/reset clocks reject
//! alignment; callers retain the native packet before requesting this mapping.
use crate::{capture::CaptureTiming, capture::CapturedBlock, model::MAX_FRAME, Error, Result};

const HISTORY: usize = 64;
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutputClockAnchor {
    /// Changes on an explicit graph-run replacement; aligned live edits retain it.
    pub epoch: u64,
    pub playback_clock: u64,
    /// Advancing audio frames in this callback, excluding paused trailing silence.
    pub frames: usize,
    pub timing: CaptureTiming,
}
#[derive(Clone, Copy, Debug)]
pub struct ClockConfig {
    pub sample_rate: u32,
    pub host_ticks_per_second: f64,
    /// Admission bound, never an instruction to silently clamp a broken clock.
    pub maximum_drift_ppm: f64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ClockObservation {
    Accepted,
    /// Retained native packet may be used after later, reliable observations.
    TimestampOutlier,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClockEstimate {
    pub frames_per_second: f64,
    pub drift_ppm: f64,
    pub accepted: u64,
    pub rejected: u64,
}
#[derive(Clone, Copy, Default)]
struct Point {
    seconds: f64,
    frames: f64,
}
/// Fixed 64-point centered regression, bounded phase/rate convergence. Native
/// timestamp positions may start anywhere; subtract host ticks before converting
/// to f64 so long-running nanosecond clocks keep their fractional precision.
pub struct SampleClock {
    config: ClockConfig,
    origin: Option<CaptureTiming>,
    history: [Point; HISTORY],
    length: usize,
    cursor: usize,
    last: Option<CaptureTiming>,
    rate: f64,
    phase: f64,
    accepted: u64,
    rejected: u64,
}
impl SampleClock {
    pub fn new(config: ClockConfig) -> Result<Self> {
        if !(8000..=384000).contains(&config.sample_rate)
            || !config.host_ticks_per_second.is_finite()
            || !(1000. ..=1e12).contains(&config.host_ticks_per_second)
            || !config.maximum_drift_ppm.is_finite()
            || !(10. ..=5000.).contains(&config.maximum_drift_ppm)
        {
            return Err(Error::Invalid("Invalid timestamp clock bounds".into()));
        }
        Ok(Self {
            config,
            origin: None,
            history: [Point::default(); HISTORY],
            length: 0,
            cursor: 0,
            last: None,
            rate: config.sample_rate as f64,
            phase: 0.,
            accepted: 0,
            rejected: 0,
        })
    }
    pub fn estimate(&self) -> ClockEstimate {
        ClockEstimate {
            frames_per_second: self.rate,
            drift_ppm: (self.rate / self.config.sample_rate as f64 - 1.) * 1e6,
            accepted: self.accepted,
            rejected: self.rejected,
        }
    }
    pub fn observe(&mut self, timing: CaptureTiming) -> Result<ClockObservation> {
        if timing.flags & 3 != 3
            || !timing.sample_time.is_finite()
            || timing.sample_time.abs() > MAX_FRAME as f64
        {
            return Err(Error::Invalid(
                "Alignment requires valid native sample and host timestamps".into(),
            ));
        }
        if let Some(last) = self.last {
            if timing.host_time <= last.host_time || timing.sample_time <= last.sample_time {
                return Err(Error::Conflict(
                    "Device clock reset or nonmonotonic timestamp".into(),
                ));
            }
        }
        let origin = *self.origin.get_or_insert(timing);
        let seconds = tick_seconds(
            timing.host_time,
            origin.host_time,
            self.config.host_ticks_per_second,
        );
        let frames = timing.sample_time - origin.sample_time;
        // Initial nominal slope can differ within admitted hardware drift. Once
        // fitted, residual phase jitter above 2 ms rejects without moving phase.
        let allowance = self.config.sample_rate as f64 * 0.002
            + if self.accepted < 16 {
                seconds * self.config.sample_rate as f64 * self.config.maximum_drift_ppm * 1e-6
            } else {
                0.
            };
        if (frames - (self.phase + self.rate * seconds)).abs() > allowance {
            self.rejected += 1;
            return Ok(ClockObservation::TimestampOutlier);
        }
        self.history[self.cursor] = Point { seconds, frames };
        self.cursor = (self.cursor + 1) % HISTORY;
        self.length = (self.length + 1).min(HISTORY);
        self.last = Some(timing);
        self.accepted += 1;
        if self.length >= 8 {
            let points = &self.history[..self.length];
            let mean_x = points.iter().map(|p| p.seconds).sum::<f64>() / self.length as f64;
            let mean_y = points.iter().map(|p| p.frames).sum::<f64>() / self.length as f64;
            let variance = points
                .iter()
                .map(|p| (p.seconds - mean_x).powi(2))
                .sum::<f64>();
            let span = points
                .iter()
                .map(|p| p.seconds)
                .fold(f64::NEG_INFINITY, f64::max)
                - points
                    .iter()
                    .map(|p| p.seconds)
                    .fold(f64::INFINITY, f64::min);
            if span >= 0.1 && variance > 0. {
                let slope = points
                    .iter()
                    .map(|p| (p.seconds - mean_x) * (p.frames - mean_y))
                    .sum::<f64>()
                    / variance;
                let drift = (slope / self.config.sample_rate as f64 - 1.) * 1e6;
                if drift.abs() > self.config.maximum_drift_ppm {
                    return Err(Error::Conflict(
                        "Native clock rate exceeds admitted drift bound".into(),
                    ));
                }
                // Pivot about the current point so changing the rate does not
                // create a discontinuous phase proportional to elapsed time.
                let current = self.phase + self.rate * seconds;
                let fitted = mean_y + slope * (seconds - mean_x);
                self.rate += (slope - self.rate) * 0.15;
                let corrected = current + (fitted - current).clamp(-0.25, 0.25);
                self.phase = corrected - self.rate * seconds;
            }
        }
        Ok(ClockObservation::Accepted)
    }
    pub fn sample_at_host(&self, host_time: u64) -> Result<f64> {
        let origin = self
            .origin
            .ok_or_else(|| Error::Invalid("No actual device clock observations".into()))?;
        Ok(origin.sample_time
            + self.phase
            + self.rate
                * tick_seconds(
                    host_time,
                    origin.host_time,
                    self.config.host_ticks_per_second,
                ))
    }
    pub fn host_seconds_at_sample(&self, sample: f64) -> Result<f64> {
        let origin = self
            .origin
            .ok_or_else(|| Error::Invalid("No actual device clock observations".into()))?;
        Ok((sample - origin.sample_time - self.phase) / self.rate)
    }
}
fn tick_seconds(a: u64, b: u64, frequency: f64) -> f64 {
    (i128::from(a) - i128::from(b)) as f64 / frequency
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PacketAlignment {
    /// Fractional unwrapped session frame; may be negative for retained preroll.
    pub output_start: f64,
    pub input_frames_per_output_frame: f64,
    pub queue_gap: u64,
    pub device_gap: u64,
    pub clock_observation: ClockObservation,
}
/// One device clock per input source, shared by all its track/channel routes.
/// Output timestamps must belong to one continuous playing run. Explicit seek,
/// output pause/reset and input sample-time reset require a new recording segment.
pub struct CaptureAligner {
    input: SampleClock,
    output: SampleClock,
    first_output: Option<OutputClockAnchor>,
    last_output: Option<OutputClockAnchor>,
    last_input: Option<CapturedBlock>,
    offset_frames: i64,
}
impl CaptureAligner {
    pub fn new(input: ClockConfig, output: ClockConfig, offset_frames: i64) -> Result<Self> {
        if output.sample_rate != 48000 || offset_frames.unsigned_abs() > 48000 * 30 {
            return Err(Error::Invalid(
                "Recording output must be 48 kHz with bounded calibration offset".into(),
            ));
        }
        if input.host_ticks_per_second != output.host_ticks_per_second {
            return Err(Error::Invalid(
                "Input/output host timestamps need the same tick domain".into(),
            ));
        }
        Ok(Self {
            input: SampleClock::new(input)?,
            output: SampleClock::new(output)?,
            first_output: None,
            last_output: None,
            last_input: None,
            offset_frames,
        })
    }
    pub fn observe_output(&mut self, anchor: OutputClockAnchor) -> Result<ClockObservation> {
        if anchor.epoch == 0
            || !(1..=8192).contains(&anchor.frames)
            || anchor
                .playback_clock
                .checked_add(anchor.frames as u64)
                .map_or(true, |v| v > MAX_FRAME)
        {
            return Err(Error::Invalid(
                "Invalid advancing output timestamp anchor".into(),
            ));
        }
        if let Some(last) = self.last_output {
            if last == anchor {
                return Ok(ClockObservation::Accepted);
            }
            let timeline = anchor.playback_clock as f64 - last.playback_clock as f64;
            let hardware = anchor.timing.sample_time - last.timing.sample_time;
            if anchor.epoch != last.epoch || (timeline - hardware).abs() > 0.1 {
                return Err(Error::Conflict(
                    "Output run paused, sought or changed its clock epoch".into(),
                ));
            }
        }
        let result = self.output.observe(anchor.timing)?;
        if result == ClockObservation::Accepted {
            self.first_output.get_or_insert(anchor);
            self.last_output = Some(anchor);
        }
        Ok(result)
    }
    pub fn align_packet(&mut self, block: CapturedBlock) -> Result<PacketAlignment> {
        let first = self
            .first_output
            .ok_or_else(|| Error::Invalid("Recording has no actual output anchor".into()))?;
        let last = self.last_output.unwrap();
        let future_seconds = tick_seconds(
            block.timing.host_time,
            last.timing.host_time,
            self.output.config.host_ticks_per_second,
        );
        if future_seconds > last.frames as f64 / self.output.rate + 0.25 {
            return Err(Error::Conflict(
                "Output clock anchor is stale for recording alignment".into(),
            ));
        }
        if !(1..=8192).contains(&block.frames)
            || block
                .source_frame
                .checked_add(block.frames as u64)
                .map_or(true, |v| v > MAX_FRAME)
        {
            return Err(Error::Invalid(
                "Invalid native packet length/sequence".into(),
            ));
        }
        let (queue_gap, device_gap) = if let Some(last) = self.last_input {
            let next = last.source_frame + last.frames as u64;
            if block.source_frame < next {
                return Err(Error::Conflict(
                    "Overlapping or reordered native capture packets".into(),
                ));
            }
            let delta = block.timing.sample_time - last.timing.sample_time;
            let expected = (block.source_frame - last.source_frame) as f64;
            if delta < expected - 0.1 {
                return Err(Error::Conflict(
                    "Native input sample clock reset or lost sequence".into(),
                ));
            }
            (
                block.source_frame - next,
                (delta - expected).round().max(0.) as u64,
            )
        } else {
            (block.source_frame, 0)
        };
        let observation = self.input.observe(block.timing)?;
        // Map an input sample through its fitted device time to the output clock.
        // Both origin host values are differenced before f64 conversion.
        let origin = self.input.origin.unwrap();
        let input_seconds = self
            .input
            .host_seconds_at_sample(block.timing.sample_time)?;
        let output_origin = self.output.origin.unwrap();
        let seconds = tick_seconds(
            origin.host_time,
            output_origin.host_time,
            self.output.config.host_ticks_per_second,
        ) + input_seconds;
        let output_sample =
            output_origin.sample_time + self.output.phase + self.output.rate * seconds;
        let output_start = first.playback_clock as f64
            + (output_sample - first.timing.sample_time)
            + self.offset_frames as f64;
        self.last_input = Some(block);
        Ok(PacketAlignment {
            output_start,
            input_frames_per_output_frame: self.input.rate / self.output.rate,
            queue_gap,
            device_gap,
            clock_observation: observation,
        })
    }
    pub fn configs(&self) -> (ClockConfig, ClockConfig) {
        (self.input.config, self.output.config)
    }
    pub fn estimates(&self) -> (ClockEstimate, ClockEstimate) {
        (self.input.estimate(), self.output.estimate())
    }
}
