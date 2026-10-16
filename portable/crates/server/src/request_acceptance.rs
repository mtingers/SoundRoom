//! Explicit live TLS gate. Uses only a manually pumped callback, never a device.
use super::*;
use crate::endpoint::{Endpoint, EndpointConfig};
use soundroom_client::{
    auth::Credential,
    client::{Client, ConnectionState},
    models::RemoteReplyStatus,
    transport::{WssConfig, WssTransport},
};
use std::sync::mpsc::sync_channel;
struct Runner {
    stop: Arc<AtomicBool>,
    joins: Vec<std::thread::JoinHandle<()>>,
}
impl Drop for Runner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        for thread in self.joins.drain(..) {
            let _ = thread.join();
        }
    }
}
struct Release(Option<std::sync::mpsc::SyncSender<()>>);
impl Drop for Release {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.try_send(());
        }
    }
}
#[test]
#[ignore = "Explicit verified localhost TLS acceptance requires supplied private fixture identity and CA; no hardware is opened"]
fn verified_wss_stop_while_large_authored_candidate_is_blocked() {
    let identity =
        std::env::var_os("SOUNDROOM_TEST_TLS_P12").expect("Provide owner-only fixture PKCS12 path");
    let password =
        std::env::var("SOUNDROOM_TEST_TLS_PASSWORD").expect("Provide fixture identity password");
    let ca =
        std::env::var_os("SOUNDROOM_TEST_TLS_CA").expect("Provide localhost fixture CA PEM path");
    let (root, shared, _editor, track_id) = super::publication_tests::control_fixture(0);
    let credential = Credential::from_bytes(
        &serde_json::to_vec(
            &shared
                .lock()
                .unwrap()
                .registry
                .issue("Operator", RemoteRole::Operator)
                .unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    let endpoint = Endpoint::bind(
        EndpointConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            approved_origins: vec!["https://localhost".into()],
        },
        Path::new(&identity),
        &password,
    )
    .unwrap();
    let config = WssConfig {
        endpoint: format!(
            "wss://localhost:{}/control",
            endpoint.address().unwrap().port()
        ),
        origin: Some("https://localhost".into()),
        root_ca_pem: Some(std::fs::read(ca).unwrap()),
        connect_timeout: Duration::from_secs(10),
    };
    let (control, mut host) = soundroom_engine::realtime::channel();
    Service::attach_audio(&shared, control).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let callback_stop = stop.clone();
    let callback = std::thread::spawn(move || {
        let mut left = [0.; 128];
        let mut right = [0.; 128];
        while !callback_stop.load(Ordering::Acquire) {
            host.render(&mut left, &mut right);
            std::thread::sleep(Duration::from_millis(2));
        }
    });
    let endpoint_stop = stop.clone();
    let endpoint_service = shared.clone();
    let listener =
        std::thread::spawn(move || endpoint.run(endpoint_service, endpoint_stop).unwrap());
    let runner = Runner {
        stop,
        joins: vec![listener, callback],
    };
    let mut client =
        Client::<WssTransport>::connect(&config, credential, "Operator", None).unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while client.connection != ConnectionState::Ready {
        assert!(Instant::now() < deadline);
        client.poll(Duration::from_millis(25)).unwrap();
    }
    let reply = client
        .request(
            RemoteCommand::Play { track_only: None },
            None,
            BTreeMap::new(),
            Duration::from_secs(3),
        )
        .unwrap();
    assert_eq!(reply.status, RemoteReplyStatus::Accepted);
    // Report ordinary transport separately from preparation/adoption of the
    // later dense revision, so a latency distribution has a stable workload.
    let mut latencies = Vec::new();
    for iteration in 0..30 {
        let play = client
            .request(
                RemoteCommand::Play { track_only: None },
                None,
                BTreeMap::new(),
                Duration::from_secs(3),
            )
            .unwrap();
        assert_eq!(
            play.status,
            RemoteReplyStatus::Accepted,
            "Play sample {iteration}: {play:?}"
        );
        let start = Instant::now();
        let stop = client
            .request(
                RemoteCommand::Stop,
                None,
                BTreeMap::new(),
                Duration::from_secs(3),
            )
            .unwrap();
        let (telemetry, revisions) = {
            let state = shared.lock().unwrap();
            let backend = state.backend.as_ref().unwrap();
            (
                backend.inner_telemetry_for_test(),
                backend.graph_revisions(),
            )
        };
        assert_eq!(
            stop.status,
            RemoteReplyStatus::Accepted,
            "Stop sample {iteration}: {stop:?}; callback={telemetry:?}; graph={revisions:?}"
        );
        latencies.push(start.elapsed().as_secs_f64() * 1000.);
    }
    latencies.sort_by(f64::total_cmp);
    eprintln!(
        "Verified WSS Stop30 samples milliseconds median={:.3} p95={:.3} max={:.3}",
        latencies[15], latencies[28], latencies[29]
    );
    let reply = client
        .request(
            RemoteCommand::Play { track_only: None },
            None,
            BTreeMap::new(),
            Duration::from_secs(3),
        )
        .unwrap();
    assert_eq!(reply.status, RemoteReplyStatus::Accepted, "{reply:?}");
    let (entered, ready) = sync_channel(1);
    let (release, wait) = sync_channel(1);
    let wait = Mutex::new(wait);
    let release = Release(Some(release));
    shared.lock().unwrap().edit_hook = Some(Arc::new(move || {
        entered.send(()).unwrap();
        wait.lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(15))
            .unwrap();
    }));
    let mut track = shared.lock().unwrap().authority.session.tracks[0].clone();
    track.name = "Verified prepared vocal".into();
    track.mix_automation.insert(
        "pan".into(),
        (0..100000)
            .map(|frame| soundroom_engine::model::AutomationPoint {
                frame,
                value: 0.5,
                curve: "linear".into(),
                curve_start: 0.,
                curve_end: 1.,
                extra: BTreeMap::new(),
            })
            .collect(),
    );
    let track = serde_json::from_value(wire(&track).unwrap()).unwrap();
    let edit = client
        .enqueue(
            RemoteCommand::ReplaceTracks {
                tracks: vec![track],
                name: Some("Prepare vocal".into()),
                gesture_id: None,
            },
            None,
            BTreeMap::from([(track_id, 0)]),
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(12);
    while ready.try_recv().is_err() {
        assert!(
            Instant::now() < deadline,
            "Full preparation did not reach gate"
        );
        client.poll(Duration::from_millis(25)).unwrap();
    }
    let start = Instant::now();
    let reply = client
        .request(
            RemoteCommand::Stop,
            None,
            BTreeMap::new(),
            Duration::from_secs(3),
        )
        .unwrap();
    assert_eq!(reply.status, RemoteReplyStatus::Accepted);
    assert!(
        client.receipt(&edit).is_none(),
        "Edit must remain gated until Stop acknowledges"
    );
    assert_eq!(shared.lock().unwrap().authority.revision, 0);
    assert!(matches!(
        shared.lock().unwrap().authority.transport,
        soundroom_engine::authority::Transport::Stopped { .. }
    ));
    eprintln!(
        "Verified WSS Stop acknowledged while 100K-point preparation blocked: {:?}",
        start.elapsed()
    );
    drop(release);
    let receipt = client.wait_receipt(&edit, Duration::from_secs(5)).unwrap();
    assert_eq!(receipt.status, RemoteReplyStatus::Accepted);
    assert_eq!(
        shared.lock().unwrap().authority.session.tracks[0].name,
        "Verified prepared vocal"
    );
    assert!(matches!(
        shared.lock().unwrap().authority.transport,
        soundroom_engine::authority::Transport::Stopped { .. }
    ));
    let deadline = Instant::now() + Duration::from_secs(15);
    while client.snapshot.as_ref().unwrap().revision != 1 {
        assert!(Instant::now() < deadline);
        client.poll(Duration::from_millis(25)).unwrap();
    }
    // The shared-state fix must work through this actual endpoint as well as
    // pure engine tests. Every receipt below requires callback adoption, with
    // the original aggregate graph admission cap still enforced.
    let dense_start = Instant::now();
    for iteration in 0..3 {
        let play = client
            .request(
                RemoteCommand::Play { track_only: None },
                None,
                BTreeMap::new(),
                Duration::from_secs(3),
            )
            .unwrap();
        assert_eq!(
            play.status,
            RemoteReplyStatus::Accepted,
            "Dense Play{iteration}: {play:?}"
        );
        {
            let state = shared.lock().unwrap();
            let backend = state.backend.as_ref().unwrap();
            assert_eq!(backend.graph_revisions().applied, Some(1));
            assert!(backend.inner_telemetry_for_test().unwrap().playing);
        }
        let stop = client
            .request(
                RemoteCommand::Stop,
                None,
                BTreeMap::new(),
                Duration::from_secs(3),
            )
            .unwrap();
        assert_eq!(
            stop.status,
            RemoteReplyStatus::Accepted,
            "Dense Stop{iteration}: {stop:?}"
        );
    }
    eprintln!(
        "Verified WSS dense100K graph callback-confirmed Play/Stop3 cycles: {:?}",
        dense_start.elapsed()
    );
    // Drain recent metadata, then qualify sleeping connection behavior without
    // changing the callback's existing2ms pump or opening any hardware.
    let drain = Instant::now() + Duration::from_millis(200);
    while Instant::now() < drain {
        client.poll(Duration::from_millis(10)).unwrap();
    }
    let signal = {
        shared
            .lock()
            .unwrap()
            .peers
            .values()
            .next()
            .unwrap()
            .out
            .wake
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .clone()
    };
    let before = signal.counts();
    fn cpu_seconds() -> f64 {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        assert_eq!(
            unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) },
            0
        );
        let usage = unsafe { usage.assume_init() };
        usage.ru_utime.tv_sec as f64
            + usage.ru_utime.tv_usec as f64 / 1_000_000.
            + usage.ru_stime.tv_sec as f64
            + usage.ru_stime.tv_usec as f64 / 1_000_000.
    }
    let cpu = cpu_seconds();
    let idle = Instant::now();
    std::thread::sleep(Duration::from_secs(2));
    let process_cpu = (cpu_seconds() - cpu) / idle.elapsed().as_secs_f64() * 100.;
    let after = signal.counts();
    eprintln!("Portable verified endpoint idle2s processCPU={process_cpu:.3}% peerWaits={} peerWakes={} (includes simulated128-frame callback every2ms)",after.0-before.0,after.1-before.1);
    assert!(
        after.0 - before.0 < 20,
        "Idle connection must sleep, not poll every25ms"
    );
    assert!(
        process_cpu < 5.,
        "Idle control plus simulated callback CPU unexpectedly high"
    );
    drop(client);
    drop(runner);
    drop(shared);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "Explicit verified localhost TLS acceptance requires private fixture identity/CA; opens no hardware"]
