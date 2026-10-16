use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use soundroom_client::{
    auth::Credential,
    client::{Client, ConnectionState, Notice},
    models::*,
    snapshot::SnapshotAssembler,
    state,
    transport::ControlTransport,
    Error,
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};
const ID: &str = "12345678-1234-4234-8234-123456789ABC";
#[derive(Default)]
struct Shared {
    sent: Vec<Value>,
    fail_next: bool,
}
struct Wire {
    incoming: VecDeque<Vec<u8>>,
    shared: Arc<Mutex<Shared>>,
}
impl Wire {
    fn new(events: Vec<Value>) -> (Self, Arc<Mutex<Shared>>) {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let mut incoming=VecDeque::from([serde_json::to_vec(&json!({"version":3,"kind":"challenge","serverID":ID,"nonce":base64::engine::general_purpose::STANDARD.encode([0;32])})).unwrap(),serde_json::to_vec(&json!({"version":3,"kind":"welcome","serverID":ID,"name":"Host","role":"editor"})).unwrap()]);
        for event in events {
            incoming.push_back(serde_json::to_vec(&event).unwrap());
        }
        (
            Self {
                incoming,
                shared: shared.clone(),
            },
            shared,
        )
    }
}
impl ControlTransport for Wire {
    fn send(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let mut shared = self.shared.lock().unwrap();
        if shared.fail_next {
            shared.fail_next = false;
            return Err(Error::Disconnected);
        }
        let value: Value = serde_json::from_slice(bytes)?;
        if value["kind"] == "request" {
            self.incoming.push_back(serde_json::to_vec(&json!({"version":3,"kind":"reply","requestID":value["id"],"status":"accepted","revision":1,"message":"","payload":base64::engine::general_purpose::STANDARD.encode(b"{\"seed\":17}")}))?);
        }
        shared.sent.push(value);
        Ok(())
    }
    fn receive(&mut self, _: Duration) -> Result<Option<Vec<u8>>, Error> {
        Ok(self.incoming.pop_front())
    }
    fn close(&mut self) -> Result<(), Error> {
        Ok(())
    }
}
fn credential() -> Credential {
    Credential::from_bytes(&serde_json::to_vec(&json!({"version":3,"serverID":ID,"credentialID":ID,"clientSecret":base64::engine::general_purpose::STANDARD.encode([0;32]),"role":"operator"})).unwrap()).unwrap()
}
fn snapshot(revision: i64) -> Value {
    json!({"serverID":ID,"serverName":"Host","session":{"version":9,"name":"Song","tracks":[]},"revision":revision,"trackRevisions":{},"transport":{"frame":0,"isPlaying":false,"isRecording":false,"isPreparing":false,"hostTime":0},"devices":[],"inputDeviceID":0,"outputDeviceID":0,"assets":[],"status":"","peers":[]})
}
fn initial() -> Value {
    json!({"version":3,"kind":"event","event":{"type":"snapshot","snapshot":snapshot(1)}})
}
fn delta(revision: i64) -> Value {
    json!({"revision":revision,"tracks":[],"controls":[],"removedTrackIDs":[],"trackRevisions":{},"metadataUpdated":false,"assets":[],"name":"Changed"})
}
fn attach(events: Vec<Value>) -> (Client<Wire>, Arc<Mutex<Shared>>) {
    let (wire, shared) = Wire::new(events);
    (
        Client::attach(wire, credential(), "Studio", Some(ID.into())).unwrap(),
        shared,
    )
}
#[test]
fn authority_comes_from_welcome_and_reply_payload_bytes_remain_exact() {
    let (mut client, _) = attach(vec![initial()]);
    assert_eq!(client.role, Some(RemoteRole::Editor));
    client.poll(Duration::from_secs(1)).unwrap();
    assert_eq!(client.connection, ConnectionState::Ready);
    let reply = client
        .request(
            RemoteCommand::Ping { client_time: 0. },
            None,
            BTreeMap::new(),
            Duration::from_secs(1),
        )
        .unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(reply.payload.unwrap())
            .unwrap(),
        b"{\"seed\":17}"
    );
}
#[test]
fn queued_cancellation_preserves_sent_uncertain_and_late_receipts() {
    use soundroom_client::client::ReceiptState;
    let (mut client, shared) = attach(vec![initial()]);
    client.poll(Duration::from_secs(1)).unwrap();
    let queued = client
        .enqueue(
            RemoteCommand::AddTrack {
                name: "Never sent".into(),
            },
            None,
            BTreeMap::new(),
        )
        .unwrap();
    assert_eq!(client.request_state(&queued), Some(ReceiptState::Queued));
    assert!(client.cancel_queued(&queued));
    assert!(client.request_state(&queued).is_none());
    client.flush().unwrap();
    assert!(shared
        .lock()
        .unwrap()
        .sent
        .iter()
        .all(|frame| frame["id"] != queued));
    let sent = client
        .enqueue(
            RemoteCommand::AddTrack {
                name: "Sent".into(),
            },
            None,
            BTreeMap::new(),
        )
        .unwrap();
    client.flush().unwrap();
    assert_eq!(client.request_state(&sent), Some(ReceiptState::Sent));
    assert!(!client.cancel_queued(&sent));
    assert!(
        matches!(client.poll(Duration::from_secs(1)).unwrap(),Some(Notice::Reply(reply)) if reply.request_id==sent)
    );
    assert_eq!(
        client.receipt(&sent).unwrap().status,
        RemoteReplyStatus::Accepted
    );
    let uncertain = client
        .enqueue(
            RemoteCommand::AddTrack {
                name: "Unknown outcome".into(),
            },
            None,
            BTreeMap::new(),
        )
        .unwrap();
    shared.lock().unwrap().fail_next = true;
    assert!(client.flush().is_err());
    assert_eq!(
        client.request_state(&uncertain),
        Some(ReceiptState::Uncertain)
    );
    assert!(!client.cancel_queued(&uncertain));
    let (wire, resumed) = Wire::new(vec![initial()]);
    client.replace_transport(wire).unwrap();
    client.poll(Duration::from_secs(1)).unwrap();
    assert_eq!(client.uncertain_requests(), vec![uncertain.clone()]);
    client.reconcile_exact(&uncertain).unwrap();
    client.flush().unwrap();
    assert_eq!(resumed.lock().unwrap().sent[1]["id"], uncertain);
}
#[test]
fn transport_priority_bypasses_bounded_bulk_queue() {
    let (mut client, shared) = attach(vec![initial()]);
    client.poll(Duration::from_secs(1)).unwrap();
    for i in 0..56 {
        client
            .enqueue(
                RemoteCommand::AddTrack {
                    name: format!("Track{i}"),
                },
                None,
                BTreeMap::new(),
            )
            .unwrap();
    }
    assert!(matches!(
        client.enqueue(
            RemoteCommand::AddTrack {
                name: "Overloaded".into()
            },
            None,
            BTreeMap::new()
        ),
        Err(Error::QueueFull)
    ));
    client
        .enqueue(RemoteCommand::Stop, None, BTreeMap::new())
        .unwrap();
    client.flush().unwrap();
    let sent = shared.lock().unwrap();
    let requests: Vec<_> = sent
        .sent
        .iter()
        .filter(|v| v["kind"] == "request")
        .collect();
    assert_eq!(requests.len(), 8);
    assert_eq!(requests[0]["command"]["type"], "stop");
}
#[test]
fn automation_terminal_uses_reserved_capacity_and_overtakes_queued_values() {
    use base64::Engine;
    let (mut client, shared) = attach(vec![initial()]);
    client.poll(Duration::from_secs(1)).unwrap();
    let intent = |operation: &str, payload: Value| {
        serde_json::from_value(json!({"operation":operation,"context":{"clipIDs":[],"cursorFrame":0},"integers":{},"numbers":{},"strings":{},"flags":{},"payload":base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&payload).unwrap())})).unwrap()
    };
    for sequence in 1..=56 {
        client
            .enqueue(
                RemoteCommand::Editor {
                    intent: intent(
                        "automation.writeValue",
                        json!({"gestureID":ID,"sequence":sequence,"value":0.5}),
                    ),
                },
                None,
                BTreeMap::new(),
            )
            .unwrap();
    }
    let end_id = client
        .enqueue(
            RemoteCommand::Editor {
                intent: intent(
                    "automation.writeEnd",
                    json!({"gestureID":ID,"finalSequence":56,"cancel":false}),
                ),
            },
            None,
            BTreeMap::new(),
        )
        .unwrap();
    client.flush().unwrap();
    let sent = shared.lock().unwrap();
    let first = sent.sent.iter().find(|v| v["kind"] == "request").unwrap();
    assert_eq!(first["id"], end_id);
    assert_eq!(
        first["command"]["intent"]["operation"],
        "automation.writeEnd"
    );
}
#[test]
fn disconnect_never_replays_authored_edits_automatically_and_private_view_survives() {
    let (mut client, shared) = attach(vec![initial()]);
    client.poll(Duration::from_secs(1)).unwrap();
    client.view.cursor_frame = 9876;
    client.view.panel = Some("effects".into());
    let id = client
        .enqueue(
            RemoteCommand::AddTrack {
                name: "Uncertain".into(),
            },
            None,
            BTreeMap::new(),
        )
        .unwrap();
    shared.lock().unwrap().fail_next = true;
    assert!(client.flush().is_err());
    assert_eq!(client.uncertain_requests(), vec![id.clone()]);
    let (wire, new_shared) = Wire::new(vec![initial()]);
    client.replace_transport(wire).unwrap();
    client.poll(Duration::from_secs(1)).unwrap();
    assert_eq!(client.view.cursor_frame, 9876);
    assert_eq!(client.view.panel.as_deref(), Some("effects"));
    assert_eq!(client.client_id, ID);
    assert_eq!(new_shared.lock().unwrap().sent.len(), 1);
    client.reconcile_exact(&id).unwrap();
    client.flush().unwrap();
    let sent = new_shared.lock().unwrap();
    assert_eq!(sent.sent[1]["id"], id);
}
#[test]
fn revision_gap_requests_resync_without_overwriting_state() {
    let (mut client, shared) = attach(vec![
        initial(),
        json!({"version":3,"kind":"event","event":{"type":"delta","delta":delta(3)}}),
    ]);
    client.poll(Duration::from_secs(1)).unwrap();
    assert!(matches!(
        client.poll(Duration::from_secs(1)).unwrap(),
        Some(Notice::ResyncRequired)
    ));
    assert_eq!(client.snapshot.as_ref().unwrap().revision, 1);
    assert_eq!(
        client.snapshot.as_ref().unwrap().session.name.as_deref(),
        Some("Song")
    );
    client.flush().unwrap();
    assert_eq!(
        shared.lock().unwrap().sent.last().unwrap()["command"]["type"],
        "snapshot"
    );
}
#[test]
fn paged_snapshot_is_atomic_and_replays_retained_deltas_then_latest_transport() {
    let bytes = serde_json::to_vec(&snapshot(1)).unwrap();
    let digest = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let begin = json!({"version":3,"kind":"snapshotBegin","transferID":ID,"revision":1,"totalBytes":bytes.len(),"sha256":digest});
    let page = json!({"version":3,"kind":"snapshotPage","transferID":ID,"revision":1,"offset":0,"data":base64::engine::general_purpose::STANDARD.encode(&bytes)});
    let end = json!({"version":3,"kind":"snapshotEnd","transferID":ID,"revision":1,"totalBytes":bytes.len(),"sha256":digest});
    let transport =
        json!({"frame":999,"isPlaying":true,"isRecording":false,"isPreparing":false,"hostTime":1});
    let (mut client, _) = attach(vec![
        begin,
        json!({"version":3,"kind":"event","event":{"type":"delta","delta":delta(2)}}),
        json!({"version":3,"kind":"event","event":{"type":"transport","transport":transport}}),
        page,
        end,
    ]);
    for _ in 0..4 {
        client.poll(Duration::from_secs(1)).unwrap();
        assert!(client.snapshot.is_none());
    }
    client.poll(Duration::from_secs(1)).unwrap();
    let s = client.snapshot.unwrap();
    assert_eq!(s.revision, 2);
    assert_eq!(s.session.name.as_deref(), Some("Changed"));
    assert_eq!(s.transport.frame, 999);
}
#[test]
fn malformed_snapshot_transfer_cancels_without_partial_install() {
    let mut assembler = SnapshotAssembler::default();
    let begin:RemoteSnapshotPage=serde_json::from_value(json!({"version":3,"kind":"snapshotBegin","transferID":ID,"revision":1,"totalBytes":5,"sha256":"0".repeat(64)})).unwrap();
    assembler.accept(begin.clone()).unwrap();
    let bad:RemoteSnapshotPage=serde_json::from_value(json!({"version":3,"kind":"snapshotPage","transferID":ID,"revision":2,"offset":0,"data":"YQ=="})).unwrap();
    assert!(assembler.accept(bad).is_err());
    assert!(!assembler.active());
    assert_eq!(assembler.next_offset(), 0);
    assembler.accept(begin).unwrap();
    let page:RemoteSnapshotPage=serde_json::from_value(json!({"version":3,"kind":"snapshotPage","transferID":ID,"revision":1,"offset":0,"data":"YWJjZGU="})).unwrap();
    assembler.accept(page).unwrap();
    let end:RemoteSnapshotPage=serde_json::from_value(json!({"version":3,"kind":"snapshotEnd","transferID":ID,"revision":1,"totalBytes":5,"sha256":"0".repeat(64)})).unwrap();
    assert!(assembler.accept(end).is_err());
    assert!(!assembler.active());
}
#[test]
fn invalid_delta_identity_is_rejected_before_any_mutation() {
    let mut s: RemoteSnapshot = serde_json::from_value(snapshot(1)).unwrap();
    let original = s.clone();
    let mut d = delta(2);
    d["tracks"] = json!([{}]);
    let d: RemoteSessionDelta = serde_json::from_value(d).unwrap();
    assert!(state::apply_delta(&mut s, d).is_err());
    assert_eq!(s, original);
}
#[test]
fn old_transport_telemetry_does_not_override_a_new_full_snapshot() {
    let transport =
        json!({"frame":999,"isPlaying":true,"isRecording":false,"isPreparing":false,"hostTime":1});
    let (mut client, _) = attach(vec![
        initial(),
        json!({"version":3,"kind":"event","event":{"type":"transport","transport":transport}}),
        json!({"version":3,"kind":"event","event":{"type":"snapshot","snapshot":snapshot(2)}}),
    ]);
    for _ in 0..3 {
        client.poll(Duration::from_secs(1)).unwrap();
    }
    assert_eq!(client.snapshot.unwrap().transport.frame, 0);
}
#[test]
fn malformed_envelope_disconnects_without_replacing_the_session() {
    let (mut client, _) = attach(vec![
        initial(),
        json!({"version":3,"kind":"event","event":{"type":"presence","peers":[]},"hiddenAuthority":"operator"}),
    ]);
    client.poll(Duration::from_secs(1)).unwrap();
    assert!(client.poll(Duration::from_secs(1)).is_err());
    assert_eq!(client.connection, ConnectionState::Disconnected);
    assert_eq!(client.snapshot.as_ref().unwrap().revision, 1);
}

