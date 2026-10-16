//! Recorded multi-mic drum kits. Each hit is a 6-channel file of stereo
//! close-mic, overhead and room stems (see `examples/build_drum_kit.rs`).
//! Kit flavors mix the same recordings differently; a missing kit falls back
//! to the modeled acoustic voice, so renders never fail on another machine.
use crate::{
    drums::{Articulation, Kit, Voice},
    wave::WaveReader,
    Error, Result,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
};

const RATE: f64 = 48_000.;
/// Directory name of the bundled recorded kit (DRSKit, CC BY 4.0).
pub const KIT_DIRECTORY: &str = "drskit";
/// Decoded-hit cache bound; stems are re-read from disk past it.
const CACHE_BYTES: usize = 192 * 1024 * 1024;
/// Brings the loudest natural snare to about -3 dBFS before the pattern gain.
const OUTPUT_GAIN: f32 = 0.9;

/// How a kit flavor balances the recorded stems.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StemMix {
    pub close: f32,
    pub overhead: f32,
    pub room: f32,
    /// Stereo width of overheads and room (1 = as recorded).
    pub width: f32,
    /// Tape-style tone: band-limited, gently saturated, narrower.
    pub vintage: bool,
}
impl Kit {
    /// Stem balance for recorded kits; `None` for modeled/synthesized kits.
    pub fn stem_mix(self) -> Option<StemMix> {
        let mix = |close, overhead, room, width, vintage| StemMix {
            close,
            overhead,
            room,
            width,
            vintage,
        };
        match self {
            Kit::StudioNatural => Some(mix(1., 0.8, 0.45, 1., false)),
            Kit::StudioTight => Some(mix(1.1, 0.45, 0.12, 0.8, false)),
            // Level-matched to the natural flavor (about -27 dBFS RMS on a groove).
            Kit::StudioRoom => Some(mix(0.95, 1.12, 1.55, 1.15, false)),
            Kit::StudioVintage => Some(mix(0.4, 1.45, 0.72, 0.45, true)),
            _ => None,
        }
    }
}

/// Decoded stems by file, and their total bytes.
type HitCache = (BTreeMap<String, Arc<Vec<Vec<f32>>>>, usize);
#[derive(Clone, Debug)]
struct Hit {
    file: String,
    power: f64,
    /// Restores the stored file's level (headroom or normalization).
    gain: f32,
    /// Generated velocity (exported kits); recorded hits are placed by power.
    velocity: Option<f64>,
}
struct Library {
    root: PathBuf,
    hits: BTreeMap<(Voice, Articulation), Vec<Hit>>,
    cache: Mutex<HitCache>,
}
static ROOTS: OnceLock<Vec<PathBuf>> = OnceLock::new();
static LIBRARY: OnceLock<Option<Library>> = OnceLock::new();

