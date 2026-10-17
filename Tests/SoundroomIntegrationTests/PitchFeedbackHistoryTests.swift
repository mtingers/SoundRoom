import SoundroomCore
import XCTest

@testable import Soundroom

final class PitchFeedbackHistoryTests: XCTestCase {
    private func pitch(_ frame: Int, frequency: Float = 450) -> PitchMeterSnapshot {
        var value = PitchMeterSnapshot()
        value.sourceFrame = frame
        value.inputFrequency = frequency
        value.confidence = 1
        return value
    }
    func testStoppedOrStalledRenderClearsPitchAndTrace() {
        var history = PitchFeedbackHistory()
        let value = pitch(4800)
        history.update(value, at: 10, running: true)
        XCTAssertEqual(history.points.count, 1)
        history.update(value, at: 10.2, running: true)
        XCTAssertNotNil(history.reading)
        history.update(value, at: 10.36, running: true)
        XCTAssertNil(history.reading)
        XCTAssertTrue(history.points.isEmpty)
        history.update(value, at: 10.8, running: true)
        XCTAssertNil(history.reading)
        XCTAssertTrue(history.points.isEmpty)
        history.update(pitch(9000), at: 11, running: true)
        history.update(pitch(9000), at: 11.1, running: false)
        XCTAssertNil(history.reading)
        XCTAssertTrue(history.points.isEmpty)
    }
    func testSeekLoopAndMissingMeterDoNotJoinUnrelatedNotes() {
        var history = PitchFeedbackHistory()
        history.update(pitch(48_000), at: 0, running: true)
        history.update(pitch(51_840), at: 0.08, running: true)
        XCTAssertEqual(history.points.count, 2)
        history.update(pitch(0, frequency: 220), at: 0.16, running: true)
        XCTAssertEqual(history.points.count, 1)
        history.update(pitch(240_000), at: 0.24, running: true)
        XCTAssertEqual(history.points.count, 1)
        history.update(nil, at: 0.32, running: true)
        XCTAssertNil(history.reading)
        XCTAssertTrue(history.points.isEmpty)
    }
    func testRollingTraceRemainsBoundedAndPreservesUnvoicedGaps() {
        var history = PitchFeedbackHistory()
        for frame in 0..<400 {
            var value = pitch(frame * 3840)
            if frame % 5 == 0 {
                value.inputFrequency = 0
                value.confidence = 0
            }
            history.update(value, at: Double(frame) * 0.08, running: true)
        }
        XCTAssertLessThanOrEqual(history.points.count, 102)
        XCTAssertLessThanOrEqual(history.points.last!.time - history.points.first!.time, 8)
        XCTAssertTrue(history.points.contains { !$0.pitch.isVoiced })
    }
    func testIdenticalSmallLoopReadingsStayFreshWhenRenderRevisionsAdvance() {
        var history = PitchFeedbackHistory()
        let value = pitch(48)
        for revision in 0..<20 {
            history.update(value, at: Double(revision) * 0.08, running: true, revision: UInt64(revision))
            XCTAssertNotNil(history.reading)
        }
        XCTAssertEqual(history.points.count, 20)
        history.update(value, at: 2, running: true, revision: 19)
        XCTAssertNil(history.reading)
    }
}
