use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use soundroom_client::{
    auth::{new_id, Credential},
    client::Client,
    models::RemoteArtifact,
    transport::ControlTransport,
    Error,
};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
const ID: &str = "12345678-1234-4234-8234-123456789ABC";
#[derive(Default)]
struct Server {
    uploaded: Vec<u8>,
    expected_bytes: u64,
    expected_digest: String,
    chunk_count: usize,
    artifact: Vec<u8>,
}
struct Wire {
    incoming: VecDeque<Vec<u8>>,
    shared: Arc<Mutex<Server>>,
}
impl ControlTransport for Wire {
    fn send(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let request: Value = serde_json::from_slice(bytes)?;
        if request["kind"] != "request" {
            return Ok(());
        }
        let c = &request["command"];
        let mut server = self.shared.lock().unwrap();
        let mut payload = None;
        match c["type"].as_str().unwrap() {
            "beginUpload" => {
                server.expected_bytes = c["bytes"].as_u64().unwrap();
                server.expected_digest = c["sha256"].as_str().unwrap().into();
            }
            "uploadChunk" => {
                assert_eq!(c["offset"].as_u64().unwrap(), server.uploaded.len() as u64);
                let data = base64::engine::general_purpose::STANDARD
                    .decode(c["data"].as_str().unwrap())
                    .unwrap();
                assert!(data.len() <= 262144);
                server.uploaded.extend(data);
                server.chunk_count += 1;
            }
            "finishUpload" => {
                assert_eq!(server.uploaded.len() as u64, server.expected_bytes);
                assert_eq!(digest(&server.uploaded), server.expected_digest);
            }
            "downloadArtifact" => {
                let offset = c["offset"].as_u64().unwrap() as usize;
                let wanted = c["bytes"].as_u64().unwrap() as usize;
                let end = (offset + wanted).min(server.artifact.len());
                let chunk = json!({"offset":offset,"complete":end==server.artifact.len(),"data":base64::engine::general_purpose::STANDARD.encode(&server.artifact[offset..end])});
                payload = Some(
                    base64::engine::general_purpose::STANDARD
                        .encode(serde_json::to_vec(&chunk).unwrap()),
                );
            }
            _ => panic!("Unexpected transfer command"),
        }
        self.incoming.push_back(serde_json::to_vec(&json!({"version":3,"kind":"reply","requestID":request["id"],"status":"accepted","revision":0,"message":"","payload":payload}))?);
        Ok(())
    }
    fn receive(&mut self, _: Duration) -> Result<Option<Vec<u8>>, Error> {
        Ok(self.incoming.pop_front())
    }
    fn close(&mut self) -> Result<(), Error> {
        Ok(())
    }
}
fn client(artifact: Vec<u8>) -> (Client<Wire>, Arc<Mutex<Server>>) {
    let server = Arc::new(Mutex::new(Server {
        artifact,
        ..Default::default()
    }));
    let secret = base64::engine::general_purpose::STANDARD.encode([0; 32]);
    let credential=Credential::from_bytes(&serde_json::to_vec(&json!({"version":3,"serverID":ID,"credentialID":ID,"clientSecret":secret,"role":"editor"})).unwrap()).unwrap();
    let incoming = VecDeque::from([
        serde_json::to_vec(&json!({"version":3,"kind":"challenge","serverID":ID,"nonce":secret}))
            .unwrap(),
        serde_json::to_vec(
            &json!({"version":3,"kind":"welcome","serverID":ID,"name":"Host","role":"editor"}),
        )
        .unwrap(),
    ]);
    (
        Client::attach(
            Wire {
                incoming,
                shared: server.clone(),
            },
            credential,
            "Transfer",
            None,
        )
        .unwrap(),
        server,
    )
}
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("soundroom-transfer-test-{}", new_id().unwrap()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
#[test]
fn uploads_are_chunked_hashed_and_paths_stay_local() {
    let scratch = Scratch::new();
    let path = scratch.file("voice.wav");
    let bytes = vec![0x37; 700000];
    std::fs::write(&path, &bytes).unwrap();
    let (mut c, server) = client(vec![]);
    let mut progress = vec![];
    c.upload_file(&path, None, 0, |n, total| {
        assert_eq!(total, 700000);
        progress.push(n)
    })
    .unwrap();
    let server = server.lock().unwrap();
    assert_eq!(server.chunk_count, 3);
    assert_eq!(server.uploaded, bytes);
    assert_eq!(progress, vec![262144, 524288, 700000]);
}
#[test]
fn downloads_preserve_raw_json_bytes_and_install_atomically_without_clobber() {
    let scratch = Scratch::new();
    let mut bytes = b"{\"seed\":18446744073709551615}".to_vec();
    bytes.extend(vec![b' '; 600000]);
    let artifact = RemoteArtifact {
        id: ID.into(),
        name: "project.json".into(),
        bytes: bytes.len() as i64,
        sha256: digest(&bytes),
    };
    let (mut c, _) = client(bytes.clone());
    let destination = scratch.file("project.json");
    c.download_artifact(&artifact, &destination, |_, _| {})
        .unwrap();
    assert_eq!(std::fs::read(&destination).unwrap(), bytes);
    assert!(c
        .download_artifact(&artifact, &destination, |_, _| {})
        .is_err());
    assert_eq!(std::fs::read(&destination).unwrap(), bytes);
    assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 1);
}
#[test]
fn bad_hash_never_installs_or_leaves_partial_files() {
    let scratch = Scratch::new();
    let artifact = RemoteArtifact {
        id: ID.into(),
        name: "bad.wav".into(),
        bytes: 6,
        sha256: "0".repeat(64),
    };
    let (mut c, _) = client(b"broken".to_vec());
    let destination = scratch.file("bad.wav");
    assert!(c
        .download_artifact(&artifact, &destination, |_, _| {})
        .is_err());
    assert!(!destination.exists());
    assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 0);
}
#[test]
fn empty_upload_and_nonregular_source_reject_before_network() {
    let scratch = Scratch::new();
    let path = scratch.file("empty.wav");
    std::fs::write(&path, []).unwrap();
    let (mut c, server) = client(vec![]);
    assert!(c.upload_file(&path, None, 0, |_, _| {}).is_err());
    assert!(c
        .upload_file(Path::new(&scratch.0), None, 0, |_, _| {})
        .is_err());
    assert_eq!(server.lock().unwrap().expected_bytes, 0);
}
