import SoundroomCore
import SoundroomRemote
import XCTest

@testable import Soundroom

final class RemoteGUIGestureTests: XCTestCase {
    @MainActor func testTransportInterruptsStaySmallAndPreserveLargeSelections() async throws {
        let store = SessionStore(headless: true)
        let serverID = UUID()
        var session = Session()
        session.tracks[0].clips = (0..<128).map {
            Clip(assetID: UUID(), name: "Selected", start: $0 * 1_000, length: 1_000)
        }
        var operations: [String] = []
        let remote = RemoteGUIController(
            store: store, credential: .native(try RemoteCredential.generate()), host: "fixture",
            commandSender: { command, versions, _, _ in
                guard case .editor(let intent) = command else { throw RemoteError.invalidFrame }
                operations.append(intent.operation)
                XCTAssertTrue(versions.isEmpty)
                XCTAssertEqual(intent.context, .init(cursorFrame: intent.context.cursorFrame))
                let data = try RemoteV3Codec.encodeMessage(.request(.init(clientID: UUID(), command: command)))
                XCTAssertLessThanOrEqual(
                    data.count, 1_024, "Transport interrupts must fit the server's reserved decoder lane")
                return .init(
                    requestID: UUID(), revision: 0,
                    payload: try JSONEncoder().encode(RemoteEditorContext(cursorFrame: 12_345)))
            })
        store.remoteGUI = remote
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .operator))
        remote.receive(.snapshot(.init(serverID: serverID, serverName: "Fixture", session: session, revision: 0)))
        let trackID = session.tracks[0].id
        let clipIDs = Set(session.tracks[0].clips.map(\.id))
        let clipID = session.tracks[0].clips[0].id
        store.selectedTrack = trackID
        store.selectedClip = clipID
        store.selectedClips = clipIDs
        store.selection = EditSelection(trackID: trackID, range: 0..<128_000)
        let original = RemoteCommand.editor(
            RemoteEditorIntent(operation: "transport.stop", context: store.remoteContext))
        XCTAssertGreaterThan(
            try RemoteV3Codec.encodeMessage(.request(.init(clientID: UUID(), command: original))).count, 1_024)
        for operation in ["transport.stop", "transport.pause", "transport.finishRecording"] {
            store.remoteRoute(operation)
            await settle()
            XCTAssertEqual(store.selectedTrack, trackID)
            XCTAssertEqual(store.selectedClip, clipID)
            XCTAssertEqual(store.selectedClips, clipIDs)
            XCTAssertEqual(store.selection?.range, 0..<128_000)
        }
        XCTAssertEqual(operations, ["transport.stop", "transport.pause", "transport.finishRecording"])
        XCTAssertEqual(store.playhead, 12_345)
        remote.disconnect()
    }

    @MainActor func testOpenAndTimingCommandsCarryOpeningSessionRevision() async throws {
        let store = SessionStore(headless: true)
        let session = Session()
        let serverID = UUID()
        var revisions: [String: Int] = [:]
        let remote = RemoteGUIController(
            store: store, credential: .native(try RemoteCredential.generate()), host: "fixture",
            commandSender: { command, _, revision, _ in
                switch command {
                case .openProject: revisions["open"] = revision
                case .editor(let intent): revisions[intent.operation] = revision
                default: break
                }
                return .init(requestID: UUID(), revision: revision ?? 0)
            })
        store.remoteGUI = remote
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .operator))
        remote.receive(.snapshot(.init(serverID: serverID, serverName: "Fixture", session: session, revision: 7)))
        remote.openServerProject("Saved.soundroom")
        store.setMusicalTiming(tempo: 128)
        await settle()
        XCTAssertEqual(revisions["open"], 7)
        XCTAssertEqual(revisions["session.timing"], 7)
        remote.disconnect()
    }

    @MainActor func testRecordingAndPlayButtonsCancelRemotePreparation() async throws {
        let store = SessionStore(headless: true)
        let serverID = UUID()
        var operations: [String] = []
        let remote = RemoteGUIController(
            store: store, credential: .native(try RemoteCredential.generate()), host: "fixture",
            commandSender: { command, _, _, _ in
                guard case .editor(let intent) = command else { throw RemoteError.invalidFrame }
                operations.append(intent.operation)
                return .init(requestID: UUID(), revision: 0)
            })
        store.remoteGUI = remote
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .operator))
        remote.receive(.transport(.init(frame: 0, isPreparing: true)))
        XCTAssertTrue(store.busy)
        XCTAssertTrue(store.canToggleTransport)
        XCTAssertTrue(store.canToggleRecording)
        store.record()
        store.togglePlay()
        await settle()
        XCTAssertEqual(operations, ["transport.stop", "transport.pause"])
        remote.disconnect()
    }

    @MainActor func testRecordToggleFinishesThroughUrgentLaneWhileAnEditIsBlocked() async throws {
        let store = SessionStore()
        let serverID = UUID()
        var held: CheckedContinuation<RemoteReply, Error>?
        var operations: [String] = []
        let remote = RemoteGUIController(
            store: store, credential: .native(try RemoteCredential.generate()), host: "fixture",
            commandSender: { command, _, _, _ in
                guard case .editor(let intent) = command else { throw RemoteError.rejected("Unexpected command") }
                operations.append(intent.operation)
                if intent.operation == "pitch.analyze" {
                    return try await withCheckedThrowingContinuation { held = $0 }
                }
                return .init(requestID: UUID(), revision: 0)
            })
        store.remoteGUI = remote
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .operator))
        remote.route(.init(operation: "pitch.analyze"))
        await settle()
        XCTAssertNotNil(held)
        remote.receive(.transport(.init(frame: 48_000, isPlaying: true, isRecording: true)))
        store.busy = true
        XCTAssertTrue(store.canToggleRecording)
        store.record()
        await settle()
        XCTAssertEqual(operations, ["pitch.analyze", "transport.finishRecording"])
        held?.resume(returning: .init(requestID: UUID(), revision: 0))
        held = nil
        await settle()
        remote.receive(.transport(.init(frame: 48_000)))
        store.busy = true
        XCTAssertFalse(store.canToggleRecording, "Busy must still prevent starting a new take")
        remote.disconnect()
    }

    @MainActor private func settle() async {
        for _ in 0..<40 { await Task.yield() }
    }

    @MainActor func testConfirmingAnOlderDragStepDoesNotPullTheControlBack() async throws {
        let store = SessionStore()
        let session = Session()
        let serverID = UUID()
        let id = session.tracks[0].id
        var held: CheckedContinuation<RemoteReply, Error>?
        let remote = RemoteGUIController(
            store: store, credential: .native(try RemoteCredential.generate()), host: "fixture",
            commandSender: { command, _, _, _ in
                guard case .setTrackGain = command else { throw RemoteError.rejected("Unexpected command") }
                return try await withCheckedThrowingContinuation { held = $0 }
            })
        store.remoteGUI = remote
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .operator))
        remote.receive(
            .snapshot(
                .init(serverID: serverID, serverName: "Fixture", session: session, revision: 3, trackRevisions: [id: 3])
            ))
        store.beginContinuousEdit("gain")
        store.updateTrack(id, coalescingKey: "gain") { $0.gain = 0.5 }
        await settle()
        store.updateTrack(id, coalescingKey: "gain") { $0.gain = 0.6 }
        // The server confirms 0.5 while 0.6 is still queued: the slider stays at 0.6.
        remote.receive(
            .delta(.init(revision: 4, controlPatches: [.init(trackID: id, gain: 0.5)], trackRevisions: [id: 4])))
        XCTAssertEqual(store.session.tracks[0].gain, 0.6)
        held?.resume(returning: .init(requestID: UUID(), revision: 4))
        held = nil
        await settle()
        remote.receive(
            .delta(.init(revision: 5, controlPatches: [.init(trackID: id, gain: 0.6)], trackRevisions: [id: 5])))
        held?.resume(returning: .init(requestID: UUID(), revision: 5))
        held = nil
        await settle()
        XCTAssertEqual(store.session.tracks[0].gain, 0.6)
        store.endContinuousEdit()
        remote.disconnect()
    }

    @MainActor func testQueuedDragWaitsForOwnMetadataBeforeAdvancingItsVersion() async throws {
        let store = SessionStore()
        let session = Session()
        let serverID = UUID()
        let id = session.tracks[0].id
        var sentVersions: [Int?] = []
        weak var target: RemoteGUIController?
        let remote = RemoteGUIController(
            store: store, credential: .native(try RemoteCredential.generate()), host: "fixture",
            commandSender: { command, versions, _, _ in
                guard case .setTrackGain(_, let value) = command else {
                    throw RemoteError.rejected("Unexpected command")
                }
                sentVersions.append(versions[id])
                let revision = 3 + sentVersions.count
                if sentVersions.count == 2 {
                    target?.receive(
                        .delta(
                            .init(
                                revision: revision, controlPatches: [.init(trackID: id, gain: value)],
                                trackRevisions: [id: revision])))
                }
                return .init(requestID: UUID(), revision: revision)
            })
        target = remote
        store.remoteGUI = remote
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .operator))
        remote.receive(
            .snapshot(
                .init(serverID: serverID, serverName: "Fixture", session: session, revision: 3, trackRevisions: [id: 3])
            ))
        store.beginContinuousEdit("gain")
        store.updateTrack(id, coalescingKey: "gain") { $0.gain = 0.5 }
        await settle()
        XCTAssertEqual(sentVersions, [3])
        store.updateTrack(id, coalescingKey: "gain") { $0.gain = 0.7 }
        await settle()
        XCTAssertEqual(sentVersions, [3], "A receipt alone cannot advance the queued edit")
        remote.receive(
            .delta(.init(revision: 4, controlPatches: [.init(trackID: id, gain: 0.5)], trackRevisions: [id: 4])))
        await settle()
        XCTAssertEqual(sentVersions, [3, 4])
        XCTAssertEqual(store.session.tracks[0].gain, 0.7)
        XCTAssertNil(remote.unsentEdit)
        store.endContinuousEdit()
        remote.disconnect()
    }

    @MainActor func testForeignEditDuringReceiptWaitPreservesLatestDraftAndNeverSendsIt() async throws {
        let store = SessionStore()
        let session = Session()
        let serverID = UUID()
        let id = session.tracks[0].id
        var sent = 0
        let remote = RemoteGUIController(
            store: store, credential: .native(try RemoteCredential.generate()), host: "fixture",
            commandSender: { _, _, _, _ in
                sent += 1
                return .init(requestID: UUID(), revision: 4)
            })
        store.remoteGUI = remote
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .operator))
        remote.receive(
            .snapshot(
                .init(serverID: serverID, serverName: "Fixture", session: session, revision: 3, trackRevisions: [id: 3])
            ))
        store.beginContinuousEdit("gain")
        store.updateTrack(id, coalescingKey: "gain") { $0.gain = 0.5 }
        await settle()
        store.updateTrack(id, coalescingKey: "gain") { $0.gain = 0.7 }
        remote.receive(
            .delta(.init(revision: 5, controlPatches: [.init(trackID: id, gain: 0.2)], trackRevisions: [id: 5])))
        await settle()
        XCTAssertEqual(sent, 1)
        XCTAssertEqual(remote.unsentEdit?.tracks.first?.gain, 0.7)
        XCTAssertEqual(store.session.tracks[0].gain, 0.2)
        remote.copyUnsentTrack(id)
        XCTAssertTrue(store.canPasteTrack)
        XCTAssertEqual(remote.trackRevisions[id], 5)
        store.endContinuousEdit()
        remote.disconnect()
    }

    @MainActor func testDisconnectDoesNotReplayAnOldDragAfterRevisionReset() async throws {
        let store = SessionStore()
        let session = Session()
        let serverID = UUID()
        let id = session.tracks[0].id
        var sent = 0
        var held: CheckedContinuation<RemoteReply, Error>?
        let remote = RemoteGUIController(
            store: store, credential: .native(try RemoteCredential.generate()), host: "fixture",
            commandSender: { _, _, _, _ in
                sent += 1
                return try await withCheckedThrowingContinuation { held = $0 }
            })
        store.remoteGUI = remote
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .operator))
        remote.receive(
            .snapshot(
                .init(serverID: serverID, serverName: "Fixture", session: session, revision: 3, trackRevisions: [id: 3])
            ))
        store.beginContinuousEdit("gain")
        store.updateTrack(id, coalescingKey: "gain") { $0.gain = 0.5 }
        await settle()
        store.updateTrack(id, coalescingKey: "gain") { $0.gain = 0.7 }
        remote.receiveState(.disconnected)
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .operator))
        var reopened = session
        reopened.tracks[0].gain = 0.1
        remote.receive(
            .snapshot(
                .init(
                    serverID: serverID, serverName: "Fixture", session: reopened, revision: 3, trackRevisions: [id: 3]))
        )
        held?.resume(returning: .init(requestID: UUID(), revision: 4))
        held = nil
        await settle()
        XCTAssertEqual(sent, 1)
        XCTAssertEqual(remote.unsentEdit?.tracks.first?.gain, 0.7)
        XCTAssertEqual(store.session.tracks[0].gain, 0.1)
        XCTAssertEqual(remote.trackRevisions[id], 3)
        store.endContinuousEdit()
        remote.disconnect()
    }

    @MainActor func testSparseForeignDeltaUsesAuthoredStateInsteadOfAnOptimisticDraft() throws {
        let store = SessionStore()
        let session = Session()
        let serverID = UUID()
        let id = session.tracks[0].id
        let remote = RemoteGUIController(store: store, credential: try RemoteCredential.generate(), host: "fixture")
        remote.receive(
            .snapshot(
                .init(serverID: serverID, serverName: "Fixture", session: session, revision: 3, trackRevisions: [id: 3])
            ))
        store.session.tracks[0].realtimeEffects = [.init(kind: .room)]
        remote.receive(
            .delta(.init(revision: 4, controlPatches: [.init(trackID: id, pan: 0.2)], trackRevisions: [id: 4])))
        XCTAssertEqual(store.session.tracks[0].realtimeEffects, session.tracks[0].realtimeEffects)
        XCTAssertEqual(store.session.tracks[0].pan, 0.2)
        remote.receive(.delta(.init(revision: 5, removedTrackIDs: [id])))
        XCTAssertNil(remote.trackRevisions[id])
        XCTAssertTrue(store.session.tracks.isEmpty)
    }

    @MainActor func testLateConnectionFailureDoesNotDiscardNewConnectionEdits() async throws {
        let store = SessionStore()
        let session = Session()
        let serverID = UUID()
        let id = session.tracks[0].id
        var sent = 0
        var held: CheckedContinuation<RemoteReply, Error>?
        weak var target: RemoteGUIController?
        let remote = RemoteGUIController(
            store: store, credential: .native(try RemoteCredential.generate()), host: "fixture",
            commandSender: { command, versions, _, _ in
                sent += 1
                if sent == 1 { return try await withCheckedThrowingContinuation { held = $0 } }
                guard case .setTrackGain(_, let value) = command else {
                    throw RemoteError.rejected("Unexpected command")
                }
                XCTAssertEqual(versions[id], 3)
                target?.receive(
                    .delta(
                        .init(revision: 4, controlPatches: [.init(trackID: id, gain: value)], trackRevisions: [id: 4])))
                return .init(requestID: UUID(), revision: 4)
            })
        target = remote
        store.remoteGUI = remote
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .operator))
        remote.receive(
            .snapshot(
                .init(serverID: serverID, serverName: "Fixture", session: session, revision: 3, trackRevisions: [id: 3])
            ))
        store.beginContinuousEdit("old gain")
        store.updateTrack(id, coalescingKey: "old gain") { $0.gain = 0.5 }
        await settle()
        remote.disconnect()
        store.endContinuousEdit()
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .operator))
        remote.receive(
            .snapshot(
                .init(serverID: serverID, serverName: "Fixture", session: session, revision: 3, trackRevisions: [id: 3])
            ))
        store.beginContinuousEdit("new gain")
        store.updateTrack(id, coalescingKey: "new gain") { $0.gain = 0.8 }
        held?.resume(throwing: RemoteError.timeout)
        held = nil
        await settle()
        XCTAssertEqual(sent, 2)
        XCTAssertEqual(store.session.tracks[0].gain, 0.8)
        XCTAssertEqual(remote.unsentEdit?.tracks.first?.gain, 0.5)
        XCTAssertNil(remote.lastError)
        store.endContinuousEdit()
        remote.disconnect()
    }

    @MainActor func testExistingTrackWithoutVersionIsPreservedRatherThanSentUnfenced() async throws {
        let store = SessionStore()
        let session = Session()
        let serverID = UUID()
        let id = session.tracks[0].id
        var sent = 0
        let remote = RemoteGUIController(
            store: store, credential: .native(try RemoteCredential.generate()), host: "fixture",
            commandSender: { _, _, _, _ in
                sent += 1
                return .init(requestID: UUID(), revision: 1)
            })
        store.remoteGUI = remote
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .operator))
        remote.receive(.snapshot(.init(serverID: serverID, serverName: "Fixture", session: session)))
        store.updateTrack(id) { $0.gain = 0.4 }
        await settle()
        XCTAssertEqual(sent, 0)
        XCTAssertEqual(remote.unsentEdit?.tracks.first?.gain, 0.4)
        XCTAssertEqual(store.session, session)
        remote.disconnect()
    }
}
