use crate::{
    auth::{new_id, Credential},
    decode::{DecodeWorker, Decoded},
    models::*,
    state::{self, ViewState},
    transport::{ControlTransport, WssConfig, WssTransport},
    validation::{self, MAX_FRAME_BYTES},
    Error,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, VecDeque},
    time::{Duration, Instant},
};
const MAX_QUEUE_BYTES: usize = 32 * 1024 * 1024;
#[derive(Clone, Debug, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Authenticating,
    Syncing,
    Ready,
    Reconnecting { attempt: u32 },
}
#[derive(Clone, Debug, PartialEq)]
pub enum ReceiptState {
    Queued,
    Sent,
    Uncertain,
}
struct Pending {
    bytes: Vec<u8>,
    state: ReceiptState,
}
#[derive(Clone, Debug)]
pub enum Notice {
    Welcome { role: RemoteRole },
    Snapshot { revision: i64 },
    Changed { revision: i64 },
    Event(RemoteEvent),
    Reply(RemoteReply),
    ResyncRequired,
    Uncertain { request_ids: Vec<String> },
}
/// A single control worker owns this value. No audio device or PCM is opened here.
pub struct Client<T: ControlTransport> {
    transport: T,
    credential: Credential,
    name: String,
    pub client_id: String,
    pub connection: ConnectionState,
    pub role: Option<RemoteRole>,
    pub snapshot: Option<RemoteSnapshot>,
    pub capabilities: Option<RemoteCapabilities>,
    pub view: ViewState,
    pending: BTreeMap<String, Pending>,
    priority: VecDeque<String>,
    ordinary: VecDeque<String>,
    queue_bytes: usize,
    pending_bytes: usize,
    receipt_bytes: usize,
    receipts: BTreeMap<String, RemoteReply>,
    receipt_order: VecDeque<String>,
    decoder: DecodeWorker,
    paged_revision: Option<i64>,
    deferred: VecDeque<RemoteSessionDelta>,
    deferred_bytes: usize,
    latest_transport: Option<RemoteTransport>,
    // One validated String (at most 64 KiB), replaced rather than queued.
    latest_status: Option<String>,
    latest_peers: Option<Vec<RemotePeer>>,
    latest_devices: Option<(Vec<RemoteDevice>, u32, u32)>,
    last_receive: Instant,
    feature_declaration_sent: bool,
}
impl<T: ControlTransport> Client<T> {
    pub fn attach(
        transport: T,
        credential: Credential,
        name: impl Into<String>,
        client_id: Option<String>,
    ) -> Result<Self, Error> {
        let client_id = client_id.map(Ok).unwrap_or_else(new_id)?;
        if !validation::uuid(&client_id) {
            return Err(Error::Protocol("Invalid runtime client identity".into()));
        }
        let mut client = Self {
            transport,
            credential,
            name: name.into(),
            client_id: client_id.to_ascii_uppercase(),
            connection: ConnectionState::Authenticating,
            role: None,
            snapshot: None,
            capabilities: None,
            view: ViewState {
                zoom: 1.,
                ..Default::default()
            },
            pending: BTreeMap::new(),
            priority: VecDeque::new(),
            ordinary: VecDeque::new(),
            queue_bytes: 0,
            pending_bytes: 0,
            receipt_bytes: 0,
            receipts: BTreeMap::new(),
            receipt_order: VecDeque::new(),
            decoder: DecodeWorker::new(),
            paged_revision: None,
            deferred: VecDeque::new(),
            deferred_bytes: 0,
            latest_transport: None,
            latest_status: None,
            latest_peers: None,
            latest_devices: None,
            last_receive: Instant::now(),
            feature_declaration_sent: false,
        };
        client.authenticate()?;
        Ok(client)
    }
    fn authenticate(&mut self) -> Result<(), Error> {
        let challenge = self
            .transport
            .receive(Duration::from_secs(10))?
            .ok_or(Error::Timeout)?;
        let challenge: Value = serde_json::from_slice(&challenge)?;
        exact_keys(&challenge, &["version", "kind", "serverID", "nonce"])?;
        let hello = self
            .credential
            .hello(&self.client_id, &self.name, &challenge)?;
        self.transport.send(&serde_json::to_vec(&hello)?)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let bytes = self
                .transport
                .receive(deadline.saturating_duration_since(Instant::now()))?
                .ok_or(Error::Timeout)?;
            let value: Value = serde_json::from_slice(&bytes)?;
            match value["kind"].as_str() {
                Some("welcome") => {
                    exact_keys(&value, &["version", "kind", "serverID", "name", "role"])?;
                    if value["version"] != 3
                        || value["serverID"]
                            .as_str()
                            .map(str::to_ascii_uppercase)
                            .as_deref()
                            != Some(&self.credential.server_id)
                    {
                        return Err(Error::Protocol("Welcome identity mismatch".into()));
                    }
                    self.role = Some(serde_json::from_value(value["role"].clone())?);
                    self.connection = ConnectionState::Syncing;
                    self.last_receive = Instant::now();
                    return Ok(());
                }
                Some("heartbeat") => self.heartbeat(&value)?,
                Some("failure") => {
                    return Err(Error::Protocol(
                        "Server rejected client authentication".into(),
                    ))
                }
                _ => return Err(Error::Protocol("Expected authenticated Welcome".into())),
            }
        }
    }
    pub fn enqueue(
        &mut self,
        command: RemoteCommand,
        expected_revision: Option<i64>,
        expected_track_revisions: BTreeMap<String, i64>,
    ) -> Result<String, Error> {
        if !matches!(
            self.connection,
            ConnectionState::Ready | ConnectionState::Syncing
        ) {
            return Err(Error::Disconnected);
        }
        let priority = is_priority(&command);
        if self.pending.len() >= if priority { 64 } else { 56 } {
            return Err(Error::QueueFull);
        }
        let id = new_id()?;
        let request = RemoteRequest {
            id: id.clone(),
            client_id: self.client_id.clone(),
            expected_revision,
            expected_track_revisions,
            command,
        };
        let mut value = serde_json::to_value(&request)?;
        validation::validate_model(&value, "RemoteRequest")?;
        value["version"] = json!(3);
        value["kind"] = json!("request");
        let bytes = serde_json::to_vec(&value)?;
        if bytes.len() > MAX_FRAME_BYTES
            || self.pending_bytes
                > if priority {
                    MAX_QUEUE_BYTES
                } else {
                    MAX_QUEUE_BYTES - 65536
                } - bytes.len()
        {
            return Err(Error::QueueFull);
        }
        if priority {
            self.priority.push_back(id.clone());
        } else {
            self.ordinary.push_back(id.clone());
        }
        self.queue_bytes += bytes.len();
        self.pending_bytes += bytes.len();
        self.pending.insert(
            id.clone(),
            Pending {
                bytes,
                state: ReceiptState::Queued,
            },
        );
        Ok(id)
    }
    pub fn flush(&mut self) -> Result<(), Error> {
        if !matches!(
            self.connection,
            ConnectionState::Ready | ConnectionState::Syncing
        ) {
            return Err(Error::Disconnected);
        }
        for _ in 0..8 {
            let Some(id) = self
                .priority
                .pop_front()
                .or_else(|| self.ordinary.pop_front())
            else {
                break;
            };
            let pending = self.pending.get_mut(&id).unwrap();
            pending.state = ReceiptState::Sent;
            if let Err(error) = self.transport.send(&pending.bytes) {
                self.mark_disconnected();
                return Err(error);
            }
            self.queue_bytes -= pending.bytes.len();
        }
        Ok(())
    }
    pub fn poll(&mut self, timeout: Duration) -> Result<Option<Notice>, Error> {
        let result = self.poll_inner(timeout);
        if result.is_err() {
            self.mark_disconnected();
        }
        result
    }
    fn poll_inner(&mut self, timeout: Duration) -> Result<Option<Notice>, Error> {
        self.flush()?;
        self.declare_features()?;
        let deadline = Instant::now() + timeout;
        let mut transport_serviced = false;
        loop {
            // Service the socket once per call even when ordinary results are
            // already queued. A dense inbox must not starve host heartbeats.
            if transport_serviced {
                if let Some(decoded) = self.decoder.take()? {
                    return self.decoded(decoded);
                }
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            let bytes = self
                .transport
                .receive(remaining.min(Duration::from_millis(5)))?;
            transport_serviced = true;
            if let Some(bytes) = bytes {
                if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
                    return Err(Error::Protocol("Invalid control frame length".into()));
                }
                self.last_receive = Instant::now();
                // These bounded envelopes have no authored state to install and
                // may be serviced while an ordinary decoder is blocked.
                if bytes.len() <= 4096 {
                    let value: Value = serde_json::from_slice(&bytes)?;
                    match value["kind"].as_str() {
                        Some("heartbeat") => {
                            self.heartbeat(&value)?;
                            return Ok(None);
                        }
                        Some("heartbeatReply") => {
                            exact_keys(&value, &["version", "kind", "id"])?;
                            if value["version"] != 3
                                || !value["id"].as_str().is_some_and(validation::uuid)
                            {
                                return Err(Error::Protocol("Invalid heartbeat".into()));
                            }
                            return Ok(None);
                        }
                        Some("reply")
                            if value["requestID"].as_str().is_some_and(|id| {
                                self.pending.contains_key(id) || self.receipts.contains_key(id)
                            }) =>
                        {
                            return self.record_reply(crate::decode::reply(value)?);
                        }
                        _ => {}
                    }
                }
                self.decoder.submit(bytes)?;
            } else {
                if self.last_receive.elapsed() > Duration::from_secs(15) {
                    return Err(Error::Timeout);
                }
                // Custom transports can return immediately; do not busy-spin
                // while the bounded decode worker prepares its next result.
                if !remaining.is_zero() {
                    std::thread::sleep(Duration::from_millis(1).min(remaining));
                }
            }
            if let Some(decoded) = self.decoder.take()? {
                return self.decoded(decoded);
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
        }
    }
    fn decoded(&mut self, decoded: Decoded) -> Result<Option<Notice>, Error> {
        match decoded {
            Decoded::Capabilities(capabilities) => {
                self.capabilities = Some(*capabilities);
                self.declare_features()?;
                Ok(None)
            }
            Decoded::Begin(revision) => {
                self.latest_transport = None;
                self.latest_status = None;
                self.latest_peers = None;
                self.latest_devices = None;
                self.deferred.clear();
                self.deferred_bytes = 0;
                self.paged_revision = Some(revision);
                self.connection = ConnectionState::Syncing;
                Ok(None)
            }
            Decoded::Page => Ok(None),
            Decoded::Snapshot(snapshot) => {
                self.install(*snapshot)?;
                self.paged_revision = None;
                Ok(Some(Notice::Snapshot {
                    revision: self.snapshot.as_ref().unwrap().revision,
                }))
            }
            Decoded::Reply(reply) => self.record_reply(reply),
            Decoded::Event(event, bytes) => self.event(event, bytes),
        }
    }
    fn record_reply(&mut self, reply: RemoteReply) -> Result<Option<Notice>, Error> {
        if let Some(pending) = self.pending.remove(&reply.request_id) {
            self.pending_bytes -= pending.bytes.len();
            if pending.state == ReceiptState::Queued {
                self.queue_bytes -= pending.bytes.len();
                self.priority.retain(|id| id != &reply.request_id);
                self.ordinary.retain(|id| id != &reply.request_id);
            }
        }
        if let Some(previous) = self
            .receipts
            .insert(reply.request_id.clone(), reply.clone())
        {
            self.receipt_bytes -= receipt_size(&previous);
        }
        self.receipt_bytes += receipt_size(&reply);
        self.receipt_order.retain(|id| id != &reply.request_id);
        self.receipt_order.push_back(reply.request_id.clone());
        while self.receipt_order.len() > 512 || self.receipt_bytes > MAX_QUEUE_BYTES {
            if let Some(id) = self.receipt_order.pop_front() {
                if let Some(old) = self.receipts.remove(&id) {
                    self.receipt_bytes -= receipt_size(&old);
                }
            }
        }
        Ok(Some(Notice::Reply(reply)))
    }
    fn heartbeat(&mut self, value: &Value) -> Result<(), Error> {
        exact_keys(value, &["version", "kind", "id"])?;
        if value["version"] != 3 || !value["id"].as_str().is_some_and(validation::uuid) {
            return Err(Error::Protocol("Invalid heartbeat".into()));
        }
        self.transport.send(&serde_json::to_vec(
            &json!({"version":3,"kind":"heartbeatReply","id":value["id"]}),
        )?)
    }
    fn declare_features(&mut self) -> Result<(), Error> {
        if self.feature_declaration_sent
            || !self.capabilities.as_ref().is_some_and(|capabilities| {
                capabilities
                    .features
                    .iter()
                    .any(|feature| feature == "compact-control-patches")
                    && capabilities
                        .editor_operations
                        .iter()
                        .any(|op| op == "client.features")
            })
        {
            return Ok(());
        }
        use base64::Engine;
        let intent: RemoteEditorIntent = serde_json::from_value(json!({
            "operation":"client.features", "context":{"clipIDs":[],"cursorFrame":0},
            "integers":{},"numbers":{},"strings":{},"flags":{},
            "payload":base64::engine::general_purpose::STANDARD.encode(b"[\"compact-control-patches\"]")
        }))?;
        match self.enqueue(RemoteCommand::Editor { intent }, None, BTreeMap::new()) {
            Ok(_) => {
                self.feature_declaration_sent = true;
                Ok(())
            }
            Err(Error::QueueFull) => Ok(()), // Retry after a later bounded flush; no authored replay.
            Err(error) => Err(error),
        }
    }
    fn event(&mut self, event: RemoteEvent, bytes: usize) -> Result<Option<Notice>, Error> {
        match event {
            RemoteEvent::Snapshot { snapshot } => {
                self.install(*snapshot)?;
                Ok(Some(Notice::Snapshot {
                    revision: self.snapshot.as_ref().unwrap().revision,
                }))
            }
            RemoteEvent::Delta { delta } => {
                if self.paged_revision.is_some() {
                    if self.deferred.len() >= 64 || self.deferred_bytes > MAX_QUEUE_BYTES - bytes {
                        return Err(Error::QueueFull);
                    }
                    self.deferred_bytes += bytes;
                    // A later authored status wins over a previously retained live overlay.
                    // Deltas without status leave that overlay intact.
                    let applied_revision = self
                        .deferred
                        .iter()
                        .map(|delta| delta.revision)
                        .chain(self.paged_revision)
                        .max()
                        .unwrap_or(0);
                    if delta.status.is_some() && delta.revision >= applied_revision {
                        self.latest_status = None;
                    }
                    self.deferred.push_back(*delta);
                    return Ok(None);
                }
                if let Some(snapshot) = self.snapshot.as_mut() {
                    match state::apply_delta(snapshot, *delta) {
                        Ok(()) => Ok(Some(Notice::Changed {
                            revision: snapshot.revision,
                        })),
                        Err(Error::Conflict(_)) => {
                            self.connection = ConnectionState::Syncing;
                            self.enqueue(RemoteCommand::Snapshot, None, BTreeMap::new())?;
                            Ok(Some(Notice::ResyncRequired))
                        }
                        Err(error) => Err(error),
                    }
                } else {
                    Err(Error::Protocol("Delta before initial snapshot".into()))
                }
            }
            RemoteEvent::Transport { transport } => {
                if let Some(snapshot) = self.snapshot.as_mut() {
                    snapshot.transport = transport.clone();
                }
                if self.paged_revision.is_some() || self.snapshot.is_none() {
                    self.latest_transport = Some(transport.clone());
                }
                Ok(Some(Notice::Event(RemoteEvent::Transport { transport })))
            }
            RemoteEvent::Status { message } => {
                if self.paged_revision.is_some() || self.snapshot.is_none() {
                    self.latest_status = Some(message.clone());
                }
                if let Some(snapshot) = self.snapshot.as_mut() {
                    snapshot.status = message.clone();
                }
                Ok(Some(Notice::Event(RemoteEvent::Status { message })))
            }
            RemoteEvent::Presence { peers } => {
                if self.paged_revision.is_some() || self.snapshot.is_none() {
                    self.latest_peers = Some(peers.clone());
                }
                if let Some(snapshot) = self.snapshot.as_mut() {
                    snapshot.peers = peers.clone();
                }
                Ok(Some(Notice::Event(RemoteEvent::Presence { peers })))
            }
            RemoteEvent::Devices {
                devices,
                input_id,
                output_id,
            } => {
                if self.paged_revision.is_some() || self.snapshot.is_none() {
                    self.latest_devices = Some((devices.clone(), input_id, output_id));
                }
                if let Some(snapshot) = self.snapshot.as_mut() {
                    snapshot.devices = devices.clone();
                    snapshot.input_device_id = input_id;
                    snapshot.output_device_id = output_id;
                }
                Ok(Some(Notice::Event(RemoteEvent::Devices {
                    devices,
                    input_id,
                    output_id,
                })))
            }
            event => Ok(Some(Notice::Event(event))),
        }
    }
    fn install(&mut self, mut snapshot: RemoteSnapshot) -> Result<(), Error> {
        if !snapshot
            .server_id
            .eq_ignore_ascii_case(&self.credential.server_id)
        {
            return Err(Error::Protocol(
                "Snapshot hardware identity mismatch".into(),
            ));
        }
        while let Some(delta) = self.deferred.pop_front() {
            state::apply_delta(&mut snapshot, delta)?;
        }
        self.deferred_bytes = 0;
        if let Some(transport) = self.latest_transport.take() {
            if transport.host_time >= snapshot.transport.host_time {
                snapshot.transport = transport;
            }
        }
        if let Some(status) = self.latest_status.take() {
            snapshot.status = status;
        }
        if let Some(peers) = self.latest_peers.take() {
            snapshot.peers = peers;
        }
        if let Some((devices, input, output)) = self.latest_devices.take() {
            snapshot.devices = devices;
            snapshot.input_device_id = input;
            snapshot.output_device_id = output;
        }
        self.snapshot = Some(snapshot);
        self.connection = ConnectionState::Ready;
        Ok(())
    }
    fn mark_disconnected(&mut self) {
        if self.connection != ConnectionState::Disconnected {
            let _ = self.transport.close();
        }
        self.connection = ConnectionState::Disconnected;
        self.decoder.reset();
        self.paged_revision = None;
        self.feature_declaration_sent = false;
        self.latest_transport = None;
        self.latest_status = None;
        self.latest_peers = None;
        self.latest_devices = None;
        self.deferred.clear();
        self.deferred_bytes = 0;
        for pending in self.pending.values_mut() {
            if pending.state == ReceiptState::Sent {
                pending.state = ReceiptState::Uncertain;
            }
        }
    }
    pub fn wait_receipt(&mut self, id: &str, timeout: Duration) -> Result<RemoteReply, Error> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(reply) = self.receipts.get(id) {
                return Ok(reply.clone());
            }
            if Instant::now() >= deadline {
                if let Some(pending) = self.pending.get_mut(id) {
                    if pending.state == ReceiptState::Sent {
                        pending.state = ReceiptState::Uncertain;
                        return Err(Error::Uncertain(id.into()));
                    }
                    if pending.state == ReceiptState::Queued {
                        self.abandon(id);
                    }
                }
                return Err(Error::Timeout);
            }
            self.poll(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(50)),
            )?;
        }
    }
    pub fn request(
        &mut self,
        command: RemoteCommand,
        expected_revision: Option<i64>,
        expected_track_revisions: BTreeMap<String, i64>,
        timeout: Duration,
    ) -> Result<RemoteReply, Error> {
        let id = self.enqueue(command, expected_revision, expected_track_revisions)?;
        self.wait_receipt(&id, timeout)
    }
    pub fn uncertain_requests(&self) -> Vec<String> {
        self.pending
            .iter()
            .filter(|(_, p)| p.state == ReceiptState::Uncertain)
            .map(|(id, _)| id.clone())
            .collect()
    }
    pub fn receipt(&self, id: &str) -> Option<&RemoteReply> {
        self.receipts.get(id)
    }
    /// Explicit operator decision only: retries identical ID/bytes, never an edit with a new ID.
    /// The host's receipt cache is bounded and runtime-local; do not use after restart/expiry.
    pub fn reconcile_exact(&mut self, id: &str) -> Result<(), Error> {
        if !matches!(
            self.connection,
            ConnectionState::Ready | ConnectionState::Syncing
        ) {
            return Err(Error::Disconnected);
        }
        let pending = self
            .pending
            .get_mut(id)
            .ok_or_else(|| Error::Protocol("Unknown request identity".into()))?;
        if pending.state != ReceiptState::Uncertain {
            return Err(Error::Protocol("Request is not uncertain".into()));
        }
        if self.queue_bytes > MAX_QUEUE_BYTES - pending.bytes.len() {
            return Err(Error::QueueFull);
        }
        pending.state = ReceiptState::Queued;
        self.queue_bytes += pending.bytes.len();
        self.priority.push_back(id.into());
        Ok(())
    }
    /// Current delivery state; sent/uncertain requests retain their exact identity for reconciliation.
    pub fn request_state(&self, id: &str) -> Option<ReceiptState> {
        self.pending.get(id).map(|pending| pending.state.clone())
    }
    /// Cancels only a request proven not to have been sent. This never cancels host work.
    pub fn cancel_queued(&mut self, id: &str) -> bool {
        if self.pending.get(id).map(|pending| &pending.state) != Some(&ReceiptState::Queued) {
            return false;
        }
        self.abandon(id);
        true
    }
    /// Explicitly forgets retained delivery/reconciliation state; it does not cancel host work.
    /// UI deadlines should use cancel_queued() and retain sent/uncertain identities instead.
    pub fn abandon(&mut self, id: &str) {
        if let Some(p) = self.pending.remove(id) {
            self.pending_bytes -= p.bytes.len();
            if p.state == ReceiptState::Queued {
                self.queue_bytes -= p.bytes.len();
            }
        }
        self.priority.retain(|v| v != id);
        self.ordinary.retain(|v| v != id);
    }
    pub fn replace_transport(&mut self, transport: T) -> Result<(), Error> {
        self.mark_disconnected();
        self.transport = transport;
        self.role = None;
        self.capabilities = None;
        self.connection = ConnectionState::Authenticating;
        if let Err(error) = self.authenticate() {
            self.mark_disconnected();
            return Err(error);
        }
        // Queued edits also need explicit reconciliation after a disconnect.
        for pending in self.pending.values_mut() {
            pending.state = ReceiptState::Uncertain;
        }
        self.priority.clear();
        self.ordinary.clear();
        self.queue_bytes = 0;
        Ok(())
    }
    pub fn close(&mut self) -> Result<(), Error> {
        let result = self.transport.close();
        self.mark_disconnected();
        result
    }
}
impl Client<WssTransport> {
    pub fn connect(
        config: &WssConfig,
        credential: Credential,
        name: impl Into<String>,
        client_id: Option<String>,
    ) -> Result<Self, Error> {
        Self::attach(WssTransport::connect(config)?, credential, name, client_id)
    }
    pub fn reconnect(&mut self, config: &WssConfig) -> Result<(), Error> {
        self.replace_transport(WssTransport::connect(config)?)
    }
}
pub(crate) fn exact_keys(value: &Value, required: &[&str]) -> Result<(), Error> {
    exact_keys_optional(value, required, &[])
}
pub(crate) fn exact_keys_optional(
    value: &Value,
    required: &[&str],
    optional: &[&str],
) -> Result<(), Error> {
    let object = value
        .as_object()
        .ok_or_else(|| Error::Protocol("Envelope is not an object".into()))?;
    if required.iter().any(|k| !object.contains_key(*k))
        || object
            .keys()
            .any(|k| !required.contains(&k.as_str()) && !optional.contains(&k.as_str()))
    {
        return Err(Error::Protocol("Unknown/missing envelope field".into()));
    }
    Ok(())
}

