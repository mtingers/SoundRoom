//! Native TLS with strict RFC 6455 upgrade, explicit origins and bounded connection work.
use crate::{
    readiness::{Interest, SocketIo, Waiter},
    registry::read_private,
    service::{Identity, Outbox, Shared},
    Error,
};
use base64::Engine;
use native_tls::{Identity as TlsIdentity, Protocol, TlsAcceptor, TlsStream};
use serde_json::{json, Value};
use soundroom_client::{
    auth::fill_random,
    validation,
    websocket::{server_frame, upgrade_accept, Decoder, FrameEvent},
};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
#[path = "request_decode.rs"]
pub(crate) mod requests;
type PeerTls = TlsStream<SocketIo>;
#[path = "tls_io.rs"]
mod writes;
use writes::FrameWrite;
const HEADER_LIMIT: usize = 16384;
const CONNECTION_LIMIT: usize = 16;
#[derive(Clone)]
pub struct EndpointConfig {
    pub bind: SocketAddr,
    pub approved_origins: Vec<String>,
}
pub struct Endpoint {
    listener: TcpListener,
    acceptor: TlsAcceptor,
    config: EndpointConfig,
    active: Arc<AtomicUsize>,
    assets: Option<Arc<crate::assets::Bundle>>,
    listener_waiter: Waiter,
    connection_diagnostics: bool,
}
fn protocol(s: &str) -> Error {
    Error::Protocol(s.into())
}
impl Endpoint {
    pub fn bind(config: EndpointConfig, pkcs12: &Path, password: &str) -> Result<Self, Error> {
        let mut bytes = read_private(pkcs12, 1024 * 1024)?;
        let imported = TlsIdentity::from_pkcs12(&bytes, password);
        for byte in &mut bytes {
            unsafe {
                std::ptr::write_volatile(byte, 0);
            }
        }
        std::sync::atomic::compiler_fence(Ordering::SeqCst);
        let identity =
            imported.map_err(|e| protocol(&format!("PKCS12 identity import failed: {e}")))?;
        let mut builder = TlsAcceptor::builder(identity);
        builder.min_protocol_version(Some(Protocol::Tlsv12));
        let acceptor = builder
            .build()
            .map_err(|e| protocol(&format!("TLS acceptor creation failed: {e}")))?;
        if config.approved_origins.len() > 16
            || config.approved_origins.iter().any(|origin| {
                let parsed = url_origin(origin);
                parsed.as_deref() != Some(origin)
            })
        {
            return Err(protocol("Approved origins must be exact HTTPS origins"));
        }
        let listener = TcpListener::bind(config.bind)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            acceptor,
            config,
            active: Arc::new(AtomicUsize::new(0)),
            assets: None,
            listener_waiter: Waiter::new()?,
            connection_diagnostics: false,
        })
    }
    /// Local development hosts may log their own transport/validation errors.
    /// Peer frames and credentials are never logged.
    pub fn with_connection_diagnostics(mut self) -> Self {
        self.connection_diagnostics = true;
        self
    }
    pub fn with_web_assets(mut self, path: &Path) -> Result<Self, Error> {
        self.assets = Some(Arc::new(crate::assets::Bundle::load(path)?));
        Ok(self)
    }
    pub fn address(&self) -> Result<SocketAddr, Error> {
        Ok(self.listener.local_addr()?)
    }
    pub fn run(&self, service: Shared, stop: Arc<AtomicBool>) -> Result<(), Error> {
        while !stop.load(Ordering::Acquire) {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if self
                        .active
                        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                            if v < CONNECTION_LIMIT {
                                Some(v + 1)
                            } else {
                                None
                            }
                        })
                        .is_err()
                    {
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                        continue;
                    }
                    let acceptor = self.acceptor.clone();
                    let service = service.clone();
                    let origins = self.config.approved_origins.clone();
                    let active = self.active.clone();
                    let stop = stop.clone();
                    let assets = self.assets.clone();
                    let diagnostics = self.connection_diagnostics;
                    std::thread::spawn(move || {
                        struct Count(Arc<AtomicUsize>);
                        impl Drop for Count {
                            fn drop(&mut self) {
                                self.0.fetch_sub(1, Ordering::AcqRel);
                            }
                        }
                        let _count = Count(active);
                        let result = connection(
                            stream,
                            acceptor,
                            service,
                            &origins,
                            stop,
                            assets.as_deref(),
                            diagnostics,
                        );
                        if let Err(error) = result {
                            // Never log peer payloads, credentials or pairing secrets.
                            // The opt-in detail is the server's own error text only.
                            if diagnostics
                                || std::env::var_os("SOUNDROOM_DEBUG_CONNECTIONS").is_some()
                            {
                                eprintln!("Soundroom control connection closed: {error}");
                            } else {
                                eprintln!("Soundroom control connection closed after a validation/transport failure");
                            }
                        }
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // Readiness drives connection admission. The sole listener
                    // deadline checks the existing external atomic shutdown API.
                    self.listener_waiter.wait_listener(
                        &self.listener,
                        Instant::now() + Duration::from_millis(250),
                    )?;
                }
                Err(e) => return Err(e.into()),
            }
        }
        if let Ok(state) = service.lock() {
            state.close_connections();
        }
        Ok(())
    }
}
fn url_origin(value: &str) -> Option<String> {
    let url = url::Url::parse(value).ok()?;
    if url.scheme() != "https"
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return None;
    }
    Some(url.origin().ascii_serialization())
}
fn connection(
    stream: TcpStream,
    acceptor: TlsAcceptor,
    service: Shared,
    origins: &[String],
    stop: Arc<AtomicBool>,
    assets: Option<&crate::assets::Bundle>,
    diagnostics: bool,
) -> Result<(), Error> {
    // BSD accept may inherit the listener's O_NONBLOCK flag. Native TLS's
    // blocking handshake must not treat an initial WouldBlock as terminal.
    // The accepted peer has dedicated bounded timeouts and its own thread.
    stream.set_nonblocking(false)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut stream = acceptor
        .accept(SocketIo::new(stream))
        .map_err(|error| match error {
            native_tls::HandshakeError::Failure(error) => {
                // Platform TLS diagnostics contain no application payload or
                // credential. Retain this reason for deployment qualification.
                eprintln!("Soundroom server TLS handshake failed: {error}");
                protocol("TLS handshake failed")
            }
            native_tls::HandshakeError::WouldBlock(_) => {
                eprintln!("Soundroom server TLS handshake exceeded its bounded I/O wait");
                Error::Timeout
            }
        })?;
    let trace = |phase: &str| {
        if diagnostics || std::env::var_os("SOUNDROOM_DEBUG_CONNECTIONS").is_some() {
            eprintln!("Soundroom connection phase: {phase}");
        }
    };
    trace("tls accepted; awaiting upgrade");
    let mut header = vec![];
    let start = Instant::now();
    let mut tail = vec![];
    loop {
        if start.elapsed() > Duration::from_secs(10) {
            return Err(Error::Timeout);
        }
        let mut bytes = [0; 2048];
        let n = stream.read(&mut bytes)?;
        if n == 0 {
            return Err(Error::Disconnected);
        }
        header.extend_from_slice(&bytes[..n]);
        if let Some(end) = header.windows(4).position(|s| s == b"\r\n\r\n") {
            tail = header.split_off(end + 4);
            break;
        }
        if header.len() > HEADER_LIMIT {
            return Err(protocol("Upgrade header exceeds limit"));
        }
    }
    if header.len() > HEADER_LIMIT {
        return Err(protocol("Upgrade header exceeds limit"));
    }
    if !header.starts_with(b"GET /control HTTP/1.1\r\n") {
        if !tail.is_empty() {
            return Err(protocol(
                "HTTP asset request cannot be pipelined or contain a body",
            ));
        }
        let bundle = assets.ok_or_else(|| protocol("No web asset bundle was configured"))?;
        let response = bundle.response(&header)?;
        stream.write_all(&response.head)?;
        stream.write_all(&response.body)?;
        stream.flush()?;
        return Ok(());
    }
    trace("upgrade received");
    let upgrade = validate_upgrade(&header, origins)?;
    // One write, so the whole 101 header is one TLS record. Apple's
    // Network.framework WebSocket client stalls if the header is split.
    let response = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\nSec-WebSocket-Protocol: soundroom.v3\r\n\r\n", upgrade_accept(&upgrade));
    stream.write_all(response.as_bytes())?;
    stream.flush()?;
    let mut decoder = Decoder::server();
    if !tail.is_empty() {
        decoder.feed(&tail)?;
    }
    let (server_id, global) = {
        let state = service
            .lock()
            .map_err(|_| protocol("Authority is unavailable"))?;
        (state.registry.server_id.clone(), state.global())
    };
    let mut bytes = [0u8; 32];
    fill_random(&mut bytes)?;
    let nonce = base64::engine::general_purpose::STANDARD.encode(bytes);
    bytes.fill(0);
    send(
        &mut stream,
        &json!({"version":3,"kind":"challenge","serverID":server_id,"nonce":nonce}),
    )?;
    trace("challenge sent; awaiting hello");
    let mut hello = receive_json(
        &mut stream,
        &mut decoder,
        Instant::now() + Duration::from_secs(10),
    )?;
    if hello["version"] != 3 {
        return Err(protocol("Unsupported protocol"));
    }
    if hello["kind"] == "pair" {
        if hello.as_object().map_or(true, |o| o.len() != 4) {
            return Err(protocol("Invalid pairing envelope"));
        }
        let code = hello["code"]
            .as_str()
            .ok_or_else(|| protocol("Pairing code is required"))?;
        let name = hello["name"]
            .as_str()
            .ok_or_else(|| protocol("Approved name is required"))?;
        let credential = {
            let result = service
                .lock()
                .map_err(|_| protocol("Authority is unavailable"))?
                .registry
                .redeem(code, name);
            match result {
                Ok(credential) => credential,
                Err(_) => {
                    send(
                        &mut stream,
                        &json!({"version":3,"kind":"failure","message":"Pairing code is invalid, expired, or not approved for this client name. Create a fresh code with the matching name."}),
                    )?;
                    stream.write_all(&server_frame(8, &1008u16.to_be_bytes())?)?;
                    stream.flush()?;
                    return Ok(());
                }
            }
        };
        send(
            &mut stream,
            &json!({"version":3,"kind":"paired","credential":credential}),
        )?;
        // Continue the challenge on this connection: the browser SDK sends
        // Hello with the newly issued credential and the original nonce.
        hello = receive_json(
            &mut stream,
            &mut decoder,
            Instant::now() + Duration::from_secs(10),
        )?;
    }
    if hello["version"] != 3 {
        return Err(protocol("Unsupported protocol"));
    }
    let mut body = hello.clone();
    let o = body
        .as_object_mut()
        .ok_or_else(|| protocol("Invalid hello"))?;
    if o.remove("kind") != Some(json!("hello")) {
        return Err(protocol("Expected hello"));
    }
    validation::validate_model(&body, "RemoteHello")?;
    let runtime = hello["peer"]["id"].as_str().unwrap().to_ascii_uppercase();
    let name = hello["peer"]["name"].as_str().unwrap();
    let credential_id = hello["credentialID"].as_str().unwrap().to_ascii_uppercase();
    let record = {
        let state = service
            .lock()
            .map_err(|_| protocol("Authority is unavailable"))?;
        state.registry.verify(
            hello["serverID"].as_str().unwrap(),
            &credential_id,
            &runtime,
            name,
            &nonce,
            hello["proof"].as_str().unwrap(),
        )?
    };
    let identity = Identity {
        runtime,
        credential: credential_id,
        name: record.name,
        role: record.role,
    };
    let (out, inbox) = Outbox::pair(global.clone());
    let waiter = Waiter::new()?;
    out.wake_on_send(waiter.signal())?;
    send(
        &mut stream,
        &json!({"version":3,"kind":"welcome","serverID":server_id,"name":service.lock().map_err(|_|protocol("Authority is unavailable"))?.registry.server_name,"role":identity.role}),
    )?;
    send(
        &mut stream,
        &json!({"version":3,"kind":"capabilities","capabilities":service.lock().map_err(|_|protocol("Authority is unavailable"))?.endpoint_capabilities()}),
    )?;
    {
        let mut state = service
            .lock()
            .map_err(|_| protocol("Authority is unavailable"))?;
        state.join(identity.clone(), out.clone())?;
        state.queue_snapshot(&out)?;
    }
    stream.get_ref().set_read_timeout(None)?;
    stream.get_ref().set_write_timeout(None)?;
    stream.get_ref().set_nonblocking(true)?;
    let request_decode =
        requests::RequestDecode::new(service.clone(), identity.clone(), out.clone(), global)?;
    let mut last = Instant::now();
    let mut next_heartbeat = Instant::now() + Duration::from_secs(5);
    let mut heartbeat: Option<String> = None;
    let mut control_rate = requests::InputRate::default();
    let mut echoed_close = None;
    let mut current: Option<FrameWrite> = None;
    let mut controls = std::collections::VecDeque::new();
    let mut read_wanted = Interest::Read;
    let result = (|| -> Result<(), Error> {
        loop {
            if let Some(error) = request_decode.failure() {
                return Err(error);
            }
            if stop.load(Ordering::Acquire) || out.closed.load(Ordering::Acquire) {
                if let Some(error) = request_decode.failure() {
                    return Err(error);
                }
                break;
            }
            if last.elapsed() > Duration::from_secs(15) {
                return Err(Error::Timeout);
            }
            // Start just one message at a time. Native TLS write retries keep
            // the exact bytes/offset, and receipts win at message boundaries.
            if current.is_none() {
                if let Some(frame) = controls.pop_front() {
                    current = Some(frame);
                } else {
                    let data = inbox
                        .priority
                        .try_recv()
                        .ok()
                        .or_else(|| inbox.ordinary.try_recv().ok());
                    if let Some(data) = data {
                        let lease = inbox.delivery_lease(data.len());
                        // JSON control frames are UTF-8 text, as browsers and native clients expect.
                        current = Some(FrameWrite::new(1, &data, Some(lease))?);
                    }
                }
            }
            for event in decoder.by_ref() {
                match event {
                    FrameEvent::Message(bytes) => {
                        let count = bytes.len();
                        last = Instant::now();
                        let Some(value) = request_decode.route(bytes)? else {
                            continue;
                        };
                        if !control_rate.reserve(count, Instant::now()).is_zero() {
                            return Err(protocol("Control message rate exceeds limit"));
                        }
                        if value["version"] != 3 {
                            return Err(protocol("Unsupported envelope"));
                        }
                        last = Instant::now();
                        match value["kind"].as_str() {
                            Some("heartbeatReply") => {
                                if value.as_object().map_or(true, |o| o.len() != 3)
                                    || !value["id"].as_str().is_some_and(validation::uuid)
                                    || heartbeat.is_none()
                                    || heartbeat.as_deref() != value["id"].as_str()
                                {
                                    return Err(protocol("Unexpected heartbeat reply"));
                                }
                                heartbeat = None;
                            }
                            Some("heartbeat") => {
                                if value.as_object().map_or(true, |o| o.len() != 3)
                                    || !value["id"].as_str().is_some_and(validation::uuid)
                                {
                                    return Err(protocol("Invalid heartbeat"));
                                }
                                out.send(
                                    &json!({"version":3,"kind":"heartbeatReply","id":value["id"]}),
                                    true,
                                )?;
                            }
                            _ => return Err(protocol("Unexpected client envelope")),
                        }
                    }
                    FrameEvent::Ping(payload) => {
                        if controls.len() >= 16 {
                            return Err(protocol("Control response queue exceeds limit"));
                        }
                        controls.push_back(FrameWrite::new(10, &payload, None)?);
                        last = Instant::now();
                    }
                    FrameEvent::Pong(_) => last = Instant::now(),
                    FrameEvent::Close(payload) => {
                        echoed_close = Some(payload);
                        return Ok(());
                    }
                }
            }
            if Instant::now() >= next_heartbeat && heartbeat.is_none() {
                let id = soundroom_client::auth::new_id()?;
                out.send(&json!({"version":3,"kind":"heartbeat","id":id}), true)?;
                heartbeat = Some(id);
                next_heartbeat = Instant::now() + Duration::from_secs(5);
            }
            if let Some(frame) = current.as_mut() {
                if frame.progress(&mut stream)? {
                    current = None;
                    // Notifications may already have been drained while this
                    // frame was active; inspect the existing inbox again.
                    continue;
                }
            }
            // Newly queued local controls/ordinary frames should start without
            // sleeping. Otherwise wait for I/O or an outbox notification.
            if current.is_none() && !controls.is_empty() {
                continue;
            }
            let heartbeat_deadline = if heartbeat.is_some() {
                last + Duration::from_secs(15)
            } else {
                next_heartbeat
            };
            let deadline = if let Some(frame) = current.as_ref() {
                frame
                    .deadline()
                    .min(heartbeat_deadline)
                    .min(last + Duration::from_secs(15))
            } else {
                heartbeat_deadline.min(last + Duration::from_secs(15))
            };
            let write = read_wanted == Interest::Write
                || current
                    .as_ref()
                    .is_some_and(|frame| frame.wanted() == Interest::Write);
            let ready = if read_wanted == Interest::Read && stream.buffered_read_size()? > 0 {
                crate::readiness::Ready {
                    read: true,
                    write: false,
                    wake: false,
                }
            } else {
                waiter.wait(stream.get_ref(), true, write, deadline)?
            };
            if ready.read || (ready.write && read_wanted == Interest::Write) {
                let mut bytes = [0; 65536];
                stream.get_ref().reset();
                match stream.read(&mut bytes) {
                    Ok(0) => return Err(Error::Disconnected),
                    Ok(n) => {
                        decoder.feed(&bytes[..n])?;
                        read_wanted = Interest::Read;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        read_wanted = stream.get_ref().blocked(Interest::Read);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(())
    })();
    if let Ok(mut state) = service.lock() {
        state.leave(&identity, &out);
    }
    out.close();
    // Finish an already-started frame before Close, with one shared bounded
    // close deadline. Never splice a Close header into an incomplete payload.
    let close_deadline = Instant::now() + Duration::from_secs(1);
    let pending_done = match current.take() {
        Some(mut frame) => frame.finish(&mut stream, &waiter, close_deadline).is_ok(),
        None => true,
    };
    let status = if result.is_ok() { 1000u16 } else { 1008 };
    if pending_done {
        let payload = echoed_close.unwrap_or_else(|| status.to_be_bytes().to_vec());
        if let Ok(mut close) = FrameWrite::new(8, &payload, None) {
            let _ = close.finish(&mut stream, &waiter, close_deadline);
        }
    }
    let _ = stream.get_ref().shutdown(std::net::Shutdown::Both);
    result
}
fn send(stream: &mut PeerTls, value: &Value) -> Result<(), Error> {
    stream.write_all(&server_frame(1, &serde_json::to_vec(value)?)?)?;
    Ok(())
}
fn receive_json(
    stream: &mut PeerTls,
    decoder: &mut Decoder,
    deadline: Instant,
) -> Result<Value, Error> {
    loop {
        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        stream
            .get_ref()
            .set_read_timeout(Some(deadline.saturating_duration_since(Instant::now())))?;
        if let Some(event) = decoder.next() {
            match event {
                FrameEvent::Message(bytes) => return Ok(serde_json::from_slice(&bytes)?),
                FrameEvent::Ping(bytes) => stream.write_all(&server_frame(10, &bytes)?)?,
                FrameEvent::Pong(_) => {}
                FrameEvent::Close(_) => return Err(Error::Disconnected),
            }
        } else {
            let mut bytes = [0; 65536];
            let n = stream.read(&mut bytes)?;
            if n == 0 {
                return Err(Error::Disconnected);
            }
            decoder.feed(&bytes[..n])?;
        }
    }
}
/// Offered extensions (every browser offers permessage-deflate) are ignored and
/// never negotiated: the 101 response names none, so frames stay uncompressed.
pub fn validate_upgrade(header: &[u8], origins: &[String]) -> Result<String, Error> {
    let text = std::str::from_utf8(header).map_err(|_| protocol("Upgrade header is not UTF-8"))?;
    if text.len() > HEADER_LIMIT || !text.ends_with("\r\n\r\n") {
        return Err(protocol("Invalid Upgrade header"));
    }
    let mut lines = text.split("\r\n");
    if lines.next() != Some("GET /control HTTP/1.1") {
        return Err(protocol("Only /control WebSocket endpoint is available"));
    }
    let mut fields = std::collections::BTreeMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (k, v) = line
            .split_once(':')
            .ok_or_else(|| protocol("Malformed Upgrade header"))?;
        if k.is_empty()
            || !k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || v.bytes().any(|b| b < 32 || b == 127)
            || fields.insert(k.to_ascii_lowercase(), v.trim()).is_some()
        {
            return Err(protocol("Duplicate or malformed Upgrade header"));
        }
    }
    if fields
        .get("upgrade")
        .map_or(true, |v| !v.eq_ignore_ascii_case("websocket"))
        || fields.get("connection").map_or(true, |v| {
            !v.split(',')
                .any(|s| s.trim().eq_ignore_ascii_case("upgrade"))
        })
        || fields.get("sec-websocket-version") != Some(&"13")
        || fields
            .get("sec-websocket-protocol")
            .map_or(true, |v| !v.split(',').any(|s| s.trim() == "soundroom.v3"))
        || !fields.contains_key("host")
        || fields.contains_key("content-length")
        || fields.contains_key("transfer-encoding")
    {
        return Err(protocol("Invalid or unsupported WebSocket Upgrade"));
    }
    if let Some(origin) = fields.get("origin") {
        if !origins.iter().any(|o| o == origin) {
            return Err(protocol("Origin was not approved"));
        }
    }
    let key = fields
        .get("sec-websocket-key")
        .ok_or_else(|| protocol("Upgrade key is missing"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(key)
        .map_err(|_| protocol("Invalid Upgrade key"))?;
    if bytes.len() != 16 || base64::engine::general_purpose::STANDARD.encode(bytes) != *key {
        return Err(protocol("Invalid Upgrade nonce"));
    }
    Ok((*key).into())
}
