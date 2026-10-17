import SoundroomCore
import SoundroomRemote
import XCTest

@testable import Soundroom

final class SourceSeparationSheetTests: XCTestCase {
    @MainActor private func settle() async { for _ in 0..<60 { await Task.yield() } }
    @MainActor private func fixture(sender: @escaping RemoteGUIController.CommandSender) throws -> (
        SessionStore, RemoteGUIController, Session, UUID
    ) {
        let store = SessionStore(headless: true)
        let serverID = UUID()
        var session = Session()
        let clip = Clip(assetID: UUID(), name: "Mixed audio", start: 100, length: 48_000)
        session.tracks[0].clips = [clip]
        session.tracks.append(Track(name: "Other track", color: 1))
        let remote = RemoteGUIController(
            store: store, credential: .native(try RemoteCredential.generate()), host: "fixture", commandSender: sender)
        store.remoteGUI = remote
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .operator))
        remote.receive(
            .snapshot(
                .init(
                    serverID: serverID, serverName: "Fixture", session: session, revision: 3,
                    trackRevisions: Dictionary(uniqueKeysWithValues: session.tracks.map { ($0.id, 3) }))))
        store.selectedTrack = session.tracks[0].id
        store.selectedClip = clip.id
        store.effectMode = .realtime
        return (store, remote, session, serverID)
    }
    private func runtimeReply(revision: Int = 3) -> RemoteReply {
        .init(
            requestID: UUID(), revision: revision,
            payload: Data(
                #"{"available":true,"models":["htdemucs_ft","htdemucs","htdemucs_6s"],"message":"Ready"}"#.utf8))
    }

    @MainActor func testCapturedSelectionCannotFollowLaterNavigation() async throws {
        let (store, remote, session, _) = try fixture { _, _, _, _ in self.runtimeReply() }
        store.selection = .init(trackID: session.tracks[0].id, range: 2400..<9600)
        let request = try XCTUnwrap(SourceSeparationController.capture(store: store))
        store.selectedTrack = session.tracks[1].id
        store.selectedClip = nil
        store.selection = nil
        store.playhead = 90_000
        request.target = .guitar
        request.quality = .standard
        request.operation = .remove
        let intent = request.intent(apply: true)
        XCTAssertEqual(intent.operation, "effects.apply")
        XCTAssertEqual(intent.context.trackID, session.tracks[0].id)
        XCTAssertEqual(intent.context.clipID, session.tracks[0].clips[0].id)
        XCTAssertEqual(intent.context.rangeStart, 2400)
        XCTAssertEqual(intent.context.rangeEnd, 9600)
        XCTAssertEqual(intent.strings["separationTarget"], "guitar")
        XCTAssertEqual(
            intent.strings["separationModel"], "extended",
            "Guitar requires the six-source model even if a caller changes quality")
        XCTAssertEqual(intent.strings["separationOperation"], "remove")
        XCTAssertEqual(intent.flags["separateSource"], true)
        XCTAssertEqual(intent.payload, Data("{}".utf8))
        remote.disconnect()
    }

    @MainActor func testForeignTrackRevisionRejectsWorkAndRetainsChoices() async throws {
        var processingSends = 0
        let (store, remote, session, _) = try fixture { command, _, _, _ in
            if case .editor(let intent) = command, intent.operation == "sources.status" { return self.runtimeReply() }
            processingSends += 1
            return .init(requestID: UUID(), revision: 4)
        }
        let request = try XCTUnwrap(SourceSeparationController.capture(store: store))
        request.target = .bass
        request.operation = .remove
        request.checkRuntime()
        await settle()
        XCTAssertTrue(request.canRun)
        remote.receive(
            .delta(
                .init(
                    revision: 4, controlPatches: [.init(trackID: session.tracks[0].id, gain: 0.4)],
                    trackRevisions: [session.tracks[0].id: 4])))
        request.run(apply: true)
        await settle()
        XCTAssertEqual(processingSends, 0)
        XCTAssertFalse(request.pending)
        XCTAssertFalse(request.completed)
        XCTAssertEqual(request.target, .bass)
        XCTAssertEqual(request.operation, .remove)
        XCTAssertTrue(request.message.contains("choices are retained"))
        remote.disconnect()
    }

    @MainActor func testPreviewCarriesOpeningVersionAndKeepsTheDraft() async throws {
        var captured: RemoteEditorIntent?
        var versions: [UUID: Int] = [:]
        let (store, remote, session, _) = try fixture { command, expected, _, _ in
            guard case .editor(let intent) = command else { throw RemoteError.invalidFrame }
            if intent.operation == "sources.status" { return self.runtimeReply() }
            captured = intent
            versions = expected
            return .init(requestID: UUID(), revision: 3)
        }
        let request = try XCTUnwrap(SourceSeparationController.capture(store: store))
        request.checkRuntime()
        await settle()
        request.run(apply: false)
        await settle()
        XCTAssertEqual(captured?.operation, "effects.preview")
        XCTAssertEqual(versions, [session.tracks[0].id: 3])
        XCTAssertTrue(request.previewing)
        XCTAssertFalse(request.completed)
        XCTAssertFalse(request.pending)
        remote.disconnect()
    }

    @MainActor func testApplyWaitsForAuthoredMetadataBeforeCompleting() async throws {
        var versions: [UUID: Int] = [:]
        let (store, remote, session, _) = try fixture { command, expected, _, _ in
            guard case .editor(let intent) = command else { throw RemoteError.invalidFrame }
            if intent.operation == "sources.status" { return self.runtimeReply() }
            versions = expected
            return .init(requestID: UUID(), revision: 4)
        }
        let request = try XCTUnwrap(SourceSeparationController.capture(store: store))
        request.checkRuntime()
        await settle()
        request.run(apply: true)
        await settle()
        XCTAssertEqual(versions, [session.tracks[0].id: 3])
        XCTAssertTrue(request.pending)
        XCTAssertFalse(request.completed)
        remote.receive(
            .delta(.init(revision: 4, tracks: [session.tracks[0]], trackRevisions: [session.tracks[0].id: 4])))
        await settle()
        XCTAssertFalse(request.pending)
        XCTAssertTrue(request.completed)
        remote.disconnect()
    }

    @MainActor func testRuntimeModelAvailabilityAndUnsupportedMasterAreEnforced() async throws {
        let (store, remote, _, _) = try fixture { _, _, _, _ in
            .init(
                requestID: UUID(), revision: 3,
                payload: Data(
                    #"{"available":true,"models":{"quality":{"name":"htdemucs_ft","available":true},"extended":{"name":"htdemucs_6s","available":false}},"message":"Partial install"}"#
                        .utf8))
        }
        let request = try XCTUnwrap(SourceSeparationController.capture(store: store))
        request.checkRuntime()
        await settle()
        XCTAssertTrue(request.canRun)
        request.target = .piano
        XCTAssertEqual(request.effectiveQuality, .extended)
        XCTAssertFalse(request.canRun)
        store.selectedTrack = Session.masterTrackID
        store.effectMode = .master
        XCTAssertNil(SourceSeparationController.capture(store: store))
        remote.disconnect()
    }

    @MainActor func testReconnectCannotReauthorizeTheOpeningSource() async throws {
        let (store, remote, session, serverID) = try fixture { _, _, _, _ in self.runtimeReply() }
        let request = try XCTUnwrap(SourceSeparationController.capture(store: store))
        request.checkRuntime()
        await settle()
        remote.receiveState(.disconnected)
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .operator))
        remote.receive(
            .snapshot(
                .init(
                    serverID: serverID, serverName: "Fixture", session: session, revision: 3,
                    trackRevisions: Dictionary(uniqueKeysWithValues: session.tracks.map { ($0.id, 3) }))))
        XCTAssertFalse(request.sourceIsCurrent)
        XCTAssertFalse(request.canRun)
        request.run(apply: true)
        XCTAssertTrue(request.message.contains("source changed"))
        remote.disconnect()
    }

    @MainActor func testEditorCanPreviewApplyAndStopOnlyItsEffectsAudition() async throws {
        var captured: [RemoteEditorIntent] = []
        let (store, remote, _, serverID) = try fixture { command, _, _, _ in
            if case .editor(let intent) = command, intent.operation == "sources.status" { return self.runtimeReply() }
            if case .editor(let intent) = command { captured.append(intent) }
            return .init(requestID: UUID(), revision: 3)
        }
        let request = try XCTUnwrap(SourceSeparationController.capture(store: store))
        request.checkRuntime()
        await settle()
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .editor))
        XCTAssertTrue(request.canApply)
        XCTAssertTrue(request.canPreview)
        request.run(apply: false)
        await settle()
        XCTAssertTrue(request.previewing)
        request.stopPreview()
        await settle()
        XCTAssertFalse(request.previewing)
        XCTAssertEqual(captured.map(\.operation), ["effects.preview", "audition.stop"])
        XCTAssertEqual(
            captured.last?.strings["kind"], "effects",
            "Stopping this audition must leave main transport and other audition kinds alone")
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .viewer))
        XCTAssertFalse(request.canApply)
        XCTAssertNil(SourceSeparationController.capture(store: store))
        remote.disconnect()
    }

    @MainActor func testRejectedCancellationAllowsRetryWhileTheJobContinues() async throws {
        var held: CheckedContinuation<RemoteReply, Error>?
        var cancellations = 0
        var jobID: String?
        var cancellationIDs: [String] = []
        let (store, remote, _, _) = try fixture { command, _, _, _ in
            guard case .editor(let intent) = command else { return .init(requestID: UUID(), revision: 3) }
            if intent.operation == "sources.status" { return self.runtimeReply() }
            if intent.operation == "jobs.cancel" {
                cancellations += 1
                if let id = intent.strings["id"] { cancellationIDs.append(id) }
                return .init(requestID: UUID(), status: .rejected, revision: 3, message: "The job has not started yet.")
            }
            jobID = intent.strings["separationJobID"]
            return try await withCheckedThrowingContinuation { held = $0 }
        }
        let request = try XCTUnwrap(SourceSeparationController.capture(store: store))
        request.checkRuntime()
        await settle()
        request.run(apply: false)
        await settle()
        XCTAssertTrue(request.pending)
        request.cancelProcessing()
        await settle()
        XCTAssertEqual(cancellations, 1)
        XCTAssertTrue(request.pending)
        XCTAssertFalse(request.cancellationRequested)
        request.cancelProcessing()
        await settle()
        XCTAssertEqual(cancellations, 2)
        XCTAssertNotNil(jobID.flatMap(UUID.init(uuidString:)))
        XCTAssertEqual(
            cancellationIDs, [try XCTUnwrap(jobID), try XCTUnwrap(jobID)],
            "Cancel must target this exact separation request")
        held?.resume(returning: .init(requestID: UUID(), status: .rejected, revision: 3, message: "Job cancelled"))
        held = nil
        await settle()
        XCTAssertFalse(request.pending)
        XCTAssertFalse(request.completed)
        XCTAssertTrue(request.sourceIsCurrent)
        XCTAssertTrue(request.canRun, "Cancelling a run must not invalidate the unchanged opening source")
        remote.disconnect()
    }

    @MainActor func testQueuedCancellationPreventsLaterProcessing() async throws {
        var held: CheckedContinuation<RemoteReply, Error>?
        var processingSends = 0
        var cancellations = 0
        let (store, remote, _, _) = try fixture { command, _, _, _ in
            if case .snapshot = command { return try await withCheckedThrowingContinuation { held = $0 } }
            guard case .editor(let intent) = command else { throw RemoteError.invalidFrame }
            if intent.operation == "sources.status" { return self.runtimeReply() }
            if intent.operation == "jobs.cancel" { cancellations += 1 } else { processingSends += 1 }
            return .init(requestID: UUID(), revision: 3)
        }
        let request = try XCTUnwrap(SourceSeparationController.capture(store: store))
        request.checkRuntime()
        await settle()
        remote.send(.snapshot)
        await settle()
        request.run(apply: true)
        XCTAssertTrue(request.pending)
        request.cancelProcessing()
        await settle()
        XCTAssertFalse(request.pending)
        XCTAssertFalse(request.completed)
        XCTAssertTrue(request.message.contains("cancelled before processing"))
        XCTAssertEqual(cancellations, 0, "A queued-only cancellation must not cancel other server work")
        held?.resume(returning: .init(requestID: UUID(), revision: 3))
        held = nil
        await settle()
        XCTAssertEqual(processingSends, 0)
        XCTAssertTrue(request.canRun)
        remote.disconnect()
    }

    @MainActor func testRangeMustBelongToTheTrackAndFitTheSeparationLimit() async throws {
        let (store, remote, session, _) = try fixture { _, _, _, _ in self.runtimeReply() }
        store.selection = .init(trackID: session.tracks[1].id, range: 100..<1000)
        XCTAssertNil(
            SourceSeparationController.capture(store: store),
            "A stale selection cannot redirect its range to the current track")
        store.selection = .init(trackID: session.tracks[0].id, range: 0..<(30 * 60 * 48_000 + 1))
        XCTAssertNil(SourceSeparationController.capture(store: store))
        XCTAssertTrue(store.status.contains("30 minutes"))
        remote.disconnect()
    }

    @MainActor func testFailedProcessingCanRetryWithANewJobIDUntilItsSourceChanges() async throws {
        var jobIDs: [String] = []
        var processingSends = 0
        let (store, remote, session, _) = try fixture { command, _, _, _ in
            guard case .editor(let intent) = command else { return .init(requestID: UUID(), revision: 3) }
            if intent.operation == "sources.status" { return self.runtimeReply() }
            processingSends += 1
            if let id = intent.strings["separationJobID"] { jobIDs.append(id) }
            return .init(
                requestID: UUID(), status: processingSends == 1 ? .rejected : .accepted,
                revision: 3, message: processingSends == 1 ? "Job cancelled" : "Preview ready")
        }
        let request = try XCTUnwrap(SourceSeparationController.capture(store: store))
        request.checkRuntime()
        await settle()
        request.run(apply: false)
        await settle()
        XCTAssertFalse(request.pending)
        XCTAssertTrue(request.canRun)
        XCTAssertTrue(request.sourceIsCurrent)
        XCTAssertEqual(request.target, .vocals)
        request.run(apply: false)
        await settle()
        XCTAssertEqual(processingSends, 2)
        XCTAssertTrue(request.previewing)
        XCTAssertEqual(jobIDs.count, 2)
        XCTAssertNotEqual(jobIDs.first, jobIDs.last)
        remote.receive(
            .delta(
                .init(
                    revision: 4, controlPatches: [.init(trackID: session.tracks[0].id, gain: 0.5)],
                    trackRevisions: [session.tracks[0].id: 4])))
        XCTAssertFalse(request.canRun)
        XCTAssertFalse(request.sourceIsCurrent)
        request.run(apply: true)
        await settle()
        XCTAssertEqual(processingSends, 2, "A retry cannot adopt a changed source revision")
        remote.disconnect()
    }

    @MainActor func testLongSelectionPreviewsItsFirstTenMinutesAndAppliesTheWholeRange() async throws {
        let (store, remote, session, _) = try fixture { _, _, _, _ in self.runtimeReply() }
        let range = 100..<(100 + 11 * 60 * 48_000)
        store.selection = .init(trackID: session.tracks[0].id, range: range)
        let request = try XCTUnwrap(SourceSeparationController.capture(store: store))
        XCTAssertTrue(request.previewIsPartial)
        let preview = request.intent(apply: false)
        let apply = request.intent(apply: true)
        XCTAssertEqual(preview.context.rangeStart, range.lowerBound)
        XCTAssertEqual(preview.context.rangeEnd, range.lowerBound + 10 * 60 * 48_000)
        XCTAssertEqual(apply.context.rangeStart, range.lowerBound)
        XCTAssertEqual(apply.context.rangeEnd, range.upperBound)
        XCTAssertEqual(request.range, range, "Preview must not shrink the opening Apply selection")
        remote.disconnect()
    }
}
