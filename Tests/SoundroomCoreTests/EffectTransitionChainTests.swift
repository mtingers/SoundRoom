import XCTest

@testable import SoundroomCore

final class EffectTransitionChainTests: XCTestCase {
    func testRemovedDelayRemainsBeforeOriginalDownstreamGainAcrossUpdates() {
        let echo = RealtimeEffect(kind: .echo, amount: 1)
        let gain = RealtimeEffect(kind: .gain, amount: 0)
        let old = EffectTransitionChain(processors: [EffectProcessor(effect: echo), EffectProcessor(effect: gain)])
        var impulse = [Float](repeating: 0, count: 1000)
        impulse[0] = 0.5
        _ = render(old, input: impulse, position: 0)
        let incoming = EffectTransitionChain(processors: [EffectProcessor(effect: gain)])
        incoming.prepareLiveTransition(from: old)
        _ = render(incoming, input: [Float](repeating: 0, count: 1000), position: 1000)
        let updated = EffectTransitionChain(processors: [EffectProcessor(effect: gain)])
        updated.prepareLiveTransition(from: incoming)
        let output = render(updated, input: [Float](repeating: 0, count: 11_000), position: 2000)
        XCTAssertEqual(output[10_000], 0.5 * 0.65 * pow(10, -24.0 / 20), accuracy: 0.000001)
        XCTAssertGreaterThan(updated.remainingTailFrames, 0)
    }
    func testInsertedEffectRampsFromDry() {
        let old = EffectTransitionChain(processors: [])
        let incoming = EffectTransitionChain(processors: [
            EffectProcessor(effect: RealtimeEffect(kind: .gain, amount: 0))
        ])
        incoming.prepareLiveTransition(from: old)
        let output = render(incoming, input: [Float](repeating: 0.3, count: 700), position: 0)
        XCTAssertEqual(output[0], 0.3, accuracy: 0.002)
        XCTAssertEqual(output[480], 0.3 * pow(10, -24.0 / 20), accuracy: 0.000001)
        XCTAssertLessThan(zip(output, output.dropFirst()).map { abs($0 - $1) }.max()!, 0.001)
    }
    func testRetirementReserveKeepsCapacityForAllActiveInserts() {
        let old = EffectTransitionChain(
            processors: (0..<300).map { _ in EffectProcessor(effect: RealtimeEffect(kind: .gain, amount: 0.5)) })
        let incoming = EffectTransitionChain(
            processors: (0..<2).map { _ in EffectProcessor(effect: RealtimeEffect(kind: .gain, amount: 0.5)) })
        incoming.prepareLiveTransition(from: old)
        XCTAssertEqual(render(incoming, input: [0.1], position: 0), [0.1])
    }
    private func render(_ chain: EffectTransitionChain, input: [Float], position: Int) -> [Float] {
        chain.reserveCapacity(frameCount: input.count)
        var left = input
        var right = input
        left.withUnsafeMutableBufferPointer { l in
            right.withUnsafeMutableBufferPointer { r in
                chain.process(left: l.baseAddress!, right: r.baseAddress!, count: l.count, position: position)
            }
        }
        return left
    }
}
