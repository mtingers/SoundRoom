use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use soundroom_client::{
    auth::new_id,
    models::{RemoteRole, RemoteSnapshotPage},
    snapshot::SnapshotAssembler,
    validation,
};
use soundroom_server::{
    registry::{private_directory, Registry},
    service::{Identity, Inbox, Outbox, Service, Shared},
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{atomic::AtomicUsize, Arc},
    time::{Duration, Instant},
};
struct Fixture {
    root: PathBuf,
    service: Shared,
    editor: Identity,
    operator: Identity,
    viewer: Identity,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("soundroom-control-{}", new_id().unwrap()));
        private_directory(&root).unwrap();
        let mut registry = Registry::create(&root, "Control qualification").unwrap();
        let mut identity = |name: &str, role: RemoteRole| {
            let c = registry.issue(name, role.clone()).unwrap();
            Identity {
                runtime: new_id().unwrap(),
                credential: c["credentialID"].as_str().unwrap().into(),
                name: name.into(),
                role,
            }
        };
        let editor = identity("Editor", RemoteRole::Editor);
        let operator = identity("Operator", RemoteRole::Operator);
        let viewer = identity("Viewer", RemoteRole::Viewer);
        let service = Service::open(&root, None).unwrap();
        Self {
            root,
            service,
            editor,
            operator,
            viewer,
        }
    }
    fn channel(&self) -> (Outbox, Inbox) {
        Outbox::pair(self.service.lock().unwrap().global())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn frame(p: &Identity, id: &str, revision: Option<u64>, tracks: Value, command: Value) -> Value {
    let mut v = json!({"version":3,"kind":"request","id":id,"clientID":p.runtime,"expectedTrackRevisions":tracks,"command":command});
    if let Some(r) = revision {
        v["expectedRevision"] = json!(r);
    }
    v
}
fn read(inbox: &Inbox) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(bytes) = inbox
            .priority
            .try_recv()
            .ok()
            .or_else(|| inbox.ordinary.try_recv().ok())
        {
            inbox.received(bytes.len());
            return serde_json::from_slice(&bytes).unwrap();
        }
        assert!(Instant::now() < deadline, "Response timeout");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn reply(inbox: &Inbox, id: &str) -> Value {
    loop {
        let v = read(inbox);
        if v["kind"] == "reply" && v["requestID"] == id {
            return v;
        }
    }
}
fn request(
    f: &Fixture,
    p: &Identity,
    out: &Outbox,
    inbox: &Inbox,
    revision: Option<u64>,
    tracks: Value,
    command: Value,
) -> Value {
    let id = new_id().unwrap();
    Service::dispatch(&f.service, p, out, frame(p, &id, revision, tracks, command)).unwrap();
    reply(inbox, &id)
}
fn payload(reply: &Value) -> Value {
    serde_json::from_slice(
        &base64::engine::general_purpose::STANDARD
            .decode(reply["payload"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap()
}
#[test]
fn catalog_parity_and_directional_framing() {
    let cap = Service::capabilities();
    validation::validate_model(&cap, "RemoteCapabilities").unwrap();
    for operation in cap["editorOperations"].as_array().unwrap() {
        assert!(
            soundroom_client::contract::EDITOR_OPERATIONS.contains(&operation.as_str().unwrap())
        );
    }
    for command in cap["commands"].as_array().unwrap() {
        assert!(soundroom_client::contract::COMMANDS.contains(&command.as_str().unwrap()));
    }
    assert_eq!(cap["effects"].as_array().unwrap().len(), 19);
    for (kind, parameters) in [
        ("vocalEnhancer", 9),
        ("sourceIsolator", 7),
        ("soundPutty", 16),
        ("gate", 7),
        ("utility", 8),
        ("guitarRig", 30),
        ("equalizer", 33),
    ] {
        let effect = cap["effects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|effect| effect["id"] == kind)
            .unwrap();
        assert_eq!(effect["parameters"].as_array().unwrap().len(), parameters);
        assert!(!effect["factoryPresets"].as_array().unwrap().is_empty());
    }
    assert_eq!(cap["instruments"].as_array().unwrap().len(), 11);
    let bytes = soundroom_client::websocket::client_frame(1, b"control").unwrap();
    let mut decoder = soundroom_client::websocket::Decoder::server();
    decoder.feed(&bytes).unwrap();
    assert_eq!(
        decoder.next(),
        Some(soundroom_client::websocket::FrameEvent::Message(
            b"control".to_vec()
        ))
    );
    let unmasked = soundroom_client::websocket::server_frame(1, b"control").unwrap();
    assert!(soundroom_client::websocket::Decoder::server()
        .feed(&unmasked)
        .is_err());
    assert!(soundroom_client::websocket::Decoder::default()
        .feed(&bytes)
        .is_err());
}

fn rename_command(name: &str) -> Value {
    json!({"type":"editor","intent":{"operation":"session.rename",
        "context":{"clipIDs":[],"cursorFrame":0},"integers":{},"numbers":{},
        "strings":{"name":name},"flags":{}}})
}

#[test]
fn session_rename_is_fenced_authorized_and_undoable_without_changing_tracks() {
    let f = Fixture::new();
    let (out, inbox) = f.channel();
    assert_eq!(
        request(
            &f,
            &f.editor,
            &out,
            &inbox,
            None,
            json!({}),
            json!({"type":"addTrack","name":"Voice"})
        )["status"],
        "accepted"
    );
    let original = f.service.lock().unwrap().authority.session.clone();
    let versions = f.service.lock().unwrap().authority.track_revisions.clone();
    for peer in [&f.viewer, &f.editor] {
        let expected = if peer.role == RemoteRole::Viewer {
            Some(1)
        } else {
            None
        };
        assert_ne!(
            request(
                &f,
                peer,
                &out,
                &inbox,
                expected,
                json!({}),
                rename_command("Denied")
            )["status"],
            "accepted"
        );
    }
    assert_eq!(
        request(
            &f,
            &f.editor,
            &out,
            &inbox,
            Some(1),
            json!({}),
            rename_command("  Studio voice  ")
        )["status"],
        "accepted"
    );
    {
        let state = f.service.lock().unwrap();
        assert_eq!(state.authority.session.name, "Studio voice");
        assert_eq!(state.authority.session.tracks, original.tracks);
        assert_eq!(state.authority.track_revisions, versions);
    }
    assert_eq!(
        request(
            &f,
            &f.editor,
            &out,
            &inbox,
            Some(1),
            json!({}),
            rename_command("Stale")
        )["status"],
        "conflict"
    );
    assert_eq!(
        request(
            &f,
            &f.editor,
            &out,
            &inbox,
            None,
            json!({}),
            json!({"type":"undo"})
        )["status"],
        "accepted"
    );
    assert_eq!(f.service.lock().unwrap().authority.session, original);
    assert_eq!(
        request(
            &f,
            &f.editor,
            &out,
            &inbox,
            None,
            json!({}),
            json!({"type":"redo"})
        )["status"],
        "accepted"
    );
    assert_eq!(
        f.service.lock().unwrap().authority.session.name,
        "Studio voice"
    );
    let revision = f.service.lock().unwrap().authority.revision;
    for invalid in ["", "../escape", "bad:name", "bad\nname"] {
        assert_ne!(
            request(
                &f,
                &f.editor,
                &out,
                &inbox,
                Some(revision),
                json!({}),
                rename_command(invalid)
            )["status"],
            "accepted"
        );
    }
}

#[test]
fn saving_updates_the_current_and_serialized_title_but_failed_save_does_not() {
    let f = Fixture::new();
    let (out, inbox) = f.channel();
    assert_eq!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            Some(0),
            json!({}),
            json!({"type":"saveProject","name":"Studio.soundroom"})
        )["status"],
        "accepted"
    );
    let document: Value = serde_json::from_slice(
        &std::fs::read(f.root.join("Projects/Studio.soundroom/session.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(document["name"], "Studio");
    assert_eq!(f.service.lock().unwrap().authority.session.name, "Studio");
    assert_eq!(
        request(
            &f,
            &f.editor,
            &out,
            &inbox,
            Some(1),
            json!({}),
            rename_command("Keep this newer title")
        )["status"],
        "accepted"
    );
    assert_ne!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            Some(2),
            json!({}),
            json!({"type":"saveProject","name":"Studio"})
        )["status"],
        "accepted"
    );
    assert_eq!(
        f.service.lock().unwrap().authority.session.name,
        "Keep this newer title"
    );
    assert_eq!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            Some(0),
            json!({}),
            json!({"type":"saveProject","name":"Stale copy"})
        )["status"],
        "conflict"
    );
    assert!(!f.root.join("Projects/Stale copy.soundroom").exists());
}

#[test]
fn opening_legacy_untitled_projects_uses_the_filename_but_preserves_explicit_titles() {
    let f = Fixture::new();
    let (out, inbox) = f.channel();
    let mut document = soundroom_engine::project::Project {
        session: soundroom_engine::model::Session::default(),
        media: BTreeMap::new(),
    };
    document
        .save_new(&f.root.join("Projects/Legacy song.soundroom"))
        .unwrap();
    assert_eq!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            Some(0),
            json!({}),
            json!({"type":"openProject","id":"Legacy song.soundroom"})
        )["status"],
        "accepted"
    );
    assert_eq!(
        f.service.lock().unwrap().authority.session.name,
        "Legacy song"
    );
    document.session.name = "Intentional title".into();
    document
        .save_new(&f.root.join("Projects/Different filename.soundroom"))
        .unwrap();
    assert_eq!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            Some(1),
            json!({}),
            json!({"type":"openProject","id":"Different filename.soundroom"})
        )["status"],
        "accepted"
    );
    assert_eq!(
        f.service.lock().unwrap().authority.session.name,
        "Intentional title"
    );
}
#[test]
fn opened_project_saves_to_the_same_filename_after_a_session_rename() {
    let f = Fixture::new();
    let (out, inbox) = f.channel();
    let saved = f.root.join("Projects/Original filename.soundroom");
    let mut document = soundroom_engine::project::Project {
        session: Default::default(),
        media: BTreeMap::new(),
    };
    document.session.name = "An intentional title".into();
    document.save_new(&saved).unwrap();
    let opened = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        Some(0),
        json!({}),
        json!({"type":"openProject","id":"Original filename.soundroom"}),
    );
    assert_eq!(opened["status"], "accepted");
    assert_eq!(
        f.service.lock().unwrap().snapshot().unwrap()["projectName"],
        "Original filename.soundroom"
    );
    let revision = f.service.lock().unwrap().authority.revision;
    assert_eq!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            Some(revision),
            json!({}),
            rename_command("Renamed session")
        )["status"],
        "accepted"
    );
    let revision = f.service.lock().unwrap().authority.revision;
    let save =
        json!({"type":"saveProject","name":"Original filename.soundroom","replaceExisting":true});
    assert_eq!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            Some(revision),
            json!({}),
            save.clone()
        )["status"],
        "accepted"
    );
    assert_eq!(
        soundroom_engine::project::Project::open(&saved)
            .unwrap()
            .session
            .name,
        "Renamed session"
    );
    assert!(!f.root.join("Projects/Renamed session.soundroom").exists());
    assert_eq!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            Some(revision),
            json!({}),
            save
        )["status"],
        "accepted"
    );
    assert_eq!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            Some(0),
            json!({}),
            json!({"type":"saveProject","name":"Original filename","replaceExisting":true})
        )["status"],
        "conflict"
    );
    assert_eq!(
        soundroom_engine::project::Project::open(&saved)
            .unwrap()
            .session
            .name,
        "Renamed session"
    );
}
#[cfg(target_os = "macos")]
#[test]
fn project_package_download_is_a_project_with_metadata_not_an_audio_mix() {
    let f = Fixture::new();
    let (out, inbox) = f.channel();
    let response = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        Some(0),
        json!({}),
        json!({"type":"editor","intent":{"operation":"project.package","context":{"cursorFrame":0,"clipIDs":[]},"integers":{},"numbers":{},"strings":{},"flags":{}}}),
    );
    assert_eq!(response["status"], "accepted", "{response}");
    let payload = base64::engine::general_purpose::STANDARD
        .decode(response["payload"].as_str().unwrap())
        .unwrap();
    let artifact: Value = serde_json::from_slice(&payload).unwrap();
    let file = f.root.join("Artifacts").join(format!(
        "{}.soundroom.zip",
        artifact["id"].as_str().unwrap()
    ));
    assert!(file.is_file());
    assert_eq!(
        file.metadata().unwrap().len(),
        artifact["bytes"].as_u64().unwrap()
    );
    let extracted = f.root.join("extracted");
    assert!(std::process::Command::new("/usr/bin/ditto")
        .args(["-x", "-k"])
        .arg(&file)
        .arg(&extracted)
        .status()
        .unwrap()
        .success());
    let restored =
        soundroom_engine::project::Project::open(&extracted.join("Session.soundroom")).unwrap();
    assert_eq!(
        restored.session,
        f.service.lock().unwrap().authority.session
    );
    let release = json!({"type":"editor","intent":{"operation":"project.package","context":{"cursorFrame":0,"clipIDs":[]},"integers":{},"numbers":{},"strings":{"release":artifact["id"]},"flags":{}}});
    assert_eq!(
        request(&f, &f.operator, &out, &inbox, None, json!({}), release)["status"],
        "accepted"
    );
    assert!(!file.exists());
    assert_eq!(
        request(
            &f,
            &f.viewer,
            &out,
            &inbox,
            None,
            json!({}),
            json!({"type":"editor","intent":{"operation":"project.package","context":{"cursorFrame":0,"clipIDs":[]},"integers":{},"numbers":{},"strings":{},"flags":{}}})
        )["status"],
        "rejected"
    );
}
#[test]
fn scoped_history_receipts_and_authentication_cannot_be_bypassed() {
    let f = Fixture::new();
    let (out, inbox) = f.channel();
    let id = new_id().unwrap();
    let original = frame(
        &f.editor,
        &id,
        Some(0),
        json!({}),
        json!({"type":"addTrack","name":"Vox"}),
    );
    Service::dispatch(&f.service, &f.editor, &out, original.clone()).unwrap();
    assert_eq!(reply(&inbox, &id)["status"], "accepted");
    Service::dispatch(&f.service, &f.editor, &out, original.clone()).unwrap();
    assert_eq!(reply(&inbox, &id)["revision"], 1);
    assert_eq!(f.service.lock().unwrap().authority.session.tracks.len(), 1);
    let mut changed = original.clone();
    changed["command"]["name"] = json!("Different");
    assert!(Service::dispatch(&f.service, &f.editor, &out, changed).is_err());
    let mut wrong = original.clone();
    wrong["clientID"] = json!(f.operator.runtime);
    assert!(Service::dispatch(&f.service, &f.editor, &out, wrong).is_err());
    let track = f.service.lock().unwrap().authority.session.tracks[0]
        .id
        .clone();
    let a = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        None,
        json!({track.clone():1}),
        json!({"type":"setTrackGain","trackID":track,"gain":0.5}),
    );
    assert_eq!(a["status"], "accepted");
    let undo = request(
        &f,
        &f.editor,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"undo"}),
    );
    assert_eq!(undo["status"], "conflict");
    assert_eq!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            None,
            json!({}),
            json!({"type":"undo"})
        )["status"],
        "accepted"
    );
    assert_eq!(
        request(
            &f,
            &f.editor,
            &out,
            &inbox,
            None,
            json!({}),
            json!({"type":"undo"})
        )["status"],
        "accepted"
    );
    assert!(f
        .service
        .lock()
        .unwrap()
        .authority
        .session
        .tracks
        .is_empty());
    assert_eq!(
        request(
            &f,
            &f.viewer,
            &out,
            &inbox,
            Some(4),
            json!({}),
            json!({"type":"addTrack","name":"Denied"})
        )["status"],
        "rejected"
    );
    f.service
        .lock()
        .unwrap()
        .registry
        .revoke(&f.editor.credential)
        .unwrap();
    assert!(Service::dispatch(&f.service, &f.editor, &out, original).is_err());
}
#[test]
fn registry_exact_hmac_approved_role_and_one_time_pairing() {
    let f = Fixture::new();
    let mut state = f.service.lock().unwrap();
    let credentials = state
        .registry
        .issue("Authenticated editor", RemoteRole::Editor)
        .unwrap();
    let secret = base64::engine::general_purpose::STANDARD
        .decode(credentials["clientSecret"].as_str().unwrap())
        .unwrap();
    let id = credentials["credentialID"].as_str().unwrap();
    let runtime = new_id().unwrap();
    let nonce = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
    let message = format!(
        "soundroom-auth-v2|{}|{}|{}|Authenticated editor|{}",
        state.registry.server_id, id, runtime, nonce
    );
    let proof = base64::engine::general_purpose::STANDARD.encode(
        soundroom_client::auth::hmac_sha256(&secret, message.as_bytes()),
    );
    let verified = state
        .registry
        .verify(
            &state.registry.server_id.to_ascii_lowercase(),
            &id.to_ascii_lowercase(),
            &runtime.to_ascii_lowercase(),
            "Authenticated editor",
            &nonce,
            &proof,
        )
        .unwrap();
    assert_eq!(verified.role, RemoteRole::Editor);
    assert!(state
        .registry
        .verify(
            &state.registry.server_id,
            id,
            &runtime,
            "Impersonated",
            "bad nonce",
            &proof
        )
        .is_err());
    assert!(state
        .registry
        .grant_with_lifetime("Browser", RemoteRole::Viewer, 0)
        .is_err());
    assert!(state
        .registry
        .grant_with_lifetime("Browser", RemoteRole::Viewer, 3601)
        .is_err());
    let longer = state
        .registry
        .grant_with_lifetime("Long-lived browser", RemoteRole::Editor, 1800)
        .unwrap();
    assert_eq!(longer["expiresInSeconds"], 1800);
    assert_eq!(
        state
            .registry
            .redeem(longer["code"].as_str().unwrap(), "Long-lived browser")
            .unwrap()["role"],
        "editor"
    );
    let grant = state
        .registry
        .grant("Paired viewer", RemoteRole::Viewer)
        .unwrap();
    let code = grant["code"].as_str().unwrap();
    assert!(state.registry.redeem(code, "Other name").is_err());
    assert!(state.registry.redeem(code, "Paired viewer").is_err());
    let grant = state
        .registry
        .grant("Paired viewer", RemoteRole::Viewer)
        .unwrap();
    let c = state
        .registry
        .redeem(grant["code"].as_str().unwrap(), "Paired viewer")
        .unwrap();
    assert_eq!(c["role"], "viewer");
}
#[test]
fn actual_media_worker_project_export_and_verified_chunks() {
    let f = Fixture::new();
    let (out, inbox) = f.channel();
    let input = f.root.join("source.wav");
    let mut writer = soundroom_engine::wave::WaveWriter::create(&input, 4800).unwrap();
    let audio = (0..4800)
        .map(|i| (i as f32 * 0.04).sin() * 0.2)
        .collect::<Vec<_>>();
    writer.write(&audio, &audio).unwrap();
    writer.finish().unwrap();
    let bytes = std::fs::read(&input).unwrap();
    let hash = format!("{:x}", Sha256::digest(&bytes));
    let upload = new_id().unwrap();
    assert_eq!(
        request(
            &f,
            &f.editor,
            &out,
            &inbox,
            None,
            json!({}),
            json!({"type":"beginUpload","id":upload,"name":"Vox.wav","bytes":bytes.len(),"sha256":hash})
        )["status"],
        "accepted"
    );
    assert_eq!(
        request(
            &f,
            &f.viewer,
            &out,
            &inbox,
            Some(0),
            json!({}),
            json!({"type":"finishUpload","id":upload,"frame":0})
        )["status"],
        "rejected"
    );
    assert_eq!(
        request(
            &f,
            &f.editor,
            &out,
            &inbox,
            None,
            json!({}),
            json!({"type":"uploadChunk","id":upload,"offset":0,"data":base64::engine::general_purpose::STANDARD.encode(&bytes)})
        )["status"],
        "accepted"
    );
    assert_eq!(
        request(
            &f,
            &f.editor,
            &out,
            &inbox,
            Some(0),
            json!({}),
            json!({"type":"finishUpload","id":upload,"frame":0})
        )["status"],
        "accepted"
    );
    assert_eq!(
        f.service.lock().unwrap().authority.session.tracks[0].clips[0].length,
        4800
    );
    let saved = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"saveProject","name":"Studio"}),
    );
    assert_eq!(saved["status"], "accepted");
    assert!(f
        .root
        .join("Projects/Studio.soundroom/session.json")
        .exists());
    let export = request(
        &f,
        &f.editor,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"exportMix","name":"Studio mix"}),
    );
    assert_eq!(export["status"], "accepted");
    let artifact = payload(&export);
    let chunk = payload(&request(
        &f,
        &f.editor,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"readArtifact","id":artifact["id"],"offset":0,"bytes":262144}),
    ));
    let data = base64::engine::general_purpose::STANDARD
        .decode(chunk["data"].as_str().unwrap())
        .unwrap();
    assert!(chunk["complete"].as_bool().unwrap());
    assert_eq!(format!("{:x}", Sha256::digest(&data)), artifact["sha256"]);
    assert_eq!(
        request(
            &f,
            &f.viewer,
            &out,
            &inbox,
            None,
            json!({}),
            json!({"type":"readArtifact","id":artifact["id"],"offset":0,"bytes":262144})
        )["status"],
        "rejected"
    );
}
#[test]
fn failed_import_does_not_publish_or_keep_an_orphan_asset() {
    let f = Fixture::new();
    let (out, inbox) = f.channel();
    let input = f.root.join("failed-source.wav");
    let mut writer = soundroom_engine::wave::WaveWriter::create(&input, 64).unwrap();
    writer.write(&[0.; 64], &[0.; 64]).unwrap();
    writer.finish().unwrap();
    let bytes = std::fs::read(&input).unwrap();
    let sha = format!("{:x}", Sha256::digest(&bytes));
    for missing in [true, false] {
        let upload = new_id().unwrap();
        assert_eq!(
            request(
                &f,
                &f.editor,
                &out,
                &inbox,
                None,
                json!({}),
                json!({"type":"beginUpload","id":upload,"name":"Take.wav","bytes":bytes.len(),"sha256":sha})
            )["status"],
            "accepted"
        );
        assert_eq!(
            request(
                &f,
                &f.editor,
                &out,
                &inbox,
                None,
                json!({}),
                json!({"type":"uploadChunk","id":upload,"offset":0,"data":base64::engine::general_purpose::STANDARD.encode(&bytes)})
            )["status"],
            "accepted"
        );
        let command = if missing {
            json!({"type":"finishUpload","id":upload,"trackID":new_id().unwrap(),"frame":0})
        } else {
            json!({"type":"finishUpload","id":upload,"frame":0})
        };
        let result = request(
            &f,
            &f.editor,
            &out,
            &inbox,
            Some(if missing { 0 } else { 99 }),
            json!({}),
            command,
        );
        assert_eq!(result["status"], "conflict", "{result}");
        let state = f.service.lock().unwrap();
        assert_eq!(state.authority.revision, 0);
        assert!(state.authority.session.tracks.is_empty());
        assert!(state.snapshot().unwrap()["assets"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(std::fs::read_dir(f.root.join("Media"))
            .unwrap()
            .next()
            .is_none());
        assert!(std::fs::read_dir(f.root.join("Uploads"))
            .unwrap()
            .next()
            .is_none());
        assert!(state.authority.history_state(&f.editor.runtime).0.is_none());
    }
}
#[test]
fn bounded_pagination_roundtrips_large_snapshot() {
    let f = Fixture::new();
    let mut state = f.service.lock().unwrap();
    let mut track = soundroom_engine::model::Track {
        id: new_id().unwrap(),
        ..Default::default()
    };
    track.mix_automation.insert(
        "volume".into(),
        (0..20000)
            .map(|i| soundroom_engine::model::AutomationPoint {
                frame: i,
                value: 0.5,
                curve: "linear".into(),
                curve_start: 0.,
                curve_end: 1.,
                extra: BTreeMap::new(),
            })
            .collect(),
    );
    state.authority.session.tracks.push(track.clone());
    state.authority.track_revisions.insert(track.id, 0);
    let snapshot = state.snapshot().unwrap();
    drop(state);
    let (out, inbox) = f.channel();
    soundroom_server::service::send_snapshot(&out, snapshot.clone()).unwrap();
    let mut assembler = SnapshotAssembler::default();
    let result = loop {
        let value = read(&inbox);
        if value["kind"] == "event" {
            continue;
        }
        let page: RemoteSnapshotPage = serde_json::from_value(value).unwrap();
        if let Some(result) = assembler.accept(page).unwrap() {
            break result;
        }
    };
    assert_eq!(result.revision, 0);
    assert_eq!(
        result.session.tracks.unwrap()[0]
            .mix_automation
            .as_ref()
            .unwrap()["volume"]
            .len(),
        20000
    );
}
#[test]
fn production_sdk_installs_neutral_paged_body_with_latest_live_metadata() {
    use soundroom_client::{auth::Credential, client::Client, transport::ControlTransport, Error};
    use std::collections::VecDeque;
    struct Wire {
        handshake: VecDeque<Vec<u8>>,
        inbox: Inbox,
    }
    impl ControlTransport for Wire {
        fn send(&mut self, _: &[u8]) -> Result<(), Error> {
            Ok(())
        }
        fn receive(&mut self, _: Duration) -> Result<Option<Vec<u8>>, Error> {
            if let Some(bytes) = self.handshake.pop_front() {
                return Ok(Some(bytes));
            }
            let bytes = self
                .inbox
                .priority
                .try_recv()
                .ok()
                .or_else(|| self.inbox.ordinary.try_recv().ok());
            if let Some(bytes) = &bytes {
                self.inbox.received(bytes.len());
            }
            Ok(bytes)
        }
        fn close(&mut self) -> Result<(), Error> {
            Ok(())
        }
    }
    let server = new_id().unwrap();
    let peer = new_id().unwrap();
    let snapshot = json!({"serverID":server,"serverName":"Host",
        "session":{"version":9,"name":"Song","tracks":(0..20).map(|_|json!({"id":new_id().unwrap(),"name":"x".repeat(60000)})).collect::<Vec<_>>()},
        "revision":5,"trackRevisions":{},"transport":{"frame":1,"isPlaying":false,"isRecording":false,"isPreparing":false,"hostTime":1},
        "devices":[],"inputDeviceID":0,"outputDeviceID":0,"assets":[],"status":"Captured earlier","peers":[]});
    let (out, inbox) = Outbox::pair(Arc::new(AtomicUsize::new(0)));
    // These arrive after capture. The immutable encoding worker must not replace
    // them with capture-time defaults, and Begin must replay them to the SDK.
    for event in [
        json!({"type":"status","message":"Latest preparation status"}),
        json!({"type":"transport","transport":{"frame":48000,"isPlaying":true,"isRecording":false,"isPreparing":false,"hostTime":2}}),
        json!({"type":"presence","peers":[{"id":peer,"name":"Native operator","role":"operator"}]}),
        json!({"type":"devices","devices":[{"id":3,"uid":"opaque-output","name":"Simulated route","inputChannels":0,"outputChannels":2}],"inputID":0,"outputID":3}),
    ] {
        out.send(&json!({"version":3,"kind":"event","event":event}), false)
            .unwrap();
    }
    soundroom_server::service::send_snapshot(&out, snapshot).unwrap();
    let credential = Credential::from_bytes(&serde_json::to_vec(&json!({"version":3,"serverID":server,"credentialID":new_id().unwrap(),"clientSecret":base64::engine::general_purpose::STANDARD.encode([0;32]),"role":"viewer"})).unwrap()).unwrap();
    let handshake = [json!({"version":3,"kind":"challenge","serverID":server,"nonce":base64::engine::general_purpose::STANDARD.encode([0;32])}),
        json!({"version":3,"kind":"welcome","serverID":server,"name":"Host","role":"viewer"})]
        .into_iter().map(|value|serde_json::to_vec(&value).unwrap()).collect();
    let mut client =
        Client::attach(Wire { handshake, inbox }, credential, "Qualification", None).unwrap();
    let deadline = Instant::now() + Duration::from_secs(4);
    while client.snapshot.is_none() {
        client.poll(Duration::ZERO).unwrap();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    let installed = client.snapshot.as_ref().unwrap();
    assert_eq!(installed.revision, 5);
    assert_eq!(installed.transport.frame, 48000);
    assert!(installed.transport.is_playing);
    assert_eq!(installed.status, "Latest preparation status");
    assert_eq!(installed.peers[0].id, peer);
    assert_eq!(installed.peers[0].role, RemoteRole::Operator);
    assert_eq!(installed.output_device_id, 3);
    assert_eq!(installed.devices[0].uid, "opaque-output");
}

#[test]
fn queue_has_fixed_control_reserve_and_releases_budget() {
    let global = Arc::new(AtomicUsize::new(0));
    let (out, inbox) = Outbox::pair(global.clone());
    for _ in 0..56 {
        out.send(&json!({"kind":"ordinary"}), false).unwrap();
    }
    for _ in 0..8 {
        out.send(&json!({"kind":"control"}), true).unwrap();
    }
    assert!(out.send(&json!({}), true).is_err());
    drop(inbox);
    assert_eq!(global.load(std::sync::atomic::Ordering::Acquire), 0);
}

#[test]
fn canonical_session_timing_supports_partial_changes_and_fences() {
    let f = Fixture::new();
    let (out, inbox) = f.channel();
    let intent = json!({"operation":"session.timing","context":{"clipIDs":[],"cursorFrame":0},"integers":{},"numbers":{"tempo":128},"strings":{},"flags":{}});
    let accepted = request(
        &f,
        &f.editor,
        &out,
        &inbox,
        Some(0),
        json!({}),
        json!({"type":"editor","intent":intent}),
    );
    assert_eq!(accepted["status"], "accepted");
    assert_eq!(
        f.service.lock().unwrap().authority.session.timing.tempo,
        128.
    );
    assert_eq!(
        request(
            &f,
            &f.editor,
            &out,
            &inbox,
            Some(0),
            json!({}),
            json!({"type":"editor","intent":intent})
        )["status"],
        "conflict"
    );
    let cancel = json!({"operation":"jobs.cancel","context":{"clipIDs":[],"cursorFrame":0},"integers":{},"numbers":{},"strings":{},"flags":{}});
    assert_eq!(
        request(
            &f,
            &f.editor,
            &out,
            &inbox,
            None,
            json!({}),
            json!({"type":"editor","intent":cancel})
        )["status"],
        "rejected"
    );
}
#[test]
fn designer_and_drums_are_server_jobs_with_history_and_stale_fences() {
    let f = Fixture::new();
    let (out, inbox) = f.channel();
    let mut pattern: Value = serde_json::from_str(include_str!(
        "../../../fixtures/instruments/voice-studioPiano.json"
    ))
    .unwrap();
    soundroom_server::service::normalize_seeds(&mut pattern, true);
    let intent = |document: &Value, operation: &str, strings: Value| json!({"operation":operation,"context":{"clipIDs":[],"cursorFrame":4800},"integers":{},"numbers":{},"strings":strings,"flags":{},"payload":base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(document).unwrap())});
    let inserted = request(
        &f,
        &f.editor,
        &out,
        &inbox,
        Some(0),
        json!({}),
        json!({"type":"editor","intent":intent(&pattern,"designer.commit",json!({}))}),
    );
    assert_eq!(inserted["status"], "accepted", "{inserted}");
    let clip_id = payload(&inserted)["clipID"].as_str().unwrap().to_owned();
    let track = f.service.lock().unwrap().authority.session.tracks[0]
        .id
        .clone();
    for pass in 0..6 {
        pattern["tone"] = json!(0.3 + pass as f64 * 0.05);
        let version = f.service.lock().unwrap().authority.track_revisions[&track];
        let rewritten = request(
            &f,
            &f.editor,
            &out,
            &inbox,
            None,
            json!({track.clone():version}),
            json!({"type":"editor","intent":intent(&pattern,"designer.commit",json!({"trackID":track,"clipID":clip_id}))}),
        );
        assert_eq!(rewritten["status"], "accepted");
    }
    let state = f.service.lock().unwrap();
    let clip = &state.authority.session.tracks[0].clips[0];
    assert_eq!(clip.start, 4800);
    assert_eq!(clip.id, clip_id);
    assert_eq!(clip.previous_states.len(), 5);
    let snapshot = state.snapshot().unwrap();
    validation::validate_model(&snapshot, "RemoteSnapshot").unwrap();
    drop(state);
    let stale = request(
        &f,
        &f.editor,
        &out,
        &inbox,
        None,
        json!({track.clone():1}),
        json!({"type":"editor","intent":intent(&pattern,"designer.commit",json!({"trackID":track,"clipID":clip_id}))}),
    );
    assert_eq!(stale["status"], "conflict");
    let drums: Value = serde_json::from_str(include_str!(
        "../../../fixtures/instruments/drum-pattern-electronic.json"
    ))
    .unwrap();
    let exported = request(
        &f,
        &f.editor,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"editor","intent":intent(&drums,"drums.export",json!({}))}),
    );
    assert_eq!(exported["status"], "accepted");
    assert!(payload(&exported)["bytes"].as_u64().unwrap() > 1000);
}

