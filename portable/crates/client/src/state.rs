use crate::{models::*, Error};
use std::collections::BTreeSet;
pub const MASTER_TRACK_ID: &str = "E3E58100-5125-4E6F-B985-10AA11000001";
/// Ephemeral navigation stays client-local through snapshots and reconnects.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ViewState {
    pub selected_track: Option<String>,
    pub selected_clips: BTreeSet<String>,
    pub cursor_frame: i64,
    pub horizontal_frame: i64,
    pub zoom: f64,
    pub panel: Option<String>,
}
impl Default for ViewState {
    fn default() -> Self {
        Self {
            selected_track: None,
            selected_clips: BTreeSet::new(),
            cursor_frame: 0,
            horizontal_frame: 0,
            zoom: 1.,
            panel: None,
        }
    }
}
pub fn apply_delta(snapshot: &mut RemoteSnapshot, delta: RemoteSessionDelta) -> Result<(), Error> {
    if delta.revision < snapshot.revision {
        return Ok(());
    }
    // Native transport revisions can be repeated for metadata; forward gaps require resync.
    if delta.revision > snapshot.revision + 1 {
        return Err(Error::Conflict(
            "Session revision gap; refresh snapshot".into(),
        ));
    }
    let mut identities = BTreeSet::new();
    for track in &delta.tracks {
        let id = track
            .id
            .as_deref()
            .filter(|id| crate::validation::uuid(id))
            .ok_or_else(|| Error::Protocol("Delta track identity is missing".into()))?;
        if !identities.insert(id.to_ascii_uppercase()) {
            return Err(Error::Protocol("Repeated delta track identity".into()));
        }
    }
    if delta
        .control_patches
        .as_ref()
        .is_some_and(|patches| patches.len() > 1024)
    {
        return Err(Error::Protocol("Too many scalar control patches".into()));
    }
    let mut patches = std::collections::BTreeSet::new();
    for patch in delta.control_patches.as_deref().unwrap_or_default() {
        if !crate::validation::uuid(&patch.track_id)
            || !patches.insert(patch.track_id.to_ascii_uppercase())
            || patch
                .gain
                .is_some_and(|gain| !gain.is_finite() || !(0. ..=4.).contains(&gain))
            || patch
                .pan
                .is_some_and(|pan| !pan.is_finite() || !(-1. ..=1.).contains(&pan))
            || (patch.gain.is_none()
                && patch.pan.is_none()
                && patch.muted.is_none()
                && patch.solo.is_none()
                && patch.armed.is_none())
        {
            return Err(Error::Protocol("Invalid scalar control patch".into()));
        }
        let available = patch.track_id.eq_ignore_ascii_case(MASTER_TRACK_ID)
            && snapshot.session.master_track.is_some()
            || snapshot
                .session
                .tracks
                .as_deref()
                .unwrap_or_default()
                .iter()
                .chain(delta.tracks.iter())
                .any(|track| {
                    track
                        .id
                        .as_ref()
                        .is_some_and(|id| id.eq_ignore_ascii_case(&patch.track_id))
                });
        if !available
            || delta
                .removed_track_ids
                .iter()
                .any(|id| id.eq_ignore_ascii_case(&patch.track_id))
        {
            return Err(Error::Protocol("Scalar patch target is unavailable".into()));
        }
    }
    let tracks = snapshot.session.tracks.get_or_insert_with(Vec::new);
    let removed: BTreeSet<_> = delta
        .removed_track_ids
        .iter()
        .map(|id| id.to_ascii_uppercase())
        .collect();
    tracks.retain(|t| {
        !t.id
            .as_ref()
            .is_some_and(|id| removed.contains(&id.to_ascii_uppercase()))
    });
    for track in delta.tracks {
        let id = track
            .id
            .as_ref()
            .ok_or_else(|| Error::Protocol("Delta track identity is missing".into()))?;
        if id.eq_ignore_ascii_case(MASTER_TRACK_ID) {
            snapshot.session.master_track = Some(track);
        } else if let Some(index) = tracks.iter().position(|t| {
            t.id.as_ref()
                .is_some_and(|existing| existing.eq_ignore_ascii_case(id))
        }) {
            tracks[index] = track;
        } else {
            tracks.push(track);
        }
    }
    for controls in delta.controls {
        let target = if controls.track_id.eq_ignore_ascii_case(MASTER_TRACK_ID) {
            snapshot.session.master_track.as_mut()
        } else {
            tracks.iter_mut().find(|t| {
                t.id.as_ref()
                    .is_some_and(|id| id.eq_ignore_ascii_case(&controls.track_id))
            })
        };
        if let Some(track) = target {
            track.name = Some(controls.name);
            track.color = Some(controls.color);
            track.gain = Some(controls.gain);
            track.pan = Some(controls.pan);
            track.muted = Some(controls.muted);
            track.solo = Some(controls.solo);
            track.armed = Some(controls.armed);
            track.input_device_uid = controls.input_device_uid;
            track.input_device_name = controls.input_device_name;
            track.input_channel = Some(controls.input_channel);
            track.stereo_input = Some(controls.stereo_input);
            track.volume_automation_scale = controls.volume_automation_scale;
            track.mix_automation = Some(controls.mix_automation);
            track.routing = controls.routing;
            track.realtime_effects = Some(controls.realtime_effects);
        }
    }
    for patch in delta.control_patches.unwrap_or_default() {
        let target = if patch.track_id.eq_ignore_ascii_case(MASTER_TRACK_ID) {
            snapshot.session.master_track.as_mut()
        } else {
            tracks.iter_mut().find(|track| {
                track
                    .id
                    .as_ref()
                    .is_some_and(|id| id.eq_ignore_ascii_case(&patch.track_id))
            })
        };
        let track = target.expect("Scalar patch targets validated before mutation");
        if let Some(gain) = patch.gain {
            track.gain = Some(gain);
        }
        if let Some(pan) = patch.pan {
            track.pan = Some(pan);
        }
        if let Some(muted) = patch.muted {
            track.muted = Some(muted);
        }
        if let Some(solo) = patch.solo {
            track.solo = Some(solo);
        }
        if let Some(armed) = patch.armed {
            track.armed = Some(armed);
        }
    }
    if let Some(order) = delta.track_order {
        let ranks: std::collections::BTreeMap<_, _> = order
            .iter()
            .enumerate()
            .map(|(i, id)| (id.to_ascii_uppercase(), i))
            .collect();
        tracks.sort_by_key(|track| {
            track
                .id
                .as_ref()
                .and_then(|id| ranks.get(&id.to_ascii_uppercase()))
                .copied()
                .unwrap_or(usize::MAX)
        });
    }
    for (id, revision) in delta.track_revisions {
        snapshot
            .track_revisions
            .insert(id.to_ascii_uppercase(), revision);
    }
    snapshot
        .track_revisions
        .retain(|id, _| !removed.contains(&id.to_ascii_uppercase()));
    if let Some(timing) = delta.timing {
        snapshot.session.timing = Some(timing);
    }
    if let Some(name) = delta.name {
        snapshot.session.name = Some(name);
    }
    if delta.metadata_updated {
        snapshot.session.mastering_analysis = delta.mastering_analysis;
    }
    if let Some(version) = delta.session_version {
        snapshot.session.version = Some(version);
    }
    if let Some(status) = delta.status {
        snapshot.status = status;
    }
    for asset in delta.assets {
        if let Some(index) = snapshot
            .assets
            .iter()
            .position(|a| a.id.eq_ignore_ascii_case(&asset.id))
        {
            snapshot.assets[index] = asset;
        } else {
            snapshot.assets.push(asset);
        }
    }
    snapshot.revision = delta.revision;
    Ok(())
}
