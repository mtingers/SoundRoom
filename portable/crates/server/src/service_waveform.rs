//! Waveform peaks for `waveform` requests, computed off the service lock.
//! Fine tiles read the WAV directly; coarse ones aggregate a per-asset
//! (min, max) summary built once on first use.
use super::{
    edits::{stamp, AssetStamp},
    engine, protocol, Error,
};
use soundroom_engine::wave::WaveReader;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

/// Immutable media resolution is captured under authority; scans and payload
/// construction run on a bounded, independently admitted worker lane.
pub(super) struct Task {
    asset: String,
    path: PathBuf,
    frames: u64,
    channels: u16,
    start: u64,
    end: u64,
    points: u64,
    #[cfg(test)]
    hook: Option<Arc<dyn Fn() + Send + Sync>>,
}
impl Task {
    pub(super) fn capture(
        state: &super::Service,
        request: &super::RemoteRequest,
    ) -> Result<Self, Error> {
        let super::RemoteCommand::Waveform {
            asset_id,
            start,
            end,
            points,
        } = &request.command
        else {
            return Err(protocol("Expected a waveform request"));
        };
        let media = state
            .media
            .get(asset_id)
            .ok_or_else(|| protocol("Audio asset is unavailable"))?;
        if *points < 1
            || *points > 8192
            || *start < 0
            || start >= end
            || *end as u64 > media.info.frames
        {
            return Err(protocol("Invalid waveform range"));
        }
        Ok(Self {
            asset: asset_id.clone(),
            path: media.path.clone(),
            frames: media.info.frames,
            channels: media.info.channels,
            start: *start as u64,
            end: *end as u64,
            points: *points as u64,
            #[cfg(test)]
            hook: state.waveform_hook.clone(),
        })
    }
    pub(super) fn run(
        self,
        shared: &super::Shared,
        peer: &super::Identity,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<Option<serde_json::Value>, Error> {
        #[cfg(test)]
        if let Some(hook) = self.hook {
            hook();
        }
        let mut measured = peaks_cancellable(
            &self.path,
            self.frames,
            self.start,
            self.end,
            self.points,
            cancel,
        )?;
        measured.channels.truncate(self.channels.min(2) as usize);
        // Large JSON traversal stays off authority too. Check access and media
        // identity again after the read, as credentials/projects may change.
        let payload = serde_json::json!({"id":self.asset,"frames":self.frames,"channels":self.channels,
            "peaks":measured.combined,"channelPeaks":measured.channels,
            "waveformStartFrame":self.start,"framesPerBin":measured.stride});
        let state = shared
            .lock()
            .map_err(|_| protocol("Authority is unavailable"))?;
        if cancel.load(std::sync::atomic::Ordering::Acquire)
            || !state.registry.authorizes(&peer.credential, &peer.role)
            || !state.media.get(&self.asset).is_some_and(|m| {
                m.path == self.path
                    && m.info.frames == self.frames
                    && m.info.channels == self.channels
            })
        {
            return Err(protocol(
                "Waveform cancelled or its media or credential changed",
            ));
        }
        Ok(Some(payload))
    }
}

const SUMMARY_BIN: u64 = 256;
/// Bounds one summary to 8 MiB however long the asset is.
const SUMMARY_BINS: u64 = 1 << 19;
/// Stereo summaries kept in memory (16 bytes per 256 frames).
/// A count limit of 32 thrashed on arrangements with more clips:
/// every scroll or zoom rescanned whole files.
const CACHED_SUMMARY_BYTES: usize = 64 * 1024 * 1024;
const CACHED_SUMMARY_ASSETS: usize = 1024;

type StereoPeak = [(f32, f32); 2];

struct Summary {
    bin: u64,
    peaks: Vec<StereoPeak>,
}
struct Entry {
    summary: Arc<Summary>,
    stamp: AssetStamp,
    frames: u64,
    used: u64,
}
#[derive(Default)]
struct CacheState {
    entries: HashMap<PathBuf, Entry>,
    flights: HashMap<PathBuf, Weak<Mutex<()>>>,
    bytes: usize,
    clock: u64,
}
impl CacheState {
    fn get(&mut self, path: &Path, frames: u64, stamp: &AssetStamp) -> Option<Arc<Summary>> {
        self.clock = self.clock.saturating_add(1);
        if let Some(entry) = self.entries.get_mut(path) {
            if entry.frames == frames && entry.stamp == *stamp {
                entry.used = self.clock;
                return Some(entry.summary.clone());
            }
        }
        if let Some(old) = self.entries.remove(path) {
            self.bytes -= peaks_bytes(&old.summary);
        }
        None
    }
    fn insert(
        &mut self,
        path: PathBuf,
        frames: u64,
        stamp: AssetStamp,
        summary: Arc<Summary>,
        budget: usize,
    ) {
        let bytes = peaks_bytes(&summary);
        if bytes > budget {
            return;
        }
        if let Some(old) = self.entries.remove(&path) {
            self.bytes -= peaks_bytes(&old.summary);
        }
        while self.bytes + bytes > budget || self.entries.len() >= CACHED_SUMMARY_ASSETS {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(path, _)| path.clone())
            else {
                break;
            };
            if let Some(old) = self.entries.remove(&oldest) {
                self.bytes -= peaks_bytes(&old.summary);
            }
        }
        self.clock = self.clock.saturating_add(1);
        self.bytes += bytes;
        self.entries.insert(
            path,
            Entry {
                summary,
                stamp,
                frames,
                used: self.clock,
            },
        );
    }
}
#[derive(Default)]
struct SummaryCache(Mutex<CacheState>);
impl SummaryCache {
    fn get_or_build(
        &self,
        path: &Path,
        frames: u64,
        build: impl FnOnce() -> Result<Arc<Summary>, Error>,
    ) -> Result<Arc<Summary>, Error> {
        let before = stamp(path)?;
        let gate = {
            let mut state = self
                .0
                .lock()
                .map_err(|_| protocol("Waveform cache is unavailable"))?;
            if let Some(summary) = state.get(path, frames, &before) {
                return Ok(summary);
            }
            // Weak gates exist only for active scans/waiters. Pruning dead
            // gates bounds metadata without retaining every historical path.
            state.flights.retain(|_, gate| gate.strong_count() > 0);
            if let Some(gate) = state.flights.get(path).and_then(Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(Mutex::new(()));
                state.flights.insert(path.to_owned(), Arc::downgrade(&gate));
                gate
            }
        };
        // Never hold the shared cache or service authority while scanning or
        // waiting. Other assets continue independently.
        let _scan = gate
            .lock()
            .map_err(|_| protocol("Waveform scan is unavailable"))?;
        let before = stamp(path)?;
        if let Some(summary) = self
            .0
            .lock()
            .map_err(|_| protocol("Waveform cache is unavailable"))?
            .get(path, frames, &before)
        {
            return Ok(summary);
        }
        let summary = build()?;
        if stamp(path)? != before {
            return Err(protocol("Audio changed while measuring its waveform"));
        }
        self.0
            .lock()
            .map_err(|_| protocol("Waveform cache is unavailable"))?
            .insert(
                path.to_owned(),
                frames,
                before,
                summary.clone(),
                CACHED_SUMMARY_BYTES,
            );
        Ok(summary)
    }
}
fn cache() -> &'static SummaryCache {
    static CACHE: OnceLock<SummaryCache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}