#[test]
fn inline_instrument_edits_preserve_slices_copies_and_live_transport_and_can_remove_last_note() {
    let f = Fixture::new();
    let _pump = Pump::attach(&f);
    let (out, inbox) = f.channel();
    let mut pattern: Value = serde_json::from_str(include_str!(
        "../../../fixtures/instruments/voice-studioPiano.json"
    ))
    .unwrap();
    pattern["instrument"] = json!("synthesizer");
    pattern["bpm"] = json!(120);
    pattern["beats"] = json!(4);
    soundroom_server::service::normalize_seeds(&mut pattern, true);
    let commit = |pattern: &Value, track: Option<&str>, clip: Option<&str>| {
        let mut strings = json!({});
        let mut expected = json!({});
        if let Some(track) = track {
            strings["trackID"] = json!(track);
            expected[track] = json!(f.service.lock().unwrap().authority.track_revisions[track]);
        }
        if let Some(clip) = clip {
            strings["clipID"] = json!(clip);
        }
        let revision = f.service.lock().unwrap().authority.revision;
        request(
            &f,
            &f.editor,
            &out,
            &inbox,
            Some(revision),
            expected,
            json!({"type":"editor","intent":{
                "operation":"designer.commit", "context":{"clipIDs":[],"cursorFrame":4800},
                "integers":{},"numbers":{},"strings":strings,"flags":{},
                "payload":base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(pattern).unwrap())
            }}),
        )
    };
    let inserted = commit(&pattern, None, None);
    assert_eq!(inserted["status"], "accepted", "{inserted}");
    let clip_id = payload(&inserted)["clipID"].as_str().unwrap().to_owned();
    let mut track = f.service.lock().unwrap().authority.session.tracks[0].clone();
    let track_id = track.id.clone();
    let original = track.clips[0].clone();
    track.clips[0].name = "Trimmed keys".into();
    track.clips[0].offset = 24000;
    track.clips[0].length = 48000;
    track.clips[0].fade_in_frames = Some(1000);
    let mut copy = track.clips[0].clone();
    copy.id = new_id().unwrap();
    copy.start = 200000;
    track.clips.push(copy.clone());
    // An older trim may have the source pattern only in its original PCM history.
    let mut history = serde_json::to_value(&original).unwrap();
    history.as_object_mut().unwrap().remove("previousStates");
    history.as_object_mut().unwrap().remove("start");
    history["operation"] = json!("Trim clip");
    history["createdAt"] = json!(0.);
    track.clips[0].previous_states = vec![serde_json::from_value(history).unwrap()];
    track.clips[0].extra.remove("soundDesign");
    let mut wire_track = serde_json::to_value(track).unwrap();
    soundroom_server::service::normalize_seeds(&mut wire_track, true);
    let version = f.service.lock().unwrap().authority.track_revisions[&track_id];
    assert_eq!(
        request(
            &f,
            &f.editor,
            &out,
            &inbox,
            None,
            json!({track_id.clone():version}),
            json!({"type":"replaceTracks","tracks":[wire_track]})
        )["status"],
        "accepted"
    );
    let copy_before =
        serde_json::to_value(&f.service.lock().unwrap().authority.session.tracks[0].clips[1])
            .unwrap();
    assert_eq!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            None,
            json!({}),
            json!({"type":"play"})
        )["status"],
        "accepted"
    );
    pattern["notes"][0]["velocity"] = json!(0.31);
    let edited = commit(&pattern, Some(&track_id), Some(&clip_id));
    assert_eq!(edited["status"], "accepted", "{edited}");
    {
        let state = f.service.lock().unwrap();
        let track = &state.authority.session.tracks[0];
        let clip = &track.clips[0];
        assert_eq!((clip.start, clip.offset, clip.length), (4800, 24000, 48000));
        assert_eq!(clip.name, "Trimmed keys");
        assert_eq!(clip.fade_in_frames, Some(1000));
        assert_ne!(clip.asset_id, original.asset_id);
        assert_eq!(clip.extra["soundDesign"]["notes"][0]["velocity"], 0.31);
        assert_eq!(serde_json::to_value(&track.clips[1]).unwrap(), copy_before);
        assert_eq!(state.snapshot().unwrap()["transport"]["isPlaying"], true);
    }
    pattern["notes"] = json!([]);
    let silent = commit(&pattern, Some(&track_id), Some(&clip_id));
    assert_eq!(silent["status"], "accepted", "{silent}");
    assert!(
        f.service.lock().unwrap().authority.session.tracks[0].clips[0].extra["soundDesign"]
            ["notes"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        commit(&pattern, None, None)["status"],
        "rejected",
        "Only replacements can remove the last note"
    );
    assert_eq!(
        request(
            &f,
            &f.editor,
            &out,
            &inbox,
            None,
            json!({}),
            json!({"type":"undo"})
        )["status"],
        "accepted"
    );
    assert_eq!(
        f.service.lock().unwrap().authority.session.tracks[0].clips[0].extra["soundDesign"]
            ["notes"][0]["velocity"],
        0.31
    );
}

