//! Original, deterministic guitar rig. No captured circuits, IRs or model weights.
use super::{
    chorus::Chorus,
    equalizer::{Section, Shape},
    room::Room,
    warmth::Warmth,
};
pub const LATENCY: usize = 32;
#[derive(Clone)]
pub struct GuitarRig {
    pedal: Warmth,
    amp: Warmth,
    room: Room,
    chorus: Chorus,
    filters: [[Section; 10]; 2],
    targets: [Section; 10],
    filter_frames: usize,
    drive_targets: [f32; 4],
    drive_frames: usize,
    delay: Vec<[f32; 2]>,
    delay_index: usize,
    dry: [[f32; 2]; LATENCY],
    dry_index: usize,
    values: [f32; 30],
    dirty: bool,
    initialized: bool,
    tick: usize,
    input_gain: f32,
    master_gain: f32,
    previous: [f32; 30],
    output_gain: f32,
    pedal_values: [f32; 8],
    amp_values: [f32; 8],
    room_values: [f32; 10],
    chorus_values: [f32; 6],
    gate_threshold: f32,
    envelope: f32,
    gate_gain: f32,
    hold: usize,
    pedal_blend: f32,
    amp_blend: f32,
    cab_blend: f32,
    mod_weights: [f32; 3],
    phase: f64,
    delay_frames: f32,
    delay_tone: f32,
    delay_filtered: [f32; 2],
    pub last_dry: [f32; 2],
}
impl Default for GuitarRig {
    fn default() -> Self {
        Self {
            pedal: Warmth::default(),
            amp: Warmth::default(),
            room: Room::default(),
            chorus: Chorus::default(),
            filters: [[Section::default(); 10]; 2],
            targets: [Section::default(); 10],
            filter_frames: 0,
            drive_targets: [0.; 4],
            drive_frames: 0,
            delay: vec![[0.; 2]; 57602],
            delay_index: 0,
            dry: [[0.; 2]; LATENCY],
            dry_index: 0,
            values: [0.; 30],
            dirty: true,
            initialized: false,
            tick: 0,
            input_gain: 1.,
            master_gain: 1.,
            previous: [f32::NAN; 30],
            output_gain: 1.,
            pedal_values: [0.; 8],
            amp_values: [0.; 8],
            room_values: [0.; 10],
            chorus_values: [0.; 6],
            gate_threshold: 0.,
            envelope: 0.,
            gate_gain: 1.,
            hold: 0,
            pedal_blend: 0.,
            amp_blend: 0.,
            cab_blend: 0.,
            mod_weights: [1., 0., 0.],
            phase: 0.,
            delay_frames: 1.,
            delay_tone: 0.,
            delay_filtered: [0.; 2],
            last_dry: [0.; 2],
        }
    }
}
fn db(v: f32) -> f32 {
    10_f32.powf(v / 20.)
}
fn glide(current: &mut f32, target: f32) {
    *current += (target - *current).clamp(-1. / 480., 1. / 480.);
}
impl GuitarRig {
    pub fn configure(&mut self, values: &[f32]) {
        self.values.copy_from_slice(values);
        self.dirty = true;
    }
    fn controls(&mut self) {
        let v = self.values;
        let previous_drives = [
            self.pedal_values[0],
            self.pedal_values[6],
            self.amp_values[0],
            self.amp_values[6],
        ];
        let pedal = v[2] as usize;
        let amp = v[6] as usize;
        let cabinet = v[14] as usize;
        if self.previous[0] != v[0] {
            self.input_gain = db(v[0]);
        }
        if self.previous[28] != v[28] {
            self.output_gain = db(v[28]);
        }
        if self.previous[13] != v[13] {
            self.master_gain = db(v[13]);
        }
        if self.previous[1] != v[1] {
            self.gate_threshold = db(v[1]);
        }
        self.pedal_values = [
            v[3] + if pedal == 3 { 6. } else { 0. },
            v[4],
            1.,
            v[5],
            0.,
            if pedal == 1 { 2. } else { 4. },
            if pedal == 3 { 0.85 } else { 0.2 },
            if pedal == 1 { 4. } else { 0. },
        ];
        let (gain, character, bias, bass, mid, treble) = match amp {
            1 => (v[7] * 0.35, 2., 0.12, 0., -2., 1.),
            2 => (3. + v[7] * 0.65, 2., 0.4, -2., 2., 2.),
            3 => (6. + v[7] * 0.8, 4., 0.3, 0., 3., -1.),
            4 => (12. + v[7] * 0.75, 4., 0.15, -1., -1., 0.),
            _ => (0., 0., 0., 0., 0., 0.),
        };
        self.amp_values = [gain, 18000., 1., 0., 0., character, bias, v[11]];
        self.drive_targets = [self.pedal_values[0], self.pedal_values[6], gain, bias];
        if self.initialized {
            self.pedal_values[0] = previous_drives[0];
            self.pedal_values[6] = previous_drives[1];
            self.amp_values[0] = previous_drives[2];
            self.amp_values[6] = previous_drives[3];
            if self.drive_targets != previous_drives {
                self.drive_frames = 480;
            }
        }
        if self.previous[2] != v[2] {
            self.set_filter(
                0,
                [20., 480., 110., 70.][pedal.min(3)],
                0.707,
                0.,
                Shape::HighPass,
            );
        }
        if self.previous[6] != v[6]
            || self.previous[8] != v[8]
            || self.previous[9] != v[9]
            || self.previous[10] != v[10]
        {
            self.set_filter(1, 140., 0.707, v[8] + bass, Shape::LowShelf);
            self.set_filter(
                2,
                if amp == 2 { 1500. } else { 800. },
                0.75,
                v[9] + mid,
                Shape::Bell,
            );
            self.set_filter(3, 3000., 0.707, v[10] + treble, Shape::HighShelf);
        }
        if self.previous[14] != v[14]
            || self.previous[15] != v[15]
            || self.previous[16] != v[16]
            || self.previous[17] != v[17]
        {
            self.set_filter(4, v[16], 0.707, 0., Shape::HighPass);
            self.set_filter(
                5,
                [100., 110., 95., 100., 85.][cabinet.min(4)],
                0.85,
                [0., 2., 1., 4., 5.][cabinet.min(4)],
                Shape::Bell,
            );
            self.set_filter(
                6,
                650.,
                0.8,
                [0., -1., 1., -3., -4.][cabinet.min(4)],
                Shape::Bell,
            );
            self.set_filter(7, 2400., 1.1, (1. - v[15]) * 4. - v[15] * 5., Shape::Bell);
            let cutoff = v[17]
                .min([16000., 6500., 7200., 5600., 6200.][cabinet.min(4)] * (1. - v[15] * 0.35));
            self.set_filter(8, cutoff, 0.5411961, 0., Shape::LowPass);
            self.set_filter(9, cutoff, 1.306563, 0., Shape::LowPass);
        }
        self.chorus_values = [v[19], v[20] * 8., 12., 0.85, 0., v[21]];
        self.room_values = [v[26], 0., 5000., 1., 1., 120., 0.8, 0.85, 0.15, 0.];
        if self.previous[26] != v[26] {
            self.room.configure(&self.room_values);
        }
        if self.previous[22] != v[22] {
            self.delay_tone = 1. - (-2. * std::f32::consts::PI * v[22] / 48000.).exp();
        }
        self.previous = v;
        if !self.initialized {
            for (i, target) in self.targets.iter().enumerate() {
                for filters in &mut self.filters {
                    filters[i].b = target.b;
                    filters[i].a = target.a;
                }
            }
            self.filter_frames = 0;
            self.pedal_blend = if pedal > 0 { 1. } else { 0. };
            self.amp_blend = if amp > 0 { 1. } else { 0. };
            self.cab_blend = if cabinet > 0 { 1. } else { 0. };
            self.mod_weights = [0.; 3];
            self.mod_weights[(v[18] as usize).min(2)] = 1.;
            self.delay_frames = v[23] * 48.;
            self.initialized = true;
        }
        self.dirty = false;
    }
    fn set_filter(&mut self, index: usize, hz: f32, q: f32, gain: f32, shape: Shape) {
        let previous = self.targets[index];
        self.targets[index].configure(hz, q, gain, shape);
        if previous.b != self.targets[index].b || previous.a != self.targets[index].a {
            self.filter_frames = 480;
        }
    }
    fn filter(&mut self, x: [f32; 2], from: usize, to: usize) -> [f32; 2] {
        let mut out = x;
        for (c, value) in out.iter_mut().enumerate() {
            for filter in &mut self.filters[c][from..to] {
                *value = filter.sample(*value as f64) as f32;
            }
        }
        out
    }
    pub fn sample(&mut self, input: [f32; 2]) -> [f32; 2] {
        if self.dirty && self.tick == 0 {
            self.controls();
        }
        self.tick = (self.tick + 1) % 32;
        if self.filter_frames > 0 {
            let step = 1. / self.filter_frames as f64;
            for (i, target) in self.targets.iter().enumerate() {
                for k in 0..3 {
                    self.filters[0][i].b[k] += (target.b[k] - self.filters[0][i].b[k]) * step;
                }
                for k in 0..2 {
                    self.filters[0][i].a[k] += (target.a[k] - self.filters[0][i].a[k]) * step;
                }
                self.filters[1][i].b = self.filters[0][i].b;
                self.filters[1][i].a = self.filters[0][i].a;
            }
            self.filter_frames -= 1;
        }
        if self.drive_frames > 0 {
            let step = 1. / self.drive_frames as f32;
            self.pedal_values[0] += (self.drive_targets[0] - self.pedal_values[0]) * step;
            self.pedal_values[6] += (self.drive_targets[1] - self.pedal_values[6]) * step;
            self.amp_values[0] += (self.drive_targets[2] - self.amp_values[0]) * step;
            self.amp_values[6] += (self.drive_targets[3] - self.amp_values[6]) * step;
            self.drive_frames -= 1;
        }
        let input = input.map(|v| if v.is_finite() { v } else { 0. });
        let v = self.values;
        self.last_dry = self.dry[self.dry_index];
        self.dry[self.dry_index] = input;
        self.dry_index = (self.dry_index + 1) % LATENCY;
        let mut x = input.map(|x| x.clamp(-64., 64.) * self.input_gain);
        let peak = x[0].abs().max(x[1].abs());
        self.envelope += if peak > self.envelope { 0.02 } else { 0.00014 } * (peak - self.envelope);
        if self.envelope >= self.gate_threshold {
            self.hold = 1200;
        } else {
            self.hold = self.hold.saturating_sub(1);
        }
        let target = if v[1] <= -89.9 || self.hold > 0 {
            1.
        } else {
            (self.envelope / self.gate_threshold).min(1.).powi(2)
        };
        self.gate_gain +=
            if target > self.gate_gain { 0.02 } else { 0.001 } * (target - self.gate_gain);
        for value in &mut x {
            *value *= self.gate_gain;
        }
        glide(&mut self.pedal_blend, if v[2] >= 0.5 { 1. } else { 0. });
        glide(&mut self.amp_blend, if v[6] >= 0.5 { 1. } else { 0. });
        glide(&mut self.cab_blend, if v[14] >= 0.5 { 1. } else { 0. });
        let filtered = self.filter(x, 0, 1);
        for c in 0..2 {
            x[c] += (filtered[c] - x[c]) * self.pedal_blend;
        }
        self.pedal_values[2] = self.pedal_blend;
        self.pedal_values[3] = v[5] * self.pedal_blend;
        x = self.pedal.sample(x, &self.pedal_values);
        let sag = 1. / (1. + self.envelope * v[12] * 3. * self.amp_blend);
        for value in &mut x {
            *value *= sag;
        }
        self.amp_values[2] = self.amp_blend;
        x = self.amp.sample(x, &self.amp_values);
        let toned = self.filter(x, 1, 4);
        let master = 1. + (self.master_gain - 1.) * self.amp_blend;
        for c in 0..2 {
            x[c] = (x[c] + (toned[c] - x[c]) * self.amp_blend) * master;
        }
        let cabinet = self.filter(x, 4, 10);
        for c in 0..2 {
            x[c] += (cabinet[c] - x[c]) * self.cab_blend;
        }
        self.chorus_values[5] = if v[18] == 1. || self.mod_weights[1] > 0. {
            v[21]
        } else {
            0.
        };
        let chorus = self.chorus.sample(x, &self.chorus_values);
        let tremolo = if v[18] == 2. || self.mod_weights[2] > 0. {
            1. - v[20] * ((self.phase.sin() + 1.) * 0.5) as f32 * v[21]
        } else {
            1.
        };
        self.phase = (self.phase + 2. * std::f64::consts::PI * v[19] as f64 / 48000.)
            % (2. * std::f64::consts::PI);
        for i in 0..3 {
            glide(
                &mut self.mod_weights[i],
                if i == v[18] as usize { 1. } else { 0. },
            );
        }
        for c in 0..2 {
            x[c] = x[c] * self.mod_weights[0]
                + chorus[c] * self.mod_weights[1]
                + x[c] * tremolo * self.mod_weights[2];
        }
        self.delay_frames += (v[23] * 48. - self.delay_frames) * 0.001;
        let position = self.delay_index as f32 - self.delay_frames;
        let floor = position.floor() as i64;
        let first = floor.rem_euclid(self.delay.len() as i64) as usize;
        let fraction = position - floor as f32;
        for (c, value) in x.iter_mut().enumerate() {
            let wet = self.delay[first][c] * (1. - fraction)
                + self.delay[(first + 1) % self.delay.len()][c] * fraction;
            self.delay_filtered[c] += self.delay_tone * (wet - self.delay_filtered[c]);
            self.delay[self.delay_index][c] = *value + self.delay_filtered[c] * v[24];
            *value += self.delay_filtered[c] * v[25];
        }
        self.delay_index = (self.delay_index + 1) % self.delay.len();
        let room = self.room.sample_dense(x, &self.room_values);
        for c in 0..2 {
            x[c] = (x[c] + (room[c] - x[c]) * v[27]) * self.output_gain;
        }
        for (c, value) in x.iter_mut().enumerate() {
            *value = self.last_dry[c] * (1. - v[29]) + *value * v[29];
        }
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn values() -> [f32; 30] {
        let descriptors = crate::render::descriptors("guitarRig");
        std::array::from_fn(|i| descriptors[i].default)
    }
    fn signal(i: usize) -> [f32; 2] {
        let t = i as f64 / 48000.;
        [
            (0.17 * (t * 2. * std::f64::consts::PI * 220.).sin()
                + 0.03 * (t * 2. * std::f64::consts::PI * 1760.).sin()) as f32,
            (0.13 * (t * 2. * std::f64::consts::PI * 330.).sin()) as f32,
        ]
    }
    #[test]
    fn empty_rig_and_dry_mix_are_exactly_latency_aligned() {
        for dry_mix in [false, true] {
            let mut v = values();
            v[1] = -90.;
            v[2] = 0.;
            v[6] = 0.;
            v[14] = 0.;
            v[18] = 0.;
            v[25] = 0.;
            v[27] = 0.;
            if dry_mix {
                v = values();
                v[29] = 0.;
            }
            let mut rig = GuitarRig::default();
            rig.configure(&v);
            for i in 0..2000 {
                let out = rig.sample(signal(i));
                let expected = if i >= LATENCY {
                    signal(i - LATENCY)
                } else {
                    [0.; 2]
                };
                assert_eq!(out, expected);
            }
        }
    }
    #[test]
    fn dry_right_channel_stays_silent_without_spatial_effects() {
        let mut v = values();
        v[2] = 3.;
        v[6] = 4.;
        v[14] = 4.;
        v[27] = 0.;
        let mut rig = GuitarRig::default();
        rig.configure(&v);
        let mut energy = 0.;
        for i in 0..6000 {
            let out = rig.sample([signal(i)[0], 0.]);
            assert_eq!(out[1], 0.);
            assert!(out[0].is_finite());
            energy += out[0] * out[0];
        }
        assert!(energy > 1.);
    }
    #[test]
    fn cloning_keeps_independent_delay_modulation_and_room_histories() {
        let mut v = values();
        v[18] = 1.;
        v[23] = 20.;
        v[25] = 0.4;
        v[27] = 0.3;
        let mut rig = GuitarRig::default();
        rig.configure(&v);
        for i in 0..12000 {
            rig.sample(signal(i));
        }
        let mut copy = rig.clone();
        for i in 12000..15000 {
            assert_eq!(rig.sample(signal(i)), copy.sample(signal(i)));
        }
        v[14] = 0.;
        copy.configure(&v);
        let a = rig.sample([0.; 2]);
        let _ = copy.sample([1.; 2]);
        assert!(a.iter().all(|v| v.is_finite()));
    }
    #[test]
    fn cross_engine_reference_and_all_factory_rigs_are_finite() {
        let catalog: serde_json::Value =
            serde_json::from_str(include_str!("../../../server/catalog/native-v3.json")).unwrap();
        let metadata = catalog["capabilities"]["effects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == "guitarRig")
            .unwrap();
        let mut references = Vec::new();
        for preset in metadata["factoryPresets"].as_array().unwrap() {
            let mut v = values();
            for (i, d) in crate::render::descriptors("guitarRig").iter().enumerate() {
                v[i] = preset["values"][d.key].as_f64().unwrap() as f32;
            }
            let mut rig = GuitarRig::default();
            rig.configure(&v);
            let mut samples = Vec::new();
            let mut peak = 0_f32;
            let mut energy = 0_f64;
            for i in 0..4096 + LATENCY {
                let out = rig.sample(if i < 4096 { signal(i) } else { [0.; 2] });
                assert!(out.iter().all(|v| v.is_finite()));
                peak = peak.max(out[0].abs()).max(out[1].abs());
                energy += out[0] as f64 * out[0] as f64;
                if i >= LATENCY && (i - LATENCY) % 64 == 0 {
                    samples.push(out);
                }
            }
            assert!(peak < 2., "Preset {} peak {peak}", preset["name"]);
            assert!(energy > 0.01, "Preset must not mute the instrument");
            references.push(serde_json::json!({"name":preset["name"],"values":preset["values"],"samples":samples,"peak":peak}));
        }
        let reference = serde_json::json!({"frames":4096,"stride":64,"presets":references});
        if std::env::var_os("SOUNDROOM_UPDATE_GUITAR_GOLDEN").is_some() {
            std::fs::write(
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/guitar-rig-reference.json"
                ),
                serde_json::to_string_pretty(&reference).unwrap(),
            )
            .unwrap();
        } else {
            let stored: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/guitar-rig-reference.json"
                ))
                .unwrap(),
            )
            .unwrap();
            assert_eq!(reference["frames"], stored["frames"]);
            let actual = reference["presets"].as_array().unwrap();
            let expected = stored["presets"].as_array().unwrap();
            assert_eq!(actual.len(), expected.len());
            for (a, b) in actual.iter().zip(expected) {
                assert_eq!(a["name"], b["name"]);
                assert_eq!(a["values"], b["values"]);
                for (a, b) in a["samples"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .zip(b["samples"].as_array().unwrap())
                {
                    for channel in 0..2 {
                        assert!(
                            (a[channel].as_f64().unwrap() - b[channel].as_f64().unwrap()).abs()
                                < 0.000002
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod adversarial_tests {
    use super::*;
    #[test]
    fn repeated_model_switches_and_nonfinite_input_leave_bounded_audio() {
        let mut v: [f32; 30] =
            std::array::from_fn(|i| crate::render::descriptors("guitarRig")[i].default);
        v[27] = 0.;
        let mut rig = GuitarRig::default();
        rig.configure(&v);
        let mut previous = 0.;
        let mut largest_jump = 0_f32;
        for i in 0..12000 {
            if i % 257 == 0 {
                v[2] = (i / 257 % 4) as f32;
                v[6] = (i / 257 % 5) as f32;
                v[14] = ((i / 257 + 2) % 5) as f32;
                rig.configure(&v);
            }
            let out =
                rig.sample([0.04 * (i as f32 * std::f32::consts::TAU * 100. / 48000.).sin(); 2]);
            assert!(out.iter().all(|v| v.is_finite() && v.abs() < 4.));
            largest_jump = largest_jump.max((out[0] - previous).abs());
            previous = out[0];
        }
        println!(
            "Largest adjacent sample jump during repeated amp/pedal/cab edits: {largest_jump}"
        );
        assert!(largest_jump < 0.1);
        for i in 0..2000 {
            assert!(rig
                .sample(if i == 0 {
                    [f32::NAN, f32::INFINITY]
                } else {
                    [0.; 2]
                })
                .iter()
                .all(|v| v.is_finite()));
        }
    }
}