/// Directories searched for `drskit/hits.jsonl`, in order. Call once at startup.
pub fn set_kit_roots(roots: Vec<PathBuf>) {
    let _ = ROOTS.set(roots);
}
fn parse_voice(name: &str) -> Option<Voice> {
    Voice::ALL.into_iter().find(|v| v.name() == name)
}
fn parse_articulation(name: &str) -> Option<Articulation> {
    Some(match name {
        "center" => Articulation::Center,
        "rimshot" => Articulation::Rimshot,
        "sideStick" => Articulation::SideStick,
        "brush" => Articulation::Brush,
        "bell" => Articulation::Bell,
        _ => return None,
    })
}
fn load(root: &Path) -> Option<Library> {
    let text = std::fs::read_to_string(root.join("hits.jsonl")).ok()?;
    let mut hits: BTreeMap<(Voice, Articulation), Vec<Hit>> = BTreeMap::new();
    for line in text.lines() {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        let voice = parse_voice(value["voice"].as_str()?)?;
        let articulation = parse_articulation(value["articulation"].as_str()?)?;
        let file = value["file"].as_str()?.to_string();
        let power = value["power"].as_f64().filter(|p| p.is_finite() && *p > 0.)?;
        // Recorded stems are stored at half scale; exported kits say so.
        let gain = value["gain"].as_f64().filter(|g| g.is_finite() && *g > 0. && *g < 1e4).unwrap_or(2.) as f32;
        // Hit files stay inside the kit directory.
        if file.contains("..") || file.starts_with('/') {
            return None;
        }
        hits.entry((voice, articulation))
            .or_default()
            .push(Hit { file, power, gain, velocity: value["velocity"].as_f64().filter(|v| (0. ..=1.).contains(v)) });
    }
    for list in hits.values_mut() {
        list.sort_by(|a, b| a.power.total_cmp(&b.power));
    }
    (!hits.is_empty()).then(|| Library {
        root: root.to_owned(),
        hits,
        cache: Mutex::new((BTreeMap::new(), 0)),
    })
}
fn library() -> Option<&'static Library> {
    // Not resolved (or cached as missing) until the roots are configured.
    ROOTS.get()?;
    LIBRARY
        .get_or_init(|| {
            ROOTS
                .get()?
                .iter()
                .find_map(|root| load(&root.join(KIT_DIRECTORY)))
        })
        .as_ref()
}
fn stems_len(hit: &Arc<Vec<Vec<f32>>>) -> usize {
    hit.len()
}
type Customs = Mutex<BTreeMap<String, Option<Arc<Library>>>>;
static CUSTOM: OnceLock<Customs> = OnceLock::new();
/// Exported (kit-designer) kits, by id, under the first root's `custom/`.
fn custom_library(id: &str) -> Option<Arc<Library>> {
    if !valid_id(id) {
        return None;
    }
    let map = CUSTOM.get_or_init(Default::default);
    if let Some(found) = map.lock().ok().and_then(|m| m.get(id).cloned()) {
        return found;
    }
    let library = custom_root().and_then(|root| load(&root.join(id))).map(Arc::new);
    if let Ok(mut m) = map.lock() {
        m.insert(id.to_owned(), library.clone());
    }
    library
}
/// Forgets a cached custom kit after it is exported again or deleted.
pub fn forget_custom(id: &str) {
    if let Ok(mut m) = CUSTOM.get_or_init(Default::default).lock() {
        m.remove(id);
    }
}
/// The first configured kits root (`<server storage>/Kits`).
pub fn kit_root() -> Option<PathBuf> {
    ROOTS.get()?.first().cloned()
}
/// Directory holding exported kits, one folder per kit id.
pub fn custom_root() -> Option<PathBuf> {
    ROOTS.get()?.first().map(|r| r.join("custom"))
}
/// Kit ids are UUIDs: they name directories.
pub fn valid_id(id: &str) -> bool {
    id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}
/// True when an exported kit's audio is installed.
pub fn custom_available(id: &str) -> bool {
    custom_library(id).is_some()
}
/// True when recorded kit audio is installed.
pub fn available() -> bool {
    library().is_some()
}
impl Library {
    fn stems(&self, file: &str) -> Result<Arc<Vec<Vec<f32>>>> {
        if let Some(hit) = self.cache.lock().ok().and_then(|c| c.0.get(file).cloned()) {
            return Ok(hit);
        }
        let mut reader = WaveReader::open(&self.root.join(file))?;
        let info = reader.info.clone();
        // 6 channels: close/overhead/room stems; 2: an exported stereo kit.
        if !matches!(info.channels, 2 | 6) || info.sample_rate != RATE as u32 || info.frames > 20 * RATE as u64 {
            return Err(Error::Invalid("Recorded drum hit has an unexpected format".into()));
        }
        let frames = info.frames as usize;
        let mut stems = vec![vec![0f32; frames]; info.channels as usize];
        let mut at = 0;
        while at < frames {
            let n = 8192.min(frames - at);
            let mut views: Vec<&mut [f32]> = stems.iter_mut().map(|s| &mut s[at..at + n]).collect();
            reader.read_planar(at as u64, &mut views)?;
            at += n;
        }
        let hit = Arc::new(stems);
        if let Ok(mut cache) = self.cache.lock() {
            let bytes = frames * stems_len(&hit) * 4;
            if cache.1 + bytes > CACHE_BYTES {
                cache.0.clear();
                cache.1 = 0;
            }
            cache.1 += bytes;
            cache.0.insert(file.to_owned(), hit.clone());
        }
        Ok(hit)
    }
    /// The recorded layer nearest the requested dynamic, varied by `robin`
    /// among its neighbours, plus the gain that bridges the remaining gap.
    fn choose(&self, voice: Voice, articulation: Articulation, velocity: f64, robin: i32) -> Option<(&Hit, f32)> {
        let list = self
            .hits
            .get(&(voice, articulation))
            .or_else(|| self.hits.get(&(voice, Articulation::Center)))?;
        if list.iter().all(|h| h.velocity.is_some()) {
            return Self::choose_by_velocity(list, velocity, robin);
        }
        let (low, high) = (list[0].power.ln(), list[list.len() - 1].power.ln());
        // Velocity is linear in log power (i.e. in decibels).
        let target = low + (high - low) * velocity.clamp(0., 1.);
        let mut order: Vec<&Hit> = list.iter().collect();
        order.sort_by(|a, b| (a.power.ln() - target).abs().total_cmp(&(b.power.ln() - target).abs()));
        let neighbours = order.len().min(3);
        let hit = order[robin.rem_euclid(neighbours as i32) as usize];
        let gain = ((target - hit.power.ln()) / 2.).exp().clamp(0.6, 1.6) as f32;
        Some((hit, gain))
    }
}

