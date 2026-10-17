use soundroom_engine::{project::Project, render, Error, Result};
use std::{io::Read, path::Path, sync::atomic::AtomicBool};
fn read_json(path: &str) -> Result<serde_json::Value> {
    let file = std::fs::File::open(path)?;
    let mut bytes = vec![];
    file.take(64 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err(Error::Invalid(
            "Worker model exceeds metadata budget".into(),
        ));
    }
    Ok(serde_json::from_slice(&bytes)?)
}
fn write_audio(path: &str, audio: &[Vec<f32>]) -> Result<()> {
    let mut writer =
        soundroom_engine::wave::WaveWriter::create(Path::new(path), audio[0].len() as u64)?;
    let result = (|| {
        for (start, left) in audio[0].chunks(8192).enumerate() {
            let offset = start * 8192;
            let right = if audio.len() > 1 {
                &audio[1][offset..offset + left.len()]
            } else {
                left
            };
            writer.write(left, right)?;
        }
        writer.finish()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}
fn run() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if matches!(
        args.first().map(String::as_str),
        Some("init" | "grant" | "serve" | "serve-local")
    ) {
        return control(&args).map_err(|e| Error::Invalid(e.to_string()));
    }
    match args.as_slice() {
        [cmd, project] if cmd == "validate" => {
            let p = Project::open(Path::new(project))?;
            println!("Validated project format {}, {} tracks, {} assets, {} frames. Hardware/DSP parity is not implied.",p.session.version,p.session.tracks.len(),p.media.len(),p.session.end());
            Ok(())
        }
        [cmd, project, output] if cmd == "render" || cmd == "render-with-tails" => {
            let p = Project::open(Path::new(project))?;
            render::export_with_tails(
                &p,
                Path::new(output),
                0,
                p.session.end(),
                cmd == "render-with-tails",
                &AtomicBool::new(false),
                |_, _| {},
            )?;
            println!(
                "Rendered to {output} at 48 kHz{}",
                if cmd == "render-with-tails" {
                    " including effect tails"
                } else {
                    ""
                }
            );
            Ok(())
        }
        [cmd, source, output] if cmd == "analyze-pitch" => {
            let mut reader = soundroom_engine::wave::WaveReader::open(Path::new(source))?;
            let points =
                soundroom_engine::pitch::analyze_wave(&mut reader, 1024, &AtomicBool::new(false))?;
            let mut writer = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(output)?;
            if let Err(error) = serde_json::to_writer_pretty(&mut writer, &points) {
                let _ = std::fs::remove_file(output);
                return Err(error.into());
            }
            writer.sync_all()?;
            println!("Measured {} pitch contour points to {output}", points.len());
            Ok(())
        }
        [cmd, source, output] if cmd == "bounce-designer" || cmd == "bounce-designer-loop" => {
            let pattern: soundroom_engine::designer::Pattern =
                serde_json::from_value(read_json(source)?)?;
            let cancelled = AtomicBool::new(false);
            let audio = if cmd == "bounce-designer-loop" {
                pattern.render_loop(&cancelled)?
            } else {
                pattern.render(48000., &cancelled)?
            };
            write_audio(output, &audio)?;
            println!("Bounced native Designer to {output}");
            Ok(())
        }
        [cmd, source, output] if cmd == "bounce-drums" => {
            let pattern: soundroom_engine::drums::Pattern =
                serde_json::from_value(read_json(source)?)?;
            let audio = pattern.render(1, false, &AtomicBool::new(false))?;
            write_audio(output, &audio)?;
            println!("Bounced native Drummer to {output}");
            Ok(())
        }
        [cmd, source] if cmd == "inspect-midi" => {
            let file = std::fs::File::open(source)?;
            let mut bytes = vec![];
            file.take(4_194_305).read_to_end(&mut bytes)?;
            let content = soundroom_engine::midi::inspect(&bytes)?;
            println!("{}", serde_json::to_string_pretty(&content)?);
            Ok(())
        }
        [cmd, project, output] if cmd == "copy-project" => {
            let p = Project::open(Path::new(project))?;
            p.save_new(Path::new(output))?;
            println!("Saved project to {output}");
            Ok(())
        }
        [cmd] if cmd == "--help" || cmd == "help" => {
            println!("soundroom-server init PRIVATE-ROOT SERVER-NAME\nsoundroom-server grant PRIVATE-ROOT APPROVED-NAME viewer|editor|operator NEW-CREDENTIAL.json\nsoundroom-server serve PRIVATE-ROOT IDENTITY.p12 HOST:PORT [PROJECT.soundroom] [--audio-default [--experimental-automation-write]] [--web-assets DIR]\nsoundroom-server serve-local PRIVATE-ROOT [--port N] [--no-audio] [--web-assets DIR]\nsoundroom-server validate PROJECT.soundroom\nsoundroom-server render PROJECT.soundroom OUTPUT.wav\nsoundroom-server render-with-tails PROJECT.soundroom OUTPUT.wav\nsoundroom-server copy-project PROJECT.soundroom NEW.soundroom\nsoundroom-server analyze-pitch INPUT.wav NEW-CONTOUR.json\nsoundroom-server bounce-designer PATCH.json OUTPUT.wav\nsoundroom-server bounce-designer-loop PATCH.json OUTPUT.wav\nsoundroom-server bounce-drums PATTERN.json OUTPUT.wav\nsoundroom-server inspect-midi INPUT.mid\n\nPortable worker engine preview. Network service and hardware host are not qualified yet.\nRender rejects enabled DSP that has not been ported; existing output files are never overwritten.");
            Ok(())
        }
        _ => Err(Error::Invalid("Use soundroom-server --help".into())),
    }
}
fn main() {
    if let Err(error) = run() {
        eprintln!("soundroom-server: {error}");
        std::process::exit(1)
    }
}

fn control(args: &[String]) -> std::result::Result<(), soundroom_client::Error> {
    use soundroom_server::{
        endpoint::{Endpoint, EndpointConfig},
        registry::{write_new_private, Registry, RootLock},
        service::Service,
    };
    let bad = || {
        soundroom_client::Error::Protocol("Use init ROOT NAME; grant ROOT NAME ROLE NEW-CREDENTIAL.json; serve ROOT IDENTITY.p12 HOST:PORT [PROJECT.soundroom] [--audio-default [--experimental-automation-write]] [--web-assets DIR]".into())
    };
    match args {
        [cmd, root, name] if cmd == "init" => {
            let _lock = RootLock::acquire(Path::new(root))?;
            let registry = Registry::create(Path::new(root), name)?;
            println!("Created server {} in {root}", registry.server_id);
            Ok(())
        }
        [cmd, root, name, role, path] if cmd == "grant" => {
            let _lock = RootLock::acquire(Path::new(root))?;
            let mut registry = Registry::load(Path::new(root))?;
            let role = serde_json::from_value(serde_json::json!(role))?;
            let credential = registry.issue(name, role)?;
            if let Err(e) = write_new_private(Path::new(path), &serde_json::to_vec(&credential)?) {
                registry.revoke(credential["credentialID"].as_str().unwrap())?;
                return Err(e);
            }
            println!("Created approved credential for {name} at {path}");
            Ok(())
        }
        [cmd, root, identity, bind, rest @ ..] if cmd == "serve" && rest.len() <= 5 => {
            let mut audio = false;
            let mut automation_write = false;
            let mut web_assets = None;
            let mut project = None;
            let mut args = rest.iter();
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--audio-default" if !audio => audio = true,
                    // Unqualified development opt-in: advertises live Write.
                    "--experimental-automation-write" if !automation_write => {
                        automation_write = true
                    }
                    "--web-assets" if web_assets.is_none() => {
                        web_assets = Some(args.next().ok_or_else(bad)?)
                    }
                    name if !name.starts_with("--") && project.is_none() => project = Some(arg),
                    _ => return Err(bad()),
                }
            }
            if automation_write && !audio {
                return Err(soundroom_client::Error::Protocol(
                    "Live automation Write requires --audio-default".into(),
                ));
            }
            #[cfg(not(target_os = "macos"))]
            if audio {
                return Err(soundroom_client::Error::Protocol("Default audio output is available only through the macOS backend in this build".into()));
            }
            let _lock = RootLock::acquire(Path::new(root))?;
            let password = std::env::var("SOUNDROOM_TLS_IDENTITY_PASSWORD").unwrap_or_default();
            let origins = std::env::var("SOUNDROOM_WEB_ORIGINS")
                .unwrap_or_default()
                .split(',')
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .collect();
            let config = EndpointConfig {
                bind: bind.parse().map_err(|_| bad())?,
                approved_origins: origins,
            };
            let endpoint = Endpoint::bind(config, Path::new(identity), &password)?;
            let endpoint = if let Some(path) = web_assets {
                endpoint.with_web_assets(Path::new(path))?
            } else {
                endpoint
            };
            let service = Service::open(Path::new(root), project.map(Path::new))?;
            // Hardware is opened only by this explicit operator opt-in. Default serve is control-only.
            #[cfg(target_os = "macos")]
            let _audio_output = if audio {
                let (control, host) = soundroom_engine::realtime::channel();
                let mut output = soundroom_engine::core_audio::CoreAudioOutput::open_default(host)
                    .map_err(|e| soundroom_client::Error::Protocol(e.to_string()))?;
                Service::attach_audio(&service, control)?;
                if automation_write {
                    if let Ok(mut state) = service.lock() {
                        state.enable_automation_write();
                    }
                    eprintln!(
                        "Experimental live automation Write is enabled; it is not qualified."
                    );
                }
                output
                    .start()
                    .map_err(|e| soundroom_client::Error::Protocol(e.to_string()))?;
                Some(output)
            } else {
                None
            };
            println!(
                "Soundroom control service listening at wss://{}/control",
                endpoint.address()?
            );
            endpoint.run(
                service,
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            )
        }
        [cmd, root, rest @ ..] if cmd == "serve-local" && rest.len() <= 5 => {
            serve_local(Path::new(root), rest)
        }
        _ => Err(bad()),
    }
}

