import Foundation

/// Fixed storage and fractional reads keep modulation continuous across audio blocks.
struct ChorusState {
    private var left = [Float](repeating: 0, count: 4096)
    private var right = [Float](repeating: 0, count: 4096)
    private var cursor = 0
    private var phase: Double = 0

    mutating func process(left inputL: Float, right inputR: Float, rate: Float, depth: Float,
                          delay: Float, width: Float, feedback: Float, mix: Float) -> (Float, Float) {
        if mix == 0 && feedback == 0 {
            left[cursor] = inputL; right[cursor] = inputR; cursor = (cursor + 1) % left.count
            phase += 2 * Double.pi * Double(rate) / sessionRate
            if phase >= 2 * Double.pi { phase -= 2 * Double.pi }
            return (inputL, inputR)
        }
        let delayL = (delay + depth * Float((sin(phase) + 1) * 0.5)) * Float(sessionRate) / 1000
        let delayR = (delay + depth * Float((sin(phase + Double.pi * Double(width)) + 1) * 0.5)) * Float(sessionRate) / 1000
        let wetL = read(left, delay: delayL), wetR = read(right, delay: delayR)
        left[cursor] = inputL + wetL * feedback
        right[cursor] = inputR + wetR * feedback
        cursor = (cursor + 1) % left.count
        phase += 2 * Double.pi * Double(rate) / sessionRate
        if phase >= 2 * Double.pi { phase -= 2 * Double.pi }
        return (inputL * (1 - mix) + wetL * mix, inputR * (1 - mix) + wetR * mix)
    }

    var seekStorageBytes: Int { (left.count + right.count) * MemoryLayout<Float>.stride + 64 }
    func seekCopy() -> ChorusState {
        var copy = self
        copy.left = left.withUnsafeBufferPointer { Array($0) }; copy.right = right.withUnsafeBufferPointer { Array($0) }
        return copy
    }

    private func read(_ buffer: [Float], delay: Float) -> Float {
        let position = Float(cursor) - max(1, min(Float(buffer.count - 2), delay))
        let lower = Int(floor(position)), fraction = position - Float(lower)
        let first = (lower % buffer.count + buffer.count) % buffer.count
        return buffer[first] * (1 - fraction) + buffer[(first + 1) % buffer.count] * fraction
    }
}

/// Counter-based TPDF: repeatable renders and seeks without maintaining RNG history.
enum DitherDSP {
    private static func uniform(_ key: UInt64) -> Float {
        var value = key &+ 0x9e3779b97f4a7c15
        value = (value ^ (value >> 30)) &* 0xbf58476d1ce4e5b9
        value = (value ^ (value >> 27)) &* 0x94d049bb133111eb
        value ^= value >> 31
        return Float(value >> 40) / 16_777_216
    }

    static func process(_ input: Float, frame: Int, channel: UInt64, seed: UInt64,
                        bits: Float, amount: Float, quantize: Bool) -> Float {
        let step = pow(Float(2), 1 - max(8, min(24, bits.rounded())))
        let key = seed ^ (UInt64(truncatingIfNeeded: frame) &* 0xd6e8feb86659fd93)
            ^ (channel &* 0xa0761d6478bd642f)
        let noise = (uniform(key) - uniform(key ^ 0xe7037ed1a0b428db)) * step * amount
        let result = input + noise
        return quantize ? (result / step).rounded() * step : result
    }
}
