use soundroom_client::{
    auth::Credential,
    client::{Client, ConnectionState},
    models::{RemoteCommand, RemoteReplyStatus},
    transport::WssConfig,
};
use std::{
    collections::BTreeMap,
    path::Path,
    time::{Duration, Instant},
};
fn main() {
    if let Err(error) = run() {
        eprintln!("WSS probe failed: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 4 {
        return Err("Usage: wss_probe <wss-url> <credential-file> <approved-name> [CA-pem] [approved-origin]".into());
    }
    let mut config = WssConfig::new(&args[1]);
    if let Some(path) = args.get(4) {
        config.root_ca_pem = Some(std::fs::read(path)?);
    }
    if let Some(origin) = args.get(5) {
        config.origin = Some(origin.clone());
    }
    let credential = Credential::load(Path::new(&args[2]))?;
    let mut client = Client::connect(&config, credential, &args[3], None)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while client.connection != ConnectionState::Ready {
        if Instant::now() >= deadline {
            return Err("Initial snapshot timed out".into());
        }
        client.poll(Duration::from_millis(50))?;
    }
    let snapshot = client.snapshot.as_ref().unwrap();
    println!(
        "Authenticated role={:?}, server={}, revision={}, tracks={}",
        client.role.as_ref().unwrap(),
        snapshot.server_id,
        snapshot.revision,
        snapshot.session.tracks.as_ref().map_or(0, Vec::len)
    );
    let reply = client.request(
        RemoteCommand::Ping { client_time: 0. },
        None,
        BTreeMap::new(),
        Duration::from_secs(10),
    )?;
    if reply.status != RemoteReplyStatus::Accepted {
        return Err("Host rejected read-only ping".into());
    }
    println!(
        "Read-only request accepted; capabilities={}",
        client.capabilities.is_some()
    );
    client.close()?;
    Ok(())
}