/// Mac local host for the native GUI: one private data root holding the
/// registry, a private loopback CA/identity and the GUI's operator credential.
/// Binds loopback only. The lock makes a second launch exit; the GUI attaches.
fn serve_local(root: &Path, rest: &[String]) -> std::result::Result<(), soundroom_client::Error> {
    use soundroom_client::models::RemoteRole;
    use soundroom_server::{
        endpoint::{Endpoint, EndpointConfig},
        registry::{private_directory, write_new_private, Registry, RootLock},
        service::Service,
    };
    let bad = || {
        soundroom_client::Error::Protocol(
            "Use serve-local ROOT [--port N] [--no-audio] [--web-assets DIR] [--experimental-automation-write]".into(),
        )
    };
    let mut port = 49_160u16;
    let mut audio = true;
    let mut automation_write = false;
    let mut web_assets = None;
    let mut simulated = false;
    let mut args = rest.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            // Tests only: a timestamped callback pumped by a thread, no device.
            "--simulated-callback" => simulated = true,
            "--port" => port = args.next().and_then(|v| v.parse().ok()).ok_or_else(bad)?,
            "--no-audio" => audio = false,
            "--experimental-automation-write" => automation_write = true,
            "--web-assets" => web_assets = Some(args.next().ok_or_else(bad)?.clone()),
            _ => return Err(bad()),
        }
    }
    private_directory(root)?;
    let _lock = RootLock::acquire(root)?;
    // Recorded drum kits: the server's own Kits folder, then the app bundle's.
    let mut kit_roots = vec![root.join("Kits")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(contents) = exe.parent().and_then(Path::parent) {
            kit_roots.push(contents.join("Resources").join("Kits"));
        }
    }
    soundroom_engine::sampled_drums::set_kit_roots(kit_roots);
    if !root.join("registry.json").exists() {
        Registry::create(root, "This Mac")?;
    }
    let tls = root.join("tls");
    let password = local_identity(&tls)?;
    // The GUI's credential is approved for the exact name "Soundroom GUI".
    let credential_path = root.join("gui-credential.json");
    let service = Service::open_with_autosave(root)?;
    {
        let mut state = service
            .lock()
            .map_err(|_| soundroom_client::Error::Protocol("Authority is unavailable".into()))?;
        let current = std::fs::read(&credential_path)
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| v["credentialID"].as_str().map(str::to_owned))
            .filter(|id| state.registry.contains(id));
        let id = match current {
            Some(id) => id,
            None => {
                let _ = std::fs::remove_file(&credential_path);
                let credential = state
                    .registry
                    .issue("Soundroom GUI", RemoteRole::Operator)?;
                write_new_private(&credential_path, &serde_json::to_vec(&credential)?)?;
                credential["credentialID"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            }
        };
        state.protect_credential(&id);
    }
    let endpoint = Endpoint::bind(
        EndpointConfig {
            bind: std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            approved_origins: vec!["https://localhost".into()],
        },
        &tls.join("server.p12"),
        &password,
    )?
    .with_connection_diagnostics();
    let endpoint = match web_assets {
        Some(path) => endpoint.with_web_assets(Path::new(&path))?,
        None => endpoint,
    };
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    if let Ok(mut state) = service.lock() {
        state.set_shutdown_signal(stop.clone());
    }
    Service::start_autosave(&service);
    if simulated {
        let (control, mut host) = soundroom_engine::realtime::channel();
        Service::attach_audio(&service, control)?;
        // A simulated mono input shares this callback's host clock (tests only).
        let input = service
            .lock()
            .map_err(|_| soundroom_client::Error::Protocol("Authority is unavailable".into()))?
            .simulated_input();
        let stop = stop.clone();
        std::thread::spawn(move || {
            let (mut left, mut right, mut frame) = ([0f32; 128], [0f32; 128], 0u64);
            let mut phase = 0u64;
            while !stop.load(std::sync::atomic::Ordering::Acquire) {
                let host_time = 1_000_000_000_000 + frame * 20_833;
                host.render_timed(
                    &mut left,
                    &mut right,
                    soundroom_engine::capture::CaptureTiming {
                        sample_time: frame as f64,
                        host_time,
                        flags: 3,
                    },
                );
                soundroom_server::service::record::push_simulated(
                    &input, &mut phase, host_time, 128,
                );
                frame += 128;
                std::thread::sleep(std::time::Duration::from_micros(2_667));
            }
        });
        audio = false;
    }
    #[cfg(target_os = "macos")]
    if audio {
        let (control, host) = soundroom_engine::realtime::channel();
        let (switch, ready) = output_owner(host)?;
        Service::attach_audio(&service, control)?;
        if let Ok(mut state) = service.lock() {
            state.set_output_switch(switch);
            if automation_write {
                state.enable_automation_write();
            }
        }
        ready
            .send(())
            .map_err(|_| soundroom_client::Error::Protocol("Output owner stopped".into()))?;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (audio, automation_write);
    println!("Soundroom local service listening at wss://localhost:{port}/control");
    endpoint.run(service, stop)
}

