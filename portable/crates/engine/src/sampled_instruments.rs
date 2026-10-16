//! Recorded instruments for the sound designer (open CC0 libraries converted
//! by `tools/instruments`): `<kits root>/instruments/<id>/manifest.json`
//! maps 16-bit 48 kHz note samples to key and velocity zones with round
//! robins and, for bowed/blown notes, crossfaded sustain loops.
//!
//! A `Note` is created on a worker (it may read the sample from disk) and
//! rendered sample by sample without allocating.
use crate::{designer::Instrument, sampled_drums, wave::WaveReader};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

const RATE: f64 = 48_000.;
const CACHE_BYTES: usize = 384 * 1024 * 1024;
/// Peak of an instrument's loudest layer: libraries are recorded at
/// arbitrary levels, so each is normalized here and keeps its own dynamics.
const LOUDEST_PEAK: f64 = 0.7;

impl Instrument {
    /// Library folder of the recorded version of this instrument.
    pub fn recording(self) -> Option<&'static str> {
        Some(match self {
            Instrument::StudioPiano => "studioPiano",
            Instrument::UprightPiano => "uprightPiano",
            Instrument::ElectricGuitar => "electricGuitar",
            Instrument::AcousticGuitar => "acousticGuitar",
            Instrument::PickedBass => "pickedBass",
            Instrument::UprightBass => "uprightBass",
            Instrument::SoloCello => "soloCello",
            Instrument::StringEnsemble => "stringEnsemble",
            Instrument::WoodenFlute => "woodenFlute",
            Instrument::ConcertHarp => "concertHarp",
            Instrument::Vibraphone => "vibraphone",
            Instrument::Marimba => "marimba",
            Instrument::Kalimba => "kalimba",
            _ => return None,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Region {
    file: String,
    lokey: i32,
    hikey: i32,
    centre: i32,
    lovel: i32,
    hivel: i32,
    robin: i32,
    #[serde(default)]
    tune: f64,
    gain: f64,
    loop_start: Option<usize>,
    loop_end: Option<usize>,
}
#[derive(Deserialize)]
struct Manifest {
    release: f64,
    regions: Vec<Region>,
}
struct Library {
    root: PathBuf,
    release: f64,
    regions: Vec<Region>,
    /// Scales the loudest layer's typical note to `LOUDEST_PEAK`.
    norm: f64,
}
/// Decoded mono sample (stereo recordings are summed).
pub struct Sample {
    data: Vec<f32>,
}
type Samples = Mutex<(BTreeMap<PathBuf, Arc<Sample>>, usize)>;
fn samples() -> &'static Samples {
    static SAMPLES: OnceLock<Samples> = OnceLock::new();
    SAMPLES.get_or_init(Default::default)
}
fn library(instrument: Instrument) -> Option<Arc<Library>> {
    type Libraries = Mutex<BTreeMap<&'static str, Option<Arc<Library>>>>;
    static LIBRARIES: OnceLock<Libraries> = OnceLock::new();
    let id = instrument.recording()?;
    let root = sampled_drums::kit_root()?.join("instruments").join(id);
    let map = LIBRARIES.get_or_init(Default::default);
    if let Some(found) = map.lock().ok().and_then(|m| m.get(id).cloned()) {
        return found;
    }
    let loaded = std::fs::read(root.join("manifest.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Manifest>(&b).ok())
        .filter(|m| !m.regions.is_empty() && m.regions.iter().all(|r| !r.file.contains('/') && !r.file.contains("..")))
        .map(|m| {
            // Stored files peak at 0.89, so `gain * 0.89` is the original peak.
            let top = m.regions.iter().map(|r| r.hivel).max().unwrap_or(127);
            let mut loud: Vec<f64> = m.regions.iter().filter(|r| r.hivel == top).map(|r| r.gain * 0.89).collect();
            loud.sort_by(f64::total_cmp);
            let typical = loud.get(loud.len() / 2).copied().filter(|g| *g > 1e-6).unwrap_or(1.);
            Arc::new(Library { root, release: m.release.clamp(0.02, 3.), regions: m.regions, norm: LOUDEST_PEAK / typical })
        });
    if let Ok(mut m) = map.lock() {
        m.insert(id, loaded.clone());
    }
    loaded
}
/// True when the recorded version of `instrument` is installed.
pub fn available(instrument: Instrument) -> bool {
    library(instrument).is_some()
}
fn load(path: &PathBuf) -> Option<Arc<Sample>> {
    if let Some(found) = samples().lock().ok().and_then(|c| c.0.get(path).cloned()) {
        return Some(found);
    }
    let mut reader = WaveReader::open(path).ok()?;
    let info = reader.info.clone();
    if info.sample_rate != RATE as u32 || info.frames > 60 * RATE as u64 || !(1..=2).contains(&info.channels) {
        return None;
    }
    let frames = info.frames as usize;
    let mut planar = vec![vec![0f32; frames]; info.channels as usize];
    let mut at = 0;
    while at < frames {
        let n = 8192.min(frames - at);
        let mut views: Vec<&mut [f32]> = planar.iter_mut().map(|c| &mut c[at..at + n]).collect();
        reader.read_planar(at as u64, &mut views).ok()?;
        at += n;
    }
    let scale = 1. / planar.len() as f32;
    let data: Vec<f32> = (0..frames).map(|i| planar.iter().map(|c| c[i]).sum::<f32>() * scale).collect();
    let sample = Arc::new(Sample { data });
    if let Ok(mut cache) = samples().lock() {
        if cache.1 + frames * 4 > CACHE_BYTES {
            cache.0.clear();
            cache.1 = 0;
        }
        cache.1 += frames * 4;
        cache.0.insert(path.clone(), sample.clone());
    }
    Some(sample)
}

/// One playing recorded note (cloning shares the sample data).
#[derive(Clone)]
pub struct Note {
    sample: Arc<Sample>,
    position: f64,
    /// Playback increment at no bend.
    step: f64,
    gain: f64,
    looping: Option<(f64, f64)>,
    /// Seconds of release fade after note-off.
    pub release: f64,
    pub finished: bool,
}
impl Note {
    /// The recording for `pitch` at `velocity` (0…1), varied by `robin`;
    /// `cents` fine-tunes it. `None` if the instrument is not installed.
    pub fn new(instrument: Instrument, pitch: i32, velocity: f64, robin: u64, cents: f64, rate: f64) -> Option<Self> {
        let library = library(instrument)?;
        let vel = ((velocity.clamp(0., 1.) * 126.).round() as i32 + 1).clamp(1, 127);
        let mut zone: Vec<&Region> = library
            .regions
            .iter()
            .filter(|r| (r.lokey..=r.hikey).contains(&pitch) && (r.lovel..=r.hivel).contains(&vel))
            .collect();
        if zone.is_empty() {
            // Outside the sampled range: the nearest note at this dynamic.
            let nearest = library.regions.iter().filter(|r| (r.lovel..=r.hivel).contains(&vel)).min_by_key(|r| (r.centre - pitch).abs())?;
            zone = library.regions.iter().filter(|r| r.centre == nearest.centre && (r.lovel..=r.hivel).contains(&vel)).collect();
        }
        zone.sort_by_key(|r| r.robin);
        let region = zone[(robin % zone.len() as u64) as usize];
        let sample = load(&library.root.join(&region.file))?;
        // Within a velocity layer, ±2.5 dB tracks the exact velocity.
        let span = (region.hivel - region.lovel).max(1) as f64;
        let within = (vel - region.lovel) as f64 / span;
        let layer_gain = 10f64.powf((within - 0.5) * 5. / 20.);
        let step = 2f64.powf(((pitch - region.centre) as f64 + (region.tune + cents) / 100.) / 12.) * RATE / rate;
        let looping = match (region.loop_start, region.loop_end) {
            (Some(start), Some(end)) if end > start + 64 && end <= sample.data.len() => Some((start as f64, end as f64)),
            _ => None,
        };
        Some(Self { sample, position: 0., step, gain: region.gain * layer_gain * library.norm, looping, release: library.release, finished: false })
    }
    /// Next sample at the given pitch-bend ratio (4-point cubic interpolation).
    pub fn next(&mut self, bend: f64) -> f64 {
        if self.finished {
            return 0.;
        }
        let data = &self.sample.data;
        if let Some((start, end)) = self.looping {
            if self.position >= end {
                self.position = start + (self.position - end);
            }
        }
        let i = self.position as usize;
        if i + 2 >= data.len() {
            self.finished = true;
            return 0.;
        }
        let f = self.position - i as f64;
        let at = |k: isize| data[(i as isize + k).max(0) as usize] as f64;
        let (y0, y1, y2, y3) = (at(-1), at(0), at(1), at(2));
        let value = y1 + 0.5 * f * (y2 - y0 + f * (2. * y0 - 5. * y1 + 4. * y2 - y3 + f * (3. * (y1 - y2) + y3 - y0)));
        self.position += self.step * bend;
        value * self.gain
    }
}
