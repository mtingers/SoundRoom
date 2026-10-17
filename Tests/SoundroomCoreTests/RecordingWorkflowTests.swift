import XCTest

@testable import SoundroomCore

final class RecordingWorkflowTests: XCTestCase {
    func testPunchCountInAndPreRollShareOneFramePlan() {
        let plan = RecordingPlan(playhead: 0, punch: 480000..<576000, preRollSeconds: 2, countInBars: 1, tempo: 120)
        XCTAssertEqual(plan.targetStart, 480000)
        XCTAssertEqual(plan.playbackStart, 384000)
        XCTAssertEqual(plan.countInFrames, 96000)
        XCTAssertEqual(plan.inputLeadFrames, 192000)
        XCTAssertEqual(plan.elapsedCaptureFrames(transportElapsedFrames: 191999), 0)
        XCTAssertEqual(plan.elapsedCaptureFrames(transportElapsedFrames: 240000), 48000)
        XCTAssertEqual(plan.elapsedCaptureFrames(transportElapsedFrames: 400000), 96000)
        XCTAssertEqual(plan.timelineFrame(renderedFrame: 400000), plan.playbackStart)
        XCTAssertEqual(plan.timelineFrame(renderedFrame: 576000), plan.targetStart)
    }
    func testRecordingPlanSanitizesControlsAndPrerollAtZero() {
        let plan = RecordingPlan(playhead: -1, preRollSeconds: .nan, countInBars: 99, tempo: .nan)
        XCTAssertEqual(plan.targetStart, 0)
        XCTAssertEqual(plan.playbackStart, 0)
        XCTAssertEqual(plan.countInFrames, 384000)
        XCTAssertEqual(plan.elapsedCaptureFrames(transportElapsedFrames: 400000), 16000)
    }
    func testAutomaticPunchUsesBothDeviceFrameClocksAndExactExclusiveRange() {
        let plan = RecordingPlan(
            playhead: 48_000, punch: 48_000..<120_000, preRollSeconds: 0.25, countInBars: 1, tempo: 240)
        XCTAssertEqual(plan.playbackStart, 36_000)
        XCTAssertEqual(plan.inputLeadFrames, 60_000)
        XCTAssertEqual(plan.punchFrameCount, 72_000)
        XCTAssertFalse(plan.shouldCompletePunch(renderedFrame: 168_000, capturedTimelineFrames: 71_182))
        XCTAssertFalse(plan.shouldCompletePunch(renderedFrame: 167_999, capturedTimelineFrames: 72_000))
        XCTAssertTrue(plan.shouldCompletePunch(renderedFrame: 168_000, capturedTimelineFrames: 72_000))
        XCTAssertTrue(plan.shouldCompletePunch(renderedFrame: 170_048, capturedTimelineFrames: 73_024))
        // A manual stop or input failure still retains its actual partial length.
        XCTAssertEqual(plan.elapsedCaptureFrames(transportElapsedFrames: 131_182), 71_182)
        XCTAssertFalse(plan.punchCompletionTimedOut(transportElapsedFrames: 227_999))
        XCTAssertTrue(plan.punchCompletionTimedOut(transportElapsedFrames: 228_000))
        XCTAssertNil(RecordingPlan(playhead: 0).punchFrameCount)
        XCTAssertFalse(
            RecordingPlan(playhead: 0).shouldCompletePunch(renderedFrame: 1_000_000, capturedTimelineFrames: 1_000_000))
        XCTAssertFalse(RecordingPlan(playhead: 0).punchCompletionTimedOut(transportElapsedFrames: 1_000_000))
    }
    func testMetronomeUsesTempoGridAndSilentFiniteEndpoints() {
        let cue = RecordingMetronome(tempo: 120, enabled: true)
        XCTAssertEqual(cue.sample(at: 0), 0)
        XCTAssertEqual(cue.sample(at: 23999), 0)
        XCTAssertNotEqual(cue.sample(at: 24), 0)
        XCTAssertNotEqual(cue.sample(at: 24024), 0)
        let entire = (0..<48000).map { cue.sample(at: $0) }
        let split = (0..<128).map { cue.sample(at: $0) } + (128..<48000).map { cue.sample(at: $0) }
        XCTAssertEqual(entire, split)
        XCTAssertTrue(entire.allSatisfy { $0.isFinite && abs($0) <= 0.15 })
        XCTAssertEqual(RecordingMetronome(enabled: false, countInStart: 100, countInEnd: 1000).sample(at: 1024), 0)
    }
    func testOldTrackDecodesWithoutTakeLanesAndTakeRoundTripRetainsSources() throws {
        let old = try JSONDecoder().decode(Track.self, from: Data("{\"name\":\"Old\",\"clips\":[]}".utf8))
        XCTAssertTrue(old.recordingTakes.isEmpty)
        XCTAssertNil(old.activeTakeID)
        var track = old
        let clip = Clip(assetID: UUID(), name: "Take", start: 100, length: 200)
        let take = RecordingTake(name: "Take 1", clips: [clip])
        track.recordingTakes = [take]
        track.activeTakeID = take.id
        XCTAssertEqual(try JSONDecoder().decode(Track.self, from: JSONEncoder().encode(track)), track)
        XCTAssertEqual(take.range, 100..<300)
    }
}
