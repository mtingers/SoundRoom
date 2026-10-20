//! Transport click synthesis. The click follows the project's bar grid from
//! timeline frame zero, so count-ins, playback and recording share one phase.

/// 25 ms sine blip: 1600 Hz on the downbeat and 1100 Hz otherwise.
pub fn click(beat_in_bar: usize, within: f64) -> f32 {
    let duration = 48000. * 0.025;
    if !(0. ..duration).contains(&within) {
        return 0.;
    }
    let attack = (within / 48.).min(1.);
    let release = (1. - within / duration).max(0.).powi(3);
    let frequency = if beat_in_bar == 0 { 1600. } else { 1100. };
    ((2. * std::f64::consts::PI * frequency * within / 48000.).sin() * attack * release * 0.15)
        as f32
}

/// Frames per beat for a tempo and beat unit at the 48 kHz session rate.
pub fn beat_frames(tempo: f64, beat_unit: u32) -> f64 {
    60. / tempo.clamp(20., 300.) * 4. / beat_unit.max(1) as f64 * 48000.
}

/// One beat grid. `beat_frames` is positive and finite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub beat_frames: f64,
    pub beats_per_bar: u32,
}
impl Grid {
    pub fn new(tempo: f64, beats_per_bar: u32, beat_unit: u32) -> Self {
        Self {
            beat_frames: beat_frames(tempo, beat_unit),
            beats_per_bar: beats_per_bar.max(1),
        }
    }
    /// The click at a timeline frame; negative frames continue the grid
    /// backwards, so a count-in before frame zero keeps the same phase.
    pub fn sample(&self, frame: f64) -> f32 {
        let beat = (frame / self.beat_frames).floor();
        let bar_beat = (beat as i64).rem_euclid(self.beats_per_bar as i64) as usize;
        click(bar_beat, frame - beat * self.beat_frames)
    }
    /// Clicks for `frames` frames ending at timeline frame `end`, phase-locked
    /// to the grid the transport click continues from `end`.
    pub fn count_in(&self, frames: usize, end: u64) -> Vec<f32> {
        let first = end as f64 - frames as f64;
        (0..frames).map(|i| self.sample(first + i as f64)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grid_clicks_on_beats_and_accents_the_downbeat() {
        let grid = Grid::new(120., 4, 4);
        assert_eq!(grid.beat_frames, 24000.);
        assert_eq!(grid.sample(0.), 0.);
        assert!(grid.sample(10.) != 0.);
        assert_eq!(grid.sample(2000.), 0.);
        // Beat four of bar one and the downbeat of bar two differ in pitch.
        let third = grid.sample(72000. + 10.);
        let downbeat = grid.sample(96000. + 10.);
        assert!(third != 0. && downbeat != 0. && third != downbeat);
        assert_eq!(grid.sample(96000. + 10.), grid.sample(10.));
    }
    #[test]
    fn count_in_ends_in_phase_with_a_mid_bar_start() {
        let grid = Grid::new(120., 4, 4);
        // Start 1.5 beats into the bar: the count-in's last click lands half a
        // beat before the start, and the transport's next click half a beat after.
        let start = 36000;
        let clicks = grid.count_in(96000, start);
        let onsets: Vec<usize> = (1..clicks.len())
            .filter(|&i| clicks[i - 1] == 0. && clicks[i] != 0.)
            .collect();
        assert_eq!(onsets, [12001, 36001, 60001, 84001]);
        for (i, value) in clicks.iter().enumerate() {
            assert_eq!(*value, grid.sample(start as f64 - 96000. + i as f64));
        }
        // The count-in's first click is beat three of an earlier bar, and the
        // downbeat at timeline zero keeps its accent.
        assert_eq!(clicks[12001], grid.sample(48001.));
        assert_eq!(clicks[60001], grid.sample(1.));
    }
}
