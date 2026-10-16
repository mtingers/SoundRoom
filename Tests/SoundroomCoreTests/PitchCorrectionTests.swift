import XCTest
@testable import SoundroomCore

final class PitchCorrectionTests: XCTestCase {
    private func voice(frequency: Float, count: Int = 96_000) -> [Float] {
        (0..<count).map { i in
            let phase = 2 * Float.pi * frequency * Float(i) / 48_000
            return 0.32 * sin(phase) + 0.12 * sin(phase * 2 + 0.4) + 0.06 * sin(phase * 3 + 0.8)
        }
    }
    private func render(_ audio: [Float], parameters: PitchCorrectionParameters, rightGain: Float = 1) -> (left: [Float], right: [Float], state: PitchCorrectionState) {
        let state = PitchCorrectionState()
        var left = [Float](), right = [Float]()
        for value in audio { let pair = state.process(left: value, right: value * rightGain, parameters: parameters); left.append(pair.0); right.append(pair.1) }
        return (left, right, state)
    }
    /// Independent frequency estimate from the rendered audio, not the processor's detector.
    private func frequency(_ audio: [Float], near expected: Float, start: Int = 48_000) -> Float {
        let firstLag = Int(48_000 / (expected * 1.08)), lastLag = Int(48_000 / (expected * 0.92))
        var correlations = [Float](repeating: 0, count: lastLag + 2)
        for lag in (firstLag - 1)...(lastLag + 1) {
            var sum: Float = 0
            for i in start..<(start + 12_000) { sum += audio[i] * audio[i + lag] }
            correlations[lag] = sum
        }
        let lag = (firstLag...lastLag).max(by: { correlations[$0] < correlations[$1] })!
        let a = correlations[lag - 1], b = correlations[lag], c = correlations[lag + 1]
        let denominator = a - 2 * b + c
        let adjustment = abs(denominator) > 0.000001 ? 0.5 * (a - c) / denominator : 0
        return 48_000 / (Float(lag) + adjustment)
    }
    func testCorrectsRealHarmonicSignalWithStereoLinkAndFixedLatency() {
        var p = PitchCorrectionParameters(); p.retune = 0; p.humanize = 0; p.vibrato = 0; p.tolerance = 0
        let input = voice(frequency: 250), result = render(input, parameters: p, rightGain: -0.7)
        XCTAssertEqual(frequency(result.left, near: 246.94165), 246.94165, accuracy: 0.5)
        XCTAssertEqual(result.state.detectedFrequency, 250, accuracy: 0.6)
        XCTAssertGreaterThan(result.state.confidence, 0.95)
        XCTAssertTrue(result.left.prefix(PitchCorrectionState.latencyFrames).allSatisfy { $0 == 0 })
        for i in 12_000..<result.left.count { XCTAssertEqual(result.right[i], result.left[i] * -0.7, accuracy: 0.00001) }
        XCTAssertTrue(result.left.allSatisfy { $0.isFinite && abs($0) < 1 })
    }
    func testCustomMaskKeyScaleAndReferenceActuallyChangePitch() {
        XCTAssertEqual(PitchCorrectionState.nearestNote(midi: 61.1, key: 0, scale: 1, mask: 4095), 62)
        XCTAssertEqual(PitchCorrectionState.nearestNote(midi: 61.1, key: 2, scale: 1, mask: 4095), 61)
        XCTAssertEqual(PitchCorrectionState.nearestNote(midi: 63.1, key: 0, scale: 2, mask: 4095), 63)
        XCTAssertEqual(PitchCorrectionState.nearestNote(midi: 70, key: 0, scale: 3, mask: 0), 70)
        var p = PitchCorrectionParameters(); p.retune = 0; p.humanize = 0; p.vibrato = 0; p.tolerance = 0; p.scale = 3; p.mask = 1 // C only
        let custom = render(voice(frequency: 250), parameters: p)
        XCTAssertEqual(frequency(custom.left, near: 261.6256), 261.6256, accuracy: 0.7)
        p.scale = 0; p.reference = 442
        let reference = render(voice(frequency: 450), parameters: p)
        XCTAssertEqual(frequency(reference.left, near: 442), 442, accuracy: 0.7)
    }
    func testNoiseSilenceAndToleranceUseLatencyAlignedDry() {
        var seed: UInt64 = 173
        let noise = (0..<24_000).map { _ -> Float in seed = seed &* 6364136223846793005 &+ 1; return Float(Int64(bitPattern: seed) >> 40) / Float(1 << 23) * 0.2 }
        let result = render(noise, parameters: PitchCorrectionParameters())
        XCTAssertEqual(result.state.confidence, 0)
        for i in 4096..<noise.count { XCTAssertEqual(result.left[i], noise[i - 4096]) }
        let silence = render([Float](repeating: 0, count: 6000), parameters: PitchCorrectionParameters())
        XCTAssertTrue(silence.left.allSatisfy { $0 == 0 })
        var p = PitchCorrectionParameters(); p.tolerance = 100
        let input = voice(frequency: 445), tolerated = render(input, parameters: p)
        for i in 4096..<input.count { XCTAssertEqual(tolerated.left[i], input[i - 4096]) }
    }
    func testMixAutomationUsesExactSourceFrameAndZeroStrengthBypasses() {
        let input = voice(frequency: 450, count: 32_000), state = PitchCorrectionState()
        var p = PitchCorrectionParameters(); p.retune = 0; p.humanize = 0; p.vibrato = 0; p.tolerance = 0
        var output = [Float]()
        for i in input.indices { p.mix = i >= 12_000 ? 0 : 1; output.append(state.process(left: input[i], right: input[i], parameters: p).0) }
        XCTAssertGreaterThan(abs(output[16_095] - input[11_999]), 0.001)
        for i in 16_096..<input.count { XCTAssertEqual(output[i], input[i - 4096]) }
        p.strength = 0; p.mix = 1
        let bypass = render(input, parameters: p)
        for i in 4096..<input.count { XCTAssertEqual(bypass.left[i], input[i - 4096]) }
        p.strength = 1; p.scale = 3; p.mask = 0
        let emptyMask = render(input, parameters: p)
        for i in 4096..<input.count { XCTAssertEqual(emptyMask.left[i], input[i - 4096]) }
    }
    func testCheckpointDeepCopyIsIndependentAndPartitionInvariant() {
        let input = voice(frequency: 235, count: 32_000), original = PitchCorrectionState()
        var p = PitchCorrectionParameters(); p.retune = 20; p.tolerance = 0
        for value in input.prefix(8000) { _ = original.process(left: value, right: value, parameters: p) }
        let copy = original.deepCopy()
        XCTAssertGreaterThan(copy.storageBytes, 500_000)
        for chunk in stride(from: 8000, to: input.count, by: 137) {
            for i in chunk..<min(input.count, chunk + 137) {
                let a = original.process(left: input[i], right: input[i], parameters: p)
                let b = copy.process(left: input[i], right: input[i], parameters: p)
                XCTAssertEqual(a.0, b.0); XCTAssertEqual(a.1, b.1)
            }
        }
        let other = copy.deepCopy()
        _ = copy.process(left: 0.9, right: -0.9, parameters: p)
        for i in 0..<5000 {
            let a = original.process(left: 0, right: 0, parameters: p), b = other.process(left: 0, right: 0, parameters: p)
            XCTAssertEqual(a.0, b.0, "checkpoint diverged at \(i)")
        }
    }
    func testFormantsChangeSpectralEnvelopeIndependentlyFromPitch() {
        let fundamental: Float = 220
        let input = (0..<72_000).map { i -> Float in
            let phase = 2 * Float.pi * fundamental * Float(i) / 48_000
            var value: Float = 0.05 * sin(phase)
            for harmonic in 2...12 {
                let weight = 0.06 * exp(-pow((Float(harmonic) - 4) / 1.5, 2))
                value += weight * sin(phase * Float(harmonic))
            }
            return value
        }
        var p = PitchCorrectionParameters(); p.retune = 0; p.vibrato = 0; p.humanize = 0; p.tolerance = 0
        let natural = render(input, parameters: p).left
        p.formant = 12; let raised = render(input, parameters: p).left
        p.formant = -12; let lowered = render(input, parameters: p).left
        func centroid(_ audio: [Float]) -> Float {
            var weighted: Float = 0, total: Float = 0
            for harmonic in 1...12 {
                var real: Float = 0, imaginary: Float = 0
                for i in 36_000..<60_000 {
                    let phase = 2 * Float.pi * fundamental * Float(harmonic) * Float(i) / 48_000
                    real += audio[i] * cos(phase); imaginary += audio[i] * sin(phase)
                }
                let power = real * real + imaginary * imaginary
                weighted += Float(harmonic) * power; total += power
            }
            return weighted / total
        }
        func fundamentalPower(_ audio: [Float]) -> Float {
            var real: Float = 0, imaginary: Float = 0
            for i in 36_000..<60_000 {
                let phase = 2 * Float.pi * fundamental * Float(i) / 48_000
                real += audio[i] * cos(phase); imaginary += audio[i] * sin(phase)
            }
            return real * real + imaginary * imaginary
        }
        // A doubled carrier can still correlate at the original period. Require
        // actual fundamental energy so formant shifting cannot masquerade as pitch preservation.
        XCTAssertGreaterThan(fundamentalPower(raised), fundamentalPower(natural) * 0.02)
        XCTAssertGreaterThan(centroid(raised), centroid(natural) + 1)
        XCTAssertLessThan(centroid(lowered), centroid(natural) - 0.7)
        XCTAssertEqual(frequency(raised, near: fundamental, start: 36_000), fundamental, accuracy: 0.8)
        XCTAssertEqual(frequency(lowered, near: fundamental, start: 36_000), fundamental, accuracy: 0.8)
    }
    func testLowHighVoicesAndNoisyHarmonicVoiceRemainMonophonic() {
        var p = PitchCorrectionParameters(); p.retune = 0; p.humanize = 0; p.vibrato = 0; p.tolerance = 0
        for (inputFrequency, target) in [(Float(75), Float(73.4162)), (Float(800), Float(783.9909))] {
            let result = render(voice(frequency: inputFrequency), parameters: p)
            XCTAssertEqual(frequency(result.left, near: target), target, accuracy: 1)
            XCTAssertEqual(result.state.detectedFrequency, inputFrequency, accuracy: 1)
        }
        var seed: UInt64 = 737
        let noisy = voice(frequency: 250).map { value -> Float in
            seed = seed &* 6364136223846793005 &+ 1
            return value + Float(Int64(bitPattern: seed) >> 40) / Float(1 << 23) * 0.025
        }
        let result = render(noisy, parameters: p)
        XCTAssertEqual(frequency(result.left, near: 246.94165), 246.94165, accuracy: 0.9)
        XCTAssertGreaterThan(result.state.confidence, 0.9)
    }

