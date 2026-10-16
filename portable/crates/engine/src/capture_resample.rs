//! Worker-only adaptive windowed-sinc sample-rate conversion. Raw packets are
//! archived first. This stream admits only contiguous native samples; gaps must
//! be declared and represented as separate segments, never concealed by a filter.
use crate::{model::MAX_FRAME, Error, Result};
const BASE_RADIUS: usize = 96;
const PHASES: usize = 2048;
const RING: usize = 32768;
#[derive(Clone, Copy, Debug)]
pub struct ResampleConfig {
    pub channels: usize,
    pub input_rate: u32,
    pub output_rate: u32,
    pub maximum_drift_ppm: f64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResampledBlock {
    /// Signed for retained count-in/preroll; publication can trim before frame 0.
    pub output_frame: i64,
    pub frames: usize,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResampleStats {
    pub input_frames: u64,
    pub output_frames: u64,
    pub invalid_samples: u64,
    pub pcm_bytes: usize,
    pub filter_bytes: usize,
}
pub struct CaptureResampler {
    config: ResampleConfig,
    nominal_ratio: f64,
    audio: Vec<Vec<f32>>,
    table: Vec<f64>,
    weights: Vec<f64>,
    radius: usize,
    taps: usize,
    begin: u64,
    end: u64,
    initial: u64,
    phase: f64,
    output_frame: i64,
    ratio: f64,
    target_ratio: f64,
    phase_error: f64,
    finished: bool,
    stats: ResampleStats,
}
impl CaptureResampler {
    /// First native queue position and its fractional aligned output position.
    /// Fractional alignment is consumed by the filter; it is not rounded away.
    pub fn new(config: ResampleConfig, source_frame: u64, output_start: f64) -> Result<Self> {
        Self::new_fractional(config, config.input_rate as f64, source_frame, output_start)
    }
    /// File import can retain exact legacy AIFF fractional sample rates. Capture
    /// continues using integer nominal rates with actual timestamp drift fitting.
    pub fn new_fractional(
        config: ResampleConfig,
        input_rate: f64,
        source_frame: u64,
        output_start: f64,
    ) -> Result<Self> {
        if !(1..=64).contains(&config.channels)
            || !(8000..=384000).contains(&config.input_rate)
            || !input_rate.is_finite()
            || !(8000. ..=384000.).contains(&input_rate)
            || (input_rate - config.input_rate as f64).abs() > 0.500001
            || config.output_rate != 48000
            || !config.maximum_drift_ppm.is_finite()
            || !(10. ..=5000.).contains(&config.maximum_drift_ppm)
            || source_frame > MAX_FRAME
            || !output_start.is_finite()
            || output_start.abs() > MAX_FRAME as f64 - 8192.
        {
            return Err(Error::Invalid(
                "Invalid capture resampler configuration".into(),
            ));
        }
        let ratio = input_rate / config.output_rate as f64;
        // Anti-alias cutoff allows the entire admitted rate range. Kaiser beta10
        // provides a high rejection stopband without a packet-boundary window.
        let maximum_ratio = ratio * (1. + config.maximum_drift_ppm * 2e-6);
        let cutoff = (1. / maximum_ratio).min(1.) * 0.96;
        // Keep the same output-domain filter support at high downsampling ratios
        // instead of degrading rejection/upper passband at 192/384 kHz input.
        let radius = BASE_RADIUS * ratio.ceil().max(1.) as usize;
        let taps = radius * 2;
        let mut table = vec![0.; (PHASES + 1) * taps];
        let denominator = bessel_i0(10.);
        for phase in 0..=PHASES {
            let fraction = phase as f64 / PHASES as f64;
            let row = &mut table[phase * taps..(phase + 1) * taps];
            let mut sum = 0.;
            for (tap, weight) in row.iter_mut().enumerate() {
                let distance = tap as f64 - (radius - 1) as f64 - fraction;
                let relative = distance / radius as f64;
                let window =
                    bessel_i0(10. * (1. - relative * relative).max(0.).sqrt()) / denominator;
                let x = std::f64::consts::PI * cutoff * distance;
                *weight = cutoff * if x.abs() < 1e-12 { 1. } else { x.sin() / x } * window;
                sum += *weight;
            }
            for weight in row {
                *weight /= sum;
            }
        }
        let output_frame = output_start.ceil() as i64;
        Ok(Self {
            config,
            nominal_ratio: ratio,
            audio: vec![vec![0.; RING]; config.channels],
            table,
            weights: vec![0.; taps],
            radius,
            taps,
            begin: source_frame,
            end: source_frame,
            initial: source_frame,
            phase: source_frame as f64 + (output_frame as f64 - output_start) * ratio,
            output_frame,
            ratio,
            target_ratio: ratio,
            phase_error: 0.,
            finished: false,
            stats: ResampleStats {
                pcm_bytes: config.channels * RING * 4,
                filter_bytes: (PHASES + 2) * taps * 8,
                ..Default::default()
            },
        })
    }
    /// Worker may update the fitted rate once per timestamp packet. Per-sample
    /// slew avoids abrupt fractional-phase steps (and therefore pitch/click steps).
    pub fn set_ratio(&mut self, ratio: f64) -> Result<()> {
        let nominal = self.nominal_ratio;
        if !ratio.is_finite() || (ratio / nominal - 1.).abs() > self.config.maximum_drift_ppm * 2e-6
        {
            return Err(Error::Conflict(
                "Capture conversion ratio exceeds admitted drift".into(),
            ));
        }
        self.target_ratio = ratio;
        Ok(())
    }
    /// Follow actual clock mapping without jumping filter phase. A drift/phase
    /// correction settles over one second and remains inside admitted slope.
    /// Source gaps require a declared new segment; do not interpolate across them.
    pub fn follow_packet(
        &mut self,
        source_frame: u64,
        alignment: crate::capture_clock::PacketAlignment,
    ) -> Result<f64> {
        if alignment.queue_gap != 0 || alignment.device_gap != 0 {
            return Err(Error::Conflict(
                "Input gap requires an explicit aligned segment boundary".into(),
            ));
        }
        if !alignment.output_start.is_finite() || alignment.output_start.abs() > MAX_FRAME as f64 {
            return Err(Error::Invalid(
                "Invalid fractional recording alignment".into(),
            ));
        }
        self.set_ratio(alignment.input_frames_per_output_frame)?;
        let expected = source_frame as f64
            + (self.output_frame as f64 - alignment.output_start)
                * alignment.input_frames_per_output_frame;
        let error = expected - self.phase;
        if error.abs() > 256. {
            return Err(Error::Conflict(
                "Input/output phase discontinuity exceeds smooth correction bound".into(),
            ));
        }
        self.phase_error = error;
        Ok(error)
    }
    pub fn remaining_capacity(&self) -> usize {
        RING - (self.end - self.begin) as usize
    }
    pub fn stats(&self) -> ResampleStats {
        self.stats
    }
    pub fn push(&mut self, source_frame: u64, channels: &[&[f32]]) -> Result<()> {
        let frames = channels.first().map_or(0, |v| v.len());
        if self.finished
            || channels.len() != self.config.channels
            || !(1..=8192).contains(&frames)
            || channels.iter().any(|v| v.len() != frames)
            || source_frame != self.end
            || source_frame
                .checked_add(frames as u64)
                .map_or(true, |v| v > MAX_FRAME)
        {
            return Err(Error::Invalid(
                "Noncontiguous, finished or invalid native resampler packet".into(),
            ));
        }
        if self.end - self.begin + frames as u64 > RING as u64 {
            return Err(Error::Conflict(
                "Capture resampler worker ring overrun; drain before pushing".into(),
            ));
        }
        for (target, source) in self.audio.iter_mut().zip(channels) {
            for (i, &sample) in source.iter().enumerate() {
                let index = (self.end as usize + i) & (RING - 1);
                target[index] = if sample.is_finite() {
                    sample
                } else {
                    self.stats.invalid_samples += 1;
                    0.
                };
            }
        }
        self.end += frames as u64;
        self.stats.input_frames += frames as u64;
        Ok(())
    }
    /// No extra audio is invented at finish. Zero-padding is filter context only;
    /// output stops at the actual exclusive native end, not an arbitrary tail.
    pub fn finish(&mut self) {
        self.finished = true;
    }
    pub fn drain(&mut self, channels: &mut [&mut [f32]]) -> Result<ResampledBlock> {
        let capacity = channels.first().map_or(0, |v| v.len());
        if channels.len() != self.config.channels
            || !(1..=8192).contains(&capacity)
            || channels.iter().any(|v| v.len() != capacity)
        {
            return Err(Error::Invalid(
                "Invalid capture conversion output buffers".into(),
            ));
        }
        let start = self.output_frame;
        let mut frames = 0;
        while frames < capacity && self.phase < self.end as f64 {
            if self.output_frame.unsigned_abs() >= MAX_FRAME {
                return Err(Error::Invalid(
                    "Aligned output position exceeds session bound".into(),
                ));
            }
            let integer = self.phase.floor() as u64;
            if !self.finished && integer.saturating_add(self.radius as u64) >= self.end {
                break;
            }
            let fraction = self.phase - integer as f64;
            let table_phase = fraction * PHASES as f64;
            let low = (table_phase.floor() as usize).min(PHASES - 1);
            let blend = table_phase - low as f64;
            let row_a = &self.table[low * self.taps..(low + 1) * self.taps];
            let row_b = &self.table[(low + 1) * self.taps..(low + 2) * self.taps];
            let base = i128::from(integer) - (self.radius - 1) as i128;
            let first = (i128::from(self.initial) - base)
                .max(0)
                .min(self.taps as i128) as usize;
            let last = (i128::from(self.end) - base).max(0).min(self.taps as i128) as usize;
            let native = base + first as i128;
            if first < last && native < i128::from(self.begin) {
                return Err(Error::Conflict(
                    "Capture resampler history was retired too early".into(),
                ));
            }
            for tap in first..last {
                self.weights[tap] = row_a[tap] + (row_b[tap] - row_a[tap]) * blend;
            }
            let slot = native as usize & (RING - 1);
            let contiguous = (last - first).min(RING - slot);
            // Compute fractional weights once for all device channels and split
            // the circular buffer into two contiguous, bounds-checked spans.
            for (output, input) in channels.iter_mut().zip(&self.audio) {
                let mut sum = 0.;
                for (&sample, &weight) in input[slot..slot + contiguous]
                    .iter()
                    .zip(&self.weights[first..first + contiguous])
                {
                    sum += sample as f64 * weight;
                }
                for (&sample, &weight) in input[..last - first - contiguous]
                    .iter()
                    .zip(&self.weights[first + contiguous..last])
                {
                    sum += sample as f64 * weight;
                }
                output[frames] = sum as f32;
            }
            self.ratio += (self.target_ratio - self.ratio) * (1. / 4096.);
            let nominal = self.nominal_ratio;
            let limit = nominal * self.config.maximum_drift_ppm * 2e-6;
            let correction =
                (self.phase_error / self.config.output_rate as f64).clamp(-limit, limit);
            self.phase += (self.ratio + correction).clamp(nominal - limit, nominal + limit);
            self.phase_error -= correction;
            self.output_frame += 1;
            frames += 1;
        }
        self.begin = (self.phase.floor() as u64)
            .saturating_sub(self.radius as u64)
            .max(self.initial)
            .min(self.end);
        self.stats.output_frames += frames as u64;
        Ok(ResampledBlock {
            output_frame: start,
            frames,
        })
    }
}
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.;
    let mut term = 1.;
    for k in 1..40 {
        term *= (x * x / 4.) / (k * k) as f64;
        sum += term;
        if term < sum * 1e-16 {
            break;
        }
    }
    sum
}
