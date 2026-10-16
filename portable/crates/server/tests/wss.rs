#![cfg(any(unix, windows))]
use soundroom_client::{
    auth::{new_id, Credential},
    client::{Client, ConnectionState},
    models::{RemoteCommand, RemoteReplyStatus, RemoteRole},
    transport::{WssConfig, WssTransport},
};
use soundroom_server::{
    endpoint::{validate_upgrade, Endpoint, EndpointConfig},
    registry::{private_directory, Registry},
    service::Service,
};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::PathBuf,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
struct Fixture {
    root: PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    config: WssConfig,
    operator: Credential,
    editor: Credential,
    viewer: Credential,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("soundroom-wss-{}", new_id().unwrap()));
        private_directory(&root).unwrap();
        let mut registry = Registry::create(&root, "WSS qualification").unwrap();
        let operator = Credential::from_bytes(
            &serde_json::to_vec(&registry.issue("Operator", RemoteRole::Operator).unwrap())
                .unwrap(),
        )
        .unwrap();
        let editor = Credential::from_bytes(
            &serde_json::to_vec(&registry.issue("Editor", RemoteRole::Editor).unwrap()).unwrap(),
        )
        .unwrap();
        let viewer = Credential::from_bytes(
            &serde_json::to_vec(&registry.issue("Viewer", RemoteRole::Viewer).unwrap()).unwrap(),
        )
        .unwrap();
        make_identity(&root);
        let web = root.join("web");
        std::fs::create_dir(&web).unwrap();
        std::fs::write(web.join("index.html"), b"Soundroom verified HTTPS fixture").unwrap();
        let endpoint = Endpoint::bind(
            EndpointConfig {
                bind: "127.0.0.1:0".parse().unwrap(),
                approved_origins: vec!["https://localhost".into()],
            },
            &root.join("identity.p12"),
            "fixture",
        )
        .unwrap()
        .with_web_assets(&web)
        .unwrap();
        let config = WssConfig {
            endpoint: format!(
                "wss://localhost:{}/control",
                endpoint.address().unwrap().port()
            ),
            origin: Some("https://localhost".into()),
            root_ca_pem: Some(std::fs::read(root.join("cert.pem")).unwrap()),
            connect_timeout: Duration::from_secs(10),
        };
        let service = Service::open(&root, None).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let thread = std::thread::spawn(move || endpoint.run(service, s).unwrap());
        Self {
            root,
            stop,
            thread: Some(thread),
            config,
            operator,
            editor,
            viewer,
        }
    }
    fn connect(&self, credential: Credential, name: &str) -> Client<WssTransport> {
        let mut c = Client::connect(&self.config, credential, name, None).unwrap();
        let end = Instant::now() + Duration::from_secs(15);
        while c.connection != ConnectionState::Ready {
            assert!(Instant::now() < end);
            c.poll(Duration::from_millis(25)).unwrap();
        }
        assert!(c.capabilities.is_some());
        c
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.thread.take().unwrap().join().unwrap();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn request(
    c: &mut Client<WssTransport>,
    command: RemoteCommand,
    expected: Option<i64>,
    tracks: BTreeMap<String, i64>,
) -> soundroom_client::models::RemoteReply {
    c.request(command, expected, tracks, Duration::from_secs(15))
        .unwrap()
}
fn sync(c: &mut Client<WssTransport>, revision: i64) {
    let end = Instant::now() + Duration::from_secs(10);
    while c.snapshot.as_ref().unwrap().revision < revision {
        assert!(Instant::now() < end);
        c.poll(Duration::from_millis(25)).unwrap();
    }
}
#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "Requires Security/keychain access outside the managed execution sandbox; no audio is opened"
)]
fn verified_wss_auth_multi_client_history_upload_project_export() {
    let f = Fixture::new();
    assert_https_bundle(&f);
    let mut operator = f.connect(f.operator.clone(), "Operator");
    let mut editor = f.connect(f.editor.clone(), "Editor");
    let mut viewer = f.connect(f.viewer.clone(), "Viewer");
    assert_eq!(viewer.role, Some(RemoteRole::Viewer));
    let rejected = request(
        &mut viewer,
        RemoteCommand::AddTrack {
            name: "Denied".into(),
        },
        Some(0),
        BTreeMap::new(),
    );
    assert_eq!(rejected.status, RemoteReplyStatus::Rejected);
    let added = request(
        &mut editor,
        RemoteCommand::AddTrack {
            name: "Vocal".into(),
        },
        Some(0),
        BTreeMap::new(),
    );
    assert_eq!(added.status, RemoteReplyStatus::Accepted);
    sync(&mut editor, added.revision);
    sync(&mut operator, added.revision);
    sync(&mut viewer, added.revision);
    let track = editor
        .snapshot
        .as_ref()
        .unwrap()
        .session
        .tracks
        .as_ref()
        .unwrap()[0]
        .id
        .clone()
        .unwrap();
    let observed = BTreeMap::from([(track.clone(), 1)]);
    assert_eq!(
        request(
            &mut operator,
            RemoteCommand::SetTrackGain {
                track_id: track.clone(),
                gain: 0.7
            },
            None,
            observed.clone()
        )
        .status,
        RemoteReplyStatus::Accepted
    );
    assert_eq!(
        request(
            &mut editor,
            RemoteCommand::SetTrackPan {
                track_id: track.clone(),
                pan: 0.2
            },
            None,
            observed
        )
        .status,
        RemoteReplyStatus::Conflict
    );
    let protected = request(&mut editor, RemoteCommand::Undo, None, BTreeMap::new());
    assert_eq!(protected.status, RemoteReplyStatus::Conflict);
    assert_eq!(
        request(&mut operator, RemoteCommand::Undo, None, BTreeMap::new()).status,
        RemoteReplyStatus::Accepted
    );
    let undone = request(&mut editor, RemoteCommand::Undo, None, BTreeMap::new());
    assert_eq!(undone.status, RemoteReplyStatus::Accepted);
    sync(&mut operator, undone.revision);
    sync(&mut editor, undone.revision);
    assert!(editor
        .snapshot
        .as_ref()
        .unwrap()
        .session
        .tracks
        .as_ref()
        .unwrap()
        .is_empty());
    // Real local media crosses WSS in verified chunks; paths never cross the protocol.
    let input = f.root.join("input.wav");
    let mut writer = soundroom_engine::wave::WaveWriter::create(&input, 4800).unwrap();
    let audio = (0..4800)
        .map(|i| (i as f32 * 0.057).sin() * 0.15)
        .collect::<Vec<_>>();
    writer.write(&audio, &audio).unwrap();
    writer.finish().unwrap();
    let imported = editor.upload_file(&input, None, 0, |_, _| {}).unwrap();
    assert_eq!(imported.status, RemoteReplyStatus::Accepted);
    sync(&mut operator, imported.revision);
    sync(&mut editor, imported.revision);
    let save = request(
        &mut operator,
        RemoteCommand::SaveProject {
            name: "Studio".into(),
            replace_existing: None,
            title: None,
        },
        None,
        BTreeMap::new(),
    );
    assert_eq!(save.status, RemoteReplyStatus::Accepted);
    assert!(f
        .root
        .join("Projects/Studio.soundroom/session.json")
        .exists());
    let export = request(
        &mut editor,
        RemoteCommand::ExportMix { name: "Mix".into() },
        None,
        BTreeMap::new(),
    );
    assert_eq!(export.status, RemoteReplyStatus::Accepted);
    use base64::Engine;
    let artifact: soundroom_client::models::RemoteArtifact = serde_json::from_slice(
        &base64::engine::general_purpose::STANDARD
            .decode(export.payload.unwrap())
            .unwrap(),
    )
    .unwrap();
    let target = f.root.join("download.wav");
    editor
        .download_artifact(&artifact, &target, |_, _| {})
        .unwrap();
    let rendered = soundroom_engine::wave::WaveReader::open(&target).unwrap();
    assert!(rendered.info.frames >= 4800);
    let original_revision = operator.snapshot.as_ref().unwrap().revision;
    let opened = request(
        &mut operator,
        RemoteCommand::OpenProject {
            id: "Studio.soundroom".into(),
        },
        Some(original_revision),
        BTreeMap::new(),
    );
    assert_eq!(opened.status, RemoteReplyStatus::Accepted);
    sync(&mut viewer, opened.revision);
    let hardware = request(
        &mut operator,
        RemoteCommand::Play { track_only: None },
        None,
        BTreeMap::new(),
    );
    assert_eq!(hardware.status, RemoteReplyStatus::Busy);
    operator.close().unwrap();
    editor.close().unwrap();
    viewer.close().unwrap();
}
#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "Requires Security/keychain access outside the managed execution sandbox; no audio is opened"
)]
fn tls_rejects_untrusted_ca_wrong_hostname_and_unapproved_origin() {
    let f = Fixture::new();
    let mut config = f.config.clone();
    config.root_ca_pem = None;
    assert!(WssTransport::connect(&config).is_err());
    let mut config = f.config.clone();
    config.endpoint = config.endpoint.replace("localhost", "127.0.0.1");
    assert!(WssTransport::connect(&config).is_err());
    let mut config = f.config.clone();
    config.origin = Some("https://attacker.example".into());
    assert!(WssTransport::connect(&config).is_err());
}
#[test]
fn upgrade_rejects_duplicates_and_unsafe_origins_but_ignores_offered_extensions() {
    let head="GET /control HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: soundroom.v3\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n";
    assert!(validate_upgrade(head.as_bytes(), &[]).is_ok());
    // Browsers always offer compression; the server accepts without negotiating it.
    let offered = head.replace(
        "\r\n\r\n",
        "\r\nSec-WebSocket-Extensions: permessage-deflate; client_max_window_bits\r\nOrigin: https://localhost\r\n\r\n",
    );
    assert!(validate_upgrade(offered.as_bytes(), &["https://localhost".into()]).is_ok());
    for extra in [
        "Host: evil",
        "Content-Length: 1",
        "Origin: https://localhost.attacker.example",
    ] {
        let h = head.replace("\r\n\r\n", &format!("\r\n{extra}\r\n\r\n"));
        assert!(validate_upgrade(h.as_bytes(), &["https://localhost".into()]).is_err());
    }
}

