import Foundation

/// Independent stereo, audio-driven version of the supplied 300-lane model.
/// All histories/coefficient storage are allocated before processing begins.
final class SoundPuttyDSP {
    var storageBytes: Int { 64 * 1024 }
    private var x = [Double](repeating: 0, count: 600)
    private var velocity = [Double](repeating: 0, count: 600)
    private var next = [Double](repeating: 0, count: 600)
    private var shape = [Double](repeating: 0, count: 900)
    private var spring = [Double](repeating: 0, count: 300)
    private var damping = [Double](repeating: 0, count: 300)
    private var coupling = [Double](repeating: 0, count: 300)
    private var weight = [Double](repeating: 0, count: 300)
    private var targetSpring = [Double](repeating: 0, count: 300)
    private var targetDamping = [Double](repeating: 0, count: 300)
    private var targetCoupling = [Double](repeating: 0, count: 300)
    private var targetWeight = [Double](repeating: 0, count: 300)
    private var controls: [Double] = [95, 560, 1.2, 160, 22, 0.7, 0, 0, 145, 85, 0.075, 1.8, 0, 0.4, 0, 1234]
    private var dirty = true, initialized = false, gliding = false
    private var phase = 0
    private var driveGain: Double = 1, outputGain: Double = 1
    private var center = sqrt(95.0 * 560)
    private var blowAlpha = 1 - exp(-2 * Double.pi * sqrt(95.0 * 560) * 2 / 48_000)
    private var previousInput = [Double](repeating: 0, count: 2)
    private var dc = [Double](repeating: 0, count: 2)
    private var blow = [Double](repeating: 0, count: 2)

    func setShape(_ value: SoundPuttyShape) {
        for axis in 0..<3 { for lane in 0..<300 { shape[axis * 300 + lane] = value.rows[axis][lane] } }
        dirty = true
    }
    func configure(_ values: [Float]) {
        for i in 0..<min(16, values.count) {
            let value = Double(values[i])
            if (i < 12 || i == 15), controls[i] != value { dirty = true }
            controls[i] = value
        }
        driveGain = pow(10, controls[12] / 20); outputGain = pow(10, controls[14] / 20)
        center = sqrt(controls[0] * controls[1])
        blowAlpha = 1 - exp(-2 * Double.pi * min(12_000, center * 2) / 48_000)
    }
    private func rebuild() {
        for i in 0..<300 {
            let q = pow(Double(i) / 299, controls[2])
            let zone = exp(-0.5 * pow((Double(i) - controls[3]) / controls[4], 2)) * controls[5]
            var frequency = controls[0] * pow(controls[1] / controls[0], q) * exp(shape[i] * 0.65)
            if controls[6].rounded() == 0 { frequency *= exp(zone * SoundPuttyShape.random(lane: i, seed: UInt32(controls[15].rounded())) * 1.5) }
            let loss = max(0.25, exp(shape[600 + i] * 2) / controls[11] + (controls[6].rounded() == 2 ? zone * 15 : 0))
            targetSpring[i] = pow(2 * Double.pi * max(15, min(13_800, frequency)) / 48_000, 2)
            targetDamping[i] = 2 * loss / 48_000
            targetCoupling[i] = (controls[10] + (controls[6].rounded() == 3 ? zone * 0.28 : 0)) * 1e6 / (48_000 * 48_000)
            targetWeight[i] = exp(-0.5 * pow((Double(i) - controls[8]) / controls[9], 2)) * exp(shape[300 + i] * 2)
                * (controls[6].rounded() == 1 ? 1 - 2 * zone : 1)
        }
        if !initialized {
            for i in 0..<300 { spring[i] = targetSpring[i]; damping[i] = targetDamping[i]; coupling[i] = targetCoupling[i]; weight[i] = targetWeight[i] }
            initialized = true
        } else { gliding = true }
        dirty = false
    }
    func process(left: Float, right: Float) -> (Float, Float) {
        if phase == 0, dirty { rebuild() }
        if gliding {
            let remaining = Double(64 - phase)
            for i in 0..<300 {
                spring[i] += (targetSpring[i] - spring[i]) / remaining
                damping[i] += (targetDamping[i] - damping[i]) / remaining
                coupling[i] += (targetCoupling[i] - coupling[i]) / remaining
                weight[i] += (targetWeight[i] - weight[i]) / remaining
            }
        }
        let l = channel(left, index: 0), r = channel(right, index: 1)
        phase = (phase + 1) % 64
        if phase == 0 { gliding = false }
        return (l, r)
    }
    private func channel(_ input: Float, index: Int) -> Float {
        let dry = input.isFinite ? Double(input) : 0
        let source = max(-4, min(4, dry * driveGain))
        dc[index] += 0.002614570 * (source - dc[index])
        let drive = source - dc[index]
        blow[index] += blowAlpha * (drive - blow[index])
        let excitation: Double
        switch Int(controls[7].rounded()) {
        case 0: excitation = max(-4, min(4, (drive - previousInput[index]) * 48_000 / (2 * Double.pi * center)))
        case 1: excitation = drive
        default: excitation = blow[index]
        }
        previousInput[index] = drive
        let base = index * 300
        let cSpring = spring, cDamping = damping, cCoupling = coupling, cWeight = weight
        let sum: Double = x.withUnsafeMutableBufferPointer { positions in
            velocity.withUnsafeMutableBufferPointer { velocities in
                next.withUnsafeMutableBufferPointer { pending in
                    for i in 0..<300 {
                        let position = base + i, current = positions[position]
                        let left = positions[base + max(0, i - 1)], right = positions[base + min(299, i + 1)]
                        pending[position] = (1 - cDamping[i]) * velocities[position] - cSpring[i] * current
                            + cCoupling[i] * (left + right - 2 * current) + cWeight[i] * excitation * (100_000 / (48_000 * 48_000))
                    }
                    var sum: Double = 0
                    for i in 0..<300 {
                        let position = base + i, v = pending[position], value = positions[position] + v
                        if !value.isFinite || !v.isFinite { positions[position] = 0; velocities[position] = 0 }
                        else { positions[position] = abs(value) < 1e-20 ? 0 : max(-32, min(32, value)); velocities[position] = max(-32, min(32, v)) }
                        sum += positions[position]
                    }
                    return sum
                }
            }
        }
        let wet = tanh(sum * (4.0 / 300))
        return Float(dry * (1 - controls[13]) + wet * controls[13] * outputGain)
    }
    func deepCopy() -> SoundPuttyDSP {
        let copy = SoundPuttyDSP()
        copy.x = x.map { $0 }; copy.velocity = velocity.map { $0 }; copy.next = next.map { $0 }
        copy.shape = shape.map { $0 }; copy.spring = spring.map { $0 }; copy.damping = damping.map { $0 }
        copy.coupling = coupling.map { $0 }; copy.weight = weight.map { $0 }
        copy.targetSpring = targetSpring.map { $0 }; copy.targetDamping = targetDamping.map { $0 }
        copy.targetCoupling = targetCoupling.map { $0 }; copy.targetWeight = targetWeight.map { $0 }
        copy.controls = controls.map { $0 }; copy.dirty = dirty; copy.initialized = initialized; copy.gliding = gliding; copy.phase = phase
        copy.driveGain = driveGain; copy.outputGain = outputGain; copy.center = center; copy.blowAlpha = blowAlpha
        copy.previousInput = previousInput.map { $0 }; copy.dc = dc.map { $0 }; copy.blow = blow.map { $0 }
        return copy
    }
}
