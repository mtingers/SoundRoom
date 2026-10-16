//! Per-authenticated-peer ordinary request decoding. Only small strict interrupt
//! shapes bypass FIFO preparation; workers still enter the same authority/ledger.
use crate::{
    service::{Identity, Outbox, Service, Shared},
    Error,
};
use serde_json::Value;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    mpsc::{sync_channel, SyncSender},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
/// Preserve the authenticated input rate budget without parking the TLS reader.
/// Ordinary frames wait on their bounded worker; interrupt/control lanes have
/// independent bounded admission and never sleep the socket loop.
pub(crate) struct InputRate {
    start: Instant,
    requests: usize,
    bytes: usize,
}
impl Default for InputRate {
    fn default() -> Self {
        Self {
            start: Instant::now(),
            requests: 0,
            bytes: 0,
        }
    }
}
impl InputRate {
    pub(crate) fn reserve(&mut self, bytes: usize, now: Instant) -> Duration {
        if now.saturating_duration_since(self.start) >= Duration::from_secs(1) {
            self.start = now;
            self.requests = 0;
            self.bytes = 0;
        }
        let delay =
            if self.requests >= 120 || bytes > (32 * 1024 * 1024usize).saturating_sub(self.bytes) {
                let delay = (self.start + Duration::from_secs(1)).saturating_duration_since(now);
                self.start = now + delay;
                self.requests = 0;
                self.bytes = 0;
                delay
            } else {
                Duration::ZERO
            };
        self.requests += 1;
        self.bytes += bytes;
        delay
    }
}
const INLINE_BYTES: usize = 1024;
const FRAME_BYTES: usize = 8 * 1024 * 1024;
const GLOBAL_BYTES: usize = 128 * 1024 * 1024;
const PEER_BYTES: usize = 16 * 1024 * 1024;
/// Requests waiting behind the one being handled. A client may pipeline
/// (waveform tiles while transport adopts a graph); bytes stay bounded by
/// `PEER_BYTES`.
const QUEUED_REQUESTS: usize = 32;
struct Budget {
    global: Arc<AtomicUsize>,
    peer: Arc<AtomicUsize>,
    bytes: usize,
}
impl Budget {
    fn reserve(
        global: &Arc<AtomicUsize>,
        peer: &Arc<AtomicUsize>,
        bytes: usize,
    ) -> Result<Self, Error> {
        let reserve = |count: &AtomicUsize, limit| {
            count
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                    current.checked_add(bytes).filter(|next| *next <= limit)
                })
                .map_err(|_| Error::QueueFull)
        };
        reserve(peer, PEER_BYTES)?;
        if let Err(error) = reserve(
            global,
            GLOBAL_BYTES - crate::service::GLOBAL_CONTROL_RESERVE,
        ) {
            peer.fetch_sub(bytes, Ordering::AcqRel);
            return Err(error);
        }
        Ok(Self {
            global: global.clone(),
            peer: peer.clone(),
            bytes,
        })
    }
}
impl Drop for Budget {
    fn drop(&mut self) {
        self.peer.fetch_sub(self.bytes, Ordering::AcqRel);
        self.global.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
struct Work {
    bytes: Vec<u8>,
    transport_generation: u64,
    _budget: Budget,
}
pub(crate) struct RequestDecode {
    sender: SyncSender<Work>,
    global: Arc<AtomicUsize>,
    peer_bytes: Arc<AtomicUsize>,
    stopped: Arc<AtomicBool>,
    failure: Arc<Mutex<Option<String>>>,
    service: Shared,
    peer: Identity,
    out: Outbox,
    transport_generation: Arc<AtomicU64>,
    interrupt_rate: Mutex<InputRate>,
}
fn error(message: &str) -> Error {
    Error::Protocol(message.into())
}
fn interrupt(value: &Value) -> bool {
    matches!(
        value["command"]["type"].as_str(),
        Some("stop" | "cancelJob")
    ) || (value["command"]["type"] == "editor"
        && matches!(
            value["command"]["intent"]["operation"].as_str(),
            Some(
                "jobs.cancel" | "transport.stop" | "transport.pause" | "transport.finishRecording"
            )
        ))
}
fn strict_interrupt(value: &Value) -> Result<(), Error> {
    if !interrupt(value) {
        return Err(error("Expected reserved interrupt"));
    }
    if value
        .get("expectedTrackRevisions")
        .is_some_and(|v| !v.as_object().is_some_and(|map| map.is_empty()))
    {
        return Err(error(
            "Interrupt requests cannot carry track revision payloads",
        ));
    }
    // The normal generated request/command validators check every key and value
    // before dispatch. The byte bound guarantees this traversal remains small.
    Ok(())
}
impl RequestDecode {
    pub(crate) fn new(
        service: Shared,
        peer: Identity,
        out: Outbox,
        global: Arc<AtomicUsize>,
    ) -> Result<Self, Error> {
        Self::start(service, peer, out, global, None)
    }
    fn start(
        service: Shared,
        peer: Identity,
        out: Outbox,
        global: Arc<AtomicUsize>,
        hook: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Result<Self, Error> {
        Self::start_with_rate(service, peer, out, global, hook, InputRate::default())
    }
    fn start_with_rate(
        service: Shared,
        peer: Identity,
        out: Outbox,
        global: Arc<AtomicUsize>,
        hook: Option<Arc<dyn Fn() + Send + Sync>>,
        mut rate: InputRate,
    ) -> Result<Self, Error> {
        let transport_generation = service
            .lock()
            .map_err(|_| error("Service authority is unavailable"))?
            .transport_generation();
        let (sender, receiver) = sync_channel::<Work>(QUEUED_REQUESTS);
        let stopped = Arc::new(AtomicBool::new(false));
        let failure = Arc::new(Mutex::new(None));
        let worker_failure = failure.clone();
        let worker_stop = stopped.clone();
        let worker_service = service.clone();
        let worker_peer = peer.clone();
        let worker_out = out.clone();
        std::thread::spawn(move || {
            while let Ok(work) = receiver.recv() {
                if worker_stop.load(Ordering::Acquire) || worker_out.closed.load(Ordering::Acquire)
                {
                    break;
                }
                if let Some(hook) = &hook {
                    hook();
                }
                if worker_stop.load(Ordering::Acquire) || worker_out.closed.load(Ordering::Acquire)
                {
                    break;
                }
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let delay = rate.reserve(work.bytes.len(), Instant::now());
                    let deadline = Instant::now() + delay;
                    while Instant::now() < deadline {
                        if worker_stop.load(Ordering::Acquire)
                            || worker_out.closed.load(Ordering::Acquire)
                        {
                            return Ok(());
                        }
                        std::thread::sleep(
                            deadline
                                .saturating_duration_since(Instant::now())
                                .min(Duration::from_millis(20)),
                        );
                    }
                    let value: Value = serde_json::from_slice(&work.bytes)?;
                    if interrupt(&value) {
                        return Err(error("Oversized or queued interrupt shape rejected"));
                    }
                    let request = Service::validate_request(&worker_peer, value)?
                        .received_in_transport_generation(work.transport_generation);
                    if request.authored() {
                        loop {
                            if worker_stop.load(Ordering::Acquire)
                                || worker_out.closed.load(Ordering::Acquire)
                            {
                                return Ok(());
                            }
                            let pending = worker_service
                                .lock()
                                .map_err(|_| error("Service authority is unavailable"))?
                                .authored_in_flight(&worker_peer);
                            if !pending {
                                break;
                            }
                            // No owner lock is held while the preceding authored
                            // job prepares, commits, rejects or is cancelled.
                            std::thread::sleep(std::time::Duration::from_millis(2));
                        }
                    }
                    Service::dispatch_validated(&worker_service, &worker_peer, &worker_out, request)
                }))
                .unwrap_or_else(|_| {
                    Err(error(
                        "Request decoder failed; no request was automatically retried",
                    ))
                });
                if let Err(error) = result {
                    if let Ok(mut failure) = worker_failure.lock() {
                        *failure = Some(error.to_string().chars().take(512).collect());
                    }
                    worker_out.close();
                    break;
                }
            }
            // Receiver destruction releases every queued input reservation.
        });
        Ok(Self {
            sender,
            global,
            peer_bytes: Arc::new(AtomicUsize::new(0)),
            stopped,
            failure,
            service,
            peer,
            out,
            transport_generation,
            interrupt_rate: Mutex::new(InputRate::default()),
        })
    }
    pub(crate) fn failure(&self) -> Option<Error> {
        self.failure
            .lock()
            .ok()
            .and_then(|failure| failure.as_ref().map(|message| error(message)))
    }
    /// Heartbeats remain on the TLS reader. Ordinary requests are FIFO; Stop and
    /// job cancellation intentionally overtake ordinary decode/validation work.
    pub(crate) fn route(&self, bytes: Vec<u8>) -> Result<Option<Value>, Error> {
        // Stamp before decoding or waiting for authority. A valid Stop on any
        // peer makes all earlier queued start requests stale at admission.
        let transport_generation = self.transport_generation.load(Ordering::Acquire);
        if bytes.len() > FRAME_BYTES {
            return Err(error("Control frame exceeds 8 MiB"));
        }
        if bytes.len() <= INLINE_BYTES {
            let value: Value = serde_json::from_slice(&bytes)?;
            if value["kind"] != "request" {
                return Ok(Some(value));
            }
            if interrupt(&value) {
                strict_interrupt(&value)?;
                if !self
                    .interrupt_rate
                    .lock()
                    .map_err(|_| error("Interrupt admission is unavailable"))?
                    .reserve(bytes.len(), Instant::now())
                    .is_zero()
                {
                    return Err(Error::QueueFull);
                }
                Service::dispatch(&self.service, &self.peer, &self.out, value)?;
                return Ok(None);
            }
        }
        let budget = Budget::reserve(&self.global, &self.peer_bytes, bytes.len())?;
        self.sender
            .try_send(Work {
                bytes,
                transport_generation,
                _budget: budget,
            })
            .map_err(|_| Error::QueueFull)?;
        Ok(None)
    }
}
impl Drop for RequestDecode {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{private_directory, Registry};
    use crate::service::Inbox;
    use serde_json::json;
    use soundroom_client::{auth::new_id, models::RemoteRole};
    use std::{
        path::PathBuf,
        time::{Duration, Instant},
    };
    struct Fixture {
        root: PathBuf,
        service: Shared,
        peer: Identity,
        out: Outbox,
        inbox: Inbox,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("soundroom-decode-{}", new_id().unwrap()));
            private_directory(&root).unwrap();
            let mut registry = Registry::create(&root, "Decode").unwrap();
            let credential = registry.issue("Operator", RemoteRole::Operator).unwrap();
            let peer = Identity {
                runtime: new_id().unwrap(),
                credential: credential["credentialID"].as_str().unwrap().into(),
                name: "Operator".into(),
                role: RemoteRole::Operator,
            };
            let service = Service::open(&root, None).unwrap();
            let (out, inbox) = Outbox::pair(service.lock().unwrap().global());
            Self {
                root,
                service,
                peer,
                out,
                inbox,
            }
        }
        fn wire(&self, id: &str, command: Value) -> Vec<u8> {
            serde_json::to_vec(
                &json!({"version":3,"kind":"request","id":id,"clientID":self.peer.runtime,
                "expectedTrackRevisions":{},"command":command}),
            )
            .unwrap()
        }
        fn reply(&self) -> Value {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if let Ok(bytes) = self.inbox.priority.try_recv() {
                    self.inbox.received(bytes.len());
                    return serde_json::from_slice(&bytes).unwrap();
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    struct SimulatedCallback {
        stopped: Arc<AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
    }
    impl Drop for SimulatedCallback {
        fn drop(&mut self) {
            self.stopped.store(true, Ordering::Release);
            if let Some(worker) = self.worker.take() {
                worker.join().unwrap();
            }
        }
    }
    fn simulate_audio(fixture: &Fixture) -> SimulatedCallback {
        use soundroom_engine::{
            authority::Authority,
            model::{Session, Track},
        };
        let mut session = Session::default();
        session.tracks.push(Track {
            id: new_id().unwrap(),
            armed: true,
            ..Track::default()
        });
        fixture.service.lock().unwrap().authority = Authority::new(session).unwrap();
        let (control, mut host) = soundroom_engine::realtime::channel();
        Service::attach_audio(&fixture.service, control).unwrap();
        let input = fixture.service.lock().unwrap().simulated_input();
        let stopped = Arc::new(AtomicBool::new(false));
        let callback_stop = stopped.clone();
        let worker = std::thread::spawn(move || {
            let (mut left, mut right, mut frame, mut phase) = ([0.; 128], [0.; 128], 0u64, 0u64);
            while !callback_stop.load(Ordering::Acquire) {
                let host_time = 1_000_000_000_000 + frame * 20_833;
                host.render_timed(
                    &mut left,
                    &mut right,
                    soundroom_engine::capture::CaptureTiming {
                        sample_time: frame as f64,
                        host_time,
                        flags: 3,
                    },
                );
                crate::service::record::push_simulated(&input, &mut phase, host_time, 128);
                frame += 128;
                std::thread::sleep(Duration::from_micros(2667));
            }
        });
        SimulatedCallback {
            stopped,
            worker: Some(worker),
        }
    }
    fn gated_decoder(
        fixture: &Fixture,
    ) -> (RequestDecode, std::sync::mpsc::Receiver<()>, SyncSender<()>) {
        let (entered, ready) = sync_channel(1);
        let (release, blocked) = sync_channel(1);
        let blocked = Mutex::new(blocked);
        let first = AtomicBool::new(true);
        let hook = Arc::new(move || {
            if first.swap(false, Ordering::AcqRel) {
                entered.send(()).unwrap();
                blocked
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
            }
        });
        let global = fixture.service.lock().unwrap().global();
        let decoder = RequestDecode::start(
            fixture.service.clone(),
            fixture.peer.clone(),
            fixture.out.clone(),
            global,
            Some(hook),
        )
        .unwrap();
        (decoder, ready, release)
    }
    fn reply_for(inbox: &Inbox, id: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(bytes) = inbox
                .priority
                .try_recv()
                .ok()
                .or_else(|| inbox.ordinary.try_recv().ok())
            {
                inbox.received(bytes.len());
                let value: Value = serde_json::from_slice(&bytes).unwrap();
                if value["kind"] == "reply" {
                    assert_eq!(
                        value["requestID"], id,
                        "ordinary receipts must retain their order"
                    );
                    return value;
                }
            }
            assert!(Instant::now() < deadline, "Response timeout for {id}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn editor(operation: &str) -> Value {
        json!({"type":"editor","intent":{"operation":operation,"context":{"clipIDs":[],"cursorFrame":0},
            "integers":{},"numbers":{},"strings":{},"flags":{}}})
    }
    #[test]
    fn stop_fences_queued_starts_across_peers_and_preserves_later_requests() {
        for cross_peer in [false, true] {
            for (command, recording) in [
                (json!({"type":"play"}), false),
                (json!({"type":"record"}), true),
                (editor("transport.play"), false),
                (editor("transport.toggle"), false),
                (editor("transport.record"), true),
            ] {
                let fixture = Fixture::new();
                let _callback = simulate_audio(&fixture);
                let (decoder, entered, release) = gated_decoder(&fixture);
                let queued = new_id().unwrap();
                decoder
                    .route(fixture.wire(&queued, command.clone()))
                    .unwrap();
                entered.recv_timeout(Duration::from_secs(2)).unwrap();
                let ping = new_id().unwrap();
                decoder
                    .route(fixture.wire(&ping, json!({"type":"ping","clientTime":1})))
                    .unwrap();
                let mut stopping_peer = fixture.peer.clone();
                stopping_peer.runtime = new_id().unwrap();
                let global = fixture.service.lock().unwrap().global();
                let (other_out, other_inbox) = Outbox::pair(global.clone());
                let other_decoder = RequestDecode::new(
                    fixture.service.clone(),
                    stopping_peer.clone(),
                    other_out,
                    global,
                )
                .unwrap();
                let stop = new_id().unwrap();
                if cross_peer {
                    let mut wire: Value =
                        serde_json::from_slice(&fixture.wire(&stop, json!({"type":"stop"})))
                            .unwrap();
                    wire["clientID"] = json!(stopping_peer.runtime);
                    other_decoder
                        .route(serde_json::to_vec(&wire).unwrap())
                        .unwrap();
                    assert_eq!(reply_for(&other_inbox, &stop)["status"], "accepted");
                } else {
                    decoder
                        .route(fixture.wire(&stop, json!({"type":"stop"})))
                        .unwrap();
                    assert_eq!(reply_for(&fixture.inbox, &stop)["status"], "accepted");
                }
                release.send(()).unwrap();
                let stale = reply_for(&fixture.inbox, &queued);
                assert_eq!(stale["status"], "conflict", "{command}: {stale}");
                assert_eq!(stale["message"], "Transport start was superseded by Stop");
                assert_eq!(reply_for(&fixture.inbox, &ping)["status"], "accepted");
                assert_eq!(
                    fixture.service.lock().unwrap().snapshot().unwrap()["transport"]["isPlaying"],
                    false
                );
                let later = new_id().unwrap();
                decoder.route(fixture.wire(&later, command)).unwrap();
                assert_eq!(reply_for(&fixture.inbox, &later)["status"], "accepted");
                let snapshot = fixture.service.lock().unwrap().snapshot().unwrap();
                // The empty playback graph can reach its end before this read;
                // its accepted receipt already proves callback-confirmed start.
                assert_eq!(snapshot["transport"]["isRecording"], recording);
                let cleanup = new_id().unwrap();
                decoder
                    .route(fixture.wire(&cleanup, json!({"type":"stop"})))
                    .unwrap();
                assert_eq!(reply_for(&fixture.inbox, &cleanup)["status"], "accepted");
            }
        }
    }
    #[test]
    fn malformed_unauthorized_and_unavailable_stop_do_not_fence_starts() {
        let fixture = Fixture::new();
        let global = fixture.service.lock().unwrap().global();
        let decoder = RequestDecode::new(
            fixture.service.clone(),
            fixture.peer.clone(),
            fixture.out.clone(),
            global.clone(),
        )
        .unwrap();
        let unavailable = new_id().unwrap();
        decoder
            .route(fixture.wire(&unavailable, json!({"type":"stop"})))
            .unwrap();
        assert_eq!(reply_for(&fixture.inbox, &unavailable)["status"], "busy");
        assert_eq!(decoder.transport_generation.load(Ordering::Acquire), 0);
        drop(decoder);
        let _callback = simulate_audio(&fixture);
        let (decoder, entered, release) = gated_decoder(&fixture);
        let queued = new_id().unwrap();
        decoder
            .route(fixture.wire(&queued, json!({"type":"record"})))
            .unwrap();
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        let malformed = fixture.wire(&new_id().unwrap(), json!({"type":"stop","unexpected":true}));
        assert!(decoder.route(malformed).is_err());
        let credential = fixture
            .service
            .lock()
            .unwrap()
            .registry
            .issue("Viewer", RemoteRole::Viewer)
            .unwrap();
        let viewer = Identity {
            runtime: new_id().unwrap(),
            credential: credential["credentialID"].as_str().unwrap().into(),
            name: "Viewer".into(),
            role: RemoteRole::Viewer,
        };
        let (viewer_out, viewer_inbox) = Outbox::pair(global.clone());
        let viewer_decoder =
            RequestDecode::new(fixture.service.clone(), viewer.clone(), viewer_out, global)
                .unwrap();
        let unauthorized = new_id().unwrap();
        let mut wire: Value =
            serde_json::from_slice(&fixture.wire(&unauthorized, json!({"type":"stop"}))).unwrap();
        wire["clientID"] = json!(viewer.runtime);
        viewer_decoder
            .route(serde_json::to_vec(&wire).unwrap())
            .unwrap();
        assert_eq!(
            reply_for(&viewer_inbox, &unauthorized)["status"],
            "rejected"
        );
        assert_eq!(decoder.transport_generation.load(Ordering::Acquire), 0);
        release.send(()).unwrap();
        assert_eq!(reply_for(&fixture.inbox, &queued)["status"], "accepted");
        let cleanup = new_id().unwrap();
        decoder
            .route(fixture.wire(&cleanup, json!({"type":"stop"})))
            .unwrap();
        assert_eq!(reply_for(&fixture.inbox, &cleanup)["status"], "accepted");
    }
    #[test]
    fn cached_stop_receipt_does_not_cancel_a_later_queued_recording() {
        let fixture = Fixture::new();
        let _callback = simulate_audio(&fixture);
        let (decoder, entered, release) = gated_decoder(&fixture);
        let stop = new_id().unwrap();
        let stop_wire = fixture.wire(&stop, json!({"type":"stop"}));
        decoder.route(stop_wire.clone()).unwrap();
        let accepted = reply_for(&fixture.inbox, &stop);
        assert_eq!(accepted["status"], "accepted");
        let queued = new_id().unwrap();
        decoder
            .route(fixture.wire(&queued, json!({"type":"record"})))
            .unwrap();
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        decoder.route(stop_wire).unwrap();
        assert_eq!(reply_for(&fixture.inbox, &stop), accepted);
        assert_eq!(decoder.transport_generation.load(Ordering::Acquire), 1);
        release.send(()).unwrap();
        assert_eq!(reply_for(&fixture.inbox, &queued)["status"], "accepted");
        let cleanup = new_id().unwrap();
        decoder
            .route(fixture.wire(&cleanup, json!({"type":"stop"})))
            .unwrap();
        assert_eq!(reply_for(&fixture.inbox, &cleanup)["status"], "accepted");
    }
    #[test]
    fn stop_rechecks_a_start_that_was_validated_before_authority_admission() {
        let fixture = Fixture::new();
        let _callback = simulate_audio(&fixture);
        let global = fixture.service.lock().unwrap().global();
        let decoder = RequestDecode::new(
            fixture.service.clone(),
            fixture.peer.clone(),
            fixture.out.clone(),
            global,
        )
        .unwrap();
        let start = new_id().unwrap();
        let value: Value =
            serde_json::from_slice(&fixture.wire(&start, json!({"type":"play"}))).unwrap();
        let prepared = Service::validate_request(&fixture.peer, value)
            .unwrap()
            .received_in_transport_generation(decoder.transport_generation.load(Ordering::Acquire));
        let stop = new_id().unwrap();
        decoder
            .route(fixture.wire(&stop, json!({"type":"stop"})))
            .unwrap();
        assert_eq!(reply_for(&fixture.inbox, &stop)["status"], "accepted");
        Service::dispatch_validated(&fixture.service, &fixture.peer, &fixture.out, prepared)
            .unwrap();
        assert_eq!(reply_for(&fixture.inbox, &start)["status"], "conflict");
        assert_eq!(
            fixture.service.lock().unwrap().snapshot().unwrap()["transport"]["isPlaying"],
            false
        );
    }
    #[test]
    fn reserved_stop_overtakes_blocked_ordinary_decode_without_reordering_ordinary_receipts() {
        let fixture = Fixture::new();
        let global = fixture.service.lock().unwrap().global();
        let (entered, ready) = sync_channel(1);
        let (release, wait) = sync_channel(1);
        let wait = Mutex::new(wait);
        let hook = Arc::new(move || {
            entered.send(()).unwrap();
            wait.lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
        });
        let decoder = RequestDecode::start(
            fixture.service.clone(),
            fixture.peer.clone(),
            fixture.out.clone(),
            global.clone(),
            Some(hook),
        )
        .unwrap();
        let first = new_id().unwrap();
        let second = new_id().unwrap();
        let stop = new_id().unwrap();
        let mut bytes = fixture.wire(&first, json!({"type":"ping","clientTime":1}));
        bytes.resize(4096, b' ');
        assert!(decoder.route(bytes).unwrap().is_none());
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        decoder
            .route(fixture.wire(&second, json!({"type":"ping","clientTime":2})))
            .unwrap();
        decoder
            .route(fixture.wire(&stop, json!({"type":"stop"})))
            .unwrap();
        let reply = fixture.reply();
        assert_eq!(reply["requestID"], stop);
        // No hardware backend was attached: this proves reserved dispatch, not
        // callback acceptance. The separate service callback test covers that.
        assert_eq!(reply["status"], "busy");
        assert_eq!(fixture.service.lock().unwrap().authority.revision, 0);
        release.send(()).unwrap();
        assert_eq!(fixture.reply()["requestID"], first);
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        release.send(()).unwrap();
        assert_eq!(fixture.reply()["requestID"], second);
        let deadline = Instant::now() + Duration::from_secs(2);
        while global.load(Ordering::Acquire) != 0 {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    #[test]
    fn same_peer_authored_jobs_commit_in_wire_order_while_stop_bypasses_wait() {
        let fixture = Fixture::new();
        let global = fixture.service.lock().unwrap().global();
        let decoder = RequestDecode::new(
            fixture.service.clone(),
            fixture.peer.clone(),
            fixture.out.clone(),
            global.clone(),
        )
        .unwrap();
        let (entered, ready) = sync_channel(1);
        let (release, wait) = sync_channel(1);
        let wait = Mutex::new(wait);
        fixture.service.lock().unwrap().edit_hook = Some(Arc::new(move || {
            entered.send(()).unwrap();
            wait.lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
        }));
        let first = new_id().unwrap();
        let second = new_id().unwrap();
        let stop = new_id().unwrap();
        let authored = |id: &str, revision, name| {
            let mut value: Value =
                serde_json::from_slice(&fixture.wire(id, json!({"type":"addTrack","name":name})))
                    .unwrap();
            value["expectedRevision"] = json!(revision);
            serde_json::to_vec(&value).unwrap()
        };
        decoder.route(authored(&first, 0, "First")).unwrap();
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        fixture.service.lock().unwrap().edit_hook = None;
        decoder.route(authored(&second, 1, "Second")).unwrap();
        decoder
            .route(fixture.wire(&stop, json!({"type":"stop"})))
            .unwrap();
        let reply = fixture.reply();
        assert_eq!(reply["requestID"], stop);
        assert_eq!(reply["status"], "busy");
        assert_eq!(fixture.service.lock().unwrap().authority.revision, 0);
        assert!(fixture.inbox.priority.try_recv().is_err());
        release.send(()).unwrap();
        let first_reply = fixture.reply();
        assert_eq!(first_reply["requestID"], first);
        assert_eq!(first_reply["status"], "accepted", "{first_reply}");
        let second_reply = fixture.reply();
        assert_eq!(second_reply["requestID"], second);
        assert_eq!(second_reply["status"], "accepted", "{second_reply}");
        let state = fixture.service.lock().unwrap();
        assert_eq!(state.authority.revision, 2);
        assert_eq!(
            state
                .authority
                .session
                .tracks
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>(),
            ["First", "Second"]
        );
        drop(state);
    }
    #[test]
    fn raw_queue_is_bounded_and_releases_rejected_reservations() {
        let fixture = Fixture::new();
        let global = fixture.service.lock().unwrap().global();
        let (entered, ready) = sync_channel(1);
        let (release, wait) = sync_channel(1);
        let wait = Mutex::new(wait);
        let hook = Arc::new(move || {
            entered.send(()).unwrap();
            wait.lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
        });
        let decoder = RequestDecode::start(
            fixture.service.clone(),
            fixture.peer.clone(),
            fixture.out.clone(),
            global.clone(),
            Some(hook),
        )
        .unwrap();
        let mut bytes = fixture.wire(&new_id().unwrap(), json!({"type":"ping","clientTime":0}));
        bytes.resize(4096, b' ');
        decoder.route(bytes.clone()).unwrap();
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        for _ in 0..QUEUED_REQUESTS {
            decoder.route(bytes.clone()).unwrap();
        }
        let held = 4096 * (QUEUED_REQUESTS + 1);
        assert_eq!(global.load(Ordering::Acquire), held);
        assert!(matches!(decoder.route(bytes), Err(Error::QueueFull)));
        assert_eq!(global.load(Ordering::Acquire), held);
        drop(decoder);
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while global.load(Ordering::Acquire) != 0 {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(fixture.inbox.priority.try_recv().is_err());
    }
    #[test]
    fn malformed_interrupt_cannot_enter_owner_and_oversized_shape_never_dispatches() {
        let fixture = Fixture::new();
        let global = fixture.service.lock().unwrap().global();
        let decoder = RequestDecode::new(
            fixture.service.clone(),
            fixture.peer.clone(),
            fixture.out.clone(),
            global,
        )
        .unwrap();
        // Holding the owner proves the validator must reject before taking it.
        let guard = fixture.service.lock().unwrap();
        let invalid = fixture.wire(&new_id().unwrap(), json!({"type":"stop","unexpected":true}));
        assert!(decoder.route(invalid).is_err());
        let mut invalid: Value =
            serde_json::from_slice(&fixture.wire(&new_id().unwrap(), json!({"type":"stop"})))
                .unwrap();
        invalid["expectedTrackRevisions"] = json!({new_id().unwrap():0});
        assert!(decoder
            .route(serde_json::to_vec(&invalid).unwrap())
            .is_err());
        drop(guard);
        let mut oversized = fixture.wire(&new_id().unwrap(), json!({"type":"stop"}));
        oversized.resize(2048, b' ');
        decoder.route(oversized).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !fixture.out.closed.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(fixture.inbox.priority.try_recv().is_err());
        assert_eq!(fixture.service.lock().unwrap().authority.revision, 0);
    }
    #[test]
    fn global_rejection_rolls_back_peer_and_budget_caps_are_exact() {
        let global = Arc::new(AtomicUsize::new(GLOBAL_BYTES));
        let peer = Arc::new(AtomicUsize::new(0));
        assert!(Budget::reserve(&global, &peer, 1).is_err());
        assert_eq!(peer.load(Ordering::Acquire), 0);
        global.store(0, Ordering::Release);
        let held = Budget::reserve(&global, &peer, PEER_BYTES).unwrap();
        assert!(Budget::reserve(&global, &peer, 1).is_err());
        assert_eq!(global.load(Ordering::Acquire), PEER_BYTES);
        drop(held);
        assert_eq!(global.load(Ordering::Acquire), 0);
        assert_eq!(peer.load(Ordering::Acquire), 0);
    }
    #[test]
    fn gui_stop_overtakes_blocked_ordinary_decode() {
        let f = Fixture::new();
        let global = f.service.lock().unwrap().global();
        let (entered, ready) = sync_channel(1);
        let (release, wait) = sync_channel(1);
        let wait = Mutex::new(wait);
        let first = AtomicBool::new(true);
        let hook = Arc::new(move || {
            if first.swap(false, Ordering::AcqRel) {
                entered.send(()).unwrap();
                wait.lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
            }
        });
        let decoder = RequestDecode::start(
            f.service.clone(),
            f.peer.clone(),
            f.out.clone(),
            global,
            Some(hook),
        )
        .unwrap();
        let ping = new_id().unwrap();
        decoder
            .route(f.wire(&ping, json!({"type":"ping","clientTime":0})))
            .unwrap();
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        for operation in [
            "transport.stop",
            "transport.pause",
            "transport.finishRecording",
        ] {
            let gui_stop = new_id().unwrap();
            decoder.route(f.wire(&gui_stop,json!({"type":"editor","intent":{"operation":operation,"context":{"clipIDs":[],"cursorFrame":0},"integers":{},"numbers":{},"strings":{},"flags":{}}}))).unwrap();
            assert_eq!(
                f.reply()["requestID"],
                gui_stop,
                "GUI Stop must bypass blocked ordinary decode"
            );
        }
        release.send(()).unwrap();
        assert_eq!(f.reply()["requestID"], ping);
    }
    #[test]
    fn input_rate_retains_byte_and_request_budgets_without_sleeping() {
        let now = Instant::now();
        let mut rate = InputRate {
            start: now,
            requests: 0,
            bytes: 0,
        };
        assert_eq!(rate.reserve(32 * 1024 * 1024, now), Duration::ZERO);
        assert_eq!(
            rate.reserve(1, now + Duration::from_millis(100)),
            Duration::from_millis(900)
        );
        let mut rate = InputRate {
            start: now,
            requests: 0,
            bytes: 0,
        };
        for _ in 0..120 {
            assert_eq!(rate.reserve(1, now), Duration::ZERO);
        }
        assert_eq!(rate.reserve(1, now), Duration::from_secs(1));
        assert_eq!(
            rate.reserve(1, now + Duration::from_secs(2)),
            Duration::ZERO
        );
    }
    #[test]
    fn reserved_stop_overtakes_a_rate_delayed_ordinary_request() {
        let f = Fixture::new();
        let (entered, ready) = sync_channel(1);
        let first = AtomicBool::new(true);
        let hook = Arc::new(move || {
            if first.swap(false, Ordering::AcqRel) {
                entered.send(()).unwrap();
            }
        });
        let rate = InputRate {
            start: Instant::now(),
            requests: 120,
            bytes: 0,
        };
        let global = f.service.lock().unwrap().global();
        let decoder = RequestDecode::start_with_rate(
            f.service.clone(),
            f.peer.clone(),
            f.out.clone(),
            global,
            Some(hook),
            rate,
        )
        .unwrap();
        let ping = new_id().unwrap();
        decoder
            .route(f.wire(&ping, json!({"type":"ping","clientTime":0})))
            .unwrap();
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        let stop = new_id().unwrap();
        decoder
            .route(f.wire(&stop, json!({"type":"stop"})))
            .unwrap();
        assert_eq!(
            f.reply()["requestID"],
            stop,
            "A throttled upload must not delay Stop behind the socket reader"
        );
        assert_eq!(f.reply()["requestID"], ping);
    }
}
