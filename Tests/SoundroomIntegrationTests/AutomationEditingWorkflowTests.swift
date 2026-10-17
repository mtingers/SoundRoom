import XCTest

@testable import Soundroom
@testable import SoundroomCore

@MainActor final class AutomationEditingWorkflowTests: XCTestCase {
    private func fixture() -> SessionStore {
        let store = SessionStore()
        store.selectedTrack = store.session.tracks[0].id
        store.automationTarget = .pan
        store.session.tracks[0].mixAutomation["pan"] = [
            .init(frame: 0, value: 0.2, curve: .smooth), .init(frame: 100, value: 0.4), .init(frame: 200, value: 0.6),
            .init(frame: 500, value: 0.8),
        ]
        return store
    }

    func testPastedSubrangePreservesCurvedValuesOutsideEditedRange() {
        let store = fixture()
        store.session.tracks[0].mixAutomation["pan"] = [
            .init(frame: 0, value: 0.1, curve: .smooth), .init(frame: 1000, value: 0.9),
        ]
        let original = store.session.tracks[0]
        store.selectAutomationRange(50..<150)
        store.copyAutomationRange()
        store.playhead = 500
        store.pasteAutomation()
        for frame in 0..<1000 where !(500..<600).contains(frame) {
            XCTAssertEqual(
                store.session.tracks[0].value(at: frame, parameter: .pan), original.value(at: frame, parameter: .pan),
                accuracy: 0.000002)
        }
        for frame in 0..<100 {
            XCTAssertEqual(
                store.session.tracks[0].value(at: 500 + frame, parameter: .pan),
                original.value(at: 50 + frame, parameter: .pan), accuracy: 0.000002)
        }
    }

    func testClipboardCannotAccidentallyPastePanValuesIntoVolume() {
        let store = fixture()
        store.selectAutomationRange(0..<200)
        store.copyAutomationRange()
        store.automationTarget = .volume
        XCTAssertFalse(store.canPasteAutomation)
        let original = store.session
        store.pasteAutomation()
        XCTAssertEqual(store.session, original)
    }
}
