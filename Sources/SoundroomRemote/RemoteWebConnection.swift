import Foundation
import Network

/// RFC 6455 is handled by Network.framework. JSON validation and bounded queues
/// are shared with the native channel; no websocket work enters the render path.
final class RemoteWebConnection: RemoteServerChannel, @unchecked Sendable {
    let id = UUID()
    private let connection: NWConnection
    private let capabilities: Data?
    private let queue = DispatchQueue(label: "soundroom.web.peer.\(UUID().uuidString)", qos: .userInitiated)
    private let admissionLock = NSLock()
    private var mailbox = RemoteMessageMailbox()
    private var priorityMailbox = RemoteMessageMailbox()
    private var outbound = RemoteOutboundQueue()
    private var mailboxScheduled = false
    private var priorityScheduled = false
    private var snapshotStream: RemoteWebSnapshotStream?
    private var preparation: Task<Void, Never>?
    private var admissionClosed = false
    private var closed = false
    private var sending = false
    private var ready = false
    private var document = RemoteWebDocumentState()
    private var incoming = Data()
    private var sendStarted: TimeInterval = 0
    private var lastReceive = ProcessInfo.processInfo.systemUptime
    private var received = 0
    private var sent = 0
    private var watchdog: DispatchSourceTimer?
    var onReady: (@Sendable () -> Void)?
    var onMessage: (@Sendable (RemoteWireMessage, Int, @escaping @Sendable () -> Void) -> Void)?
    var onClose: (@Sendable (Error?) -> Void)?
    var onPairingRequest: (@MainActor @Sendable (String, String) throws -> RemoteBrowserCredential)?
    private var pairingAttempted = false
    private var authenticated = false

    init(connection: NWConnection, capabilities: Data? = nil) { self.connection = connection; self.capabilities = capabilities }
    deinit { preparation?.cancel(); watchdog?.cancel(); connection.cancel() }

