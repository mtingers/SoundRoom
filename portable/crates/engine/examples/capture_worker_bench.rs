//! Pure offline conversion benchmark. No devices, timestamp calls or playback.
use soundroom_engine::capture_resample::{CaptureResampler, ResampleConfig};
use std::time::Instant;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    for (rate, channels) in [(44100, 2), (48000, 2), (96000, 2), (192000, 8), (384000, 2)] {
        let prepared = Instant::now();
        let mut converter = CaptureResampler::new(
            ResampleConfig {
                channels,
                input_rate: rate,
                output_rate: 48000,
                maximum_drift_ppm: 2000.,
            },
            0,
            0.,
        )?;
        let prepare_seconds = prepared.elapsed().as_secs_f64();
        let input: Vec<Vec<f32>> = (0..channels)
            .map(|channel| {
                (0..4096)
                    .map(|i| ((i + channel) as f32 * 0.07).sin() * 0.2)
                    .collect()
            })
            .collect();
        let output: Vec<Vec<f32>> = vec![vec![0.; 8192]; channels];
        let mut output = output;
        let mut refs: Vec<&mut [f32]> = output.iter_mut().map(|c| c.as_mut_slice()).collect();
        let target = rate as usize * 4;
        let start = Instant::now();
        let mut rendered = 0;
        for frame in (0..target).step_by(4096) {
            let frames = (target - frame).min(4096);
            let input_refs: Vec<&[f32]> = input.iter().map(|c| &c[..frames]).collect();
            converter.push(frame as u64, &input_refs)?;
            loop {
                let block = converter.drain(&mut refs)?;
                rendered += block.frames;
                if block.frames == 0 {
                    break;
                }
            }
        }
        converter.finish();
        loop {
            let block = converter.drain(&mut refs)?;
            rendered += block.frames;
            if block.frames == 0 {
                break;
            }
        }
        let seconds = start.elapsed().as_secs_f64();
        let stats = converter.stats();
        println!("rate={rate} channels={channels} prepared={prepare_seconds:.6}s conversion={seconds:.6}s audio=4s throughput={:.2}x output_frames={rendered} pcm_bytes={} filter_bytes={}",4./seconds,stats.pcm_bytes,stats.filter_bytes);
    }
    Ok(())
}