#[test]
fn callback_clock_transport_and_stop_are_authoritative() {
    use soundroom_engine::realtime;
    use std::sync::atomic::{AtomicBool, Ordering};
    let f = Fixture::new();
    let (control, mut host) = realtime::channel();
    Service::attach_audio(&f.service, control).unwrap();
    let capabilities = f.service.lock().unwrap().endpoint_capabilities();
    validation::validate_model(&capabilities, "RemoteCapabilities").unwrap();
    assert!(capabilities["commands"]
        .as_array()
        .unwrap()
        .contains(&json!("play")));
    let (out, inbox) = f.channel();
    let stop = Arc::new(AtomicBool::new(false));
    let finished = stop.clone();
    // This manually pumped owner opens no audio device and produces no hardware sound.
    let callback = std::thread::spawn(move || {
        let mut left = [0.; 512];
        let mut right = [0.; 512];
        while !finished.load(Ordering::Acquire) {
            host.render(&mut left, &mut right);
            std::thread::sleep(Duration::from_millis(3));
        }
    });
    let denied = request(
        &f,
        &f.viewer,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"play","trackOnly":null}),
    );
    assert_eq!(denied["status"], "rejected");
    let played = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"play","trackOnly":null}),
    );
    assert_eq!(played["status"], "accepted", "{played}");
    let active_edit = request(
        &f,
        &f.editor,
        &out,
        &inbox,
        Some(0),
        json!({}),
        json!({"type":"addTrack","name":"No stale graph"}),
    );
    assert_eq!(active_edit["status"], "accepted", "{active_edit}");
    let sought = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"seek","frame":4800}),
    );
    assert_eq!(sought["status"], "accepted", "{sought}");
    assert!(
        matches!(f.service.lock().unwrap().authority.transport, soundroom_engine::authority::Transport::Playing { frame } if frame>=4800)
    );
    let paused = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"stop"}),
    );
    assert_eq!(paused["status"], "accepted", "{paused}");
    let frame = f.service.lock().unwrap().snapshot().unwrap()["transport"]["frame"].clone();
    std::thread::sleep(Duration::from_millis(80));
    assert_eq!(
        f.service.lock().unwrap().snapshot().unwrap()["transport"]["frame"],
        frame
    );
    let seek_paused = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"seek","frame":2400}),
    );
    assert_eq!(seek_paused["status"], "accepted");
    let snapshot = f.service.lock().unwrap().snapshot().unwrap();
    assert_eq!(snapshot["transport"]["frame"], 2400);
    assert_eq!(snapshot["transport"]["isPlaying"], false);
    stop.store(true, Ordering::Release);
    callback.join().unwrap();
}
// A source-only callback runner: no device enumeration, hardware opening or sound output.
struct Pump {
    stop: Arc<std::sync::atomic::AtomicBool>,
    callback: Option<std::thread::JoinHandle<()>>,
}
impl Pump {
    fn attach(f: &Fixture) -> Self {
        use std::sync::atomic::{AtomicBool, Ordering};
        let (control, mut host) = soundroom_engine::realtime::channel();
        Service::attach_audio(&f.service, control).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let finished = stop.clone();
        let callback = std::thread::spawn(move || {
            let mut left = [0.; 128];
            let mut right = [0.; 128];
            while !finished.load(Ordering::Acquire) {
                host.render(&mut left, &mut right);
                std::thread::sleep(Duration::from_millis(2));
            }
        });
        Self {
            stop,
            callback: Some(callback),
        }
    }
}
impl Drop for Pump {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        self.callback.take().unwrap().join().unwrap();
    }
}
fn wait_applied(f: &Fixture, revision: u64) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = f.service.lock().unwrap();
        let revisions = state.graph_revisions().unwrap();
        assert_eq!(revisions.authored, revision);
        if revisions.applied == Some(revision) {
            assert_eq!(revisions.pending, None);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Graph update timeout: {revisions:?}"
        );
        drop(state);
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn drum_clip_replacements_and_undo_keep_the_mix_running_and_copies_independent() {
    let f = Fixture::new();
    let _pump = Pump::attach(&f);
    let (out, inbox) = f.channel();
    let mut pattern: Value = serde_json::from_str(include_str!(
        "../../../fixtures/instruments/drum-pattern-electronic.json"
    ))
    .unwrap();
    let intent = |pattern: &Value, strings: Value| {
        json!({
            "operation":"drums.commit", "context":{"clipIDs":[],"cursorFrame":4800},
            "integers":{},"numbers":{},"strings":strings,"flags":{},
            "payload":base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(pattern).unwrap())
        })
    };
    let first = request(
        &f,
        &f.editor,
        &out,
        &inbox,
        Some(0),
        json!({}),
        json!({"type":"editor","intent":intent(&pattern,json!({}))}),
    );
    assert_eq!(first["status"], "accepted", "{first}");
    let clip = payload(&first)["clipID"].as_str().unwrap().to_owned();
    let track = f.service.lock().unwrap().authority.session.tracks[0]
        .id
        .clone();
    let copy = request(
        &f,
        &f.editor,
        &out,
        &inbox,
        None,
        json!({track.clone():1}),
        json!({"type":"editor","intent":intent(&pattern,json!({"trackID":track}))}),
    );
    assert_eq!(copy["status"], "accepted", "{copy}");
    let original = f.service.lock().unwrap().authority.session.tracks[0].clone();
    assert_eq!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            None,
            json!({}),
            json!({"type":"play"})
        )["status"],
        "accepted"
    );
    wait_applied(&f, 2);
    let before = f.service.lock().unwrap().snapshot().unwrap()["transport"]["frame"]
        .as_u64()
        .unwrap();
    pattern["steps"][0]["velocity"] = json!(0.23);
    let edited = request(
        &f,
        &f.editor,
        &out,
        &inbox,
        None,
        json!({track.clone():2}),
        json!({"type":"editor","intent":intent(&pattern,json!({"trackID":track,"clipID":clip}))}),
    );
    assert_eq!(edited["status"], "accepted", "{edited}");
    wait_applied(&f, 3);
    {
        let state = f.service.lock().unwrap();
        let changed = &state.authority.session.tracks[0];
        assert_eq!(changed.clips[0].id, clip);
        assert_eq!(changed.clips[0].start, original.clips[0].start);
        assert_ne!(changed.clips[0].asset_id, original.clips[0].asset_id);
        assert_eq!(
            changed.clips[0].extra["drumPattern"]["steps"][0]["velocity"],
            0.23
        );
        assert_eq!(changed.clips[0].previous_states.len(), 1);
        assert_eq!(
            serde_json::to_value(&changed.clips[1]).unwrap(),
            serde_json::to_value(&original.clips[1]).unwrap()
        );
        let snapshot = state.snapshot().unwrap();
        assert_eq!(snapshot["transport"]["isPlaying"], true);
        assert!(snapshot["transport"]["frame"].as_u64().unwrap() > before);
    }
    assert_eq!(
        request(
            &f,
            &f.editor,
            &out,
            &inbox,
            None,
            json!({}),
            json!({"type":"undo"})
        )["status"],
        "accepted"
    );
    wait_applied(&f, 4);
    assert_eq!(
        f.service.lock().unwrap().authority.session.tracks[0].clips[0].asset_id,
        original.clips[0].asset_id
    );
    assert_eq!(
        request(
            &f,
            &f.editor,
            &out,
            &inbox,
            None,
            json!({}),
            json!({"type":"redo"})
        )["status"],
        "accepted"
    );
    wait_applied(&f, 5);
    let state = f.service.lock().unwrap();
    assert_eq!(state.snapshot().unwrap()["transport"]["isPlaying"], true);
    assert_eq!(
        state.authority.session.tracks[0].clips[0].extra["drumPattern"]["steps"][0]["velocity"],
        0.23
    );
}

