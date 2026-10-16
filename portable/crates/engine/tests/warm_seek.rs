//! A long seek warm-starts near the target; after the warm-up its output must
//! match an exact full-history render to within -60 dB (reverb insert).
use soundroom_engine::{
    model::{Clip, Effect, Session, Track},
    project::{Media, Project},
    render::{default_parameters, CheckpointCache, PreparedGraph, Renderer},
    wave::{WaveReader, WaveWriter},
};
use std::{collections::BTreeMap, sync::atomic::AtomicBool};

#[test]
fn warm_start_seek_matches_exact_history_after_the_warmup() {
    let dir = std::env::temp_dir().join(format!("soundroom-warm-seek-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tone.wav");
    let frames = 48_000 * 120;
    let mut writer = WaveWriter::create(&path, frames).unwrap();
    for block in 0..frames / 4800 {
        let audio: Vec<f32> = (0..4800)
            .map(|i| ((block * 4800 + i) as f32 * 0.031).sin() * 0.3 * (((block / 10) % 2) as f32))
            .collect();
        writer.write(&audio, &audio).unwrap();
    }
    writer.finish().unwrap();
    let info = WaveReader::open(&path).unwrap().info;
    let asset = "6B1C3E1A-0000-4000-8000-000000000001".to_string();
    let session = Session {
        tracks: vec![Track {
            id: "6B1C3E1A-0000-4000-8000-000000000002".into(),
            clips: vec![Clip {
                id: "6B1C3E1A-0000-4000-8000-000000000003".into(),
                asset_id: asset.clone(),
                name: "Tone".into(),
                start: 0,
                offset: 0,
                length: frames,
                edge_fade_frames: None,
                edge_protection_disabled: None,
                cut_fade_in_frames: None,
                cut_fade_out_frames: None,
                fade_in_frames: None,
                fade_out_frames: None,
                fade_reference_offset: None,
                fade_reference_length: None,
                render_envelope: None,
                previous_states: vec![],
                extra: BTreeMap::new(),
            }],
            realtime_effects: vec![Effect {
                id: "6B1C3E1A-0000-4000-8000-000000000004".into(),
                kind: "room".into(),
                amount: 0.5,
                enabled: true,
                automation: vec![],
                parameters: default_parameters("room"),
                parameter_automation: BTreeMap::new(),
                extra: Default::default(),
            }],
            ..Track::default()
        }],
        ..Session::default()
    };
    let project = Project {
        session,
        media: BTreeMap::from([(
            asset,
            Media {
                path: path.clone(),
                info,
            },
        )]),
    };
    let cancel = AtomicBool::new(false);
    let target = 100 * 48_000;
    let mut exact = Renderer::new(&project).unwrap();
    exact.advance_to(target, &cancel).unwrap();
    let mut graph = PreparedGraph::prepare_resident(&project, 256 * 1024 * 1024, &cancel).unwrap();
    let rendered = graph
        .seek(target, &CheckpointCache::new(0), &cancel)
        .unwrap();
    assert!(
        rendered < 30 * 48_000,
        "warm start renders only the warm-up, not {rendered} frames"
    );
    let (mut a, mut b) = (vec![0f32; 4096], vec![0f32; 4096]);
    let (mut c, mut d) = (vec![0f32; 4096], vec![0f32; 4096]);
    let mut worst = 0f32;
    for _ in 0..24 {
        exact.render(&mut a, &mut b).unwrap();
        graph.render_callback(&mut c, &mut d);
        for i in 0..4096 {
            worst = worst.max((a[i] - c[i]).abs()).max((b[i] - d[i]).abs());
        }
    }
    std::fs::remove_dir_all(&dir).ok();
    assert!(
        worst < 0.3 * 1e-3,
        "warm-start error {worst} exceeds -60 dB of the signal"
    );
}
