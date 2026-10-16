use crate::{model::Session, Error, Result};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrackSend {
    #[serde(rename = "trackID")]
    pub track_id: String,
    pub level: f32,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrackRouting {
    pub role: String,
    #[serde(default, rename = "outputID")]
    pub output_id: Option<String>,
    pub sends: Vec<TrackSend>,
    #[serde(default, rename = "sidechainSourceID")]
    pub sidechain_source_id: Option<String>,
}
#[derive(Clone, Debug)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub gain: f32,
    pub detector: bool,
}
#[derive(Clone, Debug)]
pub struct Plan {
    pub order: Vec<usize>,
    pub edges: Vec<Edge>,
    pub active: bool,
    pub input_latency: Vec<usize>,
    pub output_latency: Vec<usize>,
    pub delay_frames: usize,
}
impl Plan {
    pub fn new(s: &Session) -> Result<Self> {
        let n = s.tracks.len();
        if n > crate::model::MAX_TRACKS {
            return Err(Error::Invalid("Too many routed tracks".into()));
        }
        let indices = s
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.id.to_ascii_uppercase(), i))
            .collect::<std::collections::BTreeMap<_, _>>();
        let index = |id: &str| {
            indices
                .get(&id.to_ascii_uppercase())
                .copied()
                .ok_or_else(|| Error::Invalid("Routing refers to a missing track".into()))
        };
        let mut edges = vec![];
        if s.master_track.routing.is_some() {
            return Err(Error::Invalid("Master cannot route upstream".into()));
        }
        for (i, t) in s.tracks.iter().enumerate() {
            let r = t.routing.as_ref();
            if let Some(r) = r {
                if !["audio", "bus", "return"].contains(&r.role.as_str()) || r.sends.len() > 8 {
                    return Err(Error::Invalid("Invalid track routing".into()));
                }
                if r.role != "audio"
                    && (!t.clips.is_empty() || !t.recording_takes.is_empty() || t.armed)
                {
                    return Err(Error::Invalid(
                        "Bus and return tracks cannot contain clips or record".into(),
                    ));
                }
            }
            let to = r
                .and_then(|r| r.output_id.as_deref())
                .map(index)
                .transpose()?
                .unwrap_or(n);
            if to < n
                && s.tracks[to]
                    .routing
                    .as_ref()
                    .map_or(true, |r| r.role != "bus")
            {
                return Err(Error::Invalid(
                    "Track outputs must go to a group bus or Master".into(),
                ));
            }
            edges.push(Edge {
                from: i,
                to,
                gain: 1.,
                detector: false,
            });
            if let Some(r) = r {
                let mut seen = std::collections::BTreeSet::new();
                for send in &r.sends {
                    let target = index(&send.track_id)?;
                    if !seen.insert(target)
                        || !send.level.is_finite()
                        || !(0. ..=2.).contains(&send.level)
                        || s.tracks[target]
                            .routing
                            .as_ref()
                            .map_or(true, |r| r.role != "return")
                    {
                        return Err(Error::Invalid("Invalid post-fader send".into()));
                    }
                    edges.push(Edge {
                        from: i,
                        to: target,
                        gain: send.level,
                        detector: false,
                    });
                }
                if let Some(source) = &r.sidechain_source_id {
                    edges.push(Edge {
                        from: index(source)?,
                        to: i,
                        gain: 1.,
                        detector: true,
                    });
                }
            }
        }
        let mut indegree = vec![0; n + 1];
        let mut outgoing = vec![vec![]; n + 1];
        for e in &edges {
            indegree[e.to] += 1;
            outgoing[e.from].push(e.to);
        }
        let mut order = vec![];
        for (i, degree) in indegree.iter().enumerate() {
            if *degree == 0 {
                order.push(i);
            }
        }
        let mut cursor = 0;
        while cursor < order.len() {
            let i = order[cursor];
            cursor += 1;
            for &to in &outgoing[i] {
                indegree[to] -= 1;
                if indegree[to] == 0 {
                    order.push(to);
                }
            }
        }
        if order.len() != n + 1 {
            return Err(Error::Invalid(
                "Routing would create an audio or sidechain feedback loop".into(),
            ));
        }
        let active = s.tracks.iter().any(|t| t.routing.is_some());
        let mut input_latency = vec![0; n + 1];
        let mut output_latency = vec![0; n + 1];
        for &i in &order {
            let track = if i < n { &s.tracks[i] } else { &s.master_track };
            output_latency[i] = input_latency[i]
                + track
                    .realtime_effects
                    .iter()
                    .map(crate::render::effect_latency)
                    .sum::<usize>();
            for &to in &outgoing[i] {
                input_latency[to] = input_latency[to].max(output_latency[i]);
            }
        }
        let mut delay_frames = input_latency[..n].iter().sum::<usize>();
        for e in &edges {
            let delay = input_latency[e.to] - output_latency[e.from];
            if e.detector {
                let mut prefix = 0;
                for p in &s.tracks[e.to].realtime_effects {
                    if p.kind == "compressor" {
                        delay_frames += delay + prefix;
                    }
                    prefix += crate::render::effect_latency(p);
                }
            } else {
                delay_frames += delay;
            }
        }
        if active && (output_latency[n] > 32768 || delay_frames > 2_000_000) {
            return Err(Error::Invalid(
                "Routed latency or delay memory budget exceeded".into(),
            ));
        }
        Ok(Self {
            order,
            edges,
            active,
            input_latency,
            output_latency,
            delay_frames,
        })
    }
    /// Solo includes a selected bus's inputs and all downstream audio destinations.
    /// Detector-only dependencies never make a track audible.
    pub fn audible(&self, s: &Session) -> Vec<bool> {
        let mut on: Vec<bool> = s
            .tracks
            .iter()
            .map(|t| t.solo)
            .chain(std::iter::once(false))
            .collect();
        if !on.iter().any(|v| *v) {
            return vec![true; on.len()];
        }
        let solo = on.clone();
        for &i in self.order.iter().rev() {
            if on[i]
                && (i == s.tracks.len()
                    || s.tracks[i]
                        .routing
                        .as_ref()
                        .is_some_and(|r| r.role != "audio"))
            {
                for e in self.edges.iter().filter(|e| e.to == i && !e.detector) {
                    on[e.from] = true;
                }
            }
        }
        for &i in &self.order {
            if on[i] || solo[i] {
                for e in self.edges.iter().filter(|e| e.from == i && !e.detector) {
                    on[e.to] = true;
                }
            }
        }
        on
    }
}

