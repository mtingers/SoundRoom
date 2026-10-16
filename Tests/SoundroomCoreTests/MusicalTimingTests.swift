import XCTest
@testable import SoundroomCore

final class MusicalTimingTests: XCTestCase {
    func testMusicalCoordinatesAcrossMetersAndFractionalTempo() throws {
        for timing in [MusicalTiming(), MusicalTiming(tempo: 123, beatsPerBar: 7, beatUnit: 8), MusicalTiming(tempo: 87.5, beatsPerBar: 3, beatUnit: 2)] {
            for position in ["1.1.000", "2.1.123", "1000.2.200"] {
                let frame = try XCTUnwrap(timing.frame(atPosition: position))
                XCTAssertEqual(timing.position(frame), position)
            }
        }
        let sixEight = MusicalTiming(tempo: 120, beatsPerBar: 6, beatUnit: 8)
        XCTAssertEqual(sixEight.frame(atPosition: "2.1"), 72_000)
        XCTAssertNil(sixEight.frame(atPosition: "1.7"))
        XCTAssertNil(sixEight.frame(atPosition: "1.1.480"))
        XCTAssertNil(sixEight.frame(atPosition: "0.1"))
        XCTAssertNil(sixEight.frame(atPosition: "1.nan"))
    }
    func testAbsoluteSnapDoesNotAccumulateRoundedBeatDrift() {
        let timing = MusicalTiming(tempo: 123)
        for beat in [1, 11, 1000, 100_000] {
            let expected = Int((Double(beat) * 60 / 123 * 48_000).rounded())
            XCTAssertEqual(timing.snapped(expected + 100, quarterNoteDivision: 1), expected)
            XCTAssertEqual(timing.snapped(expected - 100, quarterNoteDivision: 1), expected)
        }
    }
    func testRulerWorkIsBoundedAndLocatedAtViewport() {
        let timing = MusicalTiming(tempo: 123)
        let ticks = timing.rulerTicks(start: 48_000 * 3600, end: 48_000 * 3610, pixelsPerSecond: 120)
        XCTAssertLessThan(ticks.count, 30)
        XCTAssertGreaterThan(ticks.first!.frame, 48_000 * 3590)
        XCTAssertLessThanOrEqual(timing.rulerTicks(start: 0, end: Int.max / 4, pixelsPerSecond: 120).count, 512)
    }
    func testSubdivisionsMatchSnapAndHideWhenTooDense() {
        let timing = MusicalTiming(tempo: 123)
        let grid = timing.gridFrames(start: 0, end: 48_000, quarterNoteDivision: 0.25, pixelsPerSecond: 120)
        XCTAssertGreaterThan(grid.count, 8)
        for (index, frame) in grid.enumerated() {
            XCTAssertEqual(frame, timing.frame(atQuarterNote: Double(index) * 0.25))
            XCTAssertEqual(timing.snapped(frame, quarterNoteDivision: 0.25), frame)
        }
        XCTAssertTrue(timing.gridFrames(start: 0, end: 48_000, quarterNoteDivision: 0.25, pixelsPerSecond: 5).isEmpty)
    }
    func testLegacySessionDefaultsAndInvalidTimingRejected() throws {
        let legacy = try JSONDecoder().decode(Session.self, from: Data("{\"name\":\"Legacy\",\"tracks\":[]}".utf8))
        XCTAssertEqual(legacy.timing, .standard)
        var session = Session(); session.timing = MusicalTiming(tempo: 123, beatsPerBar: 7, beatUnit: 8)
        XCTAssertEqual(session.requiredProjectVersion, 9)
        XCTAssertEqual(try JSONDecoder().decode(Session.self, from: JSONEncoder().encode(session)), session)
        session.timing.tempo = 0
        XCTAssertThrowsError(try JSONDecoder().decode(Session.self, from: JSONEncoder().encode(session)))
    }
    func testRecordingCountInAndAccentsUseProjectMeter() {
        let plan = RecordingPlan(playhead: 0, countInBars: 2, tempo: 120, beatsPerBar: 6, beatUnit: 8)
        XCTAssertEqual(plan.countInFrames, 144_000)
        let cue = RecordingMetronome(tempo: 120, enabled: true, beatsPerBar: 6, beatUnit: 8)
        XCTAssertEqual(cue.sample(at: 40), cue.sample(at: 72_040), accuracy: 0.00001)
        XCTAssertNotEqual(cue.sample(at: 40), cue.sample(at: 12_040))
        let midBar = RecordingMetronome(tempo: 120, enabled: true, countInStart: 24_000, countInEnd: 96_000, beatsPerBar: 6, beatUnit: 8)
        XCTAssertEqual(midBar.sample(at: 24_040), cue.sample(at: 40), accuracy: 0.00001)
        XCTAssertEqual(midBar.sample(at: 96_040), cue.sample(at: 24_040), accuracy: 0.00001)
        let noCount = RecordingMetronome(tempo: 120, enabled: true, countInStart: 24_000, countInEnd: 24_000, beatsPerBar: 6, beatUnit: 8)
        XCTAssertEqual(noCount.sample(at: 24_040), cue.sample(at: 24_040), accuracy: 0.00001)
    }
}
