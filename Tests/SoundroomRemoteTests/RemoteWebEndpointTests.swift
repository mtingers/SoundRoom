import XCTest
import Foundation
import Network
import Security
import SoundroomCore
@testable import SoundroomRemote

/// Opt-in TLS/WSS test with a provisioned CA, never a trust-all delegate.
/// SOUNDROOM_WEB_TLS_DIRECTORY=... points to provision-web-tls.py's output.
final class RemoteWebEndpointTests: XCTestCase {
    @MainActor func testSixteenCommonClientsRetainDenseReplacementAndLatestOverlays() async throws {
        guard let path = ProcessInfo.processInfo.environment["SOUNDROOM_WEB_TLS_DIRECTORY"] else { throw XCTSkip("Opt-in provisioned HTTPS/WSS endpoint test") }
        let directory = URL(fileURLWithPath: path, isDirectory: true)
        let password = String(data: try RemotePrivateFile.read(directory.appendingPathComponent("identity.password")), encoding: .utf8)!.trimmingCharacters(in: .newlines)
        let identity = try RemoteWebConfiguration.loadIdentity(pkcs12: directory.appendingPathComponent("server.p12"), password: password)
        let pem = try String(contentsOf: directory.appendingPathComponent("ca.crt"), encoding: .utf8)
        let der = try XCTUnwrap(Data(base64Encoded: pem.components(separatedBy: .newlines).filter { !$0.hasPrefix("---") }.joined()))
        let ca = try XCTUnwrap(SecCertificateCreateWithData(nil, der as CFData))
        let assets = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: assets, withIntermediateDirectories: true)
        try Data("<html>Dense common endpoint</html>".utf8).write(to: assets.appendingPathComponent("index.html"))
        defer { try? FileManager.default.removeItem(at: assets) }
        let credential = try RemoteCredential.generate()
        let server = RemoteServer(credential: credential, name: "Dense common host")
        server.webCapabilities = .init(deviceBackends: ["test"], codecs: ["WAV"], snapshotPagination: true, features: ["transport"])
        var authored = Session(); var pattern = SoundDesignPattern()
        pattern.notes = (0..<100000).map { SoundDesignNote(pitch: 60 + $0 % 12, startBeat: Double($0 % 64) / 4) }
        var clip = Clip(assetID: UUID(), name: "Dense MIDI", start: 0, length: 48000); clip.soundDesign = pattern
        authored.tracks[0].clips = [clip]
        let trackID = authored.tracks[0].id
        var current = RemoteSnapshot(serverID: credential.serverID, serverName: server.name, session: authored,
                                     revision: 7, trackRevisions: [trackID: 7], transport: .init(hostTime: 1))
        let documentBytes = try RemoteSnapshotPages(snapshot: current).bytes.count
        XCTAssertGreaterThan(documentBytes, RemoteProtocol.maximumFrameBytes)
        var lateStarted = false; var releaseLate = false
        server.onConnect = { peer in
            let captured = current
            if peer.name == "Dense 16" {
                lateStarted = true
                while !releaseLate { try? await Task.sleep(for: .milliseconds(10)) }
            }
            return captured
        }
        server.onRequest = { _, request in .init(requestID: request.id, revision: current.revision) }
        try server.start(port: 0)
        defer { releaseLate = true; server.stop() }
        let origin = URL(string: "https://localhost:49264")!
        let endpoint = URL(string: "wss://localhost:49265/")!
        try server.startWeb(configuration: .init(identity: identity, assetDirectory: assets, origin: origin, websocketPort: 49265))
        let before = await RemoteSnapshotEncodingPool.shared.statistics()
        let clients = (1...16).map { RemoteClient(credential: credential, name: "Dense \($0)") }
        clients.forEach { client in
            client.autoReconnect = false
            let name = client.name
            client.onState = { state in print("Dense client \(name): \(state)") }
        }
        defer { clients.forEach { $0.disconnect() } }
        let connects = clients.map { client in Task { try await client.connectWeb(websocketURL: endpoint, origin: origin, trustedCA: ca, timeout: 60) } }
        defer { connects.forEach { $0.cancel() } }
        for task in connects { try await task.value }
        var busyClient: RemoteClient?
        let decodeDeadline = ContinuousClock.now + .seconds(60)
        while busyClient == nil && ContinuousClock.now < decodeDeadline {
            for client in clients.dropLast() {
                let metrics = await client.networkMetrics()
                if metrics.queuedBytes >= documentBytes { busyClient = client; break }
            }
            if busyClient == nil { try await Task.sleep(for: .milliseconds(20)) }
        }
        let decodingClient = try XCTUnwrap(busyClient, "No dense decode backlog was observed")
        let stopBegan = ContinuousClock.now
        let stopReply = try await decodingClient.request(.stop, timeout: 3)
        XCTAssertTrue(stopReply.accepted)
        print("Dense WSS Stop receipt with decode backlog: \(stopBegan.duration(to: .now))")
        try await waitUntil("Fifteen dense clients did not install the initial snapshot") {
            lateStarted && clients.dropLast().allSatisfy { client in
                guard case .connected = client.state else { return false }
                return client.snapshot?.revision == 7
            }
        }
        XCTAssertEqual(server.peers.count, 16)
        let initial = await RemoteSnapshotEncodingPool.shared.statistics()
        XCTAssertEqual(initial.documentEncodes - before.documentEncodes, 1, "Equal documents must share serialization across actual connections")
        // The sixteenth peer deliberately captured the old revision while its
        // initial snapshot was held. It must replay the replacement and newer
        // live metadata after installing that old base.
        var replacement = authored.tracks[0]
        replacement.name = "Replaced dense performance"
        replacement.clips[0].soundDesign?.notes[50000].pitch = 72
        current.session.tracks[0] = replacement; current.revision = 8; current.trackRevisions[trackID] = 8
        server.publish(.delta(.init(revision: 8, tracks: [replacement], trackRevisions: [trackID: 8], status: "Replacement accepted")))
        current.transport = .init(frame: 32123, isPlaying: true, hostTime: 2)
        current.status = "Latest live status"
        server.publish(.transport(current.transport)); server.publish(.status(current.status))
        releaseLate = true
        try await waitUntil("Dense replacement or latest overlays were lost") {
            clients.allSatisfy { client in
                guard case .connected = client.state else { return false }
                return client.snapshot?.revision == 8 && client.snapshot?.transport == current.transport && client.snapshot?.status == current.status && client.snapshot?.peers.count == 16
            }
        }
        for client in clients {
            let installed = try XCTUnwrap(client.snapshot)
            XCTAssertTrue(installed.session == current.session, "Dense replacement changed for \(client.name)")
            XCTAssertEqual(installed.trackRevisions, current.trackRevisions)
            let reply = try await client.request(.ping(clientTime: 1))
            XCTAssertTrue(reply.accepted)
        }
        let after = await RemoteSnapshotEncodingPool.shared.statistics()
        XCTAssertEqual(after.documentEncodes - before.documentEncodes, 2, "Initial and replacement documents should each encode once")
        XCTAssertLessThanOrEqual(after.retainedBytes, 128 * 1024 * 1024)
        print("Dense WSS acceptance: peers=16 notes=100000 bodyBytes=\(documentBytes) documentEncodes=\(after.documentEncodes - before.documentEncodes) retainedSerializedBytes=\(after.retainedBytes)")
    }

    @MainActor private func waitUntil(_ message: String, condition: () -> Bool) async throws {
        let deadline = ContinuousClock.now + .seconds(60)
        while !condition() {
            guard ContinuousClock.now < deadline else { throw RemoteError.rejected(message) }
            try await Task.sleep(for: .milliseconds(20))
        }
    }

    @MainActor func testHTTPSAssetsAndBrowserAuthenticationShareNativeAuthority() async throws {
        guard let path = ProcessInfo.processInfo.environment["SOUNDROOM_WEB_TLS_DIRECTORY"] else { throw XCTSkip("Opt-in provisioned HTTPS/WSS endpoint test") }
        let directory = URL(fileURLWithPath: path, isDirectory: true)
        let password = String(data: try RemotePrivateFile.read(directory.appendingPathComponent("identity.password")), encoding: .utf8)!.trimmingCharacters(in: .newlines)
        let identity = try RemoteWebConfiguration.loadIdentity(pkcs12: directory.appendingPathComponent("server.p12"), password: password)
        let pem = try String(contentsOf: directory.appendingPathComponent("ca.crt"), encoding: .utf8)
        let der = try XCTUnwrap(Data(base64Encoded: pem.components(separatedBy: .newlines).filter { !$0.hasPrefix("---") }.joined()))
        let ca = try XCTUnwrap(SecCertificateCreateWithData(nil, der as CFData))
        let assets = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: assets, withIntermediateDirectories: true)
        try Data("<html>Soundroom endpoint test</html>".utf8).write(to: assets.appendingPathComponent("index.html"))
        defer { try? FileManager.default.removeItem(at: assets) }
        let credential = try RemoteCredential.generate()
        let server = RemoteServer(credential: credential, name: "Browser test host")
        server.webCapabilities = .init(deviceBackends: ["test"], codecs: ["WAV"], snapshotPagination: true, features: ["transport"])
        var largeSession = Session(); var pattern = SoundDesignPattern()
        pattern.notes = (0..<100000).map { SoundDesignNote(pitch: 60 + $0 % 12, startBeat: Double($0 % 64) / 4) }
        var clip = Clip(assetID: UUID(), name: "Dense MIDI", start: 0, length: 48000); clip.soundDesign = pattern
        largeSession.tracks[0].clips = [clip]
        let largeSnapshot = RemoteSnapshot(serverID: credential.serverID, serverName: server.name, session: largeSession, revision: 7)
        var received: [RemoteRequest] = []
        server.onConnect = { peer in peer.name == "Native keeper" ? .init(serverID: credential.serverID, serverName: server.name, session: Session()) : largeSnapshot }
        server.onRequest = { peer, request in
            XCTAssertEqual(peer.id, request.clientID); received.append(request)
            return .init(requestID: request.id, revision: 1)
        }
        var listenerError: String?
        server.onError = { listenerError = $0 }
        do { try server.start(port: 0) }
        catch let error as NWError {
            if case .posix(.EPERM) = error { throw XCTSkip("Listener denied by execution sandbox") }; throw error
        }
        defer { server.stop() }
        let origin = URL(string: "https://localhost:49254")!
        try server.startWeb(configuration: .init(identity: identity, assetDirectory: assets, origin: origin, websocketPort: 49255))
        let trust = TestWebTrust(certificate: ca)
        let sessionConfiguration = URLSessionConfiguration.ephemeral
        sessionConfiguration.timeoutIntervalForRequest = 3; sessionConfiguration.timeoutIntervalForResource = 10
        let session = URLSession(configuration: sessionConfiguration, delegate: trust, delegateQueue: nil)
        defer { session.invalidateAndCancel() }
        var asset: Data?
        for _ in 0..<40 {
            if let error = listenerError {
                if error.localizedCaseInsensitiveContains("not permitted") { throw XCTSkip("Listener denied by execution sandbox") }
                throw RemoteError.rejected(error)
            }
            if let value = try? await session.data(from: origin.appendingPathComponent("soundroom-config.json")) { asset = value.0; break }
            try await Task.sleep(for: .milliseconds(25))
        }
        let config = try XCTUnwrap(JSONSerialization.jsonObject(with: XCTUnwrap(asset)) as? [String: Any])
        XCTAssertEqual(config["wssURL"] as? String, "wss://localhost:49255/")
        let (html, response) = try await session.data(from: origin)
        XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 200)
        XCTAssertTrue(String(data: html, encoding: .utf8)!.contains("Soundroom endpoint test"))
        XCTAssertEqual((response as? HTTPURLResponse)?.value(forHTTPHeaderField: "X-Content-Type-Options"), "nosniff")
        var request = URLRequest(url: URL(string: "wss://localhost:49255/")!)
        request.setValue(origin.absoluteString, forHTTPHeaderField: "Origin")
        request.setValue("soundroom.v3", forHTTPHeaderField: "Sec-WebSocket-Protocol")
        let socket = session.webSocketTask(with: request); socket.resume()
        defer { socket.cancel(with: .normalClosure, reason: nil) }
        let challengeMessage = try RemoteV3Codec.decodeMessage(await receive(socket))
        guard case .challenge(let challenge) = challengeMessage else { return XCTFail("Missing browser challenge") }
        let grant = try server.issueWebPairing(name: "Test browser", role: .editor)
        try await send(socket, object: ["version": 3, "kind": "pair", "code": grant.code, "name": grant.name])
        let pairedData = try await receive(socket)
        let paired = try XCTUnwrap(JSONSerialization.jsonObject(with: pairedData) as? [String: Any])
        XCTAssertEqual(paired["kind"] as? String, "paired")
        let browser = try XCTUnwrap(paired["credential"] as? [String: Any])
        XCTAssertNil(browser["secret"])
        let clientID = try XCTUnwrap(UUID(uuidString: browser["credentialID"] as? String ?? ""))
        let clientSecret = try XCTUnwrap(Data(base64Encoded: browser["clientSecret"] as? String ?? ""))
        let browserCredential = RemoteCredential(serverID: credential.serverID, clientID: clientID, secret: credential.secret, clientSecret: clientSecret, role: .operator)
        let peer = RemotePeer(id: UUID(), name: grant.name, role: .operator)
        let hello = RemoteHello(serverID: credential.serverID, peer: peer, credentialID: clientID, proof: browserCredential.proof(for: peer, challenge: challenge.nonce))
        try await socket.send(.string(String(data: RemoteV3Codec.encodeMessage(.hello(hello)), encoding: .utf8)!))
        var welcome: RemoteWelcome?
        for _ in 0..<4 {
            let message = try RemoteV3Codec.decodeMessage(await receive(socket))
            if case .welcome(let value) = message { welcome = value; break }
        }
        XCTAssertEqual(welcome?.role, .editor, "Client role claim must not elevate approved access")
        XCTAssertTrue(server.peers.contains { $0.id == peer.id && $0.role == .editor })
        var stream = RemoteV3ClientStream(); var capabilities: RemoteCapabilities?
        let installed = try await receiveSnapshot(socket, stream: &stream, capabilities: &capabilities)
        assertDocument(installed, equals: largeSnapshot)
        XCTAssertEqual(installed.transport, largeSnapshot.transport)
        XCTAssertTrue(capabilities?.snapshotPagination ?? false)
        XCTAssertGreaterThan(try RemoteSnapshotPages(snapshot: installed).bytes.count, RemoteProtocol.maximumFrameBytes, "Live test must exercise delivery beyond the old frame bound")
        XCTAssertEqual(server.webClients, [.init(id: clientID, name: grant.name, role: .editor)])
        let keeper = RemoteClient(credential: credential, name: "Native keeper"); keeper.autoReconnect = false
        try await keeper.connect(host: "localhost", port: server.port)
        defer { keeper.disconnect() }
        let command = RemoteRequest(clientID: peer.id, command: .addTrack(name: "Browser upload destination"))
        // Browser sends text frames; binary protocol messages are intentionally rejected.
        try await socket.send(.string(String(data: RemoteV3Codec.encodeMessage(.request(command)), encoding: .utf8)!))
        var reply: RemoteReply?
        for _ in 0..<5 {
            let message = try RemoteV3Codec.decodeMessage(await receive(socket))
            if case .reply(let value) = message { reply = value; break }
        }
        XCTAssertTrue(reply?.accepted ?? false); XCTAssertEqual(received.map(\.id), [command.id])
        // This native controller uses the exact same v3 WSS endpoint/paged
        // document as the browser, retaining CA and hostname verification.
        let swiftWeb = RemoteClient(credential: credential, name: "Swift v3 controller"); swiftWeb.autoReconnect = false
        try await swiftWeb.connectWeb(websocketURL: URL(string: "wss://localhost:49255/")!, origin: origin, trustedCA: ca)
        defer { swiftWeb.disconnect() }
        for _ in 0..<160 {
            if swiftWeb.snapshot?.peers.contains(where: { $0.id == swiftWeb.clientID }) == true { break }
            try await Task.sleep(for: .milliseconds(50))
        }
        assertDocument(try XCTUnwrap(swiftWeb.snapshot), equals: largeSnapshot)
        XCTAssertEqual(swiftWeb.snapshot?.transport, largeSnapshot.transport)
        XCTAssertEqual(Set(swiftWeb.snapshot?.peers.map(\.id) ?? []), Set(server.peers.map(\.id)))
        XCTAssertEqual(swiftWeb.capabilities?.snapshotPagination, true)
        let swiftReply = try await swiftWeb.request(.ping(clientTime: 1))
        XCTAssertTrue(swiftReply.accepted)
        try server.revokeWebCredential(id: clientID)
        XCTAssertTrue(server.webClients.isEmpty)
        let keeperReply = try await keeper.request(.ping(clientTime: ProcessInfo.processInfo.systemUptime))
        XCTAssertTrue(keeperReply.accepted, "Revoking one browser must retain another native client's credential and connection")
        XCTAssertFalse(server.peers.contains { $0.id == peer.id })
        let revokedRequest = RemoteRequest(clientID: peer.id, command: .addTrack(name: "Must not execute"))
        try? await socket.send(.string(String(data: RemoteV3Codec.encodeMessage(.request(revokedRequest)), encoding: .utf8)!))
        // Presence/status frames already in URLSession's buffer may be readable
        // after revocation. They do not prove an authenticated connection survives.
        var timedOut = false
        let deadline = Task { @MainActor in
            try? await Task.sleep(for: .seconds(3))
            guard !Task.isCancelled else { return }
            timedOut = true; socket.cancel(with: .goingAway, reason: nil)
        }
        defer { deadline.cancel() }
        var closed = false
        do { for _ in 0..<256 { _ = try await socket.receive() } } catch { closed = true }
        XCTAssertTrue(closed && !timedOut, "Revoked socket did not close within the bounded drain")
        XCTAssertFalse(received.contains { $0.id == revokedRequest.id }, "Revocation must reject further requests")
    }

    private func receive(_ socket: URLSessionWebSocketTask) async throws -> Data {
        switch try await socket.receive() {
        case .data(let data): return data
        case .string(let value): return Data(value.utf8)
        @unknown default: throw RemoteError.invalidFrame
        }
    }
    private func send(_ socket: URLSessionWebSocketTask, object: [String: Any]) async throws {
        try await socket.send(.string(String(data: JSONSerialization.data(withJSONObject: object), encoding: .utf8)!))
    }
    private func assertDocument(_ actual: RemoteSnapshot, equals expected: RemoteSnapshot, file: StaticString = #filePath, line: UInt = #line) {
        // Avoid printing thirty megabytes of MIDI notes on a metadata mismatch.
        XCTAssertTrue(actual.session == expected.session, "Session document changed", file: file, line: line)
        XCTAssertEqual(actual.serverID, expected.serverID, file: file, line: line)
        XCTAssertEqual(actual.serverName, expected.serverName, file: file, line: line)
        XCTAssertEqual(actual.revision, expected.revision, file: file, line: line)
        XCTAssertEqual(actual.trackRevisions, expected.trackRevisions, file: file, line: line)
        XCTAssertEqual(actual.assets, expected.assets, file: file, line: line)
        XCTAssertEqual(actual.devices, expected.devices, file: file, line: line)
        XCTAssertEqual(actual.inputDeviceID, expected.inputDeviceID, file: file, line: line)
        XCTAssertEqual(actual.outputDeviceID, expected.outputDeviceID, file: file, line: line)
        XCTAssertEqual(actual.projectName, expected.projectName, file: file, line: line)
    }
    private func receiveSnapshot(_ socket: URLSessionWebSocketTask, stream: inout RemoteV3ClientStream, capabilities: inout RemoteCapabilities?) async throws -> RemoteSnapshot {
        var document = RemoteWebDocumentState()
        for _ in 0..<200 {
            let data = try await receive(socket)
            for message in try stream.accept(data) {
                if case .heartbeat(let id) = message { try await socket.send(.string(String(data: RemoteV3Codec.encodeMessage(.heartbeatReply(id)), encoding: .utf8)!)) }
                try document.apply(message)
            }
            capabilities = stream.capabilities
            if let snapshot = document.snapshot { return snapshot }
        }
        throw RemoteError.timeout
    }
}

private final class TestWebTrust: NSObject, URLSessionDelegate, @unchecked Sendable {
    let certificate: SecCertificate
    init(certificate: SecCertificate) { self.certificate = certificate }
    func urlSession(_ session: URLSession, didReceive challenge: URLAuthenticationChallenge,
                    completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void) {
        guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
              let trust = challenge.protectionSpace.serverTrust else { completionHandler(.performDefaultHandling, nil); return }
        SecTrustSetAnchorCertificates(trust, [certificate] as CFArray)
        SecTrustSetAnchorCertificatesOnly(trust, true)
        guard SecTrustEvaluateWithError(trust, nil) else { completionHandler(.cancelAuthenticationChallenge, nil); return }
        completionHandler(.useCredential, URLCredential(trust: trust))
    }
}
