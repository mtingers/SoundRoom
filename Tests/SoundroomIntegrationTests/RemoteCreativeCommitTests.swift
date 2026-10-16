import XCTest
import SoundroomCore
import SoundroomRemote
@testable import Soundroom

final class RemoteCreativeCommitTests: XCTestCase {
    @MainActor private func settle() async { for _ in 0..<50 { await Task.yield() } }
    @MainActor private func fixture(sender: @escaping RemoteGUIController.CommandSender) throws -> (SessionStore, RemoteGUIController, Session, UUID) {
        let store = SessionStore(headless: true), serverID = UUID()
        var session = Session(), sound = SoundDesignPattern(), drum = DrumPattern()
        sound.notes = [.init(pitch: 60, startBeat: 0)]
        drum.apply(.backbeat)
        var clip = Clip(assetID: UUID(), name: "Creative source", start: 100, length: 48_000)
        clip.soundDesign = sound; clip.drumPattern = drum
        session.tracks[0].clips = [clip]; session.tracks.append(Track(name: "Other track", color: 1))
        let remote = RemoteGUIController(store: store, credential: .native(try RemoteCredential.generate()), host: "fixture", commandSender: sender)
        store.remoteGUI = remote
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .operator))
        remote.receive(.snapshot(.init(serverID: serverID, serverName: "Fixture", session: session, revision: 3,
                                      trackRevisions: Dictionary(uniqueKeysWithValues: session.tracks.map { ($0.id, 3) }))))
        store.selectedTrack = session.tracks[0].id; store.selectedClip = clip.id; store.playhead = 450
        return (store, remote, session, serverID)
    }

    @MainActor func testRejectedDesignerCommitKeepsItsPrivateEditorOpen() async throws {
        let (store, remote, _, _) = try fixture { _, _, _, _ in .init(requestID: UUID(), status: .rejected, revision: 3, message: "Render rejected") }
        store.requestSoundDesigner(editClip: true)
        let request = try XCTUnwrap(store.soundDesignRequest)
        store.commitSoundDesign(request.pattern, request: request)
        XCTAssertEqual(store.soundDesignRequest?.id, request.id)
        XCTAssertTrue(store.soundDesignCommitPending)
        await settle()
        XCTAssertEqual(store.soundDesignRequest?.id, request.id)
        XCTAssertFalse(store.soundDesignCommitPending)
        XCTAssertEqual(store.soundDesignCommitMessage, "Render rejected")
        remote.disconnect()
    }

    @MainActor func testDesignerCommitUsesOpeningTargetAfterPrivateNavigationAndWaitsForMetadata() async throws {
        var captured: RemoteEditorIntent?, versions: [UUID: Int] = [:]
        let (store, remote, session, _) = try fixture { command, expected, expectedSession, _ in
            guard case .editor(let intent) = command else { throw RemoteError.invalidFrame }
            captured = intent; versions = expected; XCTAssertNil(expectedSession)
            return .init(requestID: UUID(), revision: 4)
        }
        store.requestSoundDesigner(editClip: true)
        let request = try XCTUnwrap(store.soundDesignRequest), trackID = session.tracks[0].id
        store.selectedTrack = session.tracks[1].id; store.selectedClip = nil; store.playhead = 9000
        store.commitSoundDesign(request.pattern, request: request)
        await settle()
        XCTAssertEqual(captured?.context.trackID, trackID)
        XCTAssertEqual(captured?.context.clipID, request.clipID)
        XCTAssertEqual(captured?.context.cursorFrame, 450)
        XCTAssertEqual(captured?.integers["frame"], 450)
        XCTAssertEqual(versions, [trackID: 3])
        XCTAssertNotNil(store.soundDesignRequest, "The receipt cannot close an editor before metadata arrives")
        remote.receive(.delta(.init(revision: 4, tracks: [session.tracks[0]], trackRevisions: [trackID: 4])))
        await settle()
        XCTAssertNil(store.soundDesignRequest)
        XCTAssertEqual(store.selectedTrack, session.tracks[1].id)
        XCTAssertEqual(store.playhead, 9000)
        remote.disconnect()
    }

    @MainActor func testForeignRevisionPreventsDesignerCommitWithoutAdoptingTheNewVersion() async throws {
        var sends = 0
        let (store, remote, session, _) = try fixture { _, _, _, _ in sends += 1; return .init(requestID: UUID(), revision: 4) }
        store.requestSoundDesigner(editClip: true)
        let request = try XCTUnwrap(store.soundDesignRequest), id = session.tracks[0].id
        remote.receive(.delta(.init(revision: 4, controlPatches: [.init(trackID: id, gain: 0.2)], trackRevisions: [id: 4])))
        store.commitSoundDesign(request.pattern, request: request)
        await settle()
        XCTAssertEqual(sends, 0)
        XCTAssertEqual(store.soundDesignRequest?.id, request.id)
        XCTAssertEqual(request.source?.fence.versions[id], 3)
        XCTAssertTrue(store.soundDesignCommitMessage.contains("draft is retained"))
        remote.disconnect()
    }

    @MainActor func testExplicitNewTrackAfterReconnectCarriesSessionFenceAndOriginalPosition() async throws {
        var captured: RemoteEditorIntent?, expectedSession: Int?, expectedTracks: [UUID: Int] = [:]
        let (store, remote, session, serverID) = try fixture { command, tracks, revision, _ in
            guard case .editor(let intent) = command else { throw RemoteError.invalidFrame }
            captured = intent; expectedSession = revision; expectedTracks = tracks
            return .init(requestID: UUID(), revision: 4)
        }
        store.requestSoundDesigner(editClip: true)
        let request = try XCTUnwrap(store.soundDesignRequest)
        remote.receiveState(.disconnected)
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .operator))
        remote.receive(.snapshot(.init(serverID: serverID, serverName: "Fixture", session: session, revision: 3, trackRevisions: Dictionary(uniqueKeysWithValues: session.tracks.map { ($0.id, 3) }))))
        store.commitSoundDesign(request.pattern, request: request)
        XCTAssertNil(captured, "A revision reset cannot authorize the old source")
        store.selectedTrack = session.tracks[1].id; store.playhead = 9000
        store.insertSoundDesignOnNewTrack(request.pattern, request: request)
        await settle()
        XCTAssertEqual(expectedSession, 3); XCTAssertTrue(expectedTracks.isEmpty)
        XCTAssertNil(captured?.context.trackID); XCTAssertNil(captured?.context.clipID)
        XCTAssertEqual(captured?.context.cursorFrame, 450)
        XCTAssertNil(captured?.strings["trackID"])
        remote.receive(.delta(.init(revision: 4)))
        await settle()
        XCTAssertNil(store.soundDesignRequest)
        remote.disconnect()
    }

    @MainActor func testCompletedDesignerRenderPreservesNewerDraftAndPinsNextReplacement() async throws {
        var held: CheckedContinuation<RemoteReply, Error>?
        var sends = 0, lastVersions: [UUID: Int] = [:]
        weak var target: RemoteGUIController?
        var renderedTrack: Track?
        let (store, remote, session, _) = try fixture { _, versions, _, _ in
            sends += 1; lastVersions = versions
            if sends == 1 { return try await withCheckedThrowingContinuation { held = $0 } }
            guard let track = renderedTrack else { throw RemoteError.invalidFrame }
            target?.receive(.delta(.init(revision: 5, tracks: [track], trackRevisions: [track.id: 5])))
            return .init(requestID: UUID(), revision: 5)
        }
        target = remote
        store.requestSoundDesigner(editClip: true)
        let request = try XCTUnwrap(store.soundDesignRequest), track = session.tracks[0]
        renderedTrack = track
        store.commitSoundDesign(request.pattern, request: request)
        await settle()
        store.soundDesignDraftVersion += 1
        remote.receive(.delta(.init(revision: 4, tracks: [track], trackRevisions: [track.id: 4])))
        let result = RemoteEditorContext(trackID: track.id, clipID: request.clipID, cursorFrame: 100)
        held?.resume(returning: .init(requestID: UUID(), revision: 4, payload: try JSONEncoder().encode(result))); held = nil
        await settle()
        let current = try XCTUnwrap(store.soundDesignRequest)
        XCTAssertEqual(current.id, request.id)
        XCTAssertTrue(store.soundDesignCommitMessage.contains("newer private draft"))
        XCTAssertEqual(current.source?.fence.versions[track.id], 4)
        var newer = request.pattern; newer.notes[0].pitch = 62
        store.commitSoundDesign(newer, request: current)
        await settle()
        XCTAssertEqual(sends, 2); XCTAssertEqual(lastVersions[track.id], 4)
        XCTAssertNil(store.soundDesignRequest)
        remote.disconnect()
    }

    @MainActor func testRejectedDrumCommitRetainsThePatternAndClearsWorking() async throws {
        let (store, remote, _, _) = try fixture { _, _, _, _ in .init(requestID: UUID(), status: .rejected, revision: 3, message: "Drum render rejected") }
        store.requestDrumDesigner(editClip: true)
        let request = try XCTUnwrap(store.drumDesignRequest)
        store.drumDesigner.commit(request.pattern, request: request)
        XCTAssertEqual(store.drumDesignRequest?.id, request.id); XCTAssertTrue(store.drumDesigner.working)
        await settle()
        XCTAssertEqual(store.drumDesignRequest?.id, request.id); XCTAssertFalse(store.drumDesigner.working)
        XCTAssertEqual(store.drumDesigner.message, "Drum render rejected")
        remote.disconnect()
    }

    @MainActor func testDrumCommitKeepsOpeningTargetAndDoesNotCloseAtReceipt() async throws {
        var captured: RemoteEditorIntent?, versions: [UUID: Int] = [:]
        let (store, remote, session, _) = try fixture { command, expected, _, _ in
            guard case .editor(let intent) = command else { throw RemoteError.invalidFrame }
            captured = intent; versions = expected
            return .init(requestID: UUID(), revision: 4)
        }
        store.requestDrumDesigner(editClip: true)
        let request = try XCTUnwrap(store.drumDesignRequest), track = session.tracks[0]
        store.selectedTrack = session.tracks[1].id; store.selectedClip = nil; store.playhead = 9000
        store.drumDesigner.commit(request.pattern, request: request)
        await settle()
        XCTAssertEqual(captured?.context.trackID, track.id); XCTAssertEqual(captured?.context.clipID, request.clipID)
        XCTAssertEqual(captured?.integers["start"], 450); XCTAssertEqual(versions, [track.id: 3])
        XCTAssertNotNil(store.drumDesignRequest)
        remote.receive(.delta(.init(revision: 4, tracks: [track], trackRevisions: [track.id: 4])))
        await settle()
        XCTAssertNil(store.drumDesignRequest)
        XCTAssertEqual(store.selectedTrack, session.tracks[1].id); XCTAssertEqual(store.playhead, 9000)
        remote.disconnect()
    }
}
