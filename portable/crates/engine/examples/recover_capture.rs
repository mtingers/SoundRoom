//! Offline artifact recovery only. The source must be immutable and paths owned
//! by the invoking user/service; this does not edit a running session or record.
use soundroom_engine::capture_recovery::{recover_take, RecoveryOutcome, RecoveryPlan};
use std::{fs::File, io::Read, path::PathBuf, sync::atomic::AtomicBool};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 3 {
        return Err("Usage: recover_capture RAW_ARCHIVE NEW_ARTIFACT_DIRECTORY PLAN.json".into());
    }
    let file = File::open(PathBuf::from(&args[2]))?;
    if file.metadata()?.len() > 1024 * 1024 {
        return Err("Recovery plan exceeds 1 MiB".into());
    }
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    let plan: RecoveryPlan = serde_json::from_slice(&bytes)?;
    let result = recover_take(
        &PathBuf::from(&args[0]),
        &PathBuf::from(&args[1]),
        &plan,
        &AtomicBool::new(false),
    )?;
    let (status, report) = match result {
        RecoveryOutcome::Committed(r) => ("committed", r),
        RecoveryOutcome::AlreadyPresent(r) => ("alreadyPresent", r),
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({"status":status,"report":report}))?
    );
    Ok(())
}
