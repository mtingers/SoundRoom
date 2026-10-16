import XCTest
import SoundroomCore
@testable import Soundroom

final class ManualPitchEditingTests: XCTestCase {
    private func point(_ frame: Int, _ frequency: Float = 440) -> PitchContourPoint {
        .init(frame: frame, frequency: frequency, confidence: 0.99)
    }

    func testRegionHasMinimumDurationAndMapsToOriginalTimeline() {
        var editing = ManualPitchEditing()
        let contour = [point(5000), point(9000)]
        editing.load(frames: 12_000, contour: contour, reference: 440)
        editing.select(5000..<5010, contour: contour, reference: 440)
        XCTAssertEqual(editing.region, 5000..<9096)
        XCTAssertEqual(editing.timelineRange(selection: 48_000..<60_000), 53_000..<57_096)
        XCTAssertNil(editing.timelineRange(selection: 0..<11_000))
        editing.select(11_999..<12_000, contour: contour, reference: 440)
        XCTAssertEqual(editing.region, 7904..<12_000)
        editing.load(frames: 2000, contour: [point(1000)], reference: 440)
        XCTAssertEqual(editing.region, 0..<2000)
    }

    func testTargetIsLocalBoundedAndUnavailableForUnvoicedRegion() {
        var editing = ManualPitchEditing()
        let contour = [point(2000), point(7000, 0)]
        editing.load(frames: 12_000, contour: contour, reference: 440)
        XCTAssertEqual(editing.target, 69)
        XCTAssertEqual(editing.targetLimits, 57...81)
        editing.stepTarget(1)
        XCTAssertEqual(editing.target, 70)
        editing.setTarget(95)
        XCTAssertEqual(editing.target, 81)
        let automatic = RealtimeEffectKind.pitchCorrection.defaultParameters
        let manual = editing.parameters(from: automatic)
        XCTAssertEqual(manual["manualTarget"], 81)
        XCTAssertEqual(automatic["manualTarget"], -1)
        editing.select(6000..<12_000, contour: contour, reference: 440)
        XCTAssertFalse(editing.hasVoicedRegion)
        editing.load(frames: 0, contour: contour, reference: 440)
        XCTAssertEqual(editing.region, 0..<0)
        XCTAssertFalse(editing.hasVoicedRegion)
    }

    func testMalformedReferenceAndNonfinitePointsStaySafe() {
        var editing = ManualPitchEditing()
        let contour = [point(1500), point(2500, .infinity), point(3500, .nan)]
        for reference in [Float.nan, .infinity, 0, -440] {
            editing.load(frames: 6000, contour: contour, reference: reference)
            XCTAssertTrue(editing.hasVoicedRegion)
            XCTAssertEqual(editing.target, 69)
        }
        editing.load(frames: 6000, contour: [point(1500, 50)], reference: 440)
        XCTAssertEqual(editing.targetLimits.lowerBound, 32)
        editing.load(frames: 6000, contour: [point(1500, 999)], reference: 440)
        XCTAssertEqual(editing.targetLimits.upperBound, 83)
    }

    func testTimeZoomAndNavigationRespectSelectionAndMinimumWindow() {
        var viewport = PitchChartViewport()
        viewport.fit(frames: 48_000, contour: [point(5000)], reference: 440)
        viewport.zoomTime(2, around: 24_000)
        XCTAssertEqual(viewport.frameRange, 12_000..<36_000)
        viewport.panTime(1)
        XCTAssertEqual(viewport.frameRange, 24_000..<48_000)
        viewport.zoomTime(1000)
        XCTAssertEqual(viewport.frameRange.count, 4800)
        viewport.zoomTime(Double.leastNonzeroMagnitude)
        XCTAssertEqual(viewport.frameRange, 0..<48_000)
        viewport.zoomTime(.nan)
        XCTAssertEqual(viewport.frameRange, 0..<48_000)
        XCTAssertEqual(viewport.frame(at: .nan), 0)
        XCTAssertEqual(viewport.frame(at: -1), 0)
        XCTAssertEqual(viewport.frame(at: 2), 48_000)
    }

    func testPitchAxisOnlyChangesOnExplicitZoomPanOrFit() {
        var viewport = PitchChartViewport(), editing = ManualPitchEditing()
        let contour = [point(2000), point(5000)]
        viewport.fit(frames: 12_000, contour: contour, reference: 440)
        editing.load(frames: 12_000, contour: contour, reference: 440)
        let original = viewport
        editing.setTarget(81)
        XCTAssertEqual(viewport, original, "Changing a requested note must not reframe the chart")
        viewport.fit(frames: 12_000, contour: contour, reference: 440, extraNote: Float(editing.target))
        XCTAssertLessThan(viewport.pitchLower, 69)
        XCTAssertGreaterThan(viewport.pitchUpper, 81)
        viewport.zoomPitch(2, around: Float(editing.target))
        XCTAssertLessThan(viewport.pitchLower, Float(editing.target))
        XCTAssertGreaterThan(viewport.pitchUpper, Float(editing.target), "Zoom keeps the note being tuned visible")
        let span = viewport.pitchUpper - viewport.pitchLower
        viewport.panPitch(-2)
        XCTAssertEqual(viewport.pitchUpper - viewport.pitchLower, span)
        XCTAssertEqual(viewport.note(at: 0), Int(viewport.pitchUpper.rounded()))
        XCTAssertEqual(viewport.note(at: 1), Int(viewport.pitchLower.rounded()))
        viewport.reset()
        XCTAssertEqual(viewport.frameRange, 0..<12_000)
        XCTAssertGreaterThan(viewport.pitchUpper, 81)
    }

    @MainActor
    private func fixture() -> (SessionStore, UUID, UUID) {
        let store = SessionStore(), asset = UUID()
        let voice = (0..<36_000).map { Float(0.2 * sin(2 * Double.pi * 450 * Double($0) / sessionRate)) }
        store.assets[asset] = AudioSamples(channels: [voice])
        let track = store.session.tracks[0].id
        let clip = Clip(assetID: asset, name: "Voice", start: 5000, length: voice.count, edgeFadeFrames: 0)
        store.session.tracks[0].clips = [clip]
        store.selectedTrack = track
        store.selection = EditSelection(trackID: track, range: 5000..<41_000)
        store.chooseEffect(.pitchCorrection)
        return (store, track, asset)
    }

    @MainActor
    private func finish(_ store: SessionStore) async {
        let deadline = Date().addingTimeInterval(10)
        while store.busy, Date() < deadline { try? await Task.sleep(nanoseconds: 1_000_000) }
        XCTAssertFalse(store.busy, "Manual render did not finish")
    }


}
