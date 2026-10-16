import XCTest
import Combine
import SoundroomCore
@testable import Soundroom

final class PlaybackDisplayTests: XCTestCase {
    @MainActor func testDemoSizedTransportDoesNotInvalidateDocumentOrMeterObservers() {
        let store = SessionStore(), asset = UUID()
        store.session.tracks[0].clips = [Clip(assetID: asset, name: "Long take", start: 0, length: Int(sessionRate * 136))]
        var editorChanges = 0, positionChanges = 0, meterChanges = 0
        let editor = store.objectWillChange.sink { editorChanges += 1 }
        let position = store.playbackPosition.objectWillChange.sink { positionChanges += 1 }
        let meter = store.playbackLevel.objectWillChange.sink { meterChanges += 1 }
        for tick in 1...600 { store.playhead = tick * 1600 }
        XCTAssertEqual(editorChanges, 0, "Clock ticks must not rebuild clip waveforms or effects controls")
        XCTAssertEqual(positionChanges, 600)
        XCTAssertEqual(meterChanges, 0, "The clock must not refresh the output meter")
        store.meter = 0.7
        XCTAssertEqual(meterChanges, 1)
        XCTAssertEqual(positionChanges, 600, "Meter updates must not refresh faders or timeline geometry")
        XCTAssertEqual(editorChanges, 0)
        store.selectedTrack = UUID()
        XCTAssertGreaterThan(editorChanges, 0, "Editing and selection must still invalidate the editor")
        withExtendedLifetime([editor, position, meter]) {}
    }
    @MainActor func testPositionReadoutsKeepExactSeekLoopAndPauseValuesAndSuppressDuplicateUpdates() {
        let store = SessionStore()
        var positions: [Int] = []
        let observation = store.playbackPosition.$frame.sink { positions.append($0) }
        store.playhead = 24_000; store.playhead = 24_000
        store.playhead = 48_000; store.playhead = 100 // Loop wrap.
        store.playhead = 12_345 // Stopped seek, sample precision retained.
        XCTAssertEqual(positions, [0, 24_000, 48_000, 100, 12_345])
        XCTAssertEqual(store.playhead, 12_345)
        store.meter = .nan; XCTAssertEqual(store.meter, 0)
        store.meter = 1.2; XCTAssertEqual(store.meter, 1.2, "Keep over-range peaks for the clipping indicator")
        withExtendedLifetime(observation) {}
    }
    @MainActor func testRecordingAndTailTimelineGrowthIsBoundedAndStillRevealsCursor() {
        let store = SessionStore()
        var editorChanges = 0
        let observation = store.objectWillChange.sink { editorChanges += 1 }
        for tick in 1...600 {
            store.playhead = tick * 1600
            XCTAssertGreaterThan(store.timelineSeconds * sessionRate, Double(store.playhead))
        }
        XCTAssertLessThanOrEqual(editorChanges, 4, "Timeline growth must be chunked, not 30 layouts per second")
        store.playhead = 0
        XCTAssertEqual(store.timelineSeconds, 20)
        store.playhead = Int(sessionRate * 100)
        XCTAssertGreaterThan(store.timelineSeconds, 100)
        withExtendedLifetime(observation) {}
    }
    @MainActor func testAutomationReadoutsUseLivePositionIncludingBackwardSeek() {
        let store = SessionStore(), id = store.session.tracks[0].id
        store.selectedTrack = id; store.automationTarget = .pan
        store.session.tracks[0].mixAutomation["pan"] = [.init(frame: 0, value: 0), .init(frame: 48_000, value: 1)]
        store.playhead = 24_000
        XCTAssertEqual(store.automationValueAtPlayhead, 0.5, accuracy: 0.0001)
        XCTAssertEqual(store.mixFaderValue(store.session.tracks[0], parameter: .pan), 0, accuracy: 0.0001)
        store.playhead = 0
        XCTAssertEqual(store.automationValueAtPlayhead, 0)
        XCTAssertEqual(store.mixFaderValue(store.session.tracks[0], parameter: .pan), -1)
    }
}