#[test]
fn drum_loops_end_on_bars_and_slices_remain_editable_after_replacement() {
    let f = Fixture::new();
    let _pump = Pump::attach(&f);
    let (out, inbox) = f.channel();
    let mut pattern: Value = serde_json::from_str(include_str!(
        "../../../fixtures/instruments/drum-pattern-electronic.json"
    ))
    .unwrap();
    pattern["bpm"] = json!(120);
    pattern["variationCycles"] = json!(1);
    pattern["bars"] = json!(1);
    let commit = |pattern: &Value, track: Option<&str>, clip: Option<&str>| {
        let mut strings = json!({});
        let mut expected = json!({});
        if let Some(track) = track {
            strings["trackID"] = json!(track);
            expected[track] = json!(f.service.lock().unwrap().authority.track_revisions[track]);
        }
        if let Some(clip) = clip {
            strings["clipID"] = json!(clip);
        }
        let revision = f.service.lock().unwrap().authority.revision;
        request(
            &f,
            &f.editor,
            &out,
            &inbox,
            Some(revision),
            expected,
            json!({"type":"editor","intent":{
                "operation":"drums.commit", "context":{"clipIDs":[],"cursorFrame":4800},
                "integers":{},"numbers":{},"strings":strings,"flags":{},
                "payload":base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(pattern).unwrap())
            }}),
        )
    };
    let inserted = commit(&pattern, None, None);
    assert_eq!(inserted["status"], "accepted", "{inserted}");
    let clip_id = payload(&inserted)["clipID"].as_str().unwrap().to_owned();
    let track_id = f.service.lock().unwrap().authority.session.tracks[0]
        .id
        .clone();
    for bars in [1, 2, 4, 8] {
        pattern["bars"] = json!(bars);
        let reply = commit(&pattern, Some(&track_id), Some(&clip_id));
        assert_eq!(reply["status"], "accepted", "{reply}");
        let state = f.service.lock().unwrap();
        let clip = &state.authority.session.tracks[0].clips[0];
        assert_eq!(
            clip.length,
            96_000 * bars,
            "Timeline loops must have no extra render tail"
        );
        assert_eq!(clip.offset, 0);
    }
    pattern["bars"] = json!(2);
    pattern["variationCycles"] = json!(2);
    assert_eq!(
        commit(&pattern, Some(&track_id), Some(&clip_id))["status"],
        "accepted"
    );
    let mut track = f.service.lock().unwrap().authority.session.tracks[0].clone();
    assert_eq!(
        track.clips[0].length, 384_000,
        "Variation passes stay intact"
    );
    let original = track.clips[0].clone();
    track.clips[0].start = 20_000;
    track.clips[0].offset = 24_000;
    track.clips[0].length = 48_000;
    track.clips[0].fade_in_frames = Some(1_000);
    let copy_id = new_id().unwrap();
    let mut copy = track.clips[0].clone();
    copy.id = copy_id.clone();
    copy.start += copy.length;
    // Emulate an old GUI trim: the source metadata survives only in history.
    let mut history = serde_json::to_value(&original).unwrap();
    history.as_object_mut().unwrap().remove("previousStates");
    history.as_object_mut().unwrap().remove("start");
    history["operation"] = json!("Trim clip");
    history["createdAt"] = json!(0.);
    copy.previous_states = vec![serde_json::from_value(history).unwrap()];
    copy.extra.remove("drumPattern");
    track.clips.push(copy);
    let version = f.service.lock().unwrap().authority.track_revisions[&track_id];
    let trimmed = request(
        &f,
        &f.editor,
        &out,
        &inbox,
        None,
        json!({track_id.clone():version}),
        json!({"type":"replaceTracks","tracks":[track]}),
    );
    assert_eq!(trimmed["status"], "accepted", "{trimmed}");
    assert_eq!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            None,
            json!({}),
            json!({"type":"play"})
        )["status"],
        "accepted"
    );
    let before = f.service.lock().unwrap().authority.session.tracks[0].clone();
    pattern["steps"][0]["velocity"] = json!(0.31);
    for id in [&clip_id, &copy_id] {
        let reply = commit(&pattern, Some(&track_id), Some(id));
        assert_eq!(reply["status"], "accepted", "{reply}");
        let state = f.service.lock().unwrap();
        let clip = state.authority.session.tracks[0]
            .clips
            .iter()
            .find(|c| &c.id == id)
            .unwrap();
        let old = before.clips.iter().find(|c| &c.id == id).unwrap();
        assert_eq!(
            (clip.start, clip.offset, clip.length),
            (old.start, 24_000, 48_000)
        );
        assert_eq!(clip.fade_in_frames, Some(1_000));
        assert_ne!(clip.asset_id, old.asset_id);
        assert_eq!(clip.extra["drumPattern"]["steps"][0]["velocity"], 0.31);
        assert_eq!(state.snapshot().unwrap()["transport"]["isPlaying"], true);
    }
    let version = f.service.lock().unwrap().authority.track_revisions[&track_id];
    let split = request(
        &f,
        &f.editor,
        &out,
        &inbox,
        None,
        json!({track_id.clone():version}),
        json!({"type":"splitClip","trackID":track_id,"clipID":clip_id,"frame":44_000}),
    );
    assert_eq!(split["status"], "accepted", "{split}");
    let split_right = f.service.lock().unwrap().authority.session.tracks[0].clips[1].clone();
    assert!(split_right.extra.contains_key("drumPattern"));
    assert_eq!(
        commit(&pattern, Some(&track_id), Some(&split_right.id))["status"],
        "accepted"
    );
    let state = f.service.lock().unwrap();
    let edited = state.authority.session.tracks[0]
        .clips
        .iter()
        .find(|c| c.id == split_right.id)
        .unwrap();
    assert_eq!(
        (edited.start, edited.offset, edited.length),
        (44_000, 48_000, 24_000)
    );
}

