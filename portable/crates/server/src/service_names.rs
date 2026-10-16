//! Session titles are authored metadata, distinct from a project's filename.
use super::*;

pub(super) fn validated(raw: &str) -> Result<String, Error> {
    let name = raw.trim();
    if !safe_filename(name) {
        return Err(protocol("Enter a short session name without slashes, colons, control characters or a leading dot"));
    }
    Ok(name.to_owned())
}

pub(super) fn project_title(raw: &str) -> Result<String, Error> {
    let name = raw.trim();
    validated(name.strip_suffix(".soundroom").unwrap_or(name))
}

pub(super) fn recover_title(session: &mut Session, path: &Path) {
    if session.name.trim().is_empty() || session.name == "Untitled session" {
        if let Some(title) = path.file_stem().and_then(|name| name.to_str()) {
            if let Ok(title) = validated(title) {
                session.name = title;
            }
        }
    }
}

/// A saved snapshot retains its requested title even if newer edits arrive.
/// Only the exact still-current source may adopt it as an authored edit.
pub(super) fn saved_title(
    shared: &Shared,
    peer: &Identity,
    request: &RemoteRequest,
    expected: u64,
    title: &str,
) {
    let Ok(mut state) = shared.lock() else { return };
    if state.authority.revision != expected
        || state.require_editable().is_err()
        || !state.registry.authorizes(&peer.credential, &peer.role)
        || state.authority.session.name == title
    {
        return;
    }
    let before = state.authority.session.clone();
    let mut candidate = before.clone();
    candidate.name = title.into();
    let mut fenced = request.clone();
    fenced.expected_revision = Some(expected as i64);
    if state.commit(peer, &fenced, candidate).is_ok() {
        if state.publish_changes(before, expected).is_err() {
            for peer in state.peers.values() {
                peer.out.close();
            }
        }
        state.history_events();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_delayed_save_cannot_rename_a_newer_session_or_a_recording() {
        let (root, shared, peer, _) = publication_tests::control_fixture(0);
        let request: RemoteRequest =
            serde_json::from_value(json!({"id":new_id().unwrap(), "clientID":peer.runtime,
            "expectedTrackRevisions":{}, "command":{"type":"saveProject","name":"Snapshot"}}))
            .unwrap();
        {
            let mut state = shared.lock().unwrap();
            state.authority.revision = 1;
            state.authority.session.name = "Newer session".into();
        }
        saved_title(&shared, &peer, &request, 0, "Snapshot");
        assert_eq!(
            shared.lock().unwrap().authority.session.name,
            "Newer session"
        );
        shared.lock().unwrap().recording_finalizing = true;
        saved_title(&shared, &peer, &request, 1, "Snapshot");
        assert_eq!(
            shared.lock().unwrap().authority.session.name,
            "Newer session"
        );
        drop(shared);
        std::fs::remove_dir_all(root).unwrap();
    }
}
