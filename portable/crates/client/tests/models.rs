use serde_json::{json, Value};
use soundroom_client::{
    auth::{hmac_sha256, new_id, Credential},
    models::*,
    validation::{self, validate_command, validate_model},
};
#[test]
fn every_enum_value_and_descriptor_is_valid() {
    for (name, cases) in MODEL_ENUMS {
        for value in *cases {
            validate_model(&json!(value), name).unwrap();
        }
        assert!(validate_model(&json!("unsupported"), name).is_err());
    }
    let manifest: Value = serde_json::from_slice(include_bytes!(
        "../../../../protocol/generated/manifest.json"
    ))
    .unwrap();
    for (name, actual) in [
        (
            "commands",
            COMMAND_FIELDS
                .iter()
                .map(|(name, _)| *name)
                .collect::<std::collections::BTreeSet<_>>(),
        ),
        (
            "events",
            EVENT_FIELDS.iter().map(|(name, _)| *name).collect(),
        ),
        (
            "models",
            MODEL_FIELDS.iter().map(|(name, _)| *name).collect(),
        ),
    ] {
        assert_eq!(
            actual,
            manifest[name]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect(),
            "{name}"
        );
    }
}
#[test]
fn typed_request_matches_shared_fixture_and_rejects_wrong_values() {
    let bytes = include_bytes!("../../../../protocol/fixtures/request-seek.v3.json");
    let mut value: Value = serde_json::from_slice(bytes).unwrap();
    let object = value.as_object_mut().unwrap();
    object.remove("version");
    object.remove("kind");
    let decoded: RemoteRequest =
        validation::decode_model(&serde_json::to_vec(&value).unwrap(), "RemoteRequest").unwrap();
    assert!(matches!(
        decoded.command,
        RemoteCommand::Seek { frame: 48000 }
    ));
    assert_eq!(serde_json::to_value(decoded).unwrap(), value);
    for command in [
        json!({"type":"seek","frame":true}),
        json!({"type":"seek","frame":1.5}),
        json!({"type":"seek","frame":-1}),
        json!({"type":"seek","frame":9007199254740992u64}),
        json!({"type":"unsafeSelector"}),
        json!({"type":"seek","frame":0,"extra":1}),
    ] {
        assert!(validate_command(&command).is_err());
    }
    assert!(validate_command(&json!({"type":"setDevices","inputUID":"opaque:device"})).is_ok());
}
#[test]
fn write_requests_require_exact_begin_resource_fence_only() {
    use base64::Engine;
    let id = "12345678-1234-4234-8234-123456789abc";
    let payload = base64::engine::general_purpose::STANDARD.encode(
        serde_json::to_vec(
            &json!({"gestureID":id,"trackID":id,"target":{"kind":"volume"},"mode":"touch"}),
        )
        .unwrap(),
    );
    let mut request = json!({"id":id,"clientID":id,"expectedTrackRevisions":{id:2},"command":{"type":"editor","intent":{"operation":"automation.writeBegin","context":{"clipIDs":[],"cursorFrame":0},"integers":{},"numbers":{},"strings":{},"flags":{},"payload":payload}}});
    validate_model(&request, "RemoteRequest").unwrap();
    for revisions in [
        json!({}),
        json!({id:2,"22345678-1234-4234-8234-123456789abc":1}),
    ] {
        let mut changed = request.clone();
        changed["expectedTrackRevisions"] = revisions;
        assert!(validate_model(&changed, "RemoteRequest").is_err());
    }
    let mut changed = request.clone();
    changed["expectedRevision"] = json!(2);
    assert!(validate_model(&changed, "RemoteRequest").is_err());
    request["expectedTrackRevisions"] = json!({});
    request["command"]["intent"]["operation"] = json!("automation.writeValue");
    request["command"]["intent"]["payload"] = json!(base64::engine::general_purpose::STANDARD
        .encode(serde_json::to_vec(&json!({"gestureID":id,"sequence":1,"value":0.5})).unwrap()));
    validate_model(&request, "RemoteRequest").unwrap();
    request["expectedTrackRevisions"] = json!({id:2});
    assert!(validate_model(&request, "RemoteRequest").is_err());
}
#[test]
fn exact_seed_strings_and_nested_structs_validate() {
    for seed in ["0", "17", "18446744073709551615"] {
        validate_model(&json!({"seed":seed}), "OrganicSound").unwrap();
    }
    for seed in [
        json!(17),
        json!("017"),
        json!("18446744073709551616"),
        json!("-1"),
    ] {
        assert!(validate_model(&json!({"seed":seed}), "OrganicSound").is_err());
    }
    assert!(validate_model(&json!({"version":12}), "Session").is_err());
    assert!(validate_model(
        &json!({"tempo":120,"beatsPerBar":4,"beatUnit":3}),
        "MusicalTiming"
    )
    .is_err());
    assert!(validate_model(
        &json!({"notes":[],"waveform":"externalPlugin"}),
        "SoundDesignPattern"
    )
    .is_err());
}
#[test]
fn legacy_track_scalar_bounds_match_compact_controls() {
    for gain in [0., 1.5, 2., 4.] {
        validate_model(&json!({"gain":gain,"pan":0.}), "Track").unwrap();
    }
    for gain in [-0.01, 4.0001] {
        assert!(validate_model(&json!({"gain":gain}), "Track").is_err());
    }
    assert!(validate_model(&json!({"gain":4.,"pan":1.001}), "Track").is_err());
}
#[test]
fn hmac_rfc4231_vectors() {
    let out = hmac_sha256(&[0x0b; 20], b"Hi There");
    assert_eq!(
        hex(&out),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
    assert_eq!(
        hex(&hmac_sha256(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First"
        )),
        "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
    );
}
#[test]
fn credentials_are_typed_and_do_not_require_native_tls_psk() {
    use base64::Engine;
    let id = new_id().unwrap();
    let secret = base64::engine::general_purpose::STANDARD.encode([0; 32]);
    let value =
        json!({"version":3,"serverID":id,"credentialID":id,"clientSecret":secret,"role":"editor"});
    let c = Credential::from_bytes(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(c.role, RemoteRole::Editor);
    let mut invalid = value;
    invalid["role"] = json!("administrator");
    assert!(Credential::from_bytes(&serde_json::to_vec(&invalid).unwrap()).is_err());
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|v| format!("{v:02x}")).collect()
}
#[test]
fn reconnect_backoff_is_bounded_and_private_view_defaults_are_stable() {
    for attempt in [0, 1, 4, u32::MAX] {
        let delay = soundroom_client::client::reconnect_delay(attempt);
        assert!(delay >= std::time::Duration::from_millis(500));
        assert!(delay <= std::time::Duration::from_secs(10));
    }
    let view: soundroom_client::state::ViewState = serde_json::from_value(json!({})).unwrap();
    assert_eq!(view.zoom, 1.0);
    assert!(view.selected_track.is_none());
}
#[cfg(unix)]
#[test]
fn credential_files_require_owner_only_regular_files_and_never_follow_symlinks() {
    use std::os::unix::fs::PermissionsExt;
    let directory =
        std::env::temp_dir().join(format!("soundroom-credential-test-{}", new_id().unwrap()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("credential.json");
    let secret = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [0; 32]);
    let value = json!({"version":3,"serverID":"12345678-1234-4234-8234-123456789ABC","credentialID":"12345678-1234-4234-8234-123456789ABC","clientSecret":secret,"role":"editor"});
    let credential = Credential::from_bytes(&serde_json::to_vec(&value).unwrap()).unwrap();
    credential.write_new(&path).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(Credential::load(&path).is_ok());
    assert!(credential.write_new(&path).is_err());
    let alias = directory.join("alias.json");
    std::os::unix::fs::symlink(&path, &alias).unwrap();
    assert!(Credential::load(&alias).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
    assert!(Credential::load(&path).is_err());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn scalar_patch_descriptor_and_optional_delta_values_are_strict_and_compatible() {
    let id = "12345678-1234-4234-8234-123456789ABC";
    let (_, fields) = MODEL_FIELDS
        .iter()
        .find(|(name, _)| *name == "RemoteTrackControlPatch")
        .unwrap();
    assert_eq!(
        fields.iter().map(|field| field.name).collect::<Vec<_>>(),
        vec!["trackID", "gain", "pan", "muted", "solo", "armed"]
    );
    for patch in [
        json!({"trackID":id,"gain":4.}),
        json!({"trackID":id,"pan":-1.}),
        json!({"trackID":id,"muted":false,"solo":true,"armed":true}),
    ] {
        validate_model(&patch, "RemoteTrackControlPatch").unwrap();
        let typed: RemoteTrackControlPatch = serde_json::from_value(patch.clone()).unwrap();
        assert_eq!(serde_json::to_value(typed).unwrap(), patch);
    }
    for patch in [
        json!({"trackID":id}),
        json!({"trackID":id,"gain":4.0001}),
        json!({"trackID":id,"pan":1.1}),
        json!({"trackID":id,"muted":1}),
        json!({"trackID":id,"gain":0.5,"payload":{}}),
    ] {
        assert!(validate_model(&patch, "RemoteTrackControlPatch").is_err());
    }
    let mut delta = json!({"revision":1,"tracks":[],"controls":[],"removedTrackIDs":[],"trackRevisions":{},"metadataUpdated":false,"assets":[]});
    validate_model(&delta, "RemoteSessionDelta").unwrap();
    let legacy: RemoteSessionDelta = serde_json::from_value(delta.clone()).unwrap();
    assert!(serde_json::to_value(legacy)
        .unwrap()
        .get("controlPatches")
        .is_none());
    delta["controlPatches"] = json!([{"trackID":id,"gain":0.75}]);
    validate_model(&delta, "RemoteSessionDelta").unwrap();
    let typed: RemoteSessionDelta = serde_json::from_value(delta.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), delta);
    delta["controlPatches"] =
        json!([{"trackID":id,"gain":0.75},{"trackID":id.to_ascii_lowercase(),"pan":0.}]);
    assert!(validate_model(&delta, "RemoteSessionDelta").is_err());
}

#[test]
fn shared_write_domains_and_cursor_fixtures_match_all_languages() {
    let fixtures: Value = serde_json::from_slice(include_bytes!(
        "../../../../protocol/fixtures/automation-write-models.v3.json"
    ))
    .unwrap();
    for (index, fixture) in fixtures.as_array().unwrap().iter().enumerate() {
        let kind = fixture["type"].as_str().unwrap();
        assert_eq!(
            validate_model(&fixture["value"], kind).is_ok(),
            fixture["valid"].as_bool().unwrap(),
            "Fixture {index}: {kind}"
        );
    }
}
