//! Bounded ordinary decode/verification worker. Transport remains on the control
//! owner so heartbeats and correlated interrupt receipts can be serviced there.
use crate::{
    client::{exact_keys, exact_keys_optional},
    models::*,
    snapshot::SnapshotAssembler,
    validation::{self, MAX_FRAME_BYTES, MAX_SNAPSHOT_BYTES},
    Error,
};
use serde_json::Value;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TryRecvError},
    Arc,
};
use std::time::Duration;
const RAW_BYTES: usize = 32 * 1024 * 1024;
const RAW_FRAMES: usize = 2048;
const DECODED_BYTES: usize = MAX_SNAPSHOT_BYTES;
struct Reservation {
    bytes: Arc<AtomicUsize>,
    frames: Arc<AtomicUsize>,
    length: usize,
}
impl Reservation {
    fn acquire(
        bytes: &Arc<AtomicUsize>,
        frames: &Arc<AtomicUsize>,
        length: usize,
        limit: usize,
        count: usize,
    ) -> Result<Self, Error> {
        frames
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                n.checked_add(1).filter(|n| *n <= count)
            })
            .map_err(|_| Error::QueueFull)?;
        if bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                n.checked_add(length).filter(|n| *n <= limit)
            })
            .is_err()
        {
            frames.fetch_sub(1, Ordering::AcqRel);
            return Err(Error::QueueFull);
        }
        Ok(Self {
            bytes: bytes.clone(),
            frames: frames.clone(),
            length,
        })
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.bytes.fetch_sub(self.length, Ordering::AcqRel);
        self.frames.fetch_sub(1, Ordering::AcqRel);
    }
}
struct Input {
    bytes: Vec<u8>,
    epoch: u64,
    _reservation: Reservation,
}
pub(crate) enum Decoded {
    Capabilities(Box<RemoteCapabilities>),
    Begin(i64),
    Page,
    Snapshot(Box<RemoteSnapshot>),
    Reply(RemoteReply),
    Event(RemoteEvent, usize),
}
struct Output {
    result: Result<Decoded, Error>,
    epoch: u64,
    _reservation: Reservation,
}
pub(crate) struct DecodeWorker {
    input: SyncSender<Input>,
    output: Receiver<Output>,
    epoch: Arc<AtomicU64>,
    stopped: Arc<AtomicBool>,
    raw_bytes: Arc<AtomicUsize>,
    raw_frames: Arc<AtomicUsize>,
}
impl DecodeWorker {
    pub(crate) fn new() -> Self {
        Self::with_hook(None)
    }
    pub(crate) fn with_hook(hook: Option<Arc<dyn Fn() + Send + Sync>>) -> Self {
        let (input, inputs) = sync_channel::<Input>(RAW_FRAMES);
        let (outputs, output) = sync_channel::<Output>(128);
        let epoch = Arc::new(AtomicU64::new(1));
        let generation = epoch.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let closed = stopped.clone();
        let decoded_bytes = Arc::new(AtomicUsize::new(0));
        let decoded_frames = Arc::new(AtomicUsize::new(0));
        std::thread::spawn(move || {
            let mut assembler = SnapshotAssembler::default();
            let mut current_epoch = 1;
            loop {
                let input = match inputs.recv_timeout(Duration::from_millis(250)) {
                    Ok(input) => input,
                    Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => {
                        if closed.load(Ordering::Acquire) {
                            break;
                        }
                        let next_epoch = generation.load(Ordering::Acquire);
                        if current_epoch != next_epoch {
                            assembler.cancel();
                            current_epoch = next_epoch;
                        }
                        continue;
                    }
                };
                if closed.load(Ordering::Acquire) {
                    break;
                }
                if input.epoch != generation.load(Ordering::Acquire) {
                    continue;
                }
                if current_epoch != input.epoch {
                    assembler.cancel();
                    current_epoch = input.epoch;
                }
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let value: Value = serde_json::from_slice(&input.bytes)?;
                    let size = if value["kind"] == "snapshotEnd" {
                        assembler.next_offset().max(input.bytes.len())
                    } else {
                        input.bytes.len()
                    };
                    if size > DECODED_BYTES {
                        return Err(Error::QueueFull);
                    }
                    let reservation = loop {
                        if closed.load(Ordering::Acquire)
                            || input.epoch != generation.load(Ordering::Acquire)
                        {
                            return Ok(None);
                        }
                        match Reservation::acquire(
                            &decoded_bytes,
                            &decoded_frames,
                            size,
                            DECODED_BYTES,
                            128,
                        ) {
                            Ok(reservation) => break reservation,
                            Err(Error::QueueFull) => std::thread::sleep(Duration::from_millis(1)),
                            Err(error) => return Err(error),
                        }
                    };
                    if let Some(hook) = &hook {
                        hook();
                    }
                    let result = ordinary(value, input.bytes.len(), &mut assembler);
                    Ok(Some(Output {
                        result,
                        epoch: input.epoch,
                        _reservation: reservation,
                    }))
                }))
                .unwrap_or_else(|_| {
                    Err(Error::Protocol(
                        "Control decoder failed; no requests were replayed".into(),
                    ))
                });
                let output = match result {
                    Ok(Some(output)) => output,
                    Ok(None) => continue,
                    Err(error) => {
                        assembler.cancel();
                        let reservation = match Reservation::acquire(
                            &decoded_bytes,
                            &decoded_frames,
                            1,
                            DECODED_BYTES,
                            128,
                        ) {
                            Ok(reservation) => reservation,
                            Err(_) => break,
                        };
                        Output {
                            result: Err(error),
                            epoch: input.epoch,
                            _reservation: reservation,
                        }
                    }
                };
                if closed.load(Ordering::Acquire)
                    || input.epoch != generation.load(Ordering::Acquire)
                {
                    continue;
                }
                if outputs.send(output).is_err() {
                    break;
                }
            }
        });
        Self {
            input,
            output,
            epoch,
            stopped,
            raw_bytes: Arc::new(AtomicUsize::new(0)),
            raw_frames: Arc::new(AtomicUsize::new(0)),
        }
    }
    pub(crate) fn submit(&self, bytes: Vec<u8>) -> Result<(), Error> {
        if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
            return Err(Error::Protocol("Invalid control frame length".into()));
        }
        let reservation = Reservation::acquire(
            &self.raw_bytes,
            &self.raw_frames,
            bytes.len(),
            RAW_BYTES,
            RAW_FRAMES,
        )?;
        self.input
            .try_send(Input {
                bytes,
                epoch: self.epoch.load(Ordering::Acquire),
                _reservation: reservation,
            })
            .map_err(|_| Error::QueueFull)
    }
    pub(crate) fn take(&self) -> Result<Option<Decoded>, Error> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(Error::Disconnected);
        }
        loop {
            match self.output.try_recv() {
                Ok(output) if output.epoch == self.epoch.load(Ordering::Acquire) => {
                    return output.result.map(Some)
                }
                Ok(_) => continue,
                Err(TryRecvError::Empty) => return Ok(None),
                Err(TryRecvError::Disconnected) => return Err(Error::Disconnected),
            }
        }
    }
    pub(crate) fn reset(&self) {
        // An old worker result never installs into a newly authenticated stream.
        // Retained reservations remain charged until their actual objects drop.
        if self
            .epoch
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |epoch| {
                epoch.checked_add(1)
            })
            .is_err()
        {
            self.stopped.store(true, Ordering::Release);
        }
    }
}
impl Drop for DecodeWorker {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}
fn ordinary(
    value: Value,
    bytes: usize,
    assembler: &mut SnapshotAssembler,
) -> Result<Decoded, Error> {
    if value["version"] != 3 {
        return Err(Error::Protocol("Unsupported protocol version".into()));
    }
    match value["kind"].as_str() {
        Some("capabilities") => {
            exact_keys(&value, &["version", "kind", "capabilities"])?;
            validation::validate_model(&value["capabilities"], "RemoteCapabilities")?;
            Ok(Decoded::Capabilities(Box::new(serde_json::from_value(
                value["capabilities"].clone(),
            )?)))
        }
        Some("snapshotBegin" | "snapshotPage" | "snapshotEnd") => {
            let page: RemoteSnapshotPage = serde_json::from_value(value)?;
            let kind = page.kind.clone();
            let revision = page.revision;
            match assembler.accept(page)? {
                Some(snapshot) => Ok(Decoded::Snapshot(Box::new(snapshot))),
                None if kind == Kind::SnapshotBegin => Ok(Decoded::Begin(revision)),
                None => Ok(Decoded::Page),
            }
        }
        Some("reply") => Ok(Decoded::Reply(reply(value)?)),
        Some("event") => {
            exact_keys(&value, &["version", "kind", "event"])?;
            validation::validate_event(&value["event"])?;
            let mut value = value;
            let event: RemoteEvent =
                serde_json::from_value(value.as_object_mut().unwrap().remove("event").unwrap())?;
            Ok(Decoded::Event(event, bytes))
        }
        Some("failure") => {
            exact_keys(&value, &["version", "kind", "message"])?;
            Err(Error::Protocol(
                "Hardware service reported a control failure".into(),
            ))
        }
        _ => Err(Error::Protocol("Unexpected control envelope".into())),
    }
}
pub(crate) fn reply(mut value: Value) -> Result<RemoteReply, Error> {
    exact_keys_optional(
        &value,
        &[
            "version",
            "kind",
            "requestID",
            "status",
            "revision",
            "message",
        ],
        &["payload"],
    )?;
    if value["version"] != 3 {
        return Err(Error::Protocol("Unsupported protocol version".into()));
    }
    value.as_object_mut().unwrap().remove("version");
    value.as_object_mut().unwrap().remove("kind");
    validation::validate_model(&value, "RemoteReply")?;
    Ok(serde_json::from_value(value)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Mutex};
    use std::time::Instant;

    #[test]
    fn weighted_admission_rolls_back_count_on_bytes_or_arithmetic_failure() {
        let bytes = Arc::new(AtomicUsize::new(0));
        let frames = Arc::new(AtomicUsize::new(0));
        let lease =
            Reservation::acquire(&bytes, &frames, DECODED_BYTES, DECODED_BYTES, 128).unwrap();
        assert!(matches!(
            Reservation::acquire(&bytes, &frames, 1, DECODED_BYTES, 128),
            Err(Error::QueueFull)
        ));
        assert_eq!(frames.load(Ordering::Acquire), 1);
        assert_eq!(bytes.load(Ordering::Acquire), DECODED_BYTES);
        drop(lease);
        assert_eq!(bytes.load(Ordering::Acquire), 0);
        assert_eq!(frames.load(Ordering::Acquire), 0);
        bytes.store(usize::MAX, Ordering::Release);
        assert!(Reservation::acquire(&bytes, &frames, 1, usize::MAX, 128).is_err());
        assert_eq!(frames.load(Ordering::Acquire), 0);
        bytes.store(0, Ordering::Release);
        frames.store(usize::MAX, Ordering::Release);
        assert!(Reservation::acquire(&bytes, &frames, 1, 1, usize::MAX).is_err());
        assert_eq!(bytes.load(Ordering::Acquire), 0);
    }
    struct Release(mpsc::SyncSender<()>);
    impl Drop for Release {
        fn drop(&mut self) {
            let _ = self.0.try_send(());
        }
    }
    #[test]
    fn blocked_decoder_has_exact_frame_bound_and_disconnect_releases_actual_bytes() {
        let (entered, ready) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::sync_channel(1);
        let wait = Mutex::new(wait);
        let once = AtomicBool::new(false);
        let worker = DecodeWorker::with_hook(Some(Arc::new(move || {
            if !once.swap(true, Ordering::AcqRel) {
                entered.send(()).unwrap();
                wait.lock().unwrap().recv().unwrap();
            }
        })));
        let release = Release(release);
        worker.submit(b"{}".to_vec()).unwrap();
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        for _ in 1..RAW_FRAMES {
            worker.submit(b"{}".to_vec()).unwrap();
        }
        assert!(matches!(
            worker.submit(b"{}".to_vec()),
            Err(Error::QueueFull)
        ));
        assert_eq!(worker.raw_frames.load(Ordering::Acquire), RAW_FRAMES);
        assert_eq!(worker.raw_bytes.load(Ordering::Acquire), RAW_FRAMES * 2);
        let bytes = worker.raw_bytes.clone();
        let frames = worker.raw_frames.clone();
        drop(worker);
        drop(release);
        let deadline = Instant::now() + Duration::from_secs(2);
        while frames.load(Ordering::Acquire) != 0 {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(bytes.load(Ordering::Acquire), 0);
    }
    #[test]
    fn malformed_decoder_output_remains_fifo_and_generation_overflow_fails_closed() {
        let worker = DecodeWorker::new();
        worker.submit(b"not JSON".to_vec()).unwrap();
        worker
            .submit(
                br#"{"version":3,"kind":"event","event":{"type":"status","message":"After"}}"#
                    .to_vec(),
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match worker.take() {
                Err(Error::Json(_)) => break,
                Ok(None) => {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(1));
                }
                _ => panic!("malformed input must precede later state"),
            }
        }
        loop {
            match worker.take() {
                Ok(Some(Decoded::Event(RemoteEvent::Status { message }, _))) => {
                    assert_eq!(message, "After");
                    break;
                }
                Ok(None) => {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(1));
                }
                _ => panic!("validated following event missing"),
            }
        }
        worker.epoch.store(u64::MAX, Ordering::Release);
        worker.reset();
        assert!(matches!(worker.take(), Err(Error::Disconnected)));
    }
}
