import Foundation

/// Vocal finishing with linked dynamics, gentle adaptive tone control and an
/// oversampled warmth stage. Histories are prepared before playback and cloned
/// explicitly; processing performs no allocation. Latency is 144 frames (3 ms).
/// Detectors estimate signal activity and spectral balance, not source identity.
final class VocalEnhancerDSP {
    static let latencyFrames = 144
    private static let defaults: [Float] = [0.45, 0, 0.45, 0.5, 0.45, 0.2, 0.2, 0, 1]
    private var controls = SIMD16<Double>(repeating: 0)
    private var trim: Float = 1
    private var rumbleFirst = VocalLowPass(hz: 65), rumbleSecond = VocalLowPass(hz: 65)
    private var body = VocalLowPass(hz: 180), ess = VocalLowPass(hz: 5500), air = VocalLowPass(hz: 9000)
    private var mud = VocalBandPass(hz: 320, q: 0.65), harsh = VocalBandPass(hz: 3000, q: 0.8)
    private var presence = VocalBandPass(hz: 2000, q: 0.65)
    private let times = VocalTimes()
    private var totalPower: Double = 0, voicePower: Double = 0, mudPower: Double = 0
    private var harshPower: Double = 0, essPower: Double = 0, slowPower: Double = 0
    private var noisePower: Double = 1e-7
    private var noiseBins = [Double](repeating: 0, count: 200)
    private var noiseCount = 0, noiseIndex = 0, noiseClock = 0
    private var periodicity = VocalPeriodicity()
    private var activity: Double = 0, cleanupDB: Double = 0, mudDB: Double = 0
    private var harshDB: Double = 0, essDB: Double = 0, levelDB: Double = 0
    private var compressionEnvelope: Double = 0, compressionDB: Double = 0
    private var warm = VocalWarmth(), guardDSP = VocalPeakGuard()
    private var dryL = [Float](repeating: 0, count: latencyFrames), dryR = [Float](repeating: 0, count: latencyFrames)
    private var mixes = [Float](repeating: 1, count: latencyFrames), reductions = [Float](repeating: 0, count: latencyFrames)
    private var dryIndex = 0, initialized = false
    private(set) var lastDryLeft: Float = 0, lastDryRight: Float = 0, reductionDB: Float = 0
    var storageBytes: Int { (200 * 8 + Self.latencyFrames * 4 * 4) + periodicity.storageBytes + warm.storageBytes + guardDSP.storageBytes + 1024 }

    init() { configure(Self.defaults) }

    /// Physical controls: cleanup, body dB, clarity, de-ess, dynamics, air,
    /// warmth, output dB, mix. Defensive clamping also protects standalone use.
    func configure(_ values: [Float]) {
        for i in 0..<9 {
            let value = i < values.count && values[i].isFinite ? values[i] : Self.defaults[i]
            switch i {
            case 1: controls[i] = max(-6, min(6, Double(value)))
            case 7: controls[i] = max(-12, min(12, Double(value)))
            default: controls[i] = Self.unit(Double(value))
            }
        }
        trim = Float(Self.gain(controls[7]))
    }

