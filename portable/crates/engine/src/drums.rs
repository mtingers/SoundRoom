//! Original Soundroom drum models and deterministic cyclic drum patterns.
//! Worker rendering is cancellable and preflights its PCM allocation budget.
use crate::{model::Extra, Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
};
const RATE: f64 = 48000.;
type SampleKey = (Kit, Voice, u64, i32, u64, Articulation, u64, Option<[u64; 10]>, Option<String>);
/// A rendered voice: mono (panned at mix time) or recorded stereo.
type Rendered = (Vec<f32>, Option<Vec<f32>>);
const CACHE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_RENDER_FRAMES: usize = 8_388_608;
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "camelCase")]
pub enum Kit {
    #[default]
    Acoustic,
    Electronic,
    JazzMaple,
    RockBirch,
    VintageDry,
    Brushed,
    Orchestral,
    FunkTight,
    VintageWarm,
    ArenaRock,
    Machine808,
    Machine909,
    LofiDusty,
    TrapModern,
    TechnoMinimal,
    StudioNatural,
    StudioTight,
    StudioRoom,
    StudioVintage,
    /// Generated from measurements of the recorded kit (`measured_drums`).
    StudioModeled,
}
impl Kit {
    pub const ALL: [Self; 20] = [
        Self::Acoustic,
        Self::Electronic,
        Self::JazzMaple,
        Self::RockBirch,
        Self::VintageDry,
        Self::Brushed,
        Self::Orchestral,
        Self::FunkTight,
        Self::VintageWarm,
        Self::ArenaRock,
        Self::Machine808,
        Self::Machine909,
        Self::LofiDusty,
        Self::TrapModern,
        Self::TechnoMinimal,
        Self::StudioNatural,
        Self::StudioTight,
        Self::StudioRoom,
        Self::StudioVintage,
        Self::StudioModeled,
    ];
    fn acoustic(self) -> bool {
        self.electronic().is_none()
    }
    fn profile(self) -> (f64, f64, f64, f64, bool) {
        match self {
            Self::JazzMaple => (1.18, 1.3, 0.9, 0.85, false),
            Self::RockBirch => (0.87, 0.95, 1.2, 1.08, false),
            Self::VintageDry => (0.94, 0.45, 0.75, 0.75, false),
            Self::Brushed => (1.08, 0.7, 1., 0.65, true),
            Self::Orchestral => (0.72, 1.55, 0.8, 0.78, false),
            Self::FunkTight => (1.12, 0.62, 1.15, 1.02, false),
            Self::VintageWarm => (0.96, 0.8, 0.9, 0.82, false),
            Self::ArenaRock => (0.8, 1.45, 1.12, 1.05, false),
            _ => (1., 1., 1., 1., false),
        }
    }
    /// Drum-machine voicing; None for modeled acoustic kits.
    fn electronic(self) -> Option<Electronic> {
        let classic = Electronic {
            kick_base: 47.,
            kick_sweep: 2.8,
            kick_sweep_time: 0.023,
            kick_decay: 0.15,
            kick_click: 0.10,
            click_time: 0.005,
            kick_drive: 1.,
            tom_scale: 1.,
            tom_decay: 0.21,
            snare_tone: 175.,
            snare_sweep: 100.,
            snare_tone_gain: 0.32,
            snare_tone_decay: 0.065,
            snare_noise_gain: 0.46,
            snare_noise_decay: 0.075,
            clap_gain: 0.39,
            clap_tail_gain: 0.18,
            clap_tail: 0.085,
            clap_spread: 1.,
            metal_scale: 1.,
            hat_decay: 1.,
            hat_noise: 0.34,
            hat_metal: 0.19,
            bits: 0.,
            hold: 1,
            lowpass: 0.,
        };
        Some(match self {
            Self::Electronic => classic,
            // Long tuned sub, soft click, two-tone snare and higher square-bank hats.
            Self::Machine808 => Electronic {
                kick_base: 44.,
                kick_sweep: 1.6,
                kick_sweep_time: 0.035,
                kick_decay: 0.48,
                kick_click: 0.03,
                tom_decay: 0.32,
                snare_tone: 190.,
                snare_sweep: 60.,
                snare_tone_gain: 0.38,
                snare_tone_decay: 0.09,
                snare_noise_gain: 0.36,
                snare_noise_decay: 0.14,
                clap_gain: 0.42,
                clap_tail_gain: 0.25,
                clap_tail: 0.12,
                metal_scale: 1.25,
                hat_decay: 0.9,
                hat_noise: 0.2,
                hat_metal: 0.3,
                ..classic
            },
            // Fast pitch sweep with a hard beater click and bright noisy hats.
            Self::Machine909 => Electronic {
                kick_base: 52.,
                kick_sweep: 3.6,
                kick_sweep_time: 0.016,
                kick_decay: 0.22,
                kick_click: 0.28,
                click_time: 0.003,
                kick_drive: 1.6,
                snare_tone: 200.,
                snare_sweep: 140.,
                snare_tone_gain: 0.28,
                snare_tone_decay: 0.045,
                snare_noise_gain: 0.62,
                snare_noise_decay: 0.11,
                clap_gain: 0.45,
                clap_tail_gain: 0.2,
                clap_tail: 0.1,
                metal_scale: 1.6,
                hat_noise: 0.5,
                hat_metal: 0.1,
                ..classic
            },
            // Sampler grit: reduced resolution, held samples and a dark top end.
            Self::LofiDusty => Electronic {
                kick_base: 50.,
                kick_sweep: 2.2,
                kick_decay: 0.2,
                kick_click: 0.12,
                kick_drive: 1.3,
                snare_noise_gain: 0.5,
                snare_noise_decay: 0.09,
                clap_tail_gain: 0.22,
                hat_decay: 0.8,
                hat_noise: 0.28,
                bits: 10.,
                hold: 3,
                lowpass: 5200.,
                ..classic
            },
            // Saturated long sub kick, crisp snare and clap, tight bright hats.
            Self::TrapModern => Electronic {
                kick_base: 41.,
                kick_sweep: 2.,
                kick_sweep_time: 0.03,
                kick_decay: 0.62,
                kick_click: 0.06,
                kick_drive: 1.25,
                snare_tone: 215.,
                snare_sweep: 120.,
                snare_tone_gain: 0.22,
                snare_tone_decay: 0.05,
                snare_noise_gain: 0.58,
                snare_noise_decay: 0.09,
                clap_gain: 0.5,
                clap_tail_gain: 0.22,
                clap_tail: 0.11,
                metal_scale: 1.7,
                hat_decay: 0.75,
                hat_noise: 0.45,
                hat_metal: 0.15,
                ..classic
            },
            // Driven rumble kick with a short click; dry metallic percussion.
            Self::TechnoMinimal => Electronic {
                kick_base: 50.,
                kick_sweep: 3.,
                kick_sweep_time: 0.02,
                kick_decay: 0.26,
                kick_click: 0.22,
                click_time: 0.004,
                kick_drive: 2.,
                tom_scale: 1.12,
                tom_decay: 0.17,
                snare_tone: 185.,
                snare_noise_gain: 0.4,
                snare_noise_decay: 0.06,
                clap_gain: 0.4,
                clap_spread: 1.25,
                metal_scale: 1.4,
                hat_decay: 0.9,
                hat_noise: 0.38,
                hat_metal: 0.22,
                ..classic
            },
            _ => return None,
        })
    }
}
/// Drum-machine voice parameters (times in seconds, frequencies in Hz).
#[derive(Clone, Copy)]
struct Electronic {
    kick_base: f64,
    kick_sweep: f64,
    kick_sweep_time: f64,
    kick_decay: f64,
    kick_click: f64,
    click_time: f64,
    kick_drive: f64,
    tom_scale: f64,
    tom_decay: f64,
    snare_tone: f64,
    snare_sweep: f64,
    snare_tone_gain: f64,
    snare_tone_decay: f64,
    snare_noise_gain: f64,
    snare_noise_decay: f64,
    clap_gain: f64,
    clap_tail_gain: f64,
    clap_tail: f64,
    clap_spread: f64,
    metal_scale: f64,
    hat_decay: f64,
    hat_noise: f64,
    hat_metal: f64,
    /// Sample resolution in bits (0 keeps full resolution).
    bits: f64,
    /// Sample-and-hold length in frames.
    hold: usize,
    /// One-pole low-pass cutoff (0 bypasses).
    lowpass: f64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Voice {
    Kick,
    Snare,
    ClosedHat,
    OpenHat,
    LowTom,
    HighTom,
    Clap,
    Ride,
    Crash,
}
impl Voice {
    pub const ALL: [Self; 9] = [
        Self::Kick,
        Self::Snare,
        Self::ClosedHat,
        Self::OpenHat,
        Self::LowTom,
        Self::HighTom,
        Self::Clap,
        Self::Ride,
        Self::Crash,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Kick => "kick",
            Self::Snare => "snare",
            Self::ClosedHat => "closedHat",
            Self::OpenHat => "openHat",
            Self::LowTom => "lowTom",
            Self::HighTom => "highTom",
            Self::Clap => "clap",
            Self::Ride => "ride",
            Self::Crash => "crash",
        }
    }
    fn duration_for(self, kit: Kit) -> f64 {
        match (self, kit) {
            (Self::Kick, Kit::Machine808 | Kit::TrapModern) => 1.8,
            _ => self.duration(),
        }
    }
    fn duration(self) -> f64 {
        match self {
            Self::Kick => 1.1,
            Self::Snare | Self::Clap => 1.,
            Self::ClosedHat => 0.35,
            Self::OpenHat => 1.6,
            Self::LowTom | Self::HighTom => 1.5,
            Self::Ride => 2.5,
            Self::Crash => 3.2,
        }
    }
    fn articulation(self, a: Articulation) -> Articulation {
        match (self, a) {
            (
                Self::Snare,
                Articulation::Rimshot | Articulation::SideStick | Articulation::Brush,
            )
            | (Self::Ride | Self::Crash, Articulation::Bell)
            | (Self::LowTom | Self::HighTom, Articulation::Rimshot) => a,
            _ => Articulation::Center,
        }
    }
}
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "camelCase")]
pub enum Articulation {
    #[default]
    Center,
    Rimshot,
    SideStick,
    Brush,
    Bell,
}
fn safe(v: f64, lo: f64, hi: f64) -> f64 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        lo
    }
}
struct Random(u64);
impl Random {
    fn signed(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 11) as f64 / (1_u64 << 53) as f64 * 2. - 1.
    }
}
struct Mode {
    phase: f64,
    step: f64,
    amplitude: f64,
    decay: f64,
}
impl Mode {
    fn sample(&mut self) -> f64 {
        let v = self.phase.sin() * self.amplitude;
        self.phase += self.step;
        self.amplitude *= self.decay;
        v
    }
}
pub fn render_voice(
    kit: Kit,
    voice: Voice,
    velocity: f64,
    robin: i32,
    articulation: Articulation,
    damping: f64,
    cancelled: &AtomicBool,
) -> Result<Vec<f32>> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(Error::Invalid("Drum render cancelled".into()));
    }
    // Recorded kits model their missing pieces (clap, or no installed audio)
    // with the acoustic studio voice.
    let kit = if kit.stem_mix().is_some() || kit == Kit::StudioModeled {
        Kit::Acoustic
    } else {
        kit
    };
    let velocity = if velocity.is_finite() {
        velocity.clamp(0., 1.)
    } else {
        0.8
    };
    let articulation = voice.articulation(articulation);
    let (pitch, body, wires, metal, brush) = kit.profile();
    let brushed = brush || articulation == Articulation::Brush;
    let damping = if damping.is_finite() {
        damping.clamp(0., 1.)
    } else {
        0.5
    };
    let damping_scale = 2_f64.powf((0.5 - damping) * 2.);
    let rr = robin.max(0) % 32;
    let voice_index = Voice::ALL.iter().position(|v| *v == voice).unwrap();
    let kit_index = Kit::ALL.iter().position(|v| *v == kit).unwrap();
    let mut random = Random(
        (991 + voice_index * 8191 + rr as usize * 401) as u64
            + if kit.acoustic() {
                kit_index as u64 * 104729
            } else {
                104729
            },
    );
    let variation = random.signed();
    let strike = 0.96 + 0.035 * random.signed();
    let size = (voice.duration_for(kit) * RATE).round() as usize;
    let mut output = vec![0.; size];
    if velocity <= 0. {
        return Ok(output);
    }
    let level = velocity.powf(1.25);
    let is_metal = matches!(
        voice,
        Voice::ClosedHat | Voice::OpenHat | Voice::Ride | Voice::Crash
    );
    let mut modes = Vec::with_capacity(48);
    if kit.acoustic() {
        let fundamental = match voice {
            Voice::Kick => 53.,
            Voice::Snare => 181.,
            Voice::LowTom => 105.,
            Voice::HighTom => 157.,
            Voice::Clap => 790.,
            Voice::Ride => 410.,
            _ => 570.,
        };
        let mut ratios = if is_metal {
            vec![
                1., 1.37, 1.91, 2.43, 3.21, 4.17, 5.07, 6.71, 8.37, 10.49, 12.13, 15.47, 18.71,
                22.19, 27.31, 32.79,
            ]
        } else {
            vec![1., 1.593, 2.135, 2.296, 2.653, 2.918, 3.156, 3.5]
        };
        if kit != Kit::Acoustic && is_metal {
            for i in 16..48 {
                ratios.push(i as f64 * 2.07 + 1.17 + random.signed() * 0.41);
            }
        }
        let decay_base = match voice {
            Voice::Kick => 0.16,
            Voice::Snare => 0.13,
            Voice::LowTom => 0.26,
            Voice::HighTom => 0.20,
            Voice::ClosedHat => 0.035,
            Voice::OpenHat => 0.21,
            Voice::Ride => 0.40,
            Voice::Crash => 0.52,
            Voice::Clap => 0.045,
        };
        for (index, ratio) in ratios.into_iter().enumerate() {
            let strike_pitch = if articulation == Articulation::SideStick {
                3.6
            } else if articulation == Articulation::Bell {
                1.8
            } else {
                1.
            };
            let frequency = fundamental
                * pitch
                * (if is_metal { metal } else { 1. })
                * strike_pitch
                * ratio
                * (1. + variation * 0.009 + random.signed() * 0.003);
            if frequency > RATE * 0.43 {
                continue;
            }
            let bright = velocity.powf(index as f64 * if is_metal { 0.025 } else { 0.11 });
            let modal_gain = if kit != Kit::Acoustic && is_metal {
                0.65
            } else {
                1.
            };
            let rim = if articulation == Articulation::Rimshot {
                if index == 0 {
                    0.7
                } else {
                    1.5
                }
            } else {
                1.
            };
            let weight = (if is_metal {
                0.030 * modal_gain / (index as f64 + 1.).powf(0.28)
            } else {
                0.36 * rim / (index as f64 + 1.).powf(1.35)
            }) * (if brushed { 0.35 } else { 1. });
            let decay = decay_base
                * body
                * damping_scale
                * (if articulation == Articulation::SideStick {
                    0.18
                } else {
                    1.
                })
                * (0.91 + random.signed() * 0.05)
                / ratio.powf(if is_metal { 0.14 } else { 0.60 });
            modes.push(Mode {
                phase: random.signed() * 0.35,
                step: 2. * std::f64::consts::PI * frequency / RATE,
                amplitude: weight * bright * strike,
                decay: (-1. / (decay * RATE)).exp(),
            });
        }
    }
    let mut noise_lp = 0.;
    let mut noise_body = 0.;
    let mut phase = 0.;
    let mut metal_phases = [0.; 6];
    let machine = kit.electronic();
    let (mut held, mut dusty) = (0., 0.);
    let cutoff = if is_metal {
        2500.
    } else if voice == Voice::Kick {
        1900.
    } else {
        950. + velocity * 6100.
    };
    let coefficient = 1. - (-2. * std::f64::consts::PI * cutoff / RATE).exp();
    for (frame, out) in output.iter_mut().enumerate() {
        if frame % 8192 == 0 && cancelled.load(Ordering::Relaxed) {
            return Err(Error::Invalid("Drum render cancelled".into()));
        }
        let t = frame as f64 / RATE;
        let noise = random.signed();
        noise_lp += coefficient * (noise - noise_lp);
        noise_body += 0.045 * (noise - noise_body);
        let high_noise = noise - noise_lp;
        let mut signal = 0.;
        if kit.acoustic() {
            for mode in &mut modes {
                signal += mode.sample();
            }
            match voice {
                Voice::Kick => {
                    if kit != Kit::Acoustic {
                        phase += 2.
                            * std::f64::consts::PI
                            * 53.
                            * pitch
                            * (1. + velocity * 0.65 * (-t / 0.018).exp())
                            / RATE;
                        signal += phase.sin() * 0.18 * (-t / (0.12 * body * damping_scale)).exp();
                    }
                    signal += noise_lp * (0.12 + 0.13 * velocity) * (-t / 0.009).exp();
                }
                Voice::Snare => {
                    let wire = if articulation == Articulation::SideStick {
                        0.035
                    } else {
                        wires
                    };
                    let decay = (if articulation == Articulation::Brush {
                        0.32
                    } else if brushed {
                        0.19
                    } else {
                        0.065 + 0.038 * velocity
                    }) * body
                        * damping_scale;
                    let diffusion = if brushed { 1. - (-t / 0.008).exp() } else { 1. };
                    signal += (noise_lp - noise_body)
                        * (0.35 + 0.15 * velocity)
                        * wire
                        * diffusion
                        * (-t / decay).exp();
                    if articulation == Articulation::Rimshot {
                        signal += high_noise * 0.22 * (-t / 0.002).exp();
                    }
                    signal += high_noise
                        * (if brushed { 0.025 } else { 0.17 })
                        * velocity
                        * (-t / 0.004).exp();
                }
                Voice::LowTom | Voice::HighTom => {
                    signal += noise_lp * 0.13 * velocity * (-t / 0.012).exp()
                }
                Voice::Clap => {
                    let burst = [0., 0.011, 0.024].iter().fold(0., |sum, &b| {
                        sum + if t >= b { (-(t - b) / 0.008).exp() } else { 0. }
                    });
                    signal += (noise_lp - noise_body) * 0.37 * burst;
                }
                _ => {
                    let decay = match voice {
                        Voice::ClosedHat => 0.037,
                        Voice::OpenHat => 0.21,
                        Voice::Ride => 0.28,
                        _ => 0.38,
                    };
                    signal += high_noise
                        * (0.11 + 0.09 * velocity)
                        * (if articulation == Articulation::Bell {
                            0.2
                        } else {
                            1.
                        })
                        * (-t / (decay * body * damping_scale)).exp();
                }
            }
        } else if let Some(e) = machine {
            match voice {
                Voice::Kick | Voice::LowTom | Voice::HighTom => {
                    let kick = voice == Voice::Kick;
                    let base = match voice {
                        Voice::Kick => e.kick_base,
                        Voice::LowTom => 94. * e.tom_scale,
                        _ => 151. * e.tom_scale,
                    };
                    let (sweep, sweep_time) = if kick {
                        (e.kick_sweep, e.kick_sweep_time)
                    } else {
                        (2.8, 0.023)
                    };
                    let pitch =
                        base * (1. + sweep * (-t / sweep_time).exp()) * (1. + variation * 0.008);
                    phase += 2. * std::f64::consts::PI * pitch / RATE;
                    signal = phase.sin()
                        * 0.75
                        * (-t / if kick { e.kick_decay } else { e.tom_decay }).exp();
                    let (click, click_time) = if kick {
                        (e.kick_click, e.click_time)
                    } else {
                        (0.10, 0.005)
                    };
                    signal += noise_lp * click * velocity * (-t / click_time).exp();
                    if kick && e.kick_drive > 1. {
                        signal = (signal * e.kick_drive).tanh() / e.kick_drive.tanh();
                    }
                }
                Voice::Snare => {
                    phase += 2.
                        * std::f64::consts::PI
                        * (e.snare_tone + e.snare_sweep * (-t / 0.01).exp())
                        / RATE;
                    signal = phase.sin() * e.snare_tone_gain * (-t / e.snare_tone_decay).exp()
                        + (noise_lp - noise_body)
                            * e.snare_noise_gain
                            * (-t / e.snare_noise_decay).exp();
                }
                Voice::Clap => {
                    let burst = [0., 0.009, 0.019, 0.031].iter().fold(0., |sum, &b| {
                        let b = b * e.clap_spread;
                        sum + if t >= b { (-(t - b) / 0.006).exp() } else { 0. }
                    });
                    signal = (noise_lp - noise_body)
                        * (e.clap_gain * burst + e.clap_tail_gain * (-t / e.clap_tail).exp());
                }
                _ => {
                    let frequencies = [317., 431., 587., 811., 1093., 1487.];
                    let mut metal = 0.;
                    for (i, phase) in metal_phases.iter_mut().enumerate() {
                        *phase += frequencies[i] * e.metal_scale * (1. + variation * 0.005) / RATE;
                        let p = *phase % 1.;
                        metal += ((p * 2. - 1.).abs() * 2. - 1.) / 6.;
                    }
                    let decay = match voice {
                        Voice::ClosedHat => 0.035,
                        Voice::OpenHat => 0.18,
                        Voice::Ride => 0.33,
                        _ => 0.45,
                    } * e.hat_decay;
                    signal = (high_noise * e.hat_noise + metal * e.hat_metal) * (-t / decay).exp();
                }
            }
            if e.hold > 1 {
                if frame % e.hold == 0 {
                    held = signal;
                }
                signal = held;
            }
            if e.bits > 0. {
                let steps = 2_f64.powf(e.bits - 1.);
                signal = (signal * steps).round() / steps;
            }
            if e.lowpass > 0. {
                let c = 1. - (-2. * std::f64::consts::PI * e.lowpass / RATE).exp();
                dusty += c * (signal - dusty);
                signal = dusty;
            }
        }
        let attack = (frame as f64 / 12.).min(1.);
        let tail = ((size - 1 - frame) as f64 / 240.).min(1.);
        let release = if frame < size - 2400 {
            1.
        } else {
            ((size - 1 - frame) as f64 / 2400.).powi(3)
        };
        *out = (signal * level * 0.82 * attack * tail * release) as f32;
    }
    Ok(output)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub articulation: Option<Articulation>,
    pub voice: Voice,
    pub index: i32,
    pub velocity: f64,
    pub pan: f64,
    pub probability: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Lane {
    pub kit: Kit,
    pub damping: Option<f64>,
    /// Kit-designer shaping (modeled kit only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shape: Option<crate::measured_drums::Shape>,
    /// Saved kit-designer kit this lane plays (its exported samples when
    /// installed; otherwise generated from `shape`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_kit: Option<String>,
    pub gain: f64,
    pub pan: f64,
    pub tuning: f64,
    pub muted: bool,
}
impl Default for Lane {
    fn default() -> Self {
        Self {
            kit: Kit::Acoustic,
            damping: None,
            shape: None,
            custom_kit: None,
            gain: 1.,
            pan: 0.,
            tuning: 0.,
            muted: false,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Pattern {
    pub velocity_humanize: Option<f64>,
    pub variation_cycles: Option<i32>,
    pub name: String,
    pub kit: Kit,
    pub bpm: f64,
    pub bars: i32,
    pub steps_per_beat: i32,
    pub swing: f64,
    pub humanize: f64,
    pub master_gain: f64,
    pub steps: Vec<Step>,
    pub lanes: BTreeMap<String, Lane>,
    #[serde(flatten)]
    pub extra: Extra,
}
impl Default for Pattern {
    fn default() -> Self {
        Self {
            velocity_humanize: None,
            variation_cycles: None,
            name: "Drum loop".into(),
            kit: Kit::Acoustic,
            bpm: 110.,
            bars: 1,
            steps_per_beat: 4,
            swing: 0.,
            humanize: 0.,
            master_gain: 0.8,
            steps: vec![],
            lanes: BTreeMap::new(),
            extra: Extra::new(),
        }
    }
}
impl Pattern {
    fn tempo(&self) -> f64 {
        if self.bpm.is_finite() {
            self.bpm.clamp(30., 300.)
        } else {
            110.
        }
    }
    fn divisions(&self) -> i32 {
        match self.steps_per_beat {
            2 => 2,
            8 => 8,
            _ => 4,
        }
    }
    pub fn step_count(&self) -> i32 {
        self.bars.clamp(1, 8) * 4 * self.divisions()
    }
    pub fn duration(&self) -> f64 {
        self.bars.clamp(1, 8) as f64 * 4. * 60. / self.tempo()
    }
    pub fn time_for_step(&self, index: i32) -> f64 {
        (index as f64
            + if index % 2 == 1 {
                safe(self.swing, 0., 0.7)
            } else {
                0.
            })
            * 60.
            / self.tempo()
            / self.divisions() as f64
    }
    /// One hit of `voice` at `velocity` as this pattern plays it: the lane's
    /// kit, damping, kit-designer shape, tuning, gain and pan.
    pub fn audition(&self, voice: Voice, velocity: f64, cancelled: &AtomicBool) -> Result<[Vec<f32>; 2]> {
        let mut single = self.clone();
        single.steps = vec![Step {
            articulation: None,
            voice,
            index: 0,
            velocity: safe(velocity, 0., 1.),
            pan: 0.,
            probability: 1.,
        }];
        single.swing = 0.;
        single.humanize = 0.;
        single.velocity_humanize = None;
        single.variation_cycles = None;
        single.master_gain = 1.;
        // 16 s at 30 BPM: the longest tail never wraps onto itself.
        single.bars = 2;
        single.bpm = 30.;
        let [mut left, mut right] = single.render(1, false, cancelled)?;
        let end = left
            .iter()
            .zip(&right)
            .rposition(|(l, r)| l.abs() > 1e-5 || r.abs() > 1e-5)
            .map_or(1, |i| i + 1);
        left.truncate(end);
        right.truncate(end);
        Ok([left, right])
    }
    pub fn render(
        &self,
        repeats: i32,
        include_tail: bool,
        cancelled: &AtomicBool,
    ) -> Result<[Vec<f32>; 2]> {
        self.render_cycles(repeats, include_tail, true, cancelled)
    }
    /// The same passes as `render` but starting from silence: no tails of a
    /// preceding pass ring into the first beat. Played once before the loop.
    pub fn render_first_pass(&self, repeats: i32, cancelled: &AtomicBool) -> Result<[Vec<f32>; 2]> {
        self.render_cycles(repeats, false, false, cancelled)
    }
    fn render_cycles(
        &self,
        repeats: i32,
        include_tail: bool,
        warm: bool,
        cancelled: &AtomicBool,
    ) -> Result<[Vec<f32>; 2]> {
        let frames = (self.duration() * RATE).round().max(1.) as usize;
        let variations = self.variation_cycles.unwrap_or(1).clamp(1, 8);
        let repeats = repeats.clamp(1, 16).max(variations);
        let length = frames
            .checked_mul(repeats as usize)
            .and_then(|v| {
                v.checked_add(if include_tail {
                    (3.2 * RATE) as usize
                } else {
                    0
                })
            })
            .filter(|v| *v <= MAX_RENDER_FRAMES)
            .ok_or_else(|| {
                Error::Invalid(
                    "Drum render exceeds bounded PCM worker budget; choose fewer repeats".into(),
                )
            })?;
        if self.steps.len() > 1_000_000 {
            return Err(Error::Invalid("Too many drum steps".into()));
        }
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::Invalid("Drum render cancelled".into()));
        }
        let mut output = [vec![0.; length], vec![0.; length]];
        let mut cache: BTreeMap<SampleKey, Rendered> = BTreeMap::new();
        let mut cache_bytes = 0;
        let mut valid = self
            .steps
            .iter()
            .take(2304)
            .filter(|s| s.index >= 0 && s.index < self.step_count())
            .collect::<Vec<_>>();
        valid.sort_by_key(|s| s.index);
        let mut counts = BTreeMap::new();
        for step in &valid {
            *counts.entry(step.voice).or_insert(0_i32) += 1;
        }
        let gain = safe(self.master_gain, 0., 1.);
        let warmup = if warm { (6.4 / self.duration()).ceil() as i32 + 1 } else { 0 };
        // Modeled hits are expensive to synthesize: a first pass collects every
        // distinct one and renders them in parallel; the mixing pass then reads
        // them from the measured kit's cache.
        let modeled = self.kit == Kit::StudioModeled
            || self.lanes.values().any(|l| l.kit == Kit::StudioModeled);
        let mut requests: Vec<crate::measured_drums::Request> = vec![];
        for collecting in [true, false] {
        if collecting && !modeled {
            continue;
        }
        if !collecting && !requests.is_empty() {
            crate::measured_drums::prefetch(&requests, cancelled)?;
        }
        for cycle in -warmup..repeats {
            if cancelled.load(Ordering::Relaxed) {
                return Err(Error::Invalid("Drum render cancelled".into()));
            }
            let mut strikes = BTreeMap::new();
            for step in &valid {
                let fallback = Lane {
                    kit: self.kit,
                    ..Lane::default()
                };
                let lane = self.lanes.get(step.voice.name()).unwrap_or(&fallback);
                if lane.muted {
                    continue;
                }
                let strike = *strikes.entry(step.voice).or_insert(0_i32);
                *strikes.get_mut(&step.voice).unwrap() += 1;
                let pass = cycle.rem_euclid(variations);
                let voice_index = Voice::ALL.iter().position(|v| *v == step.voice).unwrap();
                let hash =
                    (pass * 7919 + step.index * 977 + voice_index as i32 * 131 + strike * 31 + 17)
                        as u64;
                let chance =
                    (hash.wrapping_mul(1664525).wrapping_add(1013904223) % 10000) as f64 / 10000.;
                if chance >= safe(step.probability, 0., 1.) {
                    continue;
                }
                let jitter = ((hash.wrapping_mul(1103515245).wrapping_add(12345) % 10000) as f64
                    / 5000.
                    - 1.)
                    * safe(self.humanize, 0., 0.025);
                let onset = ((self.time_for_step(step.index) + jitter)
                    .clamp(0., self.duration() - 1. / RATE)
                    * RATE)
                    .round() as i64
                    + cycle as i64 * frames as i64;
                let velocity_offset =
                    ((hash.wrapping_mul(214013).wrapping_add(2531011) % 10000) as f64 / 5000. - 1.)
                        * safe(self.velocity_humanize.unwrap_or(0.), 0., 0.2);
                let velocity = safe(step.velocity + velocity_offset, 0., 1.);
                let stride = counts[&step.voice].max(1) | 1;
                let robin = (strike + pass * stride) % 8;
                let articulation = step.articulation.unwrap_or_default();
                let damping = lane.damping.unwrap_or(0.5);
                let key = (
                    lane.kit,
                    step.voice,
                    velocity.to_bits(),
                    robin,
                    lane.tuning.to_bits(),
                    articulation,
                    damping.to_bits(),
                    lane.shape.map(|s| s.sanitized().cache_key()),
                    lane.custom_kit.clone(),
                );
                let exported = lane
                    .custom_kit
                    .as_deref()
                    .filter(|id| crate::sampled_drums::custom_available(id));
                if collecting {
                    if lane.kit == Kit::StudioModeled && exported.is_none() {
                        requests.push(crate::measured_drums::Request {
                            voice: step.voice,
                            velocity,
                            robin,
                            articulation,
                            damping,
                            shape: lane.shape,
                        });
                    }
                    continue;
                }
                if !cache.contains_key(&key) {
                    let custom = match exported {
                        Some(id) => crate::sampled_drums::render_custom(
                            id,
                            step.voice,
                            velocity,
                            robin,
                            articulation,
                            damping,
                            cancelled,
                        )?,
                        None => None,
                    };
                    let recorded = match custom {
                        Some(hit) => Some(hit),
                        None => match crate::sampled_drums::render(
                        lane.kit,
                        step.voice,
                        velocity,
                        robin,
                        articulation,
                        damping,
                        cancelled,
                    )? {
                        Some(hit) => Some(hit),
                        None => crate::measured_drums::render(
                            lane.kit,
                            step.voice,
                            velocity,
                            robin,
                            articulation,
                            damping,
                            lane.shape,
                            cancelled,
                        )?,
                    },
                    };
                    let (raw, raw_right) = match recorded {
                        Some([l, r]) => (l, Some(r)),
                        None => (
                            render_voice(
                                lane.kit,
                                step.voice,
                                velocity,
                                robin,
                                articulation,
                                damping,
                                cancelled,
                            )?,
                            None,
                        ),
                    };
                    let ratio = 2_f64.powf(safe(lane.tuning, -12., 12.) / 12.);
                    let tune = |raw: Vec<f32>| -> Vec<f32> {
                        if ratio == 1. || raw.is_empty() {
                            return raw;
                        }
                        (0..(raw.len() as f64 / ratio) as usize)
                            .map(|i| {
                                let position = i as f64 * ratio;
                                let a = (position as usize).min(raw.len() - 1);
                                let b = (a + 1).min(raw.len() - 1);
                                let f = (position - a as f64) as f32;
                                raw[a] * (1. - f) + raw[b] * f
                            })
                            .collect()
                    };
                    let sample = (tune(raw), raw_right.map(tune));
                    let bytes = (sample.0.len() + sample.1.as_ref().map_or(0, Vec::len))
                        * std::mem::size_of::<f32>();
                    if cache_bytes + bytes > CACHE_BYTES {
                        cache.clear();
                        cache_bytes = 0;
                    }
                    cache_bytes += bytes;
                    cache.insert(key.clone(), sample);
                }
                let (sample, sample_right) = &cache[&key];
                let pan = safe(step.pan + lane.pan, -1., 1.);
                let level = gain * safe(lane.gain, 0., 1.5);
                let (left, right) = if sample_right.is_some() {
                    // Recorded stereo keeps its image; pan acts as balance.
                    (
                        (level * (1. - pan).min(1.)) as f32,
                        (level * (1. + pan).min(1.)) as f32,
                    )
                } else {
                    (
                        (((pan + 1.) * std::f64::consts::PI / 4.).cos() * level) as f32,
                        (((pan + 1.) * std::f64::consts::PI / 4.).sin() * level) as f32,
                    )
                };
                let mut choke = sample.len();
                if step.voice == Voice::OpenHat {
                    if let Some(closing) = valid
                        .iter()
                        .find(|s| s.voice == Voice::ClosedHat && s.index > step.index)
                    {
                        choke = choke.min(
                            ((self.time_for_step(closing.index) - self.time_for_step(step.index))
                                * RATE)
                                .max(0.) as usize,
                        );
                    } else if let Some(closing) = valid.iter().find(|s| s.voice == Voice::ClosedHat)
                    {
                        choke = choke.min(
                            ((self.duration() + self.time_for_step(closing.index)
                                - self.time_for_step(step.index))
                                * RATE)
                                .max(0.) as usize,
                        );
                    }
                }
                for (i, &v) in sample
                    .iter()
                    .take(sample.len().min(choke + 120))
                    .enumerate()
                {
                    if i & 8191 == 0 && cancelled.load(Ordering::Relaxed) {
                        return Err(Error::Invalid("Drum render cancelled".into()));
                    }
                    let frame = onset + i as i64;
                    if frame >= 0 && (frame as usize) < length {
                        let cg = if i < choke {
                            1.
                        } else {
                            (1. - (i - choke) as f64 / 120.).max(0.) as f32
                        };
                        let r = sample_right.as_ref().map_or(v, |r| r[i]);
                        output[0][frame as usize] += v * left * cg;
                        output[1][frame as usize] += r * right * cg;
                    }
                }
            }
        }
        }
        Ok(output)
    }
}
