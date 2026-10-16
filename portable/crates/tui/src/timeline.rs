use soundroom_client::models::Clip;
/// Rebuilt on metadata installation, never on playhead frames. Prefix maxima keep
/// overlapping long clips visible while skipping clips entirely left of view.
#[derive(Default)]
pub struct ClipIndex {
    entries: Vec<(i64, i64, usize)>,
    prefix_end: Vec<i64>,
}
impl ClipIndex {
    pub fn new(clips: &[Clip]) -> Self {
        let mut entries: Vec<_> = clips
            .iter()
            .enumerate()
            .map(|(i, c)| (c.start, c.start.saturating_add(c.length), i))
            .collect();
        entries.sort_unstable_by_key(|e| (e.0, e.2));
        let mut end = i64::MIN;
        let prefix_end = entries
            .iter()
            .map(|e| {
                end = end.max(e.1);
                end
            })
            .collect();
        Self {
            entries,
            prefix_end,
        }
    }
    pub fn visible<'a>(
        &'a self,
        clips: &'a [Clip],
        start: i64,
        end: i64,
    ) -> impl Iterator<Item = &'a Clip> {
        let first = self.prefix_end.partition_point(|e| *e <= start);
        self.entries[first..]
            .iter()
            .take_while(move |entry| entry.0 < end)
            .filter(move |entry| entry.1 > start)
            .filter_map(move |entry| clips.get(entry.2))
    }
}
/// Max-envelope overview supplied by the audio host, including source offset.
pub fn peak(asset: &soundroom_client::models::RemoteAsset, start: i64, end: i64) -> Option<f32> {
    if asset.peaks.is_empty() || end <= start {
        return None;
    }
    let bin = asset
        .frames_per_bin
        .unwrap_or_else(|| (asset.frames + asset.peaks.len() as i64 - 1) / asset.peaks.len() as i64)
        .max(1);
    let first = (start - asset.waveform_start_frame).div_euclid(bin).max(0) as usize;
    let last = ((end - asset.waveform_start_frame + bin - 1)
        .div_euclid(bin)
        .max(0) as usize)
        .min(asset.peaks.len());
    if first >= last {
        return None;
    }
    Some(
        asset.peaks[first..last]
            .iter()
            .copied()
            .fold(0_f32, f32::max)
            .clamp(0., 1.),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn clip(start: i64, length: i64) -> Clip {
        serde_json::from_value(serde_json::json!({"id":"00000000-0000-0000-0000-000000000001","assetID":"00000000-0000-0000-0000-000000000002","name":"clip","start":start,"offset":0,"length":length})).unwrap()
    }
    #[test]
    fn overlap_and_unsorted_clips_remain_visible() {
        let clips = vec![clip(100, 10), clip(0, 1000), clip(60, 5), clip(20, 4)];
        let index = ClipIndex::new(&clips);
        assert_eq!(
            index
                .visible(&clips, 60, 65)
                .map(|c| c.start)
                .collect::<Vec<_>>(),
            vec![0, 60]
        );
        assert_eq!(
            index
                .visible(&clips, 65, 100)
                .map(|c| c.start)
                .collect::<Vec<_>>(),
            vec![0]
        );
    }
    #[test]
    fn large_timeline_query_visits_visible_range() {
        let clips: Vec<_> = (0..100000).map(|i| clip(i * 100, 50)).collect();
        let index = ClipIndex::new(&clips);
        assert_eq!(index.visible(&clips, 9999000, 10000000).count(), 10);
    }
}