#[test]
fn live_authored_edits_and_history_catch_up_without_transport_jump() {
    let f = Fixture::new();
    let _pump = Pump::attach(&f);
    let (out, inbox) = f.channel();
    assert_eq!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            None,
            json!({}),
            json!({"type":"play"})
        )["status"],
        "accepted"
    );
    wait_applied(&f, 0);
    let before = f.service.lock().unwrap().snapshot().unwrap()["transport"]["frame"]
        .as_u64()
        .unwrap();
    assert_eq!(
        request(
            &f,
            &f.editor,
            &out,
            &inbox,
            Some(0),
            json!({}),
            json!({"type":"addTrack", "name":"Live instrument"})
        )["status"],
        "accepted"
    );
    wait_applied(&f, 1);
    let track = f.service.lock().unwrap().authority.session.tracks[0]
        .id
        .clone();
    // Same-author gesture commits remain responsive while the latest graph is preparing.
    for (version, pan) in [(1, -0.25), (2, 0.25), (3, 0.75)] {
        let response = request(
            &f,
            &f.editor,
            &out,
            &inbox,
            None,
            json!({track.clone():version}),
            json!({"type":"setTrackPan", "trackID":track, "pan":pan}),
        );
        assert_eq!(response["status"], "accepted", "{response}");
    }
    wait_applied(&f, 4);
    assert_eq!(
        request(
            &f,
            &f.editor,
            &out,
            &inbox,
            None,
            json!({}),
            json!({"type":"undo"})
        )["status"],
        "accepted"
    );
    wait_applied(&f, 5);
    let state = f.service.lock().unwrap();
    assert_eq!(state.authority.session.tracks[0].pan, 0.25);
    let snapshot = state.snapshot().unwrap();
    assert_eq!(snapshot["transport"]["isPlaying"], true);
    assert!(snapshot["transport"]["frame"].as_u64().unwrap() > before);
}
#[test]
fn accepted_edit_preparation_failure_preserves_authored_history_and_old_audio() {
    let f = Fixture::new();
    let _pump = Pump::attach(&f);
    let (out, inbox) = f.channel();
    assert_eq!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            None,
            json!({}),
            json!({"type":"play"})
        )["status"],
        "accepted"
    );
    wait_applied(&f, 0);
    // Authored storage permits this document; callback DSP admission is deliberately
    // smaller. Preparation failure must not turn the accepted commit into rejection.
    let tracks = (0..129)
        .map(|index| json!({"id":new_id().unwrap(),"name":format!("Layer {index}")}))
        .collect::<Vec<_>>();
    let id = new_id().unwrap();
    let edit = frame(
        &f.editor,
        &id,
        Some(0),
        json!({}),
        json!({"type":"editTracks", "edit":{
        "name":"Large layer stack", "tracks":tracks, "removedTrackIDs":[]}}),
    );
    Service::dispatch(&f.service, &f.editor, &out, edit.clone()).unwrap();
    assert_eq!(reply(&inbox, &id)["status"], "accepted");
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let state = f.service.lock().unwrap();
        let snapshot = state.snapshot().unwrap();
        if snapshot["status"].as_str().unwrap().contains("128-track") {
            assert_eq!(state.authority.revision, 1);
            assert_eq!(state.authority.session.tracks.len(), 129);
            assert_eq!(state.graph_revisions().unwrap().applied, Some(0));
            assert_eq!(state.graph_revisions().unwrap().pending, Some(1));
            assert_eq!(snapshot["transport"]["isPlaying"], true);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Missing preparation failure status: {snapshot}"
        );
        drop(state);
        std::thread::sleep(Duration::from_millis(5));
    }
    // Exact retry returns the accepted receipt, without re-executing the authored edit.
    Service::dispatch(&f.service, &f.editor, &out, edit).unwrap();
    assert_eq!(reply(&inbox, &id)["revision"], 1);
    assert_eq!(
        request(
            &f,
            &f.editor,
            &out,
            &inbox,
            None,
            json!({}),
            json!({"type":"undo"})
        )["status"],
        "accepted"
    );
    wait_applied(&f, 2);
    assert!(f
        .service
        .lock()
        .unwrap()
        .authority
        .session
        .tracks
        .is_empty());
}
#[test]
fn stop_supersedes_live_preparation_and_keeps_accepted_edit() {
    let f = Fixture::new();
    let _pump = Pump::attach(&f);
    let (out, inbox) = f.channel();
    assert_eq!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            None,
            json!({}),
            json!({"type":"play"})
        )["status"],
        "accepted"
    );
    assert_eq!(
        request(
            &f,
            &f.editor,
            &out,
            &inbox,
            Some(0),
            json!({}),
            json!({"type":"addTrack", "name":"Retained after Stop"})
        )["status"],
        "accepted"
    );
    assert_eq!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            None,
            json!({}),
            json!({"type":"stop"})
        )["status"],
        "accepted"
    );
    let frozen = f.service.lock().unwrap().snapshot().unwrap()["transport"]["frame"].clone();
    std::thread::sleep(Duration::from_millis(350));
    let state = f.service.lock().unwrap();
    let snapshot = state.snapshot().unwrap();
    assert_eq!(state.authority.revision, 1);
    assert_eq!(state.authority.session.tracks.len(), 1);
    assert_eq!(snapshot["transport"]["isPlaying"], false);
    assert_eq!(snapshot["transport"]["frame"], frozen);
    drop(state);
    assert_eq!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            None,
            json!({}),
            json!({"type":"play"})
        )["status"],
        "accepted"
    );
    wait_applied(&f, 1);
}

#[test]
fn missing_callback_never_accepts_transport_or_invents_clock() {
    let f = Fixture::new();
    let (control, _host) = soundroom_engine::realtime::channel();
    Service::attach_audio(&f.service, control).unwrap();
    let (out, inbox) = f.channel();
    let reply = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"play","trackOnly":null}),
    );
    assert_ne!(reply["status"], "accepted");
    assert_eq!(
        f.service.lock().unwrap().snapshot().unwrap()["transport"]["frame"],
        0
    );
    assert_eq!(
        f.service.lock().unwrap().snapshot().unwrap()["transport"]["isPlaying"],
        false
    );
}

#[cfg(unix)]
#[test]
fn private_files_locks_and_tls_identity_reject_aliases_and_broad_modes() {
    use soundroom_server::registry::{read_private, write_new_private, RootLock};
    use std::os::unix::fs::{symlink, PermissionsExt};
    let f = Fixture::new();
    let path = f.root.join("identity.p12");
    write_new_private(&path, b"not an identity").unwrap();
    std::fs::hard_link(&path, f.root.join("identity-alias.p12")).unwrap();
    assert!(read_private(&path, 1024).is_err());
    std::fs::remove_file(f.root.join("identity-alias.p12")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(read_private(&path, 1024).is_err());
    assert!(soundroom_server::endpoint::Endpoint::bind(
        soundroom_server::endpoint::EndpointConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            approved_origins: vec![]
        },
        &path,
        "not logged"
    )
    .is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&path, f.root.join("identity-link.p12")).unwrap();
    assert!(read_private(&f.root.join("identity-link.p12"), 1024).is_err());
    let first = RootLock::acquire(&f.root).unwrap();
    assert!(RootLock::acquire(&f.root).is_err());
    drop(first);
    std::fs::hard_link(f.root.join("service.lock"), f.root.join("lock-alias")).unwrap();
    assert!(RootLock::acquire(&f.root).is_err());
}

