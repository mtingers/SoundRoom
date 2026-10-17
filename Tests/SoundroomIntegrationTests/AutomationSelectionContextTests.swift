import SoundroomCore
import XCTest

@testable import Soundroom

@MainActor final class AutomationSelectionContextTests: XCTestCase {
    private let points: [AutomationPoint] = [.init(frame: 50, value: 0.3), .init(frame: 150, value: 0.7)]

    private func fixture() -> SessionStore {
        let store = SessionStore()
        var first = Track(name: "Voice", color: 0)
        var second = Track(name: "Music", color: 1)
        first.mixAutomation["volume"] = points
        first.mixAutomation["pan"] = points
        second.mixAutomation["volume"] = points
        second.mixAutomation["pan"] = points
        store.session.tracks = [first, second]
        store.selectedTrack = first.id
        store.automationTarget = .volume
        store.effectMode = .realtime
        store.selectAutomationRange(0..<200)
        return store
    }

    private func assertReset(_ store: SessionStore, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertTrue(store.automationEditing.selectedFrames.isEmpty, file: file, line: line)
        XCTAssertNil(store.automationEditing.range, file: file, line: line)
        XCTAssertFalse(store.automationEditing.hasFocus, file: file, line: line)
    }

    func testDirectTrackChangeToEmptyLaneClearsRangeAndFocus() {
        let store = fixture()
        store.session.tracks[1].mixAutomation["volume"] = []
        store.selectedTrack = store.session.tracks[1].id
        assertReset(store)
        XCTAssertTrue(store.automationPoints.isEmpty)
    }

    func testInsertSwitchAndRemovalResetEvenWhenParametersAndFramesMatch() {
        let store = fixture()
        var a = RealtimeEffect(kind: .gain, parameters: ["gain": 0])
        a.selectedAutomationParameter = "gain"
        a.parameterAutomation["gain"] = points
        var b = RealtimeEffect(kind: .gain, parameters: ["gain": -6])
        b.selectedAutomationParameter = "gain"
        b.parameterAutomation["gain"] = points
        store.session.tracks[0].realtimeEffects = [a, b]
        store.automationTarget = .effect
        store.selectedRealtimeEffect = a.id
        store.selectAutomationRange(0..<200)
        store.selectedRealtimeEffect = b.id
        assertReset(store)
        let original = store.session
        store.deleteSelectedAutomationPoints()
        XCTAssertEqual(store.session, original)
        store.selectAutomationRange(0..<200)
        // A removed selected insert falls back to the remaining first insert.
        store.session.tracks[0].realtimeEffects.removeAll { $0.id == b.id }
        assertReset(store)
        XCTAssertEqual(store.currentRealtimeEffect?.id, a.id)
    }

    func testOrdinaryPointEditsAndUnrelatedParameterChangesPreserveSelection() {
        let store = fixture()
        let track = store.session.tracks[0].id
        store.putCurrentAutomationPoint(frame: 50, value: 0.6, replacing: 50)
        XCTAssertEqual(store.automationEditing.selectedFrames, [50, 150])
        XCTAssertEqual(store.automationEditing.range, 0..<200)
        store.updateTrack(track, actionName: "Track name") { $0.name = "Renamed voice" }
        XCTAssertEqual(store.automationEditing.selectedFrames, [50, 150])
        XCTAssertTrue(store.automationEditing.hasFocus)
        var effect = RealtimeEffect(kind: .echo, parameters: RealtimeEffectKind.echo.defaultParameters)
        effect.selectedAutomationParameter = "time"
        effect.parameterAutomation["time"] = points
        store.session.tracks[0].realtimeEffects = [effect]
        store.automationTarget = .effect
        store.selectedRealtimeEffect = effect.id
        store.selectAutomationRange(0..<200)
        store.changeRealtimeEffect(effect.id, name: "Feedback") { $0.parameters["feedback"] = 0.5 }
        XCTAssertEqual(store.automationEditing.selectedFrames, [50, 150])
        XCTAssertEqual(store.automationEditing.range, 0..<200)
        XCTAssertTrue(store.automationEditing.hasFocus)
    }
}
