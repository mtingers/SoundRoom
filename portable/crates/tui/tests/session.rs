use serde_json::json;
use soundroom_client::models::*;
use soundroom_tui::{
    app::App,
    input::Key,
    worker::{Shared, Work},
};
use std::{collections::VecDeque, sync::Arc};
fn snapshot() -> RemoteSnapshot {
    serde_json::from_value(json!({"serverID":"00000000-0000-0000-0000-000000000001","serverName":"Host","revision":8,"trackRevisions":{"00000000-0000-0000-0000-000000000002":7,"00000000-0000-0000-0000-000000000003":6},"session":{"version":9,"name":"Song","timing":{"tempo":123,"beatsPerBar":3,"beatUnit":4},"tracks":[{"id":"00000000-0000-0000-0000-000000000002","name":"Voice","gain":1,"pan":0,"muted":false,"clips":[{"id":"00000000-0000-0000-0000-000000000004","assetID":"00000000-0000-0000-0000-000000000005","name":"Verse","start":0,"offset":0,"length":480000}]},{"id":"00000000-0000-0000-0000-000000000003","name":"Bass","clips":[]}]},"transport":{"frame":240000,"isPlaying":false,"isRecording":false,"isPreparing":false,"hostTime":0},"devices":[{"id":1,"uid":"HOST_MIC","name":"Host mic","inputChannels":1,"outputChannels":0}],"inputDeviceID":1,"outputDeviceID":2,"assets":[],"status":"Ready","projectName":"Song","peers":[]})).unwrap()
}
fn shared(role: RemoteRole) -> Shared {
    Shared {
        snapshot: Some(Arc::new(snapshot())),
        connected: true,
        role: Some(role),
        ..Default::default()
    }
}
#[test]
fn authored_controls_have_both_revision_guards_and_wait_for_metadata() {
    let mut app = App::new();
    app.install(shared(RemoteRole::Editor));
    let work = app.key(Key::Char('m'));
    assert_eq!(work.len(), 1);
    match &work[0] {
        Work::Command {
            command,
            revision,
            tracks,
            authored,
        } => {
            assert!(matches!(
                command.as_ref(),
                RemoteCommand::SetTrackMute { muted: true, .. }
            ));
            assert_eq!(*revision, Some(8));
            assert_eq!(tracks.values().copied().collect::<Vec<_>>(), vec![7]);
            assert!(*authored);
        }
        _ => panic!(),
    };
    assert!(app.key(Key::Char('s')).is_empty());
    let mut state = shared(RemoteRole::Editor);
    state.mutation_receipts = VecDeque::from([RemoteReply {
        request_id: "00000000-0000-0000-0000-000000000006".into(),
        status: RemoteReplyStatus::Accepted,
        revision: 9,
        message: "accepted".into(),
        payload: None,
    }]);
    app.install(state);
    assert!(app.key(Key::Char('s')).is_empty());
    let mut state = shared(RemoteRole::Editor);
    Arc::make_mut(state.snapshot.as_mut().unwrap()).revision = 9;
    app.install(state);
    assert_eq!(app.key(Key::Char('s')).len(), 1);
}
#[test]
fn private_track_identity_cursor_and_range_survive_server_reordering() {
    let mut app = App::new();
    app.install(shared(RemoteRole::Editor));
    app.key(Key::Char('j'));
    app.key(Key::Char('l'));
    app.key(Key::Char('v'));
    let id = app.view.selected_track.clone();
    let cursor = app.view.cursor_frame;
    let range = app.range;
    let mut next = shared(RemoteRole::Editor);
    Arc::make_mut(next.snapshot.as_mut().unwrap())
        .session
        .tracks
        .as_mut()
        .unwrap()
        .reverse();
    app.install(next);
    assert_eq!(app.row, 0);
    assert_eq!(app.view.selected_track, id);
    assert_eq!(app.view.cursor_frame, cursor);
    assert_eq!(app.range, range);
}
#[test]
fn viewer_can_refresh_and_list_but_cannot_mutate_or_play() {
    let mut app = App::new();
    app.install(shared(RemoteRole::Viewer));
    assert_eq!(app.parse_command("refresh").unwrap().len(), 1);
    assert_eq!(app.parse_command("projects").unwrap().len(), 1);
    assert!(app.parse_command("gain -6").unwrap().is_empty());
    assert!(app.key(Key::Char(' ')).is_empty());
}
#[test]
fn routing_uses_host_uid_and_rejects_stereo_on_mono_device() {
    let mut app = App::new();
    app.install(shared(RemoteRole::Editor));
    assert!(app.parse_command("route HOST_MIC 0 stereo").is_err());
    assert!(app.parse_command("route LOCAL_MIC 0").is_err());
    assert_eq!(app.parse_command("route HOST_MIC 0").unwrap().len(), 1);
}
#[test]
fn space_pause_does_not_seek_or_rewrite_private_cursor() {
    let mut app = App::new();
    let mut state = shared(RemoteRole::Operator);
    let mut transport = state.snapshot.as_ref().unwrap().transport.clone();
    transport.is_playing = true;
    state.transport = Some((transport, std::time::Instant::now()));
    app.install(state);
    app.view.cursor_frame = 1200;
    let work = app.key(Key::Char(' '));
    match &work[0] {
        Work::Command { command, .. } => match command.as_ref() {
            RemoteCommand::Editor { intent } => assert_eq!(intent.operation, "transport.pause"),
            _ => panic!(),
        },
        _ => panic!(),
    };
    assert_eq!(app.view.cursor_frame, 1200);
}
#[test]
fn beat_boundaries_do_not_accumulate_123_bpm_rounding_error() {
    let mut app = App::new();
    app.install(shared(RemoteRole::Editor));
    for _ in 0..4000 {
        app.key(Key::Char('l'));
    }
    assert_eq!(
        app.view.cursor_frame,
        (48000_f64 * 60. / 123. * 1000.).round() as i64
    );
    assert_eq!(
        app.time((48000_f64 * 60. / 123. * 3.).round() as i64),
        "2.1.000"
    );
}
#[test]
fn vim_counts_edges_gg_and_g_stay_local() {
    let mut app = App::new();
    app.install(shared(RemoteRole::Editor));
    app.key(Key::Char('8'));
    assert!(app.key(Key::Char('l')).is_empty());
    assert_eq!(
        app.view.cursor_frame,
        (48000_f64 * 60. / 123. * 2.).round() as i64
    );
    app.key(Key::Char('w'));
    assert_eq!(app.view.cursor_frame, 480000);
    app.key(Key::Char('b'));
    assert_eq!(app.view.cursor_frame, 0);
    app.key(Key::Char('G'));
    assert_eq!(app.view.cursor_frame, 480000);
    assert!(app.key(Key::Char('g')).is_empty());
    assert!(app.key(Key::Char('g')).is_empty());
    assert_eq!(app.view.cursor_frame, 0);
}
#[test]
fn y_p_and_dd_match_vim_and_guard_both_clipboard_tracks() {
    let mut app = App::new();
    app.install(shared(RemoteRole::Editor));
    assert!(app.key(Key::Char('y')).is_empty());
    app.key(Key::Char('j'));
    app.view.cursor_frame = 24000;
    let work = app.key(Key::Char('p'));
    match &work[0] {
        Work::Command {
            command, tracks, ..
        } => {
            assert_eq!(tracks.len(), 2);
            match command.as_ref() {
                RemoteCommand::EditTracks { edit } => {
                    let clip = &edit.tracks[0].clips.as_ref().unwrap()[0];
                    assert_eq!(clip.start, 24000);
                    assert_ne!(clip.id, "00000000-0000-0000-0000-000000000004");
                }
                _ => panic!(),
            }
        }
        _ => panic!(),
    };
    app.cancel_pending();
    app.key(Key::Char('k'));
    assert!(app.key(Key::Char('d')).is_empty());
    let deletion = app.key(Key::Char('d'));
    assert!(
        matches!(&deletion[0],Work::Command{command,..} if matches!(command.as_ref(),RemoteCommand::DeleteClips{..}))
    );
}
#[test]
fn clipboard_references_do_not_restore_removed_audio() {
    let mut app = App::new();
    app.install(shared(RemoteRole::Editor));
    app.key(Key::Char('y'));
    let mut next = shared(RemoteRole::Editor);
    Arc::make_mut(next.snapshot.as_mut().unwrap())
        .session
        .tracks
        .as_mut()
        .unwrap()[0]
        .clips = Some(vec![]);
    app.install(next);
    assert!(app.parse_command("paste").is_err());
}
fn creative_capabilities() -> RemoteCapabilities {
    serde_json::from_value(json!({"protocolVersions":[3],"projectVersions":[9],"commands":["editor"],"editorOperations":["audition.stop","designer.preview","designer.stop","designer.allNotesOff","designer.commit","transport.play","pitch.apply"],"sampleRate":48000,"deviceBackends":[],"codecs":["wav"],"snapshotPagination":true,"maxFrameBytes":8388608,"maxUploadChunkBytes":262144,"maxUploadBytes":2147483648_i64,"maxTimelineFrame":17280000000_i64,"features":["designer"],"effects":[],"selectionEffects":[],"instruments":[],"designerPresets":[{"id":"factory.Studio piano","name":"Studio piano","category":"Keys","sound":{"instrument":"studioPiano","beats":4,"notes":[]}}],"drumKits":[],"drumPresets":[]})).unwrap()
}
#[test]
fn creative_focus_space_isolated_and_close_releases_preview() {
    let mut app = App::new();
    let mut state = shared(RemoteRole::Operator);
    state.capabilities = Some(Arc::new(creative_capabilities()));
    app.install(state);
    app.parse_command("designer").unwrap();
    let requests = app.key(Key::Char(' '));
    assert!(
        matches!(&requests[0],Work::Command{command,..} if matches!(command.as_ref(),RemoteCommand::Editor{intent} if intent.operation=="designer.preview"))
    );
    let requests = app.key(Key::Char('t'));
    assert!(requests.iter().all(|r|matches!(r,Work::Command{command,..} if matches!(command.as_ref(),RemoteCommand::Editor{intent} if !intent.operation.starts_with("transport.")))));
    assert!(app.creative.is_none());
}
#[test]
fn creative_preview_failures_are_correlated_and_retry_does_not_stop_the_main_mix() {
    let mut app = App::new();
    let mut state = shared(RemoteRole::Operator);
    state.capabilities = Some(Arc::new(creative_capabilities()));
    app.install(state);
    app.parse_command("designer").unwrap();
    let first = app.key(Key::Char(' '));
    let failure = first[0].failure(false, "Not sent: queue full".into());
    assert!(failure.tag.is_some());
    assert!(!failure.authored);
    app.work_failed(&failure);
    assert!(!app.creative.as_ref().unwrap().playing);
    let retry = app.key(Key::Char(' '));
    assert!(
        matches!(&retry[0],Work::Command{command,..} if matches!(command.as_ref(),RemoteCommand::Editor{intent} if intent.operation=="designer.preview"))
    );
    app.work_failed(&failure);
    assert!(app.creative.as_ref().unwrap().playing);
    let current = retry[0].failure(false, "Lease rejected".into());
    let mut frame = app.shared.clone();
    frame
        .responses
        .push_back(soundroom_tui::worker::EditorResponse {
            tag: current.tag.unwrap(),
            reply: Arc::new(RemoteReply {
                request_id: "00000000-0000-0000-0000-000000000007".into(),
                status: RemoteReplyStatus::Rejected,
                revision: 8,
                message: "Lease rejected".into(),
                payload: None,
            }),
        });
    app.install(frame);
    assert!(!app.creative.as_ref().unwrap().playing);
    assert_eq!(app.creative.as_ref().unwrap().status, "Lease rejected");
}
#[test]
fn generated_clip_insert_keeps_original_authoring_revision() {
    let mut app = App::new();
    let mut state = shared(RemoteRole::Editor);
    state.capabilities = Some(Arc::new(creative_capabilities()));
    Arc::make_mut(state.snapshot.as_mut().unwrap())
        .session
        .tracks
        .as_mut()
        .unwrap()[0]
        .clips
        .as_mut()
        .unwrap()[0]
        .sound_design = Some(
        state.capabilities.as_ref().unwrap().designer_presets[0]
            .sound
            .clone(),
    );
    app.install(state.clone());
    app.parse_command("designer").unwrap();
    assert!(app.parse_command("audition").is_err());
    assert!(!app.creative.as_ref().unwrap().playing);
    Arc::make_mut(state.snapshot.as_mut().unwrap())
        .track_revisions
        .insert("00000000-0000-0000-0000-000000000002".into(), 8);
    Arc::make_mut(state.snapshot.as_mut().unwrap()).revision = 9;
    app.install(state);
    let requests = app.parse_command("insert").unwrap();
    let Work::Command {
        command, tracks, ..
    } = &requests[0]
    else {
        panic!()
    };
    assert_eq!(tracks["00000000-0000-0000-0000-000000000002"], 7);
    let RemoteCommand::Editor { intent } = command.as_ref() else {
        panic!()
    };
    assert_eq!(
        intent.strings["clipID"],
        "00000000-0000-0000-0000-000000000004"
    );
}
#[test]
fn native_editor_bake_and_track_audition_use_string_ids() {
    let mut app = App::new();
    let mut state = shared(RemoteRole::Operator);
    Arc::make_mut(state.snapshot.as_mut().unwrap()).session.tracks.as_mut().unwrap()[0].realtime_effects=Some(serde_json::from_value(json!([{"id":"00000000-0000-0000-0000-000000000006","kind":"echo","amount":0.5,"enabled":true}])).unwrap());
    app.install(state);
    let RemoteCommand::Editor { intent } = app.editor("pitch.apply") else {
        panic!()
    };
    assert_eq!(
        intent.strings["trackID"],
        "00000000-0000-0000-0000-000000000002"
    );
    assert_eq!(
        intent.strings["effectID"],
        "00000000-0000-0000-0000-000000000006"
    );
    let track = app.parse_command("play-track").unwrap();
    assert!(
        matches!(&track[0],Work::Command{command,..} if matches!(command.as_ref(),RemoteCommand::Editor{intent} if intent.strings.contains_key("trackID")))
    );
    let mix = app.parse_command("play").unwrap();
    assert!(
        matches!(&mix[0],Work::Command{command,..} if matches!(command.as_ref(),RemoteCommand::Editor{intent} if !intent.strings.contains_key("trackID")))
    );
}

