use soundroom_client::{
    auth::Credential,
    client::{Client, ConnectionState, Notice},
    models::*,
    transport::WssConfig,
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, SyncSender, TryRecvError},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
#[derive(Clone, Debug, PartialEq)]
pub struct ResponseTag {
    pub operation: String,
    pub workspace_id: Option<String>,
    pub track_id: Option<String>,
    pub effect_id: Option<String>,
    pub range: Option<(i64, i64)>,
    pub source_revision: Option<i64>,
    pub track_revision: Option<i64>,
}
impl ResponseTag {
    fn from(
        command: &RemoteCommand,
        revision: Option<i64>,
        tracks: &BTreeMap<String, i64>,
    ) -> Option<Self> {
        let RemoteCommand::Editor { intent } = command else {
            return None;
        };
        if !matches!(
            intent.operation.as_str(),
            "pitch.analyze"
                | "pitch.render"
                | "pitch.targets"
                | "master.proposal"
                | "master.analyze"
                | "master.applyProposal"
                | "designer.preview"
                | "drums.preview"
        ) {
            return None;
        }
        let track_id = intent
            .strings
            .get("trackID")
            .cloned()
            .or_else(|| intent.context.track_id.clone());
        let effect_id = intent
            .strings
            .get("effectID")
            .cloned()
            .or_else(|| intent.context.effect_id.clone());
        let start = intent
            .integers
            .get("start")
            .copied()
            .or(intent.context.range_start);
        let end = intent
            .integers
            .get("end")
            .copied()
            .or(intent.context.range_end);
        let track_revision = track_id.as_ref().and_then(|id| {
            tracks
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(id))
                .map(|(_, v)| *v)
        });
        Some(Self {
            operation: intent.operation.clone(),
            workspace_id: intent.strings.get("workspaceID").cloned(),
            track_id,
            effect_id,
            range: start.zip(end),
            source_revision: revision,
            track_revision,
        })
    }
}
#[derive(Clone)]
pub struct EditorResponse {
    pub tag: ResponseTag,
    pub reply: Arc<RemoteReply>,
}
#[derive(Clone)]
pub struct WorkFailure {
    pub authored: bool,
    pub tag: Option<ResponseTag>,
    pub uncertain: bool,
    pub message: String,
}
impl WorkFailure {
    pub fn message(mut self, message: String) -> Self {
        self.message = message.chars().take(4096).collect();
        self
    }
}
#[derive(Clone)]
pub struct Shared {
    pub snapshot: Option<Arc<RemoteSnapshot>>,
    pub capabilities: Option<Arc<RemoteCapabilities>>,
    pub transport: Option<(RemoteTransport, Instant)>,
    pub role: Option<RemoteRole>,
    pub client_id: Option<String>,
    pub connected: bool,
    pub status: String,
    pub messages: VecDeque<String>,
    pub events: VecDeque<RemoteEvent>,
    pub receipts: VecDeque<RemoteReply>,
    pub mutation_receipts: VecDeque<RemoteReply>,
    pub responses: VecDeque<EditorResponse>,
    pub failures: VecDeque<WorkFailure>,
    pub fresh_snapshot: bool,
    pub transfer: Option<crate::transfers::TransferView>,
    pub artifacts: Vec<RemoteArtifact>,
    pub local_files: VecDeque<crate::localfiles::Response>,
}
impl Default for Shared {
    fn default() -> Self {
        Self {
            snapshot: None,
            capabilities: None,
            transport: None,
            role: None,
            client_id: None,
            connected: false,
            status: "Connecting…".into(),
            messages: VecDeque::new(),
            events: VecDeque::new(),
            receipts: VecDeque::new(),
            mutation_receipts: VecDeque::new(),
            responses: VecDeque::new(),
            failures: VecDeque::new(),
            fresh_snapshot: false,
            transfer: None,
            artifacts: vec![],
            local_files: VecDeque::new(),
        }
    }
}
pub enum Work {
    Command {
        command: Box<RemoteCommand>,
        revision: Option<i64>,
        tracks: BTreeMap<String, i64>,
        authored: bool,
    },
    Reconnect,
    Transfer(crate::transfers::TransferRequest),
    LocalFile(crate::localfiles::Request),
    CancelTransfer,
    Close,
}
impl Work {
    pub fn failure(&self, uncertain: bool, message: String) -> WorkFailure {
        let (authored, tag) = if let Self::Command {
            command,
            revision,
            tracks,
            authored,
        } = self
        {
            (*authored, ResponseTag::from(command, *revision, tracks))
        } else if let Self::LocalFile(request) = self {
            (
                false,
                Some(ResponseTag {
                    operation: "local.file".into(),
                    workspace_id: Some(request.id.clone()),
                    track_id: None,
                    effect_id: None,
                    range: None,
                    source_revision: None,
                    track_revision: None,
                }),
            )
        } else {
            (false, None)
        };
        WorkFailure {
            authored,
            tag,
            uncertain,
            message: String::new(),
        }
        .message(message)
    }
}
fn retain_failure(shared: &mut Shared, failure: WorkFailure) {
    shared.status = failure.message.clone();
    if failure.authored || failure.tag.is_some() {
        if shared.failures.len() == 64 {
            shared.failures.pop_front();
        }
        shared.failures.push_back(failure);
    }
}
pub struct Worker {
    pub state: Arc<Mutex<Shared>>,
    sender: SyncSender<(Work, usize)>,
    priority: SyncSender<(Work, usize)>,
    queued_bytes: Arc<AtomicUsize>,
}
impl Worker {
    pub fn start(config: WssConfig, credential: Credential, name: String) -> Self {
        let state = Arc::new(Mutex::new(Shared::default()));
        let copy = state.clone();
        let queued_bytes = Arc::new(AtomicUsize::new(0));
        let worker_bytes = queued_bytes.clone();
        let (tx, rx) = mpsc::sync_channel(56);
        let (priority_tx, priority_rx) = mpsc::sync_channel(8);
        thread::spawn(move || {
            let mut client = match Client::connect(&config, credential.clone(), &name, None) {
                Ok(c) => Some(c),
                Err(e) => {
                    message(&copy, format!("Connection failed: {e}"));
                    None
                }
            };
            let mut pending: BTreeMap<String, (Instant, bool, Option<ResponseTag>)> =
                BTreeMap::new();
            let mut capability_installed = false;
            let mut transfers = crate::transfers::Transfers::default();
            let mut author_revision: Option<i64> = None;
            let local_files = crate::localfiles::Actor::default();
            loop {
                for _ in 0..16 {
                    match priority_rx
                        .try_recv()
                        .or_else(|error| {
                            if matches!(error, TryRecvError::Empty) {
                                rx.try_recv()
                            } else {
                                Err(error)
                            }
                        })
                        .map(|(work, bytes)| {
                            worker_bytes.fetch_sub(bytes, Ordering::Relaxed);
                            work
                        }) {
                        Ok(Work::Close) | Err(TryRecvError::Disconnected) => {
                            if let Some(c) = client.as_mut() {
                                let _ = c.close();
                            }
                            return;
                        }
                        Ok(Work::LocalFile(request)) => {
                            if let Err(response) = local_files.submit(request) {
                                retain_local_file(&copy, response);
                            }
                        }
                        Ok(Work::Transfer(request)) => {
                            if client
                                .as_ref()
                                .is_some_and(|c| c.connection == ConnectionState::Ready)
                            {
                                if let Err(error) = transfers.start(request) {
                                    message(&copy, error);
                                }
                            } else {
                                message(
                                    &copy,
                                    "Connect and synchronize before starting a transfer.".into(),
                                );
                            }
                        }
                        Ok(Work::CancelTransfer) => {
                            let canceled = if let (Some(c), Some(id)) =
                                (client.as_mut(), transfers.pending_id().map(str::to_owned))
                            {
                                c.cancel_queued(&id)
                            } else {
                                false
                            };
                            transfers.cancel(canceled);
                        }
                        Ok(Work::Reconnect) => {
                            if let Some(c) = client.as_mut() {
                                let _ = c.close();
                                let uncertain = c.uncertain_requests();
                                if let Some(id) = transfers.pending_id() {
                                    c.cancel_queued(id);
                                }
                                transfers.disconnect(&uncertain);
                                for (id, (_, authored, tag)) in std::mem::take(&mut pending) {
                                    let sent = uncertain.contains(&id);
                                    if !sent {
                                        c.cancel_queued(&id);
                                    }
                                    retain_failure(
                                        &mut copy.lock().unwrap(),
                                        WorkFailure {
                                            authored,
                                            tag,
                                            uncertain: sent,
                                            message: if sent {
                                                format!("Outcome uncertain for {id}; a fresh snapshot is required.")
                                            } else {
                                                "Not sent: reconnect discarded a queued request."
                                                    .into()
                                            },
                                        },
                                    );
                                }
                                match c.reconnect(&config) {
                                    Ok(()) => {
                                        capability_installed = false;
                                        message(&copy,"Reconnected; reconciling metadata. Sent edits are not replayed.".into());
                                    }
                                    Err(e) => message(&copy, format!("Reconnect failed: {e}")),
                                }
                            } else {
                                match Client::connect(&config, credential.clone(), &name, None) {
                                    Ok(c) => client = Some(c),
                                    Err(e) => message(&copy, format!("Reconnect failed: {e}")),
                                }
                            }
                        }
                        Ok(Work::Command {
                            command,
                            revision,
                            tracks,
                            authored,
                        }) => {
                            let command = *command;
                            if let Some(c) = client.as_mut() {
                                let timeout = if matches!(
                                    command,
                                    RemoteCommand::Editor { .. }
                                        | RemoteCommand::ExportMix { .. }
                                        | RemoteCommand::OpenProject { .. }
                                        | RemoteCommand::SaveProject { .. }
                                ) {
                                    600
                                } else {
                                    30
                                };
                                let tag = ResponseTag::from(&command, revision, &tracks);
                                match c.enqueue(command, revision, tracks) {
                                    Ok(id) => {
                                        pending.insert(
                                            id,
                                            (
                                                Instant::now() + Duration::from_secs(timeout),
                                                authored,
                                                tag,
                                            ),
                                        );
                                    }
                                    Err(e) => retain_failure(
                                        &mut copy.lock().unwrap(),
                                        WorkFailure {
                                            authored,
                                            tag,
                                            uncertain: false,
                                            message: format!("Not sent: {e}"),
                                        },
                                    ),
                                }
                            } else {
                                let tag = ResponseTag::from(&command, revision, &tracks);
                                retain_failure(
                                    &mut copy.lock().unwrap(),
                                    WorkFailure {
                                        authored,
                                        tag,
                                        uncertain: false,
                                        message: "Disconnected: command was not sent.".into(),
                                    },
                                );
                            }
                        }
                        Err(TryRecvError::Empty) => break,
                    }
                }
                if let Some(c) = client.as_mut() {
                    match c.poll(Duration::from_millis(15)) {
                        Ok(notice) => {
                            let mut shared = copy.lock().unwrap();
                            shared.connected = c.connection == ConnectionState::Ready;
                            shared.role = c.role.clone();
                            shared.client_id = Some(c.client_id.clone());
                            if !capability_installed {
                                if let Some(cap) = c.capabilities.as_ref() {
                                    shared.capabilities = Some(Arc::new(cap.clone()));
                                    capability_installed = true;
                                }
                            }
                            match notice {
                                Some(Notice::Snapshot { .. } | Notice::Changed { .. }) => {
                                    if matches!(notice, Some(Notice::Snapshot { .. })) {
                                        shared.fresh_snapshot = true;
                                    }
                                    if let Some(snapshot) = c.snapshot.as_ref() {
                                        shared.snapshot = Some(Arc::new(snapshot.clone()));
                                        shared.transport =
                                            Some((snapshot.transport.clone(), Instant::now()));
                                        shared.status = snapshot.status.clone();
                                    }
                                }
                                Some(Notice::Event(RemoteEvent::Transport { transport })) => {
                                    shared.transport = Some((transport, Instant::now()))
                                }
                                Some(Notice::Reply(reply)) => {
                                    let sent = pending.remove(&reply.request_id);
                                    if let Some((_, _, Some(tag))) = sent.as_ref() {
                                        retain_response(
                                            &mut shared,
                                            EditorResponse {
                                                tag: tag.clone(),
                                                reply: Arc::new(reply.clone()),
                                            },
                                        );
                                    }
                                    transfers.response(&reply);
                                    if sent.is_some_and(|(_, authored, _)| authored) {
                                        if reply.status == RemoteReplyStatus::Accepted {
                                            author_revision = Some(reply.revision);
                                        }
                                        if shared.mutation_receipts.len() == 32 {
                                            shared.mutation_receipts.pop_front();
                                        }
                                        shared.mutation_receipts.push_back(reply.clone());
                                    }
                                    shared.status = reply.message.clone();
                                    if shared.receipts.len() == 32 {
                                        shared.receipts.pop_front();
                                    }
                                    shared.receipts.push_back(reply);
                                }
                                Some(Notice::Event(event)) => {
                                    if shared.events.len() == 32 {
                                        shared.events.pop_front();
                                    }
                                    shared.events.push_back(event);
                                }
                                Some(Notice::Uncertain { request_ids }) => {
                                    if transfers
                                        .pending_id()
                                        .is_some_and(|id| request_ids.iter().any(|sent| sent == id))
                                    {
                                        transfers.disconnect(&request_ids);
                                    }
                                    for id in &request_ids {
                                        if let Some((_, authored, tag)) = pending.remove(id) {
                                            retain_failure(&mut shared,WorkFailure{authored,tag,uncertain:true,message:format!("Outcome uncertain for {id}; refresh before further edits.")});
                                        }
                                    }
                                    shared.status=format!("Uncertain receipt for {} sent edits. Refresh before retrying.",request_ids.len());
                                }
                                Some(Notice::ResyncRequired) => {
                                    shared.status = "Refreshing authoritative metadata.".into()
                                }
                                _ => {}
                            }
                        }
                        Err(e) => {
                            let uncertain = c.uncertain_requests();
                            if let Some(id) = transfers.pending_id() {
                                c.cancel_queued(id);
                            }
                            transfers.disconnect(&uncertain);
                            let mut shared = copy.lock().unwrap();
                            shared.connected = false;
                            for (id, (_, authored, tag)) in std::mem::take(&mut pending) {
                                let sent = uncertain.contains(&id);
                                if !sent {
                                    c.cancel_queued(&id);
                                }
                                retain_failure(
                                    &mut shared,
                                    WorkFailure {
                                        authored,
                                        tag,
                                        uncertain: sent,
                                        message: if sent {
                                            format!("Outcome uncertain for {id}; reconnect and refresh before further edits.")
                                        } else {
                                            format!("Not sent: connection failed ({e}).")
                                        },
                                    },
                                );
                            }
                            drop(shared);
                            message(&copy,format!("Connection: {e}. :reconnect explicitly reconnects; edits are not replayed."));
                            thread::sleep(Duration::from_millis(100));
                        }
                    }
                    let expired: Vec<_> = pending
                        .iter()
                        .filter(|(_, deadline)| Instant::now() >= deadline.0)
                        .map(|(id, _)| id.clone())
                        .collect();
                    for id in expired {
                        if let Some((_, authored, tag)) = pending.remove(&id) {
                            let canceled = c.cancel_queued(&id);
                            retain_failure(
                                &mut copy.lock().unwrap(),
                                WorkFailure {
                                    authored,
                                    tag,
                                    uncertain: !canceled,
                                    message: if canceled {
                                        format!("Not sent: queued request {id} expired.")
                                    } else {
                                        format!("No receipt for {id}. Outcome uncertain; refresh before further edits.")
                                    },
                                },
                            );
                        }
                    }
                    if author_revision
                        .is_some_and(|r| c.snapshot.as_ref().is_some_and(|s| s.revision >= r))
                    {
                        author_revision = None;
                    }
                    let allow_finish = author_revision.is_none()
                        && !pending.values().any(|(_, authored, _)| *authored);
                    if c.connection == ConnectionState::Ready {
                        match transfers.next(allow_finish) {
                            Ok(Some(action)) => {
                                let authored = action.authored;
                                match c.enqueue(action.command, action.revision, action.tracks) {
                                    Ok(id) => transfers.sent(id, action.timeout),
                                    Err(error) => transfers.reject(authored, error.to_string()),
                                }
                            }
                            Ok(None) => {}
                            Err(error) => transfers.reject(false, error),
                        }
                    }
                } else {
                    thread::sleep(Duration::from_millis(30));
                }
                if let Some(reply) = transfers.receipt.as_ref() {
                    author_revision = Some(reply.revision);
                }
                publish_transfer(&mut transfers, &copy);
                if let Some(response) = local_files.poll() {
                    retain_local_file(&copy, response);
                }
            }
        });
        Self {
            state,
            sender: tx,
            priority: priority_tx,
            queued_bytes,
        }
    }
    pub fn send(&self, work: Work) -> Result<(), String> {
        let bytes = if let Work::Command { command, .. } = &work {
            serde_json::to_vec(command)
                .map_err(|e| e.to_string())?
                .len()
        } else if let Work::LocalFile(request) = &work {
            request.bytes()
        } else {
            0
        };
        if matches!(work, Work::Command { .. })
            && bytes > soundroom_client::validation::MAX_FRAME_BYTES - 4096
        {
            return Err("Control request exceeds the host frame limit.".into());
        }
        let priority = is_priority(&work);
        let limit = if priority {
            32 * 1024 * 1024
        } else {
            31 * 1024 * 1024
        };
        self.queued_bytes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(bytes).filter(|next| *next <= limit)
            })
            .map_err(|_| "Control queue byte budget is full.".to_string())?;
        let lane = if priority {
            &self.priority
        } else {
            &self.sender
        };
        if lane.try_send((work, bytes)).is_err() {
            self.queued_bytes.fetch_sub(bytes, Ordering::Relaxed);
            return Err("Control worker queue is full or disconnected.".into());
        }
        Ok(())
    }
}
fn publish_transfer(transfer: &mut crate::transfers::Transfers, state: &Arc<Mutex<Shared>>) {
    let mut shared = state.lock().unwrap();
    shared.transfer = transfer.view.clone();
    shared.artifacts = transfer.artifacts.clone();
    if let Some((authored, uncertain, message)) = transfer.failure.take() {
        retain_failure(
            &mut shared,
            WorkFailure {
                authored,
                uncertain,
                tag: None,
                message,
            },
        );
    }
    if let Some(reply) = transfer.receipt.take() {
        if shared.mutation_receipts.len() == 32 {
            shared.mutation_receipts.pop_front();
        }
        shared.mutation_receipts.push_back(reply);
    }
}
fn retain_local_file(state: &Arc<Mutex<Shared>>, response: crate::localfiles::Response) {
    let mut shared = state.lock().unwrap();
    if shared.local_files.len() == 2 {
        if let Some(dropped) = shared.local_files.pop_front() {
            retain_failure(&mut shared,WorkFailure{authored:false,uncertain:false,tag:Some(ResponseTag{operation:"local.file".into(),workspace_id:Some(dropped.id.clone()),track_id:None,effect_id:None,range:None,source_revision:None,track_revision:None}),message:format!("Local result {} was not delivered before the bounded queue filled. Saved files may exist; no track was changed.",dropped.id)});
        }
    }
    shared.local_files.push_back(response);
}
fn message(state: &Arc<Mutex<Shared>>, message: String) {
    let mut state = state.lock().unwrap();
    state.status = message.clone();
    if state.messages.len() == 32 {
        state.messages.pop_front();
    }
    state.messages.push_back(message);
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.priority.try_send((Work::Close, 0));
    }
}

