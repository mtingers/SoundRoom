import Foundation

/// Eight-band channel EQ with three optional dynamic bells. Double precision protects low-frequency poles;
/// both bell and shelf histories stay warm during 5 ms shape/cut crossfades.
final class EqualizerDSP {
    var left = Array(repeating: EQSection(), count: 10)
    var right = Array(repeating: EQSection(), count: 10)
    var blends = [Double](repeating: 0, count: 4)
    var targets = [Double](repeating: 0, count: 4)
    var previous = [Float](repeating: .nan, count: 33)
    var initialized = false
    var output: Double = 1
    var detectorL = [EQSection](repeating: EQSection(), count: 3), detectorR = [EQSection](repeating: EQSection(), count: 3)
    var envelopes = [Float](repeating: 0, count: 3), dynamic = [Float](repeating: 0, count: 3)
    var tick = 0
    var attack: Float = 0, release: Float = 0
    func configure(_ values: [Float]) {
        // Automation updates only the affected sections. Arrays remain unique;
        // storing the caller's values array here would cause callback COW copies.
        if previous[0] != values[0] || previous[6] != values[6] || previous[7] != values[7] {
            left[0].configure(frequency: values[6], q: values[7], gain: values[0], shape: .bell)
            left[1].configure(frequency: values[6], q: values[7], gain: values[0], shape: .lowShelf)
            right[0].copyCoefficients(from: left[0]); right[1].copyCoefficients(from: left[1])
        }
        if previous[1] != values[1] || previous[2] != values[2] || previous[3] != values[3] {
            left[2].configure(frequency: values[1], q: values[3], gain: values[2], shape: .bell)
            right[2].copyCoefficients(from: left[2])
        }
        if previous[4] != values[4] || previous[9] != values[9] || previous[10] != values[10] {
            left[3].configure(frequency: values[9], q: values[10], gain: values[4], shape: .bell)
            left[4].configure(frequency: values[9], q: values[10], gain: values[4], shape: .highShelf)
            right[3].copyCoefficients(from: left[3]); right[4].copyCoefficients(from: left[4])
        }
        if previous[12] != values[12] {
            left[5].configure(frequency: values[12], q: Float(1 / sqrt(2.0)), gain: 0, shape: .highPass)
            right[5].copyCoefficients(from: left[5])
        }
        if previous[14] != values[14] {
            left[6].configure(frequency: values[14], q: Float(1 / sqrt(2.0)), gain: 0, shape: .lowPass)
            right[6].copyCoefficients(from: left[6])
        }
        if previous[5] != values[5] { output = pow(10, Double(values[5]) / 20) }
        for n in 0..<3 {
            let k = 16 + n * 5
            if previous[k] != values[k] || previous[k+2] != values[k+2] {
                detectorL[n].configure(frequency: values[k], q: values[k+2], gain: 0, shape: .bandPass)
                detectorR[n].copyCoefficients(from: detectorL[n])
            }
            if previous[k] != values[k] || previous[k+1] != values[k+1] || previous[k+2] != values[k+2] {
                left[7+n].configure(frequency: values[k], q: values[k+2], gain: values[k+1]+dynamic[n], shape: .bell)
                right[7+n].copyCoefficients(from: left[7+n])
            }
        }
        attack = 1-exp(-1/(values[31]*48)); release = 1-exp(-1/(values[32]*48))
        for i in previous.indices { previous[i] = values[i] }
        targets[0] = Double(values[8]); targets[1] = Double(values[11])
        targets[2] = Double(values[13]); targets[3] = Double(values[15])
        if !initialized { for i in blends.indices { blends[i] = targets[i] }; initialized = true }
    }
    static func configure(_ sections: inout [EQSection], values v: [Float]) {
        sections[0].configure(frequency: v[6], q: v[7], gain: v[0], shape: .bell)
        sections[1].configure(frequency: v[6], q: v[7], gain: v[0], shape: .lowShelf)
        sections[2].configure(frequency: v[1], q: v[3], gain: v[2], shape: .bell)
        sections[3].configure(frequency: v[9], q: v[10], gain: v[4], shape: .bell)
        sections[4].configure(frequency: v[9], q: v[10], gain: v[4], shape: .highShelf)
        sections[5].configure(frequency: v[12], q: Float(1 / sqrt(2.0)), gain: 0, shape: .highPass)
        sections[6].configure(frequency: v[14], q: Float(1 / sqrt(2.0)), gain: 0, shape: .lowPass)
        for n in 0..<3 { let k = 16+n*5; sections[7+n].configure(frequency: v[k], q: v[k+2], gain: v[k+1], shape: .bell) }
    }
    func process(left inputL: Float, right inputR: Float) -> (Float, Float) {
        for i in blends.indices {
            let difference = targets[i] - blends[i]
            blends[i] += max(-1 / 240.0, min(1 / 240.0, difference))
        }
        for n in 0..<3 {
            let k = 16 + n * 5
            if previous[k+4] == 0 && dynamic[n] == 0 { continue }
            let level = Float(max(abs(detectorL[n].process(Double(inputL))), abs(detectorR[n].process(Double(inputR)))))
            let coefficient = level > envelopes[n] ? attack : release
            envelopes[n] += coefficient * (level-envelopes[n])
            let db = 20 * log10(max(1e-9, envelopes[n]))
            let cut = max(previous[k+4], -max(0, db - previous[k+3]))
            if tick == 0 && abs(cut - dynamic[n]) > 0.01 {
                dynamic[n] = cut
                left[7+n].configure(frequency: previous[k], q: previous[k+2], gain: previous[k+1]+cut, shape: .bell)
                right[7+n].copyCoefficients(from: left[7+n])
            }
        }
        tick = (tick + 1) % 32
        return (Float(process(Double(inputL), sections: &left) * output), Float(process(Double(inputR), sections: &right) * output))
    }
    private func process(_ input: Double, sections: inout [EQSection]) -> Double {
        let lowBell = sections[0].process(input), lowShelf = sections[1].process(input)
        let mid = sections[2].process(lowBell + (lowShelf - lowBell) * blends[0])
        let highBell = sections[3].process(mid), highShelf = sections[4].process(mid)
        let high = highBell + (highShelf - highBell) * blends[1]
        let cutLow = sections[5].process(high)
        let passedLow = high + (cutLow - high) * blends[2]
        let cutHigh = sections[6].process(passedLow)
        var y = passedLow + (cutHigh - passedLow) * blends[3]
        for n in 7..<10 { y = sections[n].process(y) }
        return y
    }
    var storageBytes: Int { (left.count + right.count + detectorL.count + detectorR.count) * MemoryLayout<EQSection>.stride + previous.count * 4 + 256 }
    func deepCopy() -> EqualizerDSP {
        let copy = EqualizerDSP()
        copy.left = left.withUnsafeBufferPointer { Array($0) }; copy.right = right.withUnsafeBufferPointer { Array($0) }
        copy.blends = blends.withUnsafeBufferPointer { Array($0) }; copy.targets = targets.withUnsafeBufferPointer { Array($0) }
        copy.previous = previous.withUnsafeBufferPointer { Array($0) }
        copy.detectorL = detectorL.withUnsafeBufferPointer { Array($0) }; copy.detectorR = detectorR.withUnsafeBufferPointer { Array($0) }
        copy.envelopes = envelopes.withUnsafeBufferPointer { Array($0) }; copy.dynamic = dynamic.withUnsafeBufferPointer { Array($0) }; copy.tick = tick; copy.attack = attack; copy.release = release
        copy.initialized = initialized; copy.output = output
        return copy
    }
}

