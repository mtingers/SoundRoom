use soundroom_engine::{
    designer::{self, ControllerKind, Instrument, Pattern, Seed, Voice},
    wave::WaveReader,
};
use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/instruments")
        .join(name)
}
fn patch(name: &str) -> Pattern {
    serde_json::from_slice(&std::fs::read(fixture(name)).unwrap()).unwrap()
}
fn compare(actual: &[Vec<f32>], file: &str) {
    let mut reader = WaveReader::open(&fixture(file)).unwrap();
    assert_eq!(reader.info.frames as usize, actual[0].len());
    assert_eq!(reader.info.channels as usize, actual.len());
    let mut maximum = 0_f32;
    for start in (0..actual[0].len()).step_by(8192) {
        let count = (actual[0].len() - start).min(8192);
        let mut left = vec![0.; count];
        let mut right = left.clone();
        reader
            .read_stereo(start as u64, &mut left, &mut right)
            .unwrap();
        for (i, (&l, &r)) in left.iter().zip(&right).enumerate() {
            maximum = maximum.max((actual[0][start + i] - l).abs());
            if actual.len() > 1 {
                maximum = maximum.max((actual[1][start + i] - r).abs());
            }
        }
    }
    println!("Swift synthesis parity {file}: maximum absolute error {maximum}");
    assert!(maximum < 0.000005, "{file}: {maximum}");
}
#[test]
fn all_designer_presets_organic_layers_waveforms_and_performance_match_swift() {
    let mut names = (0..24).map(|i| format!("preset-{i}")).collect::<Vec<_>>();
    names.extend(["sine", "triangle", "saw", "pulse"].map(|w| format!("wave-{w}")));
    names.extend((0..10).map(|i| format!("organic-{i}")));
    names.push("six-layers".into());
    names.extend(["woodenFlute", "soloCello", "vibraphone"].map(|v| format!("sustained-{v}")));
    for name in names {
        let p = patch(&format!("designer-{name}.json"));
        let audio = p.render(48000., &AtomicBool::new(false)).unwrap();
        assert!(audio.iter().flatten().all(|v| v.is_finite()));
        compare(&audio, &format!("designer-{name}-swift.wav"));
    }
    let p = patch("designer-organic-1.json");
    compare(
        &p.render_loop(&AtomicBool::new(false)).unwrap(),
        "designer-organic-1-loop-swift.wav",
    );
}
#[test]
fn live_prepared_voices_match_swift_at_every_block_size_and_short_note_release() {
    for instrument in Instrument::ALL {
        let name = serde_json::to_value(instrument)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned();
        let p = patch(&format!("voice-{name}.json"));
        for block in [1, 128, 257, 8192] {
            let mut voice = Voice::new(61, 0.83, &p, 48000., 0.25);
            voice.controller(ControllerKind::Expression, 0.78, true);
            let mut audio = vec![0.; 28000];
            for start in (0..audio.len()).step_by(block) {
                let end = audio.len().min(start + block);
                for (i, out) in audio.iter_mut().enumerate().take(end).skip(start) {
                    if i == 1400 {
                        voice.controller(ControllerKind::PitchBend, 0.72, false);
                    }
                    if i == 2400 {
                        voice.controller(ControllerKind::Modulation, 0.64, false);
                    }
                    if i == 4300 {
                        voice.note_off();
                    }
                    *out = voice.render_next();
                }
            }
            compare(&[audio], &format!("voice-{name}-swift.wav"));
        }
    }
}
#[test]
fn exact_seed_transport_roundtrip_and_finite_worker_bounds() {
    let numeric: Seed = serde_json::from_str("18446744073709551613").unwrap();
    let text: Seed = serde_json::from_str("\"18446744073709551613\"").unwrap();
    assert_eq!(numeric, text);
    assert_eq!(
        serde_json::to_string(&numeric).unwrap(),
        "18446744073709551613"
    );
    for invalid in ["\"-1\"", "\"01\"", "\"18446744073709551616\"", "1.5"] {
        assert!(serde_json::from_str::<Seed>(invalid).is_err());
    }
    let cancelled = AtomicBool::new(true);
    assert!(Pattern::default().render(48000., &cancelled).is_err());
    let p = Pattern {
        beats: 64.,
        bpm: 30.,
        ..Pattern::default()
    };
    assert!(p.render(192000., &AtomicBool::new(false)).is_err());
    let mut voice = Voice::new(i32::MAX, f64::NAN, &Pattern::default(), f64::NAN, 0.);
    assert_eq!(voice.pitch, 127);
    for _ in 0..100 {
        assert_eq!(voice.render_next(), 0.);
    }
    voice.note_off();
    voice.note_off();
    let organic = designer::Organic {
        seed: numeric,
        drift: 0.3,
        timbre: 0.6,
        breath: 0.4,
    };
    assert_eq!(organic.variation(65, 0.125), organic.variation(65, 0.125));
    assert_ne!(organic.variation(65, 0.125), organic.variation(65, 0.25));
}
#[test]
fn polyphonic_bank_stealing_sustain_release_and_stereo_match_swift() {
    use designer::{LiveInstrument, PreparedVoice};
    let patch = patch("voice-bank.json");
    let mut bank = LiveInstrument::new(3);
    let mut left = vec![0.; 24000];
    let mut right = left.clone();
    for i in 0..left.len() {
        if let Some(index) = [0, 700, 1700, 2700].iter().position(|v| *v == i) {
            bank.add_voice(PreparedVoice::new(
                Voice::new(
                    55 + index as i32 * 4,
                    0.54 + index as f64 * 0.12,
                    &patch,
                    48000.,
                    index as f64 * 0.25,
                ),
                60 + index as i32,
                0.82,
                -0.7 + index as f64 * 0.4,
                true,
            ));
        }
        match i {
            1000 => bank.controller(ControllerKind::Expression, 0.73),
            2300 => bank.controller(ControllerKind::Sustain, 1.),
            3300 => bank.note_off(61),
            4300 => bank.controller(ControllerKind::PitchBend, 0.67),
            5300 => bank.controller(ControllerKind::Sustain, 0.),
            6700 => bank.all_notes_off(),
            _ => {}
        }
        bank.render_stereo(&mut left[i..i + 1], &mut right[i..i + 1])
            .unwrap();
        assert!(bank.active_voice_count() <= 3);
    }
    compare(&[left, right], "voice-bank-swift.wav");
    assert_eq!(bank.active_voice_count(), 0);
    bank.collect_finished();
    bank.reset();
    assert!(bank.render_stereo(&mut [0.], &mut []).is_err());
}

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};
thread_local! { static TRACK_ALLOCATIONS: Cell<bool> = const { Cell::new(false) }; static ALLOCATIONS: Cell<usize> = const { Cell::new(0) }; static RETIREMENTS: Cell<usize> = const { Cell::new(0) }; }
struct AllocationProbe;
unsafe impl GlobalAlloc for AllocationProbe {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if TRACK_ALLOCATIONS.try_with(Cell::get).unwrap_or(false) {
            let _ = ALLOCATIONS.try_with(|v| v.set(v.get() + 1));
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if TRACK_ALLOCATIONS.try_with(Cell::get).unwrap_or(false) {
            let _ = RETIREMENTS.try_with(|v| v.set(v.get() + 1));
        }
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: AllocationProbe = AllocationProbe;
#[test]
fn prepared_voice_and_bank_sample_loops_allocate_and_retire_nothing() {
    use designer::{LiveInstrument, PreparedVoice};
    let mut bank = LiveInstrument::new(32);
    let patch = patch("voice-vocalEnsemble.json");
    for pitch in 36..68 {
        bank.add_voice(PreparedVoice::new(
            Voice::new(pitch, 0.7, &patch, 48000., 0.),
            pitch,
            0.3,
            0.,
            true,
        ));
    }
    let mut left = [0.; 257];
    let mut right = left;
    let mut single = Voice::new(60, 0.8, &patch, 48000., 0.);
    ALLOCATIONS.with(|v| v.set(0));
    RETIREMENTS.with(|v| v.set(0));
    TRACK_ALLOCATIONS.with(|v| v.set(true));
    for _ in 0..40 {
        bank.render_stereo(&mut left, &mut right).unwrap();
        single.render_next();
    }
    bank.all_notes_off();
    for _ in 0..150 {
        bank.render_stereo(&mut left, &mut right).unwrap();
        single.render_next();
    }
    TRACK_ALLOCATIONS.with(|v| v.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert_eq!(RETIREMENTS.with(Cell::get), 0);
    bank.collect_finished();
}

#[test]
fn delayed_voice_released_before_onset_sounds_then_finishes() {
    let p = patch("voice-acousticGuitar.json");
    let mut voice = Voice::new(60, 0.8, &p, 8000., 0.).with_delay(0.05);
    voice.note_off();
    assert!(!voice.is_finished());
    for _ in 0..400 {
        assert_eq!(voice.render_next(), 0.);
    }
    let peak = (0..4000)
        .map(|_| voice.render_next().abs())
        .fold(0_f32, f32::max);
    assert!(peak > 0.02);
    assert!(voice.is_finished());
}

#[test]
fn generated_instruments_sound_every_string_of_overlapping_strums() {
    use soundroom_engine::designer::{Instrument, Note};
    // Two six-string strums 60 ms apart: twelve voices at once. The old
    // eight-voice offline limit silently dropped the last four strings.
    let strum = |pitches: &[i32], count: usize| {
        let mut p = Pattern {
            instrument: Instrument::AcousticGuitar,
            instrument_shape: Some(Default::default()),
            bpm: 60.,
            beats: 4.,
            ..Pattern::default()
        };
        p.notes = [0., 0.06]
            .iter()
            .flat_map(|&at| {
                pitches.iter().enumerate().map(move |(k, &pitch)| Note {
                    id: String::new(),
                    pitch,
                    velocity: 0.7,
                    start_beat: at + k as f64 * 0.012,
                    duration_beats: 2.,
                    articulation: None,
                })
            })
            .take(count)
            .collect();
        let audio = p.render(48000., &AtomicBool::new(false)).unwrap();
        audio[0].iter().map(|v| (*v as f64).powi(2)).sum::<f64>()
    };
    let chord = [40, 47, 52, 55, 59, 64];
    let (eight, twelve) = (strum(&chord, 8), strum(&chord, 12));
    assert!(
        twelve > eight * 1.2,
        "later strings were dropped: {eight} vs {twelve}"
    );
}
