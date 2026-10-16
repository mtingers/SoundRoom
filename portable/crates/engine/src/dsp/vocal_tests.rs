use crate::dsp::{
    limiter::TruePeak,
    vocal::{VocalEnhancer, LATENCY},
};

const RATE: f64 = 48000.;
const NEUTRAL: [f32; 9] = [0., 0., 0., 0., 0., 0., 0., 0., 1.];

fn tone(frame: usize, hz: f64) -> f32 {
    (2. * std::f64::consts::PI * hz * frame as f64 / RATE).sin() as f32
}
fn rms(values: &[f32]) -> f64 {
    (values.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / values.len() as f64).sqrt()
}
fn level_ratio(output: &[f32], input: &[f32]) -> f64 {
    20. * (rms(output) / rms(input)).log10()
}
fn component(values: &[f32], start: usize, hz: f64) -> f64 {
    let mut real = 0.;
    let mut imaginary = 0.;
    for (i, x) in values.iter().enumerate() {
        let phase = 2. * std::f64::consts::PI * hz * (start + i) as f64 / RATE;
        real += *x as f64 * phase.cos();
        imaginary += *x as f64 * phase.sin();
    }
    (real * real + imaginary * imaginary).sqrt() * 2. / values.len() as f64
}

#[test]
fn neutral_and_zero_mix_remain_exactly_aligned() {
    let mut neutral = VocalEnhancer::default();
    neutral.configure(&NEUTRAL);
    let source: Vec<[f32; 2]> = (0..4000)
        .map(|i| {
            let x = ((i * 17 % 1024) as f32 / 1024. - 0.5) * 0.2;
            [x, -x * 0.5]
        })
        .collect();
    for i in 0..source.len() + LATENCY {
        let x = source.get(i).copied().unwrap_or([0.; 2]);
        let y = neutral.sample(x);
        assert_eq!(
            y,
            if i < LATENCY {
                [0.; 2]
            } else {
                source.get(i - LATENCY).copied().unwrap_or([0.; 2])
            }
        );
    }

    let mut dry = VocalEnhancer::default();
    let mut controls = [1., 6., 1., 1., 1., 1., 1., 12., 0.];
    dry.configure(&controls);
    for i in 0..source.len() + LATENCY {
        if i == 1000 {
            controls[7] = -12.;
            dry.configure(&controls);
        }
        let x = source.get(i).copied().unwrap_or([0.; 2]);
        let y = dry.sample(x);
        assert_eq!(
            y,
            if i < LATENCY {
                [0.; 2]
            } else {
                source.get(i - LATENCY).copied().unwrap_or([0.; 2])
            }
        );
    }
}

#[test]
fn trim_and_mix_automation_arrive_with_their_source_frame() {
    let mut effect = VocalEnhancer::default();
    let mut controls = NEUTRAL;
    effect.configure(&controls);
    let source = [0.025, -0.0125];
    let mut expected = Vec::new();
    for i in 0..3000 + LATENCY {
        controls[7] = if i < 700 {
            0.
        } else if i < 1400 {
            6.
        } else {
            -6.
        };
        controls[8] = if i < 2000 { 1. } else { 0.25 };
        effect.configure(&controls);
        let trim = 10_f64.powf(controls[7] as f64 / 20.) as f32;
        expected.push(source.map(|v| v + (v * trim - v) * controls[8]));
        let output = effect.sample(source);
        if i >= LATENCY {
            for (actual, target) in output.iter().zip(expected[i - LATENCY]) {
                assert!(
                    (*actual - target).abs() < 2e-7,
                    "frame {i}: {actual} != {target}"
                );
            }
        }
    }
}

