// Preserve the exact published dyadic coefficients used by the Swift reference.
#[allow(clippy::excessive_precision)]
const COEFFICIENTS: [[f32; 12]; 4] = [
    [
        0.001708984375,
        0.010986328125,
        -0.0196533203125,
        0.033203125,
        -0.0594482421875,
        0.1373291015625,
        0.97216796875,
        -0.102294921875,
        0.047607421875,
        -0.026611328125,
        0.014892578125,
        -0.00830078125,
    ],
    [
        -0.0291748046875,
        0.029296875,
        -0.0517578125,
        0.089111328125,
        -0.16650390625,
        0.465087890625,
        0.77978515625,
        -0.2003173828125,
        0.1015625,
        -0.0582275390625,
        0.0330810546875,
        -0.0189208984375,
    ],
    [
        -0.0189208984375,
        0.0330810546875,
        -0.0582275390625,
        0.1015625,
        -0.2003173828125,
        0.77978515625,
        0.465087890625,
        -0.16650390625,
        0.089111328125,
        -0.0517578125,
        0.029296875,
        -0.0291748046875,
    ],
    [
        -0.00830078125,
        0.014892578125,
        -0.026611328125,
        0.047607421875,
        -0.102294921875,
        0.97216796875,
        0.1373291015625,
        -0.0594482421875,
        0.033203125,
        -0.0196533203125,
        0.010986328125,
        0.001708984375,
    ],
];
#[derive(Clone, Default)]
pub struct TruePeak {
    history: [[f32; 12]; 2],
    index: usize,
}
impl TruePeak {
    pub fn sample(&mut self, x: [f32; 2]) -> f32 {
        for (c, v) in x.iter().enumerate() {
            self.history[c][self.index] = *v;
        }
        let mut peak = x[0].abs().max(x[1].abs());
        for coeff in COEFFICIENTS {
            let mut a = [0.; 2];
            let mut cursor = self.index;
            for k in coeff {
                for (c, y) in a.iter_mut().enumerate() {
                    *y += self.history[c][cursor] * k;
                }
                cursor = (cursor + 11) % 12;
            }
            peak = peak.max(a[0].abs()).max(a[1].abs());
        }
        self.index = (self.index + 1) % 12;
        peak
    }
}
#[derive(Clone)]
pub struct Limiter {
    audio: [[f32; 512]; 2],
    dry: [[f32; 512]; 2],
    ceilings: [f32; 512],
    queue_values: [f32; 528],
    queue_frames: [i64; 528],
    head: usize,
    tail: usize,
    cursor: usize,
    frame: i64,
    input_gain: f32,
    ceiling: f32,
    release: f32,
    adaptive: bool,
    true_peak: bool,
    gain: f32,
    input_detector: TruePeak,
    output_detector: TruePeak,
    pub loudness: super::loudness::Loudness,
    pub reduction_db: f32,
    pub output_true_peak: f32,
    pub last_dry: [f32; 2],
}
impl Default for Limiter {
    fn default() -> Self {
        Self {
            audio: [[0.; 512]; 2],
            dry: [[0.; 512]; 2],
            ceilings: [1.; 512],
            queue_values: [0.; 528],
            queue_frames: [0; 528],
            head: 0,
            tail: 0,
            cursor: 0,
            frame: 0,
            input_gain: 1.,
            ceiling: 0.89125094,
            release: 0.00014,
            adaptive: true,
            true_peak: true,
            gain: 1.,
            input_detector: TruePeak::default(),
            output_detector: TruePeak::default(),
            loudness: Default::default(),
            reduction_db: 0.,
            output_true_peak: 0.,
            last_dry: [0.; 2],
        }
    }
}
impl Limiter {
    pub fn configure(&mut self, v: &[f32]) {
        self.input_gain = 10_f32.powf(v[0] / 20.);
        self.ceiling = 10_f32.powf(v[1] / 20.);
        self.release = 1. - (-1. / (v[2] * 0.001 * 48000.)).exp();
        self.adaptive = v[3] >= 0.5;
        self.true_peak = v[4] >= 0.5;
    }
    pub fn sample(&mut self, x: [f32; 2]) -> [f32; 2] {
        let delayed = [self.audio[0][self.cursor], self.audio[1][self.cursor]];
        let boosted = [x[0] * self.input_gain, x[1] * self.input_gain];
        let delayed_ceiling = self.ceilings[self.cursor];
        for (c, value) in x.iter().enumerate() {
            self.last_dry[c] = self.dry[c][self.cursor];
            self.dry[c][self.cursor] = *value;
            self.audio[c][self.cursor] = boosted[c];
        }
        self.ceilings[self.cursor] = self.ceiling;
        self.cursor = (self.cursor + 1) % 512;
        let reconstructed = self.input_detector.sample(boosted);
        let peak = if self.true_peak {
            reconstructed / self.ceiling.max(1e-6)
        } else {
            boosted[0].abs().max(boosted[1].abs()) / self.ceiling.max(1e-6)
        };
        while self.head != self.tail {
            let previous = (self.tail + 527) % 528;
            if self.queue_values[previous] > peak {
                break;
            }
            self.tail = previous;
        }
        self.queue_values[self.tail] = peak;
        self.queue_frames[self.tail] = self.frame;
        self.tail = (self.tail + 1) % 528;
        while self.head != self.tail && self.queue_frames[self.head] < self.frame - 524 {
            self.head = (self.head + 1) % 528;
        }
        let margin = if self.true_peak { 0.96605086 } else { 1. };
        let target = (margin / self.queue_values[self.head].max(1e-12)).min(1.);
        let coeff = if target < self.gain {
            0.04
        } else {
            self.release
                * if self.adaptive {
                    if self.gain > 0.8 {
                        2.
                    } else {
                        0.65
                    }
                } else {
                    1.
                }
        };
        self.gain += coeff * (target - self.gain);
        let gain = self
            .gain
            .min(delayed_ceiling * margin / delayed[0].abs().max(delayed[1].abs()).max(1e-12));
        let out = [delayed[0] * gain, delayed[1] * gain];
        self.reduction_db = (-20. * gain.max(1e-9).log10()).max(0.);
        self.output_true_peak = self.output_detector.sample(out);
        self.loudness.sample(out);
        self.frame += 1;
        out
    }
}