impl Library {
    /// Exported kits: the nearest generated velocity layer (robin picks the
    /// take), and the level interpolated between neighbouring layers.
    fn choose_by_velocity(list: &[Hit], velocity: f64, robin: i32) -> Option<(&Hit, f32)> {
        let mut layers: Vec<f64> = list.iter().filter_map(|h| h.velocity).collect();
        layers.sort_by(f64::total_cmp);
        layers.dedup();
        let mean_ln = |v: f64| {
            let hits: Vec<f64> = list.iter().filter(|h| h.velocity == Some(v)).map(|h| h.power.ln()).collect();
            hits.iter().sum::<f64>() / hits.len().max(1) as f64
        };
        let v = velocity.clamp(0., 1.);
        let nearest = *layers.iter().min_by(|a, b| (*a - v).abs().total_cmp(&(*b - v).abs()))?;
        let takes: Vec<&Hit> = list.iter().filter(|h| h.velocity == Some(nearest)).collect();
        let hit = takes[robin.rem_euclid(takes.len() as i32) as usize];
        // Target log power on the line through the bracketing layers
        // (extrapolated below the softest layer).
        let upper = layers.iter().position(|l| *l >= v).unwrap_or(layers.len() - 1).max(1).min(layers.len() - 1);
        let (a, b) = (layers[upper.saturating_sub(1)], layers[upper]);
        let target = if b > a { mean_ln(a) + (mean_ln(b) - mean_ln(a)) * (v - a) / (b - a) } else { mean_ln(a) };
        let gain = ((target - mean_ln(nearest)) / 2.).exp().clamp(0.05, 2.) as f32;
        Some((hit, gain))
    }
}
/// Stereo render of one recorded hit, or `None` when this kit has no
/// recordings installed (the caller then uses the modeled voice).
pub fn render(
    kit: Kit,
    voice: Voice,
    velocity: f64,
    robin: i32,
    articulation: Articulation,
    damping: f64,
    cancelled: &AtomicBool,
) -> Result<Option<[Vec<f32>; 2]>> {
    let (Some(mix), Some(library)) = (kit.stem_mix(), library()) else {
        return Ok(None);
    };
    render_library(library, mix, voice, velocity, robin, articulation, damping, cancelled)
}
/// A hit from an exported kit-designer kit, or `None` if it is not installed.
pub fn render_custom(
    id: &str,
    voice: Voice,
    velocity: f64,
    robin: i32,
    articulation: Articulation,
    damping: f64,
    cancelled: &AtomicBool,
) -> Result<Option<[Vec<f32>; 2]>> {
    let Some(library) = custom_library(id) else { return Ok(None) };
    let mix = StemMix { close: 1., overhead: 0., room: 0., width: 1., vintage: false };
    render_library(&library, mix, voice, velocity, robin, articulation, damping, cancelled)
}
#[allow(clippy::too_many_arguments)] // render's inputs plus the library and mix
fn render_library(
    library: &Library,
    mix: StemMix,
    voice: Voice,
    velocity: f64,
    robin: i32,
    articulation: Articulation,
    damping: f64,
    cancelled: &AtomicBool,
) -> Result<Option<[Vec<f32>; 2]>> {
    if voice == Voice::Clap {
        return Ok(None);
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err(Error::Invalid("Drum render cancelled".into()));
    }
    let velocity = if velocity.is_finite() { velocity.clamp(0., 1.) } else { 0.8 };
    // A rimshot is the head and rim struck together: a full snare hit
    // layered with the rim click.
    let layers: Vec<(Articulation, f64, f32)> = match (voice, articulation) {
        (Voice::Snare, Articulation::Rimshot) => vec![
            (Articulation::Center, (velocity * 1.08).min(1.), 1.),
            (Articulation::SideStick, velocity, 0.9),
        ],
        (Voice::Crash, Articulation::Bell) => vec![(Articulation::Center, velocity, 1.)],
        _ => vec![(articulation, velocity, 1.)],
    };
    let mut out = [Vec::new(), Vec::new()];
    for (articulation, velocity, level) in layers {
        let Some((hit, gain)) = library.choose(voice, articulation, velocity, robin) else {
            continue;
        };
        let stems = library.stems(&hit.file)?;
        let frames = stems[0].len();
        if out[0].len() < frames {
            out[0].resize(frames, 0.);
            out[1].resize(frames, 0.);
        }
        let g = gain * level * hit.gain * OUTPUT_GAIN;
        let width = mix.width;
        if stems.len() == 2 {
            // Exported stereo kit: already mixed.
            for i in 0..frames {
                out[0][i] += g * stems[0][i];
                out[1][i] += g * stems[1][i];
            }
            continue;
        }
        for i in 0..frames {
            let stereo = |l: f32, r: f32| {
                let (mid, side) = ((l + r) * 0.5, (l - r) * 0.5 * width);
                (mid + side, mid - side)
            };
            let (ol, or) = stereo(stems[2][i], stems[3][i]);
            let (rl, rr) = stereo(stems[4][i], stems[5][i]);
            out[0][i] += g * (stems[0][i] * mix.close + ol * mix.overhead + rl * mix.room);
            out[1][i] += g * (stems[1][i] * mix.close + or * mix.overhead + rr * mix.room);
        }
    }
    if out[0].is_empty() {
        return Ok(None);
    }
    // Damping above the default shortens the ring with a gentle extra decay.
    let damping = if damping.is_finite() { damping.clamp(0., 1.) } else { 0.5 };
    if damping > 0.5 {
        let rate = (damping - 0.5) * 2. * 9. / RATE;
        for channel in &mut out {
            for (i, v) in channel.iter_mut().enumerate() {
                *v *= (-(i as f64) * rate).exp() as f32;
            }
        }
    }
    if mix.vintage {
        vintage(&mut out);
    }
    Ok(Some(out))
}
/// 60s-style tone: 45 Hz high-pass, two-pole 7.5 kHz low-pass, soft tape
/// saturation.
fn vintage(out: &mut [Vec<f32>; 2]) {
    let hp = (-2. * std::f64::consts::PI * 45. / RATE).exp();
    let lp = 1. - (-2. * std::f64::consts::PI * 7500. / RATE).exp();
    for channel in out.iter_mut() {
        let (mut x1, mut y1, mut a, mut b) = (0f64, 0f64, 0f64, 0f64);
        for v in channel.iter_mut() {
            let x = *v as f64;
            let high = hp * (y1 + x - x1);
            x1 = x;
            y1 = high;
            a += lp * (high - a);
            b += lp * (a - b);
            *v = ((b * 1.6).tanh() / 1.6_f64.tanh()) as f32;
        }
    }
}