    func start() {
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now() + 5, repeating: 5)
        timer.setEventHandler { [weak self] in
            guard let self, self.ready, !self.closed else { return }
            let now = ProcessInfo.processInfo.systemUptime
            if now - self.lastReceive > 15 || (self.sending && now - self.sendStarted > 30) {
                self.closeOnQueue(RemoteError.timeout); return
            }
            self.send(.heartbeat(UUID()), telemetryKey: "heartbeat")
        }
        watchdog = timer; timer.resume()
        connection.stateUpdateHandler = { [weak self] state in
            guard let self, !self.closed else { return }
            switch state {
            case .ready: self.ready = true; self.lastReceive = ProcessInfo.processInfo.systemUptime; self.onReady?(); self.receive()
            case .failed(let error): self.closeOnQueue(error)
            case .cancelled: self.closeOnQueue(RemoteError.connectionLost)
            default: break
            }
        }
        connection.start(queue: queue)
    }

    func send(_ message: RemoteWireMessage, telemetryKey: String? = nil) {
        admissionLock.lock()
        guard !admissionClosed else { admissionLock.unlock(); return }
        let priority = message.usesPriorityWebEncoding
        do {
            if priority { try priorityMailbox.append(message, telemetryKey: telemetryKey) }
            else { try mailbox.append(message, telemetryKey: telemetryKey) }
            guard mailbox.bytes + priorityMailbox.bytes <= RemoteProtocol.maximumQueuedBytes else { throw RemoteError.queueFull }
        }
        catch { admissionClosed = true; admissionLock.unlock(); close(error); return }
        let schedule = priority ? !priorityScheduled : !mailboxScheduled
        if priority { priorityScheduled = true } else { mailboxScheduled = true }; admissionLock.unlock()
        if schedule { queue.async { [weak self] in self?.encodeMailbox() } }
    }

    private func encodeMailbox() {
        guard !closed else { return }
        // Receipts and heartbeat remain responsive throughout a long snapshot.
        for _ in 0..<8 {
            admissionLock.lock(); let item = priorityMailbox.next()
            if item == nil { priorityScheduled = false }; admissionLock.unlock()
            guard let item else { break }
            do {
                if case .welcome = item.message { authenticated = true }
                try outbound.append(RemoteV3Codec.encodeMessage(item.message, devices: document.devices), telemetryKey: item.telemetryKey)
                if case .welcome = item.message, let capabilities { try outbound.append(capabilities) }
                drain()
            }
            catch { closeOnQueue(error); return }
        }
        admissionLock.lock(); let prioritiesRemain = priorityMailbox.count > 0
        if !prioritiesRemain { priorityScheduled = false }; admissionLock.unlock()
        if prioritiesRemain { queue.async { [weak self] in self?.encodeMailbox() } }
        guard snapshotStream == nil, preparation == nil else { return }
        admissionLock.lock(); let item = mailbox.next()
        if item == nil { mailboxScheduled = false }; admissionLock.unlock()
        guard let item else { return }
        let state = document
        preparation = Task.detached(priority: .userInitiated) { [weak self] in
            do {
                let result = try await RemoteSnapshotEncodingPool.shared.encode(item.message, state: state)
                try Task.checkCancellation()
                self?.queue.async { [weak self] in
                    guard let self, !self.closed else { return }
                    self.preparation = nil; self.document = result.state
                    do {
                        switch result.output {
                        case .snapshot(let lease): self.snapshotStream = .init(lease: lease)
                        case .frame(let bytes): try self.outbound.append(bytes, telemetryKey: item.telemetryKey)
                        }
                        self.drain()
                        if self.snapshotStream == nil { self.encodeMailbox() }
                    } catch { self.closeOnQueue(error) }
                }
            } catch { self?.close(error) }
        }
    }

    private func receive() {
        guard !closed else { return }
        // Receive in chunks even when a malicious peer fragments a large message.
        connection.receive(minimumIncompleteLength: 1, maximumLength: 65_536) { [weak self] data, context, complete, error in
            guard let self, !self.closed else { return }
            if let error { self.closeOnQueue(error); return }
            if let metadata = context?.protocolMetadata(definition: NWProtocolWebSocket.definition) as? NWProtocolWebSocket.Metadata {
                if metadata.opcode == .close { self.closeOnQueue(nil); return }
                if metadata.opcode == .ping || metadata.opcode == .pong { self.receive(); return }
                guard metadata.opcode == .text || metadata.opcode == .cont else { self.closeOnQueue(RemoteError.invalidFrame); return }
            }
            if let data {
                guard self.incoming.count <= RemoteProtocol.maximumFrameBytes - data.count else { self.closeOnQueue(RemoteError.oversizedFrame); return }
                self.incoming.append(data)
            }
            guard complete else { self.receive(); return }
            let payload = self.incoming; self.incoming = Data()
            guard !payload.isEmpty else { self.receive(); return }
            do {
                if !self.authenticated, !self.pairingAttempted, payload.count <= 4096,
                   let object = try JSONSerialization.jsonObject(with: payload) as? [String: Any], object["kind"] as? String == "pair" {
                    guard object["version"] as? Int == 3, Set(object.keys) == ["version", "kind", "code", "name"],
                          let code = object["code"] as? String, let name = object["name"] as? String,
                          let handler = self.onPairingRequest else { throw RemoteError.invalidFrame }
                    self.pairingAttempted = true
                    Task { @MainActor [weak self] in
                        do {
                            let credential = try handler(code, name)
                            struct Paired: Encodable { let version = 3; let kind = "paired"; let credential: RemoteBrowserCredential }
                            let bytes = try JSONEncoder().encode(Paired(credential: credential))
                            self?.queue.async { [weak self] in
                                guard let self, !self.closed else { return }
                                do { try self.outbound.append(bytes); self.drain(); self.receive() } catch { self.closeOnQueue(error) }
                            }
                        } catch { self?.close(error) }
                    }
                    return
                }
                let message = try RemoteV3Codec.decodeMessage(payload, devices: self.document.devices)
                self.received += 1; self.lastReceive = ProcessInfo.processInfo.systemUptime
                switch message {
                case .heartbeat(let nonce): self.send(.heartbeatReply(nonce), telemetryKey: "heartbeatReply"); self.receive()
                case .heartbeatReply: self.receive()
                default:
                    guard let onMessage = self.onMessage else { throw RemoteError.invalidFrame }
                    onMessage(message, payload.count) { [weak self] in self?.queue.async { [weak self] in self?.receive() } }
                }
            } catch { self.closeOnQueue(error) }
        }
    }

    private func drain() {
        guard !closed, !sending else { return }
        let data: Data
        if let next = outbound.next() { data = next }
        else if snapshotStream != nil {
            do {
                guard let next = try snapshotStream!.next() else {
                    snapshotStream = nil
                    queue.async { [weak self] in self?.encodeMailbox() }; return
                }
                data = next
            } catch { closeOnQueue(error); return }
        } else { return }
        sending = true; sendStarted = ProcessInfo.processInfo.systemUptime
        let context = NWConnection.ContentContext(identifier: "soundroom-v3", metadata: [NWProtocolWebSocket.Metadata(opcode: .text)])
        connection.send(content: data, contentContext: context, isComplete: true, completion: .contentProcessed { [weak self] error in
            guard let self, !self.closed else { return }
            self.sending = false
            if let error { self.closeOnQueue(error); return }
            self.sent += 1; self.drain()
        })
    }

    func close(_ error: Error? = nil) { queue.async { [weak self] in self?.closeOnQueue(error) } }
    private func closeOnQueue(_ error: Error?) {
        guard !closed else { return }; closed = true
        admissionLock.lock(); admissionClosed = true; mailbox.clear(); priorityMailbox.clear(); admissionLock.unlock()
        preparation?.cancel(); preparation = nil
        watchdog?.cancel(); watchdog = nil; incoming = Data(); outbound.clear(); snapshotStream = nil; document = .init()
        connection.stateUpdateHandler = nil; connection.cancel()
        onClose?(error); onReady = nil; onMessage = nil; onClose = nil; onPairingRequest = nil
    }
    func metrics(_ callback: @escaping @Sendable (RemoteNetworkMetrics) -> Void) {
        queue.async { [weak self] in
            guard let self else { return }
            self.admissionLock.lock()
            let bytes = self.mailbox.bytes + self.priorityMailbox.bytes, count = self.mailbox.count + self.priorityMailbox.count, discarded = self.mailbox.discardedTelemetry + self.priorityMailbox.discardedTelemetry
            self.admissionLock.unlock()
            callback(.init(queuedBytes: bytes + self.outbound.bytes + (self.snapshotStream?.retainedBytes ?? 0), queuedFrames: count + self.outbound.reliableCount,
                           discardedTelemetry: discarded + self.outbound.discardedTelemetry, receivedFrames: self.received, sentFrames: self.sent))
        }
    }
}
