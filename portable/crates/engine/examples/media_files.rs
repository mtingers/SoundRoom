//! Offline file operations only; no device or session-authority access.
use soundroom_engine::media::{import_audio, AudioExport, AudioReader, AudioWriter};
use std::{path::PathBuf, sync::atomic::AtomicBool};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 3 && args[0] == "import" {
        let source = PathBuf::from(&args[1]);
        let channels = AudioReader::open(&source)?.info.channels;
        let route: &[usize] = if channels == 1 { &[0] } else { &[0, 1] };
        let result = import_audio(
            &source,
            &PathBuf::from(&args[2]),
            route,
            &AtomicBool::new(false),
        )?;
        println!(
            "Imported {} native channels at {} Hz into {} stereo 48 kHz frames",
            result.source.channels, result.source.sample_rate, result.frames
        );
        return Ok(());
    }
    if args.len() == 1 {
        let reader = AudioReader::open(&PathBuf::from(&args[0]))?;
        println!("{:?}", reader.info);
        return Ok(());
    }
    if args.len() == 2 && args[0] == "export-fixtures" {
        let root = PathBuf::from(&args[1]);
        std::fs::create_dir(&root)?;
        let left = [-2., -1., -0.5, 0., 0.125, 1., 2., f32::NAN];
        let right = [-0.; 8];
        for (name, format) in [
            ("pcm16", AudioExport::AiffPcm16),
            ("pcm24", AudioExport::AiffPcm24),
            ("float32", AudioExport::AiffFloat32),
        ] {
            let mut writer =
                AudioWriter::create(&root.join(format!("{name}.aiff")), 8, 48000, format)?;
            writer.write(&left, &right)?;
            writer.finish()?;
        }
        return Ok(());
    }
    Err(
        "Usage: media_files FILE | import SOURCE NEW_STEREO_WAV | export-fixtures NEW_DIRECTORY"
            .into(),
    )
}