#[test]
fn designer_keyboard_t_and_u_sound_notes_instead_of_navigation() {
    let mut app = App::new();
    let mut state = shared(RemoteRole::Operator);
    let mut cap = creative_capabilities();
    cap.editor_operations.push("designer.noteOn".into());
    state.capabilities = Some(Arc::new(cap));
    app.install(state);
    app.parse_command("designer").unwrap();
    app.key(Key::Tab);
    app.key(Key::Tab);
    for key in ['t', 'u'] {
        let work = app.key(Key::Char(key));
        assert!(
            matches!(&work[0],Work::Command{command,..} if matches!(command.as_ref(),RemoteCommand::Editor{intent} if intent.operation=="designer.noteOn"))
        );
        assert!(app.creative.is_some());
    }
}
#[test]
fn automation_commit_keeps_opening_revision_and_closes_after_queueing() {
    let mut app = App::new();
    let mut state = shared(RemoteRole::Editor);
    let mut cap = creative_capabilities();
    cap.features.push("automation".into());
    cap.commands.push("updateTrackControls".into());
    state.capabilities = Some(Arc::new(cap));
    app.install(state.clone());
    app.parse_command("automation volume").unwrap();
    app.key(Key::Char('n'));
    Arc::make_mut(state.snapshot.as_mut().unwrap())
        .track_revisions
        .insert("00000000-0000-0000-0000-000000000002".into(), 8);
    app.install(state);
    let work = app.key(Key::Enter);
    let Work::Command {
        tracks, command, ..
    } = &work[0]
    else {
        panic!()
    };
    assert_eq!(tracks["00000000-0000-0000-0000-000000000002"], 7);
    assert!(
        matches!(command.as_ref(),RemoteCommand::UpdateTrackControls{controls,..} if controls.mix_automation["volume"].len()==1)
    );
    assert!(app.automation.is_none());
}
#[test]
fn automation_focus_consumes_edit_keys_and_viewer_cannot_commit() {
    let mut app = App::new();
    let mut state = shared(RemoteRole::Viewer);
    let mut cap = creative_capabilities();
    cap.features.push("automation".into());
    state.capabilities = Some(Arc::new(cap));
    app.install(state);
    app.parse_command("automation pan").unwrap();
    let cursor = app.view.cursor_frame;
    assert!(app.key(Key::Char('l')).is_empty());
    assert_eq!(app.view.cursor_frame, cursor);
    assert!(app.key(Key::Enter).is_empty());
    assert!(app.automation.is_some());
}

