import Foundation

/// Fixed-value control block; callers can construct this on the audio thread without allocation.
struct PitchCorrectionParameters {
    var key: Int = 0
    var scale: Int = 0 // chromatic, major, natural minor, absolute custom pitch classes
    var mask: Int = 4095
    var reference: Float = 440
    var retune: Float = 40
    var strength: Float = 1
    var humanize: Float = 0.35
    var tolerance: Float = 5
    var vibrato: Float = 0.5
    var formant: Float = 0
    var mix: Float = 1
    var manualTarget: Float = -1 // automatic, otherwise an absolute fractional MIDI note including octave
}

/// Monophonic YIN analysis with stereo-linked pitch-synchronous overlap-add synthesis.
/// Algorithm references: de Cheveigné & Kawahara, JASA 2002 (PMID 12002874),
/// https://www.dafx.de/paper-archive/2011/Papers/52_e.pdf (sections 3.1 and 4.1),
/// https://praat.org/manual/overlap-add.html . Designed for individual voices/instruments,
/// not chords, percussion or separated polyphonic pitch correction.
final class PitchCorrectionState {
    static let latencyFrames = 4096
    private static let capacity = 16384
    private static let ringMask = capacity - 1
    private var inputL = [Float](repeating: 0, count: capacity)
    private var inputR = [Float](repeating: 0, count: capacity)
    private var pulseSignal = [Float](repeating: 0, count: capacity)
    private var frequencies = [Float](repeating: 0, count: capacity)
    private var confidences = [Float](repeating: 0, count: capacity)
    private var outputL = [Float](repeating: 0, count: capacity)
    private var outputR = [Float](repeating: 0, count: capacity)
    private var normalizationWeights = [Float](repeating: 0, count: capacity)
    private var targetSum = [Float](repeating: 0, count: capacity)
    private var correctionSum = [Float](repeating: 0, count: capacity)
    private var activity = [Float](repeating: 0, count: capacity)
    private var weights = [Float](repeating: 0, count: capacity)
    private var controls = [PitchCorrectionParameters](repeating: PitchCorrectionParameters(), count: capacity)
    private var decimated = [Float](repeating: 0, count: 512)
    private var difference = [Float](repeating: 0, count: 242)
    private var refinedDifference = [Float](repeating: 0, count: 7)
    private var hann = (0...2048).map { Float(0.5 + 0.5 * cos(Double.pi * Double($0) / 2048)) }
    private var frame = 0
    private var decimatedCount = 0
    private var lowpass1: Float = 0, lowpass2: Float = 0
    private var energyL: Float = 0, energyR: Float = 0
    private var usesRightChannel = false
    private var nextEpoch: Double = 0
    private var lastPulse: Double?
    private var lastPulsePeriod: Float = 0
    private var correction: Float = 0, centerPitch: Float = 0
    private var previousTarget: Float = -.infinity, sustainedFrames: Float = 0
    private var dcInputL: Float = 0, dcInputR: Float = 0, dcOutputL: Float = 0, dcOutputR: Float = 0
    private var voicedBlend: Float = 0
    private var lastGrainCorrection: Float = 0
    private var lastGrainFormant: Float = 0
    private var synthesisConfidence: Float = 0
    private var renderedManualCorrection: Float = 0
    private(set) var meterSnapshot = PitchMeterSnapshot()
    private(set) var lastDryLeft: Float = 0, lastDryRight: Float = 0
    private(set) var detectedFrequency: Float = 0, confidence: Float = 0, targetFrequency: Float = 0

