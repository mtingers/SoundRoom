use soundroom_engine::designer::{Instrument, Note, Pattern};
use soundroom_engine::{sampled_drums, sampled_instruments};
use std::sync::atomic::AtomicBool;

fn write_mono16(path: &std::path::Path, data: &[f32]) {
    let mut out = Vec::new();
    let bytes = data.len() * 2;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + bytes as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&48_000u32.to_le_bytes());
    out.extend_from_slice(&96_000u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(bytes as u32).to_le_bytes());
    for v in data {
        out.extend_from_slice(&((v * 32767.) as i16).to_le_bytes());
    }
    std::fs::write(path, out).unwrap();
}

#[test]
fn recorded_instrument_plays_at_pitch_loops_while_held_and_releases() {
    let root = std::env::temp_dir().join(format!("soundroom-instruments-{}", std::process::id()));
    let dir = root.join("instruments").join("soloCello");
    std::fs::create_dir_all(&dir).unwrap();
    // One second of middle C with a whole-cycle loop over its second half.
    let hz = 261.6256;
    let data: Vec<f32> = (0..48_000)
        .map(|i| (0.8 * (2. * std::f64::consts::PI * hz * i as f64 / 48_000.).sin()) as f32)
        .collect();
    write_mono16(&dir.join("000.wav"), &data);
    let period = 48_000. / hz;
    let (start, end) = (24_000usize, 24_000 + (period * 60.).round() as usize);
    std::fs::write(dir.join("manifest.json"), format!(r#"{{"version":1,"instrument":"soloCello","release":0.2,"regions":[{{"file":"000.wav","lokey":0,"hikey":127,"centre":60,"lovel":1,"hivel":127,"robin":0,"tune":0,"gain":1.0,"loopStart":{start},"loopEnd":{end}}}]}}"#)).unwrap();
    sampled_drums::set_kit_roots(vec![root.clone()]);
    assert!(sampled_instruments::available(Instrument::SoloCello));
    assert!(!sampled_instruments::available(Instrument::Kalimba));
    let pattern = Pattern {
        instrument: Instrument::SoloCello,
        bpm: 60.,
        beats: 4.,
        notes: vec![Note {
            id: String::new(),
            pitch: 72,
            velocity: 0.8,
            start_beat: 0.,
            duration_beats: 3.,
            articulation: None,
        }],
        ..Pattern::default()
    };
    let audio = pattern.render(48_000., &AtomicBool::new(false)).unwrap();
    let x = &audio[0];
    // Held three seconds although the recording is one: the loop sustains it.
    let level = |a: usize, b: usize| {
        (x[a..b].iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / (b - a) as f64).sqrt()
    };
    assert!(
        level(2 * 48_000, 2 * 48_000 + 9_600) > 0.5 * level(9_600, 19_200),
        "sustained by the loop"
    );
    // An octave up: about 523 Hz (rising zero crossings over half a second).
    let crossings = x[48_000..72_000]
        .windows(2)
        .filter(|w| w[0] <= 0. && w[1] > 0.)
        .count() as f64
        * 2.;
    assert!((crossings - 2. * hz).abs() < 6., "{crossings} Hz");
    // Released after beat 3, silent well after the 0.2 s release.
    assert!(level(3 * 48_000 + 14_400, 3 * 48_000 + 19_200) < 1e-3);
    assert!(x.iter().all(|v| v.is_finite()));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn designed_instruments_generate_from_measurements_and_follow_the_shape() {
    use soundroom_engine::measured_instruments::{self, InstrumentShape};
    let cancel = AtomicBool::new(false);
    let render = |instrument, shape: Option<InstrumentShape>| {
        let pattern = Pattern {
            instrument,
            bpm: 60.,
            beats: 3.,
            instrument_shape: shape,
            notes: vec![Note {
                id: String::new(),
                pitch: 57,
                velocity: 0.7,
                start_beat: 0.,
                duration_beats: 1.5,
                articulation: None,
            }],
            ..Pattern::default()
        };
        pattern.render(48_000., &cancel).unwrap().remove(0)
    };
    for instrument in Instrument::ALL
        .into_iter()
        .filter(|i| measured_instruments::available(*i))
    {
        let plain = render(instrument, Some(InstrumentShape::default()));
        let peak = plain.iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(
            peak > 0.02 && peak < 1.5 && plain.iter().all(|v| v.is_finite()),
            "{instrument:?} level {peak}"
        );
        // Autocorrelation period over 0.3–0.5 s: 220 Hz (A3) for harmonic
        // instruments (bars and tines are inharmonic).
        if !matches!(
            instrument,
            Instrument::Vibraphone | Instrument::Marimba | Instrument::Kalimba
        ) {
            let x = &plain[14_400..24_000];
            let r = |lag: usize| {
                (0..x.len() - lag)
                    .map(|i| (x[i] * x[i + lag]) as f64)
                    .sum::<f64>()
            };
            let lag = (100..400).max_by(|a, b| r(*a).total_cmp(&r(*b))).unwrap();
            let hz = 48_000. / lag as f64;
            assert!((hz / 220. - 1.).abs() < 0.04, "{instrument:?} {hz} Hz");
        }
        let hf = |x: &[f32]| {
            x.windows(2)
                .map(|w| ((w[1] - w[0]) as f64).powi(2))
                .sum::<f64>()
                / x.iter()
                    .map(|v| (*v as f64).powi(2))
                    .sum::<f64>()
                    .max(1e-12)
        };
        let bright = render(
            instrument,
            Some(InstrumentShape {
                brightness: 1.,
                ..InstrumentShape::default()
            }),
        );
        assert!(hf(&bright) > hf(&plain), "{instrument:?} brightness");
        // Released after 1.5 s and silent by the end.
        let tail = &plain[plain.len() - 2_400..];
        assert!(
            tail.iter().all(|v| v.abs() < 0.02),
            "{instrument:?} releases"
        );
    }
}

#[test]
fn designed_instruments_render_phrases_without_clicks() {
    use soundroom_engine::measured_instruments::{self, InstrumentShape};
    let cancel = AtomicBool::new(false);
    let note = |pitch, start: f64, length: f64, velocity| Note {
        id: String::new(),
        pitch,
        velocity,
        start_beat: start,
        duration_beats: length,
        articulation: None,
    };
    for instrument in Instrument::ALL
        .into_iter()
        .filter(|i| measured_instruments::available(*i))
    {
        let mut notes: Vec<Note> = [0, 4, 7, 12, 7, 4]
            .iter()
            .enumerate()
            .map(|(i, d)| note(57 + d, i as f64 * 0.5, 0.45, 0.5 + 0.07 * i as f64))
            .collect();
        notes.extend([0, 4, 7].iter().map(|d| note(57 + d, 3.5, 3.5, 0.8)));
        let p = Pattern {
            notes,
            instrument,
            bpm: 96.,
            beats: 8.,
            instrument_shape: Some(InstrumentShape::default()),
            ..Pattern::default()
        };
        let x = p.render(48_000., &cancel).unwrap().remove(0);
        let d: Vec<f32> = x.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
        for i in 240..d.len().saturating_sub(240) {
            let local = d[i - 240..i + 240].iter().map(|v| v * v).sum::<f32>() / 480.;
            assert!(
                !(d[i] > 0.01 && d[i] * d[i] > 40. * local.max(1e-12)),
                "{instrument:?} click at {:.3}s",
                i as f64 / 48_000.
            );
        }
    }
}