    func process(left: Float, right: Float) -> (Float, Float) {
        let xL = left.isFinite ? max(-16, min(16, left)) : 0
        let xR = right.isFinite ? max(-16, min(16, right)) : 0
        let v = controls
        if !initialized {
            for i in mixes.indices { mixes[i] = Float(v[8]) }
            initialized = true
        }
        let mix = mixes[dryIndex], oldReduction = reductions[dryIndex]
        // Mix before the peak guard, using the dry/control frame aligned with
        // the warmth stage. Partial wet blends receive the same protection.
        let warmIndex = (dryIndex + 128) % Self.latencyFrames
        let warmDryL = dryL[warmIndex], warmDryR = dryR[warmIndex], warmMix = mixes[warmIndex]
        lastDryLeft = dryL[dryIndex]; lastDryRight = dryR[dryIndex]
        dryL[dryIndex] = xL; dryR[dryIndex] = xR; mixes[dryIndex] = Float(v[8])
        let original = (Double(xL), Double(xR))
        let low = rumbleFirst.sample(original)
        let high = (original.0 - low.0, original.1 - low.1)
        let lowAgain = rumbleSecond.sample(high)
        let cleaned = (original.0 + (high.0 - lowAgain.0 - original.0) * v[0],
                       original.1 + (high.1 - lowAgain.1 - original.1) * v[0])
        let bodyBand = body.sample(cleaned), mudBand = mud.sample(cleaned), harshBand = harsh.sample(cleaned)
        let lowEss = ess.sample(cleaned), essBand = (cleaned.0 - lowEss.0, cleaned.1 - lowEss.1)
        let presenceBand = presence.sample(cleaned)
        let lowAir = air.sample(cleaned), airBand = (cleaned.0 - lowAir.0, cleaned.1 - lowAir.1)
        Self.follow(&totalPower, Self.power(cleaned), attack: times.ms10, release: times.ms120)
        Self.follow(&voicePower, Self.power(bodyBand) + Self.power(mudBand), attack: times.ms20, release: times.ms160)
        Self.follow(&mudPower, Self.power(mudBand), attack: times.ms25, release: times.ms150)
        Self.follow(&harshPower, Self.power(harshBand), attack: times.ms6, release: times.ms80)
        Self.follow(&essPower, Self.power(essBand), attack: times.ms1_5, release: times.ms55)
        slowPower += times.ms300 * (Self.power(cleaned) - slowPower)
        let tonal = periodicity.process(cleaned)

        // Two seconds of sampled minima estimate the background conservatively.
        // A -52 dB upper bound prevents learning a quiet voice as the noise bed.
        noiseClock += 1
        if noiseClock == 480 {
            noiseClock = 0; noiseBins[noiseIndex] = totalPower
            noiseIndex = (noiseIndex + 1) % noiseBins.count; noiseCount = min(noiseBins.count, noiseCount + 1)
            var minimum = Double.infinity
            for i in 0..<noiseCount { minimum = min(minimum, noiseBins[i]) }
            let target = max(1e-8, min(6.3095734448e-6, minimum))
            let k: Double
            if target > noisePower && tonal > 0.7 { k = 0 }
            else { k = target < noisePower ? 0.15 : 0.025 }
            noisePower += k * (target - noisePower)
        }
        let totalDB = Self.db(totalPower), noiseDB = Self.db(noisePower)
        let signalActivity = Self.smooth((totalDB - noiseDB - 6) / 12) * Self.smooth((totalDB + 64) / 14)
        let bodyHint = Self.smooth((voicePower / max(1e-20, totalPower) - 0.25) / 0.5) * Self.smooth((totalDB + 78) / 16)
        let active = max(signalActivity, bodyHint, tonal * Self.smooth((totalDB + 80) / 18))
        Self.follow(&activity, active, attack: times.ms3, release: times.ms250)
        let quiet = Self.smooth((noiseDB + 9 - totalDB) / 12)
        let cleanupTarget = 14 * v[0] * quiet * (1 - activity)
        Self.follow(&cleanupDB, cleanupTarget, attack: times.ms80, release: times.ms3)

        let mudFraction = mudPower / max(1e-20, totalPower)
        let mudTarget = 4 * v[2] * Self.smooth((mudFraction - 0.6) / 0.35) * Self.smooth((Self.db(mudPower) + 30) / 16)
        Self.follow(&mudDB, mudTarget, attack: times.ms25, release: times.ms150)
        let harshFraction = harshPower / max(1e-20, totalPower)
        let harshTarget = 3 * v[2] * Self.smooth((harshFraction - 0.32) / 0.48) * Self.smooth((Self.db(harshPower) + 36) / 18)
        Self.follow(&harshDB, harshTarget, attack: times.ms8, release: times.ms100)
        let essRatio = Self.db(essPower / max(1e-20, totalPower))
        let essTarget = 9 * v[3] * Self.smooth((essRatio + 11) / 9) * Self.smooth((Self.db(essPower) + 50) / 16)
        Self.follow(&essDB, essTarget, attack: times.ms1_5, release: times.ms55)

        let voice = activity * Self.smooth((voicePower / max(1e-20, totalPower) - 0.04) / 0.2)
        var levelTarget = max(-6, min(4.5, -22 - Self.db(slowPower))) * v[4] * voice
        if levelTarget > 0 {
            levelTarget *= Self.smooth((totalDB - noiseDB - 12) / 12) * Self.smooth((Self.db(slowPower) + 50) / 14)
        }
        let levelCoefficient = abs(levelTarget) < abs(levelDB) ? times.ms70 : times.ms300
        levelDB += levelCoefficient * (levelTarget - levelDB)
        let bodyGain = Self.gain(v[1]) - 1
        let mudCut = 1 - Self.gain(-mudDB), harshCut = 1 - Self.gain(-harshDB), essCut = 1 - Self.gain(-essDB)
        let presenceGain = Self.gain(1.8 * v[2]) - 1
        let airGain = (Self.gain(3 * v[5]) - 1) * (1 - Self.unit(essDB / 9))
        let levelGain = Self.gain(levelDB - cleanupDB)
        var wetL = (cleaned.0 + bodyBand.0 * bodyGain - mudBand.0 * mudCut - harshBand.0 * harshCut
                    - essBand.0 * essCut + presenceBand.0 * presenceGain + airBand.0 * airGain) * levelGain
        var wetR = (cleaned.1 + bodyBand.1 * bodyGain - mudBand.1 * mudCut - harshBand.1 * harshCut
                    - essBand.1 * essCut + presenceBand.1 * presenceGain + airBand.1 * airGain) * levelGain
        let peak = max(abs(wetL), abs(wetR))
        Self.follow(&compressionEnvelope, peak, attack: times.ms2, release: times.ms90)
        let over = 20 * log10(max(1e-10, compressionEnvelope)) - (-18 - 6 * v[4])
        let slope = 1 - 1 / (1 + 3 * v[4])
        let compressionTarget: Double
        if over <= -4.5 { compressionTarget = 0 }
        else if over < 4.5 { let knee = over + 4.5; compressionTarget = slope * (knee * knee) / 18 }
        else { compressionTarget = slope * over }
        Self.follow(&compressionDB, compressionTarget, attack: times.ms2, release: times.ms100)
        let compressionGain = Self.gain(-compressionDB)
        wetL *= compressionGain; wetR *= compressionGain
        reductions[dryIndex] = Float(cleanupDB + compressionDB + essDB)
        dryIndex = (dryIndex + 1) % Self.latencyFrames
        let warmed = warm.process(left: Float(wetL), right: Float(wetR), warmth: Float(v[6]), output: trim)
        let mixed = (warmDryL + (warmed.0 - warmDryL) * warmMix,
                     warmDryR + (warmed.1 - warmDryR) * warmMix)
        let protected = guardDSP.process(mixed)
        reductionDB = mix == 0 ? 0 : max(oldReduction * mix, guardDSP.reductionDB)
        if mix == 0 { return (lastDryLeft, lastDryRight) }
        return protected
    }