    var storageBytes: Int { (Self.capacity * 12 + 512 + 242 + 7 + 2049) * MemoryLayout<Float>.stride + Self.capacity * MemoryLayout<PitchCorrectionParameters>.stride }
    func deepCopy() -> PitchCorrectionState {
        let result = PitchCorrectionState()
        // Force independent storage now, never a copy-on-write allocation in process().
        result.inputL = inputL.map { $0 }; result.inputR = inputR.map { $0 }
        result.pulseSignal = pulseSignal.map { $0 }; result.frequencies = frequencies.map { $0 }
        result.confidences = confidences.map { $0 }; result.outputL = outputL.map { $0 }
        result.outputR = outputR.map { $0 }; result.weights = weights.map { $0 }; result.activity = activity.map { $0 }
        result.normalizationWeights = normalizationWeights.map { $0 }; result.targetSum = targetSum.map { $0 }; result.correctionSum = correctionSum.map { $0 }
        result.lastPulse = lastPulse; result.lastPulsePeriod = lastPulsePeriod; result.meterSnapshot = meterSnapshot
        result.controls = controls.map { $0 }; result.decimated = decimated.map { $0 }; result.difference = difference.map { $0 }; result.refinedDifference = refinedDifference.map { $0 }
        result.frame = frame; result.decimatedCount = decimatedCount
        result.lowpass1 = lowpass1; result.lowpass2 = lowpass2
        result.usesRightChannel = usesRightChannel
        result.energyL = energyL; result.energyR = energyR; result.nextEpoch = nextEpoch
        result.correction = correction; result.centerPitch = centerPitch
        result.previousTarget = previousTarget; result.sustainedFrames = sustainedFrames
        result.dcInputL = dcInputL; result.dcInputR = dcInputR; result.dcOutputL = dcOutputL; result.dcOutputR = dcOutputR
        result.voicedBlend = voicedBlend; result.lastGrainCorrection = lastGrainCorrection
        result.lastGrainFormant = lastGrainFormant; result.synthesisConfidence = synthesisConfidence
        result.renderedManualCorrection = renderedManualCorrection
        result.lastDryLeft = lastDryLeft; result.lastDryRight = lastDryRight
        result.detectedFrequency = detectedFrequency; result.confidence = confidence
        result.targetFrequency = targetFrequency
        return result
    }

