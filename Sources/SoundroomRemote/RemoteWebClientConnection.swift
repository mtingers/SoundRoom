import Foundation
import Network
import Security

/// Native v3 client transport. Uses the same WSS protocol as portable clients,
/// with normal system certificate/hostname verification and bounded worker queues.
final class RemoteWebClientConnection: RemoteServerChannel, @unchecked Sendable {
    let id = UUID()
    private let socket: NWConnection
    private let queue = DispatchQueue(label: "soundroom.web.client.\(UUID().uuidString)", qos: .userInitiated)
    private let lock = NSLock()
    private var mailbox = RemoteMessageMailbox()
    private var urgentMailbox = RemoteMessageMailbox()
    private var scheduled = false
    private var admissionClosed = false
    private var output = RemoteOutboundQueue()
    private var urgentOutput = RemoteOutboundQueue()
    private final class DecoderState: @unchecked Sendable {
        // Access exclusively on decoderQueue, never on the socket reader queue.
        var stream = RemoteV3ClientStream()
    }
    private let decoder = DecoderState()
    private let decoderQueue = DispatchQueue(label: "soundroom.web.client.decode.\(UUID().uuidString)", qos: .userInitiated)
    private var decodeInbox = RemoteV3DecodeInbox()
    private var decoding = false
    private var decodingBytes = 0
    private var retainedDocumentBytes = 0
    private var devices: [RemoteDevice] = []
    private var controlDeliveries = RemoteV3ControlDeliveryBudget()
    private var incoming = Data()
    private var closed = false
    private var sending = false
    private var ready = false
    private var sendStarted: TimeInterval = 0
    private var lastReceive = ProcessInfo.processInfo.systemUptime
    private var received = 0
    private var sent = 0
    private var watchdog: DispatchSourceTimer?
    var onReady: (@Sendable () -> Void)?
    var onMessage: (@Sendable (RemoteWireMessage, Int, @escaping @Sendable () -> Void) -> Void)?
    var onClose: (@Sendable (Error?) -> Void)?
    var onCapabilities: (@Sendable (RemoteCapabilities) -> Void)?