    func deepCopy() -> VocalEnhancerDSP {
        let copy = VocalEnhancerDSP()
        copy.controls = controls; copy.trim = trim
        copy.rumbleFirst = rumbleFirst; copy.rumbleSecond = rumbleSecond
        copy.body = body; copy.ess = ess; copy.air = air; copy.mud = mud; copy.harsh = harsh; copy.presence = presence
        copy.totalPower = totalPower; copy.voicePower = voicePower; copy.mudPower = mudPower
        copy.harshPower = harshPower; copy.essPower = essPower; copy.slowPower = slowPower; copy.noisePower = noisePower
        copy.noiseBins = noiseBins.map { $0 }; copy.noiseCount = noiseCount; copy.noiseIndex = noiseIndex; copy.noiseClock = noiseClock
        copy.periodicity = periodicity.deepCopy()
        copy.activity = activity; copy.cleanupDB = cleanupDB; copy.mudDB = mudDB; copy.harshDB = harshDB
        copy.essDB = essDB; copy.levelDB = levelDB; copy.compressionEnvelope = compressionEnvelope; copy.compressionDB = compressionDB
        copy.warm = warm.deepCopy(); copy.guardDSP = guardDSP.deepCopy()
        copy.dryL = dryL.map { $0 }; copy.dryR = dryR.map { $0 }
        copy.mixes = mixes.map { $0 }; copy.reductions = reductions.map { $0 }
        copy.dryIndex = dryIndex; copy.initialized = initialized
        copy.lastDryLeft = lastDryLeft; copy.lastDryRight = lastDryRight; copy.reductionDB = reductionDB
        return copy
    }

