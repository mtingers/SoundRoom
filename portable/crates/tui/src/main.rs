use soundroom_client::{auth::Credential, transport::WssConfig};
use soundroom_tui::{
    app::App, input::Decoder, options::Options, terminal::Terminal, worker::Worker,
};
use std::{
    io::{self, Write},
    time::{Duration, Instant},
};
fn main() {
    if let Err(error) = run() {
        eprintln!("Soundroom: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() == 1 || args.iter().any(|s| s == "--help") {
        println!("Usage: soundroom-tui --server <wss-url> --credential <file> --name <approved-name> [--ca <PEM>] [--origin <HTTPS origin>]\nAudio stays on the server. Credentials never belong in URLs.\nPress ? for navigation and commands. Ctrl-C or q exits and restores the terminal.");
        return Ok(());
    }
    let options =
        Options::parse(&args[1..]).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let mut config = WssConfig::new(options.endpoint);
    if let Some(path) = options.ca {
        config.root_ca_pem = Some(std::fs::read(path)?);
    }
    config.origin = options.origin;
    let credential = Credential::load(&options.credential)?;
    let terminal = Terminal::enter()?;
    let worker = Worker::start(config, credential, options.name);
    let mut app = App::new();
    let mut decoder = Decoder::default();
    let mut previous = None;
    let mut last = Instant::now() - Duration::from_secs(1);
    let mut last_input = Instant::now();
    while !app.quit && !terminal.exit_requested() {
        let input = terminal.read()?;
        let keys = if input.is_empty() {
            if last_input.elapsed() > Duration::from_millis(40) {
                decoder.idle()
            } else {
                vec![]
            }
        } else {
            last_input = Instant::now();
            decoder.feed(&input)
        };
        for key in keys {
            for work in app.key(key) {
                let failure = work.failure(false, String::new());
                if let Err(error) = worker.send(work) {
                    app.work_failed(&failure.message(format!("Not sent: {error}")));
                }
            }
        }
        if last.elapsed() >= Duration::from_millis(50) {
            let shared = worker.state.lock().unwrap().frame();
            app.install(shared);
            for work in app.tick() {
                let failure = work.failure(false, String::new());
                if let Err(error) = worker.send(work) {
                    app.work_failed(&failure.message(format!("Not sent: {error}")));
                }
            }
            let (w, h) = terminal.size();
            let canvas = app.draw(w, h);
            let output = canvas.diff(previous.as_ref());
            if !output.is_empty() {
                io::stdout().write_all(output.as_bytes())?;
                io::stdout().flush()?;
            }
            previous = Some(canvas);
            last = Instant::now();
        }
    }
    drop(worker);
    drop(terminal);
    Ok(())
}