#[test]
fn de_esser_reduces_sibilance_without_ducking_the_fundamental() {
    let mut bypass = VocalEnhancer::default();
    bypass.configure(&NEUTRAL);
    let mut effect = VocalEnhancer::default();
    let mut controls = NEUTRAL;
    controls[3] = 1.;
    effect.configure(&controls);
    let mut reference = Vec::new();
    let mut processed = Vec::new();
    for i in 0..96000 {
        let burst = i % 24000 >= 6000 && i % 24000 < 18000;
        let x = 0.08 * tone(i, 240.) + if burst { 0.22 * tone(i, 8000.) } else { 0. };
        let a = bypass.sample([x, x]);
        let b = effect.sample([x, x]);
        if (72000 + 9000 + LATENCY..72000 + 15000 + LATENCY).contains(&i) {
            reference.push(a[0]);
            processed.push(b[0]);
        }
    }
    let start = 81000;
    let ess_reduction =
        20. * (component(&processed, start, 8000.) / component(&reference, start, 8000.)).log10();
    let voice_reduction =
        20. * (component(&processed, start, 240.) / component(&reference, start, 240.)).log10();
    println!("De-ess: {ess_reduction:.3} dB at 8 kHz, {voice_reduction:.3} dB at 240 Hz");
    assert!(ess_reduction < -4., "HF attenuation {ess_reduction} dB");
    assert!(
        voice_reduction > -1.,
        "Fundamental attenuation {voice_reduction} dB"
    );
}

#[test]
fn vocal_leveler_reduces_phrase_contrast_and_keeps_stereo_image() {
    let mut effect = VocalEnhancer::default();
    let mut controls = NEUTRAL;
    controls[4] = 1.;
    effect.configure(&controls);
    let mut quiet = Vec::new();
    let mut loud = Vec::new();
    for i in 0..192000 {
        let amplitude = if i < 96000 { 0.025 } else { 0.4 };
        let x = amplitude * tone(i, 220.);
        let y = effect.sample([x, x * 0.25]);
        assert!((y[1] - y[0] * 0.25).abs() < 2e-6);
        if (72000 + LATENCY..96000 + LATENCY).contains(&i) {
            quiet.push(y[0]);
        }
        if (168000 + LATENCY..192000).contains(&i) {
            loud.push(y[0]);
        }
    }
    let contrast = 20. * (rms(&loud) / rms(&quiet)).log10();
    let original = 20. * (0.4_f64 / 0.025).log10();
    println!("Phrase contrast: {contrast:.3} dB versus {original:.3} dB input");
    assert!(
        contrast < original - 4.,
        "contrast {contrast} versus {original} dB"
    );
}

#[test]
fn leveling_does_not_raise_noise_and_cleanup_is_a_soft_expander() {
    let mut leveler = VocalEnhancer::default();
    let mut controls = NEUTRAL;
    controls[4] = 1.;
    leveler.configure(&controls);
    let mut cleanup = VocalEnhancer::default();
    controls[4] = 0.;
    controls[0] = 1.;
    cleanup.configure(&controls);
    let mut source = Vec::new();
    let mut leveled = Vec::new();
    let mut cleaned = Vec::new();
    let mut seed = 42_u32;
    for i in 0..192000 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let x = (seed as f64 / u32::MAX as f64 * 2. - 1.) as f32 * 0.0004;
        let a = leveler.sample([x, x]);
        let b = cleanup.sample([x, x]);
        if i >= 144000 {
            source.push(x);
            leveled.push(a[0]);
            cleaned.push(b[0]);
        }
    }
    let noise_gain = level_ratio(&leveled, &source);
    let cleanup_gain = rms(&cleaned) / rms(&source);
    println!("Noise leveler: {noise_gain:.3} dB; soft-expander gain: {cleanup_gain:.4}");
    assert!(noise_gain <= 0.15, "noise was boosted {noise_gain} dB");
    assert!(
        (0.03..0.7).contains(&cleanup_gain),
        "soft-expander gain {cleanup_gain}"
    );
}

