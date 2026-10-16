use super::*;
use base64::Engine;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc, Arc, Condvar, Mutex,
};

const ID: &str = "12345678-1234-4234-8234-123456789ABC";
#[derive(Default)]
struct Shared {
    incoming: VecDeque<Vec<u8>>,
    sent: Vec<Value>,
}
struct Wire(Arc<Mutex<Shared>>);
impl ControlTransport for Wire {
    fn send(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.0
            .lock()
            .unwrap()
            .sent
            .push(serde_json::from_slice(bytes)?);
        Ok(())
    }
    fn receive(&mut self, _: Duration) -> Result<Option<Vec<u8>>, Error> {
        Ok(self.0.lock().unwrap().incoming.pop_front())
    }
    fn close(&mut self) -> Result<(), Error> {
        Ok(())
    }
}
fn frame(shared: &Arc<Mutex<Shared>>, value: Value) {
    shared
        .lock()
        .unwrap()
        .incoming
        .push_back(serde_json::to_vec(&value).unwrap());
}
fn snapshot(revision: i64) -> Value {
    json!({"version":3,"kind":"event","event":{"type":"snapshot","snapshot":{
        "serverID":ID,"serverName":"Host","session":{"version":9,"name":"Song","tracks":[]},
        "revision":revision,"trackRevisions":{},"transport":{"frame":0,"isPlaying":false,
        "isRecording":false,"isPreparing":false,"hostTime":0},"devices":[],"inputDeviceID":0,
        "outputDeviceID":0,"assets":[],"status":"","peers":[]}}})
}
fn attach() -> (Client<Wire>, Arc<Mutex<Shared>>) {
    let shared = Arc::new(Mutex::new(Shared::default()));
    frame(
        &shared,
        json!({"version":3,"kind":"challenge","serverID":ID,"nonce":
        base64::engine::general_purpose::STANDARD.encode([0;32])}),
    );
    frame(
        &shared,
        json!({"version":3,"kind":"welcome","serverID":ID,"name":"Host","role":"editor"}),
    );
    let credential = Credential::from_bytes(&serde_json::to_vec(&json!({"version":3,
        "serverID":ID,"credentialID":ID,"clientSecret":base64::engine::general_purpose::STANDARD.encode([0;32]),
        "role":"editor"})).unwrap()).unwrap();
    (
        Client::attach(Wire(shared.clone()), credential, "Editor", Some(ID.into())).unwrap(),
        shared,
    )
}
struct Gate {
    ready: mpsc::Receiver<()>,
    release: Arc<(Mutex<bool>, Condvar)>,
}
impl Gate {
    fn new() -> (Self, Arc<dyn Fn() + Send + Sync>) {
        Self::new_after(0)
    }
    fn new_after(skip: usize) -> (Self, Arc<dyn Fn() + Send + Sync>) {
        let (tx, ready) = mpsc::sync_channel(1);
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let state = release.clone();
        let sequence = AtomicUsize::new(0);
        let hook = Arc::new(move || {
            if sequence.fetch_add(1, Ordering::AcqRel) != skip {
                return;
            }
            tx.send(()).unwrap();
            let (lock, cv) = &*state;
            let guard = lock.lock().unwrap();
            drop(cv.wait_while(guard, |released| !*released).unwrap());
        });
        (Self { ready, release }, hook)
    }
    fn entered(&self) {
        self.ready.recv_timeout(Duration::from_secs(2)).unwrap();
    }
}
impl Drop for Gate {
    fn drop(&mut self) {
        *self.release.0.lock().unwrap() = true;
        self.release.1.notify_all();
    }
}
fn ready(client: &mut Client<Wire>) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while client.connection != ConnectionState::Ready {
        assert!(Instant::now() < deadline);
        client.poll(Duration::from_millis(10)).unwrap();
    }
}
#[test]
fn blocked_snapshot_decoder_keeps_heartbeat_and_exact_stop_receipt_serviceable() {
    let (mut client, shared) = attach();
    frame(&shared, snapshot(1));
    ready(&mut client);
    // This is a real multi-page dense document. Gate the End frame just
    // before full SHA/model verification, rather than extending timeouts.
    use sha2::{Digest, Sha256};
    let mut body = snapshot(2)["event"]["snapshot"].clone();
    let points = (0..100000)
        .map(|frame| json!({"frame":frame,"value":0.5}))
        .collect::<Vec<_>>();
    body["session"]["tracks"] = json!([{"id":ID,"name":"Dense vocal","gain":1.,"pan":0.,
        "clips":[],"mixAutomation":{"pan":points}}]);
    body["trackRevisions"] = json!({ID:2});
    let bytes = serde_json::to_vec(&body).unwrap();
    assert!(bytes.len() > 2 * 1024 * 1024);
    let digest = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let pages = bytes.chunks(524288).count();
    let (gate, hook) = Gate::new_after(1 + pages);
    client.decoder = DecodeWorker::with_hook(Some(hook));
    frame(
        &shared,
        json!({"version":3,"kind":"snapshotBegin","transferID":ID,"revision":2,
        "totalBytes":bytes.len(),"sha256":digest}),
    );
    for (index, page) in bytes.chunks(524288).enumerate() {
        frame(
            &shared,
            json!({"version":3,"kind":"snapshotPage","transferID":ID,"revision":2,
            "offset":index*524288,"data":base64::engine::general_purpose::STANDARD.encode(page)}),
        );
    }
    for _ in 0..1 + pages {
        client.poll(Duration::from_secs(2)).unwrap();
    }
    frame(
        &shared,
        json!({"version":3,"kind":"snapshotEnd","transferID":ID,"revision":2,
        "totalBytes":bytes.len(),"sha256":digest}),
    );
    client.poll(Duration::ZERO).unwrap();
    gate.entered();
    let stop = client
        .enqueue(RemoteCommand::Stop, None, BTreeMap::new())
        .unwrap();
    frame(&shared, json!({"version":3,"kind":"heartbeat","id":ID}));
    assert!(client.poll(Duration::from_millis(20)).unwrap().is_none());
    assert_eq!(client.snapshot.as_ref().unwrap().revision, 1);
    assert_eq!(client.request_state(&stop), Some(ReceiptState::Sent));
    let unknown = "00000000-0000-4000-8000-000000000000";
    frame(
        &shared,
        json!({"version":3,"kind":"reply","requestID":unknown,"status":"accepted",
        "revision":2,"message":""}),
    );
    assert!(client.poll(Duration::from_millis(5)).unwrap().is_none());
    assert!(
        client.receipt(unknown).is_none(),
        "uncorrelated replies remain ordered ordinary work"
    );
    frame(
        &shared,
        json!({"version":3,"kind":"reply","requestID":stop,"status":"accepted",
        "revision":1,"message":""}),
    );
    assert!(
        matches!(client.poll(Duration::from_millis(20)).unwrap(),Some(Notice::Reply(reply))
        if reply.request_id==stop)
    );
    assert_eq!(client.snapshot.as_ref().unwrap().revision, 1);
    {
        let state = shared.lock().unwrap();
        assert!(state
            .sent
            .iter()
            .any(|value| value["kind"] == "heartbeatReply" && value["id"] == ID));
        assert_eq!(
            state
                .sent
                .iter()
                .filter(|value| value["id"] == stop)
                .count(),
            1
        );
    }
    drop(gate);
    let deadline = Instant::now() + Duration::from_secs(2);
    while client.snapshot.as_ref().unwrap().revision != 2 {
        assert!(Instant::now() < deadline);
        client.poll(Duration::from_millis(10)).unwrap();
    }
    assert_eq!(
        client
            .snapshot
            .as_ref()
            .unwrap()
            .session
            .tracks
            .as_ref()
            .unwrap()[0]
            .mix_automation
            .as_ref()
            .unwrap()["pan"]
            .len(),
        100000
    );
}
#[test]
fn reconnect_never_installs_an_old_inflight_decoder_result() {
    let (mut client, old) = attach();
    frame(&old, snapshot(1));
    ready(&mut client);
    let (gate, hook) = Gate::new();
    client.decoder = DecodeWorker::with_hook(Some(hook));
    frame(&old, snapshot(99));
    client.poll(Duration::ZERO).unwrap();
    gate.entered();
    let (replacement, new) = attach();
    // Use the authenticated fixture's transport handshake again on replacement.
    frame(
        &new,
        json!({"version":3,"kind":"challenge","serverID":ID,"nonce":
        base64::engine::general_purpose::STANDARD.encode([0;32])}),
    );
    frame(
        &new,
        json!({"version":3,"kind":"welcome","serverID":ID,"name":"Host","role":"editor"}),
    );
    drop(replacement);
    client.replace_transport(Wire(new.clone())).unwrap();
    frame(&new, snapshot(3));
    drop(gate);
    ready(&mut client);
    assert_eq!(client.snapshot.as_ref().unwrap().revision, 3);
    assert!(client.poll(Duration::from_millis(5)).unwrap().is_none());
    assert_eq!(client.snapshot.as_ref().unwrap().revision, 3);
}
#[test]
fn queued_decoded_results_do_not_starve_socket_heartbeat() {
    let (mut client, shared) = attach();
    frame(&shared, snapshot(1));
    ready(&mut client);
    frame(&shared, snapshot(2));
    client.poll(Duration::ZERO).unwrap();
    std::thread::sleep(Duration::from_millis(10));
    frame(&shared, json!({"version":3,"kind":"heartbeat","id":ID}));
    client.poll(Duration::ZERO).unwrap();
    assert!(shared
        .lock()
        .unwrap()
        .sent
        .iter()
        .any(|v| v["kind"] == "heartbeatReply"));
    client.poll(Duration::from_millis(20)).unwrap();
    assert_eq!(client.snapshot.as_ref().unwrap().revision, 2);
}
