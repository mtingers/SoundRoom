import XCTest

@testable import Soundroom
@testable import SoundroomCore

@MainActor final class AutomationLoopTouchTests: XCTestCase {
    private func fixture(_ parameter: TrackAutomationParameter = .pan) -> SessionStore {
        let store = SessionStore()
        let id = store.session.tracks[0].id
        store.selectedTrack = id
        store.automationTarget = parameter == .pan ? .pan : .volume
        store.session.tracks[0].mixAutomation[parameter.rawValue] = [
            .init(frame: 0, value: 0.2, curve: .smooth), .init(frame: 20_000, value: 0.7),
        ]
        store.selection = EditSelection(trackID: id, range: 2000..<14_000)
        store.loopSelection = true
        store.setAutomationMode(.touch)
        store.isPlaying = true
        return store
    }

    func testSerialDetectsWholeShortLapsBetweenIdenticalWrappedCursors() {
        let original = [AutomationPoint(frame: 0, value: 0.2), .init(frame: 10_000, value: 0.6)]
        var touch = AutomationTouchCurve(
            points: original, baseline: 0.2, scale: .linear, start: 2300, loop: 2000..<2600, serial: 0)
        _ = touch.write(at: 2300, value: 0.8, serial: 0)
        touch.observe(2300, serial: 1800)  // Three whole laps; wrapped cursor is unchanged.
        let result = touch.finish(at: 2400, serial: 1900)
        for frame in 2000..<2600 {
            XCTAssertEqual(AutomationInterpolation.value(result, at: frame, baseline: 0.2), 0.8, accuracy: 0.000001)
        }
        for frame in [0, 1000, 2600, 5000, 10_000] {
            XCTAssertEqual(
                AutomationInterpolation.value(result, at: frame, baseline: 0.2),
                AutomationInterpolation.value(original, at: frame, baseline: 0.2), accuracy: 0.000002)
        }
    }
}