#[test]
fn safety_guard_links_channels_and_limits_reconstructed_peaks() {
    let mut effect = VocalEnhancer::default();
    let mut controls = NEUTRAL;
    controls[7] = 12.;
    effect.configure(&controls);
    let mut detector = TruePeak::default();
    let mut true_peak = 0_f32;
    for i in 0..96000 {
        let x = 0.8 * tone(i, 19000.) + if i % 9001 == 0 { 1. } else { 0. };
        let y = effect.sample([x, x * -0.6]);
        assert!(y.iter().all(|v| v.is_finite() && v.abs() <= 0.944062));
        assert!((y[1] - y[0] * -0.6).abs() < 2e-6);
        true_peak = true_peak.max(detector.sample(y));
    }
    assert!(true_peak < 1., "Reconstructed output peak {true_peak}");
    println!("Safety guard reconstructed peak: {true_peak:.6}");
}

#[test]
fn cleanup_preserves_a_quiet_held_vocal_and_protects_partial_mix_peaks() {
    let mut quiet_effect = VocalEnhancer::default();
    let mut controls = NEUTRAL;
    controls[0] = 1.;
    quiet_effect.configure(&controls);
    let mut input = Vec::new();
    let mut output = Vec::new();
    for i in 0..192000 {
        let x = 0.002 * (tone(i, 220.) + tone(i, 440.) * 0.5);
        let y = quiet_effect.sample([x, x]);
        if i >= 144000 {
            input.push(x);
            output.push(y[0]);
        }
    }
    let attenuation = level_ratio(&output, &input);
    println!("Quiet held vocal cleanup: {attenuation:.3} dB");
    assert!(
        attenuation > -1.2,
        "quiet held vocal was gated: {attenuation} dB"
    );

    let mut mixed = VocalEnhancer::default();
    controls = NEUTRAL;
    controls[7] = -12.;
    controls[8] = 0.25;
    mixed.configure(&controls);
    let mut detector = TruePeak::default();
    let mut peak = 0_f32;
    for i in 0..48000 {
        let x = 3. * tone(i, 17000.);
        let y = mixed.sample([x, x]);
        assert!(y.iter().all(|v| v.abs() <= 0.944062));
        peak = peak.max(detector.sample(y));
    }
    assert!(peak < 1., "partial mix escaped output protection: {peak}");
}

#[test]
fn quiet_high_register_singing_is_preserved_while_noise_is_reduced() {
    for hz in [660., 880., 1000., 1500.] {
        for cleanup in [0.45, 1.] {
            let mut effect = VocalEnhancer::default();
            let mut controls = NEUTRAL;
            controls[0] = cleanup;
            effect.configure(&controls);
            let mut source_power = 0.;
            let mut output_power = 0.;
            for i in 0..192000 {
                let x = 0.002 * (tone(i, hz) + 0.5 * tone(i, hz * 2.) + 0.2 * tone(i, hz * 3.));
                let y = effect.sample([x, -x]);
                if i >= 144000 {
                    source_power += (x as f64).powi(2);
                    output_power += (y[0] as f64).powi(2);
                }
            }
            let attenuation = 10. * (output_power / source_power).log10();
            assert!(
                attenuation > -1.2,
                "{hz} Hz held vocal, cleanup {cleanup}: {attenuation} dB"
            );
        }
    }
    let mut noise = VocalEnhancer::default();
    let mut controls = NEUTRAL;
    controls[0] = 1.;
    noise.configure(&controls);
    let mut seed = 77_u32;
    let mut source_power = 0.;
    let mut output_power = 0.;
    for i in 0..192000 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let x = (seed as f64 / u32::MAX as f64 * 2. - 1.) as f32 * 0.0028;
        let y = noise.sample([x, -x]);
        if i >= 144000 {
            source_power += (x as f64).powi(2);
            output_power += (y[0] as f64).powi(2);
        }
    }
    let attenuation = 10. * (output_power / source_power).log10();
    println!("Matched quiet broadband noise: {attenuation:.3} dB");
    assert!(
        attenuation < -6.,
        "noise was mistaken for held singing: {attenuation} dB"
    );
}

