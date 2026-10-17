import XCTest

@testable import SoundroomCore

final class AutomationCurveTests: XCTestCase {
    func testCurveModesEndpointsPublicValuePreparedCursorAndBackwardSeek() throws {
        for shape in AutomationCurve.allCases {
            let points = [
                AutomationPoint(frame: 10, value: 0.2, curve: shape), AutomationPoint(frame: 110, value: 0.8),
            ]
            let cursor = AutomationCursor(points: points, baseline: 0.4)
            for frame in [-1, 0, 10, 25, 40, 85, 110, 150, 33] {
                XCTAssertEqual(
                    cursor.value(at: frame), AutomationInterpolation.value(points, at: frame, baseline: 0.4),
                    accuracy: 0.000001)
            }
            XCTAssertEqual(AutomationInterpolation.value(points, at: 10, baseline: 0.4), 0.2)
            XCTAssertEqual(AutomationInterpolation.value(points, at: 110, baseline: 0.4), 0.8)
            XCTAssertEqual(try JSONDecoder().decode([AutomationPoint].self, from: JSONEncoder().encode(points)), points)
        }
        let old = try JSONDecoder().decode(AutomationPoint.self, from: Data("{\"frame\":5,\"value\":0.3}".utf8))
        XCTAssertEqual(old.curve, .linear)
    }
    func testDecibelRampIsEvenAndReachesExactSilence() {
        let points = [AutomationPoint(frame: 0, value: 2 / 3), AutomationPoint(frame: 100, value: 0.01 * 2 / 3)]
        let cursor = AutomationCursor(points: points, baseline: 2 / 3, valueScale: .decibels)
        for frame in 0...100 {
            let gain = cursor.value(at: frame) * 1.5
            XCTAssertEqual(20 * log10(gain), -40 * Float(frame) / 100, accuracy: 0.00001)
        }
        let silence = [AutomationPoint(frame: 0, value: 2 / 3), AutomationPoint(frame: 100, value: 0)]
        XCTAssertEqual(AutomationInterpolation.value(silence, at: 100, baseline: 1, scale: .decibels), 0)
        XCTAssertEqual(
            AutomationInterpolation.value(silence, at: 50, baseline: 1, scale: .decibels) * 1.5, 0.01,
            accuracy: 0.000001)
        let muted = [AutomationPoint(frame: 0, value: 0), AutomationPoint(frame: 100, value: 0)]
        let silentCursor = AutomationCursor(points: muted, baseline: 1, valueScale: .decibels)
        XCTAssertEqual(silentCursor.value(at: 50), 0)
        XCTAssertEqual(AutomationInterpolation.value(muted, at: 50, baseline: 1, scale: .decibels), 0)
    }
    func testCurvedDbMixBlockExportAndSeekParity() {
        var session = Session()
        let asset = UUID()
        session.tracks[0].clips = [Clip(assetID: asset, name: "Tone", start: 0, length: 1024, edgeFadeFrames: 0)]
        session.tracks[0].volumeAutomationScale = .decibels
        session.tracks[0].mixAutomation["volume"] = [
            .init(frame: 0, value: 2 / 3, curve: .smooth), .init(frame: 1000, value: 0.01),
        ]
        session.tracks[0].mixAutomation["pan"] = [
            .init(frame: 0, value: 0, curve: .easeOut), .init(frame: 1000, value: 1),
        ]
        let assets = [asset: AudioSamples(channels: [Array(repeating: 1, count: 1024)])]
        let expected = Mixer.render(session: session, assets: assets, range: 0..<1024)
        for size in [1, 7, 128, 513] {
            let mixer = RealtimeMixer(session: session, assets: assets)
            var left = Array(repeating: Float(0), count: 1024)
            var right = left
            left.withUnsafeMutableBufferPointer { l in
                right.withUnsafeMutableBufferPointer { r in
                    var offset = 0
                    while offset < 1024 {
                        let count = min(size, 1024 - offset)
                        mixer.render(
                            intoLeft: l.baseAddress! + offset, intoRight: r.baseAddress! + offset, frameCount: count)
                        offset += count
                    }
                }
            }
            XCTAssertEqual(AudioSamples(channels: [left, right]), expected)
        }
        XCTAssertEqual(Mixer.render(session: session, assets: assets, range: 257..<900), expected.slice(257..<900))
        XCTAssertEqual(
            expected.channels[0][500],
            session.tracks[0].value(at: 500, parameter: .volume) * 1.5
                * (1 - (session.tracks[0].value(at: 500, parameter: .pan) * 2 - 1)), accuracy: 0.000001)
    }
    func testSimplificationErrorBoundsDenseNormalizedAndDbGestures() {
        for scale in AutomationValueScale.allCases {
            let tolerance: Float = scale == .linear ? 0.002 : 0.2
            let points = (0...3000).map { index -> AutomationPoint in
                let axis =
                    scale == .linear
                    ? Float(0.5 + 0.25 * sin(Double(index) / 350)) : Float(-24 + 8 * sin(Double(index) / 350))
                return AutomationPoint(frame: index * 4, value: scale.normalizedAxis(axis))
            }
            let reduced = AutomationSimplifier.simplify(points, tolerance: tolerance, scale: scale)
            XCTAssertLessThan(reduced.count, points.count / 10)
            XCTAssertEqual(reduced.first, points.first)
            XCTAssertEqual(reduced.last, points.last)
            for frame in 0...12_000 {
                let expected = scale.axisValue(
                    AutomationInterpolation.value(points, at: frame, baseline: 0.5, scale: scale))
                let actual = scale.axisValue(
                    AutomationInterpolation.value(reduced, at: frame, baseline: 0.5, scale: scale))
                XCTAssertLessThanOrEqual(abs(expected - actual), tolerance + 0.00002)
            }
        }
    }
    func testSplittingCurveBoundariesPreservesEveryOriginalSample() {
        for scale in AutomationValueScale.allCases {
            for curve in AutomationCurve.allCases {
                let points = [
                    AutomationPoint(frame: 0, value: 0.2, curve: curve), AutomationPoint(frame: 1000, value: 0.8),
                ]
                var split = AutomationCurveEditing.insertingBoundary(points, at: 123, baseline: 0.2, scale: scale)
                split = AutomationCurveEditing.insertingBoundary(split, at: 757, baseline: 0.2, scale: scale)
                for frame in 0...1000 {
                    XCTAssertEqual(
                        AutomationInterpolation.value(split, at: frame, baseline: 0.2, scale: scale),
                        AutomationInterpolation.value(points, at: frame, baseline: 0.2, scale: scale),
                        accuracy: 0.000002)
                }
            }
        }
    }
    func testNonlinearSimplificationKeepsAuthoredTransitions() {
        let points = [
            AutomationPoint(frame: 0, value: 0.2, curve: .smooth), .init(frame: 50, value: 0.5),
            .init(frame: 100, value: 0.6), .init(frame: 150, value: 0.7, curve: .hold), .init(frame: 200, value: 0.9),
        ]
        let reduced = AutomationSimplifier.simplify(points, tolerance: 0.001)
        XCTAssertTrue(reduced.contains(points[0]))
        XCTAssertTrue(reduced.contains(points[1]))
        XCTAssertTrue(reduced.contains(points[3]))
        XCTAssertTrue(reduced.contains(points[4]))
    }
}