    func testStrengthAndHumanizeProduceAudibleIndependentPitchBehavior() {
        var p = PitchCorrectionParameters(); p.retune = 0; p.humanize = 0; p.vibrato = 0; p.tolerance = 0; p.strength = 0.5
        let partial = render(voice(frequency: 450), parameters: p)
        XCTAssertEqual(frequency(partial.left, near: sqrt(450 * 440)), sqrt(450 * 440), accuracy: 0.7)
        var phase: Float = 0
        let slide = (0..<96_000).map { i -> Float in
            phase += 2 * Float.pi * (i < 48_000 ? 438 : 448) / 48_000
            return 0.35 * sin(phase) + 0.08 * sin(phase * 2)
        }
        p.strength = 1; p.retune = 40
        let quick = render(slide, parameters: p).left
        p.humanize = 1; let expressive = render(slide, parameters: p).left
        let quickFrequency = frequency(quick, near: 442, start: 57_600)
        let expressiveFrequency = frequency(expressive, near: 442, start: 57_600)
        XCTAssertEqual(quickFrequency, 440, accuracy: 0.8)
        XCTAssertGreaterThan(expressiveFrequency, quickFrequency + 1.3)
    }
    func testVibratoPreservationRetainsIntentionalModulation() {
        var phase: Float = 0
        let input = (0..<96_000).map { i -> Float in
            let frequency = 440 * pow(2, (Float(15) + 40 * sin(2 * Float.pi * 6 * Float(i) / 48_000)) / 1200)
            phase += 2 * Float.pi * frequency / 48_000
            return 0.35 * sin(phase)
        }
        var p = PitchCorrectionParameters(); p.retune = 0; p.humanize = 0; p.tolerance = 0; p.scale = 3; p.mask = 1 << 9; p.vibrato = 0
        let hard = render(input, parameters: p).left
        p.vibrato = 1; let expressive = render(input, parameters: p).left
        func spread(_ audio: [Float]) -> Float {
            var crossings = [Float]()
            for i in 48_001..<90_000 where audio[i - 1] <= 0 && audio[i] > 0 {
                crossings.append(Float(i - 1) - audio[i - 1] / (audio[i] - audio[i - 1]))
            }
            var values = [Float]()
            for j in stride(from: 0, to: crossings.count - 10, by: 10) { values.append(480_000 / (crossings[j + 10] - crossings[j])) }
            let mean = values.reduce(0, +) / Float(values.count)
            return sqrt(values.reduce(0) { $0 + pow($1 - mean, 2) } / Float(values.count))
        }
        XCTAssertGreaterThan(spread(expressive), spread(hard) + 2)
    }