#[test]
#[allow(clippy::excessive_precision)] // Published Rust/Swift Float checkpoint fixture.
fn deterministic_swift_parity_fixture() {
    let mut effect = VocalEnhancer::default();
    let mut controls = crate::dsp::vocal::DEFAULTS;
    effect.configure(&controls);
    let checkpoints = [
        (143, [-0.000532157, -0.000400381]),
        (144, [-0.065350316, -0.049016543]),
        (145, [-0.063091286, -0.046941914]),
        (511, [-0.060315836, -0.033522986]),
        (1023, [0.013525319, 0.018826062]),
        (1168, [-0.034652032, -0.025320491]),
        (2048, [0.015872864, 0.020928826]),
        (2192, [-0.011578474, -0.008509722]),
        (4096, [0.014452861, 0.020436611]),
        (4240, [-0.027537454, -0.020545397]),
        (6144, [0.016535927, 0.023505779]),
        (6288, [-0.010082271, -0.007295184]),
        (8191, [0.011685637, 0.016186375]),
        (8335, [0.047580063, 0.035795461]),
    ];
    for i in 0..8336 {
        if i == 1024 {
            controls[2] = 0.8;
            controls[3] = 0.9;
            effect.configure(&controls);
        }
        if i == 2048 {
            controls[4] = 0.9;
            controls[1] = 2.;
            effect.configure(&controls);
        }
        if i == 4096 {
            controls[6] = 0.7;
            controls[5] = 0.45;
            controls[7] = 1.5;
            controls[8] = 0.7;
            effect.configure(&controls);
        }
        if i == 6144 {
            controls[0] = 0.8;
            controls[8] = 1.;
            controls[7] = -2.;
            effect.configure(&controls);
        }
        let input = if i >= 8192 {
            [0.; 2]
        } else {
            let left = ((i * 17 % 1024) as f32 - 512.) / 1024. * 0.125;
            let right = left * 0.5 + ((i * 29 % 1024) as f32 - 512.) / 1024. * 0.03125;
            [left, right]
        };
        let y = effect.sample(input);
        if let Some((_, expected)) = checkpoints.iter().find(|(frame, _)| *frame == i) {
            for (actual, target) in y.iter().zip(expected) {
                assert!(
                    (actual - target).abs() < 1e-7,
                    "frame {i}: {actual} != {target}"
                );
            }
        }
    }
}

#[test]
fn invalid_inputs_cannot_poison_silence_or_later_audio() {
    let mut effect = VocalEnhancer::default();
    effect.configure(&[f32::NAN; 9]);
    for _ in 0..2000 {
        let y = effect.sample([f32::NAN, f32::INFINITY]);
        assert_eq!(y, [0.; 2]);
    }
    for i in 0..4000 {
        let y = effect.sample([tone(i, 220.) * 0.1, f32::MAX]);
        assert!(y.iter().all(|v| v.is_finite() && v.abs() < 1.));
    }
    for _ in 0..48000 {
        effect.sample([0.; 2]);
    }
    assert!(effect.sample([0.; 2]).iter().all(|v| v.abs() < 1e-12));
}

#[test]
fn cloning_is_independent_and_stream_partitioning_is_invariant() {
    let mut original = VocalEnhancer::default();
    for i in 0..1000 {
        original.sample([tone(i, 240.) * 0.2, tone(i, 480.) * 0.1]);
    }
    let mut copy = original.clone();
    let mut partitioned = original.clone();
    for i in 1000..9000 {
        let x = [tone(i, 240.) * 0.2, tone(i, 480.) * 0.1];
        assert_eq!(original.sample(x), copy.sample(x));
    }
    // A caller's block size never resets FIR, detector, or lookahead histories.
    for start in (1000..9000).step_by(127) {
        for i in start..(start + 127).min(9000) {
            partitioned.sample([tone(i, 240.) * 0.2, tone(i, 480.) * 0.1]);
        }
    }
    assert_eq!(
        original.sample([0.1, 0.05]),
        partitioned.sample([0.1, 0.05])
    );
    copy.configure(&NEUTRAL);
    for i in 0..1000 {
        copy.sample([tone(i, 400.) * 0.1; 2]);
    }
    let next = [0.2, -0.1];
    assert_eq!(original.sample(next), partitioned.sample(next));
}
