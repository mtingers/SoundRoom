use soundroom_engine::{
    authority::{Authority, Role},
    model::{Session, Track, MASTER_ID},
};
use std::collections::BTreeMap;
const A: &str = "11111111-1111-4111-8111-111111111111";
const B: &str = "22222222-2222-4222-8222-222222222222";
#[test]
fn structural_master_and_name_history_preserve_scoped_conflicts() {
    let mut a = Authority::new(Session::default()).unwrap();
    let mut candidate = a.session.clone();
    candidate.tracks.push(Track {
        id: A.into(),
        ..Default::default()
    });
    candidate.name = "Named".into();
    a.edit_session("editor", Role::Editor, Some(0), &BTreeMap::new(), candidate)
        .unwrap();
    let mut candidate = a.session.clone();
    candidate.master_track.gain = 0.8;
    a.edit_session(
        "operator",
        Role::Operator,
        None,
        &BTreeMap::from([(MASTER_ID.into(), 0)]),
        candidate,
    )
    .unwrap();
    a.undo("editor", Role::Editor).unwrap();
    assert!(a.session.tracks.is_empty());
    assert_eq!(a.session.master_track.gain, 0.8);
    assert!(!a.track_revisions.contains_key(A));
    a.redo("editor", Role::Editor).unwrap();
    assert_eq!(a.session.name, "Named");
    assert_eq!(a.session.master_track.gain, 0.8);
    a.undo("operator", Role::Operator).unwrap();
    assert_eq!(a.session.master_track.gain, 1.);
    let mut master = a.session.master_track.clone();
    master.gain = 0.6;
    let version = a.track_revisions[MASTER_ID];
    a.edit_tracks(
        "operator",
        Role::Operator,
        "request",
        &BTreeMap::from([(MASTER_ID.into(), version)]),
        vec![master],
    )
    .unwrap();
    assert_eq!(a.session.master_track.gain, 0.6);
}
#[test]
fn stale_structural_and_reorder_history_do_not_overwrite_other_authors() {
    let session = Session {
        tracks: vec![
            Track {
                id: A.into(),
                ..Default::default()
            },
            Track {
                id: B.into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let mut a = Authority::new(session).unwrap();
    let mut candidate = a.session.clone();
    candidate.tracks.swap(0, 1);
    assert!(a
        .edit_session("a", Role::Editor, None, &BTreeMap::new(), candidate.clone())
        .is_err());
    a.edit_session("a", Role::Editor, Some(0), &BTreeMap::new(), candidate)
        .unwrap();
    let mut candidate = a.session.clone();
    candidate.tracks.swap(0, 1);
    a.edit_session("b", Role::Editor, Some(1), &BTreeMap::new(), candidate)
        .unwrap();
    assert!(a.undo("a", Role::Editor).is_err());
    assert_eq!(a.session.tracks[0].id, A);
    assert!(a
        .edit_session(
            "viewer",
            Role::Viewer,
            Some(2),
            &BTreeMap::new(),
            a.session.clone()
        )
        .is_err());
}

#[test]
fn labeled_gestures_undo_once_and_do_not_cross_author_or_scope_boundaries() {
    let session = Session {
        tracks: vec![
            Track {
                id: A.into(),
                ..Default::default()
            },
            Track {
                id: B.into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let mut authority = Authority::new(session).unwrap();
    for (index, gain) in [0.7, 0.4, 0.2].into_iter().enumerate() {
        let mut candidate = authority.session.clone();
        candidate.tracks[0].gain = gain;
        authority
            .edit_session(
                "a",
                Role::Editor,
                None,
                &BTreeMap::from([(A.into(), index as u64)]),
                candidate,
            )
            .unwrap();
        authority
            .label_last_change("a", "Adjust volume", Some("drag-1"))
            .unwrap();
    }
    assert_eq!(authority.history_state("a"), (Some("Adjust volume"), None));
    assert_eq!(authority.history_state("b"), (None, None));
    authority.undo("a", Role::Editor).unwrap();
    assert_eq!(authority.session.tracks[0].gain, 1.);
    assert_eq!(authority.history_state("a"), (None, Some("Adjust volume")));
    authority.redo("a", Role::Editor).unwrap();
    assert_eq!(authority.session.tracks[0].gain, 0.2);
    let mut candidate = authority.session.clone();
    candidate.tracks[0].gain = 0.8;
    authority
        .edit_session(
            "b",
            Role::Editor,
            None,
            &BTreeMap::from([(A.into(), authority.track_revisions[A])]),
            candidate,
        )
        .unwrap();
    authority
        .label_last_change("b", "Other author", Some("drag-1"))
        .unwrap();
    assert_eq!(authority.history_state("a"), (None, None));
    assert!(authority.undo("a", Role::Editor).is_err());
    authority.undo("b", Role::Editor).unwrap();
    assert_eq!(authority.history_state("a").0, Some("Adjust volume"));
}

fn control_edit(
    a: &mut Authority,
    author: &str,
    id: &str,
    controls: soundroom_engine::authority::MixControls,
    request: &str,
    gesture: Option<&str>,
) -> soundroom_engine::Result<soundroom_engine::authority::Receipt> {
    let expected = BTreeMap::from([(id.into(), a.track_revisions[id])]);
    a.edit_controls(
        author,
        Role::Editor,
        soundroom_engine::authority::ControlEdit {
            request_id: request,
            expected: &expected,
            track_id: id,
            controls,
            label: "Adjust mixer",
            gesture,
        },
    )
}
#[test]
fn compact_control_history_preserves_mixed_content_undo_conflicts_and_gestures() {
    use soundroom_engine::authority::MixControls;
    let mut a = Authority::new(Session {
        tracks: vec![Track {
            id: A.into(),
            ..Default::default()
        }],
        ..Default::default()
    })
    .unwrap();
    let mut controls = MixControls::from_track(&a.session.tracks[0]);
    controls.gain = 0.8;
    control_edit(&mut a, "a", A, controls, "gain1", Some("gesture")).unwrap();
    controls.gain = 0.6;
    control_edit(&mut a, "a", A, controls, "gain2", Some("gesture")).unwrap();
    assert_eq!(a.history_state("a").0, Some("Adjust mixer"));
    let mut renamed = a.session.clone();
    renamed.tracks[0].name = "Another author".into();
    let expected = BTreeMap::from([(A.into(), 2)]);
    a.edit_session("b", Role::Editor, None, &expected, renamed)
        .unwrap();
    assert!(a.undo("a", Role::Editor).is_err());
    assert_eq!(a.session.tracks[0].gain, 0.6);
    a.undo("b", Role::Editor).unwrap();
    a.undo("a", Role::Editor).unwrap();
    assert_eq!(a.session.tracks[0].gain, 1.);
    assert!(
        a.undo("a", Role::Editor).is_err(),
        "The gesture is one compact action"
    );
    a.redo("a", Role::Editor).unwrap();
    assert_eq!(a.session.tracks[0].gain, 0.6);
    controls = MixControls::from_track(&a.session.tracks[0]);
    controls.pan = 0.5;
    control_edit(&mut a, "b", A, controls, "pan", None).unwrap();
    assert!(
        a.undo("a", Role::Editor).is_err(),
        "Other mixer state is part of the same resource"
    );
    a.undo("b", Role::Editor).unwrap();
    a.undo("a", Role::Editor).unwrap();
    assert_eq!(a.session.tracks[0].gain, 1.);
}
#[test]
fn compact_control_requests_validate_before_mutation_and_deduplicate_exactly() {
    use soundroom_engine::authority::{ControlEdit, MixControls};
    let mut a = Authority::new(Session::default()).unwrap();
    let mut controls = MixControls::from_track(&a.session.master_track);
    let original = a.session.clone();
    for bad in [f32::NAN, f32::INFINITY, -0.1, 4.01] {
        controls.gain = bad;
        assert!(control_edit(&mut a, "a", MASTER_ID, controls, "bad", None).is_err());
        assert_eq!(a.session, original);
        assert_eq!(a.revision, 0);
    }
    controls.gain = 0.7;
    let expected = BTreeMap::from([(MASTER_ID.into(), 0)]);
    assert!(a
        .edit_controls(
            "viewer",
            Role::Viewer,
            ControlEdit {
                request_id: "denied",
                expected: &expected,
                track_id: MASTER_ID,
                controls,
                label: "Gain",
                gesture: None
            }
        )
        .is_err());
    assert!(a
        .edit_controls(
            "a",
            Role::Editor,
            ControlEdit {
                request_id: "invalid-label",
                expected: &expected,
                track_id: MASTER_ID,
                controls,
                label: "\n",
                gesture: None
            }
        )
        .is_err());
    let first = a
        .edit_controls(
            "a",
            Role::Editor,
            ControlEdit {
                request_id: "once",
                expected: &expected,
                track_id: MASTER_ID,
                controls,
                label: "Gain",
                gesture: None,
            },
        )
        .unwrap();
    let duplicate = a
        .edit_controls(
            "a",
            Role::Editor,
            ControlEdit {
                request_id: "once",
                expected: &expected,
                track_id: MASTER_ID,
                controls,
                label: "Gain",
                gesture: None,
            },
        )
        .unwrap();
    assert_eq!(first, duplicate);
    assert_eq!(a.revision, 1);
    controls.pan = 0.1;
    assert!(a
        .edit_controls(
            "a",
            Role::Editor,
            ControlEdit {
                request_id: "once",
                expected: &expected,
                track_id: MASTER_ID,
                controls,
                label: "Gain",
                gesture: None
            }
        )
        .is_err());
    assert!(a
        .edit_controls(
            "a",
            Role::Editor,
            ControlEdit {
                request_id: "stale",
                expected: &expected,
                track_id: MASTER_ID,
                controls,
                label: "Gain",
                gesture: None
            }
        )
        .is_err());
    assert_eq!(a.revision, 1);
}
#[test]
fn dense_track_compact_history_remains_bounded_and_undoable() {
    use soundroom_engine::{authority::MixControls, model::AutomationPoint};
    let mut track = Track {
        id: A.into(),
        ..Default::default()
    };
    track.mix_automation.insert(
        "volume".into(),
        (0..100000)
            .map(|frame| AutomationPoint {
                frame,
                value: 0.5,
                curve: "linear".into(),
                curve_start: 0.,
                curve_end: 1.,
                extra: BTreeMap::new(),
            })
            .collect(),
    );
    let mut a = Authority::new(Session {
        tracks: vec![track],
        ..Default::default()
    })
    .unwrap();
    for index in 0..200 {
        let mut controls = MixControls::from_track(&a.session.tracks[0]);
        controls.gain = if index % 2 == 0 { 0.5 } else { 0.8 };
        control_edit(&mut a, "a", A, controls, &format!("dense-{index}"), None).unwrap();
    }
    for _ in 0..200 {
        a.undo("a", Role::Editor).unwrap();
    }
    assert_eq!(a.session.tracks[0].gain, 1.);
    assert_eq!(a.session.tracks[0].mix_automation["volume"].len(), 100000);
}

#[test]
fn prepared_session_token_keeps_transport_and_normal_authored_history() {
    use soundroom_engine::authority::{SessionEditRequest, Transport};
    let mut authority = Authority::new(Session::default()).unwrap();
    let context = authority.session_edit_context();
    let mut candidate = context.session().clone();
    candidate.tracks.push(Track {
        id: A.into(),
        name: "Prepared instrument".into(),
        ..Default::default()
    });
    let expected = BTreeMap::new();
    let prepared = context
        .prepare(
            SessionEditRequest {
                author: "editor",
                role: Role::Editor,
                expected_revision: Some(0),
                expected: &expected,
                label: "Add instrument",
                gesture: None,
            },
            candidate,
        )
        .unwrap();
    assert_eq!(authority.revision, 0);
    assert!(authority.session.tracks.is_empty());
    assert_eq!(prepared.receipt().revision, 1);
    authority
        .backend_committed_transport(Transport::Playing { frame: 48000 })
        .unwrap();
    authority
        .commit_prepared_session("editor", Role::Editor, prepared)
        .unwrap();
    assert_eq!(authority.transport, Transport::Playing { frame: 48000 });
    assert_eq!(authority.history_state("editor").0, Some("Add instrument"));
    authority.undo("editor", Role::Editor).unwrap();
    assert!(authority.session.tracks.is_empty());
    authority.redo("editor", Role::Editor).unwrap();
    assert_eq!(authority.session.tracks[0].name, "Prepared instrument");
}
#[test]
fn prepared_tokens_reject_foreign_authority_role_identity_and_stale_state_without_mutation() {
    use soundroom_engine::authority::SessionEditRequest;
    for failure in ["foreign", "author", "role", "stale", "recording"] {
        let mut authority = Authority::new(Session::default()).unwrap();
        let context = authority.session_edit_context();
        let mut candidate = context.session().clone();
        candidate.name = "Prepared rename".into();
        let expected = BTreeMap::new();
        let prepared = context
            .prepare(
                SessionEditRequest {
                    author: "editor",
                    role: Role::Editor,
                    expected_revision: Some(0),
                    expected: &expected,
                    label: "Rename session",
                    gesture: None,
                },
                candidate,
            )
            .unwrap();
        if failure == "foreign" {
            authority = Authority::new(Session::default()).unwrap();
        }
        if failure == "stale" {
            let mut intervening = authority.session.clone();
            intervening.name = "Other author".into();
            authority
                .edit_session("other", Role::Editor, Some(0), &expected, intervening)
                .unwrap();
        }
        if failure == "recording" {
            authority
                .backend_committed_transport(soundroom_engine::authority::Transport::Recording {
                    frame: 100,
                })
                .unwrap();
        }
        let before = authority.session.clone();
        let revision = authority.revision;
        let history = authority.history_state("other").0.map(str::to_owned);
        assert!(
            authority
                .commit_prepared_session(
                    if failure == "author" {
                        "other"
                    } else {
                        "editor"
                    },
                    if failure == "role" {
                        Role::Operator
                    } else {
                        Role::Editor
                    },
                    prepared
                )
                .is_err(),
            "{failure}"
        );
        assert_eq!(authority.session, before, "{failure}");
        assert_eq!(authority.revision, revision, "{failure}");
        assert_eq!(
            authority.history_state("other").0,
            history.as_deref(),
            "{failure}"
        );
    }
}
