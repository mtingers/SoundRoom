import XCTest
import SoundroomCore
@testable import Soundroom

final class RemoteAutomationClockTests: XCTestCase {
    @MainActor func testRemoteWriteCannotStampPointsAtPrivateCursorOrPlaybackDisplay() {
        let store = SessionStore(headless: true)
        let trackID = store.session.tracks[0].id
        store.playhead = 1200; store.clientDisplayTransport(9600)
        store.isPlaying = true
        for mode in [AutomationRecordMode.touch, .write] {
            store.setAutomationMode(mode)
            XCTAssertEqual(store.automationEditing.mode, .read)
            XCTAssertFalse(store.automationWrite)
            XCTAssertFalse(store.automationWrites)
        }
        // Stale restoration flags cannot bypass the clock authority gate.
        store.automationWrite = true; store.automationEditing.mode = .touch
        store.beginContinuousEdit("\(trackID).gain")
        store.setTrackMixValue(trackID, parameter: .volume, value: 0.5)
        XCTAssertEqual(store.session.tracks[0].gain, 0.5)
        XCTAssertTrue(store.session.tracks[0].mixAutomation.isEmpty)
        XCTAssertTrue(store.automationEditing.touches.isEmpty)
        XCTAssertEqual(store.playhead, 1200)
    }

    @MainActor func testRemoteEffectControlPreservesCurveWithoutInventingRecordedPoints() {
        let store = SessionStore(headless: true)
        let effect = RealtimeEffect(kind: .gain, parameters: ["gain": 0],
            parameterAutomation: ["gain": [.init(frame: 400, value: 0.5)]])
        store.session.tracks[0].realtimeEffects = [effect]
        store.isPlaying = true; store.playhead = 2000
        store.automationWrite = true; store.automationEditing.mode = .touch
        store.changeRealtimeEffect(effect.id, name: "Gain", coalescingKey: "gain") { $0.parameters["gain"] = 6 }
        let edited = store.session.tracks[0].realtimeEffects[0]
        XCTAssertEqual(edited.parameters["gain"], 6)
        XCTAssertEqual(edited.parameterAutomation, effect.parameterAutomation)
        XCTAssertTrue(store.automationEditing.touches.isEmpty)
        XCTAssertEqual(store.availableAutomationRecordModes, [.read])
    }

    @MainActor func testExplicitRemotePointEditingStillUsesChosenTimelinePosition() {
        let store = SessionStore(headless: true)
        store.automationTarget = .pan
        store.putCurrentAutomationPoint(frame: 4800, value: 0.75)
        XCTAssertEqual(store.session.tracks[0].mixAutomation["pan"]?.last?.frame, 4800)
        XCTAssertEqual(store.session.tracks[0].mixAutomation["pan"]?.last?.value, 0.75)
        XCTAssertFalse(store.canRecordAutomation)
    }
}
