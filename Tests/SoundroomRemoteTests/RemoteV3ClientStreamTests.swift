import SoundroomCore
import XCTest

@testable import SoundroomRemote

final class RemoteV3ClientStreamTests: XCTestCase {
    func testBufferedStatusAndDeltaRespectArrivalOrderAndPinnedRevision() throws {
        for laterStatus in [false, true] {
            var stream = RemoteV3ClientStream()
            let initial = RemoteSnapshot(
                serverID: UUID(), serverName: "Host", session: Session(), revision: 5, status: "Pinned")
            let pages = try RemoteSnapshotPages(snapshot: initial)
            _ = try stream.accept(JSONEncoder().encode(pages.begin()))
            _ = try stream.accept(RemoteV3Codec.encodeMessage(.event(.status("Earlier live status"))))
            _ = try stream.accept(
                RemoteV3Codec.encodeMessage(.event(.delta(.init(revision: 6, status: "Later accepted edit")))))
            if laterStatus { _ = try stream.accept(RemoteV3Codec.encodeMessage(.event(.status("Latest live status")))) }
            // A stale delta arriving last must not mask a current live event.
            _ = try stream.accept(RemoteV3Codec.encodeMessage(.event(.delta(.init(revision: 4, status: "Stale edit")))))
            for index in 0..<pages.pageCount { _ = try stream.accept(JSONEncoder().encode(pages.page(at: index))) }
            let messages = try stream.accept(JSONEncoder().encode(pages.end()))
            var installed: RemoteSnapshot?
            for message in messages {
                switch message {
                case .event(.snapshot(let value)): installed = value
                case .event(.delta(let delta)): installed?.apply(delta)
                case .event(.status(let status)): installed?.status = status
                default: break
                }
            }
            XCTAssertEqual(installed?.revision, 6)
            XCTAssertEqual(installed?.status, laterStatus ? "Latest live status" : "Later accepted edit")
            XCTAssertEqual(stream.retainedBytes, 0)
        }
    }
    func testNegotiatedCapabilitiesRejectUnsupportedOperationsWithoutEncodingAudioEdits() {
        var capability = RemoteCapabilities()
        capability.commands = ["snapshot", "editor"]
        capability.editorOperations = ["master.proposal"]
        XCTAssertTrue(capability.supports(.snapshot))
        XCTAssertTrue(capability.supports(.editor(.init(operation: "master.proposal"))))
        XCTAssertFalse(capability.supports(.record))
        XCTAssertFalse(capability.supports(.editor(.init(operation: "pitch.targets"))))
    }
    func testVerifiedSnapshotReplaysNewerDeltaAndLatestTransportWithoutLosingReply() throws {
        var stream = RemoteV3ClientStream()
        let snapshot = RemoteSnapshot(serverID: UUID(), serverName: "Host", session: Session(), revision: 7)
        let pages = try RemoteSnapshotPages(snapshot: snapshot)
        XCTAssertTrue(try stream.accept(JSONEncoder().encode(pages.begin())).isEmpty)
        for frame in [10, 20, 30] {
            let transport = RemoteTransport(frame: frame, isPlaying: true, isRecording: false)
            let messages = try stream.accept(RemoteV3Codec.encodeMessage(.event(.transport(transport))))
            guard case .event(.transport(let received)) = messages.first else {
                return XCTFail("Missing live transport")
            }
            XCTAssertEqual(received, transport)
        }
        let changed = RemoteSessionDelta(
            revision: 8, tracks: [], controls: [], removedTrackIDs: [], trackRevisions: [:], name: "After edit")
        XCTAssertTrue(try stream.accept(RemoteV3Codec.encodeMessage(.event(.delta(changed)))).isEmpty)
        let reply = RemoteReply(requestID: UUID(), revision: 8)
        let receipts = try stream.accept(RemoteV3Codec.encodeMessage(.reply(reply)))
        guard case .reply(let receivedReply) = receipts.first else { return XCTFail("Receipt stalled behind pages") }
        XCTAssertEqual(receivedReply, reply)
        for index in 0..<pages.pageCount {
            XCTAssertTrue(try stream.accept(JSONEncoder().encode(pages.page(at: index))).isEmpty)
        }
        let delivered = try stream.accept(JSONEncoder().encode(pages.end()))
        guard case .event(.snapshot(let receivedSnapshot)) = delivered.first,
            case .event(.delta(let receivedDelta)) = delivered.dropFirst().first
        else { return XCTFail("Snapshot/delta order lost") }
        XCTAssertEqual(receivedSnapshot, snapshot)
        XCTAssertEqual(receivedDelta, changed)
        guard case .event(.transport(let state)) = delivered.last else { return XCTFail("Missing latest transport") }
        XCTAssertEqual(state.frame, 30)
        XCTAssertEqual(stream.retainedBytes, 0)
    }
    func testSnapshotRejectsUnknownPageFieldsAndReplacementOfActiveTransfer() throws {
        var stream = RemoteV3ClientStream()
        let pages = try RemoteSnapshotPages(snapshot: .init(serverID: UUID(), serverName: "Host", session: Session()))
        let begin = try JSONEncoder().encode(pages.begin())
        var object = try XCTUnwrap(JSONSerialization.jsonObject(with: begin) as? [String: Any])
        object["surprise"] = true
        XCTAssertThrowsError(try stream.accept(JSONSerialization.data(withJSONObject: object)))
        _ = try stream.accept(begin)
        XCTAssertThrowsError(try stream.accept(begin))
    }
    func testCapabilitiesAndPreSnapshotDeltasAreValidatedAndBounded() throws {
        var stream = RemoteV3ClientStream()
        let capability = RemoteCapabilities(snapshotPagination: true)
        XCTAssertTrue(try stream.accept(RemoteV3Codec.encodeCapabilities(capability)).isEmpty)
        XCTAssertEqual(stream.capabilities, capability)
        for revision in 1...128 {
            let delta = RemoteSessionDelta(
                revision: revision, tracks: [], controls: [], removedTrackIDs: [], trackRevisions: [:])
            XCTAssertTrue(try stream.accept(RemoteV3Codec.encodeMessage(.event(.delta(delta)))).isEmpty)
        }
        let overflow = RemoteSessionDelta(
            revision: 129, tracks: [], controls: [], removedTrackIDs: [], trackRevisions: [:])
        XCTAssertThrowsError(try stream.accept(RemoteV3Codec.encodeMessage(.event(.delta(overflow)))))
        XCTAssertLessThan(stream.retainedBytes, RemoteProtocol.maximumQueuedBytes)
    }
    func testWebEndpointValidationRejectsPlaintextCredentialsQueriesAndInvalidOrigins() throws {
        let valid = URL(string: "wss://localhost:49255/")!
        let origin = URL(string: "https://localhost:49254")!
        XCTAssertNoThrow(try RemoteWebClientConnection.validate(url: valid, origin: origin))
        for value in [
            "ws://localhost/", "wss://user:secret@localhost/", "wss://localhost/?secret=123",
            "wss://localhost/#credential",
        ] {
            XCTAssertThrowsError(try RemoteWebClientConnection.validate(url: URL(string: value)!, origin: origin))
        }
        for value in ["http://localhost", "https://localhost/path", "https://localhost?secret=123"] {
            XCTAssertThrowsError(try RemoteWebClientConnection.validate(url: valid, origin: URL(string: value)!))
        }
    }
}
