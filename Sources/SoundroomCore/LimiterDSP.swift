import Foundation

/// ITU-R BS.1770-5 Annex 2, floating-point 4-phase peak estimator.
/// The filter's six-frame history delay is included in the limiter's look-ahead.
final class TruePeakDSP {
    private static let coefficients: [[Float]] = [
        [0.001708984375,0.010986328125,-0.0196533203125,0.033203125,-0.0594482421875,0.1373291015625,0.97216796875,-0.102294921875,0.047607421875,-0.026611328125,0.014892578125,-0.00830078125],
        [-0.0291748046875,0.029296875,-0.0517578125,0.089111328125,-0.16650390625,0.465087890625,0.77978515625,-0.2003173828125,0.1015625,-0.0582275390625,0.0330810546875,-0.0189208984375],
        [-0.0189208984375,0.0330810546875,-0.0582275390625,0.1015625,-0.2003173828125,0.77978515625,0.465087890625,-0.16650390625,0.089111328125,-0.0517578125,0.029296875,-0.0291748046875],
        [-0.00830078125,0.014892578125,-0.026611328125,0.047607421875,-0.102294921875,0.97216796875,0.1373291015625,-0.0594482421875,0.033203125,-0.0196533203125,0.010986328125,0.001708984375]
    ]
    private var left = [Float](repeating: 0, count: 12), right = [Float](repeating: 0, count: 12)
    private var index = 0
    init() { _ = Self.coefficients }
    func process(_ l: Float, _ r: Float) -> Float {
        left[index] = l; right[index] = r
        var peak = max(abs(l), abs(r))
        for coefficients in Self.coefficients {
            var a: Float = 0, b: Float = 0, cursor = index
            for c in coefficients { a += left[cursor] * c; b += right[cursor] * c; cursor = (cursor + 11) % 12 }
            peak = max(peak, abs(a), abs(b))
        }
        index = (index + 1) % 12; return peak
    }
    func deepCopy() -> TruePeakDSP {
        let c = TruePeakDSP(); c.left = left.map { $0 }; c.right = right.map { $0 }; c.index = index; return c
    }
}

/// Stereo BS.1770 K-weighting, exact 400 ms / 3 s sliding windows.
/// These are momentary and short-term readings, not gated integrated loudness.
final class LoudnessDSP {
    private var shelfL = Biquad(b0: 1.53512485958697, b1: -2.69169618940638, b2: 1.19839281085285, a1: -1.69065929318241, a2: 0.73248077421585)
    private var shelfR = Biquad(b0: 1.53512485958697, b1: -2.69169618940638, b2: 1.19839281085285, a1: -1.69065929318241, a2: 0.73248077421585)
    private var highL = Biquad(b0: 1, b1: -2, b2: 1, a1: -1.99004745483398, a2: 0.99007225036621)
    private var highR = Biquad(b0: 1, b1: -2, b2: 1, a1: -1.99004745483398, a2: 0.99007225036621)
    private var powers = [Double](repeating: 0, count: 144000)
    private var cursor = 0, samples = 0, update = 0
    private var shortSum: Double = 0, momentSum: Double = 0
    private(set) var lastPower: Double = 0
    private(set) var momentary: Float = -120, shortTerm: Float = -120
    var storageBytes: Int { powers.count * 8 + 256 }
    func process(_ l: Float, _ r: Float) {
        let a = Double(highL.filter(shelfL.filter(l))), b = Double(highR.filter(shelfR.filter(r))), power = a * a + b * b
        lastPower = power
        momentSum += power - powers[(cursor + powers.count - 19200) % powers.count]
        shortSum += power - powers[cursor]; powers[cursor] = power
        cursor = (cursor + 1) % powers.count; samples = min(powers.count, samples + 1); update += 1
        if update == 480 {
            momentary = Float(max(-120, -0.691 + 10 * log10(max(1e-12, momentSum / 19200))))
            shortTerm = Float(max(-120, -0.691 + 10 * log10(max(1e-12, shortSum / 144000))))
            update = 0
        }
    }
    func deepCopy() -> LoudnessDSP {
        let c = LoudnessDSP(); c.shelfL = shelfL; c.shelfR = shelfR; c.highL = highL; c.highR = highR
        c.powers = powers.map { $0 }; c.cursor = cursor; c.samples = samples; c.update = update
        c.lastPower = lastPower; c.shortSum = shortSum; c.momentSum = momentSum; c.momentary = momentary; c.shortTerm = shortTerm; return c
    }
}