fn pitch_shared(role: RemoteRole) -> Shared {
    let mut state = shared(role);
    let mut cap = creative_capabilities();
    cap.features.push("pitch".into());
    cap.commands.push("updateTrackControls".into());
    cap.editor_operations
        .extend(["pitch.analyze", "pitch.render", "pitch.targets"].map(str::to_owned));
    state.capabilities = Some(Arc::new(cap));
    Arc::make_mut(state.snapshot.as_mut().unwrap()).session.tracks.as_mut().unwrap()[0].realtime_effects=Some(vec![serde_json::from_value(json!({"id":"00000000-0000-0000-0000-000000000007","kind":"pitchCorrection","enabled":true,"parameters":{"reference":440}})).unwrap()]);
    state
}
#[test]
fn pitch_space_is_owned_preview_and_keeps_original_revision_on_commit() {
    let mut app = App::new();
    let mut state = pitch_shared(RemoteRole::Operator);
    app.install(state.clone());
    app.parse_command("pitch").unwrap();
    let work = app.key(Key::Char(' '));
    let Work::Command {
        command, tracks, ..
    } = &work[0]
    else {
        panic!()
    };
    let RemoteCommand::Editor { intent } = command.as_ref() else {
        panic!()
    };
    assert_eq!(intent.operation, "pitch.render");
    assert!(intent.flags["preview"]);
    assert_eq!(
        intent.strings["trackID"],
        "00000000-0000-0000-0000-000000000002"
    );
    assert_eq!(tracks.values().copied().collect::<Vec<_>>(), vec![7]);
    app.pitch.as_mut().unwrap().busy = false;
    app.parse_command("point 48000 C#4").unwrap();
    Arc::make_mut(state.snapshot.as_mut().unwrap())
        .track_revisions
        .insert("00000000-0000-0000-0000-000000000002".into(), 9);
    app.install(state);
    let work = app.key(Key::Enter);
    assert!(app.pitch.is_none());
    let Work::Command {
        command, tracks, ..
    } = &work[0]
    else {
        panic!()
    };
    assert!(matches!(
        command.as_ref(),
        RemoteCommand::UpdateTrackControls { .. }
    ));
    assert_eq!(tracks.values().copied().collect::<Vec<_>>(), vec![7]);
}
#[test]
fn pitch_preparation_close_is_kind_guarded_and_viewer_cannot_audition() {
    let mut app = App::new();
    app.install(pitch_shared(RemoteRole::Operator));
    app.parse_command("pitch").unwrap();
    app.pitch.as_mut().unwrap().busy = true;
    let work = app.key(Key::Escape);
    assert!(app.pitch.is_none());
    let Work::Command { command, .. } = &work[0] else {
        panic!()
    };
    let RemoteCommand::Editor { intent } = command.as_ref() else {
        panic!()
    };
    assert_eq!(intent.operation, "audition.stop");
    assert_eq!(intent.strings["kind"], "pitch");
    app.install(pitch_shared(RemoteRole::Viewer));
    app.parse_command("pitch").unwrap();
    assert!(app.key(Key::Char(' ')).is_empty());
    assert!(app.pitch.is_some());
    assert!(!app.pitch.as_ref().unwrap().busy);
    app.parse_command("point 48000 60").unwrap();
    assert!(app.key(Key::Enter).is_empty());
    assert!(app.pitch.is_some());
}

