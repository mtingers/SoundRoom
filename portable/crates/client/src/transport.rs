use crate::{
    validation::MAX_FRAME_BYTES,
    websocket::{self, Decoder, FrameEvent},
    Error,
};
use base64::Engine;
use native_tls::{Certificate, Protocol, TlsConnector, TlsStream};
use std::{
    io::{Read, Write},
    net::{Shutdown, TcpStream, ToSocketAddrs},
    time::{Duration, Instant},
};

/// Swappable transport boundary. UI callers run this on a control worker.
pub trait ControlTransport: Send {
    fn send(&mut self, json: &[u8]) -> Result<(), Error>;
    fn receive(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, Error>;
    fn close(&mut self) -> Result<(), Error>;
}
#[derive(Clone)]
pub struct WssConfig {
    pub endpoint: String,
    /// Origin explicitly approved by the hardware host; defaults to HTTPS origin.
    pub origin: Option<String>,
    /// Optional private CA certificate, still with normal hostname verification.
    pub root_ca_pem: Option<Vec<u8>>,
    pub connect_timeout: Duration,
}
impl WssConfig {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            origin: None,
            root_ca_pem: None,
            connect_timeout: Duration::from_secs(10),
        }
    }
}
pub struct WssTransport {
    stream: TlsStream<TcpStream>,
    decoder: Decoder,
    closed: bool,
}
impl WssTransport {
    pub fn connect(config: &WssConfig) -> Result<Self, Error> {
        let url = url::Url::parse(&config.endpoint)
            .map_err(|_| Error::Protocol("Invalid WSS endpoint".into()))?;
        if url.scheme() != "wss"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::Protocol(
                "A credential-free WSS URL is required".into(),
            ));
        }
        let hostname = match url
            .host()
            .ok_or_else(|| Error::Protocol("WSS host is missing".into()))?
        {
            url::Host::Domain(host) => host.to_owned(),
            url::Host::Ipv4(host) => host.to_string(),
            url::Host::Ipv6(host) => host.to_string(),
        };
        let host = hostname.as_str();
        let port = url.port_or_known_default().unwrap_or(443);
        let start = Instant::now();
        let mut stream = None;
        let mut last_error = None;
        for address in (host, port).to_socket_addrs()? {
            let left = config
                .connect_timeout
                .checked_sub(start.elapsed())
                .ok_or(Error::Timeout)?;
            match TcpStream::connect_timeout(&address, left) {
                Ok(value) => {
                    stream = Some(value);
                    break;
                }
                Err(error) => last_error = Some(error),
            }
        }
        let stream =
            stream.ok_or_else(|| last_error.map(Error::Io).unwrap_or(Error::Disconnected))?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(config.connect_timeout))?;
        stream.set_write_timeout(Some(config.connect_timeout))?;
        let mut builder = TlsConnector::builder();
        builder.min_protocol_version(Some(Protocol::Tlsv12));
        if let Some(pem) = &config.root_ca_pem {
            builder.add_root_certificate(Certificate::from_pem(pem)?);
        }
        let connector = builder.build()?;
        let mut stream = connector
            .connect(host, stream)
            .map_err(|error| match error {
                native_tls::HandshakeError::Failure(error) => Error::Tls(error),
                native_tls::HandshakeError::WouldBlock(_) => Error::Timeout,
            })?;
        let mut nonce = [0u8; 16];
        getrandom::getrandom(&mut nonce)
            .map_err(|_| Error::Protocol("Operating-system randomness unavailable".into()))?;
        let key = base64::engine::general_purpose::STANDARD.encode(nonce);
        let authority = &url[url::Position::BeforeHost..url::Position::AfterPort];
        let path = &url[url::Position::BeforePath..url::Position::AfterQuery];
        let default_origin = format!("https://{authority}");
        let origin = config.origin.as_deref().unwrap_or(&default_origin);
        if origin.contains(['\r', '\n']) || origin.len() > 1024 {
            return Err(Error::Protocol("Invalid Origin header".into()));
        }
        let request=format!("GET {path} HTTP/1.1\r\nHost: {authority}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: soundroom.v3\r\nOrigin: {origin}\r\n\r\n");
        stream.write_all(request.as_bytes())?;
        stream.flush()?;
        let mut header = Vec::new();
        let deadline = Instant::now() + config.connect_timeout;
        // Upgrade headers are tiny and capped. Byte reads cannot consume first frame.
        while !header.ends_with(b"\r\n\r\n") {
            if header.len() >= 16384 || Instant::now() >= deadline {
                return Err(Error::Protocol(
                    "Invalid or timed-out WebSocket Upgrade".into(),
                ));
            }
            stream.get_ref().set_read_timeout(Some(
                deadline
                    .saturating_duration_since(Instant::now())
                    .max(Duration::from_millis(1)),
            ))?;
            let mut b = [0u8; 1];
            stream.read_exact(&mut b)?;
            header.push(b[0]);
        }
        validate_upgrade(&header, &key)?;
        Ok(Self {
            stream,
            decoder: Decoder::default(),
            closed: false,
        })
    }
    fn control(&mut self, opcode: u8, payload: &[u8]) -> Result<(), Error> {
        let data = websocket::client_frame(opcode, payload)?;
        self.stream.write_all(&data)?;
        self.stream.flush()?;
        Ok(())
    }
    fn queued(&mut self) -> Result<Option<Vec<u8>>, Error> {
        while let Some(event) = self.decoder.next() {
            match event {
                FrameEvent::Message(bytes) => return Ok(Some(bytes)),
                FrameEvent::Ping(bytes) => self.control(10, &bytes)?,
                FrameEvent::Pong(_) => {}
                FrameEvent::Close(bytes) => {
                    if !self.closed {
                        self.control(8, &bytes)?;
                    }
                    self.closed = true;
                    let _ = self.stream.get_ref().shutdown(Shutdown::Both);
                    return Err(Error::Disconnected);
                }
            }
        }
        Ok(None)
    }
}
impl ControlTransport for WssTransport {
    fn send(&mut self, json: &[u8]) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Disconnected);
        }
        if json.len() > MAX_FRAME_BYTES {
            return Err(Error::QueueFull);
        }
        self.control(1, json)
    }
    fn receive(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, Error> {
        if self.closed {
            return Err(Error::Disconnected);
        }
        if let Some(message) = self.queued()? {
            return Ok(Some(message));
        }
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            self.stream
                .get_ref()
                .set_read_timeout(Some(remaining.max(Duration::from_millis(1))))?;
            let mut chunk = [0u8; 16384];
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    self.closed = true;
                    return Err(Error::Disconnected);
                }
                Ok(n) => {
                    self.decoder.feed(&chunk[..n])?;
                    if let Some(message) = self.queued()? {
                        return Ok(Some(message));
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    return Ok(None)
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
    fn close(&mut self) -> Result<(), Error> {
        if self.closed {
            return Ok(());
        }
        self.stream
            .get_ref()
            .set_write_timeout(Some(Duration::from_secs(1)))?;
        let send = self.control(8, &1000u16.to_be_bytes());
        let deadline = Instant::now() + Duration::from_secs(1);
        while send.is_ok() && !self.closed && Instant::now() < deadline {
            match self.receive(deadline.saturating_duration_since(Instant::now())) {
                Err(Error::Disconnected) => break,
                Err(_) => break,
                _ => {}
            }
        }
        self.closed = true;
        let _ = self.stream.get_ref().shutdown(Shutdown::Both);
        send
    }
}
impl Drop for WssTransport {
    fn drop(&mut self) {
        let _ = self.stream.get_ref().shutdown(Shutdown::Both);
    }
}
fn validate_upgrade(bytes: &[u8], key: &str) -> Result<(), Error> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| Error::Protocol("Upgrade headers are not UTF-8".into()))?;
    let mut lines = text.split("\r\n");
    let status = lines.next().unwrap_or("");
    if !status.starts_with("HTTP/1.1 101 ") {
        return Err(Error::Protocol("Server rejected WebSocket Upgrade".into()));
    }
    let mut headers = std::collections::BTreeMap::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| Error::Protocol("Invalid Upgrade header".into()))?;
        let name = name.to_ascii_lowercase();
        if headers.insert(name, value.trim()).is_some() {
            return Err(Error::Protocol("Duplicate Upgrade header".into()));
        }
    }
    if !headers
        .get("upgrade")
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
        || !headers.get("connection").is_some_and(|v| {
            v.split(',')
                .any(|t| t.trim().eq_ignore_ascii_case("upgrade"))
        })
        || headers.get("sec-websocket-accept") != Some(&websocket::upgrade_accept(key).as_str())
        || headers.get("sec-websocket-protocol") != Some(&"soundroom.v3")
        || headers.contains_key("sec-websocket-extensions")
    {
        return Err(Error::Protocol("Invalid WebSocket Upgrade response".into()));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upgrade_requires_accept_and_selected_protocol() {
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        let response=format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\nSec-WebSocket-Protocol: soundroom.v3\r\n\r\n",websocket::upgrade_accept(key));
        assert!(validate_upgrade(response.as_bytes(), key).is_ok());
        for bad in [
            response.replace("soundroom.v3", "other"),
            response.replace("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=", "bad"),
            response.replace("101 ", "200 "),
            response.replace("Upgrade: websocket", "Upgrade: other"),
        ] {
            assert!(validate_upgrade(bad.as_bytes(), key).is_err());
        }
    }
}