    private static func power(_ x: (Double, Double)) -> Double { (x.0 * x.0 + x.1 * x.1) * 0.5 }
    private static func db(_ power: Double) -> Double { 10 * log10(max(1e-20, power)) }
    private static func gain(_ db: Double) -> Double { pow(10, db / 20) }
    private static func unit(_ value: Double) -> Double { max(0, min(1, value)) }
    private static func smooth(_ value: Double) -> Double { let x = unit(value); return x * x * (3 - 2 * x) }
    private static func follow(_ state: inout Double, _ target: Double, attack: Double, release: Double) {
        state += (target > state ? attack : release) * (target - state)
    }
}

/// Register-independent periodic activity estimate on a bounded window. Stereo
/// energies are summed, so opposite-polarity vocals remain protected. This also
/// protects pitched hum; it is deliberately not a voice classifier.
private final class VocalPeriodicity {
    private static let size = 1200
    private var left = [Double](repeating: 0, count: size), right = [Double](repeating: 0, count: size)
    private var scratchL = [Double](repeating: 0, count: size), scratchR = [Double](repeating: 0, count: size)
    private var prefix = [Double](repeating: 0, count: size + 1)
    private var index = 0, count = 0, clock = 0
    private var confidence: Double = 0
    var storageBytes: Int { (Self.size * 4 + Self.size + 1) * 8 + 64 }

    func process(_ input: (Double, Double)) -> Double {
        left[index] = input.0; right[index] = input.1
        index = (index + 1) % Self.size; count = min(Self.size, count + 1); clock += 1
        if clock == 480 {
            clock = 0
            if count == Self.size { analyze() }
        }
        return confidence
    }

    private func analyze() {
        prefix[0] = 0
        for i in 0..<Self.size {
            let cursor = (index + i) % Self.size
            scratchL[i] = left[cursor]; scratchR[i] = right[cursor]
            prefix[i + 1] = prefix[i] + scratchL[i] * scratchL[i] + scratchR[i] * scratchR[i]
        }
        let energy = prefix[1200] - prefix[688]
        if energy < 1e-16 { confidence = 0; return }
        var best: Double = 0, bestLag = 40
        for lag in stride(from: 40, through: 684, by: 4) {
            let value = correlation(lag: lag, energy: energy)
            if value > best { best = value; bestLag = lag }
        }
        for lag in max(40, bestLag - 4)...min(685, bestLag + 4) {
            best = max(best, correlation(lag: lag, energy: energy))
        }
        let x = max(0, min(1, (best - 0.65) / 0.25))
        confidence = x * x * (3 - 2 * x)
    }

    private func correlation(lag: Int, energy: Double) -> Double {
        let shifted = prefix[1200 - lag] - prefix[688 - lag]
        if shifted < 1e-16 { return 0 }
        var dot: Double = 0
        for i in 688..<1200 {
            dot += scratchL[i] * scratchL[i - lag] + scratchR[i] * scratchR[i - lag]
        }
        return max(-1, min(1, dot / sqrt(energy * shifted)))
    }

    func deepCopy() -> VocalPeriodicity {
        let copy = VocalPeriodicity()
        copy.left = left.map { $0 }; copy.right = right.map { $0 }
        copy.scratchL = scratchL.map { $0 }; copy.scratchR = scratchR.map { $0 }; copy.prefix = prefix.map { $0 }
        copy.index = index; copy.count = count; copy.clock = clock; copy.confidence = confidence
        return copy
    }
}

private struct VocalTimes {
    let ms1_5 = Self.coefficient(0.0015), ms2 = Self.coefficient(0.002), ms3 = Self.coefficient(0.003)
    let ms6 = Self.coefficient(0.006), ms8 = Self.coefficient(0.008), ms10 = Self.coefficient(0.01)
    let ms20 = Self.coefficient(0.02), ms25 = Self.coefficient(0.025), ms55 = Self.coefficient(0.055)
    let ms70 = Self.coefficient(0.07), ms80 = Self.coefficient(0.08), ms90 = Self.coefficient(0.09)
    let ms100 = Self.coefficient(0.1), ms120 = Self.coefficient(0.12), ms150 = Self.coefficient(0.15)
    let ms160 = Self.coefficient(0.16), ms250 = Self.coefficient(0.25), ms300 = Self.coefficient(0.3)
    private static func coefficient(_ seconds: Double) -> Double { 1 - exp(-1 / (seconds * sessionRate)) }
}