#[test]
fn precision_charts_are_not_overwritten_by_background_timeline() {
    let mut app = App::new();
    app.install(pitch_shared(RemoteRole::Operator));
    app.parse_command("pitch").unwrap();
    app.parse_command("point 48000 C4").unwrap();
    let canvas = app.draw(100, 30);
    let row = |n: usize| {
        canvas.cells[n * canvas.width..(n + 1) * canvas.width]
            .iter()
            .map(|c| c.ch)
            .collect::<String>()
    };
    assert!(row(2).contains("Pitch / Voice"));
    assert!(!row(3).contains("Track / M S A"));
    assert!(canvas.cells.iter().any(|c| c.fg == 203 && c.ch == '@'));
    app.key(Key::Escape);
    let mut state = pitch_shared(RemoteRole::Editor);
    Arc::make_mut(state.capabilities.as_mut().unwrap())
        .features
        .push("automation".into());
    app.install(state);
    app.parse_command("automation volume").unwrap();
    let canvas = app.draw(100, 30);
    let text = canvas.cells.iter().map(|c| c.ch).collect::<String>();
    assert!(text.contains("Volume"));
    assert!(!text.contains("Track / M S A"));
}

fn master_shared(role: RemoteRole) -> Shared {
    let mut state = shared(role);
    let mut cap = creative_capabilities();
    cap.commands.push("setTrackMute".into());
    cap.features.push("mastering".into());
    cap.editor_operations.extend(
        [
            "master.analyze",
            "master.proposal",
            "master.applyProposal",
            "jobs.cancel",
        ]
        .map(str::to_owned),
    );
    cap.effects=serde_json::from_value(json!([{"id":"gain","name":"Gain","parameters":[{"id":"gain","name":"Gain","minimum":-24,"maximum":24,"defaultValue":0,"unit":"dB","integer":false,"logarithmic":false,"automationUnit":"normalized"}],"factoryPresets":[],"modes":["realtime"]}])).unwrap();
    state.capabilities = Some(Arc::new(cap));
    let snapshot = Arc::make_mut(state.snapshot.as_mut().unwrap());
    snapshot.session.master_track = Some(
        serde_json::from_value(
            json!({"id":"00000000-0000-0000-0000-000000000010","name":"Master","clips":[]}),
        )
        .unwrap(),
    );
    snapshot
        .track_revisions
        .insert("00000000-0000-0000-0000-000000000010".into(), 6);
    snapshot.session.mastering_analysis=Some(serde_json::from_value(json!({"integratedLUFS":-19,"truePeakDB":-3,"crestDB":12,"bassFraction":0.4,"airFraction":0.2,"analyzedFrames":480000,"sections":[{"start":0,"end":480000,"loudness":-19,"truePeak":-3,"resonance":1}]})).unwrap());
    state
}
fn master_reply(app: &App, work: &Work) -> soundroom_tui::worker::EditorResponse {
    let Work::Command {
        command, revision, ..
    } = work
    else {
        panic!()
    };
    let RemoteCommand::Editor { intent } = command.as_ref() else {
        panic!()
    };
    soundroom_tui::worker::EditorResponse {
        tag: soundroom_tui::worker::ResponseTag {
            operation: intent.operation.clone(),
            workspace_id: Some(intent.strings["workspaceID"].clone()),
            track_id: intent.context.track_id.clone(),
            effect_id: None,
            range: None,
            source_revision: *revision,
            track_revision: None,
        },
        reply: Arc::new(RemoteReply {
            request_id: "00000000-0000-0000-0000-000000000011".into(),
            status: RemoteReplyStatus::Accepted,
            revision: app.shared.snapshot.as_ref().unwrap().revision,
            message: String::new(),
            payload: Some({
                use base64::Engine;
                base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&json!([{"id":"00000000-0000-0000-0000-000000000012","kind":"gain","enabled":true,"parameters":{"gain":2}}])).unwrap())
            }),
        }),
    }
}
#[test]
fn master_viewer_can_request_private_proposal_but_cannot_apply_or_play() {
    let mut app = App::new();
    app.install(master_shared(RemoteRole::Viewer));
    app.parse_command("master").unwrap();
    assert!(app.key(Key::Char(' ')).is_empty());
    assert!(app.parse_command("analyze").unwrap().is_empty());
    let work = app.parse_command("proposal").unwrap();
    let Work::Command {
        revision,
        tracks,
        authored,
        ..
    } = &work[0]
    else {
        panic!()
    };
    assert_eq!(*revision, None);
    assert!(tracks.is_empty());
    assert!(!authored);
    let mut state = master_shared(RemoteRole::Viewer);
    state.responses.push_back(master_reply(&app, &work[0]));
    app.install(state);
    assert_eq!(app.master.as_ref().unwrap().proposed.len(), 1);
    assert!(app.key(Key::Enter).is_empty());
    assert!(app.master.is_some());
    let canvas = app.draw(100, 30);
    let text = canvas.cells.iter().map(|c| c.ch).collect::<String>();
    assert!(text.contains("MASTER / Native"));
    assert!(!text.contains("Track / M S A"));
}
#[test]
fn master_proposal_commit_uses_opening_master_revision_instead_of_selected_voice() {
    let mut app = App::new();
    let state = master_shared(RemoteRole::Editor);
    app.install(state.clone());
    app.parse_command("master").unwrap();
    let work = app.parse_command("proposal").unwrap();
    let mut next = state.clone();
    next.responses.push_back(master_reply(&app, &work[0]));
    app.install(next);
    let mut next = state;
    Arc::make_mut(next.snapshot.as_mut().unwrap())
        .track_revisions
        .insert("00000000-0000-0000-0000-000000000010".into(), 9);
    app.install(next);
    let work = app.key(Key::Enter);
    let Work::Command {
        command,
        tracks,
        authored,
        ..
    } = &work[0]
    else {
        panic!()
    };
    assert!(*authored);
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks["00000000-0000-0000-0000-000000000010"], 6);
    assert!(
        matches!(command.as_ref(),RemoteCommand::Editor{intent} if intent.operation=="master.applyProposal")
    );
    assert!(app.master.is_some());
    assert!(app.master.as_ref().unwrap().busy);
}
#[test]
fn editor_can_cancel_own_pending_analysis_without_transport_authority_or_mutation_barrier() {
    let mut app = App::new();
    app.install(master_shared(RemoteRole::Editor));
    app.parse_command("master").unwrap();
    let work = app.parse_command("analyze").unwrap();
    assert_eq!(work.len(), 1);
    let cancel = app.parse_command("cancel").unwrap();
    let Work::Command {
        command,
        authored,
        tracks,
        revision,
    } = &cancel[0]
    else {
        panic!()
    };
    assert!(!authored);
    assert!(tracks.is_empty());
    assert_eq!(*revision, None);
    assert!(
        matches!(command.as_ref(),RemoteCommand::Editor{intent} if intent.operation=="jobs.cancel")
    );
    let work = app.key(Key::Escape);
    assert!(work.is_empty());
    assert!(app.master.is_none());
}

