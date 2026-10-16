use super::*;
use crate::{
    registry::{private_directory, Registry},
    service::{Handled, Identity, JobKind, Outbox, Service, Shared},
};
use soundroom_client::{auth::new_id, models::RemoteRole};
use soundroom_engine::{
    authority::Role,
    model::{Clip, Session, Track},
    project::{Media, Project},
    wave::WaveWriter,
};
use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt, sync::Arc};

struct Fixture {
    root: PathBuf,
    runtime: PathBuf,
    service: Shared,
    editor: Identity,
    viewer: Identity,
    track: String,
    asset: String,
}
impl Fixture {
    fn new(script: &str) -> Self {
        let root = std::env::temp_dir().join(format!("soundroom-separation-{}", new_id().unwrap()));
        private_directory(&root).unwrap();
        let runtime = root.join("SeparationRuntime");
        fs::create_dir_all(runtime.join(".venv/bin")).unwrap();
        fs::create_dir_all(runtime.join("models")).unwrap();
        fs::write(
            runtime.join("worker.py"),
            "# Fixture: not a learned model.\n",
        )
        .unwrap();
        let python = runtime.join(".venv/bin/python");
        fs::write(&python, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&python, fs::Permissions::from_mode(0o700)).unwrap();
        let mut files = serde_json::Map::new();
        for target in &SOURCES[..4] {
            let name = weights("quality", target);
            let path = runtime.join("models").join(name);
            fs::write(&path, "fixture").unwrap();
            let stamp = path.metadata().unwrap();
            files.insert(name.into(), json!({"bytes":stamp.len(),
                "mtime_ns":stamp.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64,
                "sha256":format!("{}{}", &name[9..17], "0".repeat(56)),
                "url":format!("https://dl.fbaipublicfiles.com/demucs/hybrid_transformer/{name}")}));
        }
        fs::write(
            runtime.join("models/manifest.json"),
            serde_json::to_vec(&json!({"schema":1,
            "provider":"https://github.com/facebookresearch/demucs","files":files}))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            runtime.join("ready.json"),
            serde_json::to_vec(&json!({"schema":1,"runtimeReady":true,
            "sampleRate":48000,"models":{"quality":{"available":true,"targets":SOURCES[..4]}}}))
            .unwrap(),
        )
        .unwrap();
        let mut registry = Registry::create(&root, "Source separation tests").unwrap();
        let mut identity = |name: &str, role: RemoteRole| {
            let record = registry.issue(name, role.clone()).unwrap();
            Identity {
                runtime: new_id().unwrap(),
                credential: record["credentialID"].as_str().unwrap().into(),
                name: name.into(),
                role,
            }
        };
        let editor = identity("Editor", RemoteRole::Editor);
        let viewer = identity("Viewer", RemoteRole::Viewer);
        let track = new_id().unwrap();
        let asset = new_id().unwrap();
        let audio = root.join("source.wav");
        let mut writer = WaveWriter::create(&audio, 8192).unwrap();
        let samples: Vec<f32> = (0..8192).map(|i| (i % 512) as f32 / 512. * 0.1).collect();
        writer.write(&samples, &samples).unwrap();
        writer.finish().unwrap();
        let clip: Clip = serde_json::from_value(json!({"id":new_id().unwrap(),"assetID":asset,
            "name":"Original","start":0,"offset":0,"length":8192,"edgeFadeFrames":0,
            "edgeProtectionDisabled":true,"previousStates":[]}))
        .unwrap();
        let session = Session {
            tracks: vec![Track {
                id: track.clone(),
                name: "Source".into(),
                clips: vec![clip],
                ..Track::default()
            }],
            ..Session::default()
        };
        let project = Project {
            session,
            media: BTreeMap::from([(
                asset.clone(),
                Media {
                    path: audio.clone(),
                    info: WaveReader::open(&audio).unwrap().info,
                },
            )]),
        };
        let package = root.join("Original.soundroom");
        project.save_new(&package).unwrap();
        let service = Service::open(&root, Some(&package)).unwrap();
        Self {
            root,
            runtime,
            service,
            editor,
            viewer,
            track,
            asset,
        }
    }
    fn intent(&self) -> RemoteEditorIntent {
        serde_json::from_value(json!({"operation":"effects.apply","context":{"trackID":self.track,
            "clipIDs":[],"cursorFrame":0,"rangeStart":0,"rangeEnd":8192},"integers":{},"numbers":{},
            "strings":{"kind":"sourceIsolator","separationModel":"quality","separationTarget":"vocals","separationOperation":"extract"},
            "flags":{"separateSource":true}})).unwrap()
    }
    fn task(&self) -> Box<super::super::process::Task> {
        match self
            .service
            .lock()
            .unwrap()
            .begin_effect(&self.intent())
            .unwrap()
        {
            Handled::Job(JobKind::Process { task }) => task,
            _ => panic!("Expected a processing job"),
        }
    }
    fn request(&self, peer: &Identity, intent: RemoteEditorIntent) -> Value {
        self.request_with_tracks(peer, intent, json!({self.track.clone():0}))
    }
    fn request_with_tracks(
        &self,
        peer: &Identity,
        intent: RemoteEditorIntent,
        tracks: Value,
    ) -> Value {
        let id = new_id().unwrap();
        let (out, inbox) = Outbox::pair(self.service.lock().unwrap().global());
        Service::dispatch(
            &self.service,
            peer,
            &out,
            json!({"version":3,"kind":"request","id":id,
            "clientID":peer.runtime,"expectedRevision":0,"expectedTrackRevisions":tracks,
            "command":{"type":"editor","intent":intent}}),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(bytes) = inbox
                .priority
                .try_recv()
                .ok()
                .or_else(|| inbox.ordinary.try_recv().ok())
            {
                inbox.received(bytes.len());
                let value: Value = serde_json::from_slice(&bytes).unwrap();
                if value["kind"] == "reply" && value["requestID"] == id {
                    return value;
                }
            }
            assert!(Instant::now() < deadline, "Request reply timed out");
            thread::sleep(Duration::from_millis(2));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
const COPY: &str = r#"
while [ "$#" -gt 0 ]; do
  case "$1" in
    --input) input="$2"; shift 2 ;;
    --output) output="$2"; shift 2 ;;
    *) shift ;;
  esac