fn summary_bin(frames: u64) -> u64 {
    SUMMARY_BIN.max(frames.div_ceil(SUMMARY_BINS))
}
/// Folds frames `start..end` into `peaks`, `bin` frames per pair.
fn scan(
    reader: &mut WaveReader,
    start: u64,
    end: u64,
    bin: u64,
    peaks: &mut [StereoPeak],
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(), Error> {
    let (mut left, mut right) = (vec![0f32; 8192], vec![0f32; 8192]);
    let mut at = start;
    while at < end {
        if cancel.load(std::sync::atomic::Ordering::Acquire) {
            return Err(protocol("Waveform cancelled"));
        }
        let count = 8192.min((end - at) as usize);
        reader
            .read_stereo(at, &mut left[..count], &mut right[..count])
            .map_err(engine)?;
        for (i, (l, r)) in left[..count].iter().zip(&right[..count]).enumerate() {
            let peak = &mut peaks[((at - start + i as u64) / bin) as usize];
            for (channel, value) in peak.iter_mut().zip([*l, *r]) {
                if value.is_finite() {
                    channel.0 = channel.0.min(value);
                    channel.1 = channel.1.max(value);
                }
            }
        }
        at += count as u64;
    }
    Ok(())
}
fn summary(
    path: &Path,
    frames: u64,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Arc<Summary>, Error> {
    cache().get_or_build(path, frames, || {
        let bin = summary_bin(frames);
        let mut peaks = vec![[(0., 0.); 2]; frames.div_ceil(bin) as usize];
        let mut reader = WaveReader::open(path).map_err(engine)?;
        if reader.info.frames != frames {
            return Err(protocol(
                "Audio length changed before measuring its waveform",
            ));
        }
        scan(&mut reader, 0, frames, bin, &mut peaks, cancel)?;
        Ok(Arc::new(Summary { bin, peaks }))
    })
}
fn peaks_bytes(summary: &Summary) -> usize {
    summary.peaks.len() * std::mem::size_of::<StereoPeak>()
}
pub(super) struct WaveformPeaks {
    pub combined: Vec<f32>,
    pub channels: Vec<Vec<f32>>,
    pub stride: u64,
}
/// Combined and separate L/R min/max pairs, sharing the same time bins.
#[cfg(test)]
fn peaks(
    path: &Path,
    frames: u64,
    start: u64,
    end: u64,
    points: u64,
) -> Result<WaveformPeaks, Error> {
    peaks_cancellable(
        path,
        frames,
        start,
        end,
        points,
        &std::sync::atomic::AtomicBool::new(false),
    )
}
fn peaks_cancellable(
    path: &Path,
    frames: u64,
    start: u64,
    end: u64,
    points: u64,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<WaveformPeaks, Error> {
    if cancel.load(std::sync::atomic::Ordering::Acquire) {
        return Err(protocol("Waveform cancelled"));
    }
    if points == 0 || start >= end || end > frames {
        return Err(protocol("Invalid waveform range"));
    }
    let stride = (end - start).div_ceil(points);
    let mut peaks = vec![[(0f32, 0f32); 2]; (end - start).div_ceil(stride) as usize];
    if stride >= summary_bin(frames) {
        let summary = summary(path, frames, cancel)?;
        let last = summary.peaks.len().saturating_sub(1) as u64;
        for (i, peak) in peaks.iter_mut().enumerate() {
            let from = start + i as u64 * stride;
            let to = (from + stride).min(end);
            let (first, final_) = (
                (from / summary.bin).min(last),
                ((to - 1) / summary.bin).min(last),
            );
            for source in &summary.peaks[first as usize..=final_ as usize] {
                for (channel, source) in peak.iter_mut().zip(source) {
                    channel.0 = channel.0.min(source.0);
                    channel.1 = channel.1.max(source.1);
                }
            }
        }
    } else {
        scan(
            &mut WaveReader::open(path).map_err(engine)?,
            start,
            end,
            stride,
            &mut peaks,
            cancel,
        )?;
    }
    Ok(WaveformPeaks {
        combined: peaks
            .iter()
            .flat_map(|[l, r]| [l.0.min(r.0), l.1.max(r.1)])
            .collect(),
        channels: (0..2)
            .map(|channel| {
                peaks
                    .iter()
                    .flat_map(|peak| [peak[channel].0, peak[channel].1])
                    .collect()
            })
            .collect(),
        stride,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundroom_engine::wave::WaveWriter;
    #[test]
    fn direct_and_summary_peaks_agree() {
        let dir = std::env::temp_dir().join(format!("soundroom-wave-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ramp.wav");
        // Bins aligned to the summary make both paths exact.
        let frames = SUMMARY_BIN * 16 * 24;
        let left: Vec<f32> = (0..frames)
            .map(|i| (i as f32 / frames as f32) - 0.5)
            .collect();
        let right: Vec<f32> = left.iter().map(|v| -v * 0.5).collect();
        let mut writer = WaveWriter::create(&path, frames).unwrap();
        for (l, r) in left.chunks(4096).zip(right.chunks(4096)) {
            writer.write(l, r).unwrap();
        }
        writer.finish().unwrap();
        // 16 coarse bins come from the cached summary; compare a direct scan.
        let WaveformPeaks {
            combined: coarse,
            channels,
            stride,
        } = peaks(&path, frames, 0, frames, 16).unwrap();
        assert_eq!((coarse.len(), stride), (32, frames / 16));
        let mut direct = vec![[(0f32, 0f32); 2]; 16];
        scan(
            &mut WaveReader::open(&path).unwrap(),
            0,
            frames,
            stride,
            &mut direct,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
        for (channel, measured) in channels.iter().enumerate() {
            let expected: Vec<f32> = direct
                .iter()
                .flat_map(|peak| [peak[channel].0, peak[channel].1])
                .collect();
            assert_eq!(*measured, expected);
        }
        assert_eq!(channels[0][1], 0.);
        assert_eq!(channels[1][0], 0.);
        let direct: Vec<f32> = direct
            .into_iter()
            .flat_map(|[l, r]| [l.0.min(r.0), l.1.max(r.1)])
            .collect();
        assert_eq!(coarse, direct);
        assert!((coarse[0] + 0.5).abs() < 1e-4 && coarse[1] > 0.24);
        let WaveformPeaks {
            combined: fine,
            channels,
            stride,
        } = peaks(&path, frames, 1000, 1100, 100).unwrap();
        assert_eq!((fine.len(), stride), (200, 1));
        for (offset, i) in (1000..1100).enumerate() {
            assert_eq!(channels[0][offset * 2], left[i]);
            assert_eq!(channels[0][offset * 2 + 1], 0.);
            assert_eq!(channels[1][offset * 2], 0.);
            assert_eq!(channels[1][offset * 2 + 1], right[i]);
        }
        let _ = std::fs::remove_dir_all(dir);
    }
    fn cache_file() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "soundroom-wave-cache-{}",
            crate::service::new_id().unwrap()
        ));
        std::fs::write(&path, b"fixture").unwrap();
        path
    }
    fn fake_summary() -> Arc<Summary> {
        Arc::new(Summary {
            bin: 256,
            peaks: vec![[(-0.5, 0.5); 2]],
        })
    }
    #[test]
    fn concurrent_cold_requests_share_one_scan_without_blocking_other_assets() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            mpsc, Barrier,
        };
        use std::time::{Duration, Instant};
        let path = cache_file();
        let other = cache_file();
        let cache = Arc::new(SummaryCache::default());
        let scans = Arc::new(AtomicUsize::new(0));
        let ready = Arc::new(Barrier::new(9));
        let (entered, seen) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let wait = Arc::new(Mutex::new(wait));
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let (cache, path, scans, ready, entered, wait) = (
                    cache.clone(),
                    path.clone(),
                    scans.clone(),
                    ready.clone(),
                    entered.clone(),
                    wait.clone(),
                );
                std::thread::spawn(move || {
                    ready.wait();
                    cache
                        .get_or_build(&path, 256, || {
                            scans.fetch_add(1, Ordering::SeqCst);
                            entered.send(()).unwrap();
                            wait.lock()
                                .unwrap()
                                .recv_timeout(Duration::from_secs(5))
                                .unwrap();
                            Ok(fake_summary())
                        })
                        .unwrap()
                })
            })
            .collect();
        ready.wait();
        seen.recv_timeout(Duration::from_secs(2)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while cache.0.lock().unwrap().flights[&path].strong_count() < 8 && Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        let waiters = cache.0.lock().unwrap().flights[&path].strong_count();
        let (done, finished) = mpsc::channel();
        let (other_cache, other_path) = (cache.clone(), other.clone());
        let independent = std::thread::spawn(move || {
            other_cache
                .get_or_build(&other_path, 256, || Ok(fake_summary()))
                .unwrap();
            done.send(()).unwrap();
        });
        let independent_done = finished.recv_timeout(Duration::from_secs(2)).is_ok();
        release.send(()).unwrap();
        let values: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
        independent.join().unwrap();
        assert_eq!(waiters, 8, "All callers reached the shared scan gate");
        assert!(
            independent_done,
            "An unrelated asset must not wait for the cold scan"
        );
        assert_eq!(scans.load(Ordering::SeqCst), 1);
        assert!(values.iter().all(|v| Arc::ptr_eq(v, &values[0])));
        for p in [path, other] {
            std::fs::remove_file(p).unwrap();
        }
    }
    #[test]
    fn full_cache_evicts_only_least_recently_used_summary() {
        let path = cache_file();
        let stamp = stamp(&path).unwrap();
        let (a, b, c) = (PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c"));
        let mut state = CacheState::default();
        state.insert(a.clone(), 256, stamp.clone(), fake_summary(), 32);
        state.insert(b.clone(), 256, stamp.clone(), fake_summary(), 32);
        assert!(state.get(&a, 256, &stamp).is_some());
        state.insert(c.clone(), 256, stamp.clone(), fake_summary(), 32);
        assert_eq!(state.bytes, 32);
        assert_eq!(state.entries.len(), 2);
        assert!(state.get(&a, 256, &stamp).is_some());
        assert!(state.get(&c, 256, &stamp).is_some());
        assert!(!state.entries.contains_key(&b));
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn changed_audio_and_failed_scans_do_not_poison_future_requests() {
        let path = cache_file();
        let cache = SummaryCache::default();
        let first = cache
            .get_or_build(&path, 256, || Ok(fake_summary()))
            .unwrap();
        std::fs::write(&path, b"changed fixture length").unwrap();
        let second = cache
            .get_or_build(&path, 256, || Ok(fake_summary()))
            .unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        std::fs::write(&path, b"changed again").unwrap();
        assert!(cache
            .get_or_build(&path, 256, || Err(protocol("scan failed")))
            .is_err());
        let third = cache
            .get_or_build(&path, 256, || Ok(fake_summary()))
            .unwrap();
        assert!(!Arc::ptr_eq(&second, &third));
        std::fs::write(&path, b"yet another version").unwrap();
        assert!(cache
            .get_or_build(&path, 256, || {
                std::fs::write(&path, b"mutated while scanning").unwrap();
                Ok(fake_summary())
            })
            .is_err());
        assert!(!cache.0.lock().unwrap().entries.contains_key(&path));
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn cache_bounds_small_asset_metadata_and_rejects_invalid_ranges() {
        let path = cache_file();
        let stamp = stamp(&path).unwrap();
        let mut state = CacheState::default();
        for i in 0..CACHED_SUMMARY_ASSETS + 5 {
            state.insert(
                PathBuf::from(i.to_string()),
                0,
                stamp.clone(),
                Arc::new(Summary {
                    bin: 256,
                    peaks: vec![],
                }),
                0,
            );
        }
        assert_eq!(state.entries.len(), CACHED_SUMMARY_ASSETS);
        assert_eq!(state.bytes, 0);
        for (start, end, points) in [(0, 0, 1), (1, 0, 1), (0, 1, 0), (0, 257, 1)] {
            assert!(peaks(&path, 256, start, end, points).is_err());
        }
        std::fs::remove_file(path).unwrap();
    }
}
