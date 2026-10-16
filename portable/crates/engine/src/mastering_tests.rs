//! Reference values were produced by `SoundroomCore` (Swift) from the same
//! fixtures: `tests/fixtures/mastering-*.json`.
use super::*;
use crate::{
    project::Media,
    wave::{WaveReader, WaveWriter},
};
use std::{collections::BTreeMap, f64::consts::PI};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
}
fn reference() -> Value {
    serde_json::from_str(&fixture("mastering-swift-reference.json")).unwrap()
}
fn masked(effects: &[Effect]) -> String {
    let mut value = serde_json::to_value(effects).unwrap();
    for effect in value.as_array_mut().unwrap() {
        effect["id"] = json!("ID");
    }
    let mut text = String::new();
    swift_json(&value, &mut text);
    text
}
fn ids() -> impl FnMut() -> Result<String> {
    let mut n = 0;
    move || {
        n += 1;
        Ok(format!("00000000-0000-4000-8000-{n:012}"))
    }
}

#[test]
fn swift_json_formats_numbers_strings_and_keys_like_json_encoder() {
    let value = json!({"f":[0.1_f32,1_f32,-0.0_f32,1e-5_f32,0.0001_f32,0.00012345_f32,16777216_f32,
        16777218_f32,1e7_f32,123456.7_f32,3.4e38_f32,0.65_f32,2.5e-7_f32,100000_f32,1234567_f32],
        "d":[0.1,1e15,1e16,123456789.125,1e17],
        "s":["a/b","q\"\\","\n\t\r\u{8}\u{c}\u{1}\u{1f}","é漢😀"],
        "m":{"a10":1,"a9":2,"B":3,"a":4,"b":5,"A":6,"_x":7,"Z":8,"a_":9,"aB":10,"ab":11}});
    let mut text = String::new();
    swift_json(&value, &mut text);
    // Captured from Swift `JSONEncoder` with `.sortedKeys`.
    assert_eq!(
        text,
        r#"{"d":[0.1,1000000000000000,1e+16,123456789.125,1e+17],"f":[0.1,1,-0,1e-05,0.0001,0.00012345,16777216,1.6777218e+07,10000000,123456.7,3.4e+38,0.65,2.5e-07,100000,1234567],"m":{"A":6,"B":3,"Z":8,"_x":7,"a":4,"a10":1,"a9":2,"aB":10,"a_":9,"ab":11,"b":5},"s":["a\/b","q\"\\","\n\t\r\b\f\u0001\u001f","é漢😀"]}"#
    );
}

#[test]
fn fingerprint_matches_swift_reencoding_of_tracks() {
    let reference = reference();
    let session: Session = serde_json::from_str(&fixture("mastering-fingerprint.json")).unwrap();
    let tracks = Value::Array(session.tracks.iter().map(swift_track).collect());
    let mut text = String::new();
    swift_json(&tracks, &mut text);
    assert_eq!(text, reference["fingerprintJSON"].as_str().unwrap());
    assert_eq!(
        fingerprint(&session),
        reference["fingerprint"].as_str().unwrap()
    );
    let mix: Session = serde_json::from_str(&fixture("mastering-mix.json")).unwrap();
    assert_eq!(
        fingerprint(&mix),
        reference["mixFingerprint"].as_str().unwrap()
    );
    let analysis = MasteringAnalysis {
        source_fingerprint: Some(fingerprint(&mix)),
        ..serde_json::from_value(reference["analysis"].clone()).unwrap()
    };
    assert!(analysis.matches(&mix));
    let mut changed = mix.clone();
    changed.tracks[0].gain = 0.5;
    assert!(!analysis.matches(&changed));
}

