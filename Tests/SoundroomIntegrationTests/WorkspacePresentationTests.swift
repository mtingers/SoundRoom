import XCTest
import SoundroomCore
import SoundroomRemote
@testable import Soundroom

final class WorkspacePresentationTests: XCTestCase {
    @MainActor private func settle() async { for _ in 0..<60 { await Task.yield() } }
    @MainActor private func fixture(role: RemoteRole = .operator, sender: @escaping RemoteGUIController.CommandSender = { _, _, _, _ in .init(requestID: UUID(), revision: 0) }) throws -> (SessionStore, RemoteGUIController) {
        let store = SessionStore(headless: true), server = UUID()
        store.session.tracks[0].name = "Lead vocal"
        store.session.tracks[0].clips = [Clip(assetID: UUID(), name: "Verse", start: 48_000, length: 96_000)]
        store.session.tracks[0].realtimeEffects = [RealtimeEffect(kind: .vocalEnhancer, parameters: RealtimeEffectKind.vocalEnhancer.defaultParameters)]
        let remote = RemoteGUIController(store: store, credential: .native(try RemoteCredential.generate()), host: "fixture", commandSender: sender)
        store.remoteGUI = remote
        remote.receiveState(.connected(serverID: server, name: "Fixture", role: role))
        remote.receive(.snapshot(.init(serverID: server, serverName: "Fixture", session: store.session, revision: 0, trackRevisions: [store.session.tracks[0].id: 0], projectName: "Existing.soundroom")))
        return (store, remote)
    }

