import SoundroomCore
import SoundroomRemote
import XCTest

@testable import Soundroom

final class MasteringReviewSourceTests: XCTestCase {
    @MainActor private func settle() async { for _ in 0..<50 { await Task.yield() } }
    @MainActor private func fixture(sender: @escaping RemoteGUIController.CommandSender) throws -> (
        SessionStore, RemoteGUIController, Session, UUID
    ) {
        let store = SessionStore(headless: true)
        let serverID = UUID()
        var document = Session()
        document.tracks[0].clips = [Clip(assetID: UUID(), name: "Source", start: 0, length: 48_000)]
        document.tracks.append(Track(name: "Second", color: 1))
        var report = try MasteringAnalyzer.analyze(session: Session(), assets: [:])
        report.integratedLUFS = -18
        report.truePeakDB = -2
        report.crestDB = 12
        report.bassFraction = 0.2
        report.airFraction = 0.1
        report.analyzedFrames = 48_000
        report.sourceFingerprint = MasteringAnalysis.fingerprint(document)
        document.masteringAnalysis = report
        let remote = RemoteGUIController(
            store: store, credential: .native(try RemoteCredential.generate()), host: "fixture", commandSender: sender)
        store.remoteGUI = remote
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .operator))
        remote.receive(
            .snapshot(
                .init(
                    serverID: serverID, serverName: "Fixture", session: document, revision: 3,
                    trackRevisions: Dictionary(
                        uniqueKeysWithValues: (document.tracks + [document.masterTrack]).map { ($0.id, 3) }))))
        return (store, remote, document, serverID)
    }
    @MainActor func testForeignMasterEditRetainsPrivateChoicesAndRequiresExplicitReload() async throws {
        var sends = 0
        let (store, remote, document, _) = try fixture { _, _, _, _ in
            sends += 1
            return .init(requestID: UUID(), revision: 4)
        }
        let review = MasteringReviewController(owner: store)
        review.reload()
        await review.waitForSourceVerification()
        XCTAssertTrue(review.ready)
        review.setIncluded(2, true)
        let draft = review.proposed
        var master = document.masterTrack
        master.realtimeEffects = [.init(kind: .gain)]
        remote.receive(.delta(.init(revision: 4, tracks: [master], trackRevisions: [Session.masterTrackID: 4])))
        review.synchronize()
        review.apply()
        await settle()
        XCTAssertEqual(sends, 0)
        XCTAssertFalse(review.ready)
        XCTAssertTrue(review.sourceChanged)
        XCTAssertEqual(review.proposed, draft)
        review.reload()
        await review.waitForSourceVerification()
        XCTAssertTrue(review.ready)
        remote.disconnect()
        review.close()
    }
    @MainActor func testApplyFencesAllSourceTracksAndWaitsForAuthoredMasterMetadata() async throws {
        var captured: [UUID: Int] = [:]
        var sessionFence: Int?
        let (store, remote, document, _) = try fixture { _, versions, expectedRevision, _ in
            captured = versions
            sessionFence = expectedRevision
            return .init(requestID: UUID(), revision: 4)
        }
        let review = MasteringReviewController(owner: store)
        review.reload()
        await review.waitForSourceVerification()
        review.apply()
        await settle()
        XCTAssertEqual(Set(captured.keys), Set((document.tracks + [document.masterTrack]).map(\.id)))
        XCTAssertEqual(Set(captured.values), [3])
        XCTAssertEqual(sessionFence, 3)
        XCTAssertTrue(review.working)
        XCTAssertNil(review.completedApplyID)
        var master = document.masterTrack
        master.realtimeEffects = review.proposed
        remote.receive(.delta(.init(revision: 4, tracks: [master], trackRevisions: [master.id: 4])))
        review.synchronize()
        await settle()
        XCTAssertFalse(review.working)
        XCTAssertNotNil(review.completedApplyID)
        remote.disconnect()
        review.close()
    }
    @MainActor func testLaterForeignMetadataCannotConfirmApplyOrReplaceThePrivateProposal() async throws {
        let (store, remote, document, _) = try fixture { _, _, _, _ in .init(requestID: UUID(), revision: 4) }
        let review = MasteringReviewController(owner: store)
        review.reload()
        await review.waitForSourceVerification()
        let draft = review.proposed
        review.apply()
        await settle()
        var master = document.masterTrack
        master.realtimeEffects = [.init(kind: .gain, parameters: ["gain": -6])]
        remote.receive(.delta(.init(revision: 5, tracks: [master], trackRevisions: [master.id: 5])))
        await settle()
        XCTAssertFalse(review.working)
        XCTAssertNil(review.completedApplyID)
        XCTAssertEqual(review.proposed, draft)
        XCTAssertTrue(review.message.contains("changed"))
        remote.disconnect()
        review.close()
    }
    @MainActor func testReconnectWithEqualRevisionsDoesNotAuthorizeTheOldProposal() async throws {
        var sends = 0
        let (store, remote, document, serverID) = try fixture { _, _, _, _ in
            sends += 1
            return .init(requestID: UUID(), revision: 4)
        }
        let review = MasteringReviewController(owner: store)
        review.reload()
        await review.waitForSourceVerification()
        let draft = review.proposed
        remote.receiveState(.disconnected)
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .operator))
        remote.receive(
            .snapshot(
                .init(
                    serverID: serverID, serverName: "Fixture", session: document, revision: 3,
                    trackRevisions: Dictionary(
                        uniqueKeysWithValues: (document.tracks + [document.masterTrack]).map { ($0.id, 3) }))))
        review.apply()
        await settle()
        XCTAssertEqual(sends, 0)
        XCTAssertFalse(review.ready)
        XCTAssertEqual(review.proposed, draft)
        remote.disconnect()
        review.close()
    }
    @MainActor func testRejectedAnalysisDoesNotRebindOrRegenerateAnExistingPrivateReview() async throws {
        let (store, remote, _, _) = try fixture { _, _, _, _ in
            .init(requestID: UUID(), status: .rejected, revision: 3, message: "Analysis rejected")
        }
        let review = MasteringReviewController(owner: store)
        review.reload()
        await review.waitForSourceVerification()
        review.setIncluded(2, true)
        let draft = review.proposed
        let report = review.report
        review.analyze()
        await settle()
        XCTAssertFalse(review.working)
        XCTAssertEqual(review.message, "Analysis rejected")
        XCTAssertEqual(review.proposed, draft)
        XCTAssertEqual(review.report, report)
        remote.disconnect()
        review.close()
    }
    @MainActor func testLateAnalysisAfterCloseCannotReplaceAReopenedReview() async throws {
        var held: CheckedContinuation<RemoteReply, Error>?
        let (store, remote, _, _) = try fixture { _, _, _, _ in try await withCheckedThrowingContinuation { held = $0 }
        }
        let review = MasteringReviewController(owner: store)
        review.reload()
        await review.waitForSourceVerification()
        review.analyze()
        await settle()
        review.close()
        review.reload()
        await review.waitForSourceVerification()
        review.setIncluded(2, true)
        let current = review.proposed
        held?.resume(returning: .init(requestID: UUID(), status: .rejected, revision: 3, message: "Old failure"))
        held = nil
        await settle()
        XCTAssertEqual(review.proposed, current)
        XCTAssertNotEqual(review.message, "Old failure")
        remote.disconnect()
        review.close()
    }
    @MainActor func testSourceVerificationRunsOffMainAndSliderChangesDoNotRehashTheArrangement() async throws {
        let (store, remote, _, _) = try fixture { _, _, _, _ in .init(requestID: UUID(), revision: 4) }
        let probe = VerificationProbe()
        let review = MasteringReviewController(
            owner: store,
            verify: { report, document in
                probe.record()
                return report.matches(document)
            })
        review.reload()
        await review.waitForSourceVerification()
        review.setIncluded(2, true)
        for i in 0..<100 {
            review.intensity = Double(i) / 100
            review.target = i.isMultiple(of: 2) ? -14 : -16
            review.synchronize()
        }
        XCTAssertEqual(probe.calls, 1)
        XCTAssertFalse(probe.ranOnMain)
        XCTAssertTrue(review.proposed[2].enabled, "Loudness/amount changes retain manual include choices")
        remote.disconnect()
        review.close()
    }
}

private final class VerificationProbe: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0, main = false
    func record() {
        lock.lock()
        defer { lock.unlock() }
        count += 1
        main = main || Thread.isMainThread
    }
    var calls: Int {
        lock.lock()
        defer { lock.unlock() }
        return count
    }
    var ranOnMain: Bool {
        lock.lock()
        defer { lock.unlock() }
        return main
    }
}
