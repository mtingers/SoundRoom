import XCTest

@testable import SoundroomCore

final class EffectTransitionStageTests: XCTestCase {
    func testRemovedGainReleasesSmoothlyToDry() {
        let stage = EffectTransitionStage(processor: EffectProcessor(effect: RealtimeEffect(kind: .gain, amount: 0)))
        stage.beginRetirement()
        let output = render(stage, input: [Float](repeating: 0.3, count: 700), position: 0)
        XCTAssertLessThan(abs(output[0] - 0.3 * pow(10, -24.0 / 20)), 0.002)
        XCTAssertEqual(output[480], 0.3)
        XCTAssertTrue(stage.completed)
        XCTAssertLessThan(zip(output, output.dropFirst()).map { abs($0 - $1) }.max()!, 0.002)
    }
    func testRemovedLegacyDelayPreservesStoredEcho() {
        let processor = EffectProcessor(effect: RealtimeEffect(kind: .echo, amount: 1))
        let stage = EffectTransitionStage(processor: processor)
        var impulse = [Float](repeating: 0, count: 1000)
        impulse[0] = 0.5
        _ = render(stage, input: impulse, position: 0)
        stage.beginRetirement()
        let output = render(stage, input: [Float](repeating: 0, count: 12_000), position: 1000)
        XCTAssertEqual(output[11_000], 0.5 * 0.65, accuracy: 0.000001)
        XCTAssertGreaterThan(stage.remainingFrames, 0)
    }
    func testRetiredParallelDelayPreservesDryAndRemainingWet() {
        let effect = RealtimeEffect(
            kind: .echo, parameters: ["time": 20, "feedback": 0.5, "mix": 0.7, "damping": 20_000])
        let stage = EffectTransitionStage(processor: EffectProcessor(effect: effect))
        var impulse = [Float](repeating: 0, count: 500)
        impulse[0] = 0.5
        _ = render(stage, input: impulse, position: 0)
        stage.beginRetirement()
        let output = render(stage, input: [Float](repeating: 0.1, count: 1400), position: 500)
        XCTAssertGreaterThan(output[460], 0.4)
        XCTAssertGreaterThan(output[1300], 0.09)
        XCTAssertLessThan(output[1300], 0.13)
    }
    private func render(_ stage: EffectTransitionStage, input: [Float], position: Int) -> [Float] {
        var left = input
        var right = input
        var originalL = [Float](repeating: 0, count: input.count)
        var originalR = originalL
        var gains = originalL
        left.withUnsafeMutableBufferPointer { l in
            right.withUnsafeMutableBufferPointer { r in
                originalL.withUnsafeMutableBufferPointer { ol in
                    originalR.withUnsafeMutableBufferPointer { or in
                        gains.withUnsafeMutableBufferPointer { g in
                            stage.process(
                                left: l.baseAddress!, right: r.baseAddress!, count: l.count, position: position,
                                originalLeft: ol.baseAddress!, originalRight: or.baseAddress!, gains: g.baseAddress!)
                        }
                    }
                }
            }
        }
        return left
    }
}
