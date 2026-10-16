//! Acceptance harness only, never a product mode: the real portable service and
//! verified TLS endpoint with a manually pumped, timestamped *simulated* callback.
//! It opens no audio device. Browser live-Write acceptance (web/tests/live) uses it
//! to exercise real wire, authority, driver and callback receipts end to end.
//!
//! write_acceptance_host ROOT IDENTITY.p12 PROJECT.soundroom CREDENTIAL-OUT.json
//! Password: SOUNDROOM_TEST_TLS_PASSWORD. Prints {"port":N} and serves until stdin closes.
use soundroom_client::models::RemoteRole;
use soundroom_server::{
    endpoint::{Endpoint, EndpointConfig},
    registry::Registry,
    service::Service,
};
use std::{
    io::{Read, Write},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
fn main() -> Result<(), soundroom_client::Error> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, identity, project, credential_out] = args.as_slice() else {
        eprintln!("write_acceptance_host ROOT IDENTITY.p12 PROJECT.soundroom CREDENTIAL-OUT.json");
        std::process::exit(2);
    };
    let password = std::env::var("SOUNDROOM_TEST_TLS_PASSWORD").unwrap_or_default();
    Registry::create(Path::new(root), "Write acceptance")?;
    let service = Service::open(Path::new(root), Some(Path::new(project)))?;
    let credential = service
        .lock()
        .unwrap()
        .registry
        .issue("Browser acceptance", RemoteRole::Operator)?;
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(credential_out)?;
        file.write_all(&serde_json::to_vec(&credential)?)?;
    }
    let endpoint = Endpoint::bind(
        EndpointConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            approved_origins: vec!["https://localhost".into()],
        },
        Path::new(identity),
        &password,
    )?;
    let (control, mut host) = soundroom_engine::realtime::channel();
    Service::attach_audio(&service, control)?;
    service.lock().unwrap().enable_automation_write();
    let stop = Arc::new(AtomicBool::new(false));
    let callback_stop = stop.clone();
    // Simulated 48 kHz/128-frame callback at real-time pace with valid timestamps.
    let callback = std::thread::spawn(move || {
        let mut left = [0.; 128];
        let mut right = [0.; 128];
        let mut frame = 0u64;
        while !callback_stop.load(Ordering::Acquire) {
            host.render_timed(
                &mut left,
                &mut right,
                soundroom_engine::capture::CaptureTiming {
                    sample_time: frame as f64,
                    host_time: 1_000_000_000_000 + frame * 20_833,
                    flags: 3,
                },
            );
            frame += 128;
            std::thread::sleep(Duration::from_micros(2_667));
        }
    });
    println!("{{\"port\":{}}}", endpoint.address()?.port());
    std::io::stdout().flush()?;
    let stdin_stop = stop.clone();
    std::thread::spawn(move || {
        let mut sink = [0; 64];
        while matches!(std::io::stdin().read(&mut sink), Ok(n) if n > 0) {}
        stdin_stop.store(true, Ordering::Release);
    });
    let result = endpoint.run(service, stop.clone());
    stop.store(true, Ordering::Release);
    let _ = callback.join();
    result
}