#[test]
fn local_rejection_clears_only_the_matching_workspace_and_no_false_receipt_is_created() {
    let mut app = App::new();
    app.install(master_shared(RemoteRole::Editor));
    app.parse_command("master").unwrap();
    let work = app.parse_command("analyze").unwrap();
    let mut failure = work[0].failure(false, "Not sent: queue full".into());
    let correct = failure.tag.clone();
    failure.tag.as_mut().unwrap().workspace_id = Some("older".into());
    app.work_failed(&failure);
    assert!(app.master.as_ref().unwrap().busy);
    failure.tag = correct;
    app.work_failed(&failure);
    assert!(!app.master.as_ref().unwrap().busy);
    assert!(app.shared.receipts.is_empty());
    assert_eq!(app.parse_command("analyze").unwrap().len(), 1);
}
#[test]
fn uncertain_authored_work_requires_a_fresh_snapshot_not_an_unrelated_metadata_delta() {
    let mut app = App::new();
    app.install(shared(RemoteRole::Editor));
    let work = app.key(Key::Char('m'));
    app.work_failed(&work[0].failure(true, "Outcome uncertain".into()));
    let mut state = shared(RemoteRole::Editor);
    Arc::make_mut(state.snapshot.as_mut().unwrap()).revision = 9;
    app.install(state.clone());
    assert!(app.key(Key::Char('s')).is_empty());
    state.fresh_snapshot = true;
    app.install(state);
    assert_eq!(app.key(Key::Char('s')).len(), 1);
}
#[test]
fn failed_interrupt_does_not_clear_an_inflight_authored_barrier() {
    let mut app = App::new();
    app.install(shared(RemoteRole::Operator));
    let edit = app.key(Key::Char('m'));
    assert_eq!(edit.len(), 1);
    let stop = app.parse_command("stop").unwrap();
    app.work_failed(&stop[0].failure(false, "Not sent: interrupt queue full".into()));
    assert!(app.key(Key::Char('s')).is_empty());
    app.work_failed(&edit[0].failure(false, "Not sent: edit queue full".into()));
    assert_eq!(app.key(Key::Char('s')).len(), 1);
}

#[test]
fn master_apply_receipt_preserves_conflicted_draft_then_closes_after_acceptance_with_newer_metadata(
) {
    let mut app = App::new();
    let mut state = master_shared(RemoteRole::Editor);
    app.install(state.clone());
    app.parse_command("master").unwrap();
    let query = app.parse_command("proposal").unwrap();
    state.responses.push_back(master_reply(&app, &query[0]));
    app.install(state.clone());
    state.responses.clear();
    let apply = app.key(Key::Enter);
    assert!(app.master.is_some());
    let mut rejected = master_reply(&app, &apply[0]);
    Arc::make_mut(&mut rejected.reply).status = RemoteReplyStatus::Conflict;
    Arc::make_mut(&mut rejected.reply).message = "Master changed".into();
    state.mutation_receipts.push_back((*rejected.reply).clone());
    state.responses.push_back(rejected);
    let snapshot = Arc::make_mut(state.snapshot.as_mut().unwrap());
    snapshot.revision = 9;
    snapshot
        .track_revisions
        .insert("00000000-0000-0000-0000-000000000010".into(), 9);
    app.install(state.clone());
    assert!(app.master.is_some());
    assert!(!app.master.as_ref().unwrap().busy);
    assert_eq!(app.master.as_ref().unwrap().revision, 6);
    state.responses.clear();
    state.mutation_receipts.clear();
    let query = app.parse_command("proposal").unwrap();
    state.responses.push_back(master_reply(&app, &query[0]));
    app.install(state.clone());
    assert_eq!(app.master.as_ref().unwrap().revision, 9);
    state.responses.clear();
    let apply = app.key(Key::Enter);
    let mut accepted = master_reply(&app, &apply[0]);
    Arc::make_mut(&mut accepted.reply).revision = 10;
    state.responses.push_back(accepted.clone());
    state.mutation_receipts.push_back((*accepted.reply).clone());
    let snapshot = Arc::make_mut(state.snapshot.as_mut().unwrap());
    snapshot.revision = 12;
    snapshot
        .track_revisions
        .insert("00000000-0000-0000-0000-000000000002".into(), 12);
    snapshot
        .track_revisions
        .insert("00000000-0000-0000-0000-000000000010".into(), 11);
    app.install(state);
    assert!(app.master.is_none());
    assert_eq!(app.key(Key::Char('m')).len(), 1);
}