    func process(left: Float, right: Float, parameters p: PitchCorrectionParameters) -> (Float, Float) {
        let l = left.isFinite ? left : 0, r = right.isFinite ? right : 0
        let index = frame & Self.ringMask
        inputL[index] = l; inputR[index] = r; controls[index] = p
        energyL += 0.001 * (l * l - energyL); energyR += 0.001 * (r * r - energyR)
        // Hysteresis keeps equal-level stereo phase offsets from switching analysis
        // channels every half-cycle; summing would cancel antiphase voices.
        if energyR > 1.5 * energyL + 0.000000001 { usesRightChannel = true }
        else if energyL > 1.5 * energyR + 0.000000001 { usesRightChannel = false }
        let mono = usesRightChannel ? r : l
        lowpass1 += 0.23 * (mono - lowpass1); lowpass2 += 0.23 * (lowpass1 - lowpass2)
        pulseSignal[index] = lowpass2
        if frame & 3 == 0 {
            decimated[decimatedCount & 511] = lowpass2; decimatedCount += 1
            if decimatedCount >= 512 && decimatedCount & 63 == 0 { analyze() }
        }
        frequencies[index] = detectedFrequency; confidences[index] = confidence
        // Schedule sufficiently early that even a low voice with -12 semitone formants
        // has its whole synthesis window available before the first output sample.
        while nextEpoch <= Double(frame + 2048) { synthesize() }
        let source = frame - Self.latencyFrames
        lastDryLeft = sample(inputL, source); lastDryRight = sample(inputR, source)
        let weight = weights[index]
        let delayedControls = source >= 0 ? controls[source & Self.ringMask] : PitchCorrectionParameters()
        // At natural formants, normalize actual coverage. Expected steady overlap
        // is insufficient at vowel transitions and after a rejected/unvoiced grain.
        // Explicit formant shifts retain their pulse envelope and expected gain.
        let coverage = normalizationWeights[index]
        let naturalFormants = abs(delayedControls.formant) < 0.001
        // At a manual downward octave, grains nearly stop overlapping. Dividing
        // out their Hann envelopes recreates the original carrier and loses the
        // selected fundamental. Gradually retain expected overlap normalization
        // beyond a downward half octave; automatic keeps its exact old path.
        let decision = source + 1024
        let outputFrequency = source >= 0 ? sample(frequencies, decision) : 0
        let outputConfidence = source >= 0 ? sample(confidences, decision) : 0
        let voiced = outputFrequency >= 50 && outputFrequency <= 1000 && outputConfidence > 0.75
        let manualEpochWeight: Float
        if delayedControls.manualTarget >= 0 && (weight > 0.00001 || voiced) {
            // A downward octave intentionally leaves Hann-envelope troughs.
            // Missing coverage there is not a switch back to automatic mode:
            // reinserting the dry carrier creates an isolated sample spike.
            // Clear this fallback once both voicing and coverage end so silence
            // and unvoiced audio keep their exact dry path.
            if weight > 0.00001 { renderedManualCorrection = correctionSum[index] / weight }
            manualEpochWeight = max(0, min(1, (-renderedManualCorrection - 600) / 600))
        } else {
            renderedManualCorrection = 0
            manualEpochWeight = 0
        }
        let divisor = naturalFormants && coverage > 0.00001
            ? (manualEpochWeight > 0 ? coverage * (1 - manualEpochWeight) + manualEpochWeight : coverage) : 1
        let unfilteredL = outputL[index] / divisor
        let unfilteredR = outputR[index] / divisor
        // Formant scaling may introduce a DC offset in pulse-centred grains. Remove it
        // from the synthesized branch only; unvoiced/tolerated dry remains exact.
        dcOutputL = unfilteredL - dcInputL + 0.998692 * dcOutputL
        dcOutputR = unfilteredR - dcInputR + 0.998692 * dcOutputR
        dcInputL = unfilteredL; dcInputR = unfilteredR
        let wetL = dcOutputL, wetR = dcOutputR
        let active = voiced && weight > 0.00001 && activity[index] / weight > 0.01
        // Release quickly through consonants, without a discontinuity or a 10 ms
        // dropout caused by continuing to crossfade into an empty synthesis ring.
        voicedBlend += (active ? 1 - voicedBlend : -voicedBlend) * (active ? 1 / 480 : 1 / 96)
        let coveredBlend = naturalFormants ? min(1, max(0, coverage * 4)) : 1
        // Filling each grain's envelope trough with the dry carrier would restore
        // the octave alias. Unvoiced/uncertain audio still uses the existing dry path.
        let availableBlend = manualEpochWeight > 0 ? coveredBlend * (1 - manualEpochWeight) + manualEpochWeight : coveredBlend
        let blend = voicedBlend * availableBlend * finiteClamp(delayedControls.mix, 0, 1, fallback: 1)
        meterSnapshot.sourceFrame = source
        meterSnapshot.referenceFrequency = finiteClamp(delayedControls.reference, 420, 460, fallback: 440)
        meterSnapshot.inputFrequency = voiced ? outputFrequency : 0
        meterSnapshot.confidence = outputConfidence
        meterSnapshot.targetFrequency = voiced && weight > 0.00001 ? targetSum[index] / weight : 0
        meterSnapshot.correctionCents = voiced && weight > 0.00001 ? correctionSum[index] / weight : 0
        meterSnapshot.wetBlend = blend
        outputL[index] = 0; outputR[index] = 0; weights[index] = 0; activity[index] = 0
        normalizationWeights[index] = 0; targetSum[index] = 0; correctionSum[index] = 0
        frame += 1
        return (lastDryLeft + (wetL - lastDryLeft) * blend, lastDryRight + (wetR - lastDryRight) * blend)
    }
    private func sample(_ ring: [Float], _ position: Int) -> Float {
        position >= 0 && position <= frame && frame - position < Self.capacity ? ring[position & Self.ringMask] : 0
    }
    private func interpolated(_ ring: [Float], _ position: Double) -> Float {
        let first = Int(floor(position)), fraction = Float(position - Double(first))
        return sample(ring, first) * (1 - fraction) + sample(ring, first + 1) * fraction
    }
    private func analyze() {
        let start = decimatedCount - 512
        var energy: Float = 0
        for j in 0..<256 { let value = decimated[(start + j) & 511]; energy += value * value }
        guard energy / 256 > 0.000003 else { confidence = 0; detectedFrequency = 0; return }
        difference[0] = 1
        var cumulative: Float = 0
        for lag in 1...241 {
            var sum: Float = 0
            for j in 0..<256 {
                let centeredStart = start + 128 - lag / 2
                let d = decimated[(centeredStart + j) & 511] - decimated[(centeredStart + j + lag) & 511]
                sum += d * d
            }
            cumulative += sum
            difference[lag] = cumulative > 0 ? sum * Float(lag) / cumulative : 1
        }
        var lag = 12
        while lag < 240 {
            if difference[lag] < 0.15 {
                while lag < 240 && difference[lag + 1] < difference[lag] { lag += 1 }
                break
            }
            lag += 1
        }
        guard lag <= 240, difference[lag] < 0.25 else { confidence = 0; detectedFrequency = 0; return }
        let a = difference[lag - 1], b = difference[lag], c = difference[lag + 1]
        let denominator = a - 2 * b + c
        let adjustment = abs(denominator) > 0.000001 ? max(-0.5, min(0.5, 0.5 * (a - c) / denominator)) : 0
        let approximatePeriod = 4 * (Float(lag) + adjustment)
        let base = Int(approximatePeriod.rounded())
        // Decimated interpolation has a measurable high-register lag bias. A bounded
        // full-rate refinement recovers fractional periods without a large search.
        for index in 0..<7 {
            let candidate = base + index - 3
            let windowStart = frame - 1024 - 512 - candidate / 2
            // Accepted YIN lags 12...240 yield candidates 43...965. The first
            // decision is frame 2044: its earliest read is 26, its latest is
            // frame - 30, and the oldest retained age is 2018 < capacity.
            // Keep bounds assertions outside the hot loop so changing analysis
            // limits cannot silently invalidate these direct ring reads.
            assert(candidate >= 43 && candidate <= 965)
            assert(windowStart >= 0 && windowStart + 1023 + candidate <= frame)
            assert(frame - windowStart < Self.capacity)
            var sum: Float = 0
            for j in 0..<1024 {
                let d = pulseSignal[(windowStart + j) & Self.ringMask] - pulseSignal[(windowStart + j + candidate) & Self.ringMask]
                sum += d * d
            }
            refinedDifference[index] = sum
        }
        var best = 1
        for index in 2...5 where refinedDifference[index] < refinedDifference[best] { best = index }
        let ra = refinedDifference[best - 1], rb = refinedDifference[best], rc = refinedDifference[best + 1]
        let rd = ra - 2 * rb + rc
        let fraction = abs(rd) > 0.000001 ? max(-0.5, min(0.5, 0.5 * (ra - rc) / rd)) : 0
        detectedFrequency = Float(sessionRate) / (Float(base + best - 3) + fraction)
        confidence = 1 - b
    }
    static func nearestNote(midi: Float, key: Int, scale: Int, mask: Int) -> Int {
        let root = (key % 12 + 12) % 12
        let scaleMask: Int
        switch scale {
        case 1: scaleMask = 0b101010110101 // C major: C D E F G A B
        case 2: scaleMask = 0b010110101101 // C natural minor: C D Eb F G Ab Bb
        case 3: scaleMask = mask & 4095
        default: return Int(midi.rounded())
        }
        guard scaleMask != 0 else { return Int(midi.rounded()) }
        let middle = Int(midi.rounded())
        var nearest = middle, distance = Float.infinity
        for note in (middle - 12)...(middle + 12) {
            let pitchClass = (note % 12 + 12) % 12
            let bit = scale == 3 ? pitchClass : (pitchClass - root + 12) % 12
            let d = abs(Float(note) - midi)
            if scaleMask & (1 << bit) != 0 && d < distance { nearest = note; distance = d }
        }
        return nearest
    }
    static func clampedManualTarget(_ target: Int) -> Int? {
        target < 0 ? nil : min(95, target)
    }
    static func clampedManualTarget(_ target: Float) -> Float? {
        target.isFinite && target >= 0 ? min(95, target) : nil
    }
    private func synthesize() {
        let outputCenter = nextEpoch, sourceCenter = outputCenter - Double(Self.latencyFrames)
        // Analysis describes a window centred ~1024 samples before its decision.
        let sourceFrame = Int(sourceCenter)
        let p = sourceFrame >= 0 && sourceFrame <= frame ? controls[sourceFrame & Self.ringMask] : PitchCorrectionParameters()
        let decision = sourceFrame + 1024
        let f0 = sample(frequencies, decision), certainty = sample(confidences, decision)
        guard f0 >= 50 && f0 <= 1000 && certainty > 0.75 else {
            synthesisConfidence = 0; targetFrequency = 0
            // A new voiced phrase must not inherit the previous vowel's epoch,
            // target or retune lag. The wet output already queued fades normally.
            lastPulse = nil; lastPulsePeriod = 0; centerPitch = 0
            correction = 0; previousTarget = -.infinity; sustainedFrames = 0
            nextEpoch += 128; return
        }
        let period = Float(sessionRate) / f0
        let reference = finiteClamp(p.reference, 420, 460, fallback: 440)
        let midi = 69 + 12 * log2(f0 / reference)
        let interval = max(1, period)
        if centerPitch == 0 || abs(midi - centerPitch) > 3 { centerPitch = midi }
        centerPitch += (1 - exp(-interval / (Float(sessionRate) * 0.12))) * (midi - centerPitch)
        let vibrato = finiteClamp(p.vibrato, 0, 1, fallback: 0.5)
        let targetingPitch = midi * (1 - vibrato) + centerPitch * vibrato
        let manualTarget = Self.clampedManualTarget(p.manualTarget)
        var target = manualTarget ?? Float(Self.nearestNote(midi: targetingPitch, key: p.key, scale: p.scale, mask: p.mask))
        // Six cents of boundary hysteresis avoids a repeated semitone jump when
        // a breathy vowel's estimate straddles the nearest-note decision boundary.
        if manualTarget == nil && previousTarget.isFinite && target != previousTarget &&
            Float(Self.nearestNote(midi: previousTarget, key: p.key, scale: p.scale, mask: p.mask)) == previousTarget &&
            abs(targetingPitch - previousTarget) <= abs(targetingPitch - target) + 0.12 {
            target = previousTarget
        }
        if target == previousTarget { sustainedFrames += interval } else { sustainedFrames = 0; previousTarget = target }
        targetFrequency = reference * pow(2, (target - 69) / 12)
        var desired = (target - midi + vibrato * (midi - centerPitch)) * 100
        if manualTarget == nil && p.scale == 3 && p.mask & 4095 == 0 { desired = 0; targetFrequency = f0 }
        // Absolute note selection retains the existing one-octave synthesis limit.
        // Clamp before retune interpolation so distant manual requests cannot build
        // an oversized correction state or report a correction beyond this range.
        if manualTarget != nil { desired = max(-1200, min(1200, desired)) }
        if abs(desired) <= finiteClamp(p.tolerance, 0, 100, fallback: 5) { desired = 0 }
        desired *= finiteClamp(p.strength, 0, 1, fallback: 1)
        let human = finiteClamp(p.humanize, 0, 1, fallback: 0.35)
        let time = finiteClamp(p.retune, 0, 500, fallback: 40) + human * 250 * min(1, sustainedFrames / Float(sessionRate))
        let alpha: Float = time <= 0 ? 1 : 1 - exp(-interval / (time * 0.001 * Float(sessionRate)))
        correction += alpha * (desired - correction)
        let ratio = max(0.5, min(2, pow(2, correction / 1200)))
        let formant = finiteClamp(p.formant, -12, 12, fallback: 0)
        let formantRatio = pow(2, formant / 12)
        let radius = min(1920, max(24, period / formantRatio))
        // Link both channels to the same strongest positive pulse near the ideal source epoch.
        let tracked: Bool
        let pulseCenter: Double
        if let previous = lastPulse, abs(period - lastPulsePeriod) < period * 0.2 {
            pulseCenter = previous + ((sourceCenter - previous) / Double(period)).rounded() * Double(period)
            tracked = true
        } else {
            pulseCenter = sourceCenter; tracked = false
        }
        let middle = Int(pulseCenter.rounded()), search = max(1, Int(period * (tracked ? 0.2 : 0.5)))
        var pulse = middle, peak = -Float.infinity
        for position in (middle - search)...(middle + search) {
            let value = sample(pulseSignal, position)
            if value > peak { peak = value; pulse = position }
        }
        lastPulse = Double(pulse); lastPulsePeriod = period
        // Formant-shifted grains retain expected overlap gain: dividing by each
        // sample's coverage cancels their compressed pulse envelope and shifts
        // the fundamental along with the formants. Natural-formant grains are
        // normalized against actual coverage when read from the output ring.
        let normalization = max(0.125, radius / (period / ratio))
        let first = Int(ceil(outputCenter - Double(radius))), last = Int(floor(outputCenter + Double(radius)))
        if first <= last {
            for position in first...last where position >= frame {
                let displacement = Float(Double(position) - outputCenter)
                let windowIndex = min(2048, Int(abs(displacement) / radius * 2048))
                let window = hann[windowIndex]
                let sourcePosition = Double(pulse) + Double(displacement * formantRatio)
                let index = position & Self.ringMask
                outputL[index] += interpolated(inputL, sourcePosition) * window / normalization
                outputR[index] += interpolated(inputR, sourcePosition) * window / normalization
                weights[index] += window
                normalizationWeights[index] += window / normalization
                targetSum[index] += targetFrequency * window
                correctionSum[index] += correction * window
                if abs(correction) > 0.01 || abs(formant) > 0.001 { activity[index] += window }
            }
        }
        synthesisConfidence = certainty; lastGrainCorrection = correction; lastGrainFormant = formant
        nextEpoch += Double(period / ratio)
    }
    private func finiteClamp(_ value: Float, _ low: Float, _ high: Float, fallback: Float) -> Float {
        value.isFinite ? max(low, min(high, value)) : fallback
    }
}