#[test]
fn proposal_is_identical_to_swift_for_the_same_analysis() {
    let reference = reference();
    let analysis: MasteringAnalysis =
        serde_json::from_value(reference["analysis"].clone()).unwrap();
    for case in reference["proposals"].as_array().unwrap() {
        let effects = analysis
            .proposal(
                case["targetLUFS"].as_f64().unwrap() as f32,
                case["intensity"].as_f64().unwrap() as f32,
                ids(),
            )
            .unwrap();
        assert_eq!(masked(&effects), case["swiftJSON"].as_str().unwrap());
    }
    let fixed = MasteringAnalysis {
        integrated_lufs: -19.,
        true_peak_db: -3.,
        bass_fraction: 0.71,
        air_fraction: 0.42,
        sections: vec![
            MasteringSection {
                start: 0,
                end: 144000,
                loudness: -24.,
                true_peak: -6.,
                resonance: 2.,
            },
            MasteringSection {
                start: 144000,
                end: 288000,
                loudness: -16.,
                true_peak: -0.5,
                resonance: 0.4,
            },
            MasteringSection {
                start: 288000,
                end: 300000,
                loudness: -70.,
                true_peak: -40.,
                resonance: 3.1,
            },
        ],
        analyzed_frames: 300000,
        ..analysis.clone()
    };
    let effects = fixed.proposal(-12., 0.8, ids()).unwrap();
    assert_eq!(
        masked(&effects),
        reference["fixedProposal"].as_str().unwrap()
    );
    let silent = MasteringAnalysis {
        true_peak_db: -80.,
        ..analysis
    };
    assert!(silent.proposal(-14., 0.65, ids()).unwrap().is_empty());
}

#[test]
fn analysis_matches_swift_measurements() {
    let directory =
        std::env::temp_dir().join(format!("soundroom-mastering-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("tone.wav");
    let n = 400_000_u64;
    let mut writer = WaveWriter::create(&path, n).unwrap();
    let mut start = 0;
    while start < n {
        let count = (n - start).min(4096);
        let (mut left, mut right) = (vec![], vec![]);
        for i in start..start + count {
            let i = i as f64;
            let ramp = 0.5 + 0.5 * i / 400000.;
            left.push(
                (ramp
                    * (0.3 * (2. * PI * 110. * i / 48000.).sin()
                        + 0.05 * (2. * PI * 7000. * i / 48000.).sin())) as f32,
            );
            right.push(
                (ramp
                    * (0.25 * (2. * PI * 220. * i / 48000. + 0.3).sin()
                        + 0.05 * (2. * PI * 3000. * i / 48000.).sin())) as f32,
            );
        }
        writer.write(&left, &right).unwrap();
        start += count;
    }
    writer.finish().unwrap();
    let info = WaveReader::open(&path).unwrap().info;
    let project = Project {
        session: serde_json::from_str(&fixture("mastering-mix.json")).unwrap(),
        media: BTreeMap::from([(
            "33333333-3333-4333-8333-333333333333".into(),
            Media { path, info },
        )]),
    };
    let mut reports = 0;
    let analysis = analyze(&project, &AtomicBool::new(false), |_, _| reports += 1).unwrap();
    assert!(analyze(&project, &AtomicBool::new(true), |_, _| {}).is_err());
    std::fs::remove_dir_all(&directory).unwrap();
    assert!(reports > 0);
    let swift: MasteringAnalysis = serde_json::from_value(reference()["analysis"].clone()).unwrap();
    assert_eq!(analysis.analyzed_frames, swift.analyzed_frames);
    assert_eq!(analysis.source_fingerprint, swift.source_fingerprint);
    let close = |a: f32, b: f32, tolerance: f32, label: &str| {
        assert!((a - b).abs() <= tolerance, "{label}: rust {a} swift {b}");
    };
    close(
        analysis.integrated_lufs,
        swift.integrated_lufs,
        1e-3,
        "integrated",
    );
    close(analysis.true_peak_db, swift.true_peak_db, 1e-3, "true peak");
    close(analysis.crest_db, swift.crest_db, 1e-3, "crest");
    close(analysis.bass_fraction, swift.bass_fraction, 1e-5, "bass");
    close(analysis.air_fraction, swift.air_fraction, 1e-5, "air");
    assert_eq!(analysis.sections.len(), swift.sections.len());
    for (a, b) in analysis.sections.iter().zip(&swift.sections) {
        assert_eq!((a.start, a.end), (b.start, b.end));
        close(a.loudness, b.loudness, 1e-3, "section loudness");
        close(a.true_peak, b.true_peak, 1e-3, "section peak");
        // Swift uses vDSP; the portable radix-2 transform may differ in final rounding.
        close(a.resonance, b.resonance, 1e-3, "section resonance");
    }
}