fn pages(revision: i64) -> [Value; 3] {
    let bytes = serde_json::to_vec(&snapshot(revision)).unwrap();
    let digest = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    [
        json!({"version":3,"kind":"snapshotBegin","transferID":ID,"revision":revision,"totalBytes":bytes.len(),"sha256":digest}),
        json!({"version":3,"kind":"snapshotPage","transferID":ID,"revision":revision,"offset":0,"data":base64::engine::general_purpose::STANDARD.encode(&bytes)}),
        json!({"version":3,"kind":"snapshotEnd","transferID":ID,"revision":revision,"totalBytes":bytes.len(),"sha256":digest}),
    ]
}
fn status(message: &str) -> Value {
    json!({"version":3,"kind":"event","event":{"type":"status","message":message}})
}
#[test]
fn status_updates_live_state_and_latest_overlay_survives_shared_metadata_pages() {
    let [begin, page, end] = pages(2);
    let (mut client, _) = attach(vec![
        initial(),
        status("Ready"),
        begin,
        status("Preparing"),
        status("Playing"),
        page,
        end,
    ]);
    client.poll(Duration::from_secs(1)).unwrap();
    assert!(
        matches!(client.poll(Duration::from_secs(1)).unwrap(), Some(Notice::Event(RemoteEvent::Status { message })) if message == "Ready")
    );
    assert_eq!(client.snapshot.as_ref().unwrap().status, "Ready");
    for _ in 0..4 {
        client.poll(Duration::from_secs(1)).unwrap();
    }
    assert_eq!(client.snapshot.as_ref().unwrap().revision, 1);
    assert_eq!(client.snapshot.as_ref().unwrap().status, "Playing");
    client.poll(Duration::from_secs(1)).unwrap();
    assert_eq!(client.snapshot.as_ref().unwrap().revision, 2);
    assert_eq!(client.snapshot.as_ref().unwrap().status, "Playing");
}
#[test]
fn status_overlay_is_cleared_for_new_transfer_and_reconnect() {
    let [first_begin, _, _] = pages(1);
    let [next_begin, next_page, next_end] = pages(2);
    let (mut client, _) = attach(vec![
        first_begin,
        status("Superseded"),
        next_begin,
        next_page,
        next_end,
    ]);
    for _ in 0..5 {
        client.poll(Duration::from_secs(1)).unwrap();
    }
    assert_eq!(client.snapshot.as_ref().unwrap().status, "");
    let [begin, _, _] = pages(3);
    let (wire, _) = Wire::new(vec![begin, status("Old connection")]);
    client.replace_transport(wire).unwrap();
    client.poll(Duration::from_secs(1)).unwrap();
    client.poll(Duration::from_secs(1)).unwrap();
    let (wire, _) = Wire::new(vec![initial()]);
    client.replace_transport(wire).unwrap();
    client.poll(Duration::from_secs(1)).unwrap();
    assert_eq!(client.snapshot.as_ref().unwrap().status, "");
}

