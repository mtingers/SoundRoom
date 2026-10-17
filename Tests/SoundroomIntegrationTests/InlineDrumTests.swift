import SoundroomCore
import SoundroomRemote
import XCTest

@testable import Soundroom

final class InlineDrumTests: XCTestCase {
    @MainActor private func settle() async { for _ in 0..<100 { await Task.yield() } }
    @MainActor private func fixture(role: RemoteRole = .operator, sender: @escaping RemoteGUIController.CommandSender)
        throws -> (SessionStore, RemoteGUIController, Session, UUID)
    {
        let store = SessionStore(headless: true)
        let server = UUID()
        var session = Session()
        var pattern = DrumPattern()
        pattern.apply(.backbeat)
        var clip = Clip(assetID: UUID(), name: "Verse drums", start: 4_800, length: 192_000)
        clip.drumPattern = pattern
        var copy = clip
        copy.id = UUID()
        copy.start = 384_000
        copy.name = "Chorus drums"
        session.tracks[0].clips = [clip, copy]
        session.tracks.append(Track(name: "Bass", color: 1))
        let remote = RemoteGUIController(
            store: store, credential: .native(try RemoteCredential.generate()), host: "fixture", commandSender: sender)
        store.remoteGUI = remote
        remote.receiveState(.connected(serverID: server, name: "Fixture", role: role))
        remote.receive(
            .snapshot(
                .init(
                    serverID: server, serverName: "Fixture", session: session, revision: 3,
                    trackRevisions: Dictionary(uniqueKeysWithValues: session.tracks.map { ($0.id, 3) }))))
        store.selectedTrack = session.tracks[0].id
        store.selectedClip = clip.id
        store.isPlaying = true
        store.playhead = 80_000
        store.toggleInlineDrums(trackID: session.tracks[0].id, clipID: clip.id)
        return (store, remote, session, server)
    }
    @MainActor private func confirm(
        _ pattern: DrumPattern, track original: Track, revision: Int, remote: RemoteGUIController
    ) {
        var track = original
        track.clips[0].drumPattern = pattern
        track.clips[0].assetID = UUID()
        remote.receive(.delta(.init(revision: revision, tracks: [track], trackRevisions: [track.id: revision])))
    }
    @MainActor func testOpeningAndCollapsingNeverMovesOrStopsTransportAndPinsTheClip() throws {
        let (store, remote, session, _) = try fixture { _, _, _, _ in .init(requestID: UUID(), revision: 3) }
        defer { remote.disconnect() }
        let drums = try XCTUnwrap(store.inlineDrums)
        XCTAssertTrue(store.isPlaying)
        XCTAssertEqual(store.playhead, 80_000)
        store.selectedTrack = session.tracks[1].id
        store.selectedClip = nil
        XCTAssertEqual(drums.trackID, session.tracks[0].id)
        XCTAssertEqual(drums.clipID, session.tracks[0].clips[0].id)
        XCTAssertEqual(store.inlineDrumHeight(for: session.tracks[0]), InlineDrumLayout.height)
        store.toggleInlineDrums(trackID: drums.trackID)
        XCTAssertFalse(store.inlineDrumExpanded)
        XCTAssertEqual(store.inlineDrumHeight(for: session.tracks[0]), 0)
        XCTAssertTrue(store.isPlaying)
        XCTAssertEqual(store.playhead, 80_000)
    }
    @MainActor func testRapidChangesStayResponsiveAndSendOnlyOneRenderAtATime() async throws {
        var sent: [(RemoteEditorIntent, [UUID: Int])] = []
        let (store, remote, session, _) = try fixture { command, versions, _, _ in
            if case .editor(let intent) = command, intent.operation == "drums.commit" {
                sent.append((intent, versions))
                return .init(requestID: UUID(), revision: 3 + sent.count)
            }
            return .init(requestID: UUID(), revision: 3)
        }
        defer { remote.disconnect() }
        let drums = try XCTUnwrap(store.inlineDrums)
        let track = session.tracks[0]
        drums.editor.toggle(voice: .kick, index: 1)
        let first = drums.pattern
        drums.flush()
        await settle()
        XCTAssertEqual(sent.count, 1)
        XCTAssertTrue(drums.sending)
        XCTAssertTrue(drums.canEdit, "Rendering must keep the grid responsive")
        drums.editor.toggle(voice: .kick, index: 2)
        let latest = drums.pattern
        drums.flush()
        XCTAssertEqual(sent.count, 1, "An in-flight render must not disable the editor or queue old drafts")
        store.selectedTrack = session.tracks[1].id
        store.selectedClip = nil
        confirm(first, track: track, revision: 4, remote: remote)
        await settle()
        XCTAssertEqual(drums.pattern, latest, "An older ack must not pull controls backwards")
        drums.flush()
        await settle()
        XCTAssertEqual(sent.count, 2)
        XCTAssertEqual(sent[0].1, [track.id: 3])
        XCTAssertEqual(sent[1].1, [track.id: 4])
        XCTAssertEqual(sent[1].0.context.clipID, track.clips[0].id)
        XCTAssertEqual(sent[1].0.integers["start"], track.clips[0].start)
        XCTAssertEqual(try JSONDecoder().decode(DrumPattern.self, from: XCTUnwrap(sent[1].0.payload)), latest)
        confirm(latest, track: track, revision: 5, remote: remote)
        await settle()
        XCTAssertFalse(drums.unsettled)
        XCTAssertNil(drums.failure)
        XCTAssertEqual(store.session.tracks[0].clips[1], track.clips[1], "Copies keep independent patterns")
        XCTAssertEqual(store.selectedTrack, session.tracks[1].id)
        XCTAssertEqual(store.playhead, 80_000)
        XCTAssertTrue(store.isPlaying)
    }
    @MainActor func testRevertingWhileARenderIsInFlightStillPublishesTheLatestDraft() async throws {
        var sends = 0
        let (store, remote, session, _) = try fixture { _, _, _, _ in
            sends += 1
            return .init(requestID: UUID(), revision: 3 + sends)
        }
        defer { remote.disconnect() }
        let drums = try XCTUnwrap(store.inlineDrums)
        let original = drums.pattern
        drums.editor.toggle(voice: .kick, index: 1)
        let first = drums.pattern
        drums.flush()
        await settle()
        drums.editor.toggle(voice: .kick, index: 1)
        XCTAssertEqual(drums.pattern, original)
        confirm(first, track: session.tracks[0], revision: 4, remote: remote)
        await settle()
        XCTAssertTrue(drums.hasChanges)
        drums.flush()
        await settle()
        confirm(original, track: session.tracks[0], revision: 5, remote: remote)
        await settle()
        XCTAssertFalse(drums.unsettled)
        XCTAssertEqual(drums.pattern, original)
    }
    @MainActor func testForeignClipChangeRetainsTheDraftAndCannotOverwriteTheNewSource() async throws {
        var sends = 0
        let (store, remote, session, _) = try fixture { _, _, _, _ in
            sends += 1
            return .init(requestID: UUID(), revision: 4)
        }
        defer { remote.disconnect() }
        let drums = try XCTUnwrap(store.inlineDrums)
        drums.editor.toggle(voice: .kick, index: 1)
        let draft = drums.pattern
        var foreign = session.tracks[0].clips[0].drumPattern!
        foreign.bpm = 90
        confirm(foreign, track: session.tracks[0], revision: 4, remote: remote)
        await settle()
        drums.flush()
        await settle()
        XCTAssertEqual(sends, 0)
        XCTAssertEqual(drums.pattern, draft)
        XCTAssertNotNil(drums.failure)
        XCTAssertFalse(drums.canRetry, "Retry cannot silently rebase onto a changed clip")
        drums.reload()
        XCTAssertEqual(drums.pattern, foreign)
        XCTAssertFalse(drums.hasChanges)
        XCTAssertNil(drums.failure)
    }
    @MainActor func testUndoWaitsForTheNewestRenderSoALateAckCannotRedoTheEdit() async throws {
        var renders = 0
        var undos = 0
        let (store, remote, session, _) = try fixture { command, _, _, _ in
            if case .undo = command {
                undos += 1
                return .init(requestID: UUID(), revision: 6)
            }
            renders += 1
            return .init(requestID: UUID(), revision: 3 + renders)
        }
        defer { remote.disconnect() }
        let drums = try XCTUnwrap(store.inlineDrums)
        drums.editor.toggle(voice: .kick, index: 1)
        let first = drums.pattern
        drums.flush()
        await settle()
        drums.editor.toggle(voice: .kick, index: 2)
        let last = drums.pattern
        store.undo()
        XCTAssertEqual(undos, 0)
        confirm(first, track: session.tracks[0], revision: 4, remote: remote)
        await settle()
        drums.flush()
        await settle()
        confirm(last, track: session.tracks[0], revision: 5, remote: remote)
        await settle()
        XCTAssertEqual(undos, 1)
        XCTAssertFalse(drums.unsettled)
        confirm(first, track: session.tracks[0], revision: 6, remote: remote)
        await settle()
        XCTAssertEqual(drums.pattern, first)
        XCTAssertFalse(drums.unsettled)
    }
    @MainActor func testReconnectAndPermissionChangesCannotAuthorizeAnOldDraft() async throws {
        var sends = 0
        let (store, remote, session, server) = try fixture { _, _, _, _ in
            sends += 1
            return .init(requestID: UUID(), revision: 4)
        }
        defer { remote.disconnect() }
        let drums = try XCTUnwrap(store.inlineDrums)
        drums.editor.toggle(voice: .kick, index: 1)
        remote.receiveState(.disconnected)
        remote.receiveState(.connected(serverID: server, name: "Fixture", role: .operator))
        remote.receive(
            .snapshot(
                .init(
                    serverID: server, serverName: "Fixture", session: session, revision: 3,
                    trackRevisions: [session.tracks[0].id: 3])))
        drums.flush()
        await settle()
        XCTAssertEqual(sends, 0)
        XCTAssertNotNil(drums.failure)
        XCTAssertTrue(drums.hasChanges)
        drums.reload()
        remote.receiveState(.connected(serverID: server, name: "Fixture", role: .viewer))
        XCTAssertFalse(drums.canEdit)
    }
    @MainActor func testExpandedPanelIsIncludedInVerticalClipDropGeometry() throws {
        let (store, remote, session, _) = try fixture { _, _, _, _ in .init(requestID: UUID(), revision: 3) }
        defer { remote.disconnect() }
        XCTAssertNil(
            store.trackForVerticalMove(from: session.tracks[0].id, delta: store.trackSize.height),
            "A drop inside the expanded editor stays on its track")
        XCTAssertEqual(
            store.trackForVerticalMove(
                from: session.tracks[0].id, delta: store.trackSize.height + InlineDrumLayout.height),
            session.tracks[1].id)
        store.inlineDrumExpanded = false
        XCTAssertEqual(
            store.trackForVerticalMove(from: session.tracks[0].id, delta: store.trackSize.height), session.tracks[1].id)
    }
    @MainActor func testViewerCannotOpenAnInlineEditor() throws {
        let (store, remote, _, _) = try fixture(role: .viewer) { _, _, _, _ in .init(requestID: UUID(), revision: 3) }
        defer { remote.disconnect() }
        XCTAssertNil(store.inlineDrums)
    }
    @MainActor func testRemovingTheSourceRetainsDraftWithAnExitToAnotherClip() async throws {
        let (store, remote, session, _) = try fixture { _, _, _, _ in .init(requestID: UUID(), revision: 3) }
        defer { remote.disconnect() }
        let drums = try XCTUnwrap(store.inlineDrums)
        drums.editor.toggle(voice: .kick, index: 1)
        let draft = drums.pattern
        var track = session.tracks[0]
        track.clips.removeFirst()
        remote.receive(.delta(.init(revision: 4, tracks: [track], trackRevisions: [track.id: 4])))
        await settle()
        XCTAssertEqual(drums.pattern, draft)
        XCTAssertNotNil(drums.failure)
        XCTAssertFalse(drums.canEdit)
        drums.discardDraft()
        XCTAssertNil(store.inlineDrums)
        store.toggleInlineDrums(trackID: track.id, clipID: track.clips[0].id)
        XCTAssertEqual(store.inlineDrums?.clipID, track.clips[0].id)
    }
    @MainActor func testRejectedRenderRetainsTheBeatAndExplicitRetryUsesTheSameClip() async throws {
        var sends = 0
        let (store, remote, session, _) = try fixture { command, _, _, _ in
            if case .snapshot = command { return .init(requestID: UUID(), revision: 3) }
            sends += 1
            return .init(
                requestID: UUID(), status: sends == 1 ? .rejected : .accepted, revision: sends == 1 ? 3 : 4,
                message: sends == 1 ? "Render failed" : "")
        }
        defer { remote.disconnect() }
        let drums = try XCTUnwrap(store.inlineDrums)
        drums.editor.toggle(voice: .kick, index: 1)
        let draft = drums.pattern
        drums.flush()
        await settle()
        XCTAssertEqual(drums.pattern, draft)
        XCTAssertTrue(drums.hasChanges)
        XCTAssertNotNil(drums.failure)
        XCTAssertTrue(drums.canRetry)
        drums.retry()
        await settle()
        confirm(draft, track: session.tracks[0], revision: 4, remote: remote)
        await settle()
        XCTAssertEqual(sends, 2)
        XCTAssertNil(drums.failure)
        XCTAssertFalse(drums.unsettled)
    }
    @MainActor func testEditsAutomaticallyPublishOnlyTheNewestDebouncedPattern() async throws {
        let sent = expectation(description: "Automatic beat update")
        var patterns: [DrumPattern] = []
        let (store, remote, session, _) = try fixture { command, _, _, _ in
            guard case .editor(let intent) = command, intent.operation == "drums.commit", let data = intent.payload
            else { return .init(requestID: UUID(), revision: 3) }
            patterns.append(try JSONDecoder().decode(DrumPattern.self, from: data))
            sent.fulfill()
            return .init(requestID: UUID(), revision: 4)
        }
        defer { remote.disconnect() }
        let drums = try XCTUnwrap(store.inlineDrums)
        drums.editor.toggle(voice: .kick, index: 1)
        drums.editor.toggle(voice: .kick, index: 2)
        let latest = drums.pattern
        XCTAssertTrue(patterns.isEmpty)
        await fulfillment(of: [sent], timeout: 3)
        XCTAssertEqual(patterns, [latest])
        confirm(latest, track: session.tracks[0], revision: 4, remote: remote)
        await settle()
        XCTAssertFalse(drums.unsettled)
        XCTAssertTrue(store.isPlaying)
    }
    @MainActor func testFailedRenderCanBeCorrectedWithoutDiscardingTheDraft() async throws {
        var sends = 0
        let (store, remote, session, _) = try fixture { command, _, _, _ in
            if case .snapshot = command { return .init(requestID: UUID(), revision: 3) }
            sends += 1
            return .init(
                requestID: UUID(), status: sends == 1 ? .rejected : .accepted, revision: sends == 1 ? 3 : 4,
                message: sends == 1 ? "Reduce the level" : "")
        }
        defer { remote.disconnect() }
        let drums = try XCTUnwrap(store.inlineDrums)
        drums.editor.toggle(voice: .kick, index: 1)
        drums.flush()
        await settle()
        XCTAssertNotNil(drums.failure)
        XCTAssertTrue(drums.canEdit)
        drums.editor.change { $0.masterGain = 0.3 }
        let corrected = drums.pattern
        XCTAssertNil(drums.failure)
        drums.flush()
        await settle()
        confirm(corrected, track: session.tracks[0], revision: 4, remote: remote)
        await settle()
        XCTAssertFalse(drums.unsettled)
        XCTAssertEqual(drums.pattern.masterGain, 0.3)
        XCTAssertTrue(drums.pattern.steps.contains { $0.voice == .kick && $0.index == 1 })
    }
}
