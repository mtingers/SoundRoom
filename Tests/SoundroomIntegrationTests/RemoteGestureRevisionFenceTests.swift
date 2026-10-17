import SoundroomRemote
import XCTest

@testable import Soundroom

final class RemoteGestureRevisionFenceTests: XCTestCase {
    @MainActor func testReceiptBeforeMetadataWaitsWithoutAdoptingANewerVersion() {
        let id = UUID()
        let fence = RemoteGestureRevisionFence(versions: [:])
        let reply = RemoteReply(requestID: UUID(), revision: 1)
        XCTAssertEqual(
            fence.resolve(reply: reply, targets: [id], observedRevision: 0, observed: [:], verifiedUnchanged: []),
            .waiting)
        XCTAssertNil(fence.versions[id])
        XCTAssertEqual(
            fence.resolve(reply: reply, targets: [id], observedRevision: 1, observed: [id: 1], verifiedUnchanged: []),
            .ready)
        XCTAssertEqual(fence.versions[id], 1)
    }
    @MainActor func testMatchingOwnMetadataAdvancesOnlyItsTarget() {
        let a = UUID()
        let b = UUID()
        let fence = RemoteGestureRevisionFence(versions: [:])
        let reply = RemoteReply(requestID: UUID(), revision: 5)
        XCTAssertEqual(
            fence.resolve(
                reply: reply, targets: [a], observedRevision: 5, observed: [a: 5, b: 5], verifiedUnchanged: []), .ready)
        XCTAssertEqual(fence.versions, [a: 5])
        XCTAssertTrue(fence.isCurrent(targets: [a], observed: [a: 5, b: 6]))
        XCTAssertFalse(fence.isCurrent(targets: [a], observed: [a: 6]))
    }
    @MainActor func testForeignMetadataCannotBlessAnOldGestureEvenWithEqualValues() {
        let id = UUID()
        let fence = RemoteGestureRevisionFence(versions: [id: 3])
        let reply = RemoteReply(requestID: UUID(), revision: 4)
        XCTAssertEqual(
            fence.resolve(reply: reply, targets: [id], observedRevision: 5, observed: [id: 5], verifiedUnchanged: [id]),
            .conflict)
        XCTAssertTrue(fence.invalidated)
        XCTAssertFalse(fence.isCurrent(targets: [id], observed: fence.versions))
        XCTAssertEqual(
            fence.resolve(reply: reply, targets: [id], observedRevision: 4, observed: [id: 4], verifiedUnchanged: []),
            .conflict)
    }
    @MainActor func testVerifiedNoOpKeepsOriginalTargetVersionAfterUnrelatedEdit() {
        let id = UUID()
        let fence = RemoteGestureRevisionFence(versions: [UUID(): 1])
        let known = RemoteGestureRevisionFence(versions: [id: 3])
        let reply = RemoteReply(requestID: UUID(), revision: 9)
        XCTAssertEqual(
            known.resolve(reply: reply, targets: [id], observedRevision: 9, observed: [id: 3], verifiedUnchanged: [id]),
            .ready)
        XCTAssertEqual(known.versions[id], 3)
        XCTAssertEqual(
            fence.resolve(reply: reply, targets: [id], observedRevision: 9, observed: [id: 3], verifiedUnchanged: [id]),
            .conflict)
    }
    @MainActor func testUnverifiedNoOpAndRejectedReceiptInvalidate() {
        let id = UUID()
        let fence = RemoteGestureRevisionFence(versions: [id: 3])
        XCTAssertEqual(
            fence.resolve(
                reply: .init(requestID: UUID(), revision: 9), targets: [id], observedRevision: 9, observed: [id: 3],
                verifiedUnchanged: []), .conflict)
        let rejected = RemoteGestureRevisionFence(versions: [id: 3])
        XCTAssertEqual(
            rejected.resolve(
                reply: .init(requestID: UUID(), status: .rejected, revision: 3), targets: [id], observedRevision: 3,
                observed: [id: 3], verifiedUnchanged: [id]), .conflict)
    }
    @MainActor func testMultiTrackResolutionNeverPartiallyAdvancesAfterConflict() {
        let a = UUID()
        let b = UUID()
        let fence = RemoteGestureRevisionFence(versions: [:])
        XCTAssertEqual(
            fence.resolve(
                reply: .init(requestID: UUID(), revision: 4), targets: [a, b], observedRevision: 5,
                observed: [a: 4, b: 5], verifiedUnchanged: []), .conflict)
        XCTAssertTrue(fence.versions.isEmpty)
    }
    @MainActor func testDisconnectInvalidationSurvivesResetRevisionCounters() {
        let id = UUID()
        let fence = RemoteGestureRevisionFence(versions: [id: 0])
        fence.invalidate()
        XCTAssertEqual(
            fence.resolve(
                reply: .init(requestID: UUID(), revision: 0), targets: [id], observedRevision: 0, observed: [id: 0],
                verifiedUnchanged: [id]), .conflict)
    }
    @MainActor func testOwnRemovalClearsVersionOnlyAtItsConfirmedRevision() {
        let id = UUID()
        let fence = RemoteGestureRevisionFence(versions: [id: 3])
        XCTAssertEqual(
            fence.resolve(
                reply: .init(requestID: UUID(), revision: 4), targets: [id], observedRevision: 4, observed: [:],
                verifiedUnchanged: [], removedTargets: [id]), .ready)
        XCTAssertNil(fence.versions[id])
        let uncertain = RemoteGestureRevisionFence(versions: [id: 3])
        XCTAssertEqual(
            uncertain.resolve(
                reply: .init(requestID: UUID(), revision: 4), targets: [id], observedRevision: 5, observed: [:],
                verifiedUnchanged: [], removedTargets: [id]), .conflict)
    }
}