#[cfg(unix)]
fn make_identity(root: &std::path::Path) {
    std::fs::write(root.join("openssl.cnf"),"[req]\nprompt=no\ndistinguished_name=dn\nx509_extensions=ext\n[dn]\nCN=localhost\n[ext]\nsubjectAltName=DNS:localhost\nbasicConstraints=critical,CA:TRUE\nkeyUsage=critical,digitalSignature,keyEncipherment,keyCertSign\nextendedKeyUsage=serverAuth\n").unwrap();
    for args in [
        vec![
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            "key.pem",
            "-out",
            "cert.pem",
            "-days",
            "1",
            "-config",
            "openssl.cnf",
        ],
        vec![
            "pkcs12",
            "-export",
            "-inkey",
            "key.pem",
            "-in",
            "cert.pem",
            "-out",
            "identity.p12",
            "-passout",
            "pass:fixture",
        ],
    ] {
        let output = Command::new("openssl")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        root.join("identity.p12"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
}
#[cfg(windows)]
fn make_identity(root: &std::path::Path) {
    // Windows runner uses its native certificate API; no OpenSSL installation is required.
    let script = r#"$ErrorActionPreference='Stop';
$certificate=New-SelfSignedCertificate -Type Custom -Subject 'CN=localhost' -DnsName 'localhost' -CertStoreLocation 'Cert:\CurrentUser\My' -NotAfter (Get-Date).AddDays(1) -KeyAlgorithm RSA -KeyLength 2048 -KeyExportPolicy Exportable -HashAlgorithm SHA256 -KeyUsage DigitalSignature,KeyEncipherment,CertSign -TextExtension @('2.5.29.19={critical}{text}ca=true','2.5.29.37={text}1.3.6.1.5.5.7.3.1');
try {
$password=ConvertTo-SecureString 'fixture' -AsPlainText -Force;
Export-PfxCertificate -Cert $certificate -FilePath (Join-Path (Get-Location) 'identity.p12') -Password $password | Out-Null;
$pem="-----BEGIN CERTIFICATE-----`n"+[Convert]::ToBase64String($certificate.RawData,[Base64FormattingOptions]::InsertLineBreaks)+"`n-----END CERTIFICATE-----`n";
[IO.File]::WriteAllText((Join-Path (Get-Location) 'cert.pem'),$pem,[Text.Encoding]::ASCII);
} finally { Remove-Item ('Cert:\CurrentUser\My\'+$certificate.Thumbprint) -Force }
"#;
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Windows TLS fixture creation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let identity = root.join("identity.p12");
    let bytes = std::fs::read(&identity).unwrap();
    std::fs::remove_file(&identity).unwrap();
    soundroom_server::registry::write_new_private(&identity, &bytes).unwrap();
}

fn assert_https_bundle(fixture: &Fixture) {
    let endpoint = url::Url::parse(&fixture.config.endpoint).unwrap();
    let cert =
        native_tls::Certificate::from_pem(fixture.config.root_ca_pem.as_ref().unwrap()).unwrap();
    let mut builder = native_tls::TlsConnector::builder();
    builder.min_protocol_version(Some(native_tls::Protocol::Tlsv12));
    builder.add_root_certificate(cert);
    let connector = builder.build().unwrap();
    let tcp = std::net::TcpStream::connect(("127.0.0.1", endpoint.port().unwrap())).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut tls = connector.connect("localhost", tcp).unwrap();
    tls.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .unwrap();
    let mut header = vec![];
    while !header.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        tls.read_exact(&mut byte).unwrap();
        header.push(byte[0]);
        assert!(header.len() < 16384);
    }
    let header = std::str::from_utf8(&header).unwrap();
    assert!(header.starts_with("HTTP/1.1 200 OK"));
    assert!(header.contains("Content-Security-Policy:"));
    let mut body = vec![0; b"Soundroom verified HTTPS fixture".len()];
    tls.read_exact(&mut body).unwrap();
    assert_eq!(body, b"Soundroom verified HTTPS fixture");
}
