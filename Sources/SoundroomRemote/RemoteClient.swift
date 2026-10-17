import Foundation
import Network
import Security
import SoundroomCore

@MainActor public final class RemoteClient {
    public let credential: RemoteClientCredential
    public let clientID: UUID
    public let name: String
    public var onEvent: ((RemoteEvent) -> Void)?
    public var onState: ((RemoteConnectionState) -> Void)?
    public var onCapabilities: ((RemoteCapabilities) -> Void)?
    public var autoReconnect = true
    public private(set) var state: RemoteConnectionState = .disconnected { didSet { onState?(state) } }
    public private(set) var snapshot: RemoteSnapshot?
    public private(set) var capabilities: RemoteCapabilities?
    private var connection: (any RemoteServerChannel)?
    private var endpoint: (host: String, port: UInt16)?
    private var webEndpoint: (url: URL, origin: URL, trustedCA: SecCertificate?)?
    private var wantsConnection = false
    private var connecting: CheckedContinuation<Void, Error>?
    private var connectingTimeout: Task<Void, Never>?
    private var reconnectTask: Task<Void, Never>?
    private var reconnectAttempt = 0
    private var connectionTimeout: Double = 10
    private var helloSent = false
    private var featureDeclarationConnection: UUID?
    private struct Pending {
        let completion: RemoteUploadBatch.Completion
        let timeout: Task<Void, Never>
    }
    private var pending = RemotePendingRequests<Pending>()
    public convenience init(credential: RemoteCredential, name: String = "Soundroom client", clientID: UUID = UUID()) {
        self.init(credential: .native(credential), name: name, clientID: clientID)
    }
    public init(credential: RemoteClientCredential, name: String, clientID: UUID = UUID()) {
        self.credential = credential
        self.name = name
        self.clientID = clientID
    }
    public func connect(host: String, port: UInt16 = RemoteProtocol.defaultPort, timeout: Double = 10) async throws {
        try credential.validate()
        _ = try credential.requireNativeCredential()
        try RemoteClientCredential.validateName(name)
        guard !host.isEmpty, host.utf8.count <= 253, port > 0 else { throw RemoteError.invalidCredential }
        disconnect()
        connectionTimeout = timeout.isFinite ? max(0.1, min(60, timeout)) : 10
        endpoint = (host, port)
        webEndpoint = nil
        capabilities = nil
        wantsConnection = true
        state = .connecting
        try await withCheckedThrowingContinuation { continuation in
            connecting = continuation
            openConnection()
        }
    }
    /// Opt-in common v3 endpoint for native controllers. Existing native pairing
    /// credentials authenticate here too; TLS uses system trust rather than PSK.
    public func connect(websocketURL: URL, origin: URL, timeout: Double = 10) async throws {
        try await connectWeb(websocketURL: websocketURL, origin: origin, trustedCA: nil, timeout: timeout)
    }
    /// Local host mode: anchors only the host's private loopback CA; hostname
    /// validation is retained and system trust is not modified.
    public func connect(websocketURL: URL, origin: URL, pinnedCA: SecCertificate, timeout: Double = 10) async throws {
        try await connectWeb(websocketURL: websocketURL, origin: origin, trustedCA: pinnedCA, timeout: timeout)
    }
    public nonisolated static func validateWebEndpoint(websocketURL: URL, origin: URL) throws {
        try RemoteWebClientConnection.validate(url: websocketURL, origin: origin)
    }
    func connectWeb(websocketURL: URL, origin: URL, trustedCA: SecCertificate?, timeout: Double = 10) async throws {
        try credential.validate()
        try RemoteClientCredential.validateName(name)
        try RemoteWebClientConnection.validate(url: websocketURL, origin: origin)
        disconnect()
        capabilities = nil
        endpoint = nil
        webEndpoint = (websocketURL, origin, trustedCA)
        connectionTimeout = timeout.isFinite ? max(0.1, min(60, timeout)) : 10
        wantsConnection = true
        state = .connecting
        try await withCheckedThrowingContinuation { continuation in
            connecting = continuation
            openConnection()
        }
    }
    /// A lost connection makes command outcome uncertain. Reconcile the snapshot
    /// before explicitly retrying the same ID within the host's runtime receipt
    /// window. This does not promise deduplication after a server restart/expiry.
    public func request(
        _ command: RemoteCommand, expectedRevision: Int? = nil,
        expectedTrackRevisions: [UUID: Int] = [:], id: UUID = UUID(), timeout: Double = 30
    ) async throws -> RemoteReply {
        try await withCheckedThrowingContinuation { continuation in
            do {
                try enqueue(
                    command, expectedRevision: expectedRevision, expectedTrackRevisions: expectedTrackRevisions,
                    id: id, timeout: timeout
                ) { continuation.resume(with: $0) }
            } catch { continuation.resume(throwing: error) }
        }
    }
    /// At most three contiguous upload chunks are put on the channel in order,
    /// before awaiting acknowledgments. The server's existing FIFO retains the
    /// strict offset/integrity semantics; no retries cross a connection change.
    public func uploadChunks(id: UUID, offset: Int, chunks: [Data]) async throws -> [RemoteReply] {
        guard
            pending.count + chunks.count <= RemoteProtocol.maximumPendingRequests
                - RemoteProtocol.reservedPendingInterruptRequests
        else { throw RemoteError.queueFull }
        return try await RemoteUploadBatch.run(id: id, offset: offset, chunks: chunks) { command, completion in
            try self.enqueue(command, id: UUID(), timeout: 30, completion: completion)
        }
    }
    private func enqueue(
        _ command: RemoteCommand, expectedRevision: Int? = nil,
        expectedTrackRevisions: [UUID: Int] = [:], id: UUID, timeout: Double,
        completion: @escaping RemoteUploadBatch.Completion
    ) throws {
        guard case .connected = state, let connection else { throw RemoteError.connectionLost }
        if let capabilities, !capabilities.supports(command) {
            throw RemoteError.rejected(
                "The connected server does not support this operation (\(command.capabilityName)).")
        }
        if case .uploadChunk(_, _, let data) = command, data.count > RemoteProtocol.maximumUploadChunkBytes {
            throw RemoteError.oversizedFrame
        }
        if case .editor(let intent) = command, (intent.payload?.count ?? 0) > 4 * 1024 * 1024 {
            throw RemoteError.oversizedFrame
        }
        let request = RemoteRequest(
            id: id, clientID: clientID, expectedRevision: expectedRevision,
            expectedTrackRevisions: expectedTrackRevisions, command: command)
        let deadline = Self.requestDeadline(command, timeout: timeout)
        let task = Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(deadline * 1_000_000_000))
            guard !Task.isCancelled, let self, let item = self.pending.removeValue(forKey: id) else { return }
            item.completion(.failure(RemoteError.timeout))
        }
        do { try pending.insert(Pending(completion: completion, timeout: task), for: id, command: command) } catch {
            task.cancel()
            throw error
        }
        connection.send(.request(request))
    }
    static func requestDeadline(_ command: RemoteCommand, timeout: Double) -> Double {
        let maximum: Double
        if case .editor(let intent) = command, intent.flags["separateSource"] == true,
            ["effects.apply", "effects.preview"].contains(intent.operation)
        {
            maximum = 3900
        } else {
            maximum = 600
        }
        return timeout.isFinite ? max(0.1, min(maximum, timeout)) : 30
    }
    public func disconnect() {
        wantsConnection = false
        reconnectTask?.cancel()
        reconnectTask = nil
        connectingTimeout?.cancel()
        connectingTimeout = nil
        connecting?.resume(throwing: RemoteError.connectionLost)
        connecting = nil
        let old = connection
        connection = nil
        old?.close()
        failPending(RemoteError.connectionLost)
        state = .disconnected
    }
    public func networkMetrics() async -> RemoteNetworkMetrics {
        guard let connection else { return .init() }
        return await withCheckedContinuation { continuation in connection.metrics { continuation.resume(returning: $0) }
        }
    }
    private func openConnection() {
        guard wantsConnection else { return }
        capabilities = nil
        featureDeclarationConnection = nil
        let framed: any RemoteServerChannel
        if let webEndpoint {
            do {
                let web = try RemoteWebClientConnection(
                    url: webEndpoint.url, origin: webEndpoint.origin, trustedCA: webEndpoint.trustedCA)
                let id = web.id
                web.onCapabilities = { [weak self] value in
                    Task { @MainActor [weak self] in
                        guard let self, self.connection?.id == id else { return }
                        self.capabilities = value
                        self.onCapabilities?(value)
                        self.declareClientFeatures()
                    }
                }
                framed = web
            } catch {
                lostConnection(error)
                return
            }
        } else if let endpoint {
            guard let native = try? credential.requireNativeCredential() else {
                lostConnection(RemoteError.invalidCredential)
                return
            }
            let socket = NWConnection(
                host: NWEndpoint.Host(endpoint.host), port: NWEndpoint.Port(rawValue: endpoint.port)!,
                using: remoteTLSParameters(native))
            framed = RemoteConnection(
                connection: socket,
                queue: DispatchQueue(label: "soundroom.remote.client.\(clientID)", qos: .userInitiated))
        } else {
            return
        }
        connection = framed
        let connectionID = framed.id
        helloSent = false
        connectingTimeout?.cancel()
        // Every attempt has a deadline, including reconnects stuck in NW waiting
        // before TLS is ready (the normal liveness watchdog begins after TLS).
        let timeout = connectionTimeout
        connectingTimeout = Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(timeout * 1_000_000_000))
            guard !Task.isCancelled, let self, self.connection?.id == connectionID else { return }
            self.connecting?.resume(throwing: RemoteError.timeout)
            self.connecting = nil
            self.connection?.close(RemoteError.timeout)
        }
        framed.onMessage = { [weak self] message, _, consumed in
            Task { @MainActor [weak self] in
                if let self, self.connection?.id == connectionID { self.receive(message) }
                consumed()
            }
        }
        framed.onClose = { [weak self] error in
            Task { @MainActor [weak self] in
                guard let self, self.connection?.id == connectionID else { return }
                self.connection = nil
                self.lostConnection(error ?? RemoteError.connectionLost)
            }
        }
        framed.start()
    }
    private func receive(_ message: RemoteWireMessage) {
        switch message {
        case .challenge(let challenge):
            guard !helloSent, challenge.version == RemoteProtocol.version,
                challenge.serverID == credential.serverID, challenge.nonce.count == 32,
                connection != nil
            else {
                connection?.close(RemoteError.protocolMismatch)
                return
            }
            if case .connected = state {
                connection?.close(RemoteError.invalidFrame)
                return
            }
            helloSent = true
            let peer = RemotePeer(id: clientID, name: name, role: credential.role)
            connection?.send(
                .hello(
                    .init(
                        serverID: credential.serverID, peer: peer, credentialID: credential.credentialID,
                        proof: credential.proof(for: peer, challenge: challenge.nonce))))
        case .welcome(let welcome):
            guard helloSent, welcome.version == RemoteProtocol.version, welcome.serverID == credential.serverID else {
                connection?.close(RemoteError.protocolMismatch)
                return
            }
            reconnectAttempt = 0
            state = .connected(serverID: welcome.serverID, name: welcome.name, role: welcome.role)
            connectingTimeout?.cancel()
            connectingTimeout = nil
            connecting?.resume()
            connecting = nil
            declareClientFeatures()
        case .reply(let reply):
            guard let item = pending.removeValue(forKey: reply.requestID) else { return }
            item.timeout.cancel()
            item.completion(.success(reply))
        case .event(let event):
            guard case .connected = state else {
                connection?.close(RemoteError.invalidFrame)
                return
            }
            if case .snapshot(let value) = event, value.serverID != credential.serverID {
                connection?.close(RemoteError.invalidCredential)
                return
            }
            guard apply(event) else {
                connection?.close(RemoteError.invalidFrame)
                return
            }
            onEvent?(event)
        case .failure(let message):
            wantsConnection = false
            connection?.close(RemoteError.rejected(message))
        default: connection?.close(RemoteError.invalidFrame)
        }
    }
    private func declareClientFeatures() {
        guard webEndpoint != nil, case .connected = state, let connection,
            featureDeclarationConnection != connection.id,
            let command = capabilities?.clientFeatureDeclaration()
        else { return }
        let id = connection.id
        featureDeclarationConnection = id
        Task { @MainActor [weak self] in
            guard let self, self.connection?.id == id else { return }
            // This is an optional encoding optimization. Rejection/timeout leaves
            // the decoder able to consume the original full-controls format.
            _ = try? await self.request(command, timeout: 5)
        }
    }
    @discardableResult private func apply(_ event: RemoteEvent) -> Bool {
        switch event {
        case .snapshot(let value): snapshot = value
        case .delta(let delta):
            guard var value = snapshot else { return false }
            if delta.revision < value.revision { return true }
            guard value.apply(delta) else { return false }
            snapshot = value
        case .transport(let value): snapshot?.transport = value
        case .devices(let devices, let input, let output):
            snapshot?.devices = devices
            snapshot?.inputDeviceID = input
            snapshot?.outputDeviceID = output
        case .presence(let peers): snapshot?.peers = peers
        case .status(let status): snapshot?.status = status
        case .waveform(let asset):
            if let index = snapshot?.assets.firstIndex(where: { $0.id == asset.id }) {
                snapshot?.assets[index] = asset
            } else {
                snapshot?.assets.append(asset)
            }
        default: break
        }
        return true
    }
    private func lostConnection(_ error: Error) {
        connectingTimeout?.cancel()
        connectingTimeout = nil
        connecting?.resume(throwing: error)
        connecting = nil
        failPending(RemoteError.connectionLost)
        guard wantsConnection, autoReconnect else {
            state = .failed(error.localizedDescription)
            return
        }
        reconnectAttempt += 1
        state = .reconnecting(attempt: reconnectAttempt)
        let delay = min(8.0, pow(2, Double(min(3, reconnectAttempt - 1))))
        reconnectTask = Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))
            guard !Task.isCancelled, let self, self.wantsConnection else { return }
            self.openConnection()
        }
    }
    private func failPending(_ error: Error) {
        let items = pending.removeAll()
        for item in items {
            item.timeout.cancel()
            item.completion(.failure(error))
        }
    }
}