done
cp "$input" "$output"
printf '%s\n' '{"type":"progress","progress":0.5}' '{"type":"complete"}'
"#;

#[test]
fn sound_putty_dense_sculpt_renders_and_commits_one_undoable_selection() {
    use base64::Engine;
    let fixture = Fixture::new(COPY);
    let mut intent = fixture.intent();
    intent.flags.clear(); intent.strings.clear();
    intent.strings.insert("kind".into(), "soundPutty".into());
    let mut parameters = soundroom_engine::render::default_parameters("soundPutty");
    for axis in ["puttyPitch", "puttyEnergy", "puttyDamping"] {
        for lane in 0..300 { parameters.insert(format!("{axis}{lane:03}"), (lane as f32 * 0.03).sin() * 0.5); }
    }
    assert_eq!(parameters.len(), 916);
    intent.payload = Some(base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&parameters).unwrap()));
    let task = match fixture.service.lock().unwrap().begin_effect(&intent).unwrap() {
        Handled::Job(JobKind::Process { task }) => task,
        _ => panic!("Expected Sound Putty processing job"),
    };
    let processed = task.run(&AtomicBool::new(false), |_, _| {}).unwrap();
    let mut state = fixture.service.lock().unwrap();
    state.commit_processed(&fixture.editor, &processed).unwrap();
    let clip = &state.authority.session.tracks[0].clips[0];
    assert_eq!(clip.length, 8192); assert_ne!(clip.asset_id, fixture.asset);
    assert_eq!(clip.previous_states[0].asset_id, fixture.asset);
    state.authority.undo(&fixture.editor.runtime, Role::Editor).unwrap();
    assert_eq!(state.authority.session.tracks[0].clips[0].asset_id, fixture.asset);
}

