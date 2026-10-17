import AppKit
import Combine
import SoundroomCore
import SoundroomRemote
import XCTest

@testable import Soundroom

final class RemoteGUIStateTests: XCTestCase {
    @MainActor func testControlOnlyDeltasDoNotRepublishClipSelectionInDenseArrangements() throws {
        let store = SessionStore(headless: true)
        let remote = try client(store)
        var session = Session()
        session.tracks[0].clips = (0..<10_000).map {
            Clip(assetID: UUID(), name: "Region \($0)", start: $0 * 48_000, length: 48_000)
        }
        let track = session.tracks[0]
        let clip = track.clips[500]
        remote.receive(.snapshot(.init(serverID: UUID(), serverName: "Host", session: session)))
        store.choose(track: track.id, clip: clip.id)
        var selectionPublications = 0
        let observer = store.$selectedClips.dropFirst().sink { _ in selectionPublications += 1 }
        defer { observer.cancel() }
        for revision in 1...120 {
            remote.receive(
                .delta(
                    .init(
                        revision: revision, controlPatches: [.init(trackID: track.id, gain: Float(revision) / 120)],
                        trackRevisions: [track.id: revision])))
        }
        XCTAssertEqual(
            selectionPublications, 0, "A mixer update must not rescan or republish unrelated region selection")
        XCTAssertEqual(store.selectedClip, clip.id)
        XCTAssertEqual(store.selectedClips, [clip.id])
        var replacement = track
        replacement.clips = []
        remote.receive(.delta(.init(revision: 121, tracks: [replacement], trackRevisions: [track.id: 121])))
        XCTAssertNil(store.selectedClip, "Structural updates still remove stale selections")
        XCTAssertTrue(store.selectedClips.isEmpty)
    }
    @MainActor func testEditorWindowDoesNotProbeTheDynamicTimelineForIntrinsicSizing() {
        let store = SessionStore(headless: true)
        let host = EditorHostingView(rootView: MainView(store: store))
        XCTAssertTrue(host.sizingOptions.isEmpty, "The window's explicit limits must own editor sizing")
        XCTAssertTrue(host.acceptsFirstResponder)
    }
    @MainActor func testRecordingTelemetryUpdatesOnlyInputDisplayViews() throws {
        let store = SessionStore(headless: true)
        let remote = try client(store)
        store.session.tracks[0].clips = [Clip(assetID: UUID(), name: "Telemetry fixture", start: 0, length: 96_000)]
        remote.receive(.transport(.init(frame: 0, isPlaying: true, isRecording: true)))
        var editorInvalidations = 0
        var inputInvalidations = 0
        let editor = store.objectWillChange.sink { editorInvalidations += 1 }
        let input = store.inputActivity.objectWillChange.sink { inputInvalidations += 1 }
        defer {
            editor.cancel()
            input.cancel()
        }
        for tick in 1...120 {
            remote.receive(.transport(.init(frame: tick * 256, isPlaying: true, isRecording: true)))
            remote.receive(.meters(left: 0.2, right: 0.1, input: Float(tick) / 120))
        }
        XCTAssertEqual(editorInvalidations, 0, "Recording ticks must not redraw timeline clips and effect controls")
        XCTAssertGreaterThanOrEqual(inputInvalidations, 120, "Armed meters and live waveform must still update")
        XCTAssertEqual(store.inputPeaks, [1])
        XCTAssertEqual(store.liveInputHistory.channels.first?.count, 120)
        XCTAssertEqual(store.playbackPosition.frame, 120 * 256)
    }
    @MainActor func testMonitoringTelemetryDoesNotInvalidateEditorOrGrowTakeHistory() throws {
        let store = SessionStore(headless: true)
        store.inputMonitoring = true
        let remote = try client(store)
        var invalidations = 0
        let observer = store.objectWillChange.sink { invalidations += 1 }
        defer { observer.cancel() }
        for tick in 1...120 { remote.receive(.meters(left: 0, right: 0, input: Float(tick) / 120)) }
        XCTAssertEqual(invalidations, 0)
        XCTAssertEqual(store.inputPeaks, [1])
        XCTAssertTrue(store.liveInputHistory.channels.isEmpty)
    }
    @MainActor func testNewRecordingClearsPreviousLiveWaveform() throws {
        let store = SessionStore(headless: true)
        let remote = try client(store)
        remote.receive(.transport(.init(frame: 0, isPlaying: true, isRecording: true)))
        remote.receive(.meters(left: 0, right: 0, input: 0.1))
        remote.receive(.meters(left: 0, right: 0, input: 0.2))
        remote.receive(.transport(.init(frame: 0)))
        remote.receive(.transport(.init(frame: 0, isPlaying: true, isRecording: true)))
        XCTAssertTrue(store.liveInputHistory.channels.isEmpty)
        remote.receive(.meters(left: 0, right: 0, input: 0.3))
        XCTAssertEqual(store.liveInputHistory.channels, [[0.3]])
    }

