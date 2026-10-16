import Foundation
import Network
import Security

func remoteTLSParameters(_ credential: RemoteCredential) -> NWParameters {
    let tls = NWProtocolTLS.Options()
    // TLS PSK authenticates both endpoints without an ad-hoc certificate trust
    // override. This suite is authenticated AES-GCM with a 256-bit random key.
    sec_protocol_options_set_min_tls_protocol_version(tls.securityProtocolOptions, .TLSv12)
    sec_protocol_options_set_max_tls_protocol_version(tls.securityProtocolOptions, .TLSv12)
    sec_protocol_options_append_tls_ciphersuite(tls.securityProtocolOptions, tls_ciphersuite_t(rawValue: 0x00A8)!)
    let key = credential.secret.withUnsafeBytes { DispatchData(bytes: $0) }
    let identity = Data("soundroom-v1-\(credential.serverID.uuidString)".utf8).withUnsafeBytes { DispatchData(bytes: $0) }
    sec_protocol_options_add_pre_shared_key(tls.securityProtocolOptions, key as __DispatchData, identity as __DispatchData)
    let tcp = NWProtocolTCP.Options(); tcp.noDelay = true
    let parameters = NWParameters(tls: tls, tcp: tcp)
    parameters.includePeerToPeer = false
    parameters.allowLocalEndpointReuse = true
    return parameters
}

