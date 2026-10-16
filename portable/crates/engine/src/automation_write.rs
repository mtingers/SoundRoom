//! Fixed storage for callback-applied automation gestures. Wire/authored history
//! live outside this module; no client timer or desired value is an audio receipt.
use std::{
    cell::UnsafeCell,
    sync::atomic::{AtomicU8, Ordering},
};
pub const MAX_WRITE_SLOTS: usize = 16;
pub const RAMP_FRAMES: u64 = 256;
/// Worker-side canonical target, preserving saved parameter IDs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteTarget {
    Volume {
        track_id: String,
    },
    Pan {
        track_id: String,
    },
    EffectAmount {
        track_id: String,
        effect_id: String,
    },
    EffectParameter {
        track_id: String,
        effect_id: String,
        parameter_id: String,
    },
}
/// Canonical worker-side mapping from the exact prepared DSP descriptor. The
/// baseline is the saved lane's pre-first-point value, not a desired live value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WriteMapping {
    pub minimum: f32,
    pub maximum: f32,
    pub default_value: f32,
    pub unit: &'static str,
    pub logarithmic: bool,
    pub integer: bool,
    pub baseline_normalized: f32,
    pub interpolation_scale: WriteInterpolation,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteInterpolation {
    Linear,
    Decibels,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    Volume,
    Pan,
    Amount { effect: u8 },
    Parameter { effect: u8, parameter: u8 },
}
/// Resolved only by a prepared graph on a worker. Runtime indices are not a wire
/// API and cannot be caller-forged in safe Rust.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedWriteTarget {
    pub(crate) generation: u64,
    pub(crate) track: u16,
    pub(crate) route: Route,
    pub(crate) latency: u64,
}
impl ResolvedWriteTarget {
    pub fn graph_generation(&self) -> u64 {
        self.generation
    }
    pub fn latency_offset_frames(&self) -> u64 {
        self.latency
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriteLease {
    pub(crate) slot: u8,
    pub(crate) cookie: u128,
    pub(crate) nonce: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteError {
    Busy,
    InvalidValue,
    InvalidSequence,
    StaleLease,
    Unavailable,
    Exhausted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseReason {
    Finished,
    Stopped,
    Cancelled,
    Revoked,
    Disconnected,
    Conflict,
    Discontinuity,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriteStamp {
    pub transport_epoch: u64,
    pub output_frame: u64,
    pub control_frame: u64,
    pub playback_clock: u64,
    /// Structural DSP lookup offset, not a measured acoustic onset prediction.
    pub latency_offset_frames: u64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RampReceipt {
    pub start_value: f32,
    pub target_value: f32,
    pub frames: u64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WriteReceipt {
    Began {
        gesture: u128,
        stamp: WriteStamp,
    },
    Applied {
        gesture: u128,
        sequence: u64,
        stamp: WriteStamp,
        ramp: RampReceipt,
    },
    Restoring {
        gesture: u128,
        reason: ReleaseReason,
        stamp: WriteStamp,
        ramp: RampReceipt,
    },
    Released {
        gesture: u128,
        reason: ReleaseReason,
        stamp: WriteStamp,
        last_applied: Option<(u64, f32)>,
    },
    Rejected {
        gesture: u128,
        sequence: Option<u64>,
        error: WriteError,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteSubmission {
    Queued,
    Superseded { sequence: u64 },
}
#[derive(Clone, Copy)]
pub(crate) enum Message {
    Begin {
        cookie: u128,
        nonce: u64,
        target: ResolvedWriteTarget,
        token: u64,
        epoch: u64,
    },
    Value {
        cookie: u128,
        nonce: u64,
        sequence: u64,
        value: f32,
    },
}
const IDLE: u8 = 0;
const READY: u8 = 1;
const CLAIMED: u8 = 2;
const APPLIED: u8 = 3;
const EDITING: u8 = 4;
/// Single control publisher and callback consumer. CAS ownership, rather than
/// timing guesses, decides whether a pending Value can genuinely be superseded.
pub(crate) struct Slot {
    state: AtomicU8,
    message: UnsafeCell<Option<Message>>,
    receipt: UnsafeCell<Option<WriteReceipt>>,
    release: AtomicU8,
    restore_receipt: UnsafeCell<Option<WriteReceipt>>,
    restore_ready: AtomicU8,
    terminal: UnsafeCell<Option<WriteReceipt>>,
    terminal_ready: AtomicU8,
}
// Every UnsafeCell has one state-machine owner: Ready/Edit control; Claimed/receipt
// callback; receipt transfer through acquire/release. Terminal fields write once.
unsafe impl Sync for Slot {}
impl Default for Slot {
    fn default() -> Self {
        Self {
            state: AtomicU8::new(IDLE),
            message: UnsafeCell::new(None),
            receipt: UnsafeCell::new(None),
            release: AtomicU8::new(0),
            restore_receipt: UnsafeCell::new(None),
            restore_ready: AtomicU8::new(0),
            terminal: UnsafeCell::new(None),
            terminal_ready: AtomicU8::new(0),
        }
    }
}
impl Slot {
    pub fn publish(&self, message: Message) -> Result<WriteSubmission, WriteError> {
        let previous = self.state.load(Ordering::Acquire);
        if ![IDLE, READY].contains(&previous)
            || self
                .state
                .compare_exchange(previous, EDITING, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
        {
            return Err(WriteError::Busy);
        }
        let old = unsafe { (*self.message.get()).take() };
        if self.release.load(Ordering::Acquire) != 0 {
            unsafe {
                *self.message.get() = old;
            };
            self.state.store(previous, Ordering::Release);
            return Err(WriteError::StaleLease);
        }
        let result = match (previous, old) {
            (READY, Some(Message::Value { sequence, .. })) => {
                WriteSubmission::Superseded { sequence }
            }
            (READY, _) => {
                unsafe {
                    *self.message.get() = old;
                };
                self.state.store(READY, Ordering::Release);
                return Err(WriteError::Busy);
            }
            _ => WriteSubmission::Queued,
        };
        unsafe {
            *self.message.get() = Some(message);
        };
        self.state.store(READY, Ordering::Release);
        Ok(result)
    }
    pub fn claim(&self) -> Option<Message> {
        self.state
            .compare_exchange(READY, CLAIMED, Ordering::Acquire, Ordering::Relaxed)
            .ok()?;
        unsafe { *self.message.get() }
    }
    pub fn applied(&self, receipt: WriteReceipt) {
        unsafe {
            *self.receipt.get() = Some(receipt);
        };
        self.state.store(APPLIED, Ordering::Release);
    }
    pub fn poll(&self) -> Option<WriteReceipt> {
        if self.state.load(Ordering::Acquire) == APPLIED {
            let receipt = unsafe { (*self.receipt.get()).take() };
            self.state.store(IDLE, Ordering::Release);
            return receipt;
        }
        if self.restore_ready.swap(0, Ordering::AcqRel) == 1 {
            return unsafe { (*self.restore_receipt.get()).take() };
        }
        if self.terminal_ready.swap(0, Ordering::AcqRel) == 1 {
            return unsafe { (*self.terminal.get()).take() };
        }
        None
    }
    pub fn request_release(&self, reason: ReleaseReason) {
        let desired = reason as u8 + 1;
        let _ = self
            .release
            .fetch_update(Ordering::Release, Ordering::Relaxed, |old| {
                if old == 0 || old == 1 || (old == 2 && desired > 2) {
                    Some(desired)
                } else {
                    None
                }
            });
    }
    pub fn release_reason(&self) -> Option<ReleaseReason> {
        match self.release.load(Ordering::Acquire) {
            1 => Some(ReleaseReason::Finished),
            2 => Some(ReleaseReason::Stopped),
            3 => Some(ReleaseReason::Cancelled),
            4 => Some(ReleaseReason::Revoked),
            5 => Some(ReleaseReason::Disconnected),
            6 => Some(ReleaseReason::Conflict),
            7 => Some(ReleaseReason::Discontinuity),
            _ => None,
        }
    }
    pub fn restoring(&self, receipt: WriteReceipt) {
        unsafe {
            *self.restore_receipt.get() = Some(receipt);
        };
        self.restore_ready.store(1, Ordering::Release);
    }
    pub fn terminal(&self, receipt: WriteReceipt) {
        unsafe {
            *self.terminal.get() = Some(receipt);
        };
        self.terminal_ready.store(1, Ordering::Release);
    }
    pub fn pending(&self) -> bool {
        self.state.load(Ordering::Acquire) == READY
    }
    pub fn editing(&self) -> bool {
        self.state.load(Ordering::Acquire) == EDITING
    }
    // Control owner calls only after ordinary/restoring/terminal receipts drained,
    // and after the callback retired its runtime slot before terminal publication.
    pub fn reset(&self) {
        unsafe {
            *self.message.get() = None;
            *self.receipt.get() = None;
            *self.restore_receipt.get() = None;
            *self.terminal.get() = None;
        }
        self.release.store(0, Ordering::Relaxed);
        self.restore_ready.store(0, Ordering::Relaxed);
        self.terminal_ready.store(0, Ordering::Relaxed);
        self.state.store(IDLE, Ordering::Release);
    }
}
#[derive(Clone, Copy)]
pub(crate) struct ControlLease {
    pub lease: WriteLease,
    pub target: ResolvedWriteTarget,
    pub sequence: u64,
    pub closing: bool,
    pub final_sequence: Option<u64>,
}
#[derive(Clone, Copy)]
pub(crate) struct RuntimeLease {
    pub cookie: u128,
    pub nonce: u64,
    pub target: ResolvedWriteTarget,
    pub token: u64,
    pub epoch: u64,
    pub reason: Option<ReleaseReason>,
    pub restoring: bool,
    pub touched: bool,
    pub terminal: Option<WriteReceipt>,
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Override {
    pub slot: u8,
    start_frame: u64,
    start: f32,
    target: f32,
    decibels: bool,
    pub restoring: bool,
    pub last: Option<(u64, f32)>,
}
impl Override {
    pub fn new(
        slot: u8,
        frame: u64,
        start: f32,
        target: f32,
        decibels: bool,
        restoring: bool,
    ) -> Self {
        Self {
            slot,
            start_frame: frame,
            start,
            target,
            decibels,
            restoring,
            last: None,
        }
    }
    pub fn value(&self, frame: u64) -> f32 {
        crate::automation::interpolate(
            self.start,
            self.target,
            ((frame.saturating_sub(self.start_frame) as f64 / RAMP_FRAMES as f64).min(1.)) as f32,
            self.decibels,
        )
    }
    pub fn sample(&mut self, frame: u64, original: f32) -> f32 {
        let value = if self.restoring && self.finished(frame) {
            original
        } else {
            self.value(frame)
        };
        self.last = Some((frame, value));
        value
    }
    pub fn remaining(&self, frame: u64) -> u64 {
        RAMP_FRAMES.saturating_sub(frame.saturating_sub(self.start_frame))
    }
    pub fn finished(&self, frame: u64) -> bool {
        frame.saturating_sub(self.start_frame) >= RAMP_FRAMES
    }
    pub fn receipt(&self) -> RampReceipt {
        RampReceipt {
            start_value: self.start,
            target_value: self.target,
            frames: RAMP_FRAMES,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    #[test]
    fn concurrent_latest_value_mailbox_resolves_every_sequence_once() {
        const TOTAL: u64 = 100_000;
        let slot = Arc::new(Slot::default());
        let finished = Arc::new(AtomicBool::new(false));
        let callback_slot = Arc::clone(&slot);
        let callback_finished = Arc::clone(&finished);
        let consumer = std::thread::spawn(move || {
            while !callback_finished.load(Ordering::Acquire) {
                if let Some(Message::Value {
                    cookie, sequence, ..
                }) = callback_slot.claim()
                {
                    callback_slot.applied(WriteReceipt::Rejected {
                        gesture: cookie,
                        sequence: Some(sequence),
                        error: WriteError::Unavailable,
                    });
                } else {
                    std::thread::yield_now();
                }
            }
        });
        let mut outcomes = vec![0u8; TOTAL as usize + 1];
        let mut sequence = 1;
        while sequence <= TOTAL {
            if let Some(WriteReceipt::Rejected {
                sequence: Some(s), ..
            }) = slot.poll()
            {
                assert_eq!(outcomes[s as usize], 0);
                outcomes[s as usize] = 1;
            }
            match slot.publish(Message::Value {
                cookie: 1,
                nonce: 1,
                sequence,
                value: 0.5,
            }) {
                Ok(WriteSubmission::Queued) => sequence += 1,
                Ok(WriteSubmission::Superseded { sequence: old }) => {
                    assert_eq!(outcomes[old as usize], 0);
                    outcomes[old as usize] = 2;
                    sequence += 1;
                }
                Err(WriteError::Busy) => std::thread::yield_now(),
                other => panic!("{other:?}"),
            }
        }
        while outcomes[TOTAL as usize] == 0 {
            if let Some(WriteReceipt::Rejected {
                sequence: Some(s), ..
            }) = slot.poll()
            {
                assert_eq!(outcomes[s as usize], 0);
                outcomes[s as usize] = 1;
            } else {
                std::thread::yield_now();
            }
        }
        finished.store(true, Ordering::Release);
        consumer.join().unwrap();
        assert!(outcomes[1..].iter().all(|n| [1, 2].contains(n)));
        assert!(outcomes[1..].contains(&1));
    }
}
