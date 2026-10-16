//! Live per-insert meters. Graphs prepared for the output callback claim one
//! slot per effect ID (off the callback); processors accumulate peak, energy
//! and gain reduction locally and flush to the slot with plain atomics every
//! `FLUSH` samples. The control tick drains slots into snapshots.
use std::sync::{
    atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering},
    OnceLock,
};
/// Pitch-stage feedback (Swift `PitchMeterSnapshot`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PitchReading {
    pub input_frequency: f32,
    pub target_frequency: f32,
    pub correction_cents: f32,
    pub confidence: f32,
    pub wet_blend: f32,
    pub reference_frequency: f32,
    pub source_frame: i64,
}
/// Effect-specific readings copied at flush time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extras {
    pub true_peak: f32,
    pub momentary: f32,
    pub short_term: f32,
    pub bands: [f32; 12],
    pub pitch: Option<PitchReading>,
}
impl Default for Extras {
    fn default() -> Self {
        Self {
            true_peak: 0.,
            momentary: -120.,
            short_term: -120.,
            bands: [0.; 12],
            pitch: None,
        }
    }
}

pub const SLOTS: usize = 512;
const FLUSH: u32 = 256;
/// Fixed-point scale for summed squares (exact integer atomics, no CAS loops).
const SCALE: f64 = 1e9;

