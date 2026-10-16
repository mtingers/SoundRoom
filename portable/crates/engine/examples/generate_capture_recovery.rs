//! Pure file/clock fixture generator. It never opens or enumerates audio devices.
use soundroom_engine::{
    capture::{CaptureTiming, CapturedBlock},
    capture_archive::{ArchiveManifest, CaptureArchive, CaptureEvent},
    capture_recovery::{recover_take, RecoveryOutcome, RecoveryPlan, RecoveryRange, RecoveryRoute},
};
use std::{fs, path::PathBuf, sync::atomic::AtomicBool};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("Expected a NEW fixture directory")?,
    );
    let root = if root.is_absolute() {
        root
    } else {
        std::env::current_dir()?.join(root)
    };
    let custody = soundroom_platform::PrivateDirectory::create_root(&root)?;
    for name in ["continuous", "gap", "loop-punch"] {
        let destination = root.join(name);
        custody.create_dir(name)?;
        let raw = destination.join("raw");
        let mut archive = CaptureArchive::create(
            &raw,
            ArchiveManifest {
                version: 1,
                take_id: "11223344-5566-7788-9900-aabbccddeeff".into(),
                device_uid: "simulated-reference-only".into(),
                channels: 2,
                sample_rate: 48000,
                maximum_frames: 512,
                host_ticks_per_second: 1e9,
                calibration_offset_frames: 0,
            },
        )?;
        let mut consumed = 0;
        for packet in 0..8u64 {
            let frame = packet * 512 + if name == "gap" && packet >= 4 { 512 } else { 0 };
            let host = 1_000_000_000_000_000_000 + (frame as f64 * 1e9 / 48000.).round() as u64;
            archive.event(CaptureEvent::OutputAnchor {
                source_frame: Some(consumed),
                epoch: 1,
                playback_clock: 40000 + frame,
                frames: 512,
                sample_time_bits: (100000. + frame as f64).to_bits(),
                host_time: host,
                flags: 3,
            })?;
            let left: Vec<f32> = (frame..frame + 512)
                .map(|i| ((i * 17 % 2048) as i32 - 1024) as f32 / 8192.)
                .collect();
            let right: Vec<f32> = left.iter().map(|&v| v * -0.5).collect();
            archive.append(
                CapturedBlock {
                    source_frame: frame,
                    frames: 512,
                    timing: CaptureTiming {
                        sample_time: 500000. + frame as f64,
                        host_time: host,
                        flags: 3,
                    },
                },
                &[&left, &right],
            )?;
            consumed = frame + 512;
        }
        archive.finish()?;
        let plan = RecoveryPlan {
            version: 1,
            name: "Reference capture".into(),
            routes: vec![
                RecoveryRoute {
                    track_id: "00112233-4455-6677-8899-AABBCCDDEEFF".into(),
                    channels: vec![0, 1],
                },
                RecoveryRoute {
                    track_id: "FFEEDDCC-BBAA-9988-7766-554433221100".into(),
                    channels: vec![1],
                },
            ],
            range: (name == "loop-punch").then_some(RecoveryRange {
                start: 40256,
                end: 40768,
            }),
            loop_range: (name == "loop-punch").then_some(RecoveryRange {
                start: 40000,
                end: 41024,
            }),
            maximum_drift_ppm: 2000.,
            allow_incomplete: name == "gap",
        };
        fs::write(
            destination.join("plan.json"),
            serde_json::to_vec_pretty(&plan)?,
        )?;
        let artifacts = destination.join("artifacts");
        let RecoveryOutcome::Committed(report) =
            recover_take(&raw, &artifacts, &plan, &AtomicBool::new(false))?
        else {
            unreachable!()
        };
        fs::write(
            destination.join("expected.json"),
            serde_json::to_vec_pretty(&report)?,
        )?;
        fs::remove_dir_all(artifacts)?;
        println!(
            "{name}: {} validated WAV references, {} take references",
            report.assets.len(),
            report.takes.len()
        );
    }
    Ok(())
}