#[test]
fn readiness_and_requests_fail_closed_before_rendering() {
    let fixture = Fixture::new(COPY);
    assert_eq!(status(&fixture.root)["available"], true);
    let mut intent = fixture.intent();
    intent
        .strings
        .insert("separationTarget".into(), "../arbitrary".into());
    assert_eq!(
        fixture.request(&fixture.editor, intent)["status"],
        "rejected"
    );
    assert_eq!(
        fixture.request(&fixture.viewer, fixture.intent())["status"],
        "rejected"
    );
    let mut intent = fixture.intent();
    intent
        .strings
        .insert("separationTarget".into(), "piano".into());
    assert!(plan(&fixture.root, &intent)
        .unwrap_err()
        .to_string()
        .contains("extended"));
    fs::remove_file(
        fixture
            .runtime
            .join("models")
            .join(weights("quality", "vocals")),
    )
    .unwrap();
    // The installed application runtime may legitimately be a fallback; the
    // damaged fixture itself must no longer qualify as a ready runtime.
    assert!(load(fixture.runtime.clone()).is_none());
    assert!(fs::read_dir(fixture.root.join("Artifacts"))
        .unwrap()
        .next()
        .is_none());
}

#[test]
fn worker_copy_commits_one_undoable_clip_and_keeps_the_original() {
    let fixture = Fixture::new(COPY);
    let processed = fixture
        .task()
        .run(&AtomicBool::new(false), |_, _| {})
        .unwrap();
    let result_path = processed.path().to_owned();
    let mut state = fixture.service.lock().unwrap();
    state.commit_processed(&fixture.editor, &processed).unwrap();
    let result_asset = state.authority.session.tracks[0].clips[0].asset_id.clone();
    assert_ne!(result_asset, fixture.asset);
    assert_eq!(
        state.authority.session.tracks[0].clips[0].previous_states[0].asset_id,
        fixture.asset
    );
    assert_eq!(state.authority.session.tracks[0].clips[0].length, 8192);
    drop(processed);
    assert!(!result_path.exists());
    assert!(state.media[&result_asset].path.exists());
    state
        .authority
        .undo(&fixture.editor.runtime, Role::Editor)
        .unwrap();
    assert_eq!(
        state.authority.session.tracks[0].clips[0].asset_id,
        fixture.asset
    );
    state
        .authority
        .redo(&fixture.editor.runtime, Role::Editor)
        .unwrap();
    assert_eq!(
        state.authority.session.tracks[0].clips[0].asset_id,
        result_asset
    );
}

#[test]
fn stale_credentials_tracks_and_recording_cannot_commit_a_result() {
    let fixture = Fixture::new(COPY);
    let processed = fixture
        .task()
        .run(&AtomicBool::new(false), |_, _| {})
        .unwrap();
    let mut state = fixture.service.lock().unwrap();
    state.recording_finalizing = true;
    assert!(state.commit_processed(&fixture.editor, &processed).is_err());
    assert!(state.begin_effect(&fixture.intent()).is_err());
    state.recording_finalizing = false;
    let mut changed = state.authority.session.clone();
    changed.tracks[0].name = "Changed while rendering".into();
    let revision = state.authority.revision;
    let tracks = state.authority.track_revisions.clone();
    state
        .authority
        .edit_session(
            &fixture.editor.runtime,
            Role::Editor,
            Some(revision),
            &tracks,
            changed,
        )
        .unwrap();
    assert!(state
        .commit_processed(&fixture.editor, &processed)
        .unwrap_err()
        .to_string()
        .contains("track changed"));
    assert_eq!(state.media.len(), 1);
    state.registry.revoke(&fixture.editor.credential).unwrap();
    assert!(state
        .commit_processed(&fixture.editor, &processed)
        .unwrap_err()
        .to_string()
        .contains("credential"));
    assert_eq!(state.media.len(), 1);
}

#[test]
fn stale_or_missing_source_revision_rejects_separation_before_job_admission() {
    let fixture = Fixture::new(COPY);
    for operation in ["effects.apply", "effects.preview"] {
        let mut intent = fixture.intent();
        intent.operation = operation.into();
        for expected in [json!({}), json!({fixture.track.clone():1})] {
            let reply = fixture.request_with_tracks(&fixture.editor, intent.clone(), expected);
            assert_eq!(reply["status"], "conflict", "{reply}");
            assert!(reply["message"]
                .as_str()
                .unwrap()
                .contains("source changed"));
        }
    }
    let state = fixture.service.lock().unwrap();
    assert!(state.pending.is_empty());
    assert_eq!(state.authority.revision, 0);
    assert!(fs::read_dir(fixture.root.join("Artifacts"))
        .unwrap()
        .next()
        .is_none());
}