    init(url: URL, origin: URL, trustedCA: SecCertificate? = nil) throws {
        try Self.validate(url: url, origin: origin)
        let tls = NWProtocolTLS.Options()
        sec_protocol_options_set_min_tls_protocol_version(tls.securityProtocolOptions, .TLSv12)
        // Explicit anchors are for opt-in local CA installations/tests. Verification
        // retains the endpoint hostname and never accepts an untrusted chain.
        if let trustedCA {
            let host = url.host!
            sec_protocol_options_set_verify_block(tls.securityProtocolOptions, { _, trust, complete in
                let value = sec_trust_copy_ref(trust).takeRetainedValue()
                SecTrustSetPolicies(value, SecPolicyCreateSSL(true, host as CFString))
                SecTrustSetAnchorCertificates(value, [trustedCA] as CFArray)
                SecTrustSetAnchorCertificatesOnly(value, true)
                complete(SecTrustEvaluateWithError(value, nil))
            }, DispatchQueue(label: "soundroom.web.client.trust"))
        }
        let tcp = NWProtocolTCP.Options(); tcp.noDelay = true
        let parameters = NWParameters(tls: tls, tcp: tcp)
        let websocket = NWProtocolWebSocket.Options(); websocket.autoReplyPing = true
        websocket.maximumMessageSize = RemoteProtocol.maximumFrameBytes
        websocket.setSubprotocols(["soundroom.v3"])
        websocket.setAdditionalHeaders([(name: "Origin", value: RemoteWebPolicy.canonicalOrigin(origin.absoluteString)!)])
        parameters.defaultProtocolStack.applicationProtocols.insert(websocket, at: 0)
        socket = NWConnection(to: .url(url), using: parameters)
    }
    static func validate(url: URL, origin: URL) throws {
        guard url.scheme == "wss", let host = url.host, !host.isEmpty, host.utf8.count <= 253,
              url.user == nil, url.password == nil, url.query == nil, url.fragment == nil,
              url.port == nil || (1...65535).contains(url.port!),
              RemoteWebPolicy.canonicalOrigin(origin.absoluteString) != nil, origin.user == nil, origin.password == nil,
              origin.query == nil, origin.fragment == nil, origin.path.isEmpty || origin.path == "/",
              origin.port == nil || (1...65535).contains(origin.port!) else { throw RemoteError.invalidCredential }
    }
    deinit { watchdog?.cancel(); socket.cancel() }
    func start() {
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now() + 5, repeating: 5)
        timer.setEventHandler { [weak self] in
            guard let self, self.ready, !self.closed else { return }
            let now = ProcessInfo.processInfo.systemUptime
            if now - self.lastReceive > 15 || (self.sending && now - self.sendStarted > 30) { self.closeOnQueue(RemoteError.timeout); return }
            self.send(.heartbeat(UUID()), telemetryKey: "heartbeat")
        }
        watchdog = timer; timer.resume()
        socket.stateUpdateHandler = { [weak self] state in
            guard let self, !self.closed else { return }
            switch state {
            case .ready:
                guard let metadata = self.socket.metadata(definition: NWProtocolWebSocket.definition) as? NWProtocolWebSocket.Metadata,
                      metadata.selectedSubprotocol == "soundroom.v3" else { self.closeOnQueue(RemoteError.protocolMismatch); return }
                self.ready = true; self.lastReceive = ProcessInfo.processInfo.systemUptime; self.onReady?(); self.receive()
            case .failed(let error): self.closeOnQueue(error)
            case .cancelled: self.closeOnQueue(RemoteError.connectionLost)
            default: break
            }
        }
        socket.start(queue: queue)
    }
    func send(_ message: RemoteWireMessage, telemetryKey: String? = nil) {
        lock.lock()
        guard !admissionClosed else { lock.unlock(); return }
        let urgent: Bool
        switch message { case .request(let request): urgent = request.command.isInterrupt; case .heartbeat, .heartbeatReply: urgent = true; default: urgent = false }
        do {
            if urgent { try urgentMailbox.append(message, telemetryKey: telemetryKey) }
            else { try mailbox.append(message, telemetryKey: telemetryKey) }
            guard mailbox.bytes + urgentMailbox.bytes <= RemoteProtocol.maximumQueuedBytes - (urgent ? 0 : 65536) else { throw RemoteError.queueFull }
        } catch { admissionClosed = true; lock.unlock(); close(error); return }
        let schedule = !scheduled; scheduled = true; lock.unlock()
        if schedule { queue.async { [weak self] in self?.encode() } }
    }
    private func encode() {
        guard !closed else { return }
        for _ in 0..<8 {
            lock.lock()
            let urgent = urgentMailbox.count > 0
            let item = urgent ? urgentMailbox.next() : mailbox.next()
            if item == nil { scheduled = false }; lock.unlock()
            guard let item else { drain(); return }
            do {
                let bytes = try RemoteV3Codec.encodeMessage(item.message, devices: devices)
                if urgent { try urgentOutput.append(bytes, telemetryKey: item.telemetryKey) }
                else { try output.append(bytes, telemetryKey: item.telemetryKey) }
                drain()
            } catch { closeOnQueue(error); return }
        }
        queue.async { [weak self] in self?.encode() }
    }
    private func drain() {
        guard ready, !closed, !sending, let bytes = urgentOutput.next() ?? output.next() else { return }
        sending = true; sendStarted = ProcessInfo.processInfo.systemUptime
        let context = NWConnection.ContentContext(identifier: "soundroom-v3-client", metadata: [NWProtocolWebSocket.Metadata(opcode: .text)])
        socket.send(content: bytes, contentContext: context, isComplete: true, completion: .contentProcessed { [weak self] error in
            guard let self, !self.closed else { return }; self.sending = false
            if let error { self.closeOnQueue(error); return }; self.sent += 1; self.drain()
        })
    }
    private func receive() {
        guard !closed else { return }
        socket.receive(minimumIncompleteLength: 1, maximumLength: 65536) { [weak self] data, context, complete, error in
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
            do {
                self.received += 1; self.lastReceive = ProcessInfo.processInfo.systemUptime
                if let control = try RemoteV3DecodeInbox.immediateControl(payload) {
                    switch control {
                    case .heartbeat(let nonce): self.send(.heartbeatReply(nonce), telemetryKey: "heartbeatReply")
                    case .heartbeatReply: break
                    case .reply:
                        guard let onMessage = self.onMessage else { throw RemoteError.invalidFrame }
                        let ticket = try self.controlDeliveries.reserve(payload.count)
                        onMessage(control, payload.count) { [weak self] in
                            self?.queue.async { [weak self] in self?.controlDeliveries.release(ticket) }
                        }
                    default: throw RemoteError.invalidFrame
                    }
                } else {
                    try self.decodeInbox.append(payload)
                    self.startDecoding()
                }
                // The socket keeps reading while a dense document is decoded or
                // its installation waits for the controller actor. Heartbeats
                // and small receipts remain live without concurrent access to
                // the stream assembler or premature metadata installation.
                self.receive()
            } catch { self.closeOnQueue(error) }
        }
    }
    private func startDecoding() {
        guard !closed, !decoding, let payload = decodeInbox.next() else { return }
        decoding = true; decodingBytes = payload.count
        let decoder = decoder
        decoderQueue.async { [weak self] in
            guard let self else { return }
            do {
                let previousCapabilities = decoder.stream.capabilities
                let messages = try decoder.stream.accept(payload)
                let changedCapabilities = previousCapabilities == decoder.stream.capabilities ? nil : decoder.stream.capabilities
                let devices = decoder.stream.devices
                let retained = decoder.stream.retainedDocumentBytes
                self.queue.async { [weak self] in
                    guard let self, !self.closed else { return }
                    self.devices = devices; self.retainedDocumentBytes = retained
                    if let changedCapabilities { self.onCapabilities?(changedCapabilities) }
                    self.deliver(messages, at: 0, bytes: payload.count)
                }
            } catch { self.close(error) }
        }
    }
    private func deliver(_ messages: [RemoteWireMessage], at index: Int, bytes: Int) {
        guard !closed else { return }
        guard index < messages.count else {
            decoding = false; decodingBytes = 0; startDecoding(); return
        }
        switch messages[index] {
        case .heartbeat(let nonce): send(.heartbeatReply(nonce), telemetryKey: "heartbeatReply"); deliver(messages, at: index + 1, bytes: bytes)
        case .heartbeatReply: deliver(messages, at: index + 1, bytes: bytes)
        default:
            guard let onMessage else { closeOnQueue(RemoteError.invalidFrame); return }
            onMessage(messages[index], bytes) { [weak self] in
                self?.queue.async { [weak self] in self?.deliver(messages, at: index + 1, bytes: bytes) }
            }
        }
    }
    func close(_ error: Error? = nil) { queue.async { [weak self] in self?.closeOnQueue(error) } }
    private func closeOnQueue(_ error: Error?) {
        guard !closed else { return }; closed = true
        lock.lock(); admissionClosed = true; mailbox.clear(); urgentMailbox.clear(); lock.unlock()
        watchdog?.cancel(); watchdog = nil; output.clear(); urgentOutput.clear(); incoming = Data()
        decodeInbox.clear(); decodingBytes = 0; retainedDocumentBytes = 0; devices = []
        controlDeliveries.clear()
        let decoder = decoder
        decoderQueue.async { decoder.stream = .init() }
        socket.stateUpdateHandler = nil; socket.cancel()
        onClose?(error); onClose = nil; onReady = nil; onMessage = nil; onCapabilities = nil
    }
    func metrics(_ callback: @escaping @Sendable (RemoteNetworkMetrics) -> Void) {
        queue.async { [weak self] in
            guard let self else { return }
            self.lock.lock(); let bytes = self.mailbox.bytes + self.urgentMailbox.bytes, count = self.mailbox.count + self.urgentMailbox.count
            let discarded = self.mailbox.discardedTelemetry + self.urgentMailbox.discardedTelemetry; self.lock.unlock()
            callback(.init(queuedBytes: bytes + self.output.bytes + self.urgentOutput.bytes + self.decodeInbox.bytes + self.decodingBytes + self.retainedDocumentBytes + self.controlDeliveries.bytes,
                           queuedFrames: count + self.output.reliableCount + self.urgentOutput.reliableCount + self.decodeInbox.count + (self.decoding ? 1 : 0) + self.controlDeliveries.count,
                           discardedTelemetry: discarded + self.output.discardedTelemetry + self.urgentOutput.discardedTelemetry,
                           receivedFrames: self.received, sentFrames: self.sent))
        }
    }
}