/// The Core Audio output unit is bound to one control thread. This thread opens
/// the default output, starts it once the graph is attached, and serves device
/// switches by closing the unit, reopening the same AudioHost on the requested
/// device and restarting it (falling back to the previous device on failure).
#[cfg(target_os = "macos")]
fn output_owner(
    host: soundroom_engine::realtime::AudioHost,
) -> std::result::Result<
    (
        std::sync::mpsc::SyncSender<soundroom_server::service::OutputRequest>,
        std::sync::mpsc::SyncSender<()>,
    ),
    soundroom_client::Error,
> {
    use soundroom_engine::core_audio::CoreAudioOutput;
    let (switch, requests) =
        std::sync::mpsc::sync_channel::<soundroom_server::service::OutputRequest>(1);
    let (ready, start) = std::sync::mpsc::sync_channel::<()>(1);
    let (opened, result) = std::sync::mpsc::sync_channel::<std::result::Result<(), String>>(1);
    std::thread::Builder::new()
        .name("soundroom-output".into())
        .stack_size(32 << 20)
        .spawn(move || {
            let mut output = match CoreAudioOutput::open_default(host) {
                Ok(output) => {
                    let _ = opened.send(Ok(()));
                    output
                }
                Err(error) => {
                    let _ = opened.send(Err(error.to_string()));
                    return;
                }
            };
            if start.recv().is_err() || output.start().is_err() {
                return;
            }
            let mut current: Option<String> = None;
            for request in requests {
                let host = match output.close_into_host() {
                    Ok(host) => host,
                    Err(reason) => {
                        let _ = request.reply.send(Err(format!("{reason:?}")));
                        return;
                    }
                };
                let attempt = CoreAudioOutput::open_recoverable(host, request.uid.as_deref())
                    .and_then(|mut next| match next.start() {
                        Ok(()) => Ok(next),
                        Err(error) => Err((error, next.close_into_host().ok().map(Box::new))),
                    });
                match attempt {
                    Ok(next) => {
                        output = next;
                        current = request.uid.clone();
                        let _ = request.reply.send(Ok(()));
                    }
                    Err((error, Some(host))) => {
                        let _ = request.reply.send(Err(error.to_string()));
                        match CoreAudioOutput::open_recoverable(*host, current.as_deref()) {
                            Ok(mut previous) => {
                                if previous.start().is_err() {
                                    return;
                                }
                                output = previous;
                            }
                            Err(_) => return,
                        }
                    }
                    Err((error, None)) => {
                        let _ = request.reply.send(Err(error.to_string()));
                        return;
                    }
                }
            }
        })?;
    result
        .recv()
        .map_err(|_| soundroom_client::Error::Protocol("Output owner stopped".into()))?
        .map_err(soundroom_client::Error::Protocol)?;
    Ok((switch, ready))
}

