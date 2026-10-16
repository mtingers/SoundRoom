//! Explicit opt-in real output smoke. It uses a muted session, proving backend
//! callback clocks/handoff without playing unexpected sound. Not a load/listening test.
#[cfg(target_os = "macos")]
fn run() -> soundroom_engine::Result<String> {
    use soundroom_engine::{
        core_audio::CoreAudioOutput, project::Project, realtime, render::PreparedGraph,
    };
    use std::{
        path::Path,
        sync::atomic::AtomicBool,
        time::{Duration, Instant},
    };
    let source = std::env::args().nth(1).unwrap_or_else(|| {
        format!(
            "{}/../../fixtures/pitch-chain.soundroom",
            env!("CARGO_MANIFEST_DIR")
        )
    });
    let mut project = Project::open(Path::new(&source))?;
    project.session.master_track.muted = true;
    let graph =
        PreparedGraph::prepare_resident(&project, 64 * 1024 * 1024, &AtomicBool::new(false))?;
    let (mut control, host) = realtime::channel();
    control
        .publish(Box::new(graph))
        .ok()
        .expect("initial graph slot empty");
    let sequence = control.request_playing(true).unwrap();
    let mut output = CoreAudioOutput::open_default(host)?;
    output.start()?;
    let end = Instant::now() + Duration::from_secs(2);
    while Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
        control.reclaim_retired();
    }
    let before = control.telemetry().expect("coherent callback telemetry");
    assert!(before.playing && before.command_sequence == sequence);
    assert!(before.frame > 48000 && before.callback_count > 10);
    assert_eq!(before.faults, 0);
    let pause = control.request_playing(false).unwrap();
    let end = Instant::now() + Duration::from_secs(1);
    while Instant::now() < end
        && control
            .telemetry()
            .map_or(true, |v| v.command_sequence != pause)
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    let paused = control.telemetry().unwrap();
    assert_eq!(paused.command_sequence, pause);
    assert!(!paused.playing);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(control.telemetry().unwrap().frame, paused.frame);
    output.stop()?;
    Ok(format!(
        "PASS actual Core Audio DefaultOutput: {before:?}; callback-committed pause: {paused:?}"
    ))
}
#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    let result = run();
    let report = match &result {
        Ok(value) => value.clone(),
        Err(error) => format!("FAIL Core Audio smoke: {error}"),
    };
    let target = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "/private/tmp/soundroom-coreaudio-probe-report.txt".into());
    let _ = std::fs::write(target, format!("{report}\n"));
    println!("{report}");
    if result.is_ok() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("Core Audio smoke is available only on macOS");
}
