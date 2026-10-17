import Foundation
import SoundroomCore
import XCTest

@testable import SoundroomRemote

final class RemoteProtocolTests: XCTestCase {
    func testFramingSurvivesEveryFragmentBoundaryAndMultipleMessages() throws {
        let payloads = [Data("first".utf8), Data(repeating: 29, count: 4096), Data("last".utf8)]
        let stream = try payloads.reduce(into: Data()) { $0.append(try RemoteFrameDecoder.encode($1)) }
        for chunkSize in [1, 2, 3, 4, 7, 64, 4096, stream.count] {
            var decoder = RemoteFrameDecoder()
            var decoded: [Data] = []
            for offset in stride(from: 0, to: stream.count, by: chunkSize) {
                decoded += try decoder.append(stream.subdata(in: offset..<min(stream.count, offset + chunkSize)))
            }
            XCTAssertEqual(decoded, payloads, "fragment size \(chunkSize)")
        }
    }
    func testFramingRejectsOversizedHeaderBeforeReadingPayloadAndRejectsEmptyFrame() throws {
        var decoder = RemoteFrameDecoder()
        XCTAssertThrowsError(try decoder.append(Data([0, 128, 0, 1]))) {
            XCTAssertEqual($0 as? RemoteError, .oversizedFrame)
        }
        decoder = RemoteFrameDecoder()
        XCTAssertThrowsError(try decoder.append(Data([0, 0, 0, 0]))) {
            XCTAssertEqual($0 as? RemoteError, .invalidFrame)
        }
    }
    func testReliableMessagesStayOrderedAndTelemetryIsLatestValue() throws {
        var queue = RemoteOutboundQueue()
        try queue.append(Data([1]), telemetryKey: "transport")
        try queue.append(Data([2]), telemetryKey: "transport")
        try queue.append(Data([9]), telemetryKey: "meters")
        try queue.append(Data([3]))
        try queue.append(Data([4]))
        XCTAssertEqual(queue.discardedTelemetry, 1)
        XCTAssertEqual(queue.bytes, 4)
        XCTAssertEqual(queue.next(), Data([3]))
        XCTAssertEqual(queue.next(), Data([4]))
        XCTAssertEqual(queue.next(), Data([2]))
        XCTAssertEqual(queue.next(), Data([9]))
        XCTAssertNil(queue.next())
        XCTAssertEqual(queue.bytes, 0)
    }
    func testContinuousReliableTrafficReleasesSentBuffersAndBoundsBookkeeping() throws {
        var queue = RemoteOutboundQueue()
        try queue.append(Data([0]))
        for index in 0..<10_000 {
            try queue.append(Data([UInt8(index % 255)]))
            XCTAssertNotNil(queue.next())
            XCTAssertEqual(queue.bytes, 1)
            XCTAssertLessThanOrEqual(queue.retainedFrameSlots, 256)
        }
        XCTAssertNotNil(queue.next())
        XCTAssertNil(queue.next())
    }
    func testSlowSubscriberCannotRetainUnboundedReliableData() throws {
        var queue = RemoteOutboundQueue()
        let frame = Data(repeating: 1, count: RemoteProtocol.maximumFrameBytes)
        for _ in 0..<4 { try queue.append(frame) }
        XCTAssertEqual(queue.bytes, RemoteProtocol.maximumQueuedBytes)
        XCTAssertThrowsError(try queue.append(Data([2]))) { XCTAssertEqual($0 as? RemoteError, .queueFull) }
        // Cosmetic frames are discarded without dropping reliable edits.
        try queue.append(Data([4]), telemetryKey: "meters")
        XCTAssertEqual(queue.discardedTelemetry, 1)
        XCTAssertEqual(queue.reliableCount, 4)
    }
    func testPreEncodingMailboxCoalescesTelemetryAndBoundsRetainedModels() throws {
        var queue = RemoteMessageMailbox()
        try queue.append(.event(.transport(.init(frame: 1))), telemetryKey: "transport")
        try queue.append(.event(.transport(.init(frame: 2))), telemetryKey: "transport")
        XCTAssertEqual(queue.count, 1)
        XCTAssertEqual(queue.discardedTelemetry, 1)
        let snapshot = RemoteSnapshot(serverID: UUID(), serverName: "Host", session: Session())
        for _ in 0..<3 { try queue.append(.event(.snapshot(snapshot)), telemetryKey: nil) }
        XCTAssertThrowsError(try queue.append(.event(.snapshot(snapshot)), telemetryKey: nil))
        XCTAssertLessThanOrEqual(queue.bytes, RemoteProtocol.maximumQueuedBytes)
        for _ in 0..<3 {
            guard case .event(.snapshot)? = queue.next()?.message else { return XCTFail("Reliable state lost") }
        }
        guard case .event(.transport(let transport))? = queue.next()?.message else {
            return XCTFail("Latest transport lost")
        }
        XCTAssertEqual(transport.frame, 2)
        XCTAssertEqual(queue.bytes, 0)
    }
    func testMixerControlDeltaDoesNotSerializeOrReplaceClipNotes() throws {
        var session = Session()
        var pattern = SoundDesignPattern()
        pattern.notes = (0..<256).map {
            SoundDesignNote(pitch: 60, velocity: 0.8, startBeat: Double($0) / 4, durationBeats: 0.2)
        }
        var clip = Clip(assetID: UUID(), name: "MIDI", start: 0, length: 48000)
        clip.soundDesign = pattern
        session.tracks[0].clips = [clip]
        var controls = RemoteTrackControls(track: session.tracks[0])
        controls.gain = 0.4
        let delta = RemoteSessionDelta(revision: 1, controls: [controls])
        let encoded = try JSONEncoder().encode(delta)
        XCTAssertLessThan(encoded.count, 2048, "A fader change retransmitted the note arrangement")
        var snapshot = RemoteSnapshot(serverID: UUID(), serverName: "Host", session: session)
        snapshot.apply(delta)
        XCTAssertEqual(snapshot.session.tracks[0].gain, 0.4)
        XCTAssertEqual(snapshot.session.tracks[0].clips, [clip])
        XCTAssertEqual(snapshot.session.tracks[0].clips[0].soundDesign?.notes.count, 256)
    }
    func testScalarMailboxBurstRetainsReliableOrderWithoutFullFrameCharges() throws {
        var queue = RemoteMessageMailbox()
        let trackID = UUID()
        try queue.append(.event(.transport(.init(frame: 5))), telemetryKey: "transport")
        for revision in 1...RemoteProtocol.maximumPendingRequests {
            let delta = RemoteSessionDelta(
                revision: revision,
                controlPatches: [.init(trackID: trackID, gain: 0.7)],
                trackRevisions: [trackID: revision])
            try queue.append(.event(.delta(delta)), telemetryKey: nil)
        }
        XCTAssertLessThan(queue.bytes, 2 * 1024 * 1024)
        XCTAssertThrowsError(try queue.append(.heartbeat(UUID()), telemetryKey: nil)) {
            XCTAssertEqual($0 as? RemoteError, .queueFull)
        }
        for revision in 1...RemoteProtocol.maximumPendingRequests {
            guard case .event(.delta(let delta))? = queue.next()?.message else {
                return XCTFail("Reliable scalar edit lost")
            }
            XCTAssertEqual(delta.revision, revision)
        }
        guard case .event(.transport(let transport))? = queue.next()?.message else {
            return XCTFail("Latest transport lost")
        }
        XCTAssertEqual(transport.frame, 5)
        XCTAssertEqual(queue.bytes, 0)
    }
    func testScalarMailboxBudgetCoversLargestBoundedWireShapesAndDenseFallback() throws {
        let ids = (0..<1024).map { _ in UUID() }
        let patches = ids.map {
            RemoteTrackControlPatch(trackID: $0, gain: 1.5, pan: -1, muted: false, solo: true, armed: false)
        }
        let maximumWireInteger = 9_007_199_254_740_991
        let delta = RemoteSessionDelta(
            revision: maximumWireInteger, controlPatches: patches,
            removedTrackIDs: (0..<1024).map { _ in UUID() },
            trackRevisions: Dictionary(uniqueKeysWithValues: ids.map { ($0, maximumWireInteger) }),
            status: String(repeating: "\u{0001}", count: 4096))
        let message = RemoteWireMessage.event(.delta(delta))
        var queue = RemoteMessageMailbox()
        try queue.append(message, telemetryKey: nil)
        XCTAssertGreaterThanOrEqual(queue.bytes, try JSONEncoder().encode(message).count)
        XCTAssertGreaterThanOrEqual(queue.bytes, try RemoteV3Codec.encodeMessage(message).count)
        XCTAssertLessThan(queue.bytes, RemoteProtocol.maximumFrameBytes)

        // Large assets and whole-track controls must keep conservative admission.
        var assetDelta = RemoteSessionDelta(revision: 1)
        assetDelta.assets = [
            .init(
                id: UUID(), frames: 48_000, channels: 2,
                peaks: Array(repeating: 0.5, count: 100_000))
        ]
        var controls = RemoteTrackControls(track: Session().tracks[0])
        controls.mixAutomation["volume"] = (0..<100_000).map { AutomationPoint(frame: $0, value: 0.5) }
        let dense = [
            assetDelta, RemoteSessionDelta(revision: 1, controls: [controls]),
            RemoteSessionDelta(revision: 1, controlPatches: patches + [patches[0]]),
        ]
        for delta in dense {
            var bounded = RemoteMessageMailbox()
            try bounded.append(.event(.delta(delta)), telemetryKey: nil)
            XCTAssertEqual(bounded.bytes, RemoteProtocol.maximumFrameBytes)
        }
    }
    func testCredentialRoundTripRejectsInsecurePermissionsAndSymlinks() throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: folder) }
        let url = folder.appendingPathComponent("operator.json")
        let credential = try RemoteCredential.generate()
        try credential.write(to: url)
        XCTAssertEqual(try RemoteCredential.load(from: url), credential)
        try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: url.path)
        XCTAssertThrowsError(try RemoteCredential.load(from: url))
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
        let alias = folder.appendingPathComponent("alias.json")
        try FileManager.default.createSymbolicLink(at: alias, withDestinationURL: url)
        XCTAssertThrowsError(try RemoteCredential.load(from: alias))
    }
    func testCredentialProofBindsClientIdentityAndNameAndPairingScope() throws {
        let credential = try RemoteCredential.generate()
        let peer = RemotePeer(id: UUID(), name: "Studio")
        let challenge = try RemoteCredential.authenticationNonce()
        let proof = credential.proof(for: peer, challenge: challenge)
        XCTAssertTrue(credential.authenticates(peer: peer, proof: proof, challenge: challenge))
        var other = peer
        other.id = UUID()
        XCTAssertFalse(credential.authenticates(peer: other, proof: proof, challenge: challenge))
        other = peer
        other.name = "Other client"
        XCTAssertFalse(credential.authenticates(peer: other, proof: proof, challenge: challenge))
        let paired = try credential.paired(role: .viewer)
        XCTAssertEqual(paired.serverID, credential.serverID)
        XCTAssertEqual(paired.secret, credential.secret)
        XCTAssertNotEqual(paired.clientSecret, credential.clientSecret)
        XCTAssertFalse(paired.authenticates(peer: peer, proof: proof, challenge: challenge))
    }
    func testCapturedHelloCannotBeReplayedOnAnotherConnection() throws {
        let credential = try RemoteCredential.generate()
        let peer = RemotePeer(id: UUID(), name: "Operator")
        let previous = try RemoteCredential.authenticationNonce()
        let current = try RemoteCredential.authenticationNonce()
        XCTAssertNotEqual(previous, current)
        let captured = credential.proof(for: peer, challenge: previous)
        XCTAssertTrue(credential.authenticates(peer: peer, proof: captured, challenge: previous))
        XCTAssertFalse(credential.authenticates(peer: peer, proof: captured, challenge: current))
        XCTAssertFalse(credential.authenticates(peer: peer, proof: captured, challenge: Data()))
        XCTAssertFalse(credential.authenticates(peer: peer, proof: "not a MAC", challenge: previous))
        XCTAssertEqual(credential.version, 1, "Wire upgrades must preserve securely stored pairing files")
        XCTAssertNotEqual(credential.version, RemoteProtocol.version)
    }
    func testListenerAdmissionBoundsWaitingSocketsBeforeActorDispatchAndReleasesPermits() {
        let gate = RemoteAdmissionGate(capacity: 16)
        var permits = (0..<16).compactMap { _ in gate.acquire() }
        XCTAssertEqual(permits.count, 16)
        for _ in 0..<10_000 { XCTAssertNil(gate.acquire(), "A stalled actor must not accumulate incoming sockets") }
        permits.removeLast()
        var replacement = gate.acquire()
        XCTAssertNotNil(replacement)
        XCTAssertNil(gate.acquire())
        gate.close()
        permits.removeAll()
        replacement = nil
        XCTAssertNil(gate.acquire(), "A stopped listener must not admit queued callbacks")
        let nextListener = RemoteAdmissionGate(capacity: 1)
        XCTAssertNotNil(nextListener.acquire(), "Listener restarts own independent capacity")
    }
    func testStopAndReleaseOperationsAlwaysUseInterruptLane() {
        for operation in [
            "transport.stop", "transport.pause", "transport.finishRecording", "designer.noteOn", "designer.noteOff",
            "designer.allNotesOff", "take.stop", "drums.stop", "audition.stop",
        ] {
            XCTAssertTrue(RemoteCommand.editor(.init(operation: operation)).isInterrupt, operation)
        }
        XCTAssertTrue(RemoteCommand.stop.isInterrupt)
        XCTAssertFalse(RemoteCommand.editor(.init(operation: "effects.apply")).isInterrupt)
    }
    func testProtocolRoundTripPreservesExplicitTargetsAndAuthoredRequestID() throws {
        let context = RemoteEditorContext(
            trackID: UUID(), clipID: UUID(), clipIDs: [UUID()], cursorFrame: 96000,
            rangeStart: 48000, rangeEnd: 96000, effectID: UUID(), automationTarget: "pan")
        let request = RemoteRequest(
            clientID: UUID(), expectedRevision: 4, expectedTrackRevisions: [context.trackID!: 3],
            command: .editor(
                .init(
                    operation: "designer.commit", context: context, numbers: ["gain": 0.7], payload: Data("preset".utf8)
                )))
        let message = RemoteWireMessage.request(request)
        let decoded = try JSONDecoder().decode(RemoteWireMessage.self, from: JSONEncoder().encode(message))
        guard case .request(let result) = decoded else { return XCTFail("Wrong message") }
        XCTAssertEqual(result, request)
    }
    func testDeltaPreservesIndependentLocalViewAndUpdatesOnlyChangedTrack() {
        var session = Session()
        let original = session.tracks[0]
        let added = Track(name: "Second", color: 1)
        session.tracks.append(added)
        var snapshot = RemoteSnapshot(serverID: UUID(), serverName: "Host", session: session, revision: 1)
        var edited = added
        edited.pan = 0.5
        snapshot.apply(.init(revision: 2, tracks: [edited], trackRevisions: [edited.id: 2]))
        XCTAssertEqual(snapshot.session.tracks[0], original)
        XCTAssertEqual(snapshot.session.tracks[1].pan, 0.5)
        XCTAssertEqual(snapshot.trackRevisions[edited.id], 2)
        snapshot.apply(.init(revision: 1, removedTrackIDs: [original.id]))
        XCTAssertEqual(snapshot.session.tracks.count, 2, "Outdated events must not roll back state")
        snapshot.apply(.init(revision: 3, removedTrackIDs: [original.id], trackOrder: [added.id]))
        XCTAssertEqual(snapshot.session.tracks.map(\.id), [added.id])
    }
}
