//! Correction quality on a sung-vowel model: glottal pulses with vibrato,
//! jitter, shimmer and breath through vowel formants. Garble shows up as lost
//! periodicity (harmonic-to-noise ratio) and as wet/dry flicker.
use soundroom_engine::pitch::{self, Controls, Regions};
use soundroom_engine::pitch_targets::{seed, Preset};
use std::sync::atomic::AtomicBool;

const RATE: f64 = 48000.;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64 * 2. - 1.
    }
}

/// Two-pole resonator (formant).
struct Resonator {
    a1: f64,
    a2: f64,
    gain: f64,
    y: [f64; 2],
}
impl Resonator {
    fn new(frequency: f64, bandwidth: f64) -> Self {
        let r = (-std::f64::consts::PI * bandwidth / RATE).exp();
        let theta = 2. * std::f64::consts::PI * frequency / RATE;
        Self {
            a1: 2. * r * theta.cos(),
            a2: -r * r,
            gain: 1. - r,
            y: [0.; 2],
        }
    }
    fn process(&mut self, x: f64) -> f64 {
        let y = self.gain * x + self.a1 * self.y[0] + self.a2 * self.y[1];
        self.y = [y, self.y[0]];
        y
    }
}

pub struct Voice {
    pub samples: Vec<f32>,
    /// True fundamental per sample (0 while unvoiced).
    pub f0: Vec<f64>,
}

/// `cents` offsets the sung pitch from `note`; `breath` is the aspiration level.
pub fn sing(
    note: f64,
    cents: f64,
    seconds: f64,
    breath: f64,
    gaps: bool,
    vowel: [f64; 4],
    seed: u64,
) -> Voice {
    sing_rough(note, cents, seconds, breath, gaps, vowel, 0., seed)
}

/// `rasp` (0…1) weakens every other glottal pulse and drifts its timing, the
/// period doubling heard in rough or raspy voices.
#[allow(clippy::too_many_arguments)]
pub fn sing_rough(
    note: f64,
    cents: f64,
    seconds: f64,
    breath: f64,
    gaps: bool,
    vowel: [f64; 4],
    rasp: f64,
    seed: u64,
) -> Voice {
    let mut rng = Rng(seed | 1);
    let base = 440. * 2_f64.powf((note - 69. + cents / 100.) / 12.);
    let mut formants = [
        Resonator::new(vowel[0], 60. + vowel[0] * 0.04),
        Resonator::new(vowel[1], 80. + vowel[1] * 0.03),
        Resonator::new(vowel[2], 160.),
        Resonator::new(vowel[3], 250.),
    ];
    let levels = [1., 0.55, 0.28, 0.12];
    let frames = (seconds * RATE) as usize;
    let mut samples = Vec::with_capacity(frames);
    let mut f0 = Vec::with_capacity(frames);
    let mut phase = 0.;
    let mut jitter = 0.;
    let mut shimmer = 1.;
    let mut drift = 0.;
    let mut odd = false;
    for n in 0..frames {
        let t = n as f64 / RATE;
        // Consonant gaps: 60 ms of noise every 700 ms.
        let unvoiced = gaps && (t % 0.7) > 0.64;
        drift += 0.00002 * rng.next();
        drift *= 0.9999;
        let vibrato = 0.3 * (2. * std::f64::consts::PI * 5.4 * t).sin() * (t / 0.6).min(1.);
        let frequency = base * 2_f64.powf((vibrato + drift * 100.) / 12. / 100. * 100.);
        let frequency = frequency * (1. + jitter);
        let mut excitation = 0.;
        if !unvoiced {
            phase += frequency / RATE;
            if phase >= 1. {
                phase -= 1.;
                odd = !odd;
                jitter = 0.004 * rng.next() + if odd { 0.06 * rasp } else { -0.06 * rasp };
                shimmer = (1. + 0.06 * rng.next()) * if odd { 1. - 0.7 * rasp } else { 1. };
            }
            // Rosenberg glottal flow derivative: open 60%, sharp closure.
            excitation = if phase < 0.6 {
                (std::f64::consts::PI * phase / 0.6).sin() * 0.6
            } else if phase < 0.68 {
                -((phase - 0.6) / 0.08) * 1.2
            } else {
                0.
            } * shimmer;
        }
        let noise = rng.next() * if unvoiced { 0.25 } else { breath };
        let source = excitation + noise;
        let mut y = 0.;
        for (formant, level) in formants.iter_mut().zip(levels) {
            y += formant.process(source) * level;
        }
        samples.push((y * 1.2) as f32);
        f0.push(if unvoiced { 0. } else { frequency });
    }
    Voice { samples, f0 }
}