#[derive(Default)]
struct Cell {
    key: AtomicU64,
    input_peak: AtomicU32,
    output_peak: AtomicU32,
    input_energy: AtomicU64,
    output_energy: AtomicU64,
    reduction: AtomicU32,
    frames: AtomicU64,
    revision: AtomicU64,
    true_peak: AtomicU32,
    momentary: AtomicU32,
    short_term: AtomicU32,
    bands: [AtomicU32; 12],
    has_pitch: AtomicBool,
    pitch: [AtomicU32; 6],
    pitch_frame: AtomicI64,
}
fn table() -> &'static [Cell] {
    static TABLE: OnceLock<Box<[Cell]>> = OnceLock::new();
    TABLE.get_or_init(|| (0..SLOTS).map(|_| Cell::default()).collect())
}
fn key(id: &str) -> u64 {
    // FNV-1a over the case-folded UUID; zero marks an empty slot.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in id.bytes() {
        hash ^= byte.to_ascii_uppercase() as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash.max(1)
}
/// Finds or claims the slot for `id`. Not for the callback.
pub fn slot(id: &str) -> Option<usize> {
    let key = key(id);
    let table = table();
    let start = (key % SLOTS as u64) as usize;
    for probe in 0..SLOTS {
        let index = (start + probe) % SLOTS;
        let cell = &table[index];
        match cell
            .key
            .compare_exchange(0, key, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => return Some(index),
            Err(existing) if existing == key => return Some(index),
            Err(_) => {}
        }
    }
    None
}
/// Callback-side accumulator owned by one processor.
#[derive(Clone, Default)]
pub struct Accumulator {
    slot: Option<usize>,
    input_peak: f32,
    output_peak: f32,
    input_energy: f64,
    output_energy: f64,
    reduction: f32,
    frames: u32,
    extras: Option<Extras>,
}
impl Accumulator {
    /// True when the next sample flushes; effect readings are gathered then.
    pub fn due(&self) -> bool {
        self.slot.is_some() && self.frames + 1 >= FLUSH
    }
    pub fn extras(&mut self, extras: Extras) {
        self.extras = Some(extras);
    }
    pub fn attach(&mut self, id: &str) {
        self.slot = slot(id);
    }
    pub fn active(&self) -> bool {
        self.slot.is_some()
    }
    #[inline]
    pub fn sample(&mut self, input: [f32; 2], output: [f32; 2], reduction_db: f32) {
        let Some(slot) = self.slot else { return };
        let level = |x: [f32; 2]| {
            let (a, b) = (x[0].abs(), x[1].abs());
            if a.is_finite() && b.is_finite() {
                a.max(b)
            } else {
                0.
            }
        };
        let (i, o) = (level(input), level(output));
        self.input_peak = self.input_peak.max(i);
        self.output_peak = self.output_peak.max(o);
        self.input_energy += (i * i) as f64;
        self.output_energy += (o * o) as f64;
        if reduction_db.is_finite() {
            self.reduction = self.reduction.max(reduction_db);
        }
        self.frames += 1;
        if self.frames >= FLUSH {
            let cell = &table()[slot];
            cell.input_peak
                .fetch_max(self.input_peak.to_bits(), Ordering::Relaxed);
            cell.output_peak
                .fetch_max(self.output_peak.to_bits(), Ordering::Relaxed);
            cell.input_energy
                .fetch_add((self.input_energy * SCALE) as u64, Ordering::Relaxed);
            cell.output_energy
                .fetch_add((self.output_energy * SCALE) as u64, Ordering::Relaxed);
            cell.reduction
                .fetch_max(self.reduction.to_bits(), Ordering::Relaxed);
            if let Some(extras) = self.extras {
                cell.true_peak
                    .fetch_max(extras.true_peak.max(0.).to_bits(), Ordering::Relaxed);
                cell.momentary
                    .store(extras.momentary.to_bits(), Ordering::Relaxed);
                cell.short_term
                    .store(extras.short_term.to_bits(), Ordering::Relaxed);
                for (band, value) in cell.bands.iter().zip(extras.bands) {
                    band.fetch_max(value.max(0.).to_bits(), Ordering::Relaxed);
                }
                if let Some(p) = extras.pitch {
                    for (slot, value) in cell.pitch.iter().zip([
                        p.input_frequency,
                        p.target_frequency,
                        p.correction_cents,
                        p.confidence,
                        p.wet_blend,
                        p.reference_frequency,
                    ]) {
                        slot.store(value.to_bits(), Ordering::Relaxed);
                    }
                    cell.pitch_frame.store(p.source_frame, Ordering::Relaxed);
                    cell.has_pitch.store(true, Ordering::Relaxed);
                }
            }
            cell.frames.fetch_add(self.frames as u64, Ordering::Release);
            *self = Self {
                slot: self.slot,
                ..Self::default()
            };
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub input_peak: f32,
    pub output_peak: f32,
    pub input_rms: f32,
    pub output_rms: f32,
    pub gain_reduction_db: f32,
    pub frames: u64,
    pub revision: u64,
    pub extras: Extras,
}
/// Drains what the callback measured for `id` since the previous take.
pub fn take(id: &str) -> Option<Snapshot> {
    let key = key(id);
    let table = table();
    let start = (key % SLOTS as u64) as usize;
    let cell = (0..SLOTS)
        .map(|probe| &table[(start + probe) % SLOTS])
        .take_while(|c| c.key.load(Ordering::Acquire) != 0)
        .find(|c| c.key.load(Ordering::Acquire) == key)?;
    let frames = cell.frames.swap(0, Ordering::Acquire);
    if frames == 0 {
        return None;
    }
    let energy =
        |a: &AtomicU64| (a.swap(0, Ordering::AcqRel) as f64 / SCALE / frames as f64).sqrt() as f32;
    Some(Snapshot {
        input_peak: f32::from_bits(cell.input_peak.swap(0, Ordering::AcqRel)),
        output_peak: f32::from_bits(cell.output_peak.swap(0, Ordering::AcqRel)),
        input_rms: energy(&cell.input_energy),
        output_rms: energy(&cell.output_energy),
        gain_reduction_db: f32::from_bits(cell.reduction.swap(0, Ordering::AcqRel)),
        extras: Extras {
            true_peak: f32::from_bits(cell.true_peak.swap(0, Ordering::AcqRel)),
            momentary: match cell.momentary.load(Ordering::Acquire) {
                0 => -120.,
                bits => f32::from_bits(bits),
            },
            short_term: match cell.short_term.load(Ordering::Acquire) {
                0 => -120.,
                bits => f32::from_bits(bits),
            },
            bands: std::array::from_fn(|i| f32::from_bits(cell.bands[i].swap(0, Ordering::AcqRel))),
            pitch: cell.has_pitch.load(Ordering::Acquire).then(|| {
                let value = |i: usize| f32::from_bits(cell.pitch[i].load(Ordering::Acquire));
                PitchReading {
                    input_frequency: value(0),
                    target_frequency: value(1),
                    correction_cents: value(2),
                    confidence: value(3),
                    wet_blend: value(4),
                    reference_frequency: value(5),
                    source_frame: cell.pitch_frame.load(Ordering::Acquire),
                }
            }),
        },
        frames,
        revision: cell.revision.fetch_add(1, Ordering::AcqRel) + 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accumulates_and_drains_one_effect() {
        let mut meter = Accumulator::default();
        meter.attach("4E0B0F8C-0000-4000-8000-000000000001");
        for _ in 0..512 {
            meter.sample([0.5, -0.25], [0.25, 0.1], 3.);
        }
        let snapshot = take("4e0b0f8c-0000-4000-8000-000000000001").unwrap();
        assert_eq!(snapshot.frames, 512);
        assert_eq!((snapshot.input_peak, snapshot.output_peak), (0.5, 0.25));
        assert!((snapshot.input_rms - 0.5).abs() < 1e-4);
        assert_eq!(snapshot.gain_reduction_db, 3.);
        assert!(take("4E0B0F8C-0000-4000-8000-000000000001").is_none());
    }
}
