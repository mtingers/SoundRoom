use super::*;

#[test]
fn blocked_artifact_io_does_not_hold_authority_and_rechecks_revocation() {
    let (root, shared, peer, _) = publication_tests::control_fixture(0);
    let path = root.join("Artifacts/chunk.bin");
    std::fs::write(&path, [7_u8; 128]).unwrap();
    let artifact = new_id().unwrap();
    let (entered, wait_entered) = std::sync::mpsc::sync_channel(1);
    let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let worker_release = release.clone();
    {
        let mut state = shared.lock().unwrap();
        state.artifacts.insert(artifact.clone(), Artifact { owner: peer.credential.clone(), runtime: Some(peer.runtime.clone()),
            path, bytes: 128, expires: Instant::now()+Duration::from_secs(60) });
        state.artifact_read_hook = Some(Arc::new(move || {
            entered.send(()).unwrap();
            let (lock, wake) = &*worker_release;
            drop(wake.wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(5), |released| !*released).unwrap());
        }));
    }
    let (out, inbox) = Outbox::pair(shared.lock().unwrap().global());
    let request = new_id().unwrap();
    Service::dispatch(&shared, &peer, &out, json!({"version":3,"kind":"request","id":request,"clientID":peer.runtime,"expectedTrackRevisions":{},
        "command":{"type":"downloadArtifact","id":artifact,"offset":0,"bytes":128}})).unwrap();
    wait_entered.recv_timeout(Duration::from_secs(2)).unwrap();
    // Disk latency belongs to a worker. Even while it is deliberately blocked,
    // another request can acquire authority and return immediately.
    assert!(shared.try_lock().is_ok());
    let (ping_out, ping_in) = Outbox::pair(shared.lock().unwrap().global());
    Service::dispatch(&shared, &peer, &ping_out, json!({"version":3,"kind":"request","id":new_id().unwrap(),"clientID":peer.runtime,"expectedTrackRevisions":{},
        "command":{"type":"ping","clientTime":0}})).unwrap();
    let ping = ping_in.priority.recv_timeout(Duration::from_secs(1)).unwrap();
    ping_in.received(ping.len());
    assert_eq!(serde_json::from_slice::<Value>(&ping).unwrap()["status"], "accepted");
    shared.lock().unwrap().registry.revoke(&peer.credential).unwrap();
    {
        let (lock, wake) = &*release; *lock.lock().unwrap() = true; wake.notify_all();
    }
    let response = inbox.priority.recv_timeout(Duration::from_secs(2)).unwrap();
    inbox.received(response.len());
    let response: Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(response["status"], "rejected");
    assert!(response.get("payload").map_or(true, Value::is_null));
    drop(shared); std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cold_waveform_does_not_block_same_peer_fifo_and_cancellation_returns_no_payload() {
    for mode in ["cancel", "revoke", "disconnect"] {
        let (root, shared, peer, _) = publication_tests::control_fixture(0);
        let asset = new_id().unwrap();
        let path = root.join("waveform.wav");
        let mut wave = soundroom_engine::wave::WaveWriter::create(&path, 1024).unwrap();
        wave.write(&[0.25; 1024], &[-0.5; 1024]).unwrap();
        wave.finish().unwrap();
        let info = soundroom_engine::wave::WaveReader::open(&path).unwrap().info;
        let (entered, ready) = std::sync::mpsc::sync_channel(1);
        let (release, blocked) = std::sync::mpsc::sync_channel(1);
        let blocked = Mutex::new(blocked);
        {
            let mut state = shared.lock().unwrap();
            state.media.insert(asset.clone(), Media { path, info });
            state.waveform_hook = Some(Arc::new(move || {
                entered.send(()).unwrap();
                blocked.lock().unwrap().recv_timeout(Duration::from_secs(5)).unwrap();
            }));
        }
        let (out, inbox) = Outbox::pair(shared.lock().unwrap().global());
        if mode == "disconnect" {
            shared.lock().unwrap().peers.insert(peer.runtime.clone(), Peer { identity: peer.clone(), out: out.clone() });
        }
        let connection_out = out.clone();
        let global = shared.lock().unwrap().global();
        let decoder = crate::endpoint::requests::RequestDecode::new(shared.clone(), peer.clone(), out, global).unwrap();
        let wire = |id: &str, command: Value| serde_json::to_vec(&json!({"version":3,"kind":"request",
            "id":id,"clientID":peer.runtime,"expectedTrackRevisions":{},"command":command})).unwrap();
        let waveform = new_id().unwrap();
        decoder.route(wire(&waveform, json!({"type":"waveform","assetID":asset,"start":0,"end":1024,"points":16}))).unwrap();
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        // An ordinary request on this exact decoder must pass the cold scan.
        let listing = new_id().unwrap();
        decoder.route(wire(&listing, json!({"type":"listProjects"}))).unwrap();
        let bytes = inbox.priority.recv_timeout(Duration::from_secs(2)).unwrap();
        inbox.received(bytes.len());
        let reply: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(reply["requestID"], listing);
        assert_eq!(reply["status"], "accepted");
        {
            let mut state = shared.lock().unwrap();
            assert!(state.admits_job(&peer, false));
            if mode == "revoke" { state.registry.revoke(&peer.credential).unwrap(); }
            else if mode == "disconnect" { state.leave(&peer, &connection_out); }
            else { state.pending[&(peer.credential.clone(), peer.runtime.clone(), waveform.clone())].cancel.store(true, Ordering::Release); }
        }
        release.send(()).unwrap();
        let bytes = inbox.priority.recv_timeout(Duration::from_secs(2)).unwrap();
        inbox.received(bytes.len());
        let reply: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(reply["requestID"], waveform);
        assert_eq!(reply["status"], "rejected");
        assert!(reply.get("payload").map_or(true, Value::is_null));
        drop(decoder); drop(shared); std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn waveform_admission_has_peer_and_global_bounds_without_consuming_edit_or_stop_capacity() {
    let (root, shared, peer, _) = publication_tests::control_fixture(0);
    let mut other = peer.clone(); other.runtime = new_id().unwrap();
    let mut third = peer.clone(); third.runtime = new_id().unwrap();
    let mut state = shared.lock().unwrap();
    for owner in [&peer, &other] {
        assert!(state.admits_waveform(owner));
        for _ in 0..2 {
            state.pending.insert((owner.credential.clone(), owner.runtime.clone(), new_id().unwrap()), Pending {
                authored: false, critical: false, transport_start: false, write: false, save: false,
                waveform: true, digest: [0; 32], cancel: Arc::new(AtomicBool::new(false)),
            });
        }
        assert!(!state.admits_waveform(owner));
        assert!(state.admits_job(owner, false));
        assert!(state.admits_job(owner, true));
    }
    assert!(!state.admits_waveform(&third));
    assert!(state.admits_job(&third, false));
    drop(state); drop(shared); std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn request_decode_preserves_canonical_identity_digest_and_rejects_spoofing_without_duplicate_models() {
    let (root, shared, peer, _) = publication_tests::control_fixture(0);
    let wire = json!({"version":3,"kind":"request","id":new_id().unwrap().to_lowercase(),
        "clientID":peer.runtime.to_lowercase(),"expectedTrackRevisions":{},
        "command":{"type":"uploadChunk","id":new_id().unwrap().to_lowercase(),"offset":262144,
            "data":base64::engine::general_purpose::STANDARD.encode(vec![17;262144])}});
    let previous = |mut value: Value| {
        value.as_object_mut().unwrap().remove("version"); value.as_object_mut().unwrap().remove("kind");
        validation::validate_model(&value, "RemoteRequest").unwrap();
        let request: RemoteRequest = serde_json::from_value(value.clone()).unwrap();
        assert!(request.client_id.eq_ignore_ascii_case(&peer.runtime));
        canonical_ids(&mut value);
        let request: RemoteRequest = serde_json::from_value(value.clone()).unwrap();
        let digest: [u8;32] = Sha256::digest(serde_json::to_vec(&value).unwrap()).into();
        (request, digest)
    };
    let (expected, digest) = previous(wire.clone());
    let actual = Service::validate_request(&peer, wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(expected).unwrap(), serde_json::to_value(actual.request).unwrap());
    assert_eq!(digest, actual.digest);
    let mut spoofed = wire.clone(); spoofed["clientID"] = json!(new_id().unwrap());
    assert!(Service::validate_request(&peer, spoofed).is_err());
    let mut records = Vec::new();
    for trial in 0..4 {
        for old in if trial % 2 == 0 { [true, false] } else { [false, true] } {
            let start = Instant::now();
            for _ in 0..100 {
                if old { std::hint::black_box(previous(wire.clone())); }
                else { std::hint::black_box(Service::validate_request(&peer, wire.clone()).unwrap()); }
            }
            records.push(json!({"previous":old,"trial":trial,"requests":100,"seconds":start.elapsed().as_secs_f64()}));
        }
    }
    if let Some(path) = std::env::var_os("SOUNDROOM_REQUEST_PERF") {
        std::fs::write(path, serde_json::to_vec_pretty(&records).unwrap()).unwrap();
    }
    println!("REQUEST_DECODE_PERFORMANCE {}", json!(records));
    drop(shared); std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn obsolete_waveform_cleanup_respects_connection_tokens_and_preserves_authored_jobs() {
    let (root, shared, peer, _) = publication_tests::control_fixture(0);
    let global = shared.lock().unwrap().global();
    let (old, _old_in) = Outbox::pair(global.clone());
    let (new, _new_in) = Outbox::pair(global);
    let wave = Arc::new(AtomicBool::new(false));
    let edit = Arc::new(AtomicBool::new(false));
    let entry = |waveform: bool, cancel| Pending {
        authored: !waveform, critical: false, transport_start: false, write: false, save: false,
        waveform, digest: [0;32], cancel,
    };
    let mut state = shared.lock().unwrap();
    state.peers.insert(peer.runtime.clone(), Peer { identity: peer.clone(), out: old.clone() });
    state.pending.insert((peer.credential.clone(), peer.runtime.clone(), new_id().unwrap()), entry(true, wave.clone()));
    state.pending.insert((peer.credential.clone(), peer.runtime.clone(), new_id().unwrap()), entry(false, edit.clone()));
    state.join(peer.clone(), new).unwrap();
    assert!(wave.load(Ordering::Acquire));
    assert!(!edit.load(Ordering::Acquire));
    let latest = Arc::new(AtomicBool::new(false));
    state.pending.insert((peer.credential.clone(), peer.runtime.clone(), new_id().unwrap()), entry(true, latest.clone()));
    state.leave(&peer, &old);
    assert!(!latest.load(Ordering::Acquire), "An old connection must not cancel the replacement's reads");
    state.cancel_waveforms(None);
    assert!(latest.load(Ordering::Acquire));
    assert!(!edit.load(Ordering::Acquire));
    drop(state); drop(shared); std::fs::remove_dir_all(root).unwrap();
}