fn verified_wss_pairing_continues_and_revoke_preserves_operator() {
    use soundroom_client::transport::ControlTransport;
    let identity = std::env::var_os("SOUNDROOM_TEST_TLS_P12").unwrap();
    let password = std::env::var("SOUNDROOM_TEST_TLS_PASSWORD").unwrap();
    let ca = std::env::var_os("SOUNDROOM_TEST_TLS_CA").unwrap();
    let (root, shared, _, _) = super::publication_tests::control_fixture(0);
    let (operator, grant) = {
        let mut state = shared.lock().unwrap();
        let operator = Credential::from_bytes(&serde_json::to_vec(&state.registry.issue("Operator", RemoteRole::Operator).unwrap()).unwrap()).unwrap();
        let grant = state.registry.grant("Browser", RemoteRole::Editor).unwrap();
        (operator, grant)
    };
    let operator_id = operator.credential_id.clone();
    let endpoint = Endpoint::bind(EndpointConfig {bind: "127.0.0.1:0".parse().unwrap(), approved_origins: vec!["https://localhost".into()]}, Path::new(&identity), &password).unwrap();
    let config = WssConfig {endpoint: format!("wss://localhost:{}/control",endpoint.address().unwrap().port()),origin:Some("https://localhost".into()),root_ca_pem:Some(std::fs::read(ca).unwrap()),connect_timeout:Duration::from_secs(10)};
    let stop=Arc::new(AtomicBool::new(false));let listener_stop=stop.clone();let listener_shared=shared.clone();
    let listener=std::thread::spawn(move||endpoint.run(listener_shared,listener_stop).unwrap());
    let runner=Runner{stop,joins:vec![listener]};
    fn receive(transport:&mut WssTransport,kind:&str)->Value {
        let deadline=Instant::now()+Duration::from_secs(10);
        loop {
            assert!(Instant::now()<deadline,"Expected protocol phase {kind}");
            if let Some(bytes)=transport.receive(Duration::from_millis(50)).unwrap(){
                let value:Value=serde_json::from_slice(&bytes).unwrap();
                if value["kind"]==kind{return value;}
                if value["kind"]=="heartbeat"{transport.send(&serde_json::to_vec(&json!({"version":3,"kind":"heartbeatReply","id":value["id"]})).unwrap()).unwrap();}
            }
        }
    }
    let mut browser=WssTransport::connect(&config).unwrap();
    let challenge=receive(&mut browser,"challenge");
    browser.send(&serde_json::to_vec(&json!({"version":3,"kind":"pair","code":grant["code"],"name":"Browser"})).unwrap()).unwrap();
    let paired=receive(&mut browser,"paired");
    let credential=&paired["credential"];
    let runtime=soundroom_client::auth::new_id().unwrap();
    let secret=base64::engine::general_purpose::STANDARD.decode(credential["clientSecret"].as_str().unwrap()).unwrap();
    let text=format!("soundroom-auth-v2|{}|{}|{}|Browser|{}",credential["serverID"].as_str().unwrap(),credential["credentialID"].as_str().unwrap(),runtime,challenge["nonce"].as_str().unwrap());
    let proof=base64::engine::general_purpose::STANDARD.encode(soundroom_client::auth::hmac_sha256(&secret,text.as_bytes()));
    browser.send(&serde_json::to_vec(&json!({"version":3,"kind":"hello","serverID":credential["serverID"],"credentialID":credential["credentialID"],"peer":{"id":runtime,"name":"Browser","role":"operator"},"proof":proof})).unwrap()).unwrap();
    let welcome=receive(&mut browser,"welcome");
    assert_eq!(welcome["role"],"editor","Approved role must override the claimed role");
    let mut operator=Client::<WssTransport>::connect(&config,operator,"Operator",None).unwrap();
    let deadline=Instant::now()+Duration::from_secs(10);
    while operator.connection!=ConnectionState::Ready{assert!(Instant::now()<deadline);operator.poll(Duration::from_millis(25)).unwrap();}
    let revoke=|id:&str|serde_json::from_value(json!({"type":"editor","intent":{"operation":"server.revokeWebCredential","context":{"cursorFrame":0,"clipIDs":[]},"integers":{},"numbers":{},"strings":{"credentialID":id},"flags":{}}})).unwrap();
    let denied=operator.request(revoke(&operator_id),None,BTreeMap::new(),Duration::from_secs(3)).unwrap();
    assert_eq!(denied.status,RemoteReplyStatus::Rejected);
    let revoked=operator.request(revoke(credential["credentialID"].as_str().unwrap()),None,BTreeMap::new(),Duration::from_secs(3)).unwrap();
    assert_eq!(revoked.status,RemoteReplyStatus::Accepted);
    let ping=operator.request(RemoteCommand::Ping { client_time: 0.0 },None,BTreeMap::new(),Duration::from_secs(3)).unwrap();
    assert_eq!(ping.status,RemoteReplyStatus::Accepted);
    assert_eq!(operator.connection,ConnectionState::Ready);
    let _=browser.close();drop(browser);drop(operator);drop(runner);drop(shared);std::fs::remove_dir_all(root).unwrap();
}