/// Payload-bearing workspace responses never accumulate an unbounded queue.
fn retain_response(shared: &mut Shared, response: EditorResponse) {
    let bytes = |r: &EditorResponse| {
        r.reply.payload.as_ref().map_or(0, String::len) + r.reply.message.len() + 512
    };
    if bytes(&response) > 32 * 1024 * 1024 {
        return;
    }
    while shared.responses.len() >= 8
        || shared.responses.iter().map(bytes).sum::<usize>() + bytes(&response) > 32 * 1024 * 1024
    {
        shared.responses.pop_front();
    }
    shared.responses.push_back(response);
}

fn is_priority(work: &Work) -> bool {
    match work {
        Work::Close | Work::CancelTransfer | Work::Reconnect => true,
        Work::Command { command, .. } => match command.as_ref() {
            RemoteCommand::Stop | RemoteCommand::TakeTransport => true,
            RemoteCommand::Editor { intent } => matches!(
                intent.operation.as_str(),
                "audition.stop"
                    | "transport.pause"
                    | "transport.stop"
                    | "transport.finishRecording"
                    | "designer.stop"
                    | "designer.noteOff"
                    | "designer.allNotesOff"
                    | "drums.stop"
                    | "take.stop"
                    | "jobs.cancel"
            ),
            _ => false,
        },
        _ => false,
    }
}
impl Shared {
    pub fn frame(&mut self) -> Self {
        let mut frame = Self {
            snapshot: self.snapshot.clone(),
            capabilities: self.capabilities.clone(),
            transport: self.transport.clone(),
            role: self.role.clone(),
            client_id: self.client_id.clone(),
            connected: self.connected,
            status: self.status.clone(),
            messages: self.messages.clone(),
            events: VecDeque::new(),
            receipts: VecDeque::new(),
            mutation_receipts: VecDeque::new(),
            responses: VecDeque::new(),
            failures: VecDeque::new(),
            fresh_snapshot: std::mem::take(&mut self.fresh_snapshot),
            transfer: self.transfer.clone(),
            artifacts: self.artifacts.clone(),
            local_files: VecDeque::new(),
        };
        frame.events = std::mem::take(&mut self.events);
        frame.receipts = std::mem::take(&mut self.receipts);
        frame.mutation_receipts = std::mem::take(&mut self.mutation_receipts);
        frame.responses = std::mem::take(&mut self.responses);
        frame.failures = std::mem::take(&mut self.failures);
        frame.local_files = std::mem::take(&mut self.local_files);
        frame
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn work(command: RemoteCommand) -> Work {
        Work::Command {
            command: Box::new(command),
            revision: None,
            tracks: BTreeMap::new(),
            authored: false,
        }
    }
    #[test]
    fn transfer_cancel_and_reconnect_use_the_reserved_interrupt_lane() {
        assert!(is_priority(&Work::CancelTransfer));
        assert!(is_priority(&Work::Reconnect));
        let download = Work::Transfer(crate::transfers::TransferRequest::Export {
            path: "mix.wav".into(),
            name: "Mix".into(),
        });
        assert!(!is_priority(&download));
        assert!(!download.failure(false, "Queue full".into()).authored);
    }
    #[test]
    fn analysis_response_tag_preserves_source_scope_and_revision() {
        let context:RemoteEditorContext=serde_json::from_value(serde_json::json!({"trackID":"AAAAAAAA-0000-4000-8000-000000000001","effectID":"BBBBBBBB-0000-4000-8000-000000000001","clipIDs":[],"cursorFrame":10,"rangeStart":10,"rangeEnd":100})).unwrap();
        let command = RemoteCommand::Editor {
            intent: RemoteEditorIntent {
                operation: "pitch.analyze".into(),
                context,
                integers: BTreeMap::new(),
                numbers: BTreeMap::new(),
                strings: BTreeMap::new(),
                flags: BTreeMap::new(),
                payload: None,
            },
        };
        let tracks = BTreeMap::from([("aaaaaaaa-0000-4000-8000-000000000001".into(), 7)]);
        let tag = ResponseTag::from(&command, Some(9), &tracks).unwrap();
        assert_eq!(tag.track_revision, Some(7));
        assert_eq!(tag.source_revision, Some(9));
        assert_eq!(tag.range, Some((10, 100)));
        assert!(ResponseTag::from(&RemoteCommand::Stop, None, &BTreeMap::new()).is_none());
    }
    #[test]
    fn responses_are_bounded_and_drained_once_per_ui_frame() {
        let mut shared = Shared::default();
        for i in 0..20 {
            retain_response(
                &mut shared,
                EditorResponse {
                    tag: ResponseTag {
                        operation: "master.proposal".into(),
                        workspace_id: None,
                        track_id: None,
                        effect_id: None,
                        range: None,
                        source_revision: Some(i),
                        track_revision: None,
                    },
                    reply: Arc::new(RemoteReply {
                        request_id: i.to_string(),
                        status: RemoteReplyStatus::Accepted,
                        revision: i,
                        message: String::new(),
                        payload: Some("x".repeat(5 * 1024 * 1024)),
                    }),
                },
            );
        }
        assert!(shared.responses.len() <= 6);
        let frame = shared.frame();
        assert!(!frame.responses.is_empty());
        assert!(shared.frame().responses.is_empty());
    }
    #[test]
    fn ordinary_backlog_cannot_consume_stop_lane() {
        let (tx, rx) = mpsc::sync_channel(56);
        let (priority, priority_rx) = mpsc::sync_channel(8);
        let worker = Worker {
            state: Arc::new(Mutex::new(Shared::default())),
            sender: tx,
            priority,
            queued_bytes: Arc::new(AtomicUsize::new(0)),
        };
        for _ in 0..56 {
            worker.send(work(RemoteCommand::Snapshot)).unwrap();
        }
        assert!(worker.send(work(RemoteCommand::Snapshot)).is_err());
        worker.send(work(RemoteCommand::Stop)).unwrap();
        assert!(
            matches!(priority_rx.try_recv().unwrap().0,Work::Command{command,..} if matches!(*command,RemoteCommand::Stop))
        );
        assert!(rx.try_recv().is_ok());
    }
    #[test]
    fn worker_bound_precedes_channel_allocation() {
        let (tx, _rx) = mpsc::sync_channel(56);
        let (priority, _priority_rx) = mpsc::sync_channel(8);
        let worker = Worker {
            state: Arc::new(Mutex::new(Shared::default())),
            sender: tx,
            priority,
            queued_bytes: Arc::new(AtomicUsize::new(0)),
        };
        assert!(worker
            .send(work(RemoteCommand::AddTrack {
                name: "x".repeat(8 * 1024 * 1024)
            }))
            .unwrap_err()
            .contains("frame limit"));
    }
    #[test]
    fn byte_backlog_preserves_interrupt_reservation() {
        let (tx, _rx) = mpsc::sync_channel(56);
        let (priority, _priority_rx) = mpsc::sync_channel(8);
        let bytes = Arc::new(AtomicUsize::new(31 * 1024 * 1024));
        let worker = Worker {
            state: Arc::new(Mutex::new(Shared::default())),
            sender: tx,
            priority,
            queued_bytes: bytes.clone(),
        };
        assert!(worker.send(work(RemoteCommand::Snapshot)).is_err());
        assert_eq!(bytes.load(Ordering::Relaxed), 31 * 1024 * 1024);
        worker.send(work(RemoteCommand::Stop)).unwrap();
        assert!(bytes.load(Ordering::Relaxed) > 31 * 1024 * 1024);
    }
    #[test]
    fn local_failures_are_bounded_separate_from_receipts_and_drained_once() {
        let mut shared = Shared::default();
        for _ in 0..100 {
            retain_failure(
                &mut shared,
                WorkFailure {
                    authored: true,
                    tag: None,
                    uncertain: false,
                    message: "Not sent".into(),
                },
            );
        }
        assert_eq!(shared.failures.len(), 64);
        shared.fresh_snapshot = true;
        let frame = shared.frame();
        assert_eq!(frame.failures.len(), 64);
        assert!(frame.fresh_snapshot);
        assert!(frame.receipts.is_empty());
        assert!(frame.mutation_receipts.is_empty());
        let next = shared.frame();
        assert!(next.failures.is_empty());
        assert!(!next.fresh_snapshot);
    }
    #[test]
    fn per_frame_telemetry_moves_events_once() {
        let mut state = Shared::default();
        state.events.push_back(RemoteEvent::Projects {
            projects: vec!["song".into()],
        });
        let frame = state.frame();
        assert_eq!(frame.events.len(), 1);
        assert!(state.frame().events.is_empty());
    }
    #[test]
    fn local_result_eviction_releases_matching_job_instead_of_leaving_it_pending() {
        let state = Arc::new(Mutex::new(Shared::default()));
        for id in ["old", "new", "newest"] {
            retain_local_file(
                &state,
                crate::localfiles::Response {
                    workspace: "effect-library".into(),
                    id: id.into(),
                    epoch: 0,
                    result: Ok(crate::localfiles::Data::Saved),
                },
            );
        }
        let mut state = state.lock().unwrap();
        assert_eq!(state.local_files.len(), 2);
        let failure = state.failures.front().unwrap();
        assert_eq!(
            failure.tag.as_ref().unwrap().workspace_id.as_deref(),
            Some("old")
        );
        assert!(!failure.authored);
        assert!(failure.message.contains("Saved files may exist"));
        let frame = state.frame();
        assert_eq!(frame.local_files.len(), 2);
        assert!(state.local_files.is_empty());
    }
}
