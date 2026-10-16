use soundroom_engine::{
    designer::{ControllerKind, Pattern},
    midi::{self, Content},
};
use std::path::{Path, PathBuf};
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/instruments")
        .join(name)
}
fn assert_content(actual: Content, file: &str) {
    let expected: Content = serde_json::from_slice(&std::fs::read(fixture(file)).unwrap()).unwrap();
    assert_eq!(actual.bpm, expected.bpm);
    assert_eq!(actual.beats, expected.beats);
    assert_eq!(actual.parts.len(), expected.parts.len());
    for (a, b) in actual.parts.iter().zip(&expected.parts) {
        assert_eq!(a.id, b.id);
        assert_eq!(a.name, b.name);
        assert_eq!(a.notes.len(), b.notes.len());
        for (a, b) in a.notes.iter().zip(&b.notes) {
            assert_eq!(a.pitch, b.pitch);
            assert_eq!(a.velocity, b.velocity);
            assert_eq!(a.start_beat, b.start_beat);
            assert_eq!(a.duration_beats, b.duration_beats);
            assert!(soundroom_engine::model::is_id(&a.id));
        }
        assert_eq!(a.controllers.len(), b.controllers.len());
        for (a, b) in a.controllers.iter().zip(&b.controllers) {
            assert_eq!(a.kind, b.kind);
            assert_eq!(a.beat, b.beat);
            assert_eq!(a.value, b.value);
        }
    }
}
#[test]
fn swift_midi_bytes_and_inspection_cover_articulation_controllers_and_named_parts() {
    let mut names = (0..24).map(|i| format!("preset-{i}")).collect::<Vec<_>>();
    names.extend(["sine", "triangle", "saw", "pulse"].map(|w| format!("wave-{w}")));
    names.extend((0..10).map(|i| format!("organic-{i}")));
    for name in names {
        let p: Pattern = serde_json::from_slice(
            &std::fs::read(fixture(&format!("designer-{name}.json"))).unwrap(),
        )
        .unwrap();
        let expected = std::fs::read(fixture(&format!("designer-{name}.mid"))).unwrap();
        assert_eq!(midi::encode(&p).unwrap(), expected);
        assert_content(
            midi::inspect(&expected).unwrap(),
            &format!("midi-{name}-swift.json"),
        );
    }
    let file = std::fs::read(fixture("midi-format1.mid")).unwrap();
    let content = midi::inspect(&file).unwrap();
    assert_content(content.clone(), "midi-format1-swift.json");
    let p = content.pattern(Some(16), 0., None).unwrap();
    assert_eq!(
        p.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(),
        [60, 64]
    );
    assert_eq!(p.controllers[0].kind, ControllerKind::Expression);
    assert_eq!(p.controllers[0].value, 64. / 127.);
}
fn file(tracks: Vec<Vec<u8>>, format: u16) -> Vec<u8> {
    let mut bytes = b"MThd".to_vec();
    bytes.extend(6_u32.to_be_bytes());
    bytes.extend(format.to_be_bytes());
    bytes.extend((tracks.len() as u16).to_be_bytes());
    bytes.extend(480_u16.to_be_bytes());
    for track in tracks {
        bytes.extend(b"MTrk");
        bytes.extend((track.len() as u32).to_be_bytes());
        bytes.extend(track);
    }
    bytes
}
fn variable(mut value: u32) -> Vec<u8> {
    let mut bytes = vec![(value & 127) as u8];
    value >>= 7;
    while value > 0 {
        bytes.insert(0, (value & 127) as u8 | 128);
        value >>= 7;
    }
    bytes
}
#[test]
fn long_and_dense_midi_requires_explicit_excerpt_without_silent_truncation() {
    let mut track = vec![0, 0x90, 60, 90];
    track.extend(variable(480 * 96));
    track.extend([0x80, 60, 0, 0, 0xFF, 0x2F, 0]);
    let content = midi::inspect(&file(vec![track], 0)).unwrap();
    assert_eq!(content.beats, 96.);
    assert!(content.pattern(None, 0., None).is_err());
    let p = content.pattern(None, 32., Some(16.)).unwrap();
    assert_eq!(p.notes.len(), 1);
    assert_eq!(p.notes[0].start_beat, 0.);
    assert_eq!(p.notes[0].duration_beats, 16.);
    let mut track = vec![];
    for _ in 0..300 {
        track.extend([0, 0x90, 60, 90, 96, 0x80, 60, 0]);
    }
    track.extend([0, 0xFF, 0x2F, 0]);
    let content = midi::inspect(&file(vec![track], 0)).unwrap();
    assert_eq!(content.parts[0].notes.len(), 300);
    assert!(content.pattern(None, 0., None).is_err());
    assert_eq!(content.pattern(None, 0., Some(4.)).unwrap().notes.len(), 20);
}
#[test]
fn midi_excerpt_carries_prior_controller_states_and_rebases_events() {
    let track = vec![
        0, 0xB0, 11, 64, 0, 0x90, 60, 100, 0x83, 0x60, 0xB0, 1, 127, 0x83, 0x60, 0xB0, 11, 127,
        0x87, 0x40, 0x80, 60, 0, 0, 0xFF, 0x2F, 0,
    ];
    let content = midi::inspect(&file(vec![track], 0)).unwrap();
    let p = content.pattern(None, 1.5, Some(1.)).unwrap();
    assert_eq!(p.notes[0].duration_beats, 1.);
    assert_eq!(p.controllers.len(), 3);
    assert_eq!(p.controllers.iter().filter(|c| c.beat == 0.).count(), 2);
    assert_eq!(p.controllers.last().unwrap().beat, 0.5);
    assert!(content.pattern(None, f64::NAN, Some(1.)).is_err());
    assert!(content.pattern(Some(999), 0., Some(1.)).is_err());
}
#[test]
fn malformed_midi_is_bounded_and_never_invents_noteoffs() {
    let valid = midi::encode(&Pattern::default()).unwrap();
    for size in [0, 4, 14, valid.len() - 2] {
        assert!(midi::inspect(&valid[..size]).is_err());
    }
    for track in [
        vec![0, 0x90, 60, 100, 0, 0xFF, 0x2F, 0],
        vec![0x80, 0x80, 0x80, 0x80, 0x80],
        vec![0, 0x90, 60, 100, 0, 0x80, 60, 0],
        vec![
            0, 0xFF, 0x51, 3, 7, 0xA1, 0x20, 0x83, 0x60, 0xFF, 0x51, 3, 6, 0x1A, 0x80, 0, 0xFF,
            0x2F, 0,
        ],
    ] {
        assert!(midi::inspect(&file(vec![track], 0)).is_err());
    }
    let mut smpte = file(vec![vec![0, 0xFF, 0x2F, 0]], 0);
    smpte[12] = 0xE8;
    smpte[13] = 40;
    assert!(midi::inspect(&smpte).is_err());
    assert!(midi::inspect(&vec![0; 4_194_305]).is_err());
    assert!(midi::inspect(&file(
        vec![vec![0, 0xFF, 0x2F, 0], vec![0, 0xFF, 0x2F, 0]],
        0
    ))
    .is_err());
}