#[test]
fn history_events_are_private_and_gesture_labels_survive_reconciliation() {
    let f = Fixture::new();
    let (editor_out, editor_in) = f.channel();
    let (operator_out, operator_in) = f.channel();
    f.service
        .lock()
        .unwrap()
        .join(f.editor.clone(), editor_out.clone())
        .unwrap();
    f.service
        .lock()
        .unwrap()
        .join(f.operator.clone(), operator_out.clone())
        .unwrap();
    for inbox in [&editor_in, &operator_in] {
        while let Some(bytes) = inbox
            .priority
            .try_recv()
            .ok()
            .or_else(|| inbox.ordinary.try_recv().ok())
        {
            inbox.received(bytes.len());
        }
    }
    let added = request(
        &f,
        &f.editor,
        &editor_out,
        &editor_in,
        Some(0),
        json!({}),
        json!({"type":"addTrack","name":"Gesture"}),
    );
    assert_eq!(added["status"], "accepted");
    let history = |inbox: &Inbox| loop {
        let event = read(inbox);
        if event["kind"] == "event" && event["event"]["type"] == "history" {
            break event["event"].clone();
        }
    };
    let e = history(&editor_in);
    let o = history(&operator_in);
    assert_eq!(e["canUndo"], true);
    assert_eq!(e["undoName"], "Add track");
    assert_eq!(o["canUndo"], false);
    assert_eq!(o["undoName"], Value::Null);
    let track = f.service.lock().unwrap().authority.session.tracks[0]
        .id
        .clone();
    for gain in [0.7, 0.3] {
        let mut value =
            serde_json::to_value(&f.service.lock().unwrap().authority.session.tracks[0]).unwrap();
        value["gain"] = json!(gain);
        let version = f.service.lock().unwrap().authority.track_revisions[&track];
        let result = request(
            &f,
            &f.editor,
            &editor_out,
            &editor_in,
            None,
            json!({track.clone():version}),
            json!({"type":"replaceTracks","tracks":[value],"name":"Adjust volume","gestureID":"fader-1"}),
        );
        assert_eq!(result["status"], "accepted");
        assert_eq!(history(&editor_in)["undoName"], "Adjust volume");
        assert_eq!(history(&operator_in)["canUndo"], false);
    }
    let undone = request(
        &f,
        &f.editor,
        &editor_out,
        &editor_in,
        None,
        json!({}),
        json!({"type":"undo"}),
    );
    assert_eq!(undone["status"], "accepted");
    assert_eq!(
        f.service.lock().unwrap().authority.session.tracks[0].gain,
        1.
    );
    let event = history(&editor_in);
    assert_eq!(event["redoName"], "Adjust volume");
    assert_eq!(event["canRedo"], true);
}

#[test]
fn oversized_delta_uses_shared_pages_and_coalesces_edits_during_transfer() {
    use std::sync::atomic::Ordering;
    let f = Fixture::new();
    let id = new_id().unwrap();
    {
        let mut state = f.service.lock().unwrap();
        let mut track = soundroom_engine::model::Track {
            id: id.clone(),
            ..Default::default()
        };
        for target in ["volume", "pan"] {
            track.mix_automation.insert(
                target.into(),
                (0..60000)
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
        }
        state.authority.session.tracks.push(track);
        state.authority.track_revisions.insert(id.clone(), 0);
    }
    let (out_a, in_a) = f.channel();
    let (out_b, in_b) = f.channel();
    f.service
        .lock()
        .unwrap()
        .join(f.editor.clone(), out_a.clone())
        .unwrap();
    f.service
        .lock()
        .unwrap()
        .join(f.operator.clone(), out_b.clone())
        .unwrap();
    for inbox in [&in_a, &in_b] {
        while let Some(bytes) = inbox
            .priority
            .try_recv()
            .ok()
            .or_else(|| inbox.ordinary.try_recv().ok())
        {
            inbox.received(bytes.len());
        }
    }
    let result = request(
        &f,
        &f.editor,
        &out_a,
        &in_a,
        None,
        json!({id.clone():0}),
        json!({"type":"setTrackGain","trackID":id,"gain":0.5}),
    );
    assert_eq!(result["status"], "accepted", "{result}");
    let doc_bytes = serde_json::to_vec(&f.service.lock().unwrap().snapshot().unwrap())
        .unwrap()
        .len();
    assert!(doc_bytes > soundroom_client::validation::MAX_FRAME_BYTES);
    // Immutable encoding is retained once while page frames have independent queue budgets.
    std::thread::sleep(Duration::from_millis(40));
    assert!(
        f.service.lock().unwrap().global().load(Ordering::Acquire) < doc_bytes + 5 * 1024 * 1024
    );
    let second = request(
        &f,
        &f.editor,
        &out_a,
        &in_a,
        None,
        json!({id.clone():1}),
        json!({"type":"setTrackPan","trackID":id,"pan":0.2}),
    );
    assert_eq!(second["status"], "accepted");
    let third = request(
        &f,
        &f.editor,
        &out_a,
        &in_a,
        None,
        json!({id.clone():2}),
        json!({"type":"setTrackGain","trackID":id,"gain":0.3}),
    );
    assert_eq!(third["status"], "accepted");
    for inbox in [&in_a, &in_b] {
        let mut assembler = SnapshotAssembler::default();
        let mut revisions = vec![];
        loop {
            let value = read(inbox);
            assert_ne!(
                value["event"]["type"], "delta",
                "No revision gaps may leak across paged replacement"
            );
            if value["kind"]
                .as_str()
                .is_some_and(|s| s.starts_with("snapshot"))
            {
                let page: RemoteSnapshotPage = serde_json::from_value(value).unwrap();
                if let Some(snapshot) = assembler.accept(page).unwrap() {
                    revisions.push(snapshot.revision);
                    if snapshot.revision == 3 {
                        let track = &snapshot.session.tracks.as_ref().unwrap()[0];
                        assert_eq!(track.gain, Some(0.3));
                        assert_eq!(track.pan, Some(0.2));
                        break;
                    }
                }
            }
        }
        assert_eq!(revisions.first(), Some(&1));
        assert_eq!(revisions.last(), Some(&3));
        assert!(revisions.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(revisions.len() <= 3);
    }
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        f.service.lock().unwrap().global().load(Ordering::Acquire),
        0
    );
}
#[test]
fn mastering_analysis_proposal_and_apply_are_authored_and_undoable() {
    let f = Fixture::new();
    let (out, inbox) = f.channel();
    let editor = |op: &str, numbers: Value, payload: Option<&Value>| {
        let mut intent = json!({"operation":op,"context":{"clipIDs":[],"cursorFrame":0},"integers":{},"numbers":numbers,"strings":{},"flags":{}});
        if let Some(p) = payload {
            intent["payload"] =
                json!(base64::engine::general_purpose::STANDARD
                    .encode(serde_json::to_vec(p).unwrap()));
        }
        json!({"type":"editor","intent":intent})
    };
    let analysis = |f: &Fixture| {
        f.service
            .lock()
            .unwrap()
            .authority
            .session
            .extra
            .get("masteringAnalysis")
            .cloned()
    };
    let refused = request(
        &f,
        &f.editor,
        &out,
        &inbox,
        None,
        json!({}),
        editor("master.analyze", json!({}), None),
    );
    assert_eq!(
        refused["status"], "rejected",
        "an empty mix has nothing to analyze"
    );
    let input = f.root.join("mix.wav");
    let frames = 96_000;
    let mut writer = soundroom_engine::wave::WaveWriter::create(&input, frames).unwrap();
    let left = (0..frames)
        .map(|i| (i as f32 * 0.0144).sin() * 0.3)
        .collect::<Vec<_>>();
    let right = (0..frames)
        .map(|i| (i as f32 * 0.37).sin() * 0.1)
        .collect::<Vec<_>>();
    for (l, r) in left.chunks(8192).zip(right.chunks(8192)) {
        writer.write(l, r).unwrap();
    }
    writer.finish().unwrap();
    let bytes = std::fs::read(&input).unwrap();
    let upload = new_id().unwrap();
    let hash = format!("{:x}", Sha256::digest(&bytes));
    let mut commands = vec![
        json!({"type":"beginUpload","id":upload,"name":"Mix.wav","bytes":bytes.len(),"sha256":hash}),
    ];
    for (i, chunk) in bytes.chunks(262144).enumerate() {
        commands.push(json!({"type":"uploadChunk","id":upload,"offset":i * 262144,"data":base64::engine::general_purpose::STANDARD.encode(chunk)}));
    }
    commands.push(json!({"type":"finishUpload","id":upload,"frame":0}));
    for command in commands {
        assert_eq!(
            request(&f, &f.editor, &out, &inbox, None, json!({}), command)["status"],
            "accepted"
        );
    }
    let early = request(
        &f,
        &f.editor,
        &out,
        &inbox,
        None,
        json!({}),
        editor("master.proposal", json!({}), None),
    );
    assert_eq!(early["status"], "rejected");
    assert_eq!(
        early["message"],
        "Analyze the mix before requesting a mastering proposal."
    );
    let revision = f.service.lock().unwrap().authority.revision;
    // The native GUI fences only its context track; analysis is server-placed.
    let analyzed = request(
        &f,
        &f.editor,
        &out,
        &inbox,
        None,
        json!({}),
        editor("master.analyze", json!({}), None),
    );
    assert_eq!(analyzed["status"], "accepted", "{analyzed}");
    assert_eq!(analyzed["revision"], revision + 1);
    let stored = analysis(&f).expect("analysis is stored in the session");
    validation::validate_model(&stored, "MasteringAnalysis").unwrap();
    assert_eq!(stored["analyzedFrames"], frames);
    assert!(stored["integratedLUFS"].as_f64().unwrap() < -5.);
    {
        let state = f.service.lock().unwrap();
        let session = &state.authority.session;
        assert_eq!(
            stored["sourceFingerprint"].as_str(),
            Some(soundroom_engine::mastering::fingerprint(session).as_str())
        );
        assert!(session.version >= 7);
        let snapshot = state.snapshot().unwrap();
        validation::validate_model(&snapshot, "RemoteSnapshot").unwrap();
        assert_eq!(snapshot["session"]["masteringAnalysis"], stored);
    }
    for numbers in [
        json!({"targetLUFS":-30}),
        json!({"targetLUFS":-14,"intensity":1.5}),
    ] {
        let invalid = request(
            &f,
            &f.editor,
            &out,
            &inbox,
            None,
            json!({}),
            editor("master.proposal", numbers, None),
        );
        assert_eq!(invalid["status"], "rejected");
    }
    let proposal = request(
        &f,
        &f.viewer,
        &out,
        &inbox,
        None,
        json!({}),
        editor(
            "master.proposal",
            json!({"targetLUFS":-12,"intensity":0.8}),
            None,
        ),
    );
    assert_eq!(proposal["status"], "accepted", "{proposal}");
    let effects = payload(&proposal);
    validation::validate_model(&effects, "[RealtimeEffect]").unwrap();
    let kinds = effects
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(kinds, ["equalizer", "resonance", "warmth", "limiter"]);
    assert_eq!(effects[3]["selectedAutomationParameter"], "input");
    assert_eq!(effects[3]["parameters"]["ceiling"], -1.0);
    let master = soundroom_engine::model::MASTER_ID;
    let version = f.service.lock().unwrap().authority.track_revisions[master];
    let viewer = request(
        &f,
        &f.viewer,
        &out,
        &inbox,
        None,
        json!({master:version}),
        editor("master.applyProposal", json!({}), Some(&effects)),
    );
    assert_eq!(viewer["status"], "rejected");
    let stale = request(
        &f,
        &f.editor,
        &out,
        &inbox,
        None,
        json!({master:version + 1}),
        editor("master.applyProposal", json!({}), Some(&effects)),
    );
    assert_eq!(stale["status"], "conflict");
    let applied = request(
        &f,
        &f.editor,
        &out,
        &inbox,
        None,
        json!({master:version}),
        editor("master.applyProposal", json!({}), Some(&effects)),
    );
    assert_eq!(applied["status"], "accepted", "{applied}");
    {
        let state = f.service.lock().unwrap();
        let rack =
            serde_json::to_value(&state.authority.session.master_track.realtime_effects).unwrap();
        // The rack stores f32 values; compare the proposal at that precision.
        let stored: Vec<soundroom_engine::model::Effect> =
            serde_json::from_value(effects.clone()).unwrap();
        assert_eq!(rack, serde_json::to_value(stored).unwrap());
        assert_eq!(
            state.authority.track_revisions[master],
            applied["revision"].as_u64().unwrap()
        );
    }
    let undo = request(
        &f,
        &f.editor,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"undo"}),
    );
    assert_eq!(undo["status"], "accepted");
    assert!(f
        .service
        .lock()
        .unwrap()
        .authority
        .session
        .master_track
        .realtime_effects
        .is_empty());
    let undo = request(
        &f,
        &f.editor,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"undo"}),
    );
    assert_eq!(undo["status"], "accepted");
    assert!(analysis(&f).is_none(), "Undo removes the analysis");
    let redo = request(
        &f,
        &f.editor,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"redo"}),
    );
    assert_eq!(redo["status"], "accepted");
    assert_eq!(analysis(&f), Some(stored));
    // Any mix change invalidates the stored analysis for proposals and Apply.
    let track = f.service.lock().unwrap().authority.session.tracks[0]
        .id
        .clone();
    let version = f.service.lock().unwrap().authority.track_revisions[&track];
    assert_eq!(
        request(
            &f,
            &f.editor,
            &out,
            &inbox,
            None,
            json!({track.clone():version}),
            json!({"type":"setTrackGain","trackID":track,"gain":0.5})
        )["status"],
        "accepted"
    );
    let changed = request(
        &f,
        &f.editor,
        &out,
        &inbox,
        None,
        json!({}),
        editor("master.proposal", json!({}), None),
    );
    assert_eq!(changed["status"], "rejected");
    assert_eq!(
        changed["message"],
        "Mix changed; analyze again before using a proposal."
    );
}

