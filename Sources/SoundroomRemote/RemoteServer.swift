import Foundation
import Network

/// The host's authoritative command handler is supplied by its hardware service.
/// Network queues do framing/TLS/JSON; this facade only forwards typed commands.
@MainActor public final class RemoteServer {
    public let credential: RemoteCredential
    public let name: String
    public var onConnect: ((RemotePeer) async -> RemoteSnapshot)?
    public var onRequest: ((RemotePeer, RemoteRequest) async -> RemoteReply)?
    public var onDisconnect: ((RemotePeer) -> Void)?
    public var onError: ((String) -> Void)?
    public var webCapabilities: RemoteCapabilities?
    public private(set) var port: UInt16 = 0
    public var peers: [RemotePeer] { entries.values.compactMap(\.peer).sorted { $0.name < $1.name } }
    public var hasActiveLegacyClients: Bool {
        entries.values.contains { $0.peer != nil && $0.connection is RemoteConnection }
    }
    public var webDocumentsEnabled: Bool { web != nil }
    private var credentials: [UUID: RemoteCredential]
    private var listener: NWListener?
    private var web: RemoteWebListener?
    private let webPairing = RemoteWebPairingStore()
    private var webCredentialDirectory: URL?
    private var webClientRecords: [UUID: RemoteWebClientRecord] = [:]
    public var webClients: [RemoteWebClientRecord] { webClientRecords.values.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending } }
    private let listenerQueue = DispatchQueue(label: "soundroom.remote.listener", qos: .userInitiated)
    private var admission: RemoteAdmissionGate?
    private var entries: [UUID: Entry] = [:]
    private final class Entry {
        let connection: any RemoteServerChannel
        let permit: RemoteAdmissionGate.Permit
        let challenge: Data
        var peer: RemotePeer?
        var credentialID: UUID?
        var requests: [(RemoteRequest, Int)] = []
        var queuedBytes = 0
        var interruptCount = 0
        var interruptBytes = 0
        var processing = false
        var initialEvents = RemoteInitialEventBuffer()
        init(_ connection: any RemoteServerChannel, permit: RemoteAdmissionGate.Permit, challenge: Data) {
            self.connection = connection; self.permit = permit; self.challenge = challenge
        }
    }
    public init(credential: RemoteCredential, name: String = Host.current().localizedName ?? "Soundroom") {
        self.credential = credential; self.name = name
        self.credentials = [credential.clientID: credential]
    }
    /// Scope is taken from this host registry, never from a client's claimed role.
    public func authorize(_ credential: RemoteCredential) throws {
        try credential.validate()
        guard credential.serverID == self.credential.serverID, credential.secret == self.credential.secret,
              credentials.count < 64 || credentials[credential.clientID] != nil else { throw RemoteError.invalidCredential }
        credentials[credential.clientID] = credential
    }
    public func revoke(credentialID: UUID) {
        credentials.removeValue(forKey: credentialID)
        for entry in entries.values where entry.credentialID == credentialID { entry.connection.close(RemoteError.invalidCredential) }
    }
    public func start(port: UInt16 = RemoteProtocol.defaultPort) throws {
        guard listener == nil else { return }
        try credential.validate()
        let value = try NWListener(using: remoteTLSParameters(credential), on: port == 0 ? .any : NWEndpoint.Port(rawValue: port)!)
        self.listener = value; self.port = port
        let gate = RemoteAdmissionGate(capacity: RemoteProtocol.maximumClients)
        admission = gate
        value.service = NWListener.Service(name: "Soundroom-\(credential.serverID.uuidString.prefix(8))", type: "_soundroom._tcp")
        value.newConnectionHandler = { [weak self] connection in
            // Reserve on the listener queue before creating actor work. Even a
            // stalled UI actor retains at most maximumClients incoming sockets.
            guard let permit = gate.acquire() else { connection.cancel(); return }
            Task { @MainActor [weak self] in
                guard let self else { connection.cancel(); return }
                self.accept(connection, permit: permit, gate: gate)
            }
        }
        value.stateUpdateHandler = { [weak self] state in
            Task { @MainActor [weak self] in
                guard let self else { return }
                switch state {
                case .ready: self.port = value.port?.rawValue ?? port
                case .failed(let error): self.onError?(error.localizedDescription)
                default: break
                }
            }
        }
        value.start(queue: listenerQueue)
    }
    public func stop() {
        web?.stop(); web = nil
        webPairing.clear()
        admission?.close(); admission = nil
        listener?.stateUpdateHandler = nil; listener?.newConnectionHandler = nil
        listener?.cancel(); listener = nil
        for entry in entries.values { entry.connection.close() }
    }
    public func publish(_ event: RemoteEvent) {
        let key = telemetryKey(for: event)
        for entry in entries.values where entry.peer != nil { send(event, telemetryKey: key, entry: entry) }
    }
    public func send(_ event: RemoteEvent, to clientID: UUID) {
        for entry in entries.values where entry.peer?.id == clientID { send(event, telemetryKey: telemetryKey(for: event), entry: entry) }
    }
    private func send(_ event: RemoteEvent, telemetryKey: String?, entry: Entry) {
        do {
            try entry.initialEvents.submit(event, telemetryKey: telemetryKey) {
                entry.connection.send($0, telemetryKey: $1)
            }
        } catch { entry.connection.close(error) }
    }
    public func networkMetrics() async -> [UUID: RemoteNetworkMetrics] {
        var result: [UUID: RemoteNetworkMetrics] = [:]
        for entry in entries.values {
            let id = entry.peer?.id ?? entry.connection.id
            result[id] = await withCheckedContinuation { continuation in entry.connection.metrics { continuation.resume(returning: $0) } }
        }
        return result
    }
    private func accept(_ connection: NWConnection, permit: RemoteAdmissionGate.Permit, gate: RemoteAdmissionGate) {
        let framed = RemoteConnection(connection: connection,
                                      queue: DispatchQueue(label: "soundroom.remote.peer.\(UUID().uuidString)", qos: .userInitiated))
        accept(framed, permit: permit, gate: gate)
    }
    /// Browser and native clients enter the same identity/role/request scheduler.
    /// The shared gate includes peers still completing TLS/authentication.
    public func startWeb(configuration: RemoteWebConfiguration, credentialDirectory: URL? = nil) throws {
        guard web == nil, let gate = admission else { throw RemoteError.rejected("Start the hardware service before its web endpoint.") }
        if let credentialDirectory {
            try FileManager.default.createDirectory(at: credentialDirectory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            let values = try credentialDirectory.resourceValues(forKeys: [.isSymbolicLinkKey])
            guard values.isSymbolicLink != true else { throw RemoteError.invalidCredential }
            let files = try FileManager.default.contentsOfDirectory(at: credentialDirectory, includingPropertiesForKeys: nil).filter { $0.pathExtension == "json" }
            guard files.count <= 63 else { throw RemoteError.queueFull }
            for file in files {
                let stored = try RemoteWebCredentialFile.load(from: file)
                let paired = stored.credential
                guard paired.clientID != credential.clientID, file.lastPathComponent == paired.clientID.uuidString + ".json" else { throw RemoteError.invalidCredential }
                try authorize(paired); webClientRecords[paired.clientID] = stored.record
            }
        }
        webCredentialDirectory = credentialDirectory
        let capabilityData = try webCapabilities.map(RemoteV3Codec.encodeCapabilities)
        let endpoint = try RemoteWebListener(configuration: configuration)
        endpoint.onConnection = { [weak self] connection in
            guard let permit = gate.acquire() else { connection.cancel(); return }
            let channel = RemoteWebConnection(connection: connection, capabilities: capabilityData)
            Task { @MainActor [weak self] in
                guard let self else { channel.close(); return }
                channel.onPairingRequest = { [weak self] code, name in
                    guard let self else { throw RemoteError.connectionLost }
                    return try self.redeemWebPairing(code: code, name: name)
                }
                self.accept(channel, permit: permit, gate: gate)
            }
        }
        endpoint.onError = { [weak self] error in Task { @MainActor [weak self] in self?.onError?(error) } }
        try endpoint.start(); web = endpoint
    }
    public func issueWebPairing(name: String, role: RemoteRole, expiresInSeconds: Int = 120) throws -> RemoteWebPairingGrant {
        guard web != nil else { throw RemoteError.rejected("Web access is not enabled on this server.") }
        return try webPairing.issue(name: name, role: role, expiresInSeconds: expiresInSeconds)
    }
    public func revokeWebCredential(id: UUID) throws {
        guard webClientRecords[id] != nil else { throw RemoteError.rejected("That browser credential is not registered.") }
        if let directory = webCredentialDirectory { try FileManager.default.removeItem(at: directory.appendingPathComponent(id.uuidString + ".json")) }
        webClientRecords[id] = nil; revoke(credentialID: id)
    }
    private func redeemWebPairing(code: String, name: String) throws -> RemoteBrowserCredential {
        let role = try webPairing.redeem(code: code, name: name)
        let paired = try credential.paired(role: role)
        guard credentials.count < 64 else { throw RemoteError.queueFull }
        let record = RemoteWebClientRecord(id: paired.clientID, name: name, role: role)
        if let directory = webCredentialDirectory { try RemoteWebCredentialFile(credential: paired, record: record).write(to: directory.appendingPathComponent(paired.clientID.uuidString + ".json")) }
        try authorize(paired); webClientRecords[paired.clientID] = record
        return .init(paired)
    }
    private func accept(_ framed: any RemoteServerChannel, permit: RemoteAdmissionGate.Permit, gate: RemoteAdmissionGate) {
        guard listener != nil, admission === gate, entries.count < RemoteProtocol.maximumClients,
              let challenge = try? RemoteCredential.authenticationNonce() else { framed.close(); return }
        let id = framed.id; entries[id] = Entry(framed, permit: permit, challenge: challenge)
        let serverID = credential.serverID
        framed.onReady = { [weak framed] in framed?.send(.challenge(.init(serverID: serverID, nonce: challenge))) }
        framed.onMessage = { [weak self] message, bytes, consumed in
            Task { @MainActor [weak self] in self?.receive(message, bytes: bytes, connectionID: id); consumed() }
        }
        framed.onClose = { [weak self] _ in Task { @MainActor [weak self] in self?.closed(id) } }
        framed.start()
        Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: 10_000_000_000)
            guard let self, let entry = self.entries[id], entry.peer == nil else { return }
            entry.connection.close(RemoteError.timeout)
        }
    }
    private func receive(_ message: RemoteWireMessage, bytes: Int, connectionID: UUID) {
        guard let entry = entries[connectionID] else { return }
        if entry.peer == nil {
            guard case .hello(let hello) = message, hello.version == RemoteProtocol.version,
                  hello.serverID == credential.serverID, !hello.peer.name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                  hello.peer.name.utf8.count <= 128, !hello.peer.name.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }),
                  let paired = credentials[hello.credentialID], paired.authenticates(peer: hello.peer, proof: hello.proof, challenge: entry.challenge),
                  !entries.values.contains(where: { $0.peer?.id == hello.peer.id }) else {
                entry.connection.close(RemoteError.invalidCredential); return
            }
            var peer = hello.peer; peer.role = paired.role; entry.peer = peer; entry.credentialID = paired.clientID
            entry.connection.send(.welcome(.init(serverID: credential.serverID, name: name, role: peer.role)))
            publish(.presence(peers))
            if let onConnect {
                Task { @MainActor [weak self, weak entry] in
                    let snapshot = await onConnect(peer)
                    guard let self, let entry, self.entries[connectionID] === entry else { return }
                    entry.initialEvents.install(snapshot) {
                        entry.connection.send($0, telemetryKey: $1)
                    }
                }
            }
            return
        }
        guard case .request(let request) = message, let peer = entry.peer, request.clientID == peer.id else {
            entry.connection.close(RemoteError.invalidFrame); return
        }
        guard entry.queuedBytes + entry.interruptBytes + bytes <= RemoteProtocol.maximumQueuedBytes else {
            entry.connection.close(RemoteError.queueFull); return
        }
        if entry.processing, request.command.isInterrupt || (request.command.isTransportPriority && entry.requests.allSatisfy({ $0.0.command.isReadOnlyControl })) {
            guard entry.interruptCount < 8 else { entry.connection.close(RemoteError.queueFull); return }
            entry.interruptCount += 1; entry.interruptBytes += bytes
            Task { @MainActor [weak self, weak entry] in
                guard let self, let entry else { return }
                let reply: RemoteReply
                if let onRequest = self.onRequest { reply = await onRequest(peer, request) }
                else { reply = .init(requestID: request.id, status: .rejected, revision: 0, message: "The hardware service is unavailable.") }
                entry.interruptCount -= 1; entry.interruptBytes -= bytes
                guard self.entries[connectionID] === entry else { return }
                entry.connection.send(.reply(reply))
            }
            return
        }
        guard entry.requests.count < RemoteProtocol.maximumPendingRequests else { entry.connection.close(RemoteError.queueFull); return }
        entry.requests.append((request, bytes)); entry.queuedBytes += bytes; process(entry, connectionID: connectionID)
    }
    private func process(_ entry: Entry, connectionID: UUID) {
        guard !entry.processing, !entry.requests.isEmpty, let peer = entry.peer else { return }
        entry.processing = true
        let (request, bytes) = entry.requests.removeFirst(); entry.queuedBytes -= bytes
        Task { @MainActor [weak self, weak entry] in
            guard let self, let entry else { return }
            let reply: RemoteReply
            if let onRequest { reply = await onRequest(peer, request) }
            else { reply = .init(requestID: request.id, status: .rejected, revision: 0, message: "The hardware service is unavailable.") }
            guard self.entries[connectionID] === entry else { return }
            entry.connection.send(.reply(reply)); entry.processing = false
            self.process(entry, connectionID: connectionID)
        }
    }
    private func closed(_ id: UUID) {
        guard let entry = entries.removeValue(forKey: id) else { return }
        if let peer = entry.peer { onDisconnect?(peer); publish(.presence(peers)) }
    }
}

