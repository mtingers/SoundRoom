import XCTest
import SoundroomCore
@testable import Soundroom

final class TrackClipboardTests: XCTestCase {

    @MainActor private func fixture() -> SessionStore {
        let store = SessionStore(), asset = UUID()
        store.assets[asset] = AudioSamples(channels: [Array(repeating: 0.2, count: 100)])
        var track = Track(name: "Voice", color: 2)
        track.gain = 0.7; track.pan = -0.2; track.armed = true
        track.clips = [Clip(assetID: asset, name: "A", start: 100, offset: 10, length: 20, edgeFadeFrames: 0), Clip(assetID: asset, name: "B", start: 150, length: 30, edgeFadeFrames: 0)]
        track.clips[0].soundDesign = SoundDesignPattern()
        track.realtimeEffects = [RealtimeEffect(kind: .gain, amount: 0.5, automation: [AutomationPoint(frame: 0, value: 0.2), AutomationPoint(frame: 200, value: 0.8)])]
        store.session.tracks = [track]; store.choose(track: track.id); store.playhead = 300
        return store
    }

    func testAutomationMovingEarlierSeedsCurveAtZero() {
        MainActor.assumeIsolated {
            let store = fixture(); store.copyTrack(includeAutomation: true); store.playhead = 0; store.paste()
            let effect = store.session.tracks[1].realtimeEffects[0]
            XCTAssertEqual(effect.automation.map(\.frame), [0, 100])
            XCTAssertEqual(effect.value(at: 0), 0.5, accuracy: 0.0001)
            XCTAssertEqual(effect.value(at: 50), 0.65, accuracy: 0.0001)
        }
    }

    func testNormalClipCopyReplacesTrackClipboardAndPastesIntoExistingTrack() {
        MainActor.assumeIsolated {
            let store = fixture(); store.copyTrack(); store.choose(track: store.session.tracks[0].id, clip: store.session.tracks[0].clips[1].id)
            store.copy(); XCTAssertFalse(store.canPasteTrack); store.playhead = 500; store.paste()
            XCTAssertEqual(store.session.tracks.count, 1); XCTAssertEqual(store.session.tracks[0].clips.last?.start, 500)
        }
    }
    func testRepeatedClipPasteLaysCopiesEndToEndUntilTheCursorMoves() {
        MainActor.assumeIsolated {
            let store = fixture(), track = store.session.tracks[0]
            store.choose(track: track.id, clip: track.clips[1].id); store.copy()
            store.paste(); store.paste()
            XCTAssertEqual(store.session.tracks[0].clips.suffix(2).map(\.start), [180, 210], "Pastes follow the copied clip (150..<180) end to end")
            store.seek(400); store.paste()
            XCTAssertEqual(store.session.tracks[0].clips.last?.start, 400, "A placed cursor wins")
            store.paste()
            XCTAssertEqual(store.session.tracks[0].clips.last?.start, 430)
        }
    }
    func testChainedPasteContinuesAfterTheLatestSurvivingPasteWhenOneIsRolledBack() {
        MainActor.assumeIsolated {
            let store = fixture(), track = store.session.tracks[0]
            store.choose(track: track.id, clip: track.clips[1].id); store.copy()
            store.paste(); store.paste(); store.paste()
            XCTAssertEqual(store.session.tracks[0].clips.suffix(3).map(\.start), [180, 210, 240])
            // A server rejection restores the authoritative track without the
            // last two pastes, leaving the selection on the rolled-back range.
            store.session.tracks[0].clips.removeLast(2)
            store.paste()
            XCTAssertEqual(store.session.tracks[0].clips.last?.start, 210, "No gap after the surviving paste")
        }
    }
    func testTabMovesCursorToNextAndPreviousClipEdge() {
        MainActor.assumeIsolated {
            let store = fixture()
            store.playhead = 0
            store.jumpToClipEdge(forward: true); XCTAssertEqual(store.playhead, 100)
            store.jumpToClipEdge(forward: true); XCTAssertEqual(store.playhead, 120)
            store.jumpToClipEdge(forward: true); XCTAssertEqual(store.playhead, 150)
            store.jumpToClipEdge(forward: false); XCTAssertEqual(store.playhead, 120)
            store.playhead = 180; store.jumpToClipEdge(forward: true); XCTAssertEqual(store.playhead, 180, "No edge after the last clip")
        }
    }

    func testBusyAndRecordingBlockTrackPasteAndCopy() {
        MainActor.assumeIsolated {
            let store = fixture(); store.copyTrack(); let original = store.session
            store.busy = true; store.paste(); XCTAssertEqual(store.session, original)
            store.busy = false; store.isRecording = true; store.paste(); XCTAssertEqual(store.session, original)
        }
    }
}