    func testEqualLevelPhaseShiftedStereoUsesOneStableAnalysisChannel() {
        let state = PitchCorrectionState()
        var p = PitchCorrectionParameters(); p.retune = 0; p.humanize = 0; p.vibrato = 0; p.tolerance = 0
        var left = [Float](), right = [Float]()
        for i in 0..<32_000 {
            let phase = 2 * Float.pi * 450 * Float(i) / 48_000
            let a = 0.35 * sin(phase), b = 0.35 * sin(phase + Float.pi / 3)
            let output = state.process(left: a, right: b, parameters: p)
            left.append(output.0); right.append(output.1)
        }
        XCTAssertEqual(state.detectedFrequency, 450, accuracy: 0.5)
        XCTAssertEqual(frequency(left, near: 440, start: 12_000), 440, accuracy: 0.6)
        XCTAssertEqual(frequency(right, near: 440, start: 12_000), 440, accuracy: 0.6)
    }
    func testAbsoluteManualTargetsHonorOctaveAndOverrideScaleMask() throws {
        let input = voice(frequency: 440)
        var p = PitchCorrectionParameters(); p.retune = 0; p.humanize = 0; p.vibrato = 0; p.tolerance = 0
        p.key = 11; p.scale = 3; p.mask = 0 // empty automatic targets must not disable a manual note
        for (note, expected) in [(57, Float(220)), (81, Float(880))] {
            p.manualTarget = Float(note)
            let tuned = render(input, parameters: p)
            XCTAssertEqual(tuned.state.targetFrequency, expected, accuracy: 0.01)
            XCTAssertEqual(frequency(tuned.left, near: expected), expected, accuracy: 1)
            // An estimator constrained near 220 Hz can mistake unchanged 440 Hz
            // for its doubled period. Analyze the actual output over the detector's
            // full voice range, as the before/after chart does.
            let measured = try PitchContourAnalysis.analyze(AudioSamples(channels: [tuned.left]))
                .filter { $0.frame >= 48_000 && $0.isVoiced }.map(\.frequency).sorted()
            if note == 57 {
                func amplitude(_ frequency: Double) -> Double {
                    var real: Double = 0, imaginary: Double = 0
                    for frame in 48_000..<72_000 {
                        let phase = 2 * Double.pi * frequency * Double(frame) / sessionRate
                        real += Double(tuned.left[frame]) * cos(phase)
                        imaginary += Double(tuned.left[frame]) * sin(phase)
                    }
                    return 2 * sqrt(real * real + imaginary * imaginary) / 24_000
                }
                print("Manual octave-down spectrum: 220 Hz \(amplitude(220)), 440 Hz \(amplitude(440)), 660 Hz \(amplitude(660)); measured median \(measured.isEmpty ? 0 : measured[measured.count / 2]) Hz")
            }
            XCTAssertFalse(measured.isEmpty)
            if !measured.isEmpty { XCTAssertEqual(measured[measured.count / 2], expected, accuracy: 2) }
            XCTAssertTrue(tuned.left.allSatisfy { $0.isFinite })
        }
        p.manualTarget = -1
        let automatic = render(input, parameters: p)
        for frame in PitchCorrectionState.latencyFrames..<input.count {
            XCTAssertEqual(automatic.left[frame], input[frame - PitchCorrectionState.latencyFrames], "Automatic empty-mask bypass remains exact")
        }
    }
    func testManualTargetValidationAndOneOctaveSafetyBound() {
        XCTAssertNil(PitchCorrectionState.clampedManualTarget(Int.min))
        XCTAssertNil(PitchCorrectionState.clampedManualTarget(-1))
        XCTAssertEqual(PitchCorrectionState.clampedManualTarget(0), 0)
        XCTAssertEqual(PitchCorrectionState.clampedManualTarget(Int.max), 95)
        var p = PitchCorrectionParameters(); p.retune = 0; p.humanize = 0; p.vibrato = 0; p.tolerance = 0; p.manualTarget = 95
        let input = voice(frequency: 220), tuned = render(input, parameters: p)
        XCTAssertEqual(frequency(tuned.left, near: 440), 440, accuracy: 1, "Distant absolute notes cannot shift beyond the supported octave")
        XCTAssertLessThanOrEqual(abs(tuned.state.meterSnapshot.correctionCents), 1200.01)
        XCTAssertTrue(tuned.left.allSatisfy { $0.isFinite })
    }
    func testMeasuredManualDownwardSeventhNinthAndOctave() throws {
        let input = voice(frequency: 440, count: 72_000)
        var p = PitchCorrectionParameters(); p.retune = 0; p.humanize = 0; p.vibrato = 0; p.tolerance = 0
        for semitones in [7, 9, 12] {
            p.manualTarget = Float(69 - semitones)
            let tuned = render(input, parameters: p)
            let measured = try PitchContourAnalysis.analyze(AudioSamples(channels: [tuned.left]))
                .filter { $0.frame >= 36_000 && $0.isVoiced }.map(\.frequency).sorted()
            let expected = Float(440) * pow(2, -Float(semitones) / 12)
            XCTAssertFalse(measured.isEmpty)
            if !measured.isEmpty { XCTAssertEqual(measured[measured.count / 2], expected, accuracy: 2, "Actual output after \(semitones)-semitone manual downshift") }
            XCTAssertTrue(tuned.left.allSatisfy { $0.isFinite && abs($0) <= 1 })
        }
    }
    func testManualDownwardTuningKeepsStereoLinkedAndReleasesToUnvoicedDry() {
        var seed: UInt64 = 173
        let noise = (0..<16_000).map { _ -> Float in
            seed = seed &* 6364136223846793005 &+ 1
            return Float(Int64(bitPattern: seed) >> 40) / Float(1 << 23) * 0.2
        }
        let input = voice(frequency: 440, count: 32_000) + Array(repeating: Float(0), count: 8192) + noise
        var p = PitchCorrectionParameters(); p.manualTarget = 57; p.retune = 0; p.humanize = 0; p.vibrato = 0; p.tolerance = 0
        let tuned = render(input, parameters: p, rightGain: -0.7)
        XCTAssertTrue(tuned.left.allSatisfy { $0.isFinite && abs($0) <= 1 })
        for frame in 12_000..<tuned.left.count {
            XCTAssertEqual(tuned.right[frame], tuned.left[frame] * -0.7, accuracy: 0.00001)
        }
        for frame in (32_000 + PitchCorrectionState.latencyFrames + 4096)..<tuned.left.count {
            XCTAssertEqual(tuned.left[frame], input[frame - PitchCorrectionState.latencyFrames], "Unvoiced audio remains latency-aligned dry after manual tuning")
        }
    }

}