#[test]
fn paged_status_and_deferred_delta_follow_arrival_order() {
    for (delta_first, authored_status) in [
        (false, Some("Authored")),
        (true, Some("Authored")),
        (false, None),
    ] {
        let [begin, page, end] = pages(1);
        let mut d = delta(2);
        if let Some(message) = authored_status {
            d["status"] = json!(message);
        }
        let d = json!({"version":3,"kind":"event","event":{"type":"delta","delta":d}});
        let events = if delta_first {
            vec![begin, d, status("Live"), page, end]
        } else {
            vec![begin, status("Live"), d, page, end]
        };
        let (mut client, _) = attach(events);
        for _ in 0..5 {
            client.poll(Duration::from_secs(1)).unwrap();
        }
        let snapshot = client.snapshot.unwrap();
        assert_eq!(snapshot.revision, 2);
        assert_eq!(
            snapshot.status,
            if !delta_first && authored_status.is_some() {
                "Authored"
            } else {
                "Live"
            }
        );
    }
}
#[test]
fn oversized_status_is_rejected_without_installing_or_retaining_it() {
    let [begin, _, _] = pages(1);
    let (mut client, _) = attach(vec![begin, status(&"x".repeat(65537))]);
    client.poll(Duration::from_secs(1)).unwrap();
    assert!(client.poll(Duration::from_secs(1)).is_err());
    assert_eq!(client.connection, ConnectionState::Disconnected);
    assert!(client.snapshot.is_none());
    let (wire, _) = Wire::new(vec![initial()]);
    client.replace_transport(wire).unwrap();
    client.poll(Duration::from_secs(1)).unwrap();
    assert_eq!(client.snapshot.unwrap().status, "");
}