/// Every method schedules work on a private serial network queue. It never
/// invokes hardware, holds an audio lock, or waits for the main thread.
final class RemoteConnection: RemoteServerChannel, @unchecked Sendable {
    let id = UUID()
    private let connection: NWConnection
    private let queue: DispatchQueue
    private var decoder = RemoteFrameDecoder()
    private var outbound = RemoteOutboundQueue()
    private let admissionLock = NSLock()
    private var mailbox = RemoteMessageMailbox()
    private var mailboxScheduled = false
    private var admissionClosed = false
    private var sending = false
    private var closed = false
    private var ready = false
    private var lastReceiveTime = ProcessInfo.processInfo.systemUptime
    private var receiveCount = 0
    private var sendCount = 0
    private var sendStarted: TimeInterval = 0
    private var watchdog: DispatchSourceTimer?
    private let jsonDecoder = JSONDecoder()
    var onReady: (@Sendable () -> Void)?
    var onMessage: (@Sendable (RemoteWireMessage, Int, @escaping @Sendable () -> Void) -> Void)?
    var onClose: (@Sendable (Error?) -> Void)?
    init(connection: NWConnection, queue: DispatchQueue) { self.connection = connection; self.queue = queue }
    deinit { watchdog?.cancel(); connection.cancel() }
    func start() {
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now() + 5, repeating: 5)
        timer.setEventHandler { [weak self] in
            guard let self, !self.closed, self.ready else { return }
            let now = ProcessInfo.processInfo.systemUptime
            if now - self.lastReceiveTime > 15 || (self.sending && now - self.sendStarted > 30) {
                self.closeOnQueue(RemoteError.timeout); return
            }
            self.send(.heartbeat(UUID()), telemetryKey: "heartbeat")
        }
        watchdog = timer; timer.resume()
        connection.stateUpdateHandler = { [weak self] state in
            guard let self, !self.closed else { return }
            switch state {
            case .ready: self.ready = true; self.lastReceiveTime = ProcessInfo.processInfo.systemUptime; self.onReady?(); self.receive()
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
        do { try mailbox.append(message, telemetryKey: telemetryKey) }
        catch {
            admissionClosed = true; admissionLock.unlock(); close(error); return
        }
        let schedule = !mailboxScheduled
        mailboxScheduled = true; admissionLock.unlock()
        if schedule { queue.async { [weak self] in self?.encodeMailbox() } }
    }
    private func encodeMailbox() {
        guard !closed else { return }
        for _ in 0..<8 {
            admissionLock.lock()
            let item = mailbox.next()
            if item == nil { mailboxScheduled = false }
            admissionLock.unlock()
            guard let item else { return }
            do {
                let frame = try RemoteFrameDecoder.encode(RemoteLegacyMessageCodec.encode(item.message))
                try outbound.append(frame, telemetryKey: item.telemetryKey); drain()
            } catch { closeOnQueue(error); return }
        }
        // Give socket completions a turn between encoding batches.
        queue.async { [weak self] in self?.encodeMailbox() }
    }
    func close(_ error: Error? = nil) { queue.async { [weak self] in self?.closeOnQueue(error) } }
    func metrics(_ callback: @escaping @Sendable (RemoteNetworkMetrics) -> Void) {
        queue.async { [weak self] in
            guard let self else { return }
            self.admissionLock.lock()
            let admittedBytes = self.mailbox.bytes, admittedCount = self.mailbox.count, discarded = self.mailbox.discardedTelemetry
            self.admissionLock.unlock()
            callback(.init(queuedBytes: self.outbound.bytes + admittedBytes, queuedFrames: self.outbound.reliableCount + admittedCount,
                           discardedTelemetry: self.outbound.discardedTelemetry + discarded, receivedFrames: self.receiveCount, sentFrames: self.sendCount))
        }
    }
    private func receive() {
        guard !closed else { return }
        connection.receive(minimumIncompleteLength: 1, maximumLength: 65_536) { [weak self] data, _, complete, error in
            guard let self, !self.closed else { return }
            if let data, !data.isEmpty {
                do {
                    let frames = try self.decoder.append(data)
                    self.deliver(frames, index: 0) { [weak self] in
                        guard let self, !self.closed else { return }
                        if let error { self.closeOnQueue(error) }
                        else if complete { self.closeOnQueue(RemoteError.connectionLost) }
                        else { self.receive() }
                    }
                    return
                } catch { self.closeOnQueue(error); return }
            }
            if let error { self.closeOnQueue(error); return }
            if complete { self.closeOnQueue(RemoteError.connectionLost); return }
            self.receive()
        }
    }
    private func deliver(_ frames: [Data], index: Int, completion: @escaping @Sendable () -> Void) {
        guard !closed else { return }
        guard index < frames.count else { completion(); return }
        do {
            let message = try jsonDecoder.decode(RemoteWireMessage.self, from: frames[index])
            receiveCount += 1; lastReceiveTime = ProcessInfo.processInfo.systemUptime
            // Liveness is transport work: a long render/export cannot block it.
            switch message {
            case .heartbeat(let nonce):
                send(.heartbeatReply(nonce), telemetryKey: "heartbeatReply")
                deliver(frames, index: index + 1, completion: completion); return
            case .heartbeatReply:
                deliver(frames, index: index + 1, completion: completion); return
            default: break
            }
            guard let onMessage else { closeOnQueue(RemoteError.invalidFrame); return }
            onMessage(message, frames[index].count) { [weak self] in
                guard let self else { return }
                self.queue.async { [weak self] in self?.deliver(frames, index: index + 1, completion: completion) }
            }
        } catch { closeOnQueue(error) }
    }
    private func drain() {
        guard !sending, !closed, let frame = outbound.next() else { return }
        sending = true; sendStarted = ProcessInfo.processInfo.systemUptime
        connection.send(content: frame, completion: .contentProcessed { [weak self] error in
            guard let self, !self.closed else { return }
            self.sending = false
            if let error { self.closeOnQueue(error); return }
            self.sendCount += 1; self.drain()
        })
    }
    private func closeOnQueue(_ error: Error?) {
        guard !closed else { return }; closed = true
        admissionLock.lock(); admissionClosed = true; mailbox.clear(); admissionLock.unlock()
        watchdog?.cancel(); watchdog = nil
        connection.stateUpdateHandler = nil; connection.cancel(); outbound.clear()
        onClose?(error); onReady = nil; onMessage = nil; onClose = nil
    }
}

func telemetryKey(for event: RemoteEvent) -> String? {
    switch event { case .transport: return "transport"; case .meters: return "meters"; case .effectMeters: return "effectMeters"; case .audition(let kind, _, _, _, _): return "audition-\(kind)"; case .job(let id, _, _): return "job-\(id)"; default: return nil }
}
