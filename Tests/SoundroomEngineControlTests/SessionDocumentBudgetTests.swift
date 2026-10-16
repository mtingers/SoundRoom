import XCTest
import SoundroomCore
import SoundroomRemote
@testable import SoundroomEngineControl

final class SessionDocumentBudgetTests: XCTestCase {
    func testLargeMetadataUsesPagedBudgetWithoutRelaxingLegacyBound() async throws {
        var session = Session(), pattern = SoundDesignPattern()
        pattern.notes = Array(repeating: SoundDesignNote(pitch: 60, startBeat: 0), count: 100_000)
        var clip = Clip(assetID: UUID(), name: "Large MIDI", start: 0, length: 48_000)
        clip.soundDesign = pattern; session.tracks[0].clips = [clip]
        let snapshot = RemoteSnapshot(serverID: UUID(), serverName: "Host", session: session)
        XCTAssertGreaterThan(try RemoteV3Codec.encodeSnapshotDocument(snapshot).count, RemoteProtocol.maximumFrameBytes)
        try await SessionDocumentBudget.validate(snapshot, transport: .pagedWeb)
        do {
            try await SessionDocumentBudget.validate(snapshot, transport: .legacyNative)
            XCTFail("The legacy adapter must keep its document bound")
        } catch { XCTAssertTrue(error.localizedDescription.contains("common WSS")) }
    }

    func testExactBoundAndCancellation() async throws {
        for transport in [SessionDocumentTransport.legacyNative, .pagedWeb] {
            try SessionDocumentBudget.validateEncodedByteCount(transport.maximumDocumentBytes, transport: transport)
            XCTAssertThrowsError(try SessionDocumentBudget.validateEncodedByteCount(transport.maximumDocumentBytes + 1, transport: transport))
        }
        let worker = Task {
            withUnsafeCurrentTask { $0?.cancel() }
            try await SessionDocumentBudget.validate(.init(serverID: UUID(), serverName: "Host", session: Session()), transport: .pagedWeb)
        }
        worker.cancel()
        do { try await worker.value; XCTFail("Cancelled work must not validate a commit") }
        catch { XCTAssertTrue(error is CancellationError) }
    }
}