private struct VocalLowPass {
    let b: Double, a: Double
    private var zL: Double = 0, zR: Double = 0
    init(hz: Double) {
        let g = tan(Double.pi * hz / sessionRate)
        b = g / (1 + g); a = (g - 1) / (g + 1)
    }
    mutating func sample(_ input: (Double, Double)) -> (Double, Double) {
        let left = b * input.0 + zL, right = b * input.1 + zR
        zL = b * input.0 - a * left; zR = b * input.1 - a * right
        if abs(zL) < 1e-30 { zL = 0 }; if abs(zR) < 1e-30 { zR = 0 }
        return (left, right)
    }
}

private struct VocalBandPass {
    let b: Double, a1: Double, a2: Double
    private var z1L: Double = 0, z2L: Double = 0, z1R: Double = 0, z2R: Double = 0
    init(hz: Double, q: Double) {
        let omega = 2 * Double.pi * hz / sessionRate, alpha = sin(omega) / (2 * q)
        let denominator = 1 + alpha
        b = alpha / denominator; a1 = -2 * cos(omega) / denominator; a2 = (1 - alpha) / denominator
    }
    mutating func sample(_ input: (Double, Double)) -> (Double, Double) {
        let left = b * input.0 + z1L, right = b * input.1 + z1R
        z1L = -a1 * left + z2L; z1R = -a1 * right + z2R
        z2L = -b * input.0 - a2 * left; z2R = -b * input.1 - a2 * right
        if abs(z1L) < 1e-30 { z1L = 0 }; if abs(z1R) < 1e-30 { z1R = 0 }
        if abs(z2L) < 1e-30 { z2L = 0 }; if abs(z2R) < 1e-30 { z2R = 0 }
        return (left, right)
    }
}

/// Reconstruction-aware linked peak catch without automatic makeup gain.
private final class VocalPeakGuard {
    private static let lookahead = 128, capacity = 144
    private static let ceiling: Float = 0.94406086
    private var left = [Float](repeating: 0, count: lookahead), right = [Float](repeating: 0, count: lookahead)
    private var values = [Float](repeating: 0, count: capacity), frames = [Int64](repeating: 0, count: capacity)
    private var head = 0, tail = 0, cursor = 0
    private var frame: Int64 = 0
    private var detector = TruePeakDSP(), gain: Float = 1
    private let release = Float(1 - exp(-1 / (0.08 * sessionRate)))
    private(set) var reductionDB: Float = 0
    var storageBytes: Int { Self.lookahead * 2 * 4 + Self.capacity * 12 + 256 }

    func process(_ x: (Float, Float)) -> (Float, Float) {
        let delayedLeft = left[cursor], delayedRight = right[cursor]
        left[cursor] = x.0; right[cursor] = x.1; cursor = (cursor + 1) % Self.lookahead
        let peak = detector.process(x.0, x.1)
        while head != tail {
            let previous = (tail + Self.capacity - 1) % Self.capacity
            if values[previous] > peak { break }
            tail = previous
        }
        values[tail] = peak; frames[tail] = frame; tail = (tail + 1) % Self.capacity
        while head != tail && frames[head] < frame - 140 { head = (head + 1) % Self.capacity }
        let target = min(1, Self.ceiling / max(1e-20, values[head]))
        let coefficient: Float = target < gain ? 0.08 : release
        gain += coefficient * (target - gain)
        let applied = min(gain, Self.ceiling / max(1e-20, abs(delayedLeft), abs(delayedRight)))
        reductionDB = max(0, -20 * log10(max(1e-20, applied)))
        frame += 1
        return (delayedLeft * applied, delayedRight * applied)
    }

