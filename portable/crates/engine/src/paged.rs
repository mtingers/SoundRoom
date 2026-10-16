//! Bounded worker-decoded read-ahead. Callback reads pin fixed page storage;
//! file handles, decoding, eviction and decoder locks belong only to workers.
use crate::{
    project::{Media, Project},
    wave::WaveReader,
    Error, Result,
};
use std::{
    cell::UnsafeCell,
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
pub(crate) const PAGE: usize = 8192;
const WAYS: usize = 8;
const WRITING: usize = usize::MAX;
const READER_BUDGET: usize = 32 * 1024 * 1024;
fn hash(mut x: u64) -> usize {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58476d1ce4e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d049bb133111eb);
    (x ^ (x >> 31)) as usize
}
pub(crate) fn key(source: usize, page: u64) -> u64 {
    ((source as u64) << 40 | page) + 1
}
fn unpack(value: u64) -> (usize, u64) {
    let value = value - 1;
    ((value >> 40) as usize, value & ((1 << 40) - 1))
}
struct PageSlot {
    state: AtomicUsize,
    key: AtomicU64,
    used: AtomicU64,
    audio: UnsafeCell<[Vec<f32>; 2]>,
}
// PCM is written only while state=WRITING, held by the unique loader. Readers
// increment READY's count before dereferencing and release it afterward. Worker
// eviction can acquire WRITING only from unpinned READY/EMPTY.
unsafe impl Sync for PageSlot {}
pub(crate) struct Cache {
    pages: Vec<PageSlot>,
    requests: Vec<AtomicU64>,
    clock: AtomicU64,
    file_reads: Arc<AtomicU64>,
    decoded: AtomicU64,
    dropped: AtomicU64,
}
impl Cache {
    fn new(budget: usize, file_reads: Arc<AtomicU64>) -> Result<Self> {
        if !(WAYS * PAGE * 8..=64 * 1024 * 1024).contains(&budget) {
            return Err(Error::Invalid(
                "Page PCM budget must be 512 KiB through 64 MiB".into(),
            ));
        }
        let maximum = budget / (PAGE * 8);
        let slots = 1usize << (usize::BITS - 1 - maximum.leading_zeros());
        let pages = (0..slots)
            .map(|_| PageSlot {
                state: AtomicUsize::new(0),
                key: AtomicU64::new(0),
                used: AtomicU64::new(0),
                audio: UnsafeCell::new([vec![0.; PAGE], vec![0.; PAGE]]),
            })
            .collect();
        let requests = (0..(slots * 4).max(64))
            .map(|_| AtomicU64::new(0))
            .collect();
        Ok(Self {
            pages,
            requests,
            clock: AtomicU64::new(1),
            file_reads,
            decoded: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        })
    }
    fn bucket(&self, key: u64) -> &[PageSlot] {
        let index = (hash(key) & (self.pages.len() / WAYS - 1)) * WAYS;
        &self.pages[index..index + WAYS]
    }
    fn contains(&self, key: u64) -> bool {
        self.bucket(key).iter().any(|s| {
            let state = s.state.load(Ordering::Acquire);
            state != 0 && state != WRITING && s.key.load(Ordering::Relaxed) == key
        })
    }
    fn request(&self, key: u64) {
        if self.contains(key) {
            return;
        }
        let index = (hash(key) & (self.requests.len() / WAYS - 1)) * WAYS;
        for request in &self.requests[index..index + WAYS] {
            let previous = request.load(Ordering::Relaxed);
            if previous == key {
                return;
            }
            if previous == 0
                && request
                    .compare_exchange(0, key, Ordering::Release, Ordering::Relaxed)
                    .is_ok()
            {
                return;
            }
        }
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }
    fn copy(&self, key: u64, offset: usize, left: &mut [f32], right: &mut [f32]) -> bool {
        for slot in self.bucket(key) {
            if slot.key.load(Ordering::Relaxed) != key {
                continue;
            }
            for _ in 0..4 {
                let state = slot.state.load(Ordering::Acquire);
                if state == 0 || state == WRITING || state == WRITING - 1 {
                    break;
                }
                if slot
                    .state
                    .compare_exchange(state, state + 1, Ordering::Acquire, Ordering::Relaxed)
                    .is_err()
                {
                    continue;
                }
                // Recheck after pinning: eviction may have changed the tag between
                // the preliminary key read and the successful ownership exchange.
                let matches = slot.key.load(Ordering::Relaxed) == key;
                if matches {
                    slot.used.store(
                        self.clock.fetch_add(1, Ordering::Relaxed),
                        Ordering::Relaxed,
                    );
                    let audio = unsafe { &*slot.audio.get() };
                    left.copy_from_slice(&audio[0][offset..offset + left.len()]);
                    right.copy_from_slice(&audio[1][offset..offset + right.len()]);
                }
                slot.state.fetch_sub(1, Ordering::Release);
                if matches {
                    return true;
                }
                break;
            }
        }
        false
    }
}
struct Decoder {
    reader: WaveReader,
    used: u64,
    bytes: usize,
}
struct WorkerFiles {
    media: Vec<Media>,
    readers: BTreeMap<usize, Decoder>,
    clock: u64,
    used_bytes: usize,
    file_reads: Arc<AtomicU64>,
}
impl WorkerFiles {
    fn read(
        &mut self,
        source: usize,
        start: u64,
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<()> {
        if !self.readers.contains_key(&source) {
            let reader = WaveReader::open(&self.media[source].path)?;
            let bytes = PAGE * reader.info.block_align as usize;
            while self.readers.len() >= 128 || self.used_bytes + bytes > READER_BUDGET {
                let id = *self.readers.iter().min_by_key(|(_, v)| v.used).unwrap().0;
                self.used_bytes -= self.readers.remove(&id).unwrap().bytes;
            }
            self.used_bytes += bytes;
            self.readers.insert(
                source,
                Decoder {
                    reader,
                    used: 0,
                    bytes,
                },
            );
        }
        self.clock = self.clock.wrapping_add(1);
        let decoder = self.readers.get_mut(&source).unwrap();
        decoder.used = self.clock;
        decoder.reader.read_stereo(start, left, right)?;
        self.file_reads.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}
#[derive(Clone)]
pub(crate) struct PagedSource {
    pub(crate) cache: Arc<Cache>,
    files: Arc<Mutex<WorkerFiles>>,
    source: usize,
    frames: u64,
}
impl PagedSource {
    pub(crate) fn source_index(&self) -> usize {
        self.source
    }
    pub(crate) fn read_worker(
        &self,
        start: u64,
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<()> {
        self.files
            .lock()
            .map_err(|_| Error::Conflict("Worker decoder pool poisoned".into()))?
            .read(self.source, start, left, right)
    }
    pub(crate) fn read_callback(&self, start: u64, left: &mut [f32], right: &mut [f32]) -> bool {
        let mut offset = 0;
        let mut valid = true;
        while offset < left.len() {
            let frame = start + offset as u64;
            let page = frame / PAGE as u64;
            let within = (frame % PAGE as u64) as usize;
            let count = (left.len() - offset).min(PAGE - within);
            valid &= self.cache.copy(
                key(self.source, page),
                within,
                &mut left[offset..offset + count],
                &mut right[offset..offset + count],
            );
            // Demand plus roughly half a second of page read-ahead. Posting is
            // bounded, duplicate-aware, allocation-free and independent of file I/O.
            for ahead in 0..=3 {
                let wanted = page + ahead;
                if wanted * (PAGE as u64) < self.frames {
                    self.cache.request(key(self.source, wanted));
                }
            }
            offset += count;
        }
        valid
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageStats {
    pub pcm_bytes: usize,
    pub decoded_pages: u64,
    pub worker_file_reads: u64,
    pub dropped_requests: u64,
}
/// Unique control/IO-worker loader. It is deliberately not Clone: page writes
/// and eviction are serialized here, independently of callback reader pins.
pub struct PagedLoader {
    pub(crate) cache: Arc<Cache>,
    files: Arc<Mutex<WorkerFiles>>,
    ids: BTreeMap<String, usize>,
    cursor: usize,
}
impl PagedLoader {
    pub(crate) fn new(project: &Project, budget: usize) -> Result<Self> {
        let active = project.session.playback_asset_ids();
        if active.len() >= (1 << 20) {
            return Err(Error::Invalid(
                "Paged source table exceeds admission bound".into(),
            ));
        }
        let file_reads = Arc::new(AtomicU64::new(0));
        let cache = Arc::new(Cache::new(budget, file_reads.clone())?);
        let ids = active
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), i))
            .collect();
        let files = Arc::new(Mutex::new(WorkerFiles {
            media: active.iter().map(|id| project.media[id].clone()).collect(),
            readers: BTreeMap::new(),
            clock: 0,
            used_bytes: 0,
            file_reads,
        }));
        Ok(Self {
            cache,
            files,
            ids,
            cursor: 0,
        })
    }
    pub(crate) fn sources(&self) -> impl Iterator<Item = (String, PagedSource)> + '_ {
        self.ids.keys().map(|id| (id.clone(), self.source(id)))
    }
    pub(crate) fn source(&self, id: &str) -> PagedSource {
        let source = self.ids[id];
        let frames = self.files.lock().unwrap().media[source].info.frames;
        PagedSource {
            cache: self.cache.clone(),
            files: self.files.clone(),
            source,
            frames,
        }
    }
    pub fn stats(&self) -> PageStats {
        PageStats {
            pcm_bytes: self.cache.pages.len() * PAGE * 8,
            decoded_pages: self.cache.decoded.load(Ordering::Relaxed),
            worker_file_reads: self.cache.file_reads.load(Ordering::Relaxed),
            dropped_requests: self.cache.dropped.load(Ordering::Relaxed),
        }
    }
    pub(crate) fn capacity(&self) -> usize {
        self.cache.pages.len()
    }
    pub(crate) fn warm(
        &mut self,
        keys: &[u64],
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<()> {
        if keys.len() > self.capacity() {
            return Err(Error::Invalid(
                "Prepared source working set exceeds page budget".into(),
            ));
        }
        for &key in keys {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Conflict("Read-ahead preparation cancelled".into()));
            }
            if !self.load(key)? {
                return Err(Error::Conflict(
                    "Page pin temporarily prevents worker preparation".into(),
                ));
            }
        }
        if keys.iter().any(|&key| !self.cache.contains(key)) {
            return Err(Error::Invalid(
                "Prepared source working set exceeds cache associativity".into(),
            ));
        }
        Ok(())
    }
    pub fn prepare_graph(
        &mut self,
        graph: &crate::render::PreparedGraph,
        frames: usize,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<()> {
        let keys = graph.read_ahead_keys(&self.cache, frames, self.capacity())?;
        self.warm(&keys, cancel)
    }
    fn load(&mut self, key: u64) -> Result<bool> {
        if self.cache.contains(key) {
            return Ok(true);
        }
        let slot = self
            .cache
            .bucket(key)
            .iter()
            .filter(|s| matches!(s.state.load(Ordering::Acquire), 0 | 1))
            .min_by_key(|s| s.used.load(Ordering::Relaxed));
        let Some(slot) = slot else {
            return Ok(false);
        };
        let state = slot.state.load(Ordering::Acquire);
        if state > 1
            || slot
                .state
                .compare_exchange(state, WRITING, Ordering::AcqRel, Ordering::Relaxed)
                .is_err()
        {
            return Ok(false);
        }
        let (source, page) = unpack(key);
        let result = (|| {
            let mut files = self
                .files
                .lock()
                .map_err(|_| Error::Conflict("Worker decoder pool poisoned".into()))?;
            let start = page * PAGE as u64;
            let frames = files
                .media
                .get(source)
                .ok_or_else(|| Error::Invalid("Page source index is out of range".into()))?
                .info
                .frames;
            let remaining = frames
                .checked_sub(start)
                .filter(|&v| v != 0)
                .ok_or_else(|| Error::Invalid("Page lies outside its source".into()))?;
            let count = remaining.min(PAGE as u64) as usize;
            let audio = unsafe { &mut *slot.audio.get() };
            audio[0].fill(0.);
            audio[1].fill(0.);
            let [left, right] = audio;
            files.read(source, start, &mut left[..count], &mut right[..count])
        })();
        match result {
            Ok(()) => {
                slot.key.store(key, Ordering::Relaxed);
                slot.used.store(
                    self.cache.clock.fetch_add(1, Ordering::Relaxed),
                    Ordering::Relaxed,
                );
                slot.state.store(1, Ordering::Release);
                self.cache.decoded.fetch_add(1, Ordering::Relaxed);
                Ok(true)
            }
            Err(error) => {
                slot.state.store(0, Ordering::Release);
                Err(error)
            }
        }
    }
    /// Call frequently on a dedicated worker while playback runs. Each pump is
    /// bounded; decoder buffers/file handles are an LRU bounded to 32 MiB/128 files.
    pub fn service_requests(
        &mut self,
        maximum: usize,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<usize> {
        if maximum > 1024 {
            return Err(Error::Invalid(
                "Read-ahead pump exceeds the 1024-request bound".into(),
            ));
        }
        let mut attempts = 0;
        let mut decoded = 0;
        for _ in 0..self.cache.requests.len() {
            if attempts == maximum {
                break;
            }
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Conflict("Read-ahead pump cancelled".into()));
            }
            let index = self.cursor;
            self.cursor = (self.cursor + 1) % self.cache.requests.len();
            let key = self.cache.requests[index].swap(0, Ordering::AcqRel);
            if key != 0 {
                attempts += 1;
                if self.load(key)? {
                    decoded += 1;
                } else {
                    self.cache.request(key);
                }
            }
        }
        Ok(decoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wave::WaveWriter;
    use std::sync::{atomic::AtomicBool, Barrier};

    #[test]
    fn worker_decoder_pool_enforces_file_and_byte_budgets() {
        fn pool(media: Media, count: usize) -> WorkerFiles {
            WorkerFiles {
                media: vec![media; count],
                readers: BTreeMap::new(),
                clock: 0,
                used_bytes: 0,
                file_reads: Arc::new(AtomicU64::new(0)),
            }
        }
        let path = std::env::temp_dir().join(format!(
            "soundroom-decoder-budget-{}.wav",
            std::process::id()
        ));
        let mut writer = WaveWriter::create(&path, 1).unwrap();
        writer.write(&[0.1], &[-0.1]).unwrap();
        writer.finish().unwrap();
        let media = Media {
            path: path.clone(),
            info: WaveReader::open(&path).unwrap().info,
        };
        let mut files = pool(media, 130);
        let mut left = [0.];
        let mut right = left;
        for id in 0..130 {
            files.read(id, 0, &mut left, &mut right).unwrap();
        }
        assert_eq!(files.readers.len(), 128);
        assert!(files.used_bytes <= READER_BUDGET);
        assert!(!files.readers.contains_key(&0));
        drop(files);
        // The widest admitted WAV allocates a 2 MiB decoder scratch buffer.
        // Seventeen logical sources must evict on bytes before the handle cap.
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[22..24].copy_from_slice(&64_u16.to_le_bytes());
        bytes[28..32].copy_from_slice(&(48_000_u32 * 256).to_le_bytes());
        bytes[32..34].copy_from_slice(&256_u16.to_le_bytes());
        bytes[4..8].copy_from_slice(&292_u32.to_le_bytes());
        bytes[40..44].copy_from_slice(&256_u32.to_le_bytes());
        bytes.resize(44 + 256, 0);
        std::fs::write(&path, &bytes).unwrap();
        let media = Media {
            path: path.clone(),
            info: WaveReader::open(&path).unwrap().info,
        };
        let mut files = pool(media, 17);
        for id in 0..17 {
            files.read(id, 0, &mut left, &mut right).unwrap();
        }
        assert_eq!(files.readers.len(), 16);
        assert_eq!(files.used_bytes, READER_BUDGET);
        assert!(!files.readers.contains_key(&0));
        drop(files);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn concurrent_reader_pins_never_observe_replaced_page_pcm() {
        let path =
            std::env::temp_dir().join(format!("soundroom-page-pins-{}.wav", std::process::id()));
        let frames = 32 * PAGE as u64;
        let mut writer = WaveWriter::create(&path, frames).unwrap();
        for page in 0..32 {
            let value = (page + 1) as f32 / 64.;
            writer.write(&[value; PAGE], &[-value; PAGE]).unwrap();
        }
        writer.finish().unwrap();
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/modern-gain.soundroom");
        let mut project = Project::open(&fixture).unwrap();
        let id = project.session.tracks[0].clips[0].asset_id.to_uppercase();
        let media = project.media.get_mut(&id).unwrap();
        media.path = path.clone();
        media.info = WaveReader::open(&path).unwrap().info;
        let mut loader = PagedLoader::new(&project, WAYS * PAGE * 8).unwrap();
        let source = loader.source(&id);
        for page in 0..WAYS as u64 {
            assert!(loader.load(key(source.source, page)).unwrap());
        }
        // One eight-way bucket forces repeated eviction. Two simultaneous readers
        // exercise the reference count, tag recheck and writer exclusion protocol.
        let barrier = Arc::new(Barrier::new(3));
        let finished = Arc::new(AtomicUsize::new(0));
        struct Completion(Arc<AtomicUsize>);
        impl Drop for Completion {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Release);
            }
        }
        let mut readers = Vec::new();
        for reader in 0..2 {
            let source = source.clone();
            let barrier = barrier.clone();
            let finished = finished.clone();
            readers.push(std::thread::spawn(move || {
                let _completion = Completion(finished);
                let mut left = [0.; 257];
                let mut right = left;
                let mut hits = 0;
                barrier.wait();
                for i in 0..20_000 {
                    let page = ((i / 16 + reader) % 32) as u64;
                    let offset = (i * 131) % (PAGE - left.len());
                    if source.read_callback(
                        page * PAGE as u64 + offset as u64,
                        &mut left,
                        &mut right,
                    ) {
                        let value = (page + 1) as f32 / 64.;
                        assert!(left.iter().all(|&v| v == value));
                        assert!(right.iter().all(|&v| v == -value));
                        hits += 1;
                    }
                    std::thread::yield_now();
                }
                hits
            }));
        }
        barrier.wait();
        let cancel = AtomicBool::new(false);
        while finished.load(Ordering::Acquire) < 2 {
            loader.service_requests(64, &cancel).unwrap();
            std::thread::yield_now();
        }
        for reader in readers {
            assert!(reader.join().unwrap() > 100);
        }
        assert!(loader.stats().decoded_pages > 32);
        assert_eq!(loader.stats().pcm_bytes, WAYS * PAGE * 8);
        assert!(loader.load(key(source.source, 32)).is_err());
        drop(source);
        drop(loader);
        std::fs::remove_file(path).unwrap();
    }
}