#[test]
fn pairing_lifetime_is_operator_selected_and_bounded() {
    let f = Fixture::new();
    let (out, inbox) = f.channel();
    let command = |seconds| json!({"type":"editor","intent":{"operation":"server.webPairing","context":{"cursorFrame":0,"clipIDs":[]},"integers":{"expiresInSeconds":seconds},"numbers":{},"strings":{"name":"Browser","role":"editor"},"flags":{}}});
    let denied = request(&f, &f.editor, &out, &inbox, None, json!({}), command(1800));
    assert_ne!(denied["status"], "accepted");
    let accepted = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        None,
        json!({}),
        command(1800),
    );
    assert_eq!(accepted["status"], "accepted");
    assert_eq!(payload(&accepted)["expiresInSeconds"], 1800);
    let invalid = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        None,
        json!({}),
        command(3601),
    );
    assert_ne!(invalid["status"], "accepted");
}

#[test]
fn browser_access_cannot_revoke_its_own_operator_connection() {
    let f = Fixture::new();
    let (out, inbox) = f.channel();
    let result = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"editor","intent":{"operation":"server.revokeWebCredential","context":{"cursorFrame":0,"clipIDs":[]},"integers":{},"numbers":{},"strings":{"credentialID":f.operator.credential.to_ascii_lowercase()},"flags":{}}}),
    );
    assert_ne!(result["status"], "accepted");
    assert!(f
        .service
        .lock()
        .unwrap()
        .registry
        .contains(&f.operator.credential));
    let still_connected = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"ping","clientTime":0}),
    );
    assert_eq!(still_connected["status"], "accepted");
}

#[test]
fn connected_operators_can_restart_stopped_transport_without_disconnect() {
    let f = Fixture::new();
    let _pump = Pump::attach(&f);
    let credential = f
        .service
        .lock()
        .unwrap()
        .registry
        .issue("Browser", RemoteRole::Operator)
        .unwrap();
    let browser = Identity {
        runtime: new_id().unwrap(),
        credential: credential["credentialID"].as_str().unwrap().into(),
        name: "Browser".into(),
        role: RemoteRole::Operator,
    };
    let (gui_out, gui_in) = f.channel();
    let (browser_out, browser_in) = f.channel();
    f.service
        .lock()
        .unwrap()
        .join(f.operator.clone(), gui_out.clone())
        .unwrap();
    f.service
        .lock()
        .unwrap()
        .join(browser.clone(), browser_out.clone())
        .unwrap();
    let command = |peer: &Identity, out: &Outbox, inbox: &Inbox, kind: &str| {
        request(&f, peer, out, inbox, None, json!({}), json!({"type":kind}))
    };
    assert_eq!(
        command(&f.operator, &gui_out, &gui_in, "play")["status"],
        "accepted"
    );
    assert_eq!(
        command(&browser, &browser_out, &browser_in, "snapshot")["status"],
        "accepted"
    );
    assert_eq!(
        command(&browser, &browser_out, &browser_in, "play")["status"],
        "conflict"
    );
    assert_eq!(
        command(&f.operator, &gui_out, &gui_in, "stop")["status"],
        "accepted"
    );
    assert_eq!(
        command(&browser, &browser_out, &browser_in, "play")["status"],
        "accepted"
    );
    assert_eq!(
        command(&f.operator, &gui_out, &gui_in, "play")["status"],
        "conflict"
    );
    assert_eq!(
        command(&browser, &browser_out, &browser_in, "stop")["status"],
        "accepted"
    );
    assert_eq!(
        command(&f.operator, &gui_out, &gui_in, "play")["status"],
        "accepted"
    );
    assert_eq!(
        command(&f.operator, &gui_out, &gui_in, "stop")["status"],
        "accepted"
    );
    assert_eq!(
        f.service.lock().unwrap().snapshot().unwrap()["peers"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn loop_permissions_precede_shared_state_changes() {
    let f = Fixture::new();
    let _pump = Pump::attach(&f);
    let (out, inbox) = f.channel();
    let paused = request(
        &f,
        &f.editor,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"setLoop","start":0,"end":48000}),
    );
    assert_eq!(paused["status"], "rejected");
    assert!(f.service.lock().unwrap().snapshot().unwrap()["transport"]["loopStart"].is_null());
    request(
        &f,
        &f.operator,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"setLoop","start":null,"end":null}),
    );
    let played = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"play","trackOnly":null}),
    );
    assert_eq!(played["status"], "accepted", "{played}");
    let refused = request(
        &f,
        &f.editor,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"setLoop","start":1000,"end":2000}),
    );
    assert_eq!(refused["status"], "rejected");
    assert!(f.service.lock().unwrap().snapshot().unwrap()["transport"]["loopStart"].is_null());
    let mut other = f.operator.clone();
    other.runtime = new_id().unwrap();
    let refused = request(
        &f,
        &other,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"setLoop","start":1000,"end":2000}),
    );
    assert_eq!(refused["status"], "conflict");
    assert!(f.service.lock().unwrap().snapshot().unwrap()["transport"]["loopStart"].is_null());
    request(
        &f,
        &f.operator,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"stop"}),
    );
}

#[test]
fn track_controls_preserve_and_clear_explicit_input_assignments() {
    let f = Fixture::new();
    let _pump = Pump::attach(&f);
    let (out, inbox) = f.channel();
    let added = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        Some(0),
        json!({}),
        json!({"type":"addTrack","name":"Input"}),
    );
    assert_eq!(added["status"], "accepted");
    let state = f.service.lock().unwrap();
    let track = state.snapshot().unwrap()["session"]["tracks"][0].clone();
    let id = track["id"].as_str().unwrap().to_string();
    let version = state.authority.track_revisions[&id];
    drop(state);
    let mut controls = track;
    let object = controls.as_object_mut().unwrap();
    object.remove("id");
    object.remove("clips");
    object.remove("recordingTakes");
    object.remove("activeTakeID");
    object.insert("trackID".into(), json!(id));
    object.insert("inputChannel".into(), json!(0));
    object.insert("stereoInput".into(), json!(false));
    object.insert(
        "inputDeviceUID".into(),
        json!("abcdef12-3456-4abc-8abc-0123456789ab"),
    );
    let routed = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        None,
        json!({id:version}),
        json!({"type":"updateTrackControls","controls":controls,"name":"Input routing","gestureID":null}),
    );
    assert_eq!(routed["status"], "accepted", "{routed}");
    let mut controls =
        f.service.lock().unwrap().snapshot().unwrap()["session"]["tracks"][0].clone();
    assert_eq!(
        controls["inputDeviceUID"],
        "abcdef12-3456-4abc-8abc-0123456789ab"
    );
    let id = controls["id"].as_str().unwrap().to_owned();
    let object = controls.as_object_mut().unwrap();
    object.remove("id");
    object.remove("clips");
    object.remove("recordingTakes");
    object.remove("inputDeviceUID");
    object.insert("trackID".into(), json!(id));
    let cleared = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        None,
        json!({id:2}),
        json!({"type":"updateTrackControls","controls":controls,"name":"Session input","gestureID":null}),
    );
    assert_eq!(cleared["status"], "accepted", "{cleared}");
    assert!(
        f.service.lock().unwrap().snapshot().unwrap()["session"]["tracks"][0]["inputDeviceUID"]
            .is_null()
    );
}

#[test]
fn stop_during_count_in_cancels_capture_startup() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let f = Fixture::new();
    let (control, mut host) = soundroom_engine::realtime::channel();
    Service::attach_audio(&f.service, control).unwrap();
    let input = f.service.lock().unwrap().simulated_input();
    let quit = Arc::new(AtomicBool::new(false));
    let quit_worker = quit.clone();
    let callback = std::thread::spawn(move || {
        let (mut left, mut right, mut frame) = ([0.; 128], [0.; 128], 0u64);
        let mut phase = 0;
        while !quit_worker.load(Ordering::Acquire) {
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
            soundroom_server::service::record::push_simulated(&input, &mut phase, host_time, 128);
            frame += 128;
            std::thread::sleep(Duration::from_micros(2667));
        }
    });
    let (out, inbox) = f.channel();
    assert_eq!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            Some(0),
            json!({}),
            json!({"type":"addTrack","name":"Record"})
        )["status"],
        "accepted"
    );
    let id = f.service.lock().unwrap().authority.session.tracks[0]
        .id
        .clone();
    assert_eq!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            None,
            json!({id.clone():1}),
            json!({"type":"setTrackArm","trackID":id,"armed":true})
        )["status"],
        "accepted"
    );
    let record_id = new_id().unwrap();
    Service::dispatch(&f.service,&f.operator,&out,frame(&f.operator,&record_id,None,json!({}),json!({"type":"editor","intent":{"operation":"transport.record","context":{"clipIDs":[],"cursorFrame":0},"integers":{"countInBars":1},"numbers":{},"strings":{},"flags":{}}}))).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while f.service.lock().unwrap().snapshot().unwrap()["transport"]["isPlaying"] != true {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    let stop_id = new_id().unwrap();
    let stop_started = Instant::now();
    Service::dispatch(&f.service, &f.operator, &out,
        frame(&f.operator, &stop_id, None, json!({}), json!({"type":"editor","intent":{"operation":"transport.pause","context":{"clipIDs":[],"cursorFrame":0},"integers":{},"numbers":{},"strings":{},"flags":{}}}))).unwrap();
    let (mut stopped, mut cancelled) = (None, None);
    while stopped.is_none() || cancelled.is_none() {
        let value = read(&inbox);
        if value["kind"] == "reply" {
            if value["requestID"] == stop_id {
                stopped = Some(value);
            } else if value["requestID"] == record_id {
                cancelled = Some(value);
            }
        }
    }
    assert_eq!(stopped.unwrap()["status"], "accepted");
    assert_ne!(cancelled.unwrap()["status"], "accepted");
    assert!(stop_started.elapsed() < Duration::from_secs(1));
    let transport = f.service.lock().unwrap().snapshot().unwrap()["transport"].clone();
    assert_eq!(transport["isRecording"], false);
    assert_eq!(transport["isPreparing"], false);
    assert_eq!(transport["isPlaying"], false);
    quit.store(true, Ordering::Release);
    callback.join().unwrap();
}

