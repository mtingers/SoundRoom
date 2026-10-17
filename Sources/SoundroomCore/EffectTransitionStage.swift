import Foundation

/// A prepared graph stage. Removed zero-latency processors release their input
/// smoothly and continue emitting their existing decay at their original chain position.
/// Scratch belongs to the containing track and is reused between retired stages.
final class EffectTransitionStage {
    let processor: EffectProcessor
    private(set) var retiring = false
    private var excitation: LiveRamp?
    private var excitationGain: Float = 1
    private(set) var remainingFrames = 0
    var completed: Bool { retiring && remainingFrames == 0 }
    init(processor: EffectProcessor) { self.processor = processor }
    func beginRetirement() {
        guard !retiring else { return }
        retiring = true
        excitation = LiveRamp(from: 1, to: 0)
        remainingFrames = LiveRamp.frameCount + min(Int(sessionRate * 30), processor.effect.tailFrames)
    }
    func process(
        left: UnsafeMutablePointer<Float>, right: UnsafeMutablePointer<Float>, count: Int, position: Int,
        originalLeft: UnsafeMutablePointer<Float>, originalRight: UnsafeMutablePointer<Float>,
        gains: UnsafeMutablePointer<Float>
    ) {
        guard retiring else {
            processor.process(left: left, right: right, count: count, position: position)
            return
        }
        let active = min(count, remainingFrames)
        guard active > 0 else { return }
        for i in 0..<active {
            originalLeft[i] = left[i]
            originalRight[i] = right[i]
            if excitation != nil {
                excitationGain = excitation!.next()
                if excitation!.completed { excitation = nil }
            }
            gains[i] = excitationGain
            left[i] *= excitationGain
            right[i] *= excitationGain
        }
        processor.process(left: left, right: right, count: active, position: position)
        for i in 0..<active {
            // Generic wet-minus-excitation works for parallel inserts as well as
            // legacy additive delay. Downstream inserts still process the decay.
            let ending = min(1, Float(remainingFrames - i) / Float(LiveRamp.frameCount))
            left[i] = originalLeft[i] + (left[i] - originalLeft[i] * gains[i]) * ending
            right[i] = originalRight[i] + (right[i] - originalRight[i] * gains[i]) * ending
        }
        remainingFrames -= active
    }
}