/// Stereo-linked 10.67 ms look-ahead. A monotonic deque bounds peak detection
/// work independently of block size. No callback allocation or blocking locks.
final class LimiterDSP {
    static let latencyFrames = 512
    private let capacity = latencyFrames + 16
    private var left = [Float](repeating: 0, count: latencyFrames), right = [Float](repeating: 0, count: latencyFrames)
    private var dryLeft = [Float](repeating: 0, count: latencyFrames), dryRight = [Float](repeating: 0, count: latencyFrames)
    private var ceilings = [Float](repeating: 1, count: latencyFrames)
    private var queueValues = [Float](repeating: 0, count: latencyFrames + 16)
    private var queueFrames = [Int64](repeating: 0, count: latencyFrames + 16)
    private var head = 0, tail = 0, cursor = 0
    private var frame: Int64 = 0
    private var inputGain: Float = 1, ceiling: Float = 0.89125094, release: Float = 0.00014
    private var adaptive = true, truePeak = true, gain: Float = 1
    private var inputDetector = TruePeakDSP(), outputDetector = TruePeakDSP()
    var loudness = LoudnessDSP()
    private(set) var reductionDB: Float = 0, outputTruePeak: Float = 0
    private(set) var lastDryLeft: Float = 0, lastDryRight: Float = 0
    var storageBytes: Int { loudness.storageBytes + (Self.latencyFrames * 5 + capacity) * 4 + capacity * 8 + 512 }
    func configure(_ v: [Float]) {
        inputGain = pow(10, v[0] / 20); ceiling = pow(10, v[1] / 20)
        release = 1 - exp(-1 / (v[2] * 0.001 * Float(sessionRate)))
        adaptive = v[3] >= 0.5; truePeak = v[4] >= 0.5
    }
    func process(left l: Float, right r: Float) -> (Float, Float) {
        let delayedLeft = left[cursor], delayedRight = right[cursor]
        lastDryLeft = dryLeft[cursor]; lastDryRight = dryRight[cursor]
        dryLeft[cursor] = l; dryRight[cursor] = r
        // Store boosted audio so input automation follows the source timeline.
        let a = l * inputGain, b = r * inputGain, delayedCeiling = ceilings[cursor]
        left[cursor] = a; right[cursor] = b; ceilings[cursor] = ceiling
        cursor = (cursor + 1) % Self.latencyFrames
        let reconstructed = inputDetector.process(a, b)
        // Peak estimates arrive six frames late; retain twelve extra frames to
        // cover the FIR neighborhood of an output sample and stereo transients.
        let peak = truePeak ? reconstructed / max(1e-6, ceiling) : max(abs(a), abs(b)) / max(1e-6, ceiling)
        while head != tail {
            let previous = (tail + capacity - 1) % capacity
            if queueValues[previous] > peak { break }; tail = previous
        }
        queueValues[tail] = peak; queueFrames[tail] = frame; tail = (tail + 1) % capacity
        while head != tail && queueFrames[head] < frame - Int64(Self.latencyFrames + 12) { head = (head + 1) % capacity }
        // 0.3 dB reconstruction margin in true-peak mode; sample mode has none.
        let margin: Float = truePeak ? 0.96605086 : 1
        let target = min(1, margin / max(1e-12, queueValues[head]))
        let coefficient: Float = target < gain ? 0.04 : release * (adaptive ? (gain > 0.8 ? 2 : 0.65) : 1)
        gain += coefficient * (target - gain)
        // Protect individual samples without waveform clipping. The anticipatory
        // detector normally reaches this bound well before the sample arrives.
        let outputGain = min(gain, delayedCeiling * margin / max(1e-12, abs(delayedLeft), abs(delayedRight)))
        let outL = delayedLeft * outputGain, outR = delayedRight * outputGain
        // Dry comparison must exclude input gain; it is part of the wet processor.
        // Original dry history remains bit-exact under input-gain automation.
        reductionDB = max(0, -20 * log10(max(1e-9, outputGain)))
        outputTruePeak = outputDetector.process(outL, outR); loudness.process(outL, outR)
        frame += 1; return (outL, outR)
    }
    func deepCopy() -> LimiterDSP {
        let c = LimiterDSP(); c.left = left.map { $0 }; c.right = right.map { $0 }; c.ceilings = ceilings.map { $0 }; c.dryLeft = dryLeft.map { $0 }; c.dryRight = dryRight.map { $0 }
        c.queueValues = queueValues.map { $0 }; c.queueFrames = queueFrames.map { $0 }
        c.head = head; c.tail = tail; c.cursor = cursor; c.frame = frame
        c.inputGain = inputGain; c.ceiling = ceiling; c.release = release; c.adaptive = adaptive; c.truePeak = truePeak; c.gain = gain
        c.inputDetector = inputDetector.deepCopy(); c.outputDetector = outputDetector.deepCopy(); c.loudness = loudness.deepCopy()
        c.reductionDB = reductionDB; c.outputTruePeak = outputTruePeak; c.lastDryLeft = lastDryLeft; c.lastDryRight = lastDryRight; return c
    }
}