extension RemoteCommand {
    var isReadOnlyControl: Bool {
        switch self { case .snapshot, .ping, .waveform, .listProjects, .downloadArtifact, .readArtifact: return true; default: return false }
    }
    var isTransportPriority: Bool {
        switch self {
        case .play, .seek, .record, .takeTransport, .setLoop: return true
        case .editor(let intent): return ["transport.toggle", "transport.play", "transport.record", "transport.audition"].contains(intent.operation)
        default: return false
        }
    }
    var isInterrupt: Bool {
        switch self {
        case .stop: return true
        case .editor(let intent):
            return ["transport.stop", "transport.pause", "transport.finishRecording", "jobs.cancel", "designer.noteOff", "designer.allNotesOff",
                    "designer.controller", "designer.configure", "designer.noteOn", "designer.stop", "drums.stop", "take.stop", "audition.stop"].contains(intent.operation)
        default: return false
        }
    }
}

/// Reservation spans dispatch-to-actor, TLS/authentication and accepted lifetime.
/// Each listener owns its gate so queued callbacks from a stopped listener cannot
/// consume slots in its replacement. This lock is never touched by audio code.
final class RemoteAdmissionGate: @unchecked Sendable {
    final class Permit: @unchecked Sendable {
        private let gate: RemoteAdmissionGate
        fileprivate init(_ gate: RemoteAdmissionGate) { self.gate = gate }
        deinit { gate.release() }
    }
    private let lock = NSLock()
    private let capacity: Int
    private var occupied = 0
    private var open = true
    init(capacity: Int) { self.capacity = max(0, capacity) }
    func acquire() -> Permit? {
        lock.lock(); defer { lock.unlock() }
        guard open, occupied < capacity else { return nil }
        occupied += 1; return Permit(self)
    }
    func close() { lock.lock(); open = false; lock.unlock() }
    private func release() { lock.lock(); occupied -= 1; lock.unlock() }
}
