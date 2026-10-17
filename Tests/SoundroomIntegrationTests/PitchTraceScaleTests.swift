import SoundroomCore
import XCTest

@testable import Soundroom

final class PitchTraceScaleTests: XCTestCase {
    private func meter(_ frame: Int, midi: Float) -> PitchMeterSnapshot {
        var pitch = PitchMeterSnapshot()
        pitch.sourceFrame = frame
        pitch.confidence = 1
        pitch.inputFrequency = 440 * pow(2, (midi - 69) / 12)
        pitch.targetFrequency = 440
        pitch.correctionCents = (69 - midi) * 100
        return pitch
    }
    func testSmallVariationsNeverMoveEstablishedVerticalScaleOrAlterReadings() {
        var history = PitchFeedbackHistory()
        history.update(meter(0, midi: 69), at: 0, running: true)
        let scale = history.scale
        for index in 1...100 {
            let raw = meter(index * 3840, midi: index % 2 == 0 ? 68.98 : 69.02)
            history.update(raw, at: Double(index) * 0.08, running: true)
            XCTAssertEqual(history.scale, scale)
            XCTAssertEqual(history.reading, raw)
        }
    }
    func testLoopSeekSilenceAndExpiredExtremaDoNotReframeTrace() {
        var history = PitchFeedbackHistory()
        history.update(meter(48_000, midi: 60), at: 0, running: true)
        let scale = history.scale
        history.update(meter(0, midi: 72), at: 0.08, running: true)
        XCTAssertEqual(history.scale, scale)
        history.update(nil, at: 0.16, running: true)
        XCTAssertEqual(history.scale, scale)
        for index in 1...120 {
            history.update(meter(index * 3840, midi: 69), at: 1 + Double(index) * 0.08, running: true)
        }
        XCTAssertEqual(history.scale, scale)
        history.clear()
        history.update(meter(0, midi: 82), at: 12, running: true)
        XCTAssertEqual(history.scale, scale)
        XCTAssertTrue(history.hasNotesOutsideScale)
        history.fitScale()
        XCTAssertFalse(history.hasNotesOutsideScale)
        XCTAssertNotEqual(history.scale, scale)
    }
    func testZoomAndNavigationAreExplicitAndBounded() {
        var scale = PitchTraceScale()
        scale.fit(notes: [60, 62])
        let original = scale
        scale.zoom(0.5)
        XCTAssertEqual(scale.span, original.span / 2)
        XCTAssertEqual(scale.lower + scale.upper, original.lower + original.upper)
        scale.move(semitones: 6)
        XCTAssertEqual(scale.lower, original.lower + original.span / 4 + 6)
        for _ in 0..<20 { scale.zoom(0.5) }
        XCTAssertEqual(scale.span, 4)
        for _ in 0..<20 { scale.zoom(2) }
        XCTAssertEqual(scale.span, 96)
        let valid = scale
        scale.zoom(.nan)
        scale.move(semitones: .infinity)
        XCTAssertEqual(scale, valid)
    }
    func testUnvoicedAndInvalidDataCannotInitializeScale() {
        var history = PitchFeedbackHistory()
        var pitch = meter(0, midi: 69)
        pitch.confidence = 0
        history.update(pitch, at: 0, running: true)
        XCTAssertFalse(history.scale.initialized)
        var scale = PitchTraceScale()
        scale.fit(notes: [.nan, .infinity])
        XCTAssertFalse(scale.initialized)
        scale.initialize(notes: [60])
        XCTAssertEqual(scale.lower, 48)
        XCTAssertEqual(scale.upper, 72)
        scale.initialize(notes: [90])
        XCTAssertEqual(scale.lower, 48)
    }
}