fn editing_state() -> Shared {
    let mut state = shared(RemoteRole::Editor);
    let snapshot = Arc::make_mut(state.snapshot.as_mut().unwrap());
    snapshot.assets.push(RemoteAsset {
        channel_peaks: None,
        id: "00000000-0000-0000-0000-000000000005".into(),
        frames: 960000,
        channels: 1,
        peaks: vec![],
        waveform_start_frame: 0,
        frames_per_bin: None,
    });
    let clip = &mut snapshot.session.tracks.as_mut().unwrap()[0]
        .clips
        .as_mut()
        .unwrap()[0];
    clip.previous_states=Some(vec![serde_json::from_value(json!({"id":"00000000-0000-0000-0000-000000000007","operation":"Reverb","createdAt":0,"assetID":"00000000-0000-0000-0000-000000000008","name":"Dry","offset":200,"length":240000})).unwrap()]);
    state
}
fn edited(work: &[Work]) -> (&RemoteTrackEdit, &std::collections::BTreeMap<String, i64>) {
    match &work[0] {
        Work::Command {
            command, tracks, ..
        } => match command.as_ref() {
            RemoteCommand::EditTracks { edit } => (edit, tracks),
            _ => panic!("Expected atomic track edit"),
        },
        _ => panic!("Expected guarded command"),
    }
}
#[test]
fn clip_commands_guard_history_restore_trim_source_and_new_track_placement() {
    let mut app = App::new();
    app.install(editing_state());
    app.parse_command("history").unwrap();
    assert_eq!(app.panel, soundroom_tui::app::Panel::Clip);
    let canvas = app.draw(120, 36);
    assert!(canvas
        .cells
        .iter()
        .map(|c| c.ch)
        .collect::<String>()
        .contains("Before Reverb"));
    let work = app.parse_command("history-track 1").unwrap();
    let (edit, guards) = edited(&work);
    assert_eq!(guards["00000000-0000-0000-0000-000000000002"], 7);
    let c = &edit.tracks[0].clips.as_ref().unwrap()[0];
    assert_eq!(c.start, 0);
    assert_eq!(c.offset, 200);
    assert_eq!(c.length, 240000);
    assert_ne!(c.id, "00000000-0000-0000-0000-000000000004");
    assert!(c.previous_states.is_none());
    let mut app = App::new();
    app.install(editing_state());
    let work = app.parse_command("trim seconds:1 seconds:3").unwrap();
    let (edit, guards) = edited(&work);
    let c = &edit.tracks[0].clips.as_ref().unwrap()[0];
    assert_eq!((c.start, c.offset, c.length), (48000, 48000, 96000));
    assert_eq!(guards.values().copied().collect::<Vec<_>>(), vec![7]);
    let mut app = App::new();
    app.install(editing_state());
    assert!(app.parse_command("slip seconds:99").is_err());
    let work = app.parse_command("fade out ms:250").unwrap();
    assert_eq!(
        edited(&work).0.tracks[0].clips.as_ref().unwrap()[0].fade_out_frames,
        Some(12000)
    );
}
#[test]
fn range_copy_and_cut_preserve_audio_extent_without_removing_unselected_audio() {
    let mut app = App::new();
    app.install(editing_state());
    app.range = Some((48000, 96000));
    app.key(Key::Char('y'));
    app.view.cursor_frame = 144000;
    let work = app.key(Key::Char('p'));
    let (edit, _) = edited(&work);
    let clips = edit.tracks[0].clips.as_ref().unwrap();
    assert_eq!(clips.len(), 2);
    assert_eq!(
        (clips[1].start, clips[1].offset, clips[1].length),
        (144000, 48000, 48000)
    );
    let mut app = App::new();
    app.install(editing_state());
    app.range = Some((48000, 96000));
    let work = app.parse_command("cut").unwrap();
    let clips = edited(&work).0.tracks[0].clips.as_ref().unwrap();
    assert_eq!(clips.len(), 2);
    assert_eq!((clips[0].start, clips[0].length), (0, 48000));
    assert_eq!(
        (clips[1].start, clips[1].offset, clips[1].length),
        (96000, 96000, 384000)
    );
}
#[test]
fn track_clipboard_orders_independent_track_and_requires_the_reviewed_source_revision() {
    let mut app = App::new();
    app.install(editing_state());
    app.parse_command("copy-track automation").unwrap();
    app.view.cursor_frame = 48000;
    let work = app.key(Key::Char('p'));
    let (edit, guards) = edited(&work);
    assert_eq!(edit.name, "Paste track + automation");
    assert_eq!(guards["00000000-0000-0000-0000-000000000002"], 7);
    let id = edit.tracks[0].id.as_ref().unwrap();
    assert_ne!(id, "00000000-0000-0000-0000-000000000002");
    assert_eq!(&edit.track_order.as_ref().unwrap()[1], id);
    assert_eq!(edit.tracks[0].clips.as_ref().unwrap()[0].start, 48000);
    let mut app = App::new();
    app.install(editing_state());
    app.parse_command("copy-track").unwrap();
    let mut updated = editing_state();
    Arc::make_mut(updated.snapshot.as_mut().unwrap())
        .track_revisions
        .insert("00000000-0000-0000-0000-000000000002".into(), 8);
    app.install(updated);
    assert!(app.parse_command("paste").is_err());
}
#[test]
fn crossfade_preflights_source_extent_and_updates_both_edges_atomically() {
    let mut state = editing_state();
    let clips = Arc::make_mut(state.snapshot.as_mut().unwrap())
        .session
        .tracks
        .as_mut()
        .unwrap()[0]
        .clips
        .as_mut()
        .unwrap();
    let mut next = clips[0].clone();
    next.id = "00000000-0000-0000-0000-000000000009".into();
    next.start = 480000;
    next.length = 120000;
    clips.push(next);
    let mut app = App::new();
    app.install(state);
    let work = app.parse_command("crossfade ms:20").unwrap();
    let (edit, guards) = edited(&work);
    let clips = edit.tracks[0].clips.as_ref().unwrap();
    assert_eq!(clips[0].length, 480960);
    assert_eq!(clips[0].edge_protection_disabled, Some(false));
    assert_eq!(clips[1].edge_protection_disabled, Some(false));
    assert_eq!(guards.values().copied().collect::<Vec<_>>(), vec![7]);
    let mut app = App::new();
    app.install(editing_state());
    assert!(app.parse_command("crossfade ms:20").is_err());
}

#[test]
fn cut_clipboard_pastes_original_source_after_authoritative_split_or_removal() {
    let mut app = App::new();
    app.install(editing_state());
    app.range = Some((48000, 96000));
    let cut = app.parse_command("cut").unwrap();
    let edited_track = edited(&cut).0.tracks[0].clone();
    let mut accepted = editing_state();
    let snapshot = Arc::make_mut(accepted.snapshot.as_mut().unwrap());
    snapshot.session.tracks.as_mut().unwrap()[0] = edited_track;
    snapshot.revision = 9;
    snapshot
        .track_revisions
        .insert("00000000-0000-0000-0000-000000000002".into(), 8);
    accepted.mutation_receipts.push_back(RemoteReply {
        request_id: "00000000-0000-0000-0000-000000000020".into(),
        status: RemoteReplyStatus::Accepted,
        revision: 9,
        message: "Cut".into(),
        payload: None,
    });
    app.install(accepted);
    app.view.cursor_frame = 144000;
    app.range = None;
    let paste = app.key(Key::Char('p'));
    let (edit, guards) = edited(&paste);
    let clips = edit.tracks[0].clips.as_ref().unwrap();
    assert_eq!(clips.len(), 3);
    assert_eq!(
        (clips[2].start, clips[2].offset, clips[2].length),
        (144000, 48000, 48000)
    );
    assert_eq!(guards["00000000-0000-0000-0000-000000000002"], 8);
}
#[test]
fn right_click_opens_clip_inspector_and_viewers_cannot_restore_or_trim() {
    let mut app = App::new();
    app.install(editing_state());
    app.draw(120, 36);
    assert!(app
        .key(Key::Mouse {
            button: 2,
            column: 40,
            row: 4,
            released: false
        })
        .is_empty());
    assert_eq!(app.panel, soundroom_tui::app::Panel::Clip);
    let mut state = editing_state();
    state.role = Some(RemoteRole::Viewer);
    let mut viewer = App::new();
    viewer.install(state);
    viewer.parse_command("history").unwrap();
    assert!(viewer.parse_command("restore 1").unwrap().is_empty());
    assert!(viewer.parse_command("trim 0 24000").unwrap().is_empty());
    assert!(viewer.status.contains("read-only"));
}

