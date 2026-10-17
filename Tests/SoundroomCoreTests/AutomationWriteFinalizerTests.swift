import XCTest

@testable import SoundroomCore

final class AutomationWriteFinalizerTests: XCTestCase {
    func testBeforeFirstAndNonlinearOutsideSamplesArePreserved() throws {
        for scale in AutomationValueScale.allCases {
            for curve in AutomationCurve.allCases {
                let original: [AutomationPoint] = [
                    .init(frame: 1000, value: 0.8, curve: curve, curveStart: 0.1, curveEnd: 0.85),
                    .init(frame: 2000, value: 0),
                ]
                for start in [0, 100, 1200, 2300] {
                    let value = AutomationInterpolation.value(original, at: start, baseline: 0.5, scale: scale)
                    let result = try AutomationWriteFinalizer.finalize(
                        original: original, baseline: 0.5, scale: scale,
                        ramps: [.init(frame: start, start: value, target: 0.2, duration: 256)],
                        released: .init(lastFrame: start + 299, lastValue: 0.2, nextReadFrame: start + 300),
                        accuracy: .normalized(0.000001))
                    for frame in 0...2800 where !(start..<(start + 300)).contains(frame) {
                        XCTAssertEqual(
                            AutomationInterpolation.value(result, at: frame, baseline: 0.5, scale: scale),
                            AutomationInterpolation.value(original, at: frame, baseline: 0.5, scale: scale),
                            accuracy: 0.00003, "\(curve)/\(scale) frame=\(frame)")
                    }
                    if start < 700 { XCTAssertEqual(result.first(where: { $0.frame == start + 300 })?.curve, .hold) }
                }
            }
        }
    }
    func testPartialAndInterruptedRampsUseActualPrefix() throws {
        let start: Float = 0.5 + 0.5 * 50 / 256
        let last: Float = start * (1 - 100 / 256) + 0.1 * 100 / 256
        let result = try AutomationWriteFinalizer.finalize(
            original: [], baseline: 0.5, scale: .linear,
            ramps: [
                .init(frame: 100, start: 0.5, target: 1, duration: 256),
                .init(frame: 150, start: start, target: 0.1, duration: 256),
            ],
            released: .init(lastFrame: 250, lastValue: last, nextReadFrame: 251), accuracy: .normalized(0.000001))
        for frame in 100...250 {
            let expected: Float =
                frame < 150
                ? 0.5 + 0.5 * Float(frame - 100) / 256
                : start * (1 - Float(frame - 150) / 256) + 0.1 * Float(frame - 150) / 256
            XCTAssertEqual(
                AutomationInterpolation.value(result, at: frame, baseline: 0.5), expected, accuracy: 0.000001)
        }
        XCTAssertEqual(AutomationInterpolation.value(result, at: 251, baseline: 0.5), 0.5)
        XCTAssertFalse(result.contains { $0.value == 1 })
    }
    func testSampleAccuracyAndSilenceBudgetsFailWithoutTruncatingCallerCapture() throws {
        for scale in AutomationValueScale.allCases {
            var ramps: [AutomationWriteRamp] = []
            var value: Float = 0.5
            for i in 0..<80 {
                let target = Float(0.15 + 0.65 * abs(sin(Double(i) * 0.1)))
                ramps.append(.init(frame: 100 + i * 32, start: value, target: target, duration: 16))
                value = target
            }
            let end = 100 + 79 * 32 + 24
            let result = try AutomationWriteFinalizer.finalize(
                original: [], baseline: 0.5, scale: scale, ramps: ramps,
                released: .init(lastFrame: end, lastValue: value, nextReadFrame: end + 1),
                accuracy: scale == .linear ? .normalized(0.002) : .decibels(0.04))
            for frame in 100...end {
                let ramp = ramps.last(where: { $0.frame <= frame })!
                let expected = scale.interpolate(
                    ramp.start, ramp.target, fraction: min(1, Float(frame - ramp.frame) / Float(ramp.duration)))
                let actual = AutomationInterpolation.value(result, at: frame, baseline: 0.5, scale: scale)
                XCTAssertLessThanOrEqual(
                    abs(scale.axisValue(expected) - scale.axisValue(actual)), scale == .linear ? 0.00201 : 0.04001)
            }
        }
        let detail = (0..<5000).map {
            AutomationWriteRamp(frame: $0, start: $0 % 2 == 0 ? 0 : 1, target: 0, duration: 256)
        }
        XCTAssertThrowsError(
            try AutomationWriteFinalizer.finalize(
                original: [], baseline: 0.5, scale: .decibels, ramps: detail,
                released: .init(lastFrame: 5000, lastValue: 0, nextReadFrame: 5001), accuracy: .decibels(0.01))
        ) { XCTAssertEqual($0 as? AutomationWriteFinalizerError, .pointLimit) }
        XCTAssertEqual(detail.count, 5000)
    }
    func testUntouchedBeginCancellationAndInvalidRelease() throws {
        let original: [AutomationPoint] = [.init(frame: 10, value: 0.4, curve: .easeIn, curveStart: 0.1, curveEnd: 0.9)]
        XCTAssertEqual(
            try AutomationWriteFinalizer.finalize(
                original: original, baseline: 0.5, scale: .linear, ramps: [], released: nil,
                accuracy: .normalized(0.001)), original)
        let ramp = AutomationWriteRamp(frame: 100, start: 0.5, target: 0.2, duration: 256)
        XCTAssertThrowsError(
            try AutomationWriteFinalizer.finalize(
                original: original, baseline: 0.5, scale: .linear, ramps: [ramp], released: nil,
                accuracy: .normalized(0.001)))
        XCTAssertThrowsError(
            try AutomationWriteFinalizer.finalize(
                original: original, baseline: 0.5, scale: .linear, ramps: [ramp],
                released: .init(lastFrame: 120, lastValue: 0.4, nextReadFrame: 121), accuracy: .normalized(0.001),
                cancelled: { true })
        ) { XCTAssertEqual($0 as? AutomationWriteFinalizerError, .cancelled) }
    }
    func testDuplicateOriginalFramesRejectWithoutNormalizingUntouchedMetadata() throws {
        let original: [AutomationPoint] = [
            .init(frame: 20, value: 0.2, curve: .easeIn, curveStart: 0.1, curveEnd: 0.6),
            .init(frame: 20, value: 0.8, curve: .smooth, curveStart: 0.3, curveEnd: 0.9),
        ]
        let preserved = original
        XCTAssertThrowsError(
            try AutomationWriteFinalizer.finalize(
                original: original, baseline: 0.5, scale: .linear,
                ramps: [.init(frame: 100, start: 0.5, target: 0.2, duration: 256)],
                released: .init(lastFrame: 120, lastValue: 0.4, nextReadFrame: 121), accuracy: .normalized(0.001))
        ) { XCTAssertEqual($0 as? AutomationWriteFinalizerError, .invalidCapture) }
        XCTAssertEqual(original, preserved)
        XCTAssertEqual(
            try AutomationWriteFinalizer.finalize(
                original: original, baseline: 0.5, scale: .linear, ramps: [], released: nil,
                accuracy: .normalized(0.001)), original)
    }
}
