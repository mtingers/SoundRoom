//! Bounded single-producer/single-consumer capture handoff. A device callback
//! copies prepared planar samples; workers perform routing, conversion and disk
//! writes. This queue does not invent an audio clock or imply recording support.
use crate::{Error, Result};
use std::{
    cell::UnsafeCell,
    sync::{
        atomic::{AtomicI32, AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureConfig {
    pub channels: usize,
    pub maximum_frames: usize,
    pub blocks: usize,
    pub sample_rate: u32,
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CaptureTiming {
    /// Device-provided sample position; meaningful only with flag bit 1.
    pub sample_time: f64,
    /// Device-provided host ticks; meaningful only with flag bit 2.
    pub host_time: u64,
    pub flags: u32,
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CapturedBlock {
    /// Queue-local sequence including dropped frames, not a session position.
    pub source_frame: u64,
    pub frames: usize,
    pub timing: CaptureTiming,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureStatus {
    Captured,
    Overflow,
    InvalidBlock,
    /// Irreversibly closed producer scope; no PCM or counters were changed.
    Sealed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureStats {
    pub pcm_bytes: usize,
    pub received_frames: u64,
    pub dropped_frames: u64,
    pub invalid_samples: u64,
    pub device_faults: u64,
    pub last_device_error: i32,
}
struct Slot {
    audio: Vec<Vec<f32>>,
    block: CapturedBlock,
}
struct Shared {
    scope_id: u64,
    config: CaptureConfig,
    slots: Vec<UnsafeCell<Slot>>,
    head: AtomicUsize,
    tail: AtomicUsize,
    received: AtomicU64,
    dropped: AtomicU64,
    invalid: AtomicU64,
    faults: AtomicU64,
    last_error: AtomicI32,
    producer: AtomicUsize,
}
const SEALED: usize = 1;
const COPYING: usize = 2;
struct Mutation<'a>(&'a Shared);
impl Drop for Mutation<'_> {
    fn drop(&mut self) {
        self.0.producer.fetch_and(!COPYING, Ordering::Release);
    }
}
impl Shared {
    fn mutation(&self) -> Option<Mutation<'_>> {
        self.producer
            .compare_exchange(0, COPYING, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Mutation(self))
    }
    fn stats(&self) -> CaptureStats {
        let config = self.config;
        CaptureStats {
            pcm_bytes: config.channels * config.maximum_frames * config.blocks * 4,
            received_frames: self.received.load(Ordering::Relaxed),
            dropped_frames: self.dropped.load(Ordering::Relaxed),
            invalid_samples: self.invalid.load(Ordering::Relaxed),
            device_faults: self.faults.load(Ordering::Relaxed),
            last_device_error: self.last_error.load(Ordering::Relaxed),
        }
    }
}
/// Control-plane fence for one native producer scope. Closing the queue does not
/// stop hardware; native Stop success and worker/archive completion are separate.
#[derive(Clone)]
pub struct CaptureControl {
    shared: Arc<Shared>,
}
impl CaptureControl {
    pub fn config(&self) -> CaptureConfig {
        self.shared.config
    }
    pub(crate) fn scope_id(&self) -> u64 {
        self.shared.scope_id
    }
    /// Irreversible and nonblocking. A previously admitted copy/fault update may
    /// finish; final_stats remains unavailable until its release fence completes.
    pub fn seal(&self) {
        self.shared.producer.fetch_or(SEALED, Ordering::AcqRel);
    }
    pub fn is_sealing(&self) -> bool {
        self.shared.producer.load(Ordering::Acquire) & SEALED != 0
    }
    pub fn final_stats(&self) -> Option<CaptureStats> {
        (self.shared.producer.load(Ordering::Acquire) == SEALED).then(|| self.shared.stats())
    }
    pub fn stats(&self) -> CaptureStats {
        self.shared.stats()
    }
}
/// Final source counters after an irreversible producer fence and complete queue
/// drain. Private construction prevents caller-supplied request-time statistics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureSealReceipt {
    stats: CaptureStats,
    pub(crate) scope_id: u64,
    pub(crate) config: CaptureConfig,
}
impl CaptureSealReceipt {
    pub fn stats(&self) -> CaptureStats {
        self.stats
    }
}
// Exactly one writer owns head and exactly one reader owns tail. Writer observes
// tail before mutating a slot; reader observes published head before reading it.
// Neither endpoint is cloneable. Slot ownership changes through acquire/release,
// and reader copies finish before tail release permits replacement.
unsafe impl Sync for Shared {}
pub struct CaptureWriter {
    shared: Arc<Shared>,
    position: u64,
}
pub struct CaptureReader {
    shared: Arc<Shared>,
}
pub fn capture_channel(config: CaptureConfig) -> Result<(CaptureWriter, CaptureReader)> {
    if !(1..=64).contains(&config.channels)
        || !(1..=8192).contains(&config.maximum_frames)
        || !(2..=64).contains(&config.blocks)
        || !config.blocks.is_power_of_two()
        || !(8000..=384000).contains(&config.sample_rate)
    {
        return Err(Error::Invalid(
            "Capture configuration exceeds channel/frame/rate/queue bounds".into(),
        ));
    }
    let bytes = config.channels * config.maximum_frames * config.blocks * 4;
    if bytes > 32 * 1024 * 1024 {
        return Err(Error::Invalid(
            "Capture PCM exceeds 32 MiB queue budget".into(),
        ));
    }
    static SCOPE: AtomicU64 = AtomicU64::new(1);
    let scope_id = SCOPE
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map_err(|_| Error::Conflict("Capture scope identity exhausted".into()))?;
    let shared = Arc::new(Shared {
        scope_id,
        config,
        slots: (0..config.blocks)
            .map(|_| {
                UnsafeCell::new(Slot {
                    audio: (0..config.channels)
                        .map(|_| vec![0.; config.maximum_frames])
                        .collect(),
                    block: CapturedBlock::default(),
                })
            })
            .collect(),
        head: AtomicUsize::new(0),
        tail: AtomicUsize::new(0),
        received: AtomicU64::new(0),
        dropped: AtomicU64::new(0),
        invalid: AtomicU64::new(0),
        faults: AtomicU64::new(0),
        last_error: AtomicI32::new(0),
        producer: AtomicUsize::new(0),
    });
    Ok((
        CaptureWriter {
            shared: shared.clone(),
            position: 0,
        },
        CaptureReader { shared },
    ))
}
impl CaptureWriter {
    pub fn control(&self) -> CaptureControl {
        CaptureControl {
            shared: Arc::clone(&self.shared),
        }
    }
    pub fn config(&self) -> CaptureConfig {
        self.shared.config
    }
    /// Actual device error only; this does not create or advance a packet clock.
    pub fn device_fault(&mut self, status: i32) {
        let Some(_scope) = self.shared.mutation() else {
            return;
        };
        if status != 0 {
            self.shared.last_error.store(status, Ordering::Relaxed);
            self.shared.faults.fetch_add(1, Ordering::Relaxed);
        }
    }
    /// Fixed bounded callback operation. Full queues explicitly drop the new
    /// block; they never overwrite worker-owned PCM or hide a source-frame gap.
    pub fn push(&mut self, channels: &[&[f32]], timing: CaptureTiming) -> CaptureStatus {
        let Some(_scope) = self.shared.mutation() else {
            return CaptureStatus::Sealed;
        };
        let config = self.shared.config;
        let frames = channels.first().map_or(0, |v| v.len());
        if channels.len() != config.channels
            || frames == 0
            || frames > config.maximum_frames
            || channels.iter().any(|v| v.len() != frames)
            || (timing.flags & 1 != 0 && !timing.sample_time.is_finite())
        {
            return CaptureStatus::InvalidBlock;
        }
        let Some(next) = self
            .position
            .checked_add(frames as u64)
            .filter(|&v| v <= crate::model::MAX_FRAME)
        else {
            return CaptureStatus::InvalidBlock;
        };
        let source_frame = self.position;
        self.position = next;
        self.shared.received.store(next, Ordering::Relaxed);
        let head = self.shared.head.load(Ordering::Relaxed);
        let tail = self.shared.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) >= config.blocks {
            self.shared
                .dropped
                .fetch_add(frames as u64, Ordering::Relaxed);
            return CaptureStatus::Overflow;
        }
        let slot = unsafe { &mut *self.shared.slots[head & (config.blocks - 1)].get() };
        let mut invalid = 0;
        for (source, target) in channels.iter().zip(&mut slot.audio) {
            for (&sample, destination) in source.iter().zip(target) {
                *destination = sample;
                if !sample.is_finite() {
                    invalid += 1;
                }
            }
        }
        if invalid != 0 {
            self.shared.invalid.fetch_add(invalid, Ordering::Relaxed);
        }
        slot.block = CapturedBlock {
            source_frame,
            frames,
            timing,
        };
        self.shared
            .head
            .store(head.wrapping_add(1), Ordering::Release);
        CaptureStatus::Captured
    }
}
impl CaptureReader {
    pub fn control(&self) -> CaptureControl {
        CaptureControl {
            shared: Arc::clone(&self.shared),
        }
    }
    /// Obtain only after every accepted packet is consumed. Archive completion
    /// must still persist those packets plus these final loss/fault counters.
    pub fn seal_receipt(&self) -> Option<CaptureSealReceipt> {
        if self.shared.producer.load(Ordering::Acquire) != SEALED
            || self.shared.tail.load(Ordering::Acquire) != self.shared.head.load(Ordering::Acquire)
        {
            return None;
        }
        Some(CaptureSealReceipt {
            stats: self.shared.stats(),
            scope_id: self.shared.scope_id,
            config: self.shared.config,
        })
    }
    pub fn config(&self) -> CaptureConfig {
        self.shared.config
    }
    /// Worker buffers must hold maximum_frames for every channel. Copies all
    /// device channels so one worker can route the same block to multiple tracks.
    /// Returned timestamps are source metadata; session alignment is a separate
    /// clock-domain operation, particularly for independently clocked devices.
    pub fn read_planar(&mut self, channels: &mut [&mut [f32]]) -> Result<Option<CapturedBlock>> {
        let config = self.shared.config;
        if channels.len() != config.channels
            || channels.iter().any(|v| v.len() < config.maximum_frames)
        {
            return Err(Error::Invalid(
                "Capture worker buffers do not match prepared configuration".into(),
            ));
        }
        let tail = self.shared.tail.load(Ordering::Relaxed);
        if tail == self.shared.head.load(Ordering::Acquire) {
            return Ok(None);
        }
        let slot = unsafe { &*self.shared.slots[tail & (config.blocks - 1)].get() };
        let block = slot.block;
        for (source, target) in slot.audio.iter().zip(channels) {
            target[..block.frames].copy_from_slice(&source[..block.frames]);
        }
        self.shared
            .tail
            .store(tail.wrapping_add(1), Ordering::Release);
        Ok(Some(block))
    }
    /// Independent health counters; no transactional/session revision semantics.
    pub fn stats(&self) -> CaptureStats {
        self.shared.stats()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sealing_waits_for_an_admitted_mutation_before_exposing_final_stats() {
        let (writer, reader) = capture_channel(CaptureConfig {
            channels: 1,
            maximum_frames: 4,
            blocks: 2,
            sample_rate: 48000,
        })
        .unwrap();
        let control = reader.control();
        let scope = writer.shared.mutation().unwrap();
        control.seal();
        assert!(control.is_sealing());
        assert!(control.final_stats().is_none());
        assert!(reader.seal_receipt().is_none());
        writer.shared.received.store(123, Ordering::Relaxed);
        writer.shared.last_error.store(-10851, Ordering::Relaxed);
        drop(scope);
        assert_eq!(control.final_stats().unwrap().received_frames, 123);
        assert_eq!(
            reader.seal_receipt().unwrap().stats().last_device_error,
            -10851
        );
        assert!(writer.shared.mutation().is_none());
    }
    #[test]
    fn queue_indices_wrap_without_reusing_worker_owned_slots() {
        let (mut writer, mut reader) = capture_channel(CaptureConfig {
            channels: 1,
            maximum_frames: 4,
            blocks: 8,
            sample_rate: 48000,
        })
        .unwrap();
        let start = usize::MAX - 3;
        writer.shared.head.store(start, Ordering::Relaxed);
        writer.shared.tail.store(start, Ordering::Relaxed);
        for value in 0..8 {
            assert_eq!(
                writer.push(&[&[value as f32; 4]], CaptureTiming::default()),
                CaptureStatus::Captured
            );
        }
        assert_eq!(
            writer.push(&[&[99.; 4]], CaptureTiming::default()),
            CaptureStatus::Overflow
        );
        let mut audio = [0.; 4];
        for value in 0..8 {
            let block = reader.read_planar(&mut [&mut audio]).unwrap().unwrap();
            assert_eq!(block.source_frame, value * 4);
            assert_eq!(audio, [value as f32; 4]);
        }
        assert!(reader.read_planar(&mut [&mut audio]).unwrap().is_none());
        assert_eq!(
            writer.push(&[&[88.; 4]], CaptureTiming::default()),
            CaptureStatus::Captured
        );
        assert_eq!(
            reader
                .read_planar(&mut [&mut audio])
                .unwrap()
                .unwrap()
                .source_frame,
            36
        );
        assert_eq!(audio, [88.; 4]);
    }
}