    @MainActor func testSparseDeltasPreserveDenseDraftsAndRejectOlderRevisions() throws {
        let store = SessionStore()
        let remote = try client(store)
        var session = Session()
        var pattern = SoundDesignPattern()
        pattern.notes = Array(repeating: .init(pitch: 60, startBeat: 0), count: 10_000)
        var clip = Clip(assetID: UUID(), name: "Dense MIDI", start: 0, length: 48_000)
        clip.soundDesign = pattern
        session.tracks[0].clips = [clip]
        session.tracks[0].mixAutomation["gain"] = Array(repeating: .init(frame: 0, value: 0.5), count: 10_000)
        let id = session.tracks[0].id
        remote.receive(.snapshot(.init(serverID: UUID(), serverName: "Host", session: session, revision: 4)))
        store.selectedTrack = id
        store.selectedClip = clip.id
        store.selectedClips = [clip.id]
        store.zoom = 420
        store.playhead = 12_345
        remote.receive(
            .delta(
                .init(
                    revision: 5, controlPatches: [.init(trackID: id, gain: 0, pan: -0.5)], trackRevisions: [id: 5],
                    status: "Latest")))
        XCTAssertEqual(store.session.tracks[0].gain, 0)
        XCTAssertEqual(store.session.tracks[0].pan, -0.5)
        XCTAssertEqual(store.session.tracks[0].clips, session.tracks[0].clips)
        XCTAssertEqual(store.session.tracks[0].mixAutomation, session.tracks[0].mixAutomation)
        XCTAssertEqual(store.selectedClip, clip.id)
        XCTAssertEqual(store.selectedClips, [clip.id])
        XCTAssertEqual(store.zoom, 420)
        XCTAssertEqual(store.playhead, 12_345)
        remote.receive(
            .delta(
                .init(
                    revision: 3, controlPatches: [.init(trackID: id, gain: 4)], trackRevisions: [id: 3], status: "Stale"
                )))
        XCTAssertEqual(remote.revision, 5)
        XCTAssertEqual(remote.trackRevisions[id], 5)
        XCTAssertEqual(store.session.tracks[0].gain, 0)
        XCTAssertEqual(store.status, "Latest")
    }
    @MainActor private func client(_ store: SessionStore) throws -> RemoteGUIController {
        let remote = RemoteGUIController(store: store, credential: try RemoteCredential.generate(), host: "127.0.0.1")
        store.remoteGUI = remote
        return remote
    }
    @MainActor func testSnapshotAndAcceptedEditsPreserveIndependentViews() throws {
        let first = SessionStore()
        let second = SessionStore()
        let a = try client(first)
        let b = try client(second)
        var session = Session()
        session.tracks.append(Track(name: "Bass", color: 1))
        let serverID = UUID()
        let snapshot = RemoteSnapshot(serverID: serverID, serverName: "Hardware Mac", session: session)
        a.receive(.snapshot(snapshot))
        b.receive(.snapshot(snapshot))
        first.selectedTrack = session.tracks[0].id
        first.zoom = 420
        second.selectedTrack = session.tracks[1].id
        second.zoom = 35
        var edited = session.tracks[1]
        edited.pan = -0.25
        edited.name = "Organic bass"
        let delta = RemoteSessionDelta(revision: 1, tracks: [edited], trackRevisions: [edited.id: 1])
        a.receive(.delta(delta))
        b.receive(.delta(delta))
        XCTAssertEqual(first.session.tracks[1], edited)
        XCTAssertEqual(second.session.tracks[1], edited)
        XCTAssertEqual(first.selectedTrack, session.tracks[0].id)
        XCTAssertEqual(second.selectedTrack, session.tracks[1].id)
        XCTAssertEqual(first.zoom, 420)
        XCTAssertEqual(second.zoom, 35)
        a.receive(.snapshot(.init(serverID: serverID, serverName: "Hardware Mac", session: first.session, revision: 1)))
        XCTAssertEqual(first.selectedTrack, session.tracks[0].id)
        XCTAssertEqual(first.zoom, 420)
    }
    @MainActor func testDeletingForeignSelectionInvalidatesOnlyRemovedIDs() throws {
        let store = SessionStore()
        let remote = try client(store)
        var session = Session()
        let asset = UUID()
        let first = Clip(assetID: asset, name: "One", start: 0, length: 100)
        let second = Clip(assetID: asset, name: "Two", start: 100, length: 100)
        session.tracks[0].clips = [first, second]
        remote.receive(.snapshot(.init(serverID: UUID(), serverName: "Host", session: session)))
        store.selectedTrack = session.tracks[0].id
        store.selectedClip = second.id
        store.selectedClips = [first.id, second.id]
        var edited = session.tracks[0]
        edited.clips = [first]
        remote.receive(.delta(.init(revision: 1, tracks: [edited])))
        XCTAssertNil(store.selectedClip)
        XCTAssertEqual(store.selectedClips, [first.id])
        XCTAssertEqual(store.selectedTrack, edited.id)
    }
    @MainActor func testWaveformAndTransportAreDisplayDataOnly() throws {
        let store = SessionStore()
        let remote = try client(store)
        let id = UUID()
        let asset = RemoteAsset(id: id, frames: 48_000 * 60 * 60, channels: 2, peaks: [-0.4, 0.6, -0.2, 0.3])
        remote.receive(.waveform(asset))
        let metadata = try XCTUnwrap(store.assets[id])
        XCTAssertTrue(metadata.isDisplayMetadata)
        XCTAssertEqual(metadata.count, asset.frames)
        XCTAssertEqual(metadata.channelCount, 2)
        XCTAssertTrue(metadata.channels.isEmpty)
        XCTAssertEqual(metadata.readFrames(0..<metadata.count), [[], []])
        let envelope = try XCTUnwrap(remote.waveform(id))
        XCTAssertEqual(envelope.count, 2)
        XCTAssertEqual(envelope[0].0, -0.4)
        store.zoom = 500
        remote.receive(.transport(.init(frame: 48_000, isPlaying: true)))
        XCTAssertTrue(store.isPlaying)
        XCTAssertEqual(store.playbackPosition.frame, 48_000)
        XCTAssertEqual(store.zoom, 500)
    }
    @MainActor func testSteadyPlaybackTelemetryDoesNotInvalidateEditor() throws {
        let store = SessionStore()
        let remote = try client(store)
        store.session.tracks[0].clips = [Clip(assetID: UUID(), name: "Playback fixture", start: 0, length: 96_000)]
        remote.receive(.transport(.init(frame: 0, isPlaying: true)))
        var invalidations = 0
        let observer = store.objectWillChange.sink { invalidations += 1 }
        defer { observer.cancel() }
        for frame in stride(from: 2400, through: 48_000, by: 2400) {
            remote.receive(.transport(.init(frame: frame, isPlaying: true)))
            remote.receive(.meters(left: Float(frame) / 48_000, right: 0.1, input: 0))
        }
        XCTAssertEqual(invalidations, 0, "Playback ticks must update only the clock and meter child views")
        XCTAssertEqual(store.playbackPosition.frame, 48_000)
    }
    @MainActor func testFaderAndControlPatchesDoNotSerializeLongMIDIArrangements() throws {
        var pattern = SoundDesignPattern()
        pattern.notes = (0..<10_000).map { _ in SoundDesignNote(pitch: 60, startBeat: 0, durationBeats: 1) }
        var before = Track(name: "Long MIDI", color: 0)
        var clip = Clip(assetID: UUID(), name: "Sequence", start: 0, length: 48_000)
        clip.soundDesign = pattern
        before.clips = [clip]
        var after = before
        after.gain = 0.5
        let fader = RemoteGUIController.controlCommand(before: before, after: after, name: "Volume", gesture: "gain")
        if case .setTrackGain(let id, let gain) = fader {
            XCTAssertEqual(id, before.id)
            XCTAssertEqual(gain, 0.5)
        } else {
            XCTFail("A fader must use the small value command")
        }
        XCTAssertLessThan(try JSONEncoder().encode(fader).count, 256)
        after.realtimeEffects = [RealtimeEffect(kind: .room)]
        let controls = RemoteTrackControls(track: after)
        var mirrored = before
        controls.apply(to: &mirrored)
        XCTAssertEqual(mirrored.clips, before.clips)
        XCTAssertEqual(mirrored.realtimeEffects, after.realtimeEffects)
        XCTAssertLessThan(try JSONEncoder().encode(controls).count, 4096)
    }
    @MainActor func testPlaybackAndPausePreserveEachClientsEditCursor() throws {
        let first = SessionStore()
        let second = SessionStore()
        let a = try client(first)
        let b = try client(second)
        first.playhead = 1200
        second.playhead = 9600
        for remote in [a, b] { remote.receive(.transport(.init(frame: 4800, isPlaying: true))) }
        XCTAssertEqual(first.playhead, 1200)
        XCTAssertEqual(second.playhead, 9600)
        XCTAssertEqual(first.playbackPosition.frame, 4800)
        XCTAssertEqual(second.playbackPosition.frame, 4800)
        for remote in [a, b] { remote.receive(.transport(.init(frame: 7200, isPlaying: false))) }
        XCTAssertEqual(first.playhead, 1200)
        XCTAssertEqual(second.playhead, 9600)
        XCTAssertEqual(first.playbackPosition.frame, 7200)
        XCTAssertEqual(second.playbackPosition.frame, 7200)
    }
    @MainActor func testDisconnectedPreviewsResolveWithoutOpeningClientAudio() throws {
        let store = SessionStore()
        let remote = try client(store)
        XCTAssertFalse(remote.connected)
        store.drumDesigner.preview(DrumPattern())
        XCTAssertFalse(store.drumDesigner.previewing)
        XCTAssertFalse(store.drumDesigner.preparing)
        XCTAssertFalse(store.drumDesigner.message.isEmpty)
        let clip = Clip(assetID: UUID(), name: "Take", start: 0, length: 48_000)
        let take = RecordingTake(name: "Recorded take", clips: [clip])
        store.session.tracks[0].recordingTakes = [take]
        let audition = TakeAudition()
        audition.toggle(store: store, takeID: take.id, range: nil)
        XCTAssertFalse(audition.playing)
        XCTAssertFalse(audition.preparing)
        XCTAssertNotNil(audition.error)
    }
    @MainActor func testAuditionStopsRemoveUnsentPreviewsButPreserveQueuedEdits() async throws {
        let credential = try RemoteCredential.generate()
        let server = RemoteServer(credential: credential)
        let store = SessionStore()
        let remote = RemoteGUIController(store: store, credential: credential, host: "127.0.0.1")
        store.remoteGUI = remote
        remote.client.autoReconnect = false
        var listenerError: String?
        var holding = false
        var operations: [String] = []
        server.onError = { listenerError = $0 }
        server.onConnect = { _ in .init(serverID: credential.serverID, serverName: "Queue test", session: store.session)
        }
        server.onRequest = { _, request in
            if case .editor(let intent) = request.command {
                operations.append(intent.operation)
                if intent.operation == "test.hold" {
                    holding = true
                    try? await Task.sleep(nanoseconds: 300_000_000)
                }
            } else if case .setTrackPan = request.command {
                operations.append("pan")
            }
            return .init(requestID: request.id, revision: 0)
        }
        try server.start(port: 0)
        defer {
            remote.disconnect()
            server.stop()
        }
        for _ in 0..<500 {
            if server.port > 0 { break }
            if let listenerError {
                if listenerError.contains("Operation not permitted") {
                    throw XCTSkip("Network listeners are unavailable in this execution sandbox")
                }
                throw RemoteError.rejected(listenerError)
            }
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        guard server.port > 0 else { throw RemoteError.timeout }
        try await remote.client.connect(host: "127.0.0.1", port: server.port, timeout: 5)
        remote.send(.editor(.init(operation: "test.hold")))
        for _ in 0..<200 {
            if holding { break }
            try await Task.sleep(nanoseconds: 5_000_000)
        }
        XCTAssertTrue(holding)
        var cancelled = 0
        for operation in ["designer.preview", "designer.configure", "designer.noteOn", "drums.preview", "take.preview"]
        {
            remote.send(.editor(.init(operation: operation))) { reply in if !reply.accepted { cancelled += 1 } }
        }
        remote.send(.setTrackPan(trackID: store.session.tracks[0].id, pan: 0.2))
        for operation in ["designer.stop", "drums.stop", "take.stop"] { remote.route(.init(operation: operation)) }
        XCTAssertEqual(cancelled, 5, "Stop must resolve callbacks for every cancelled unsent audition")
        for _ in 0..<300 {
            if operations.contains("pan"), operations.contains("take.stop") { break }
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        XCTAssertTrue(operations.contains("pan"), "Stopping previews must preserve unrelated queued document edits")
        XCTAssertFalse(operations.contains("designer.preview"))
        XCTAssertFalse(operations.contains("designer.configure"))
        XCTAssertFalse(operations.contains("designer.noteOn"))
        XCTAssertFalse(operations.contains("drums.preview"))
        XCTAssertFalse(operations.contains("take.preview"))
    }
}
