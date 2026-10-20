import SoundroomCore
import XCTest

@testable import Soundroom

final class PitchChartSamplingTests: XCTestCase {
    func testDenseMeasuredContourPreservesExtremaWithBoundedPlotVertices() {
        let raw = (0..<65_536).map { index in
            PitchContourPoint(
                frame: index * 256, frequency: index == 37 ? 910 : index == 91 ? 60 : 440,
                confidence: 0.99)
        }
        let sampled = PitchChartSampling.contour(raw, frames: 0..<(65_536 * 256), pixels: 400)
        XCTAssertLessThanOrEqual(sampled.count, 1600)
        XCTAssertTrue(sampled.contains { $0.frame == 37 * 256 && $0.frequency == 910 })
        XCTAssertTrue(sampled.contains { $0.frame == 91 * 256 && $0.frequency == 60 })
        XCTAssertEqual(sampled.first, raw.first)
        XCTAssertEqual(sampled.last, raw.last)
    }
    func testFineZoomCullsUnrelatedMeasuredAudioByTime() {
        let raw = (0..<65_536).map { PitchContourPoint(frame: $0 * 256, frequency: 440, confidence: 0.99) }
        let viewport = 12_000_000..<12_010_000
        let sampled = PitchChartSampling.contour(raw, frames: viewport, pixels: 400)
        XCTAssertLessThan(sampled.count, 45)
        XCTAssertTrue(
            sampled.allSatisfy { $0.frame >= viewport.lowerBound - 256 && $0.frame <= viewport.upperBound + 256 })
    }
    func testUnvoicedGapInsideOnePixelIsNeverJoined() {
        let raw = [
            PitchContourPoint(frame: 0, frequency: 440, confidence: 0.99),
            .init(frame: 1, frequency: 500, confidence: 0.99),
            .init(frame: 2, frequency: 0, confidence: 0),
            .init(frame: 3, frequency: 0, confidence: 0),
            .init(frame: 4, frequency: 600, confidence: 0.99),
            .init(frame: 5, frequency: 700, confidence: 0.99),
        ]
        let sampled = PitchChartSampling.contour(raw, frames: 0..<1000, pixels: 1)
        XCTAssertEqual(sampled.filter { !$0.isVoiced }.count, 1)
        let gap = sampled.firstIndex { !$0.isVoiced }!
        XCTAssertTrue(sampled[..<gap].allSatisfy { $0.frame < 2 })
        XCTAssertTrue(sampled[(gap + 1)...].allSatisfy { $0.frame >= 4 })
        XCTAssertTrue(sampled.contains { $0.frequency == 700 })
    }
    func testPreparedTargetSamplerMatchesActualNonlinearAndSlicedInterpolation() {
        let original = PitchEditRegion(
            startFrame: 1000, endFrame: 15_000,
            points: [
                .init(frame: 1000, note: 69.25, curve: .smooth),
                .init(frame: 6000, note: 72.6, curve: .easeOut),
                .init(frame: 15_000, note: 70.15),
            ])
        let region = original.sliced(to: 2500..<11_000)!
        let sampled = PreparedPitchChartRegion(region).samples(frames: 5000..<9000, pixels: 200)
        XCTAssertEqual(sampled.first?.frame, 5000)
        XCTAssertEqual(sampled.last?.frame, 8999)
        XCTAssertTrue(sampled.contains { $0.frame == 6000 })
        for sample in sampled { XCTAssertEqual(sample.note, region.note(at: sample.frame)!, accuracy: 0.00001) }
    }
    func testHeldTargetTransitionRetainsImmediateBeforeAndAfterSamples() {
        let region = PitchEditRegion(
            startFrame: 1000, endFrame: 10_000,
            points: [
                .init(frame: 1000, note: 69, curve: .hold), .init(frame: 5000, note: 72, curve: .hold),
            ])
        let sampled = PreparedPitchChartRegion(region).samples(frames: 1000..<10_000, pixels: 400)
        XCTAssertEqual(sampled.first { $0.frame == 4999 }?.note, 69)
        XCTAssertEqual(sampled.first { $0.frame == 5000 }?.note, 72)
    }
    func testDenseTargetVerticesAndAnchorDotsAreBoundedByViewportPixels() {
        let points = (0..<33_000).map { index in
            PitchEditPoint(frame: index * 256, note: index == 31 ? 80 : index == 52 ? 60 : 69)
        }
        let frames = 0..<(33_000 * 256)
        let region = PitchEditRegion(range: frames, points: points)
        let sampled = PreparedPitchChartRegion(region).samples(frames: frames, pixels: 200)
        XCTAssertLessThanOrEqual(sampled.count, 800)
        XCTAssertEqual(sampled.map(\.note).max()!, 80, accuracy: 0.00001)
        XCTAssertEqual(sampled.map(\.note).min()!, 60, accuracy: 0.00001)
        let dots = PitchChartSampling.anchors(points, frames: frames, pixels: 40)
        XCTAssertLessThanOrEqual(dots.count, 160)
        XCTAssertTrue(dots.contains { $0.id == points[31].id })
        XCTAssertTrue(dots.contains { $0.id == points[52].id })
        let zoom = 5_000_000..<5_003_000
        let zoomed = PitchChartSampling.anchors(points, frames: zoom, pixels: 200)
        XCTAssertLessThan(zoomed.count, 14)
        XCTAssertTrue(zoomed.allSatisfy { zoom.contains($0.frame) })
    }
    func testManyShortPhrasesCostTheirOwnWidthNotTheChartWidthEach() {
        // 100 phrases of 0.2 s across 30 s on an 860 pt chart previously drew
        // 1,720 samples each (172,000 per redraw).
        let regions = (0..<100).map { index in
            let start = index * 14_400
            return PitchEditRegion(
                startFrame: start, endFrame: start + 9_600,
                points: [
                    .init(frame: start, note: 60), .init(frame: start + 4_800, note: 62),
                    .init(frame: start + 9_599, note: 61),
                ])
        }
        let total = regions.map { PreparedPitchChartRegion($0).samples(frames: 0..<1_440_000, pixels: 860).count }
            .reduce(0, +)
        XCTAssertLessThan(total, 2_500)
    }
}