    func testOverviewMapsBothEdgesAndPreservesTheGrabPoint() {
        let viewport = TimelineViewport(offset: 1_000, width: 800, contentWidth: 6_000)
        let model = TimelineOverviewGeometry(totalSeconds: 60, viewport: viewport, zoom: 100, width: 600)
        XCTAssertEqual(model.thumbWidth, 80)
        XCTAssertEqual(model.offset(at: model.thumbX + 17, grabOffset: 17), 1_000, accuracy: 0.000001)
        XCTAssertEqual(model.offset(at: -100, grabOffset: 17), 0)
        XCTAssertEqual(model.offset(at: 900, grabOffset: 17), 5_200)
        let fitted = TimelineOverviewGeometry(totalSeconds: 5, viewport: viewport, zoom: 100, width: 600)
        XCTAssertEqual(fitted.thumbWidth, 600)
        XCTAssertEqual(fitted.offset(at: 400, grabOffset: 0), 0)
    }
    @MainActor func testOverviewNavigationNeverSeeksOrChangesZoomOrSelection() {
        let store = SessionStore(headless: true)
        store.playhead = 96_000; store.zoom = 240; store.isPlaying = true
        store.selection = EditSelection(trackID: store.session.tracks[0].id, range: 48_000..<96_000)
        let selection = store.selection, initial = store.timelineScrollSerial
        store.requestTimelineScroll(1_200)
        XCTAssertEqual(store.timelineScrollOffset, 1_200)
        XCTAssertEqual(store.timelineScrollSerial, initial + 1)
        XCTAssertFalse(store.followPlayhead)
        XCTAssertTrue(store.isPlaying)
        XCTAssertEqual(store.playhead, 96_000)
        XCTAssertEqual(store.zoom, 240)
        XCTAssertEqual(store.selection, selection)
        XCTAssertEqual(store.status, "Ready", "Navigation must not send a transport command")
        store.requestTimelineScroll(.infinity)
        XCTAssertEqual(store.timelineScrollSerial, initial + 1)
    }
    @MainActor func testScopeDistinguishesWholeTrackClipRangeAndMaster() {
        let store = SessionStore(headless: true), clip = Clip(assetID: UUID(), name: "Verse", start: 48_000, length: 96_000)
        store.session.tracks[0].name = "Lead vocal"; store.session.tracks[0].clips = [clip]
        XCTAssertEqual(store.editingScope.detail, "Whole track · live processing")
        store.selectedClip = clip.id
        XCTAssertEqual(store.editingScope.detail, "Verse")
        XCTAssertEqual(store.editingScope.range, 48_000..<144_000)
        store.selection = EditSelection(trackID: store.selectedTrack!, range: 60_000..<70_000)
        XCTAssertEqual(store.editingScope.detail, "Selected audio")
        XCTAssertEqual(store.editingScope.range, 60_000..<70_000)
        store.selectedTrack = Session.masterTrackID
        XCTAssertEqual(store.editingScope.trackName, "Master")
        XCTAssertNil(store.editingScope.range)
    }
    @MainActor func testTrackHeightKeepsDropTargetsAndWaveformGeometryAligned() {
        let store = SessionStore(headless: true)
        store.session.tracks.append(Track(name: "Second", color: 1))
        for size in TrackSize.allCases {
            store.trackSize = size
            XCTAssertEqual(store.trackForVerticalMove(from: store.session.tracks[0].id, delta: size.height), store.session.tracks[1].id)
            XCTAssertEqual(store.trackForVerticalMove(from: store.session.tracks[1].id, delta: -size.height), store.session.tracks[0].id)
            let lane = AudioLaneGeometry(height: size.height)
            XCTAssertEqual(lane.bottom, size.height - 8)
            XCTAssertLessThan(lane.center + lane.amplitude, lane.bottom)
            XCTAssertGreaterThan(lane.center - lane.amplitude, 36)
        }
    }
    @MainActor func testVocalEntryReusesExistingInsertAndPinsTheTrack() throws {
        let (store, remote) = try fixture()
        defer { remote.disconnect() }
        let id = store.session.tracks[0].realtimeEffects[0].id
        store.requestVocalEnhancement()
        let request = try XCTUnwrap(store.vocalEnhancementRequest)
        XCTAssertEqual(request.trackName, "Lead vocal")
        XCTAssertEqual(request.target, .realtime(trackID: store.session.tracks[0].id, effectID: id, kind: .vocalEnhancer))
        XCTAssertEqual(store.session.tracks[0].realtimeEffects.count, 1)
        store.selectedTrack = Session.masterTrackID
        XCTAssertFalse(request.target.matches(store), "A stale sheet must not edit a different target")
    }
    @MainActor func testTrackSeparationCapturesWholeTrackAndDoesNotNeedAnIsolatorInsert() throws {
        let (store, remote) = try fixture()
        defer { remote.disconnect() }
        let track = store.session.tracks[0]
        store.requestSourceSeparation(trackID: track.id, wholeTrack: true)
        let request = try XCTUnwrap(store.sourceSeparationRequest)
        XCTAssertEqual(request.range, 0..<144_000)
        XCTAssertEqual(request.context.trackID, track.id)
        XCTAssertEqual(store.session.tracks[0].realtimeEffects.count, 1)
        XCTAssertFalse(store.session.tracks[0].realtimeEffects.contains { $0.kind == .sourceIsolator })
    }
    @MainActor func testViewerCannotOpenEditingTools() throws {
        let (store, remote) = try fixture(role: .viewer)
        defer { remote.disconnect() }
        store.requestVocalEnhancement()
        store.requestSourceSeparation(trackID: store.selectedTrack, wholeTrack: true)
        XCTAssertNil(store.vocalEnhancementRequest)
        XCTAssertNil(store.sourceSeparationRequest)
    }
    @MainActor func testFailureDetailsSurviveStatusChangesAndActivityIsBounded() {
        let store = SessionStore(headless: true)
        store.recordFailure("The destination is unavailable. Choose another folder.")
        for index in 0..<150 { store.status = "Processing \(index)" }
        XCTAssertEqual(store.activity.entries.count, 100)
        XCTAssertTrue(store.activity.entries.contains { $0.kind == .failure && $0.message.contains("destination") })
        XCTAssertEqual(store.activity.unreadFailures, 1)
        store.activity.markRead()
        XCTAssertEqual(store.activity.unreadFailures, 0)
        XCTAssertTrue(store.activity.entries.contains { $0.kind == .failure })
    }
    @MainActor func testSaveBadgeRequiresAConfirmedMatchingSaveAndInvalidatesOnNewEdits() async throws {
        let (store, remote) = try fixture()
        defer { remote.disconnect() }
        XCTAssertEqual(remote.projectSaveStatus, .unverified, "A project name does not prove its disk copy is current")
        remote.send(.saveProject(name: store.session.name), expectedRevision: 0)
        await settle()
        XCTAssertEqual(remote.projectSaveStatus, .saved)
        var track = store.session.tracks[0]; track.gain = 0.25
        remote.receive(.delta(.init(revision: 1, tracks: [track], trackRevisions: [track.id: 1])))
        XCTAssertEqual(remote.projectSaveStatus, .modified)
        remote.disconnect()
        XCTAssertEqual(remote.projectSaveStatus, .unverified)
    }
    @MainActor func testSaveOfAnOlderSnapshotCannotShowTheNewerSessionAsSaved() async throws {
        var held: CheckedContinuation<RemoteReply, Error>?
        let (store, remote) = try fixture { _, _, _, _ in try await withCheckedThrowingContinuation { held = $0 } }
        defer { remote.disconnect() }
        remote.send(.saveProject(name: store.session.name), expectedRevision: 0)
        await settle()
        XCTAssertNotNil(held); XCTAssertNotNil(remote.activeWork)
        var track = store.session.tracks[0]; track.name = "A newer edit"
        remote.receive(.delta(.init(revision: 1, tracks: [track], trackRevisions: [track.id: 1])))
        held?.resume(returning: .init(requestID: UUID(), revision: 1)); held = nil
        await settle()
        XCTAssertEqual(remote.projectSaveStatus, .modified)
        XCTAssertNil(remote.activeWork)
    }
    @MainActor func testSeparationFailureCanReopenItsChoicesWithoutReplayingTheCommand() async throws {
        var processingRequests = 0
        let (store, remote) = try fixture { command, _, _, _ in
            if case .editor(let intent) = command, intent.operation == "sources.status" {
                return .init(requestID: UUID(), revision: 0, payload: Data(#"{"available":true,"models":["htdemucs_ft"],"message":"Ready"}"#.utf8))
            }
            if case .editor(let intent) = command, intent.operation == "effects.apply" {
                processingRequests += 1
                return .init(requestID: UUID(), status: .rejected, revision: 0, message: "The source model is unavailable. Check installation and retry.")
            }
            return .init(requestID: UUID(), revision: 0)
        }
        defer { remote.disconnect() }
        store.requestSourceSeparation(trackID: store.selectedTrack, wholeTrack: true)
        let request = try XCTUnwrap(store.sourceSeparationRequest)
        request.target = .bass; request.operation = .remove
        request.checkRuntime(); await settle()
        XCTAssertTrue(request.canApply)
        request.run(apply: true); await settle()
        XCTAssertEqual(processingRequests, 1)
        XCTAssertEqual(store.activity.unreadFailures, 1, "A failure and its recovery action are one notification")
        store.sourceSeparationRequest = nil
        let recovery = try XCTUnwrap(store.activity.entries.first { $0.recovery != nil }?.recovery)
        recovery()
        XCTAssertEqual(store.sourceSeparationRequest?.id, request.id)
        XCTAssertEqual(store.sourceSeparationRequest?.target, .bass)
        XCTAssertEqual(store.sourceSeparationRequest?.operation, .remove)
        XCTAssertEqual(processingRequests, 1, "Recovery must reopen choices, not replay an authored operation")
        var track = store.session.tracks[0]; track.name = "Changed source"
        remote.receive(.delta(.init(revision: 1, tracks: [track], trackRevisions: [track.id: 1])))
        request.run(apply: true); await settle()
        XCTAssertEqual(processingRequests, 1, "A recovered workspace must retain its original source fence")
    }
}