#[test]
fn prepared_separation_preview_rechecks_cancel_recording_source_and_credential() {
    let fixture = Fixture::new(COPY);
    let processed = fixture
        .task()
        .run(&AtomicBool::new(false), |_, _| {})
        .unwrap();
    let cancel = AtomicBool::new(false);
    let mut state = fixture.service.lock().unwrap();
    assert!(processed
        .preview_allowed(&state, &fixture.editor, &cancel)
        .is_ok());
    cancel.store(true, Ordering::Release);
    assert!(processed
        .preview_allowed(&state, &fixture.editor, &cancel)
        .unwrap_err()
        .to_string()
        .contains("cancelled"));
    cancel.store(false, Ordering::Release);
    state.recording_preparing = Some(Arc::new(AtomicBool::new(false)));
    assert!(processed
        .preview_allowed(&state, &fixture.editor, &cancel)
        .is_err());
    state.recording_preparing = None;
    state.recording_finalizing = true;
    assert!(processed
        .preview_allowed(&state, &fixture.editor, &cancel)
        .is_err());
    state.recording_finalizing = false;
    let mut changed = state.authority.session.clone();
    changed.tracks[0].name = "Edited during preview loading".into();
    let revision = state.authority.revision;
    let tracks = state.authority.track_revisions.clone();
    state
        .authority
        .edit_session(
            &fixture.editor.runtime,
            Role::Editor,
            Some(revision),
            &tracks,
            changed,
        )
        .unwrap();
    assert!(processed
        .preview_allowed(&state, &fixture.editor, &cancel)
        .unwrap_err()
        .to_string()
        .contains("track changed"));
    state.registry.revoke(&fixture.editor.credential).unwrap();
    assert!(processed
        .preview_allowed(&state, &fixture.editor, &cancel)
        .unwrap_err()
        .to_string()
        .contains("credential"));
    assert_eq!(state.media.len(), 1);
}

#[test]
fn stopped_or_superseded_separation_cannot_reclaim_the_same_preview_lease() {
    let fixture = Fixture::new(COPY);
    let (control, _host) = soundroom_engine::realtime::channel();
    Service::attach_audio(&fixture.service, control).unwrap();
    let start = || {
        let mut intent = fixture.intent();
        intent.operation = "effects.preview".into();
        let request = serde_json::from_value(json!({"id":new_id().unwrap(),
            "clientID":fixture.editor.runtime,"expectedRevision":0,"expectedTrackRevisions":{fixture.track.clone():0},
            "command":{"type":"editor","intent":intent}})).unwrap();
        match fixture
            .service
            .lock()
            .unwrap()
            .editor(&fixture.editor, &request, &intent)
            .unwrap()
        {
            Handled::Job(JobKind::Process { task }) => task,
            _ => panic!("Expected an Effects preview job"),
        }
    };
    let task = start();
    let sequence = task.preview.as_ref().unwrap().1;
    let preview_cancel = task.preview.as_ref().unwrap().2.clone();
    let processed = task.run(&preview_cancel, |_, _| {}).unwrap();
    let kind = super::super::preview::Kind::Effects;
    {
        let mut state = fixture.service.lock().unwrap();
        assert!(processed
            .preview_allowed(&state, &fixture.editor, &AtomicBool::new(false))
            .is_ok());
        assert!(state.file_preview_current(&fixture.editor.runtime, kind, sequence));
        state.stop_previews();
        assert!(preview_cancel.load(Ordering::Acquire));
        assert!(processed
            .preview_allowed(&state, &fixture.editor, &preview_cancel)
            .is_err());
        assert!(!state.file_preview_current(&fixture.editor.runtime, kind, sequence));
    }
    let later = start();
    let later_sequence = later.preview.as_ref().unwrap().1;
    let later_cancel = later.preview.as_ref().unwrap().2.clone();
    assert!(!later_cancel.load(Ordering::Acquire));
    let latest = start();
    let latest_sequence = latest.preview.as_ref().unwrap().1;
    assert!(later_cancel.load(Ordering::Acquire));
    let state = fixture.service.lock().unwrap();
    assert!(later_sequence > sequence);
    assert!(!state.file_preview_current(&fixture.editor.runtime, kind, sequence));
    assert!(!state.file_preview_current(&fixture.editor.runtime, kind, later_sequence));
    assert!(state.file_preview_current(&fixture.editor.runtime, kind, latest_sequence));
}