fn receipt_size(reply: &RemoteReply) -> usize {
    reply.payload.as_ref().map_or(0, String::len) + reply.message.len() + 128
}

fn is_priority(command: &RemoteCommand) -> bool {
    matches!(
        command,
        RemoteCommand::Stop
            | RemoteCommand::Seek { .. }
            | RemoteCommand::Play { .. }
            | RemoteCommand::Record
            | RemoteCommand::TakeTransport
            | RemoteCommand::Ping { .. }
    ) || matches!(command,RemoteCommand::Editor{intent}if ["transport.stop","transport.pause","transport.toggle","designer.stop","drums.stop","jobs.cancel","designer.noteOff","designer.allNotesOff","automation.writeEnd"].contains(&intent.operation.as_str()))
}
/// Bounded exponential reconnect delay with cryptographic jitter. Never executes edits.
pub fn reconnect_delay(attempt: u32) -> Duration {
    let base = 500u64.saturating_mul(1u64 << attempt.min(4)).min(8000);
    let mut random = [0u8; 2];
    let _ = getrandom::getrandom(&mut random);
    Duration::from_millis(base + u16::from_le_bytes(random) as u64 % (base / 4 + 1))
}

#[cfg(test)]
#[path = "client_decode_tests.rs"]
mod decode_tests;