impl Session {
    /// Monitor one track before its output bus/returns, while retaining the full
    /// upstream bus and sidechain graph. Other Master outputs go to a silent sink.
    pub fn isolated_track(&self, id: &str) -> Result<Self> {
        let target = self
            .tracks
            .iter()
            .position(|t| t.id.eq_ignore_ascii_case(id))
            .ok_or_else(|| Error::Invalid("Export track is missing".into()))?;
        let mut s = self.clone();
        s.master_track = Session::default().master_track;
        if !self.tracks.iter().any(|t| t.routing.is_some()) {
            s.tracks = vec![self.tracks[target].clone()];
            s.tracks[0].muted = false;
            s.tracks[0].solo = false;
            return Ok(s);
        }
        let mut number = 2;
        let sink_id = loop {
            let id = format!("E3E58100-5125-4E6F-B985-{:012}", number);
            number += 1;
            if !self.tracks.iter().any(|t| t.id.eq_ignore_ascii_case(&id)) {
                break id;
            }
        };
        for (i, t) in s.tracks.iter_mut().enumerate() {
            t.solo = false;
            let r = t.routing.get_or_insert(TrackRouting {
                role: "audio".into(),
                output_id: None,
                sends: vec![],
                sidechain_source_id: None,
            });
            if i == target {
                t.muted = false;
                r.output_id = None;
                r.sends.clear();
            } else if r.output_id.is_none() {
                r.output_id = Some(sink_id.clone());
            }
        }
        s.tracks.push(crate::model::Track {
            id: sink_id,
            name: "Render sink".into(),
            muted: true,
            routing: Some(TrackRouting {
                role: "bus".into(),
                output_id: None,
                sends: vec![],
                sidechain_source_id: None,
            }),
            ..Default::default()
        });
        let selected = s.tracks.remove(target);
        s.tracks.insert(0, selected);
        s.validate()?;
        Ok(s)
    }
}