#[test]
fn stale_deferred_status_does_not_clear_newer_live_overlay() {
    for applied_revision in [5, 6] {
        let [begin, page, end] = pages(5);
        let mut events = vec![begin];
        if applied_revision == 6 {
            events.push(
                json!({"version":3,"kind":"event","event":{"type":"delta","delta":delta(6)}}),
            );
        }
        events.push(status("Live"));
        let mut stale = delta(applied_revision - 1);
        stale["status"] = json!("Stale");
        events.push(json!({"version":3,"kind":"event","event":{"type":"delta","delta":stale}}));
        events.extend([page, end]);
        let count = events.len();
        let (mut client, _) = attach(events);
        for _ in 0..count {
            client.poll(Duration::from_secs(1)).unwrap();
        }
        let snapshot = client.snapshot.unwrap();
        assert_eq!(snapshot.revision, applied_revision);
        assert_eq!(snapshot.status, "Live");
    }
}

fn feature_capabilities(feature: bool, operation: bool) -> Value {
    let mut capabilities: Value =
        serde_json::from_slice(include_bytes!("../../server/catalog/native-v3.json")).unwrap();
    let capabilities = &mut capabilities["capabilities"];
    capabilities["features"] = if feature {
        json!(["compact-control-patches"])
    } else {
        json!([])
    };
    capabilities["editorOperations"] = if operation {
        json!(["client.features"])
    } else {
        json!([])
    };
    json!({"version":3,"kind":"capabilities","capabilities":capabilities})
}
#[test]
fn compact_patch_opt_in_requires_both_capability_and_operation_and_resets_on_reconnect() {
    for (feature, operation) in [(false, false), (true, false), (false, true), (true, true)] {
        let caps = feature_capabilities(feature, operation);
        let (mut client, shared) = attach(vec![caps.clone(), caps.clone(), initial()]);
        for _ in 0..3 {
            client.poll(Duration::from_secs(1)).unwrap();
        }
        client.flush().unwrap();
        let requests = shared
            .lock()
            .unwrap()
            .sent
            .iter()
            .filter(|frame| frame["kind"] == "request")
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(requests.len(), usize::from(feature && operation));
        if let Some(request) = requests.first() {
            assert_eq!(request["command"]["intent"]["operation"], "client.features");
            let payload = base64::engine::general_purpose::STANDARD
                .decode(request["command"]["intent"]["payload"].as_str().unwrap())
                .unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(&payload).unwrap(),
                json!(["compact-control-patches"])
            );
            let (wire, new_shared) = Wire::new(vec![caps, initial()]);
            client.replace_transport(wire).unwrap();
            client.poll(Duration::from_secs(1)).unwrap();
            client.flush().unwrap();
            let request = new_shared.lock().unwrap().sent[1].clone();
            assert_eq!(request["command"]["intent"]["operation"], "client.features");
            assert_ne!(request["id"], requests[0]["id"]);
        }
    }
}
#[test]
fn sparse_control_patch_preserves_dense_content_and_rejects_bad_targets_atomically() {
    let mut full = snapshot(1);
    full["session"]["tracks"] = json!([{"id":ID,"name":"Vox","gain":1.,"pan":0.,"mixAutomation":{"volume":[{"frame":0,"value":0.7}]},"clips":[]}]);
    let mut snapshot: RemoteSnapshot = serde_json::from_value(full).unwrap();
    let mut d = delta(2);
    d["controlPatches"] = json!([{"trackID":ID,"gain":0.5,"pan":-0.25,"muted":true}]);
    state::apply_delta(&mut snapshot, serde_json::from_value(d).unwrap()).unwrap();
    let track = &snapshot.session.tracks.as_ref().unwrap()[0];
    assert_eq!(track.gain, Some(0.5));
    assert_eq!(track.pan, Some(-0.25));
    assert_eq!(track.muted, Some(true));
    assert_eq!(track.name.as_deref(), Some("Vox"));
    assert_eq!(
        track.mix_automation.as_ref().unwrap()["volume"][0].value,
        0.7
    );
    let original = snapshot.clone();
    for patches in [
        json!([{"trackID":ID,"gain":0.4},{"trackID":"00000000-0000-4000-8000-000000000000","gain":0.2}]),
        json!([{"trackID":ID,"gain":0.4},{"trackID":ID.to_ascii_lowercase(),"pan":0.5}]),
        json!([{"trackID":ID,"pan":2.}]),
    ] {
        let mut d = delta(3);
        d["controlPatches"] = patches;
        assert!(state::apply_delta(&mut snapshot, serde_json::from_value(d).unwrap()).is_err());
        assert_eq!(snapshot, original);
    }
}
