//! Preview voices mixed after the transport graph: drum/instrument/take/effect
//! auditions and live designer notes. Buffers are prepared off the callback and
//! published through one pending slot per voice; the callback hands finished
//! buffers back through a retired slot. The callback never allocates or frees.
use std::sync::{
    atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    Arc,
};
/// Live-instrument ring capacity (frames).
pub const RING: usize = 4096;

pub const VOICES: usize = 16;
/// Voice reserved for recording count-in clicks.
pub const COUNT_IN_VOICE: usize = 15;
const FADE: usize = 256;
/// Raised-cosine crossfade when a live edit replaces a playing loop.
const XFADE: usize = 480;

#[derive(Default)]
pub struct Buffer {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    pub looping: bool,
    pub gain: f32,
    /// Musical length; a continuing replacement keeps the same beat.
    pub beats: f64,
    /// Replaces a playing buffer in place (same beat, crossfaded) instead of
    /// restarting it.
    pub continuous: bool,
    /// A looping buffer returns here, not to 0: frames before it are a
    /// one-time intro (e.g. a drum loop's first pass without the previous
    /// pass's tails folded in). `beats` describes the looped part.
    pub loop_start: usize,
}
#[derive(Default)]
struct Voice {
    pending: AtomicPtr<Buffer>,
    retired: AtomicPtr<Buffer>,
    release: AtomicU64,
    /// A pending buffer that waits for the callback's trigger.
    armed: AtomicBool,
    playing: AtomicBool,
    position: AtomicU64,
}
fn free(slot: &AtomicPtr<Buffer>) {
    let ptr = slot.swap(std::ptr::null_mut(), Ordering::AcqRel);
    if !ptr.is_null() {
        drop(unsafe { Box::from_raw(ptr) });
    }
}
/// Single-producer/single-consumer stereo sample ring (f32 bits).
struct Ring {
    left: Box<[AtomicU32]>,
    right: Box<[AtomicU32]>,
    written: AtomicUsize,
    read: AtomicUsize,
}
impl Default for Ring {
    fn default() -> Self {
        Self {
            left: (0..RING).map(|_| AtomicU32::new(0)).collect(),
            right: (0..RING).map(|_| AtomicU32::new(0)).collect(),
            written: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
        }
    }
}
#[derive(Default)]
struct Shared {
    voices: [Voice; VOICES],
    /// 0: live instrument, 1: recording input monitor.
    rings: [Ring; 2],
    writer_claimed: [AtomicBool; 2],
}
/// The one producer for the live-instrument ring.
pub struct StreamWriter {
    shared: Arc<Shared>,
    ring: usize,
}
impl StreamWriter {
    /// Frames currently queued ahead of the callback.
    pub fn queued(&self) -> usize {
        let ring = &self.shared.rings[self.ring];
        ring.written.load(Ordering::Acquire) - ring.read.load(Ordering::Acquire)
    }
    /// Appends as many frames as fit; returns the count written.
    pub fn write(&self, left: &[f32], right: &[f32]) -> usize {
        let ring = &self.shared.rings[self.ring];
        let written = ring.written.load(Ordering::Relaxed);
        let count = left.len().min(right.len()).min(RING - self.queued());
        for i in 0..count {
            let slot = (written + i) % RING;
            ring.left[slot].store(left[i].to_bits(), Ordering::Relaxed);
            ring.right[slot].store(right[i].to_bits(), Ordering::Relaxed);
        }
        ring.written.store(written + count, Ordering::Release);
        count
    }
}
impl Drop for StreamWriter {
    fn drop(&mut self) {
        self.shared.writer_claimed[self.ring].store(false, Ordering::Release);
    }
}
impl Drop for Shared {
    fn drop(&mut self) {
        for voice in &self.voices {
            free(&voice.pending);
            free(&voice.retired);
        }
    }
}
/// Control endpoint (any non-realtime thread).
#[derive(Clone)]
pub struct PreviewControl {
    shared: Arc<Shared>,
}
/// Callback endpoint, owned by the audio host.
pub struct PreviewMixer {
    shared: Arc<Shared>,
    current: [*mut Buffer; VOICES],
    position: [usize; VOICES],
    /// The replaced buffer still sounding during a crossfade.
    previous: [*mut Buffer; VOICES],
    previous_position: [usize; VOICES],
    /// The replaced buffer was cut off by a new sound: it fades out under the
    /// new one (from this level) instead of crossfading into it.
    cut: [bool; VOICES],
    previous_level: [f32; VOICES],
    transition: [usize; VOICES],
    release: [u64; VOICES],
    fade: [Option<usize>; VOICES],
    triggered: [bool; VOICES],
}
unsafe impl Send for PreviewMixer {}
impl Drop for PreviewMixer {
    fn drop(&mut self) {
        for ptr in self.current.iter_mut().chain(self.previous.iter_mut()) {
            if !ptr.is_null() {
                drop(unsafe { Box::from_raw(*ptr) });
                *ptr = std::ptr::null_mut();
            }
        }
    }
}
pub fn channel() -> (PreviewControl, PreviewMixer) {
    let shared = Arc::new(Shared::default());
    (
        PreviewControl {
            shared: shared.clone(),
        },
        PreviewMixer {
            shared,
            current: [std::ptr::null_mut(); VOICES],
            position: [0; VOICES],
            previous: [std::ptr::null_mut(); VOICES],
            previous_position: [0; VOICES],
            cut: [false; VOICES],
            previous_level: [1.; VOICES],
            transition: [XFADE; VOICES],
            release: [0; VOICES],
            fade: [None; VOICES],
            triggered: [false; VOICES],
        },
    )
}
impl PreviewControl {
    /// Claims the live-instrument ring producer (one at a time).
    pub fn stream_writer(&self) -> Option<StreamWriter> {
        self.claim(0)
    }
    /// Claims the input-monitor ring producer (one recording at a time).
    pub fn monitor_writer(&self) -> Option<StreamWriter> {
        self.claim(1)
    }
    fn claim(&self, ring: usize) -> Option<StreamWriter> {
        self.shared.writer_claimed[ring]
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            .then(|| StreamWriter {
                shared: self.shared.clone(),
                ring,
            })
    }
    /// Frees buffers the callback finished with.
    pub fn reclaim(&self) {
        for voice in &self.shared.voices {
            free(&voice.retired);
        }
    }
    /// Starts `buffer` on `voice`, replacing whatever it plays.
    pub fn play(&self, voice: usize, buffer: Buffer) {
        self.reclaim();
        let Some(slot) = self.shared.voices.get(voice) else {
            return;
        };
        let old = slot
            .pending
            .swap(Box::into_raw(Box::new(buffer)), Ordering::AcqRel);
        if !old.is_null() {
            drop(unsafe { Box::from_raw(old) });
        }
    }
    /// Publishes `buffer` to start only when the callback triggers `voice`.
    pub fn arm(&self, voice: usize, buffer: Buffer) {
        if let Some(slot) = self.shared.voices.get(voice) {
            slot.armed.store(true, Ordering::Release);
        }
        self.play(voice, buffer);
    }
    /// Fades `voice` out (note-off / stop).
    pub fn release(&self, voice: usize) {
        if let Some(slot) = self.shared.voices.get(voice) {
            free(&slot.pending);
            slot.release.fetch_add(1, Ordering::AcqRel);
        }
    }
    pub fn release_all(&self) {
        for voice in 0..VOICES {
            self.release(voice);
        }
    }
    pub fn playing(&self, voice: usize) -> bool {
        self.shared.voices.get(voice).is_some_and(|v| {
            v.playing.load(Ordering::Acquire) || !v.pending.load(Ordering::Acquire).is_null()
        })
    }
    pub fn any_playing(&self) -> bool {
        (0..VOICES).any(|v| self.playing(v))
    }
    pub fn position(&self, voice: usize) -> u64 {
        self.shared
            .voices
            .get(voice)
            .map_or(0, |v| v.position.load(Ordering::Acquire))
    }
}
fn retire(slot: &Voice, current: &mut *mut Buffer) -> bool {
    if !hand_back(slot, current) {
        return false;
    }
    slot.playing.store(false, Ordering::Release);
    true
}
/// Passes a finished buffer to the control side through the retired slot.
fn hand_back(slot: &Voice, buffer: &mut *mut Buffer) -> bool {
    if !slot.retired.load(Ordering::Acquire).is_null() {
        return false;
    }
    slot.retired.store(*buffer, Ordering::Release);
    *buffer = std::ptr::null_mut();
    true
}
fn frames(buffer: &Buffer) -> usize {
    buffer.left.len().min(buffer.right.len())
}
/// Where `next` continues from `position` in `old`: the same beat when both
/// have a musical length, else the same frame.
fn continued_position(old: &Buffer, position: usize, next: &Buffer) -> usize {
    let (old_start, start) = (loop_start(old), loop_start(next));
    let (old_len, len) = (
        (frames(old) - old_start).max(1),
        (frames(next) - start).max(1),
    );
    // Position within the old loop (an intro maps proportionally onto it).
    let relative = if position >= old_start {
        position - old_start
    } else {
        (position as f64 / old_start.max(1) as f64 * old_len as f64) as usize
    };
    start
        + if old.beats > 0. && next.beats > 0. {
            let beat = relative as f64 / old_len as f64 * old.beats;
            ((beat % next.beats) / next.beats * len as f64).round() as usize % len
        } else {
            relative % len
        }
}
fn loop_start(buffer: &Buffer) -> usize {
    buffer.loop_start.min(frames(buffer).saturating_sub(1))
}
impl PreviewMixer {
    /// Starts an armed voice at the start of this callback's block.
    pub fn trigger(&mut self, voice: usize) {
        if voice < VOICES {
            self.triggered[voice] = true;
        }
    }
    /// Adds every active voice into the output block.
    pub fn mix(&mut self, left: &mut [f32], right: &mut [f32]) {
        for ring in &self.shared.rings {
            let read = ring.read.load(Ordering::Relaxed);
            let available = ring.written.load(Ordering::Acquire) - read;
            let count = available.min(left.len());
            for i in 0..count {
                let slot = (read + i) % RING;
                left[i] += f32::from_bits(ring.left[slot].load(Ordering::Relaxed));
                right[i] += f32::from_bits(ring.right[slot].load(Ordering::Relaxed));
            }
            ring.read.store(read + count, Ordering::Release);
        }
        for voice in 0..VOICES {
            let slot = &self.shared.voices[voice];
            let release = slot.release.load(Ordering::Acquire);
            if release != self.release[voice] {
                self.release[voice] = release;
                if !self.current[voice].is_null() && self.fade[voice].is_none() {
                    self.fade[voice] = Some(FADE);
                }
            }
            // A finished crossfade hands its old buffer back.
            if !self.previous[voice].is_null()
                && (self.transition[voice] >= XFADE || self.current[voice].is_null())
            {
                hand_back(slot, &mut self.previous[voice]);
            }
            let waiting = slot.armed.load(Ordering::Acquire) && !self.triggered[voice];
            if !waiting
                && self.previous[voice].is_null()
                && !slot.pending.load(Ordering::Acquire).is_null()
                && (self.current[voice].is_null() || slot.retired.load(Ordering::Acquire).is_null())
            {
                let next = slot.pending.swap(std::ptr::null_mut(), Ordering::AcqRel);
                let continuing = !self.current[voice].is_null()
                    && self.fade[voice].is_none()
                    && unsafe { (*next).continuous };
                if continuing {
                    let position = continued_position(
                        unsafe { &*self.current[voice] },
                        self.position[voice],
                        unsafe { &*next },
                    );
                    self.previous[voice] = self.current[voice];
                    self.previous_position[voice] = self.position[voice];
                    self.cut[voice] = false;
                    self.transition[voice] = 0;
                    self.position[voice] = position;
                } else if !self.current[voice].is_null() {
                    // A new sound replacing one still sounding starts at full
                    // level while the old one fades out (a hard cut clicks).
                    self.previous[voice] = self.current[voice];
                    self.previous_position[voice] = self.position[voice];
                    self.previous_level[voice] =
                        self.fade[voice].map_or(1., |remaining| remaining as f32 / FADE as f32);
                    self.cut[voice] = true;
                    self.transition[voice] = 0;
                    self.position[voice] = 0;
                } else {
                    self.position[voice] = 0;
                    self.transition[voice] = XFADE;
                }
                self.current[voice] = next;
                slot.armed.store(false, Ordering::Release);
                self.triggered[voice] = false;
                self.fade[voice] = None;
                slot.playing.store(true, Ordering::Release);
            }
            if self.current[voice].is_null() {
                continue;
            }
            let buffer = unsafe { &*self.current[voice] };
            let previous = unsafe { self.previous[voice].as_ref() };
            let length = frames(buffer);
            let mut finished = length == 0;
            for i in 0..left.len() {
                if finished {
                    break;
                }
                if self.position[voice] >= length {
                    if buffer.looping {
                        self.position[voice] = loop_start(buffer);
                    } else {
                        finished = true;
                        break;
                    }
                }
                let mut level = 1.;
                if let Some(remaining) = self.fade[voice].as_mut() {
                    if *remaining == 0 {
                        finished = true;
                        break;
                    }
                    level = *remaining as f32 / FADE as f32;
                    *remaining -= 1;
                }
                let p = self.position[voice];
                let (mut l, mut r) = (buffer.left[p] * buffer.gain, buffer.right[p] * buffer.gain);
                if let Some(old) = previous.filter(|_| self.transition[voice] < XFADE) {
                    let weight = (0.5
                        - 0.5
                            * (self.transition[voice] as f32 / XFADE as f32
                                * std::f32::consts::PI)
                                .cos())
                    .clamp(0., 1.);
                    let old_length = frames(old);
                    let q = self.previous_position[voice];
                    let (ol, or) = if q < old_length {
                        (old.left[q] * old.gain, old.right[q] * old.gain)
                    } else {
                        (0., 0.)
                    };
                    if self.cut[voice] {
                        let level = self.previous_level[voice] * (1. - weight);
                        l += ol * level;
                        r += or * level;
                    } else {
                        l = ol + (l - ol) * weight;
                        r = or + (r - or) * weight;
                    }
                    self.previous_position[voice] = if old.looping && q + 1 >= old_length {
                        loop_start(old)
                    } else {
                        q + 1
                    };
                    self.transition[voice] += 1;
                }
                left[i] += l * level;
                right[i] += r * level;
                self.position[voice] += 1;
            }
            slot.position
                .store(self.position[voice] as u64, Ordering::Release);
            if !self.previous[voice].is_null() && self.transition[voice] >= XFADE {
                hand_back(slot, &mut self.previous[voice]);
            }
            if finished {
                retire(slot, &mut self.current[voice]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plays_once_then_retires_and_release_fades() {
        let (control, mut mixer) = channel();
        control.play(
            0,
            Buffer {
                left: vec![1.; 300],
                right: vec![1.; 300],
                looping: false,
                gain: 0.5,
                ..Default::default()
            },
        );
        let (mut l, mut r) = (vec![0.; 256], vec![0.; 256]);
        mixer.mix(&mut l, &mut r);
        assert_eq!(l[0], 0.5);
        assert!(control.playing(0));
        let (mut l, mut r) = (vec![0.; 256], vec![0.; 256]);
        mixer.mix(&mut l, &mut r);
        assert_eq!(l[43], 0.5);
        assert_eq!(l[44], 0.);
        assert!(!control.playing(0));
        control.play(
            1,
            Buffer {
                left: vec![1.; 10],
                right: vec![1.; 10],
                looping: true,
                gain: 1.,
                ..Default::default()
            },
        );
        let (mut l, mut r) = (vec![0.; 64], vec![0.; 64]);
        mixer.mix(&mut l, &mut r);
        assert!(l.iter().all(|v| *v == 1.));
        control.release(1);
        let (mut l, mut r) = (vec![0.; 512], vec![0.; 512]);
        mixer.mix(&mut l, &mut r);
        assert!(l[10] < 1. && l[300] == 0.);
        assert!(!control.playing(1));
        control.reclaim();
        let writer = control.stream_writer().unwrap();
        assert!(control.stream_writer().is_none());
        assert_eq!(writer.write(&[0.25; 100], &[0.25; 100]), 100);
        let (mut l, mut r) = (vec![0.; 64], vec![0.; 64]);
        mixer.mix(&mut l, &mut r);
        assert_eq!((l[63], writer.queued()), (0.25, 36));
    }
    #[test]
    fn a_new_sound_fades_out_the_one_it_replaces() {
        let (control, mut mixer) = channel();
        let constant = |value: f32| Buffer {
            left: vec![value; 4000],
            right: vec![value; 4000],
            gain: 1.,
            ..Default::default()
        };
        control.play(0, constant(1.));
        let (mut l, mut r) = (vec![0.; 64], vec![0.; 64]);
        mixer.mix(&mut l, &mut r);
        control.play(0, constant(0.25));
        let (mut l, mut r) = (vec![0.; 1024], vec![0.; 1024]);
        mixer.mix(&mut l, &mut r);
        // The new sound starts at once; the old one glides out under it.
        assert!(l[0] > 1.2 && l[0] <= 1.25);
        assert!(l[..XFADE]
            .windows(2)
            .all(|w| w[1] <= w[0] && w[0] - w[1] < 0.01));
        assert!(l[XFADE..].iter().all(|v| *v == 0.25));
        control.reclaim();
    }
    #[test]
    fn continuous_replacement_keeps_the_beat_and_crossfades() {
        let (control, mut mixer) = channel();
        let loop_of = |value: f32, frames: usize, beats: f64, continuous| Buffer {
            left: vec![value; frames],
            right: vec![value; frames],
            looping: true,
            gain: 1.,
            beats,
            continuous,
            loop_start: 0,
        };
        control.play(0, loop_of(1., 4000, 4., false));
        let (mut l, mut r) = (vec![0.; 1000], vec![0.; 1000]);
        mixer.mix(&mut l, &mut r);
        assert_eq!(control.position(0), 1000);
        // Same beat (1 of 4) in a loop twice as long, without restarting.
        control.play(0, loop_of(-1., 8000, 4., true));
        let (mut l, mut r) = (vec![0.; 600], vec![0.; 600]);
        mixer.mix(&mut l, &mut r);
        assert_eq!(control.position(0), 2600);
        assert!(l[0] > 0.99, "the old loop sounds first");
        assert!(
            l[1..XFADE].windows(2).all(|w| w[1] <= w[0]),
            "monotonic crossfade"
        );
        assert!(l[XFADE..].iter().all(|v| *v == -1.));
        // A non-continuous buffer still restarts, fading the old loop out.
        control.play(0, loop_of(0.5, 4000, 4., false));
        let (mut l, mut r) = (vec![0.; 10], vec![0.; 10]);
        mixer.mix(&mut l, &mut r);
        assert_eq!((control.position(0), l[0]), (10, -0.5));
        assert!(l.windows(2).all(|w| w[1] >= w[0]));
        control.reclaim();
    }
    #[test]
    fn looping_buffer_plays_its_intro_once_then_loops_from_loop_start() {
        let (control, mut mixer) = channel();
        // Intro frames are 1.0, loop frames 2.0.
        let mut left = vec![1.; 100];
        left.extend(vec![2.; 50]);
        control.play(
            0,
            Buffer {
                right: left.clone(),
                left,
                looping: true,
                gain: 1.,
                loop_start: 100,
                ..Default::default()
            },
        );
        let (mut l, mut r) = (vec![0.; 400], vec![0.; 400]);
        mixer.mix(&mut l, &mut r);
        assert!(l[..100].iter().all(|v| *v == 1.), "intro once");
        assert!(l[100..].iter().all(|v| *v == 2.), "then only the loop");
    }
}
