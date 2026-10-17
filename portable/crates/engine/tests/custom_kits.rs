use soundroom_engine::custom_kits::{self, KitDesign};
use soundroom_engine::drums::{Kit, Lane, Pattern, Step, Voice};
use soundroom_engine::measured_drums::Shape;
use soundroom_engine::sampled_drums;
use std::sync::atomic::AtomicBool;

#[test]
fn exported_kit_round_trips_plays_from_samples_and_deletes() {
    let root = std::env::temp_dir().join(format!("soundroom-custom-kits-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    sampled_drums::set_kit_roots(vec![root.clone()]);
    let cancel = AtomicBool::new(false);
    let id = "6F1C2A0E-1B7D-4E2A-9C55-0D8B5A3E7F21".to_string();
    let mut design = KitDesign {
        id: id.clone(),
        name: "Dry funk".into(),
        shapes: Default::default(),
    };
    design.shapes.insert(
        "snare".into(),
        Shape {
            wires: -60.,
            sustain: 0.6,
            ..Shape::default()
        },
    );
    assert!(KitDesign {
        name: " ".into(),
        ..design.clone()
    }
    .validate()
    .is_err());
    assert!(KitDesign {
        id: "../evil".into(),
        ..design.clone()
    }
    .validate()
    .is_err());
    let hits = custom_kits::export(&design, &cancel).unwrap();
    assert!(hits >= 9 * 5 * 3, "{hits}");
    assert_eq!(custom_kits::list(), vec![design.clone()]);
    assert!(sampled_drums::custom_available(&id));
    // A pattern lane naming the kit plays its exported samples.
    let lane = |custom: Option<String>| Lane {
        kit: Kit::StudioModeled,
        shape: design.shapes.get("snare").copied(),
        custom_kit: custom,
        ..Lane::default()
    };
    let mut pattern = Pattern {
        kit: Kit::StudioModeled,
        steps: (0..4)
            .map(|i| Step {
                articulation: None,
                voice: Voice::Snare,
                index: i * 4,
                velocity: 0.4 + 0.15 * i as f64,
                pan: 0.,
                probability: 1.,
            })
            .collect(),
        ..Pattern::default()
    };
    pattern.lanes.insert("snare".into(), lane(Some(id.clone())));
    let sampled = pattern.render(1, false, &cancel).unwrap();
    pattern.lanes.insert("snare".into(), lane(None));
    let generated = pattern.render(1, false, &cancel).unwrap();
    let rms =
        |x: &[f32]| (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt();
    let (a, b) = (rms(&sampled[0]), rms(&generated[0]));
    assert!(
        a > 0. && (20. * (a / b).log10()).abs() < 3.,
        "exported level matches the generator: {a} vs {b}"
    );
    custom_kits::delete(&id).unwrap();
    assert!(custom_kits::list().is_empty() && !sampled_drums::custom_available(&id));
    std::fs::remove_dir_all(root).unwrap();
}