/// Normalized autocorrelation at the best lag near `period`, over one window.
fn periodicity(signal: &[f32], start: usize, length: usize, period: f64) -> f64 {
    let lo = (period * 0.85) as usize;
    let hi = (period * 1.15).ceil() as usize;
    let mut best = 0_f64;
    for lag in lo..=hi {
        if start + length + lag > signal.len() {
            break;
        }
        let (mut xy, mut xx, mut yy) = (0., 0., 0.);
        for i in start..start + length {
            let x = signal[i] as f64;
            let y = signal[i + lag] as f64;
            xy += x * y;
            xx += x * x;
            yy += y * y;
        }
        if xx > 0. && yy > 0. {
            best = best.max(xy / (xx * yy).sqrt());
        }
    }
    best
}

pub struct Quality {
    pub hnr_db: f64,
    pub worst_hnr_db: f64,
    pub flicker: usize,
}

/// Mean and 5th-percentile HNR over voiced windows away from gaps, and the
/// number of wet-blend dropouts while the voice is steadily voiced.
pub fn measure(output: &[f32], blends: &[f32], f0: &[f64], target: f64, delay: usize) -> Quality {
    let length = 1536;
    let mut values = vec![];
    let mut start = 9600;
    while start + length + 2400 < f0.len() {
        let steady = f0[start..start + length + 1200].iter().all(|&f| f > 0.);
        if steady {
            let r = periodicity(output, start + delay, length, RATE / target).min(0.999_999);
            values.push(10. * (r / (1. - r)).log10());
        }
        start += 480;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut flicker = 0;
    let mut previous = true;
    for (i, &blend) in blends.iter().enumerate().skip(9600 + delay) {
        let source = i - delay;
        if source + 2400 >= f0.len() {
            break;
        }
        let steady = f0[source.saturating_sub(2400)..source + 2400]
            .iter()
            .all(|&f| f > 0.);
        let wet = blend > 0.5;
        if steady && previous && !wet {
            flicker += 1;
        }
        previous = wet || !steady;
    }
    Quality {
        hnr_db: values.iter().sum::<f64>() / values.len().max(1) as f64,
        worst_hnr_db: values.get(values.len() / 20).copied().unwrap_or(0.),
        flicker,
    }
}

pub fn correct(voice: &Voice, controls: Controls) -> (Vec<f32>, Vec<f32>) {
    let mut state = pitch::Pitch::default();
    let mut output = Vec::with_capacity(voice.samples.len() + pitch::LATENCY);
    let mut blends = Vec::with_capacity(output.capacity());
    for i in 0..voice.samples.len() + pitch::LATENCY {
        let x = voice.samples.get(i).copied().unwrap_or(0.);
        let [l, _] = state.sample([x, x], controls);
        output.push(l);
        blends.push(state.meter.wet_blend);
    }
    (output, blends)
}

/// The editor path: analyze like the server, seed targets with `preset`, and
/// play them as a pitch-edits-only insert. Returns output, blends and targets.
pub fn correct_targets(voice: &Voice, preset: Preset) -> (Vec<f32>, Vec<f32>, Vec<Option<f32>>) {
    let (output, blends, targets, _) = correct_targets_traced(voice, preset);
    (output, blends, targets)
}
/// Also returns the correction (cents) the shifter applied at each output frame.
pub fn correct_targets_traced(
    voice: &Voice,
    preset: Preset,
) -> (Vec<f32>, Vec<f32>, Vec<Option<f32>>, Vec<f32>) {
    let contour = pitch::analyze(
        &voice.samples,
        &voice.samples,
        65_536,
        &AtomicBool::new(false),
    )
    .unwrap();
    let regions = seed(
        &contour,
        (0, voice.samples.len() as u64),
        440.,
        preset,
        5.,
        4095,
    );
    let regions = Regions::new(&regions).unwrap();
    let mut state = pitch::Pitch::default();
    let mut output = vec![];
    let mut blends = vec![];
    let mut targets = vec![];
    let mut applied = vec![];
    for i in 0..voice.samples.len() + pitch::LATENCY {
        let x = voice.samples.get(i).copied().unwrap_or(0.);
        let mut controls = Controls::default();
        let sampled = regions.sample(i as i64);
        if let Some((note, blend)) = sampled {
            controls.manual_target = note;
            controls.curve = true;
            controls.mix *= blend;
        } else {
            controls.mix = 0.;
        }
        targets.push(sampled.map(|(note, _)| note));
        let [l, _] = state.sample([x, x], controls);
        output.push(l);
        blends.push(state.meter.wet_blend);
        applied.push(state.meter.correction_cents * state.meter.wet_blend);
    }
    (output, blends, targets, applied)
}
/// Share of wet output time spent shifting by more than 100 and 600 cents.
pub fn large_shifts(applied: &[f32]) -> (f64, f64) {
    let wet: Vec<f32> = applied.iter().copied().filter(|c| *c != 0.).collect();
    let over = |limit: f32| {
        wet.iter().filter(|c| c.abs() > limit).count() as f64 / wet.len().max(1) as f64 * 100.
    };
    (over(100.), over(600.))
}

/// RMS and 95th-percentile distance (cents) between the output's measured pitch
/// and the requested target, over steadily voiced, targeted frames.
pub fn accuracy(output: &[f32], targets: &[Option<f32>], f0: &[f64]) -> (f64, f64) {
    let delayed = &output[pitch::LATENCY..];
    let contour = pitch::analyze(delayed, delayed, 65_536, &AtomicBool::new(false)).unwrap();
    let mut errors = vec![];
    for point in contour.iter().filter(|p| p.is_voiced()) {
        let frame = point.frame as usize;
        if frame < 9600
            || frame + 2400 >= f0.len()
            || !f0[frame - 2400..frame + 2400].iter().all(|&f| f > 0.)
        {
            continue;
        }
        let Some(target) = targets[frame] else {
            continue;
        };
        let measured = 69. + 12. * (point.frequency as f64 / 440.).log2();
        // Heard error: the same note an octave away is not a wrong note.
        let cents = (measured - target as f64) * 100.;
        errors.push((cents - (cents / 1200.).round() * 1200.).abs());
    }
    errors.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let rms = (errors.iter().map(|e| e * e).sum::<f64>() / errors.len().max(1) as f64).sqrt();
    (
        rms,
        errors.get(errors.len() * 95 / 100).copied().unwrap_or(0.),
    )
}

pub const AH: [f64; 4] = [730., 1090., 2440., 3400.];
pub const EE: [f64; 4] = [270., 2290., 3010., 3500.];
pub const OO: [f64; 4] = [300., 870., 2240., 3400.];

#[test]
#[ignore = "prints the quality report; run with --ignored --nocapture"]
fn report() {
    println!("voice                          | in HNR | auto HNR | Natural HNR  err | Tight HNR  err  p95");
    for (name, vowel) in [("ah", AH), ("ee", EE), ("oo", OO)] {
        for (note, cents, breath, gaps) in [
            (45., -35., 0.03, true),
            (50., 40., 0.06, false),
            (57., -35., 0.03, true),
            (64., 25., 0.08, false),
            (69., -20., 0.05, true),
        ] {
            let voice = sing(note, cents, 4., breath, gaps, vowel, 7 + note as u64);
            let target = 440. * 2_f64.powf((note - 69.) / 12.);
            let ones = vec![1.; voice.samples.len()];
            let input = measure(
                &voice.samples,
                &ones,
                &voice.f0,
                target * 2_f64.powf(cents / 1200.),
                0,
            );
            let (output, blends) = correct(&voice, Controls::default());
            let auto = measure(&output, &blends, &voice.f0, target, pitch::LATENCY);
            let (output, blends, targets) = correct_targets(&voice, Preset::Natural);
            let natural = measure(&output, &blends, &voice.f0, target, pitch::LATENCY);
            let (natural_rms, _) = accuracy(&output, &targets, &voice.f0);
            let (output, blends, targets) = correct_targets(&voice, Preset::Tight);
            let tight = measure(&output, &blends, &voice.f0, target, pitch::LATENCY);
            let (tight_rms, tight_p95) = accuracy(&output, &targets, &voice.f0);
            println!(
                "{name} note {note:2} {cents:+3}c breath {breath:.2} | {:5.1}  | {:5.1}    | {:5.1} {:5.1}c | {:5.1} {:5.1}c {:5.1}c",
                input.hnr_db, auto.hnr_db, natural.hnr_db, natural_rms, tight.hnr_db, tight_rms, tight_p95
            );
        }
    }
}

/// Real takes named in SOUNDROOM_VOCALS (colon-separated WAV paths): detector
/// stability, and correction quality with periods taken from the detector.
/// Mean HNR of a corrected output at its own measured pitch, over windows the
/// detector holds steadily voiced (so it scores texture, not the target).
fn output_hnr(output: &[f32], frames: usize) -> f64 {
    let delayed = &output[pitch::LATENCY..pitch::LATENCY + frames];
    let contour = pitch::analyze(delayed, delayed, 65_536, &AtomicBool::new(false)).unwrap();
    let mut values = vec![];
    for i in (10..contour.len().saturating_sub(10)).step_by(4) {
        if !contour[i - 10..i + 10].iter().all(|p| p.is_voiced()) {
            continue;
        }
        let start = contour[i].frame as usize;
        if start + 1536 + 2400 >= delayed.len() {
            break;
        }
        let r =
            periodicity(delayed, start, 1536, 48_000. / contour[i].frequency as f64).min(0.999_999);
        values.push(10. * (r / (1. - r)).log10());
    }
    values.iter().sum::<f64>() / values.len().max(1) as f64
}

#[test]
#[ignore = "needs SOUNDROOM_VOCALS; run with --ignored --nocapture"]
fn real_takes() {
    let Ok(paths) = std::env::var("SOUNDROOM_VOCALS") else {
        return;
    };
    for path in paths.split(':') {
        let mut reader =
            soundroom_engine::wave::WaveReader::open(std::path::Path::new(path)).unwrap();
        let frames = reader.info.frames.min(48_000 * 90) as usize;
        let mut left = vec![0.; frames];
        let mut right = vec![0.; frames];
        for (index, (l, r)) in left
            .chunks_mut(8192)
            .zip(right.chunks_mut(8192))
            .enumerate()
        {
            reader.read_stereo((index * 8192) as u64, l, r).unwrap();
        }
        let mono: Vec<f32> = left
            .iter()
            .zip(&right)
            .map(|(l, r)| (l + r) * 0.5)
            .collect();
        let contour = pitch::analyze(&mono, &mono, 65_536, &AtomicBool::new(false)).unwrap();
        let voiced = contour.iter().filter(|p| p.is_voiced()).count();
        let mut dropouts = 0;
        let mut octaves = 0;
        for window in contour.windows(3) {
            if window[0].is_voiced() && !window[1].is_voiced() && window[2].is_voiced() {
                dropouts += 1;
            }
            if window[0].is_voiced() && window[1].is_voiced() {
                let ratio = window[1].frequency / window[0].frequency;
                if !(0.7..1.4).contains(&ratio) {
                    octaves += 1;
                }
            }
        }
        // A pseudo ground truth for the steady-voice gate: voiced for ±50 ms.
        let mut f0 = vec![0.; frames];
        for (i, point) in contour.iter().enumerate() {
            let steady = contour[i.saturating_sub(10)..(i + 10).min(contour.len())]
                .iter()
                .all(|p| p.is_voiced());
            if steady {
                let end = (point.frame as usize + 256).min(frames);
                f0[point.frame as usize..end].fill(point.frequency as f64);
            }
        }
        let voice = Voice { samples: mono, f0 };
        let seconds = frames as f64 / 48_000.;
        println!(
            "{}: {seconds:.0}s, voiced {:.0}%, single-hop dropouts {}/s, octave jumps {}/s",
            path.rsplit('/').next().unwrap(),
            voiced as f64 / contour.len() as f64 * 100.,
            (dropouts as f64 / seconds).round(),
            (octaves as f64 / seconds).round()
        );
        let local = |signal: &[f32], delay: usize| -> f64 {
            let mut values = vec![];
            let mut start = 0;
            while start + 2400 < frames {
                let f = voice.f0[start];
                if f > 0. && voice.f0[(start + 1536).min(frames - 1)] > 0. {
                    let r = periodicity(signal, start + delay, 1536, 48_000. / f).min(0.999_999);
                    values.push(10. * (r / (1. - r)).log10());
                }
                start += 960;
            }
            values.iter().sum::<f64>() / values.len().max(1) as f64
        };
        let input = local(&voice.samples, 0);
        // Periodicity at the input period, so this measures texture at no shift.
        let neutral = Controls {
            strength: 0.,
            ..Controls::default()
        };
        let (output, _) = correct(&voice, neutral);
        let unshifted = local(&output, pitch::LATENCY);
        let (output, _, targets) = correct_targets(&voice, Preset::Natural);
        let (natural, _) = accuracy(&output, &targets, &voice.f0);
        let natural_hnr = output_hnr(&output, frames);
        let (output, _, targets, applied) = correct_targets_traced(&voice, Preset::Tight);
        let tight_hnr = output_hnr(&output, frames);
        let (tight, tight95) = accuracy(&output, &targets, &voice.f0);
        let (over100, over600) = large_shifts(&applied);
        let _ = unshifted;
        println!(
            "    HNR in {input:.1} dB, out Natural {natural_hnr:.1} dB, Tight {tight_hnr:.1} dB | error Natural {natural:.1}c, Tight {tight:.1}c (p95 {tight95:.1}c) | shifting >100c {over100:.1}%, >600c {over600:.1}%"
        );
    }
}

/// Prints the moments a Tight curve shifts by more than 600 cents.
#[test]
#[ignore = "needs SOUNDROOM_VOCALS; run with --ignored --nocapture"]
fn real_take_outliers() {
    let Ok(paths) = std::env::var("SOUNDROOM_VOCALS") else {
        return;
    };
    let pick: usize = std::env::var("SOUNDROOM_TAKE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let limit: f32 = std::env::var("SOUNDROOM_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(600.);
    let path = paths.split(':').nth(pick).unwrap();
    let mut reader = soundroom_engine::wave::WaveReader::open(std::path::Path::new(path)).unwrap();
    let frames = reader.info.frames.min(48_000 * 90) as usize;
    let mut left = vec![0.; frames];
    let mut right = vec![0.; frames];
    for (index, (l, r)) in left
        .chunks_mut(8192)
        .zip(right.chunks_mut(8192))
        .enumerate()
    {
        reader.read_stereo((index * 8192) as u64, l, r).unwrap();
    }
    let mono: Vec<f32> = left
        .iter()
        .zip(&right)
        .map(|(l, r)| (l + r) * 0.5)
        .collect();
    let contour = pitch::analyze(&mono, &mono, 65_536, &AtomicBool::new(false)).unwrap();
    let regions = seed(&contour, (0, frames as u64), 440., Preset::Tight, 5., 4095);
    let lanes = Regions::new(&regions).unwrap();
    let mut state = pitch::Pitch::default();
    let mut last = 0;
    let mut shown = 0;
    for (i, &sample) in mono.iter().enumerate() {
        let mut controls = Controls::default();
        match lanes.sample(i as i64) {
            Some((note, blend)) => {
                controls.manual_target = note;
                controls.curve = true;
                controls.mix *= blend;
            }
            None => controls.mix = 0.,
        }
        state.sample([sample, sample], controls);
        let m = state.meter;
        if m.wet_blend > 0.5 && m.correction_cents.abs() > limit && i > last + 4800 && shown < 16 {
            last = i;
            shown += 1;
            let source = m.source_frame.max(0) as usize;
            let near: Vec<String> = contour
                .iter()
                .filter(|p| (p.frame as i64 - source as i64).abs() < 1500)
                .map(|p| {
                    if p.is_voiced() {
                        format!("{:.1}", 69. + 12. * (p.frequency / 440.).log2())
                    } else {
                        "-".into()
                    }
                })
                .collect();
            println!(
                "t={:.3}s detected {:.1} target {:.1} correction {:+.0}c | analysis ±30ms: {}",
                source as f64 / 48000.,
                69. + 12. * (m.input_frequency.max(1.) / 440.).log2(),
                lanes.sample(source as i64).map_or(-1., |s| s.0),
                m.correction_cents,
                near.join(" ")
            );
        }
    }
}

#[test]
#[ignore = "prints Swift-reference differences for the pitch fixtures"]
fn fixture_differences() {
    use soundroom_engine::{project::Project, render::Renderer, wave::WaveReader};
    for name in [
        "pitch-automatic",
        "pitch-major",
        "pitch-custom",
        "pitch-mask-zero",
        "pitch-up",
        "pitch-down",
        "pitch-formant",
        "pitch-edits",
        "pitch-automation",
        "pitch-bypass",
        "pitch-master",
        "pitch-chain",
    ] {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
        let project = Project::open(&dir.join(format!("{name}.soundroom"))).unwrap();
        let mut renderer = Renderer::new(&project).unwrap();
        let mut reference = WaveReader::open(&dir.join(format!("{name}-swift.wav"))).unwrap();
        let end = reference.info.frames;
        let (mut worst, mut first) = (0_f32, None);
        while renderer.position() < end {
            let pos = renderer.position();
            let n = (end - pos).min(4096) as usize;
            let (mut l, mut r, mut el, mut er) =
                (vec![0.; n], vec![0.; n], vec![0.; n], vec![0.; n]);
            renderer.render(&mut l, &mut r).unwrap();
            reference.read_stereo(pos, &mut el, &mut er).unwrap();
            for i in 0..n {
                let e = (l[i] - el[i]).abs().max((r[i] - er[i]).abs());
                if e > 0.000005 && first.is_none() {
                    first = Some(pos + i as u64);
                }
                worst = worst.max(e);
            }
        }
        println!("{name}: max {worst:.6} first difference at {first:?} of {end}");
    }
}

/// RMS cents from a seeded Tight curve, and the percentage of wet time spent
/// shifting by more than 600 cents.
fn tight_on(voice: &Voice) -> (f64, f64) {
    let (output, _, targets, applied) = correct_targets_traced(voice, Preset::Tight);
    let (rms, _) = accuracy(&output, &targets, &voice.f0);
    let (_, over600) = large_shifts(&applied);
    (rms, over600)
}

#[test]
fn drawn_targets_are_followed_without_keeping_the_vibrato() {
    for (note, vowel) in [(45., AH), (57., EE), (64., OO)] {
        // The previous corrector kept half the vibrato and lagged 40 ms: ~22 cents.
        let voice = sing(note, -35., 3., 0.03, true, vowel, 11);
        let (rms, over600) = tight_on(&voice);
        assert!(
            rms < 12.,
            "note {note}: a flat Tight curve came out {rms:.1} cents RMS off"
        );
        assert!(
            over600 == 0.,
            "note {note}: {over600:.2}% of the time shifted an octave"
        );
    }
}

#[test]
fn raspy_low_voices_never_jump_an_octave_against_a_drawn_curve() {
    // Previously 139 and 19 cents, with 1.4% of the first spent an octave off.
    for (note, rasp) in [(52., 0.5), (47., 0.65)] {
        let voice = sing_rough(note, 20., 4., 0.05, true, AH, rasp, 23);
        let (rms, over600) = tight_on(&voice);
        assert!(
            over600 == 0.,
            "note {note} rasp {rasp}: {over600:.2}% shifted over 600 cents"
        );
        assert!(
            rms < 10.,
            "note {note} rasp {rasp}: {rms:.1} cents RMS from the curve"
        );
    }
}

#[test]
#[ignore = "prints the rough-voice table"]
fn rough_report() {
    for (note, vowel) in [(45., AH), (57., EE), (64., OO)] {
        let (rms, over600) = tight_on(&sing(note, -35., 3., 0.03, true, vowel, 11));
        println!("clean {note}: rms {rms:.1}c, over600 {over600:.2}%");
    }
    for (note, rasp) in [(40., 0.3), (40., 0.5), (43., 0.8), (47., 0.65), (52., 0.5)] {
        let (rms, over600) = tight_on(&sing_rough(note, 20., 4., 0.05, true, AH, rasp, 23));
        println!("rasp {rasp} {note}: rms {rms:.1}c, over600 {over600:.2}%");
    }
}