fn transfer_state(role: RemoteRole) -> Shared {
    let mut s = shared(role);
    let mut c = creative_capabilities();
    c.commands.extend(
        [
            "beginUpload",
            "uploadChunk",
            "finishUpload",
            "exportMix",
            "downloadArtifact",
            "setTrackMute",
        ]
        .map(str::to_owned),
    );
    s.capabilities = Some(Arc::new(c));
    s
}
#[test]
fn transfer_commands_keep_paths_local_guard_opening_track_and_gate_roles_and_features() {
    let mut app = App::new();
    app.install(transfer_state(RemoteRole::Editor));
    app.view.cursor_frame = 96000;
    let work = app
        .parse_command("import \"/private/tmp/client-only recording.wav\"")
        .unwrap();
    let Work::Transfer(soundroom_tui::transfers::TransferRequest::Import {
        path,
        track_id,
        frame,
        revision,
        tracks,
        maximum_bytes,
        chunk_bytes,
    }) = &work[0]
    else {
        panic!()
    };
    assert_eq!(
        path.to_str(),
        Some("/private/tmp/client-only recording.wav")
    );
    assert_eq!(
        track_id.as_deref(),
        Some("00000000-0000-0000-0000-000000000002")
    );
    assert_eq!((*frame, *revision), (96000, 8));
    assert_eq!(tracks.values().copied().collect::<Vec<_>>(), vec![7]);
    assert_eq!((*maximum_bytes, *chunk_bytes), (2_147_483_648, 262144));
    assert!(
        matches!(&app.parse_command("import voice.wav new").unwrap()[0],Work::Transfer(soundroom_tui::transfers::TransferRequest::Import{track_id:None,tracks,..})if tracks.is_empty())
    );
    assert!(
        matches!(&app.parse_command("export mix.wav \"My mix\"").unwrap()[0],Work::Transfer(soundroom_tui::transfers::TransferRequest::Export{name,..})if name=="My mix")
    );
    assert!(matches!(
        app.parse_command("transfer-cancel").unwrap()[0],
        Work::CancelTransfer
    ));
    let mut state = transfer_state(RemoteRole::Viewer);
    let artifact:RemoteArtifact=serde_json::from_value(json!({"id":"00000000-0000-0000-0000-000000000040","name":"Mix.wav","bytes":100,"sha256":"0".repeat(64)})).unwrap();
    state.artifacts.push(artifact.clone());
    let mut viewer = App::new();
    viewer.install(state);
    assert!(viewer.parse_command("import voice.wav").is_err());
    assert!(viewer.parse_command("export mix.wav").is_err());
    assert!(
        matches!(&viewer.parse_command("download 1 local.wav").unwrap()[0],Work::Transfer(soundroom_tui::transfers::TransferRequest::Download{artifact:a,..})if a==&artifact)
    );
    viewer.parse_command("files").unwrap();
    assert_eq!(viewer.panel, soundroom_tui::app::Panel::Files);
    let mut limited = transfer_state(RemoteRole::Editor);
    Arc::make_mut(limited.capabilities.as_mut().unwrap())
        .commands
        .retain(|c| c != "finishUpload");
    app.install(limited);
    assert!(app.parse_command("import voice.wav").is_err());
}
#[test]
fn import_finish_blocks_edits_until_receipt_and_metadata_but_transport_remains_available() {
    let mut app = App::new();
    let mut state = transfer_state(RemoteRole::Operator);
    state.transfer = Some(soundroom_tui::transfers::TransferView {
        label: "Voice.wav".into(),
        phase: "Importing on host".into(),
        completed: 100,
        total: 100,
        active: true,
        importing: true,
        request_id: None,
    });
    app.install(state.clone());
    assert!(app.key(Key::Char('m')).is_empty());
    assert!(
        matches!(&app.key(Key::Char(' '))[0],Work::Command{command,..}if matches!(command.as_ref(),RemoteCommand::Editor{intent}if intent.operation=="transport.play"))
    );
    state.transfer.as_mut().unwrap().active = false;
    state.transfer.as_mut().unwrap().importing = false;
    state.mutation_receipts.push_back(RemoteReply {
        request_id: "00000000-0000-0000-0000-000000000041".into(),
        status: RemoteReplyStatus::Accepted,
        revision: 9,
        message: "Imported".into(),
        payload: None,
    });
    app.install(state.clone());
    assert!(app.key(Key::Char('m')).is_empty());
    state.mutation_receipts.clear();
    Arc::make_mut(state.snapshot.as_mut().unwrap()).revision = 9;
    app.install(state);
    assert_eq!(app.key(Key::Char('m')).len(), 1);
}
#[test]
fn immediate_import_receipt_also_waits_for_metadata_without_a_visible_preparing_frame() {
    let mut app = App::new();
    app.install(transfer_state(RemoteRole::Editor));
    let mut state = transfer_state(RemoteRole::Editor);
    state.mutation_receipts.push_back(RemoteReply {
        request_id: "00000000-0000-0000-0000-000000000041".into(),
        status: RemoteReplyStatus::Accepted,
        revision: 10,
        message: "Imported".into(),
        payload: None,
    });
    app.install(state.clone());
    assert!(app.key(Key::Char('m')).is_empty());
    state.mutation_receipts.clear();
    Arc::make_mut(state.snapshot.as_mut().unwrap()).revision = 10;
    app.install(state);
    assert_eq!(app.key(Key::Char('m')).len(), 1);
}

#[test]
fn creative_local_files_stay_private_and_never_block_or_gain_transport_authority() {
    let mut app = App::new();
    let mut state = shared(RemoteRole::Viewer);
    state.capabilities = Some(Arc::new(creative_capabilities()));
    app.install(state.clone());
    app.parse_command("designer").unwrap();
    let work = app
        .parse_command("midi-load \"/local/My Song.mid\"")
        .unwrap();
    let Work::LocalFile(request) = &work[0] else {
        panic!("Local path must not be a server command")
    };
    assert!(
        matches!(&request.operation,soundroom_tui::localfiles::Operation::ReadMIDI(path) if path.to_string_lossy()=="/local/My Song.mid")
    );
    assert!(app.key(Key::Char(' ')).is_empty());
    assert!(app.parse_command("midi-load other.mid").is_err());
    state.local_files = VecDeque::from([soundroom_tui::localfiles::Response {
        workspace: request.workspace.clone(),
        id: request.id.clone(),
        epoch: request.epoch,
        result: Err("Choose a regular MIDI file.".into()),
    }]);
    app.install(state);
    assert_eq!(
        app.creative.as_ref().unwrap().status,
        "Choose a regular MIDI file."
    );
    assert_eq!(
        app.parse_command("preset-save local.json").unwrap().len(),
        1
    );
    assert!(app.parse_command("insert").unwrap().is_empty());
}