/// Creates (once) a private loopback CA and a localhost leaf identity using the
/// system's /usr/bin/openssl. Nothing is added to the keychain or system trust;
/// clients anchor tls/ca.crt explicitly. Returns the identity password.
fn local_identity(tls: &Path) -> std::result::Result<String, soundroom_client::Error> {
    use soundroom_server::registry::{private_directory, write_new_private};
    let fail =
        |m: String| soundroom_client::Error::Protocol(format!("Local TLS setup failed: {m}"));
    private_directory(tls)?;
    let password_path = tls.join("identity.password");
    if tls.join("server.p12").exists() && tls.join("ca.crt").exists() && password_path.exists() {
        return Ok(std::fs::read_to_string(&password_path)?.trim().to_owned());
    }
    for name in [
        "ca.key",
        "ca.crt",
        "ca.srl",
        "server.key",
        "server.csr",
        "server.crt",
        "server.p12",
        "identity.password",
        "server.ext",
    ] {
        let _ = std::fs::remove_file(tls.join(name));
    }
    let mut secret = [0u8; 24];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut secret)?;
    let password: String = secret.iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(
        tls.join("server.ext"),
        "basicConstraints=CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:localhost,IP:127.0.0.1\n",
    )?;
    let pass = format!("pass:{password}");
    let steps: [&[&str]; 5] = [
        &[
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            "ca.key",
            "-out",
            "ca.crt",
            "-days",
            "3650",
            "-subj",
            "/CN=Soundroom Local CA",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
            "-addext",
            "keyUsage=critical,keyCertSign,cRLSign",
        ],
        &[
            "req",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            "server.key",
            "-out",
            "server.csr",
            "-subj",
            "/CN=localhost",
        ],
        &[
            "x509",
            "-req",
            "-in",
            "server.csr",
            "-CA",
            "ca.crt",
            "-CAkey",
            "ca.key",
            "-CAcreateserial",
            "-out",
            "server.crt",
            "-days",
            "825",
            "-sha256",
            "-extfile",
            "server.ext",
        ],
        &[
            "pkcs12",
            "-export",
            "-inkey",
            "server.key",
            "-in",
            "server.crt",
            "-certfile",
            "ca.crt",
            "-out",
            "server.p12",
            "-passout",
            &pass,
        ],
        &["x509", "-in", "ca.crt", "-noout"],
    ];
    for args in steps {
        let output = std::process::Command::new("/usr/bin/openssl")
            .args(args)
            .current_dir(tls)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| fail(e.to_string()))?;
        if !output.status.success() {
            return Err(fail(String::from_utf8_lossy(&output.stderr).into_owned()));
        }
    }
    for name in ["ca.key", "server.key", "server.p12"] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tls.join(name), std::fs::Permissions::from_mode(0o600))?;
    }
    write_new_private(&password_path, password.as_bytes())?;
    Ok(password)
}
