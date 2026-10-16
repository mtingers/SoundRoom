//! Exact authored target proof. Read snapshots and candidate transforms execute
//! on workers; control authority still fences the final prepared-token commit.
use crate::Error;
use soundroom_client::models::{RemoteAutomationWriteTarget, RemoteAutomationWriteTargetKind};
use soundroom_engine::{
    automation_write::WriteTarget,
    model::{AutomationPoint, Session, Track},
};
use std::collections::BTreeSet;
#[derive(Clone, Debug, PartialEq)]
enum Saved {
    Volume {
        gain: f32,
        scale: Option<String>,
        points: Vec<AutomationPoint>,
    },
    Pan {
        pan: f32,
        points: Vec<AutomationPoint>,
    },
    Amount {
        kind: String,
        enabled: bool,
        amount: f32,
        parameters_empty: bool,
        primary_lane_present: bool,
        baked: Option<serde_json::Value>,
        points: Vec<AutomationPoint>,
    },
    Parameter {
        kind: String,
        enabled: bool,
        physical: Option<f32>,
        baked: Option<serde_json::Value>,
        points: Vec<AutomationPoint>,
    },
}
#[derive(Clone, Debug)]
pub(crate) struct Snapshot {
    pub track_id: String,
    pub target: RemoteAutomationWriteTarget,
    saved: Saved,
}
fn fail(message: &str) -> Error {
    Error::Conflict(message.into())
}
impl Snapshot {
    pub fn capture(
        session: &Session,
        track_id: &str,
        mut target: RemoteAutomationWriteTarget,
    ) -> Result<Self, Error> {
        let track = session
            .tracks
            .iter()
            .chain(std::iter::once(&session.master_track))
            .find(|t| t.id.eq_ignore_ascii_case(track_id))
            .ok_or_else(|| fail("Write target track is unavailable"))?;
        let points = |key: &str| track.mix_automation.get(key).cloned().unwrap_or_default();
        let saved = match target.kind {
            RemoteAutomationWriteTargetKind::Volume => Saved::Volume {
                gain: track.gain,
                scale: track.volume_automation_scale.clone(),
                points: points("volume"),
            },
            RemoteAutomationWriteTargetKind::Pan => Saved::Pan {
                pan: track.pan,
                points: points("pan"),
            },
            RemoteAutomationWriteTargetKind::EffectAmount
            | RemoteAutomationWriteTargetKind::EffectParameter => {
                let id = target
                    .effect_id
                    .as_deref()
                    .ok_or_else(|| fail("Write insert ID is required"))?;
                let effect = track
                    .realtime_effects
                    .iter()
                    .find(|e| e.id.eq_ignore_ascii_case(id))
                    .ok_or_else(|| fail("Write insert is unavailable"))?;
                target.effect_id = Some(effect.id.clone());
                if target.kind == RemoteAutomationWriteTargetKind::EffectAmount {
                    let primary = match effect.kind.as_str() {
                        "gain" => "gain",
                        "lowPass" => "frequency",
                        "echo" => "mix",
                        "warmth" => "drive",
                        _ => "",
                    };
                    Saved::Amount {
                        kind: effect.kind.clone(),
                        enabled: effect.enabled,
                        amount: effect.amount,
                        parameters_empty: effect.parameters.is_empty(),
                        primary_lane_present: effect
                            .parameter_automation
                            .get(primary)
                            .is_some_and(|p| !p.is_empty()),
                        baked: effect.extra.get("bakedRanges").cloned(),
                        points: effect.automation.clone(),
                    }
                } else {
                    let key = target
                        .parameter_id
                        .as_deref()
                        .ok_or_else(|| fail("Write parameter ID is required"))?;
                    Saved::Parameter {
                        kind: effect.kind.clone(),
                        enabled: effect.enabled,
                        physical: effect.parameters.get(key).copied(),
                        baked: effect.extra.get("bakedRanges").cloned(),
                        points: effect
                            .parameter_automation
                            .get(key)
                            .cloned()
                            .unwrap_or_default(),
                    }
                }
            }
        };
        let snapshot = Self {
            track_id: track.id.clone(),
            target,
            saved,
        };
        if snapshot.points().len() > crate::write_curve::MAX_LANE_POINTS
            || snapshot
                .points()
                .iter()
                .map(|p| p.frame)
                .collect::<BTreeSet<_>>()
                .len()
                != snapshot.points().len()
        {
            return Err(fail(
                "Ambiguous or oversized original Write lane requires review",
            ));
        }
        // A draft must be capable of retaining every original point field. New
        // unknown metadata cannot be silently stripped just to fit the wire DTO.
        for point in snapshot.points() {
            soundroom_client::validation::validate_model(
                &serde_json::to_value(point)?,
                "AutomationPoint",
            )?;
        }
        Ok(snapshot)
    }
    pub fn points(&self) -> &[AutomationPoint] {
        match &self.saved {
            Saved::Volume { points, .. }
            | Saved::Pan { points, .. }
            | Saved::Amount { points, .. }
            | Saved::Parameter { points, .. } => points,
        }
    }
    pub fn engine_target(&self) -> WriteTarget {
        match self.target.kind {
            RemoteAutomationWriteTargetKind::Volume => WriteTarget::Volume {
                track_id: self.track_id.clone(),
            },
            RemoteAutomationWriteTargetKind::Pan => WriteTarget::Pan {
                track_id: self.track_id.clone(),
            },
            RemoteAutomationWriteTargetKind::EffectAmount => WriteTarget::EffectAmount {
                track_id: self.track_id.clone(),
                effect_id: self.target.effect_id.clone().unwrap(),
            },
            RemoteAutomationWriteTargetKind::EffectParameter => WriteTarget::EffectParameter {
                track_id: self.track_id.clone(),
                effect_id: self.target.effect_id.clone().unwrap(),
                parameter_id: self.target.parameter_id.clone().unwrap(),
            },
        }
    }
    pub fn merge(
        &self,
        mut candidate: Session,
        points: Vec<AutomationPoint>,
    ) -> Result<Session, Error> {
        let current = Self::capture(&candidate, &self.track_id, self.target.clone())?;
        if current.saved != self.saved {
            return Err(fail(
                "The leased automation target changed; review the applied prefix",
            ));
        }
        let track = track_mut(&mut candidate, &self.track_id)
            .ok_or_else(|| fail("Write track was removed"))?;
        match self.target.kind {
            RemoteAutomationWriteTargetKind::Volume => {
                track.mix_automation.insert("volume".into(), points);
            }
            RemoteAutomationWriteTargetKind::Pan => {
                track.mix_automation.insert("pan".into(), points);
            }
            RemoteAutomationWriteTargetKind::EffectAmount => {
                track
                    .realtime_effects
                    .iter_mut()
                    .find(|e| {
                        e.id.eq_ignore_ascii_case(self.target.effect_id.as_deref().unwrap())
                    })
                    .unwrap()
                    .automation = points;
            }
            RemoteAutomationWriteTargetKind::EffectParameter => {
                track
                    .realtime_effects
                    .iter_mut()
                    .find(|e| {
                        e.id.eq_ignore_ascii_case(self.target.effect_id.as_deref().unwrap())
                    })
                    .unwrap()
                    .parameter_automation
                    .insert(self.target.parameter_id.clone().unwrap(), points);
            }
        }
        Ok(candidate)
    }
}
fn track_mut<'a>(session: &'a mut Session, id: &str) -> Option<&'a mut Track> {
    session
        .tracks
        .iter_mut()
        .chain(std::iter::once(&mut session.master_track))
        .find(|t| t.id.eq_ignore_ascii_case(id))
}
#[cfg(test)]
mod tests {
    use super::*;
    use soundroom_engine::model::Effect;
    use std::collections::BTreeMap;
    const TRACK: &str = "12345678-1234-4234-8234-123456789ABC";
    const INSERT: &str = "22345678-1234-4234-8234-123456789ABC";
    fn session() -> Session {
        Session {
            tracks: vec![Track {
                id: TRACK.into(),
                ..Track::default()
            }],
            ..Session::default()
        }
    }
    fn point(frame: u64, value: f32) -> AutomationPoint {
        AutomationPoint {
            frame,
            value,
            curve: "linear".into(),
            curve_start: 0.,
            curve_end: 1.,
            extra: BTreeMap::new(),
        }
    }
    fn target(kind: RemoteAutomationWriteTargetKind) -> RemoteAutomationWriteTarget {
        RemoteAutomationWriteTarget {
            kind,
            effect_id: None,
            parameter_id: None,
        }
    }
    #[test]
    fn lane_only_merge_preserves_unrelated_authored_content_exactly() {
        let original = session();
        let source = Snapshot::capture(
            &original,
            TRACK,
            target(RemoteAutomationWriteTargetKind::Volume),
        )
        .unwrap();
        let mut current = original.clone();
        current.name = "Other author".into();
        current.tracks[0].name = "Renamed".into();
        current.tracks[0].pan = 0.3;
        current.tracks[0].extra.insert(
            "futureMetadata".into(),
            serde_json::json!({"preserve":true}),
        );
        current.tracks[0]
            .mix_automation
            .insert("pan".into(), vec![point(20, 0.8)]);
        let curve = vec![point(100, 0.2)];
        let mut expected = current.clone();
        expected.tracks[0]
            .mix_automation
            .insert("volume".into(), curve.clone());
        assert_eq!(source.merge(current, curve).unwrap(), expected);
    }
    #[test]
    fn changed_target_or_scalar_conflicts_and_duplicates_are_not_normalized() {
        let original = session();
        let source = Snapshot::capture(
            &original,
            TRACK,
            target(RemoteAutomationWriteTargetKind::Volume),
        )
        .unwrap();
        let mut changed = original.clone();
        changed.tracks[0].gain = 0.7;
        assert!(source.merge(changed, vec![point(1, 0.2)]).is_err());
        let mut duplicates = original;
        duplicates.tracks[0]
            .mix_automation
            .insert("volume".into(), vec![point(10, 0.2), point(10, 0.8)]);
        let preserved = duplicates.clone();
        assert!(Snapshot::capture(
            &duplicates,
            TRACK,
            target(RemoteAutomationWriteTargetKind::Volume)
        )
        .is_err());
        assert_eq!(duplicates, preserved);
    }
    #[test]
    fn named_parameter_merge_preserves_other_parameters_and_tracks() {
        let mut original = session();
        original.tracks[0].realtime_effects = vec![Effect {
            id: INSERT.into(),
            kind: "echo".into(),
            amount: 0.4,
            enabled: true,
            automation: vec![],
            parameters: BTreeMap::from([("mix".into(), 0.25), ("feedback".into(), 0.4)]),
            parameter_automation: BTreeMap::new(),
            extra: BTreeMap::new(),
        }];
        let target = RemoteAutomationWriteTarget {
            kind: RemoteAutomationWriteTargetKind::EffectParameter,
            effect_id: Some(INSERT.to_lowercase()),
            parameter_id: Some("mix".into()),
        };
        let source = Snapshot::capture(&original, &TRACK.to_lowercase(), target).unwrap();
        let mut current = original;
        current.tracks[0].realtime_effects[0]
            .parameters
            .insert("feedback".into(), 0.6);
        let curve = vec![point(20, 0.3)];
        let mut expected = current.clone();
        expected.tracks[0].realtime_effects[0]
            .parameter_automation
            .insert("mix".into(), curve.clone());
        assert_eq!(source.merge(current, curve).unwrap(), expected);
    }
}
