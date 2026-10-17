import Foundation

/// Control-thread preparation reserves stage references and shared scratch. Live
/// adoption inserts existing decays before their next surviving original insert.
/// The retirement reserve bounds simultaneous tails from repeated rapid edits.
final class EffectTransitionChain {
    private let active: [EffectTransitionStage]
    private var slots: [EffectTransitionStage?]
    private var slotCount: Int
    private var retirementCount = 0
    private var originalLeft = [Float](repeating: 0, count: 8192)
    private var originalRight = [Float](repeating: 0, count: 8192)
    private var gains = [Float](repeating: 0, count: 8192)
    private static let retirementReserve = 256
    init(processors: [EffectProcessor]) {
        var downstreamLatency = 0
        for processor in processors.reversed() {
            if processor.effect.kind == .pitchCorrection {
                processor.pitchMeterDelay = PitchMeterDelay(frames: downstreamLatency)
            }
            downstreamLatency += processor.effect.latencyFrames
        }
        active = processors.map { EffectTransitionStage(processor: $0) }
        slotCount = active.count
        slots = [EffectTransitionStage?](repeating: nil, count: active.count + Self.retirementReserve)
        for i in active.indices { slots[i] = active[i] }
    }
    private func survives(_ stage: EffectTransitionStage) -> Bool {
        !stage.retiring
            && active.contains {
                $0.processor.effect.id == stage.processor.effect.id
                    && $0.processor.effect.kind == stage.processor.effect.kind
                    && $0.processor.usesLegacy == stage.processor.usesLegacy
            }
    }
    private func nextAnchor(after index: Int, in previous: EffectTransitionChain) -> UUID? {
        guard index + 1 < previous.slotCount else { return nil }
        for i in (index + 1)..<previous.slotCount {
            if let stage = previous.slots[i], survives(stage) { return stage.processor.effect.id }
        }
        return nil
    }
    private func appendRetired(anchoredTo anchor: UUID?, from previous: EffectTransitionChain) {
        for i in 0..<previous.slotCount {
            guard let stage = previous.slots[i], !stage.completed, !survives(stage),
                nextAnchor(after: i, in: previous) == anchor
            else { continue }
            // Latency-changing replacements are handled as aligned whole-track
            // transitions by the graph owner, rather than inserting delayed tails.
            guard stage.processor.effect.latencyFrames == 0 else { continue }
            guard retirementCount < Self.retirementReserve else { continue }
            stage.beginRetirement()
            slots[slotCount] = stage
            slotCount += 1
            retirementCount += 1
        }
    }
    func prepareLiveTransition(from previous: EffectTransitionChain) {
        for i in 0..<slotCount { slots[i] = nil }
        slotCount = 0
        retirementCount = 0
        for stage in active {
            appendRetired(anchoredTo: stage.processor.effect.id, from: previous)
            let existed = (0..<previous.slotCount).contains { index in
                guard let old = previous.slots[index], !old.retiring else { return false }
                return old.processor.effect.id == stage.processor.effect.id
                    && old.processor.effect.kind == stage.processor.effect.kind
                    && old.processor.usesLegacy == stage.processor.usesLegacy
            }
            if !existed, stage.processor.effect.enabled {
                stage.processor.bypassBlend = 0
                stage.processor.bypassRamp = LiveRamp(from: 0, to: 1)
            }
            slots[slotCount] = stage
            slotCount += 1
        }
        appendRetired(anchoredTo: nil, from: previous)
    }
    var remainingTailFrames: Int {
        (0..<slotCount).reduce(0) { max($0, slots[$1]?.remainingFrames ?? 0) }
    }
    func reserveCapacity(frameCount: Int) {
        if frameCount > originalLeft.count {
            originalLeft = [Float](repeating: 0, count: frameCount)
            originalRight = [Float](repeating: 0, count: frameCount)
            gains = [Float](repeating: 0, count: frameCount)
        }
    }
    func process(
        left: UnsafeMutablePointer<Float>, right: UnsafeMutablePointer<Float>, count: Int, position: Int,
        loopRange: Range<Int>? = nil
    ) {
        originalLeft.withUnsafeMutableBufferPointer { ol in
            originalRight.withUnsafeMutableBufferPointer { or in
                gains.withUnsafeMutableBufferPointer { g in
                    var sourcePosition = position
                    for i in 0..<slotCount {
                        guard let stage = slots[i] else { continue }
                        stage.processor.pitchMeterLoopRange = loopRange
                        var offset = 0
                        while offset < count {
                            let frame =
                                loopRange.map {
                                    $0.lowerBound + ((sourcePosition + offset - $0.lowerBound) % $0.count + $0.count)
                                        % $0.count
                                } ?? sourcePosition + offset
                            let block = min(count - offset, loopRange.map { $0.upperBound - frame } ?? count)
                            stage.process(
                                left: left + offset, right: right + offset, count: block, position: frame,
                                originalLeft: ol.baseAddress! + offset, originalRight: or.baseAddress! + offset,
                                gains: g.baseAddress! + offset)
                            offset += block
                        }
                        sourcePosition -= stage.processor.effect.latencyFrames
                    }
                }
            }
        }
    }
}