#[test]
fn cancelled_separation_file_preview_does_not_load_or_keep_audio() {
    let fixture = Fixture::new(COPY);
    let file = fixture.root.join("preview.wav");
    fs::copy(fixture.root.join("source.wav"), &file).unwrap();
    let task = super::super::preview::Task {
        source: super::super::preview::Source::File(file.clone()),
        kind: super::super::preview::Kind::Effects,
        owner: fixture.editor.runtime.clone(),
        path: PathBuf::new(),
        looping: false,
        tempo: 120.,
        sequence: 0,
        superseded: Arc::new(AtomicBool::new(false)),
    };
    let result = task.render(&AtomicBool::new(true));
    assert!(matches!(result, Err(error) if error.to_string().contains("cancelled")));
    assert!(!file.exists());
    assert!(fixture.service.lock().unwrap().media[&fixture.asset]
        .path
        .exists());
}

#[test]
fn cancellation_reaps_the_worker_and_cleans_partial_scratch() {
    let fixture = Fixture::new(
        r#"
while [ "$#" -gt 0 ]; do
  case "$1" in --output) output="$2"; shift 2 ;; *) shift ;; esac
done
printf '%s' partial > "${output%/*}/.${output##*/}.separation.tmp"
printf '%s' "$$" > pid.txt
sleep 30
"#,
    );
    let plan = plan(&fixture.root, &fixture.intent()).unwrap();
    let output = fixture.root.join("cancelled.wav");
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let marker = fixture.runtime.join("pid.txt");
    let marker_thread = marker.clone();
    let sender = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !marker_thread.exists() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        flag.store(true, Ordering::Release);
    });
    let started = Instant::now();
    let result = run_timed(
        &plan,
        &fixture.root.join("source.wav"),
        &output,
        8192,
        &cancel,
        |_, _| {},
        Duration::from_secs(5),
    );
    sender.join().unwrap();
    assert!(result.unwrap_err().to_string().contains("cancelled"));
    assert!(started.elapsed() < Duration::from_secs(4));
    assert!(!output
        .with_file_name(".cancelled.wav.separation.tmp")
        .exists());
    let pid: i32 = fs::read_to_string(marker).unwrap().parse().unwrap();
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "Worker was not reaped");
}

#[test]
fn timeout_and_large_failure_logs_cannot_hang_the_control_worker() {
    let fixture = Fixture::new("sleep 30");
    let plan = plan(&fixture.root, &fixture.intent()).unwrap();
    let result = run_timed(
        &plan,
        &fixture.root.join("source.wav"),
        &fixture.root.join("out.wav"),
        8192,
        &AtomicBool::new(false),
        |_, _| {},
        Duration::from_millis(100),
    );
    assert!(result.unwrap_err().to_string().contains("timed out"));
    fs::write(fixture.runtime.join(".venv/bin/python"), "#!/bin/sh\nhead -c 200000 /dev/zero >&2\nprintf '%s\\n' '{\"type\":\"error\",\"message\":\"Fixture model failure\"}'\nexit 1\n").unwrap();
    let result = run_timed(
        &plan,
        &fixture.root.join("source.wav"),
        &fixture.root.join("out.wav"),
        8192,
        &AtomicBool::new(false),
        |_, _| {},
        Duration::from_secs(3),
    );
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Fixture model failure"));
}

#[test]
fn frame_count_format_and_nonfinite_output_are_rejected() {
    let fixture = Fixture::new(COPY);
    let source = fixture.root.join("source.wav");
    assert!(validate_output(&source, 8191, &AtomicBool::new(false)).is_err());
    let info = WaveReader::open(&source).unwrap().info;
    use std::io::Write;
    let mut file = fs::OpenOptions::new().write(true).open(&source).unwrap();
    file.seek(SeekFrom::Start(info.data_offset)).unwrap();
    file.write_all(&f32::NAN.to_le_bytes()).unwrap();
    assert!(validate_output(&source, 8192, &AtomicBool::new(false))
        .unwrap_err()
        .to_string()
        .contains("invalid audio"));
}