struct EQSection: Sendable {
    enum Shape { case bell, lowShelf, highShelf, highPass, lowPass, bandPass }
    var b0: Double = 1, b1: Double = 0, b2: Double = 0, a1: Double = 0, a2: Double = 0
    var z1: Double = 0, z2: Double = 0
    mutating func process(_ input: Double) -> Double {
        let result = b0 * input + z1
        z1 = b1 * input - a1 * result + z2; z2 = b2 * input - a2 * result
        if !result.isFinite || !z1.isFinite || !z2.isFinite { z1 = 0; z2 = 0; return 0 }
        // Flush inaudible denormals, without adding noise to quiet tails.
        if abs(z1) < 1e-30 { z1 = 0 }; if abs(z2) < 1e-30 { z2 = 0 }
        return result
    }
    mutating func configure(frequency: Float, q: Float, gain: Float, shape: Shape) {
        let omega = 2 * Double.pi * min(sessionRate * 0.45, max(20, Double(frequency))) / sessionRate
        let c = cos(omega), alpha = sin(omega) / (2 * Double(q)), a = pow(10, Double(gain) / 40)
        var a0: Double
        switch shape {
        case .bell:
            a0 = 1 + alpha / a; b0 = 1 + alpha * a; b1 = -2 * c; b2 = 1 - alpha * a
            a1 = -2 * c; a2 = 1 - alpha / a
        case .lowShelf:
            let beta = 2 * sqrt(a) * alpha
            a0 = (a + 1) + (a - 1) * c + beta
            b0 = a * ((a + 1) - (a - 1) * c + beta); b1 = 2 * a * ((a - 1) - (a + 1) * c)
            b2 = a * ((a + 1) - (a - 1) * c - beta)
            a1 = -2 * ((a - 1) + (a + 1) * c); a2 = (a + 1) + (a - 1) * c - beta
        case .highShelf:
            let beta = 2 * sqrt(a) * alpha
            a0 = (a + 1) - (a - 1) * c + beta
            b0 = a * ((a + 1) + (a - 1) * c + beta); b1 = -2 * a * ((a - 1) + (a + 1) * c)
            b2 = a * ((a + 1) + (a - 1) * c - beta)
            a1 = 2 * ((a - 1) - (a + 1) * c); a2 = (a + 1) - (a - 1) * c - beta
        case .highPass:
            a0 = 1 + alpha; b0 = (1 + c) / 2; b1 = -(1 + c); b2 = b0; a1 = -2 * c; a2 = 1 - alpha
        case .bandPass:
            a0 = 1 + alpha; b0 = alpha; b1 = 0; b2 = -alpha; a1 = -2 * c; a2 = 1 - alpha
        case .lowPass:
            a0 = 1 + alpha; b0 = (1 - c) / 2; b1 = 1 - c; b2 = b0; a1 = -2 * c; a2 = 1 - alpha
        }
        b0 /= a0; b1 /= a0; b2 /= a0; a1 /= a0; a2 /= a0
    }
    mutating func copyCoefficients(from other: EQSection) {
        b0 = other.b0; b1 = other.b1; b2 = other.b2; a1 = other.a1; a2 = other.a2
    }
    func response(at frequency: Double) -> (real: Double, imaginary: Double) {
        let omega = 2 * Double.pi * min(sessionRate / 2, max(0, frequency)) / sessionRate
        let c = cos(omega), s = sin(omega), c2 = cos(2 * omega), s2 = sin(2 * omega)
        let nr = b0 + b1 * c + b2 * c2, ni = -b1 * s - b2 * s2
        let dr = 1 + a1 * c + a2 * c2, di = -a1 * s - a2 * s2
        let denominator = max(1e-30, dr * dr + di * di)
        return ((nr * dr + ni * di) / denominator, (ni * dr - nr * di) / denominator)
    }
}
