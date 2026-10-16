import XCTest
import SoundroomCore
@testable import SoundroomRemote

final class RemoteWebDocumentStateTests: XCTestCase {
    func testInitialSnapshotBarrierOrdersConcurrentEditsAndCoalescesTelemetry() throws {
        var buffer = RemoteInitialEventBuffer(), messages: [RemoteWireMessage] = []
        let initial = RemoteSnapshot(serverID: UUID(), serverName: "Host", session: Session(), revision: 5)
        try buffer.submit(.delta(.init(revision: 6, name: "First edit")), telemetryKey: nil) { message, _ in messages.append(message) }
        try buffer.submit(.transport(.init(frame: 1)), telemetryKey: "transport") { message, _ in messages.append(message) }
        try buffer.submit(.transport(.init(frame: 2)), telemetryKey: "transport") { message, _ in messages.append(message) }
        try buffer.submit(.delta(.init(revision: 7, name: "Second edit")), telemetryKey: nil) { message, _ in messages.append(message) }
        XCTAssertTrue(messages.isEmpty)
        buffer.install(initial) { message, _ in messages.append(message) }
        XCTAssertEqual(messages.count, 4)
        var state = RemoteWebDocumentState()
        for message in messages { _ = try state.encode(message) }
        XCTAssertEqual(state.snapshot?.revision, 7)
        XCTAssertEqual(state.snapshot?.session.name, "Second edit")
        XCTAssertEqual(state.snapshot?.transport.frame, 2)
        try buffer.submit(.status("Ready"), telemetryKey: "status") { message, _ in messages.append(message) }
        XCTAssertEqual(messages.count, 5)

        var full = RemoteInitialEventBuffer()
        for _ in 0..<4 { try full.submit(.delta(.init(revision: 6, tracks: initial.session.tracks)), telemetryKey: nil) { _, _ in XCTFail() } }
        XCTAssertThrowsError(try full.submit(.delta(.init(revision: 7, tracks: initial.session.tracks)), telemetryKey: nil) { _, _ in XCTFail() })
    }
    func testOversizedTrackEditBecomesExactPagedReplacementWithLatestTelemetry() throws {
        let initial = RemoteSnapshot(serverID: UUID(), serverName: "Host", session: Session(), revision: 7)
        var state = RemoteWebDocumentState()
        _ = try state.encode(.event(.snapshot(initial)))
        let transport = RemoteTransport(frame: 1234, isPlaying: true)
        _ = try state.encode(.event(.transport(transport)))
        _ = try state.encode(.event(.status("Playing")))
        let peer = RemotePeer(id: UUID(), name: "Editor")
        _ = try state.encode(.event(.presence([peer])))
        var track = initial.session.tracks[0], pattern = SoundDesignPattern()
        pattern.notes = Array(repeating: SoundDesignNote(pitch: 60, startBeat: 0), count: 100_000)
        var clip = Clip(assetID: UUID(), name: "Large MIDI", start: 0, length: 48_000)
        clip.soundDesign = pattern; track.clips = [clip]
        let delta = RemoteSessionDelta(revision: 8, tracks: [track], trackRevisions: [track.id: 8])
        XCTAssertThrowsError(try RemoteV3Codec.encodeMessage(.event(.delta(delta)))) {
            XCTAssertEqual($0 as? RemoteError, .oversizedFrame)
        }
        guard case .snapshot(let replacement) = try state.encode(.event(.delta(delta))) else {
            return XCTFail("An oversized entity update must be paged")
        }
        var expected = initial; expected.apply(delta)
        expected.transport = transport; expected.status = "Playing"; expected.peers = [peer]
        var stream = try RemoteWebSnapshotStream(snapshot: replacement), assembler = RemoteSnapshotAssembler()
        var installed: RemoteSnapshot?, pages = 0
        while let bytes = try stream.next() {
            XCTAssertLessThan(bytes.count, RemoteProtocol.maximumFrameBytes)
            if let snapshot = try assembler.accept(JSONDecoder().decode(RemoteSnapshotPage.self, from: bytes)) { installed = snapshot }
            pages += 1
        }
        XCTAssertGreaterThan(pages, 3); XCTAssertEqual(installed, expected)

        var controls = RemoteTrackControls(track: track); controls.gain = 0.25
        let smallDelta = RemoteSessionDelta(revision: 9, controls: [controls], trackRevisions: [track.id: 9])
        guard case .frame(let bytes) = try state.encode(.event(.delta(smallDelta))) else {
            return XCTFail("Mixer changes must stay incremental")
        }
        XCTAssertLessThan(bytes.count, 8192)
        guard case .event(let decoded) = try RemoteV3Codec.decodeMessage(bytes) else { return XCTFail() }
        XCTAssertEqual(decoded, .delta(smallDelta))
        XCTAssertEqual(state.snapshot?.session.tracks[0].clips[0].soundDesign?.notes.count, 100_000)
        XCTAssertEqual(state.snapshot?.session.tracks[0].gain, 0.25)
        guard case .frame(let heartbeat) = try state.encode(.heartbeat(UUID())) else { return XCTFail() }
        XCTAssertLessThan(heartbeat.count, 256)
    }

    func testMissingBaseAndRevisionGapCannotFabricateReplacementDocument() throws {
        var state = RemoteWebDocumentState()
        XCTAssertThrowsError(try state.encode(.event(.delta(.init(revision: 1)))))
        let initial = RemoteSnapshot(serverID: UUID(), serverName: "Host", session: Session(), revision: 5)
        _ = try state.encode(.event(.snapshot(initial)))
        XCTAssertThrowsError(try state.encode(.event(.delta(.init(revision: 7, name: "Missing change")))))
        XCTAssertEqual(state.snapshot, initial)
    }

    func testLegacyCodecPreservesSmallSnapshotsAndRejectsLargeDocumentsExplicitly() throws {
        let initial = RemoteSnapshot(serverID: UUID(), serverName: "Host", session: Session())
        for message in [RemoteWireMessage.event(.snapshot(initial)), .event(.delta(.init(revision: 1, name: "Renamed")))] {
            guard case .event(let decoded) = try JSONDecoder().decode(RemoteWireMessage.self, from: RemoteLegacyMessageCodec.encode(message)),
                  case .event(let expected) = message else { return XCTFail() }
            XCTAssertEqual(decoded, expected)
        }
        var session = initial.session, pattern = SoundDesignPattern()
        pattern.notes = Array(repeating: SoundDesignNote(pitch: 60, startBeat: 0), count: 100_000)
        var clip = Clip(assetID: UUID(), name: "Large MIDI", start: 0, length: 48_000)
        clip.soundDesign = pattern; session.tracks[0].clips = [clip]
        let large = RemoteSnapshot(serverID: initial.serverID, serverName: "Host", session: session)
        for message in [RemoteWireMessage.event(.snapshot(large)), .event(.delta(.init(revision: 1, tracks: session.tracks)))] {
            let bytes = try RemoteLegacyMessageCodec.encode(message)
            XCTAssertLessThan(bytes.count, 1024)
            guard case .failure(let reason) = try JSONDecoder().decode(RemoteWireMessage.self, from: bytes) else { return XCTFail() }
            XCTAssertTrue(reason.contains("common WSS"))
        }
    }
}