#[test]
fn recipes_gate_codecs_scope_and_preserve_private_selection_without_audio_on_client() {
    let mut app = App::new();
    let mut state = shared(RemoteRole::Editor);
    let mut cap = creative_capabilities();
    cap.commands
        .extend(["exportMix", "downloadArtifact"].map(str::to_owned));
    cap.editor_operations.push("project.export".into());
    cap.codecs = vec!["WAV".into(), "AIFF".into()];
    state.capabilities = Some(Arc::new(cap));
    app.install(state.clone());
    app.range = Some((96000, 48000));
    let w = app
        .parse_command("export \"/local/mix.aiff\" \"My Mix\" selection no-tails")
        .unwrap();
    assert!(
        matches!(&w[0],Work::Transfer(soundroom_tui::transfers::TransferRequest::ProjectExport{path:Some(path),name,format,context,tails:false,tracks:None}) if path.to_string_lossy()=="/local/mix.aiff"&&name=="My Mix"&&format=="aiff"&&context.range_start==Some(48000)&&context.range_end==Some(96000))
    );
    assert!(app.parse_command("export /local/mix.flac").is_err());
    assert!(app.parse_command("stems wav wrong").is_err());
    let w = app.parse_command("stems wav track no-tails").unwrap();
    assert!(
        matches!(&w[0],Work::Transfer(soundroom_tui::transfers::TransferRequest::ProjectExport{path:None,tracks:Some(ids),..}) if ids.len()==1)
    );
    state.role = Some(RemoteRole::Viewer);
    app.install(state);
    assert!(app.parse_command("stems wav all").is_err());
}
#[test]
fn project_numbers_and_host_output_keep_role_and_device_identity() {
    let mut app = App::new();
    let mut state = shared(RemoteRole::Operator);
    Arc::make_mut(state.snapshot.as_mut().unwrap()).devices.push(serde_json::from_value(json!({"id":2,"uid":"HOST_OUT","name":"Host output","inputChannels":0,"outputChannels":2})).unwrap());
    state.events.push_back(RemoteEvent::Projects {
        projects: vec!["Demo.soundroom".into(), "Mix.soundroom".into()],
    });
    app.install(state.clone());
    let w = app.parse_command("open 2").unwrap();
    assert!(
        matches!(&w[0],Work::Command{command,..} if matches!(command.as_ref(),RemoteCommand::OpenProject{id} if id=="Mix.soundroom"))
    );
    app.cancel_pending();
    let w = app.parse_command("output HOST_OUT").unwrap();
    assert!(
        matches!(&w[0],Work::Command{command,authored:false,..} if matches!(command.as_ref(),RemoteCommand::SetDevices{output_uid:Some(uid),input_uid:Some(input)} if uid=="HOST_OUT"&&input=="HOST_MIC"))
    );
    assert!(app.parse_command("output LOCAL_OUT").is_err());
    state.role = Some(RemoteRole::Editor);
    app.install(state);
    assert!(app.parse_command("output HOST_OUT").unwrap().is_empty());
}

fn effect_library_state(role: RemoteRole) -> Shared {
    let mut state = shared(role);
    let mut cap = creative_capabilities();
    cap.commands.push("updateTrackControls".into());
    cap.effects.push(serde_json::from_value(json!({"id":"gain","name":"Gain","parameters":[{"id":"gain","name":"Gain","minimum":-24,"maximum":24,"defaultValue":0,"unit":"dB","logarithmic":false,"integer":false,"automationUnit":"normalized"}],"factoryPresets":[],"modes":["realtime"]})).unwrap());
    state.capabilities = Some(Arc::new(cap));
    Arc::make_mut(state.snapshot.as_mut().unwrap()).session.tracks.as_mut().unwrap()[0].realtime_effects=Some(vec![serde_json::from_value(json!({"id":"00000000-0000-0000-0000-000000000007","kind":"gain","amount":0.75,"parameters":{},"automation":[{"frame":0,"value":0.8}],"parameterAutomation":{},"bakedRanges":[{"startFrame":100,"endFrame":200}]})).unwrap()]);
    state
}
#[test]
fn effect_custom_save_receipt_publishes_library_and_loading_never_authors_audio() {
    let mut app = App::new();
    let mut state = effect_library_state(RemoteRole::Editor);
    app.install(state.clone());
    let w = app
        .parse_command("fx-preset-save /local/new.json \"Quiet bass\"")
        .unwrap();
    let Work::LocalFile(r) = &w[0] else { panic!() };
    let soundroom_tui::localfiles::Operation::WriteEffects(_, presets, _) = &r.operation else {
        panic!()
    };
    assert_eq!(presets[0].legacy_amount, Some(0.75));
    state.local_files = VecDeque::from([soundroom_tui::localfiles::Response {
        workspace: r.workspace.clone(),
        id: r.id.clone(),
        epoch: 0,
        result: Ok(soundroom_tui::localfiles::Data::Saved),
    }]);
    app.install(state.clone());
    let w = app.parse_command("fx-custom 0").unwrap();
    let Work::Command {
        command,
        tracks,
        authored: true,
        ..
    } = &w[0]
    else {
        panic!()
    };
    assert_eq!(tracks.values().copied().collect::<Vec<_>>(), vec![7]);
    let RemoteCommand::UpdateTrackControls { controls, .. } = command.as_ref() else {
        panic!()
    };
    let effect = &controls.realtime_effects[0];
    assert_eq!(effect.amount, Some(0.75));
    assert!(effect.parameters.as_ref().unwrap().is_empty());
    assert_eq!(effect.automation.as_ref().unwrap().len(), 1);
    assert_eq!(effect.baked_ranges.as_ref().unwrap().len(), 1);
    app.cancel_pending();
    assert!(app
        .parse_command("fx-preset-save /local/duplicate.json \"Quiet bass\"")
        .is_err());
    let w = app
        .parse_command("fx-presets-load /local/library.json")
        .unwrap();
    assert!(matches!(&w[0], Work::LocalFile(_)));
    assert!(app.key(Key::Char(' ')).is_empty()); // editor never gains transport permission
}
#[test]
fn effect_library_navigation_is_private_and_custom_application_preserves_automation() {
    let mut app = App::new();
    let mut state = effect_library_state(RemoteRole::Editor);
    app.install(state.clone());
    let w = app.parse_command("fx-presets-load local.json").unwrap();
    let Work::LocalFile(r) = &w[0] else { panic!() };
    let presets = (0..30)
        .map(|i| soundroom_tui::localfiles::SavedEffectPreset {
            id: soundroom_client::auth::new_id().unwrap(),
            name: format!("My gain {i}"),
            kind: RealtimeEffectKind::Gain,
            parameters: std::collections::BTreeMap::from([("gain".into(), -6.)]),
            legacy_amount: None,
        })
        .collect();
    state.local_files = VecDeque::from([soundroom_tui::localfiles::Response {
        workspace: r.workspace.clone(),
        id: r.id.clone(),
        epoch: 0,
        result: Ok(soundroom_tui::localfiles::Data::Effects(Arc::new(presets))),
    }]);
    app.install(state.clone());
    let view = app.view.clone();
    assert_eq!(app.panel, soundroom_tui::app::Panel::EffectPresets);
    app.key(Key::End);
    app.key(Key::Char('d'));
    app.key(Key::Char('d'));
    assert_eq!(app.view.selected_track, view.selected_track);
    assert_eq!(app.view.cursor_frame, view.cursor_frame);
    let canvas = app.draw(100, 24);
    let text = canvas.cells.iter().map(|c| c.ch).collect::<String>();
    assert!(text.contains("My gain 29"));
    let w = app.key(Key::Enter);
    let Work::Command { command, .. } = &w[0] else {
        panic!()
    };
    let RemoteCommand::UpdateTrackControls { controls, name, .. } = command.as_ref() else {
        panic!()
    };
    assert_eq!(name, "Preset My gain 29");
    assert_eq!(
        controls.realtime_effects[0].parameters.as_ref().unwrap()["gain"],
        -6.
    );
    assert_eq!(
        controls.realtime_effects[0]
            .automation
            .as_ref()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        controls.realtime_effects[0]
            .baked_ranges
            .as_ref()
            .unwrap()
            .len(),
        1
    );
    app.cancel_pending();
    state.role = Some(RemoteRole::Viewer);
    state.local_files.clear();
    app.install(state);
    assert!(app.key(Key::Enter).is_empty());
}
