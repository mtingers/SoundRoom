import XCTest
import Foundation
import Network
import SoundroomCore
@testable import SoundroomRemote

final class RemoteLANTests: XCTestCase {
    @MainActor func testTLSSharedSessionAndIndependentClientIdentities() async throws {
        let credential = try RemoteCredential.generate()
        let server = RemoteServer(credential: credential, name: "Test hardware host")
        var session = Session(); var revision = 0
        server.onConnect = { _ in .init(serverID: credential.serverID, serverName: server.name, session: session, revision: revision) }
        server.onRequest = { _, request in
            if case .setTrackMute(let id, let muted) = request.command,
               let index = session.tracks.firstIndex(where: { $0.id == id }) {
                session.tracks[index].muted = muted; revision += 1
                server.publish(.delta(.init(revision: revision, tracks: [session.tracks[index]], trackRevisions: [id: revision])))
            }
            return .init(requestID: request.id, revision: revision)
        }
        try await listen(server); defer { server.stop() }
        let first = RemoteClient(credential: credential, name: "GUI")
        let second = RemoteClient(credential: credential, name: "TUI")
        first.autoReconnect = false; second.autoReconnect = false
        defer { first.disconnect(); second.disconnect() }
        try await first.connect(host: "127.0.0.1", port: server.port, timeout: 5)
        try await second.connect(host: "127.0.0.1", port: server.port, timeout: 5)
        try await waitUntil { first.snapshot != nil && second.snapshot != nil }
        XCTAssertNotEqual(first.clientID, second.clientID)
        XCTAssertEqual(server.peers.count, 2)
        let result = try await first.request(.setTrackMute(trackID: session.tracks[0].id, muted: true))
        XCTAssertTrue(result.accepted)
        try await waitUntil { second.snapshot?.revision == 1 }
        XCTAssertTrue(second.snapshot?.session.tracks[0].muted ?? false)
        let metrics = await first.networkMetrics()
        XCTAssertGreaterThan(metrics.receivedFrames, 1)
    }
    @MainActor func testHostConfiguredRoleCannotBeElevatedByClientClaim() async throws {
        let owner = try RemoteCredential.generate()
        let viewer = try owner.paired(role: .viewer)
        let server = RemoteServer(credential: owner); try server.authorize(viewer)
        var acceptedRole: RemoteRole?
        server.onConnect = { peer in
            acceptedRole = peer.role
            return .init(serverID: owner.serverID, serverName: server.name, session: Session())
        }
        try await listen(server); defer { server.stop() }
        var dishonest = viewer; dishonest.role = .operator
        let client = RemoteClient(credential: dishonest); client.autoReconnect = false
        defer { client.disconnect() }
        try await client.connect(host: "127.0.0.1", port: server.port, timeout: 5)
        try await waitUntil { acceptedRole != nil }
        XCTAssertEqual(acceptedRole, .viewer)
        if case .connected(_, _, let role) = client.state { XCTAssertEqual(role, .viewer) }
        else { XCTFail("Not connected") }
    }
    @MainActor func testUnregisteredCredentialWithCorrectTLSKeyGetsNoSession() async throws {
        let owner = try RemoteCredential.generate()
        let stranger = try owner.paired()
        let server = RemoteServer(credential: owner)
        var connected = false; server.onConnect = { _ in connected = true; return .init(serverID: owner.serverID, serverName: "Host", session: Session()) }
        try await listen(server); defer { server.stop() }
        let client = RemoteClient(credential: stranger); client.autoReconnect = false
        defer { client.disconnect() }
        do { try await client.connect(host: "127.0.0.1", port: server.port, timeout: 2); XCTFail("Unregistered client was admitted") }
        catch { }
        XCTAssertFalse(connected); XCTAssertNil(client.snapshot); XCTAssertTrue(server.peers.isEmpty)
    }
    @MainActor func testUrgentLiveNotesAndStopBypassAwaitingRenderWithoutReorderingGates() async throws {
        let credential = try RemoteCredential.generate()
        let server = RemoteServer(credential: credential)
        var waiting = false, held = false
        var received: [String] = []
        server.onConnect = { _ in .init(serverID: credential.serverID, serverName: "Host", session: Session()) }
        server.onRequest = { _, request in
            if case .editor(let intent) = request.command {
                received.append(intent.operation)
                switch intent.operation {
                case "test.render": waiting = true; try? await Task.sleep(nanoseconds: 1_000_000_000); waiting = false
                case "designer.noteOn": held = true
                case "designer.noteOff": held = false
                default: break
                }
            }
            return .init(requestID: request.id, revision: 0)
        }
        try await listen(server); defer { server.stop() }
        let client = RemoteClient(credential: credential); client.autoReconnect = false
        defer { client.disconnect() }
        try await client.connect(host: "127.0.0.1", port: server.port, timeout: 5)
        let render = Task { try await client.request(.editor(.init(operation: "test.render"))) }
        try await waitUntil { waiting }
        let started = ProcessInfo.processInfo.systemUptime
        _ = try await client.request(.editor(.init(operation: "designer.noteOn")))
        _ = try await client.request(.editor(.init(operation: "designer.noteOff")))
        _ = try await client.request(.editor(.init(operation: "take.stop")))
        _ = try await client.request(.editor(.init(operation: "transport.finishRecording")))
        _ = try await client.request(.stop)
        XCTAssertLessThan(ProcessInfo.processInfo.systemUptime - started, 0.8, "Release/Stop waited for an unrelated render")
        XCTAssertEqual(Array(received.prefix(3)), ["test.render", "designer.noteOn", "designer.noteOff"])
        XCTAssertFalse(held)
        _ = try await render.value
    }
    @MainActor func testCapturedOperatorHelloFailsAfterReconnectChallengeChanges() async throws {
        let credential = try RemoteCredential.generate()
        let server = RemoteServer(credential: credential)
        var admitted = 0
        server.onConnect = { _ in admitted += 1; return .init(serverID: credential.serverID, serverName: "Host", session: Session()) }
        try await listen(server); defer { server.stop() }
        let peer = RemotePeer(id: UUID(), name: "Operator")
        let original = ChallengeTestClient(credential: credential, peer: peer, port: server.port)
        defer { original.close() }
        try await waitUntil { original.authenticated && admitted == 1 }
        let captured = try XCTUnwrap(original.proof), previous = try XCTUnwrap(original.challenge)
        original.close(); try await waitUntil { server.peers.isEmpty }
        let replay = ChallengeTestClient(credential: credential, peer: peer, port: server.port, replay: captured)
        defer { replay.close() }
        try await waitUntil { replay.closed }
        XCTAssertNotEqual(replay.challenge, previous)
        XCTAssertFalse(replay.authenticated); XCTAssertEqual(admitted, 1); XCTAssertTrue(server.peers.isEmpty)
    }
    @MainActor func testUnavailableHostHasBoundedInitialAndReconnectAttempts() async throws {
        let client = RemoteClient(credential: try RemoteCredential.generate())
        defer { client.disconnect() }
        let started = ProcessInfo.processInfo.systemUptime
        // TEST-NET has no audio server; this exercises a stalled or refused
        // pre-TLS connection rather than an authenticated heartbeat timeout.
        do { try await client.connect(host: "192.0.2.1", port: 49152, timeout: 0.1); XCTFail("Unavailable host connected") }
        catch { }
        XCTAssertLessThan(ProcessInfo.processInfo.systemUptime - started, 1)
        try await waitUntil {
            if case .reconnecting(let attempt) = client.state { return attempt >= 2 }
            return false
        }
    }
    @MainActor private func listen(_ server: RemoteServer) async throws {
        var failure: String?
        server.onError = { failure = $0 }
        try server.start(port: 0)
        for _ in 0..<500 {
            if server.port > 0 { return }
            if let failure {
                server.stop()
                if failure.contains("Operation not permitted") {
                    throw XCTSkip("The execution sandbox prohibits network listeners. Run the LAN checks outside that sandbox: \(failure)")
                }
                throw RemoteError.rejected(failure)
            }
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        throw RemoteError.timeout
    }
    @MainActor private func waitUntil(_ predicate: @escaping @MainActor () -> Bool) async throws {
        for _ in 0..<500 {
            if predicate() { return }
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        throw RemoteError.timeout
    }
}

/// Raw protocol peer lets the LAN test replay a captured hello while preserving
/// the runtime identity. Regular RemoteClient always computes a fresh response.
@MainActor private final class ChallengeTestClient {
    private let connection: RemoteConnection
    private let credential: RemoteCredential
    private let peer: RemotePeer
    private let replay: String?
    private(set) var challenge: Data?
    private(set) var proof: String?
    private(set) var authenticated = false
    private(set) var closed = false
    init(credential: RemoteCredential, peer: RemotePeer, port: UInt16, replay: String? = nil) {
        self.credential = credential; self.peer = peer; self.replay = replay
        let socket = NWConnection(host: "127.0.0.1", port: NWEndpoint.Port(rawValue: port)!, using: remoteTLSParameters(credential))
        connection = RemoteConnection(connection: socket, queue: DispatchQueue(label: "soundroom.test.challenge"))
        connection.onMessage = { [weak self] message, _, consumed in
            Task { @MainActor [weak self] in self?.receive(message); consumed() }
        }
        connection.onClose = { [weak self] _ in Task { @MainActor [weak self] in self?.closed = true } }
        connection.start()
    }
    func close() { connection.close() }
    private func receive(_ message: RemoteWireMessage) {
        switch message {
        case .challenge(let value):
            challenge = value.nonce
            let response = replay ?? credential.proof(for: peer, challenge: value.nonce); proof = response
            connection.send(.hello(.init(serverID: credential.serverID, peer: peer, credentialID: credential.clientID, proof: response)))
        case .welcome: authenticated = true
        default: break
        }
    }
}