    func deepCopy() -> VocalPeakGuard {
        let copy = VocalPeakGuard()
        copy.left = left.map { $0 }; copy.right = right.map { $0 }
        copy.values = values.map { $0 }; copy.frames = frames.map { $0 }
        copy.head = head; copy.tail = tail; copy.cursor = cursor; copy.frame = frame
        copy.detector = detector.deepCopy(); copy.gain = gain; copy.reductionDB = reductionDB
        return copy
    }
}

/// Original four-times oversampled gentle vocal saturation. Both paths retain
/// the same sixteen-frame delay, including their source-time control values.
private final class VocalWarmth {
    static let latencyFrames = 16
    private static let taps: [Float] = {
        let values = (0..<65).map { i -> Double in
            let distance = Double(i - 32), cutoff = 0.105
            let sinc = distance == 0 ? 2 * cutoff : sin(2 * .pi * cutoff * distance) / (.pi * distance)
            let phase = 2 * Double.pi * Double(i) / 64
            return sinc * (0.42 - 0.5 * cos(phase) + 0.08 * cos(2 * phase))
        }
        let sum = values.reduce(0, +)
        return values.map { Float($0 / sum) }
    }()
    private var inputL = [Float](repeating: 0, count: 17), inputR = [Float](repeating: 0, count: 17)
    private var highL = [Float](repeating: 0, count: 65), highR = [Float](repeating: 0, count: 65)
    private var dryL = [Float](repeating: 0, count: latencyFrames), dryR = [Float](repeating: 0, count: latencyFrames)
    private var blends = [Float](repeating: 0, count: latencyFrames), outputs = [Float](repeating: 1, count: latencyFrames)
    private var inputIndex = 0, highIndex = 0, dryIndex = 0
    private var initialized = false
    var storageBytes: Int { (17 * 2 + 65 * 2 + Self.latencyFrames * 4) * 4 + 64 }
    init() { _ = Self.taps }

    func process(left: Float, right: Float, warmth: Float, output: Float) -> (Float, Float) {
        if !initialized {
            for i in blends.indices { blends[i] = warmth * 0.35; outputs[i] = output }
            initialized = true
        }
        let delayedLeft = dryL[dryIndex], delayedRight = dryR[dryIndex]
        let delayedBlend = blends[dryIndex], delayedOutput = outputs[dryIndex]
        dryL[dryIndex] = left; dryR[dryIndex] = right
        blends[dryIndex] = warmth * 0.35; outputs[dryIndex] = output
        dryIndex = (dryIndex + 1) % Self.latencyFrames
        inputL[inputIndex] = left; inputR[inputIndex] = right
        var downL: Float = 0, downR: Float = 0
        for phase in 0..<4 {
            var upL: Float = 0, upR: Float = 0
            var tap = phase, history = inputIndex
            while tap < Self.taps.count {
                let coefficient = Self.taps[tap] * 4
                upL += coefficient * inputL[history]; upR += coefficient * inputR[history]
                tap += 4; history = (history + 16) % 17
            }
            highL[highIndex] = tanh(upL * 1.8) / 1.8
            highR[highIndex] = tanh(upR * 1.8) / 1.8
            if phase == 0 {
                var history = highIndex
                for coefficient in Self.taps {
                    downL += coefficient * highL[history]; downR += coefficient * highR[history]
                    history = (history + 64) % 65
                }
            }
            highIndex = (highIndex + 1) % 65
        }
        inputIndex = (inputIndex + 1) % 17
        return ((delayedLeft + (downL - delayedLeft) * delayedBlend) * delayedOutput,
                (delayedRight + (downR - delayedRight) * delayedBlend) * delayedOutput)
    }

    func deepCopy() -> VocalWarmth {
        let copy = VocalWarmth()
        copy.inputL = inputL.map { $0 }; copy.inputR = inputR.map { $0 }
        copy.highL = highL.map { $0 }; copy.highR = highR.map { $0 }
        copy.dryL = dryL.map { $0 }; copy.dryR = dryR.map { $0 }
        copy.blends = blends.map { $0 }; copy.outputs = outputs.map { $0 }
        copy.inputIndex = inputIndex; copy.highIndex = highIndex; copy.dryIndex = dryIndex
        copy.initialized = initialized
        return copy
    }
}
