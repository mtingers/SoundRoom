import SoundroomCore
import SoundroomRemote
import XCTest

@testable import Soundroom

final class InlineSoundTests: XCTestCase {
    @MainActor private func settle() async { for _ in 0..<100 { await Task.yield() } }
    @MainActor private func fixture(role: RemoteRole = .operator, sender: @escaping RemoteGUIController.CommandSender)
        throws -> (SessionStore, RemoteGUIController, Session, UUID)
    {
        let store = SessionStore(headless: true)
        let server = UUID()
        var session = Session()
        let pattern = SoundDesignPattern()
        var clip = Clip(assetID: UUID(), name: "Verse keys", start: 4_800, length: 192_000)
        clip.soundDesign = pattern
        var copy = clip
        copy.id = UUID()
        copy.start = 384_000
        copy.name = "Chorus keys"
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
        store.toggleInlineSound(trackID: session.tracks[0].id, clipID: clip.id)
        return (store, remote, session, server)
    }
    @MainActor private func confirm(
        _ pattern: SoundDesignPattern, track original: Track, revision: Int, remote: RemoteGUIController
    ) {
        var track = original
        track.clips[0].soundDesign = pattern
        track.clips[0].assetID = UUID()
        remote.receive(.delta(.init(revision: revision, tracks: [track], trackRevisions: [track.id: revision])))
    }
    @MainActor func testOpeningAndCollapsingNeverMovesOrStopsTransportAndPinsTheClip() throws {
        let (store, remote, session, _) = try fixture { _, _, _, _ in .init(requestID: UUID(), revision: 3) }
        defer { remote.disconnect() }
        let sound = try XCTUnwrap(store.inlineSound)
        XCTAssertTrue(store.isPlaying)
        XCTAssertEqual(store.playhead, 80_000)
        store.selectedTrack = session.tracks[1].id
        store.selectedClip = nil
        XCTAssertEqual(sound.trackID, session.tracks[0].id)
        XCTAssertEqual(sound.clipID, session.tracks[0].clips[0].id)
        XCTAssertEqual(store.inlineSoundHeight(for: session.tracks[0]), InlineSoundLayout.height)
        store.toggleInlineSound(trackID: sound.trackID)
        XCTAssertFalse(store.inlineSoundExpanded)
        XCTAssertEqual(store.inlineSoundHeight(for: session.tracks[0]), 0)
        XCTAssertTrue(store.isPlaying)
        XCTAssertEqual(store.playhead, 80_000)
    }
    @MainActor func testRapidChangesStayResponsiveAndSendOnlyOneRenderAtATime() async throws {
        var sent: [(RemoteEditorIntent, [UUID: Int])] = []
        let (store, remote, session, _) = try fixture { command, versions, _, _ in
            if case .editor(let intent) = command, intent.operation == "designer.commit" {
                sent.append((intent, versions))
                return .init(requestID: UUID(), revision: 3 + sent.count)
            }
            return .init(requestID: UUID(), revision: 3)
        }
        defer { remote.disconnect() }
        let sound = try XCTUnwrap(store.inlineSound)
        let track = session.tracks[0]
        sound.change { $0.tone = $0.tone == 0.4 ? 0.65 : 0.4 }
        let first = sound.pattern
        sound.flush()
        await settle()
        XCTAssertEqual(sent.count, 1)
        XCTAssertTrue(sound.sending)
        XCTAssertTrue(sound.canEdit, "Rendering must keep the grid responsive")
        sound.change { $0.attack = 0.2 }
        let latest = sound.pattern
        sound.flush()
        XCTAssertEqual(sent.count, 1, "An in-flight render must not disable the editor or queue old drafts")
        store.selectedTrack = session.tracks[1].id
        store.selectedClip = nil
        confirm(first, track: track, revision: 4, remote: remote)
        await settle()
        XCTAssertEqual(sound.pattern, latest, "An older ack must not pull controls backwards")
        sound.flush()
        await settle()
        XCTAssertEqual(sent.count, 2)
        XCTAssertEqual(sent[0].1, [track.id: 3])
        XCTAssertEqual(sent[1].1, [track.id: 4])
        XCTAssertEqual(sent[1].0.context.clipID, track.clips[0].id)
        XCTAssertEqual(sent[1].0.integers["frame"], track.clips[0].start)
        XCTAssertEqual(try JSONDecoder().decode(SoundDesignPattern.self, from: XCTUnwrap(sent[1].0.payload)), latest)
        confirm(latest, track: track, revision: 5, remote: remote)
        await settle()
        XCTAssertFalse(sound.unsettled)
        XCTAssertNil(sound.failure)
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
        let sound = try XCTUnwrap(store.inlineSound)
        let original = sound.pattern
        sound.change { $0.tone = $0.tone == 0.4 ? 0.65 : 0.4 }
        let first = sound.pattern
        sound.flush()
        await settle()
        sound.change { $0.tone = $0.tone == 0.4 ? 0.65 : 0.4 }
        XCTAssertEqual(sound.pattern, original)
        confirm(first, track: session.tracks[0], revision: 4, remote: remote)
        await settle()
        XCTAssertTrue(sound.hasChanges)
        sound.flush()
        await settle()
        confirm(original, track: session.tracks[0], revision: 5, remote: remote)
        await settle()
        XCTAssertFalse(sound.unsettled)
        XCTAssertEqual(sound.pattern, original)
    }
    @MainActor func testForeignClipChangeRetainsTheDraftAndCannotOverwriteTheNewSource() async throws {
        var sends = 0
        let (store, remote, session, _) = try fixture { _, _, _, _ in
            sends += 1
            return .init(requestID: UUID(), revision: 4)
        }
        defer { remote.disconnect() }
        let sound = try XCTUnwrap(store.inlineSound)
        sound.change { $0.tone = $0.tone == 0.4 ? 0.65 : 0.4 }
        let draft = sound.pattern
        var foreign = session.tracks[0].clips[0].soundDesign!
        foreign.bpm = 90
        confirm(foreign, track: session.tracks[0], revision: 4, remote: remote)
        await settle()
        sound.flush()
        await settle()
        XCTAssertEqual(sends, 0)
        XCTAssertEqual(sound.pattern, draft)
        XCTAssertNotNil(sound.failure)
        XCTAssertFalse(sound.canRetry, "Retry cannot silently rebase onto a changed clip")
        sound.reload()
        XCTAssertEqual(sound.pattern, foreign)
        XCTAssertFalse(sound.hasChanges)
        XCTAssertNil(sound.failure)
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
        let sound = try XCTUnwrap(store.inlineSound)
        sound.change { $0.tone = $0.tone == 0.4 ? 0.65 : 0.4 }
        let first = sound.pattern
        sound.flush()
        await settle()
        sound.change { $0.attack = 0.2 }
        let last = sound.pattern
        store.undo()
        XCTAssertEqual(undos, 0)
        confirm(first, track: session.tracks[0], revision: 4, remote: remote)
        await settle()
        sound.flush()
        await settle()
        confirm(last, track: session.tracks[0], revision: 5, remote: remote)
        await settle()
        XCTAssertEqual(undos, 1)
        XCTAssertFalse(sound.unsettled)
        confirm(first, track: session.tracks[0], revision: 6, remote: remote)
        await settle()
        XCTAssertEqual(sound.pattern, first)
        XCTAssertFalse(sound.unsettled)
    }
    @MainActor func testReconnectAndPermissionChangesCannotAuthorizeAnOldDraft() async throws {
        var sends = 0
        let (store, remote, session, server) = try fixture { _, _, _, _ in
            sends += 1
            return .init(requestID: UUID(), revision: 4)
        }
        defer { remote.disconnect() }
        let sound = try XCTUnwrap(store.inlineSound)
        sound.change { $0.tone = $0.tone == 0.4 ? 0.65 : 0.4 }
        remote.receiveState(.disconnected)
        remote.receiveState(.connected(serverID: server, name: "Fixture", role: .operator))
        remote.receive(
            .snapshot(
                .init(
                    serverID: server, serverName: "Fixture", session: session, revision: 3,
                    trackRevisions: [session.tracks[0].id: 3])))
        sound.flush()
        await settle()
        XCTAssertEqual(sends, 0)
        XCTAssertNotNil(sound.failure)
        XCTAssertTrue(sound.hasChanges)
        sound.reload()
        remote.receiveState(.connected(serverID: server, name: "Fixture", role: .viewer))
        XCTAssertFalse(sound.canEdit)
    }
    @MainActor func testExpandedPanelIsIncludedInVerticalClipDropGeometry() throws {
        let (store, remote, session, _) = try fixture { _, _, _, _ in .init(requestID: UUID(), revision: 3) }
        defer { remote.disconnect() }
        XCTAssertNil(
            store.trackForVerticalMove(from: session.tracks[0].id, delta: store.trackSize.height),
            "A drop inside the expanded editor stays on its track")
        XCTAssertEqual(
            store.trackForVerticalMove(
                from: session.tracks[0].id, delta: store.trackSize.height + InlineSoundLayout.height),
            session.tracks[1].id)
        store.inlineSoundExpanded = false
        XCTAssertEqual(
            store.trackForVerticalMove(from: session.tracks[0].id, delta: store.trackSize.height), session.tracks[1].id)
    }
    @MainActor func testViewerCannotOpenAnInlineEditor() throws {
        let (store, remote, _, _) = try fixture(role: .viewer) { _, _, _, _ in .init(requestID: UUID(), revision: 3) }
        defer { remote.disconnect() }
        XCTAssertNil(store.inlineSound)
    }
    @MainActor func testRemovingTheSourceRetainsDraftWithAnExitToAnotherClip() async throws {
        let (store, remote, session, _) = try fixture { _, _, _, _ in .init(requestID: UUID(), revision: 3) }
        defer { remote.disconnect() }
        let sound = try XCTUnwrap(store.inlineSound)
        sound.change { $0.tone = $0.tone == 0.4 ? 0.65 : 0.4 }
        let draft = sound.pattern
        var track = session.tracks[0]
        track.clips.removeFirst()
        remote.receive(.delta(.init(revision: 4, tracks: [track], trackRevisions: [track.id: 4])))
        await settle()
        XCTAssertEqual(sound.pattern, draft)
        XCTAssertNotNil(sound.failure)
        XCTAssertFalse(sound.canEdit)
        sound.discardDraft()
        XCTAssertNil(store.inlineSound)
        store.toggleInlineSound(trackID: track.id, clipID: track.clips[0].id)
        XCTAssertEqual(store.inlineSound?.clipID, track.clips[0].id)
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
        let sound = try XCTUnwrap(store.inlineSound)
        sound.change { $0.tone = $0.tone == 0.4 ? 0.65 : 0.4 }
        let draft = sound.pattern
        sound.flush()
        await settle()
        XCTAssertEqual(sound.pattern, draft)
        XCTAssertTrue(sound.hasChanges)
        XCTAssertNotNil(sound.failure)
        XCTAssertTrue(sound.canRetry)
        sound.retry()
        await settle()
        confirm(draft, track: session.tracks[0], revision: 4, remote: remote)
        await settle()
        XCTAssertEqual(sends, 2)
        XCTAssertNil(sound.failure)
        XCTAssertFalse(sound.unsettled)
    }
    @MainActor func testEditsAutomaticallyPublishOnlyTheNewestDebouncedPattern() async throws {
        let sent = expectation(description: "Automatic beat update")
        var patterns: [SoundDesignPattern] = []
        let (store, remote, session, _) = try fixture { command, _, _, _ in
            guard case .editor(let intent) = command, intent.operation == "designer.commit", let data = intent.payload
            else { return .init(requestID: UUID(), revision: 3) }
            patterns.append(try JSONDecoder().decode(SoundDesignPattern.self, from: data))
            sent.fulfill()
            return .init(requestID: UUID(), revision: 4)
        }
        defer { remote.disconnect() }
        let sound = try XCTUnwrap(store.inlineSound)
        sound.change { $0.tone = $0.tone == 0.4 ? 0.65 : 0.4 }
        sound.change { $0.attack = 0.2 }
        let latest = sound.pattern
        XCTAssertTrue(patterns.isEmpty)
        await fulfillment(of: [sent], timeout: 3)
        XCTAssertEqual(patterns, [latest])
        confirm(latest, track: session.tracks[0], revision: 4, remote: remote)
        await settle()
        XCTAssertFalse(sound.unsettled)
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
        let sound = try XCTUnwrap(store.inlineSound)
        sound.change { $0.tone = $0.tone == 0.4 ? 0.65 : 0.4 }
        sound.flush()
        await settle()
        XCTAssertNotNil(sound.failure)
        XCTAssertTrue(sound.canEdit)
        sound.change { $0.baseGain = 0.3 }
        let corrected = sound.pattern
        XCTAssertNil(sound.failure)
        sound.flush()
        await settle()
        confirm(corrected, track: session.tracks[0], revision: 4, remote: remote)
        await settle()
        XCTAssertFalse(sound.unsettled)
        XCTAssertEqual(sound.pattern.baseGain, 0.3)
        XCTAssertEqual(sound.pattern.tone, 0.4)
    }
    @MainActor func testNoteGesturesWaitForReleaseAndCopyPasteKeepsIndependentIDs() async throws {
        var sends = 0
        let (store, remote, session, _) = try fixture { _, _, _, _ in
            sends += 1
            return .init(requestID: UUID(), revision: 4)
        }
        defer { remote.disconnect() }
        let sound = try XCTUnwrap(store.inlineSound)
        let original = sound.pattern
        sound.selectedNotes = Set(original.notes.prefix(2).map(\.id))
        sound.beginGesture()
        sound.drag(by: 0.5, semitones: 2, resizing: false)
        sound.flush()
        await settle()
        XCTAssertEqual(sends, 0)
        XCTAssertEqual(sound.pattern.notes[0].startBeat, original.notes[0].startBeat + 0.5)
        XCTAssertEqual(sound.pattern.notes[0].pitch, original.notes[0].pitch + 2)
        sound.copyNotes()
        sound.endGesture()
        let moved = sound.pattern
        sound.flush()
        await settle()
        XCTAssertEqual(sends, 1)
        confirm(moved, track: session.tracks[0], revision: 4, remote: remote)
        await settle()
        sound.pasteNotes(at: 2)
        XCTAssertEqual(sound.pattern.notes.count, original.notes.count + 2)
        XCTAssertTrue(Set(sound.pattern.notes.suffix(2).map(\.id)).isDisjoint(with: Set(original.notes.map(\.id))))
        sound.resizeSequence(1)
        XCTAssertTrue(sound.pattern.notes.allSatisfy { $0.startBeat + $0.durationBeats <= 1 })
    }
}
