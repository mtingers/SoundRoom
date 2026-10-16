import XCTest
@testable import SoundroomCore

/// Signal-level regressions for vowel boundaries and consonants; these are not a
/// substitute for a listening pass on a microphone recording.
final class PitchArtifactRefinementTests: XCTestCase {
    private func hardTune() -> PitchCorrectionParameters {
        var p = PitchCorrectionParameters()
        p.retune = 0; p.humanize = 0; p.vibrato = 0; p.tolerance = 0
        return p
    }
    private func render(_ input: [Float], _ p: PitchCorrectionParameters) -> [Float] {
        let state = PitchCorrectionState()
        return input.map { state.process(left: $0, right: -0.5 * $0, parameters: p).0 }
    }
    func testUnvoicedConsonantAfterVowelDoesNotLoseItsTransient() {
        let count = 36_000, boundary = 18_000
        var random: UInt64 = 937
        var input = [Float](repeating: 0, count: count)
        for i in 0..<boundary {
            let phase = Double(i) * 2 * .pi * 450 / 48_000
            input[i] = Float(0.24 * sin(phase) + 0.08 * sin(phase * 2 + 0.4))
        }
        for i in boundary..<count {
            random = random &* 6364136223846793005 &+ 1
            input[i] = Float(Int64(bitPattern: random) >> 40) / Float(1 << 23) * 0.2
        }
        // A plosive 18.75 ms into the unvoiced phrase must retain its energy;
        // keeping the voiced crossfade on an empty grain ring attenuated it.
        let transient = boundary + 900
        input[transient] = 0.72
        let output = render(input, hardTune())
        let offset = PitchCorrectionState.latencyFrames
        XCTAssertEqual(output[transient + offset], input[transient], accuracy: 0.025)
        var error: Double = 0, energy: Double = 0
        for i in (boundary + 1500)..<(boundary + 4000) {
            let d = Double(output[i + offset] - input[i])
            error += d * d; energy += Double(input[i] * input[i])
        }
        XCTAssertLessThan(sqrt(error / energy), 0.005)
    }
    func testChangingVowelMaintainsGainWithoutPulseWindowDropouts() {
        let count = 60_000
        let input = (0..<count).map { i -> Float in
            let phase = Double(i) * 2 * .pi * 450 / 48_000
            // Fundamental remains fixed while vocal-like harmonic balance moves.
            let vowel = 0.5 + 0.5 * sin(Double(i) * 2 * .pi * 3 / 48_000)
            return Float(0.2 * sin(phase) + (0.03 + 0.15 * vowel) * sin(2 * phase + 0.2) +
                (0.16 - 0.13 * vowel) * sin(3 * phase + 0.8))
        }
        let output = render(input, hardTune())
        var leastRatio = Double.infinity, greatestRatio: Double = 0
        for source in stride(from: 6000, to: 50_000, by: 1000) {
            var dry: Double = 0, wet: Double = 0
            for i in source..<(source + 1000) {
                dry += Double(input[i] * input[i])
                let value = output[i + PitchCorrectionState.latencyFrames]
                wet += Double(value * value)
            }
            let ratio = sqrt(wet / dry)
            leastRatio = min(leastRatio, ratio); greatestRatio = max(greatestRatio, ratio)
        }
        XCTAssertGreaterThan(leastRatio, 0.8)
        XCTAssertLessThan(greatestRatio, 1.15)
        XCTAssertTrue(output.allSatisfy { $0.isFinite && abs($0) < 0.8 })
        let dryStep = (6001..<50_000).map { abs(input[$0] - input[$0 - 1]) }.max()!
        let wetStep = (6001..<50_000).map { abs(output[$0 + PitchCorrectionState.latencyFrames] - output[$0 + PitchCorrectionState.latencyFrames - 1]) }.max()!
        XCTAssertLessThan(wetStep, dryStep * 1.35, "A changing harmonic envelope must not introduce an extra pulse discontinuity")
    }
    func testNoteBoundaryDoesNotChatterOnSmallDetectorVariation() {
        let state = PitchCorrectionState(), p = hardTune()
        var phase: Double = 0
        var targets: [Float] = []
        for i in 0..<48_000 {
            // Approach the A/A# midpoint from below, then fluctuate ±2 cents.
            let cents = i < 12_000 ? 43 : 50 + 2 * sin(Double(i) * 2 * .pi * 8 / 48_000)
            let frequency = 440 * pow(2, cents / 1200)
            phase += 2 * .pi * frequency / 48_000
            let value = Float(0.25 * sin(phase) + 0.06 * sin(phase * 2 + 0.4))
            _ = state.process(left: value, right: value, parameters: p)
            if i >= 16_000 && i % 256 == 0 { targets.append(state.targetFrequency) }
        }
        XCTAssertTrue(targets.allSatisfy { abs($0 - 440) < 0.1 }, "Repeated target flips introduce semitone jumps: \(targets)")
    }
    func testNewVoicedPhraseResetsPreviousNoteCorrectionAndMeterIsSourceAligned() {
        let state = PitchCorrectionState(), p = hardTune()
        var lastMeter = PitchMeterSnapshot()
        for i in 0..<38_000 {
            let value: Float
            if i < 12_000 {
                value = Float(0.3 * sin(Double(i) * 2 * .pi * 450 / 48_000))
            } else if i < 24_000 {
                value = 0
            } else {
                value = Float(0.3 * sin(Double(i - 24_000) * 2 * .pi * 255 / 48_000))
            }
            _ = state.process(left: value, right: value, parameters: p)
            let meter = state.meterSnapshot
            XCTAssertEqual(meter.sourceFrame, i - PitchCorrectionState.latencyFrames)
            if i == 22_000 {
                XCTAssertEqual(meter.inputFrequency, 0)
                XCTAssertEqual(meter.targetFrequency, 0)
                XCTAssertEqual(meter.correctionCents, 0)
                XCTAssertLessThan(meter.wetBlend, 0.00001)
            }
            lastMeter = meter
        }
        XCTAssertEqual(lastMeter.inputFrequency, 255, accuracy: 0.6)
        XCTAssertEqual(lastMeter.targetFrequency, 261.6256, accuracy: 0.5)
        XCTAssertEqual(lastMeter.correctionCents, 1200 * log2(261.6256 / 255), accuracy: 3)
        XCTAssertGreaterThan(lastMeter.wetBlend, 0.99)
    }
}
