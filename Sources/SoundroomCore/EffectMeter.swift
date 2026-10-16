import Foundation

/// Feedback for the source sample currently leaving the pitch stage, after its
/// analysis/synthesis latency. Estimated wet pitch is not a measurement of the
/// mixed output, particularly while dry and wet voices are both audible.
public struct PitchMeterSnapshot: Equatable, Codable, Sendable {
    public var inputFrequency: Float = 0, targetFrequency: Float = 0
    public var correctionCents: Float = 0, confidence: Float = 0, wetBlend: Float = 0
    public var referenceFrequency: Float = 440
    public var sourceFrame: Int = -1
    public init() {}

    public var isVoiced: Bool {
        inputFrequency.isFinite && (50...1000).contains(inputFrequency) &&
            confidence.isFinite && confidence > 0.75 && sourceFrame >= 0
    }
    public var estimatedWetFrequency: Float {
        guard isVoiced, correctionCents.isFinite else { return 0 }
        return inputFrequency * pow(2, max(-1200, min(1200, correctionCents)) / 1200)
    }
    public func midiNote(for frequency: Float) -> Float? {
        guard frequency.isFinite, frequency > 0, referenceFrequency.isFinite, referenceFrequency > 0 else { return nil }
        return 69 + 12 * log2(frequency / referenceFrequency)
    }
}

public struct EffectMeterSnapshot: Equatable, Codable, Sendable {
    public var inputPeak: Float = 0, outputPeak: Float = 0
    public var inputRMS: Float = 0, outputRMS: Float = 0
    /// Positive attenuation in dB, independent of makeup gain.
    public var gainReductionDB: Float = 0
    public var truePeak: Float = 0
    public var momentaryLUFS: Float = -120, shortTermLUFS: Float = -120
    public var resonance = SpectralReductionSnapshot()
    public var frameCount: Int = 0
    public var pitch: PitchMeterSnapshot?
    public var revision: UInt64 = 0
    public init() {}
}

/// Delay visual feedback by the latency of subsequent inserts, just as those
/// inserts delay audio. Prepared outside render; every callback write is a
/// fixed-size value into exclusively owned storage, with no allocation/lock.
final class PitchMeterDelay {
    private var samples: [PitchMeterSnapshot]
    private var index = 0
    init(frames: Int) { samples = Array(repeating: PitchMeterSnapshot(), count: max(0, frames)) }
    var storageBytes: Int { samples.count * MemoryLayout<PitchMeterSnapshot>.stride + 32 }
    func deepCopy() -> PitchMeterDelay {
        let copy = PitchMeterDelay(frames: 0)
        copy.samples = samples.map { $0 }; copy.index = index
        return copy
    }
    func process(_ value: PitchMeterSnapshot) -> PitchMeterSnapshot {
        guard !samples.isEmpty else { return value }
        let delayed = samples[index]
        samples[index] = value
        index += 1
        if index == samples.count { index = 0 }
        return delayed
    }
}

/// The audio producer only tries the lock. A busy reader drops a meter update,
/// never waits on the callback or interrupts audio. Snapshot reads belong off audio.
public final class EffectMeter {
    private let lock = NSLock()
    private var value = EffectMeterSnapshot()
    public init() {}
    public func snapshot() -> EffectMeterSnapshot { lock.lock(); defer { lock.unlock() }; return value }
    func publish(_ snapshot: EffectMeterSnapshot) {
        guard lock.try() else { return }
        let next = value.revision &+ 1
        value = snapshot; value.revision = next; lock.unlock()
    }
}

struct EffectMeterAccumulator {
    var snapshot = EffectMeterSnapshot()
    private var inputSquares: Double = 0, outputSquares: Double = 0
    mutating func add(inputL: Float, inputR: Float, outputL: Float, outputR: Float, reduction: Float = 0) {
        snapshot.inputPeak = max(snapshot.inputPeak, abs(inputL), abs(inputR))
        snapshot.outputPeak = max(snapshot.outputPeak, abs(outputL), abs(outputR))
        inputSquares += Double(inputL) * Double(inputL) + Double(inputR) * Double(inputR)
        outputSquares += Double(outputL) * Double(outputL) + Double(outputR) * Double(outputR)
        snapshot.gainReductionDB = max(snapshot.gainReductionDB, reduction)
        snapshot.frameCount += 1
    }
    mutating func finish() -> EffectMeterSnapshot {
        if snapshot.frameCount > 0 {
            snapshot.inputRMS = Float(sqrt(inputSquares / Double(snapshot.frameCount * 2)))
            snapshot.outputRMS = Float(sqrt(outputSquares / Double(snapshot.frameCount * 2)))
        }
        return snapshot
    }
}
