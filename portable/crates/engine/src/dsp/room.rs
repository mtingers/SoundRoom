/// Original four-comb/two-allpass legacy room network, kept warm alongside the dense room.
#[derive(Clone)]
struct Classic {
    lines: Vec<Vec<f32>>,
    positions: [usize; 4],
    damping: [f32; 4],
    allpasses: Vec<Vec<f32>>,
    all_positions: [usize; 2],
    previous_decay: f32,
    previous_tone: f32,
    coefficient: f32,
    feedback: [f32; 4],
}
impl Classic {
    fn new(offset: usize) -> Self {
        Self {
            lines: [1423, 2137, 3163, 4789]
                .iter()
                .map(|v| vec![0.; v + offset])
                .collect(),
            positions: [0; 4],
            damping: [0.; 4],
            allpasses: [347, 113].iter().map(|v| vec![0.; v + offset]).collect(),
            all_positions: [0; 2],
            previous_decay: -1.,
            previous_tone: -1.,
            coefficient: 0.,
            feedback: [0.; 4],
        }
    }
    fn sample(&mut self, x: f32, decay: f32, tone: f32) -> f32 {
        if tone != self.previous_tone {
            self.coefficient = 1. - (-2. * std::f32::consts::PI * tone / 48000.).exp();
            self.previous_tone = tone;
        }
        if decay != self.previous_decay {
            for i in 0..4 {
                self.feedback[i] = 10_f32.powf(-3. * self.lines[i].len() as f32 / (48000. * decay));
            }
            self.previous_decay = decay;
        }
        let mut y = 0.;
        for i in 0..4 {
            let delayed = self.lines[i][self.positions[i]];
            self.damping[i] += self.coefficient * (delayed - self.damping[i]);
            self.lines[i][self.positions[i]] = x + self.damping[i] * self.feedback[i];
            self.positions[i] = (self.positions[i] + 1) % self.lines[i].len();
            y += delayed * 0.25;
        }
        for i in 0..2 {
            let delayed = self.allpasses[i][self.all_positions[i]];
            let output = delayed - y * 0.5;
            self.allpasses[i][self.all_positions[i]] = y + output * 0.5;
            self.all_positions[i] = (self.all_positions[i] + 1) % self.allpasses[i].len();
            y = output;
        }
        y
    }
}
#[derive(Clone)]
pub struct Room {
    classic: [Classic; 2],
    lines: Vec<Vec<f32>>,
    positions: [usize; 8],
    filtered: [f32; 8],
    feedback: [f32; 8],
    scratch: [f32; 8],
    diffusion_lines: Vec<Vec<f32>>,
    diffusion_positions: [usize; 6],
    early: [Vec<f32>; 2],
    early_position: usize,
    low: [f32; 2],
    tone: f32,
    low_cut: f32,
    diffusion: f32,
    width: f32,
    early_balance: f32,
    pub output_gain: f32,
    blend: f32,
    target: f32,
    configured: bool,
    previous: [f32; 3],
}
impl Default for Room {
    fn default() -> Self {
        Self {
            classic: [Classic::new(0), Classic::new(23)],
            lines: [1499, 1601, 1867, 2053, 2251, 2399, 2791, 3163]
                .iter()
                .map(|v| vec![0.; *v])
                .collect(),
            positions: [0; 8],
            filtered: [0.; 8],
            feedback: [0.; 8],
            scratch: [0.; 8],
            diffusion_lines: [211, 397, 683, 227, 419, 719]
                .iter()
                .map(|v| vec![0.; *v])
                .collect(),
            diffusion_positions: [0; 6],
            early: [vec![0.; 3200], vec![0.; 3200]],
            early_position: 0,
            low: [0.; 2],
            tone: 0.,
            low_cut: 0.,
            diffusion: 0.,
            width: 1.,
            early_balance: 0.,
            output_gain: 1.,
            blend: 0.,
            target: 0.,
            configured: false,
            previous: [-1.; 3],
        }
    }
}
impl Room {
    pub fn configure(&mut self, v: &[f32]) {
        if v[0] != self.previous[0] {
            for i in 0..8 {
                self.feedback[i] = 10_f32.powf(-3. * self.lines[i].len() as f32 / (48000. * v[0]));
            }
            self.previous[0] = v[0];
        }
        if v[2] != self.previous[1] {
            self.tone = 1. - (-2. * std::f32::consts::PI * v[2] / 48000.).exp();
            self.previous[1] = v[2];
        }
        if v[5] != self.previous[2] {
            self.low_cut = if v[5] == 0. {
                0.
            } else {
                1. - (-2. * std::f32::consts::PI * v[5] / 48000.).exp()
            };
            self.previous[2] = v[5];
        }
        self.diffusion = 0.35 + v[6] * 0.4;
        self.width = v[7];
        self.early_balance = v[8];
        self.output_gain = 10_f32.powf(v[9] / 20.);
        self.target = v[4];
        if !self.configured {
            self.blend = self.target;
            self.configured = true;
        }
    }
    /// x has already passed through the shared predelay. Classic and dense networks always advance.
    pub fn sample(&mut self, x: [f32; 2], v: &[f32]) -> [f32; 2] {
        self.sample_with_classic(x, v, true)
    }
    /// Dedicated dense-only users never switch to Classic; omit its two networks.
    pub fn sample_dense(&mut self, x: [f32; 2], v: &[f32]) -> [f32; 2] {
        self.sample_with_classic(x, v, false)
    }
    fn sample_with_classic(&mut self, x: [f32; 2], v: &[f32], keep_classic: bool) -> [f32; 2] {
        let classic = if keep_classic {
            [
                self.classic[0].sample(x[0], v[0], v[2]),
                self.classic[1].sample(x[1], v[0], v[2]),
            ]
        } else {
            [0.; 2]
        };
        let mut input = [0.; 2];
        for c in 0..2 {
            self.low[c] += self.low_cut * (x[c] - self.low[c]);
            input[c] = if self.low_cut == 0. {
                x[c]
            } else {
                x[c] - self.low[c]
            };
            self.early[c][self.early_position] = input[c];
        }
        let lt = [173, 431, 809, 1237, 1861, 2741];
        let rt = [239, 557, 947, 1459, 2081, 3011];
        let gains = [0.38, -0.31, 0.25, 0.20, -0.15, 0.11];
        let mut reflection = [0.; 2];
        for i in 0..6 {
            let l = (self.early_position + 3200 - lt[i]) % 3200;
            let r = (self.early_position + 3200 - rt[i]) % 3200;
            reflection[0] += (self.early[0][l] * 0.8 + self.early[1][r] * 0.2) * gains[i];
            reflection[1] += (self.early[1][r] * 0.8 + self.early[0][l] * 0.2) * gains[i];
        }
        self.early_position = (self.early_position + 1) % 3200;
        let mut diffuse = input;
        for i in 0..6 {
            let c = if i < 3 { 0 } else { 1 };
            let input = diffuse[c];
            let delayed = self.diffusion_lines[i][self.diffusion_positions[i]];
            let result = delayed - self.diffusion * input;
            self.diffusion_lines[i][self.diffusion_positions[i]] = input + self.diffusion * result;
            self.diffusion_positions[i] =
                (self.diffusion_positions[i] + 1) % self.diffusion_lines[i].len();
            diffuse[c] = result;
        }
        for i in 0..8 {
            let delayed = self.lines[i][self.positions[i]];
            self.filtered[i] += self.tone * (delayed - self.filtered[i]);
            self.scratch[i] = self.filtered[i] * self.feedback[i];
        }
        let s = self.scratch;
        let late = [
            (s[0] + s[1] - s[2] - s[3] + s[4] + s[5] - s[6] - s[7]) * 0.35355339,
            (s[0] - s[1] + s[2] - s[3] + s[4] - s[5] + s[6] - s[7]) * 0.35355339,
        ];
        let mut stride = 1;
        while stride < 8 {
            let mut base = 0;
            while base < 8 {
                for j in 0..stride {
                    let a = self.scratch[base + j];
                    let b = self.scratch[base + j + stride];
                    self.scratch[base + j] = a + b;
                    self.scratch[base + j + stride] = a - b;
                }
                base += stride * 2;
            }
            stride *= 2;
        }
        for i in 0..8 {
            let sign_l = if i & 2 == 0 { 1. } else { -1. };
            let sign_r = if i & 1 == 0 { 1. } else { -1. };
            self.lines[i][self.positions[i]] =
                self.scratch[i] * 0.35355339 + (diffuse[0] * sign_l + diffuse[1] * sign_r) * 0.25;
            self.positions[i] = (self.positions[i] + 1) % self.lines[i].len();
        }
        let wet = [
            reflection[0] * self.early_balance + late[0] * (1. - self.early_balance),
            reflection[1] * self.early_balance + late[1] * (1. - self.early_balance),
        ];
        let mid = (wet[0] + wet[1]) * 0.5;
        let side = (wet[0] - wet[1]) * 0.5 * self.width;
        if self.blend < self.target {
            self.blend = (self.blend + 1. / 240.).min(self.target)
        } else if self.blend > self.target {
            self.blend = (self.blend - 1. / 240.).max(self.target)
        }
        [
            classic[0] * (1. - self.blend) + (mid + side) * self.blend,
            classic[1] * (1. - self.blend) + (mid - side) * self.blend,
        ]
    }
}
