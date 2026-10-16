import XCTest
import SoundroomCore
import SoundroomRemote
@testable import Soundroom

/// Native remote Pitch requests are bound to their opening track, insert, range,
/// authored version and connection. Results never replace newer private targets.
final class PitchEditorSourceTests: XCTestCase {
    @MainActor private func settle() async { for _ in 0..<50 { await Task.yield() } }
    /// Waits for an observable outcome so a loaded runner cannot pass a negative check vacuously.
    @MainActor private func settle(until done: () -> Bool) async {
        let deadline = Date().addingTimeInterval(5)
        while !done(), Date() < deadline { try? await Task.sleep(for: .milliseconds(2)) }
        XCTAssertTrue(done(), "outcome not observed")
    }
    private let scope = 0..<48_000
    private func region(_ note: Float) -> PitchEditRegion {
        PitchEditRegion(startFrame: 0, endFrame: 48_000, points: [.init(frame: 0, note: note), .init(frame: 47_999, note: note)])
    }
    private func analysis(_ note: Float, draft: Bool = true) throws -> Data {
        try JSONEncoder().encode(RemotePitchAnalysis(points: [.init(frame: 0, frequency: 440, confidence: 0.9)],
                                                      regions: [region(note)], targetsAreDraft: draft))
    }
    @MainActor private func fixture(sender: @escaping RemoteGUIController.CommandSender) throws
        -> (SessionStore, RemoteGUIController, PitchEditorController, UUID, UUID, UUID) {
        let store = SessionStore(headless: true), serverID = UUID()
        var document = Session()
        document.tracks[0].clips = [Clip(assetID: UUID(), name: "Vocal", start: 0, length: 48_000)]
        var pitch = RealtimeEffect(kind: .pitchCorrection); pitch.pitchEdits = [region(60)]
        document.tracks[0].realtimeEffects = [pitch]
        let remote = RemoteGUIController(store: store, credential: .native(try RemoteCredential.generate()), host: "fixture", commandSender: sender)
        store.remoteGUI = remote
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .operator))
        remote.receive(.snapshot(.init(serverID: serverID, serverName: "Fixture", session: document, revision: 3,
            trackRevisions: Dictionary(uniqueKeysWithValues: (document.tracks + [document.masterTrack]).map { ($0.id, 3) }))))
        let editor = PitchEditorController(owner: store)
        editor.synchronize(track: store.session.tracks[0], effectID: pitch.id, range: scope, assets: [:])
        return (store, remote, editor, document.tracks[0].id, pitch.id, serverID)
    }
    @MainActor private func sync(_ editor: PitchEditorController, _ store: SessionStore, _ effectID: UUID) {
        editor.synchronize(track: store.session.tracks.first, effectID: effectID, range: scope, assets: [:])
    }

    @MainActor func testAnalysisFencesItsTargetVersionAndAdoptsOnlyItsOwnSource() async throws {
        var captured: [UUID: Int] = [:]
        let payload = try analysis(64)
        let (store, remote, editor, trackID, _, _) = try fixture { command, versions, _, _ in
            if case .editor(let intent) = command, intent.operation == "pitch.analyze" { captured = versions }
            return .init(requestID: UUID(), revision: 3, payload: payload)
        }
        editor.analyze(); await settle { editor.targetsAreDraft }
        XCTAssertEqual(captured, [trackID: 3])
        XCTAssertEqual(editor.visibleRegions.first?.points.first?.note, 64)
        XCTAssertTrue(editor.targetsAreDraft); XCTAssertNil(editor.error)
        remote.disconnect(); withExtendedLifetime(store) {}
    }

    @MainActor func testRejectedAnalysisKeepsTheBindingAndPrivateDraftTargets() async throws {
        var reject = false
        let payload = try analysis(64)
        let (store, remote, editor, _, effectID, _) = try fixture { _, _, _, _ in
            reject ? .init(requestID: UUID(), status: .rejected, revision: 3, message: "Busy") : .init(requestID: UUID(), revision: 3, payload: payload)
        }
        editor.analyze(); await settle { editor.targetsAreDraft }
        let draft = editor.visibleRegions
        reject = true
        editor.analyze(); await settle { editor.error != nil }
        XCTAssertEqual(editor.visibleRegions, draft); XCTAssertTrue(editor.targetsAreDraft)
        XCTAssertEqual(editor.effectID, effectID); XCTAssertEqual(editor.error, "Busy")
        remote.disconnect(); withExtendedLifetime(store) {}
    }

    @MainActor func testForeignMetadataWinningTheReplyRaceCannotInstallTheResult() async throws {
        var foreign: (() -> Void)?
        let payload = try analysis(70)
        let (store, remote, editor, _, effectID, _) = try fixture { _, _, _, _ in
            foreign?()
            return .init(requestID: UUID(), revision: 3, payload: payload)
        }
        foreign = { [unowned store] in
            var track = store.session.tracks[0]; track.name = "Renamed elsewhere"
            remote.receive(.delta(.init(revision: 4, tracks: [track], trackRevisions: [track.id: 4])))
        }
        let before = editor.visibleRegions
        editor.analyze(); await settle { editor.error != nil }; sync(editor, store, effectID)
        XCTAssertEqual(editor.visibleRegions, before)
        XCTAssertTrue(editor.before.isEmpty, "a contour measured from a superseded source is not installed")
        // settle(until:) above already required the explanation before the editor resynchronized.
        remote.disconnect(); withExtendedLifetime(store) {}
    }

    @MainActor func testForeignTargetEditKeepsAnUnconfirmedPrivateDraft() async throws {
        let payload = try analysis(64)
        let (store, remote, editor, _, effectID, _) = try fixture { _, _, _, _ in .init(requestID: UUID(), revision: 3, payload: payload) }
        editor.analyze(); await settle { editor.targetsAreDraft }
        let draft = editor.visibleRegions
        var track = store.session.tracks[0]; track.realtimeEffects[0].pitchEdits = [region(52)]
        remote.receive(.delta(.init(revision: 4, tracks: [track], trackRevisions: [track.id: 4])))
        await settle(); sync(editor, store, effectID)
        XCTAssertEqual(editor.visibleRegions, draft); XCTAssertTrue(editor.targetsAreDraft)
        XCTAssertNotNil(editor.sourceNotice)
        remote.disconnect(); withExtendedLifetime(store) {}
    }

    @MainActor func testReconnectWithEqualRevisionsCannotConfirmAnEarlierAnalysis() async throws {
        var reconnect: (() -> Void)?
        let payload = try analysis(70, draft: false)
        let (store, remote, editor, _, effectID, serverID) = try fixture { _, _, _, _ in
            reconnect?()
            return .init(requestID: UUID(), revision: 3, payload: payload)
        }
        reconnect = { [unowned store] in
            remote.receiveState(.disconnected)
            remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .operator))
            remote.receive(.snapshot(.init(serverID: serverID, serverName: "Fixture", session: store.session, revision: 3,
                trackRevisions: Dictionary(uniqueKeysWithValues: (store.session.tracks + [store.session.masterTrack]).map { ($0.id, 3) }))))
        }
        let before = editor.visibleRegions
        editor.analyze(); await settle { editor.error != nil }; sync(editor, store, effectID)
        XCTAssertEqual(editor.visibleRegions, before)
        XCTAssertTrue(editor.before.isEmpty)
        remote.disconnect(); withExtendedLifetime(store) {}
    }

    @MainActor func testDeletedSourceSendsNothing() async throws {
        var sends = 0
        let (store, remote, editor, trackID, _, _) = try fixture { _, _, _, _ in sends += 1; return .init(requestID: UUID(), revision: 4) }
        remote.receive(.delta(.init(revision: 4, tracks: [], removedTrackIDs: [trackID], trackRevisions: [:])))
        await settle()
        XCTAssertNil(store.session.tracks.first { $0.id == trackID })
        editor.analyze(); editor.bake(); await settle()
        XCTAssertEqual(sends, 0)
        remote.disconnect(); withExtendedLifetime(store) {}
    }

    @MainActor func testApplyFencesReviewedTargetsAndLateResultCannotCloseAReopenedEditor() async throws {
        var captured: [UUID: Int] = [:], hold: CheckedContinuation<Void, Never>?
        let (store, remote, editor, trackID, effectID, _) = try fixture { command, versions, _, _ in
            if case .editor(let intent) = command, intent.operation == "pitch.apply" {
                captured = versions
                await withCheckedContinuation { hold = $0 }
            }
            return .init(requestID: UUID(), revision: 4)
        }
        func observeOwnCommit() {
            var track = store.session.tracks[0]; track.realtimeEffects = []
            remote.receive(.delta(.init(revision: 4, tracks: [track], trackRevisions: [trackID: 4])))
        }
        XCTAssertNotNil(editor.bakePlan)
        editor.presented(); editor.bake(); await settle { hold != nil }
        XCTAssertEqual(captured, [trackID: 3])
        editor.presented() // the sheet closed and reopened while Apply ran
        observeOwnCommit(); hold?.resume(); await settle { !remote.hasPendingEdits(for: trackID) }
        XCTAssertNil(editor.completedBakeID, "a late Apply cannot dismiss the reopened editor")
        _ = effectID
        remote.disconnect(); withExtendedLifetime(store) {}
    }

    @MainActor func testApplyCompletesOnlyWhenItsOwnAuthoredRevisionIsObserved() async throws {
        for observed in [4, 5] { // own commit, or a newer foreign version racing the receipt
            var context: (SessionStore, RemoteGUIController, UUID)?
            let (store, remote, editor, trackID, _, _) = try fixture { command, _, _, _ in
                if case .editor(let intent) = command, intent.operation == "pitch.apply", let (store, remote, trackID) = context {
                    var track = store.session.tracks[0]; track.realtimeEffects = []
                    remote.receive(.delta(.init(revision: observed, tracks: [track], trackRevisions: [trackID: observed])))
                }
                return .init(requestID: UUID(), revision: 4)
            }
            context = (store, remote, trackID)
            editor.presented(); editor.bake(); await settle { editor.completedBakeID != nil || editor.error != nil }
            if observed == 4 { XCTAssertNotNil(editor.completedBakeID) }
            else { XCTAssertNil(editor.completedBakeID); XCTAssertTrue(editor.error?.contains("changed") == true) }
            remote.disconnect()
        }
    }
}