#[test]
fn overlapping_autosave_keeps_newest_synchronous_recovery() {
    let f = Fixture::new();
    {
        let mut state = f.service.lock().unwrap();
        state.authority.session.name = "Old".into();
        state
            .authority
            .session
            .extra
            .insert("reviewPayload".into(), json!("x".repeat(50 * 1024 * 1024)));
        state.authority.revision = 1;
    }
    Service::start_autosave(&f.service);
    let autosave = f.root.join("Autosave");
    let deadline = Instant::now() + Duration::from_secs(5);
    let stage = loop {
        if let Ok(entries) = std::fs::read_dir(&autosave) {
            if let Some(stage) = entries.flatten().find(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".soundroom-stage-")
            }) {
                break stage.path();
            }
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_micros(50));
    };
    {
        let mut state = f.service.lock().unwrap();
        state.authority.session.name = "New".into();
        state.authority.session.extra.clear();
        state.authority.revision = 2;
    }
    let (out, inbox) = f.channel();
    let saved = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"editor","intent":{"operation":"media.cleanup","context":{"clipIDs":[],"cursorFrame":0},"integers":{},"numbers":{},"strings":{},"flags":{}}}),
    );
    assert_eq!(saved["status"], "accepted");
    let fresh_name = std::fs::read_to_string(autosave.join("current")).unwrap();
    let fresh = autosave.join(&fresh_name);
    let metadata: Value =
        serde_json::from_slice(&std::fs::read(fresh.join("session.json")).unwrap()).unwrap();
    assert_eq!(metadata["name"], "New");
    std::thread::sleep(Duration::from_millis(150));
    let pointer = std::fs::read_to_string(autosave.join("current")).unwrap();
    let metadata: Value = serde_json::from_slice(
        &std::fs::read(autosave.join(pointer).join("session.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(metadata["name"], "New");
    assert!(fresh.exists());
    assert!(!stage.exists());
}

#[test]
fn capture_blocks_destructive_edits_and_commits_the_armed_take() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let f = Fixture::new();
    let (control, mut host) = soundroom_engine::realtime::channel();
    Service::attach_audio(&f.service, control).unwrap();
    let input = f.service.lock().unwrap().simulated_input();
    let quit = Arc::new(AtomicBool::new(false));
    let quit_worker = quit.clone();
    let callback = std::thread::spawn(move || {
        let (mut left, mut right, mut frame) = ([0.; 128], [0.; 128], 0u64);
        let mut phase = 0;
        while !quit_worker.load(Ordering::Acquire) {
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
            soundroom_server::service::record::push_simulated(&input, &mut phase, host_time, 128);
            frame += 128;
            std::thread::sleep(Duration::from_micros(2667));
        }
    });
    let (out, inbox) = f.channel();
    assert_eq!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            Some(0),
            json!({}),
            json!({"type":"addTrack","name":"Record"})
        )["status"],
        "accepted"
    );
    let id = f.service.lock().unwrap().authority.session.tracks[0]
        .id
        .clone();
    assert_eq!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            None,
            json!({id.clone():1}),
            json!({"type":"setTrackArm","trackID":id,"armed":true})
        )["status"],
        "accepted"
    );
    assert_eq!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            None,
            json!({}),
            json!({"type":"record"})
        )["status"],
        "accepted"
    );
    std::thread::sleep(Duration::from_millis(250));
    assert_eq!(
        f.service.lock().unwrap().snapshot().unwrap()["transport"]["isRecording"],
        true
    );
    let removed = request(
        &f,
        &f.editor,
        &out,
        &inbox,
        Some(2),
        json!({id.clone():2}),
        json!({"type":"removeTrack","trackID":id}),
    );
    assert_eq!(removed["status"], "conflict", "{removed}");
    let stopped = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"stop"}),
    );
    assert_eq!(stopped["status"], "accepted");
    assert_eq!(payload(&stopped)["takes"], 1);
    assert_eq!(
        f.service.lock().unwrap().authority.session.tracks[0]
            .clips
            .len(),
        1
    );
    quit.store(true, Ordering::Release);
    callback.join().unwrap();
}

#[test]
fn cleanup_preserves_audio_inside_saved_projects() {
    use soundroom_engine::{
        model::{Session, Track, MASTER_ID},
        project::{Media, Project},
        wave::{WaveReader, WaveWriter},
    };
    let f = Fixture::new();
    let input = f.root.join("source.wav");
    let mut writer = WaveWriter::create(&input, 128).unwrap();
    writer.write(&[0.2; 128], &[0.2; 128]).unwrap();
    writer.finish().unwrap();
    let asset = new_id().unwrap();
    let track_id = new_id().unwrap();
    let mut session = Session::default();
    session.tracks.push(Track { id: track_id.clone(), clips: vec![serde_json::from_value(json!({"id":new_id().unwrap(),"assetID":asset,"name":"Take","start":0,"offset":0,"length":128})).unwrap()], ..Track::default() });
    let info = WaveReader::open(&input).unwrap().info;
    let saved = f.root.join("Projects/Original.soundroom");
    Project {
        session,
        media: [(asset.clone(), Media { path: input, info })]
            .into_iter()
            .collect(),
    }
    .save_new(&saved)
    .unwrap();
    let (out, inbox) = f.channel();
    assert_eq!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            Some(0),
            json!({}),
            json!({"type":"openProject","id":"Original.soundroom"})
        )["status"],
        "accepted"
    );
    assert_eq!(
        request(
            &f,
            &f.operator,
            &out,
            &inbox,
            Some(1),
            json!({track_id.clone():1}),
            json!({"type":"removeTrack","trackID":track_id})
        )["status"],
        "accepted"
    );
    // Once authored undo no longer needs this source, cleanup treats it as unused.
    for n in 0..257 {
        let version = f.service.lock().unwrap().authority.track_revisions[MASTER_ID];
        assert_eq!(
            request(
                &f,
                &f.operator,
                &out,
                &inbox,
                None,
                json!({MASTER_ID:version}),
                json!({"type":"setTrackGain","trackID":MASTER_ID,"gain":if n%2==0 {0.5} else {0.6}})
            )["status"],
            "accepted"
        );
    }
    assert!(!f
        .service
        .lock()
        .unwrap()
        .authority
        .referenced_asset_ids()
        .contains(&asset));
    assert!(saved.join(format!("Audio/{asset}.wav")).exists());
    let cleanup = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"editor","intent":{"operation":"media.cleanup","context":{"clipIDs":[],"cursorFrame":0},"integers":{},"numbers":{},"strings":{},"flags":{}}}),
    );
    assert_eq!(cleanup["status"], "accepted");
    assert_eq!(payload(&cleanup)["released"], 1);
    assert!(saved.join(format!("Audio/{asset}.wav")).exists());
    assert!(Project::open(&saved).is_ok());
}

#[test]
fn shutdown_closes_admission_after_the_final_recovery_save() {
    use std::sync::atomic::AtomicBool;
    let f = Fixture::new();
    f.service
        .lock()
        .unwrap()
        .set_shutdown_signal(Arc::new(AtomicBool::new(false)));
    let (out, inbox) = f.channel();
    let stopped = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"editor","intent":{"operation":"server.shutdown","context":{"clipIDs":[],"cursorFrame":0},"integers":{},"numbers":{},"strings":{},"flags":{}}}),
    );
    assert_eq!(stopped["status"], "accepted");
    let late = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        Some(0),
        json!({}),
        json!({"type":"addTrack","name":"Too late"}),
    );
    assert_eq!(late["status"], "rejected");
    assert_eq!(f.service.lock().unwrap().authority.revision, 0);
    let name = std::fs::read_to_string(f.root.join("Autosave/current")).unwrap();
    assert!(
        soundroom_engine::project::Project::open(&f.root.join("Autosave").join(name))
            .unwrap()
            .session
            .tracks
            .is_empty()
    );
}

#[test]
fn live_open_normalizes_project_ids_and_preserves_device_identity() {
    use soundroom_engine::{
        model::{Session, Track},
        project::{Media, Project},
        wave::{WaveReader, WaveWriter},
    };
    let f = Fixture::new();
    let input = f.root.join("source.wav");
    let mut writer = WaveWriter::create(&input, 128).unwrap();
    writer.write(&[0.2; 128], &[0.2; 128]).unwrap();
    writer.finish().unwrap();
    let info = WaveReader::open(&input).unwrap().info;
    let track_id = new_id().unwrap().to_lowercase();
    let clip_id = new_id().unwrap().to_lowercase();
    let history_id = new_id().unwrap().to_lowercase();
    let take_id = new_id().unwrap().to_lowercase();
    let take_clip_id = new_id().unwrap().to_lowercase();
    let asset_id = new_id().unwrap().to_lowercase();
    // A device UID that happens to resemble a UUID is still case-sensitive.
    let device_uid = "abcdefab-cdef-abcd-efab-cdefabcdefab";
    let mut session = Session::default();
    session.tracks.push(Track {
        id: track_id.clone(),
        clips: vec![serde_json::from_value(json!({
            "id":clip_id,
            "assetID":asset_id,
            "name":"External clip",
            "start":0,
            "offset":0,
            "length":128,
            "previousStates":[{
                "id":history_id,"operation":"Imported","createdAt":0,
                "assetID":asset_id,"name":"Original","offset":0,"length":128
            }]
        }))
        .unwrap()],
        recording_takes: vec![serde_json::from_value(
            json!({"id":take_id,"name":"Take", "clips":[{
                "id":take_clip_id,
                "assetID":asset_id,
                "name":"External take",
                "start":0,
                "offset":0,
                "length":128
            }]}),
        )
        .unwrap()],
        extra: [
            ("inputDeviceUID".into(), json!(device_uid)),
            ("activeTakeID".into(), json!(take_id)),
        ]
        .into_iter()
        .collect(),
        ..Track::default()
    });
    let saved = f.root.join("Projects/Lowercase.soundroom");
    Project {
        session,
        media: [(asset_id.to_uppercase(), Media { path: input, info })]
            .into_iter()
            .collect(),
    }
    .save_new(&saved)
    .unwrap();
    let (out, inbox) = f.channel();
    let opened = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        Some(0),
        json!({}),
        json!({"type":"openProject","id":"Lowercase.soundroom"}),
    );
    assert_eq!(opened["status"], "accepted", "{opened}");
    let track_id = track_id.to_uppercase();
    let clip_id = clip_id.to_uppercase();
    let asset_id = asset_id.to_uppercase();
    {
        let state = f.service.lock().unwrap();
        let track = &state.authority.session.tracks[0];
        assert_eq!(track.id, track_id);
        assert_eq!(track.clips[0].id, clip_id);
        assert_eq!(track.clips[0].asset_id, asset_id);
        assert_eq!(track.clips[0].previous_states[0].asset_id, asset_id);
        assert_eq!(
            track.clips[0].previous_states[0].extra["id"],
            history_id.to_uppercase()
        );
        assert_eq!(
            track.recording_takes[0].clips[0].id,
            take_clip_id.to_uppercase()
        );
        assert_eq!(track.recording_takes[0].clips[0].asset_id, asset_id);
        assert_eq!(track.recording_takes[0].extra["id"], take_id.to_uppercase());
        assert_eq!(track.extra["activeTakeID"], take_id.to_uppercase());
        assert_eq!(track.extra["inputDeviceUID"], device_uid);
        assert_eq!(state.authority.track_revisions[&track_id], 1);
    }
    // These are the uppercase resource identities and revision fences the GUI uses.
    let gain = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        None,
        json!({track_id.clone():1}),
        json!({"type":"setTrackGain","trackID":track_id,"gain":0.5}),
    );
    assert_eq!(gain["status"], "accepted", "{gain}");
    let moved = request(
        &f,
        &f.operator,
        &out,
        &inbox,
        None,
        json!({track_id.clone():2}),
        json!({"type":"moveClip","trackID":track_id,"clipID":clip_id,"start":64}),
    );
    assert_eq!(moved["status"], "accepted", "{moved}");
    let waveform = request(
        &f,
        &f.viewer,
        &out,
        &inbox,
        None,
        json!({}),
        json!({"type":"waveform","assetID":asset_id,"start":0,"end":128,"points":8}),
    );
    assert_eq!(waveform["status"], "accepted", "{waveform}");
    let measured = payload(&waveform);
    assert_eq!(measured["channelPeaks"].as_array().unwrap().len(), 2);
    for channel in measured["channelPeaks"].as_array().unwrap() {
        assert_eq!(
            channel.as_array().unwrap().len(),
            measured["peaks"].as_array().unwrap().len()
        );
    }
    let state = f.service.lock().unwrap();
    assert_eq!(state.authority.session.tracks[0].gain, 0.5);
    assert_eq!(state.authority.session.tracks[0].clips[0].start, 64);
}