public extension RemoteSnapshot {
    @discardableResult mutating func apply(_ delta: RemoteSessionDelta) -> Bool {
        guard delta.revision >= revision, delta.hasValidControlPatches(in: session) else { return false }
        revision = delta.revision
        session.tracks.removeAll { delta.removedTrackIDs.contains($0.id) }
        for track in delta.tracks {
            if track.id == Session.masterTrackID {
                session.masterTrack = track
            } else if let index = session.tracks.firstIndex(where: { $0.id == track.id }) {
                session.tracks[index] = track
            } else {
                session.tracks.append(track)
            }
        }
        for controls in delta.controls {
            if controls.trackID == Session.masterTrackID {
                controls.apply(to: &session.masterTrack)
            } else if let index = session.tracks.firstIndex(where: { $0.id == controls.trackID }) {
                controls.apply(to: &session.tracks[index])
            }
        }
        for patch in delta.controlPatches ?? [] {
            if patch.trackID == Session.masterTrackID {
                patch.apply(to: &session.masterTrack)
            } else if let index = session.tracks.firstIndex(where: { $0.id == patch.trackID }) {
                patch.apply(to: &session.tracks[index])
            }
        }
        if let order = delta.trackOrder {
            let lookup = Dictionary(uniqueKeysWithValues: session.tracks.map { ($0.id, $0) })
            session.tracks = order.compactMap { lookup[$0] } + session.tracks.filter { !order.contains($0.id) }
        }
        for (id, version) in delta.trackRevisions { trackRevisions[id] = version }
        for id in delta.removedTrackIDs { trackRevisions.removeValue(forKey: id) }
        if let timing = delta.timing { session.timing = timing }
        if let name = delta.name { session.name = name }
        if delta.metadataUpdated { session.masteringAnalysis = delta.masteringAnalysis }
        if let version = delta.sessionVersion { session.version = version }
        if let status = delta.status { self.status = status }
        for asset in delta.assets {
            if let index = assets.firstIndex(where: { $0.id == asset.id }) {
                assets[index] = asset
            } else {
                assets.append(asset)
            }
        }
        return true
    }
}
