import XCTest

@testable import SoundroomCore

/// Signal-level checks measure the rendered signal instead of relying on meters.
final class VocalEnhancerTests: XCTestCase {
    private let neutral: [Float] = [0, 0, 0, 0, 0, 0, 0, 0, 1]
    private let natural: [Float] = [0.45, 0, 0.45, 0.5, 0.45, 0.2, 0.2, 0, 1]

    func testNeutralAndDryBlendPreserveExactDelayedAudio() {
        for dryOnly in [false, true] {
            let state = VocalEnhancerDSP()
            var values = dryOnly ? natural : neutral
            values[8] = dryOnly ? 0 : 1
            state.configure(values)
            var input = [Float]()
            for frame in 0..<8000 {
                if dryOnly, frame % 31 == 0 {
                    values[0] = Float(frame % 100) / 100
                    values[1] = Float(frame % 13) - 6
                    values[7] = Float(frame % 25) - 12
                    state.configure(values)
                }
                let x = Float(sin(Double(frame) * 0.137)) * 0.12
                input.append(x)
                let y = state.process(left: x, right: -x * 0.25)
                let expected =
                    frame < VocalEnhancerDSP.latencyFrames ? 0 : input[frame - VocalEnhancerDSP.latencyFrames]
                XCTAssertEqual(state.lastDryLeft, expected)
                XCTAssertEqual(state.lastDryRight, -expected * 0.25)
                XCTAssertEqual(y.0, expected, accuracy: 0.0000002)
                XCTAssertEqual(y.1, -expected * 0.25, accuracy: 0.0000002)
            }
        }
    }

    func testSibilanceReductionIsSelective() {
        let plain = VocalEnhancerDSP()
        let deEssed = VocalEnhancerDSP()
        plain.configure(neutral)
        var deEss = neutral
        deEss[3] = 1
        deEssed.configure(deEss)
        var reference = [Float]()
        var treated = [Float]()
        for frame in 0..<96_000 {
            let t = Double(frame) / sessionRate
            let sibilant = frame % 24_000 >= 6000 && frame % 24_000 < 18_000
            let x = Float(sin(2 * .pi * 240 * t)) * 0.08 + (sibilant ? Float(sin(2 * .pi * 8000 * t)) * 0.22 : 0)
            let a = plain.process(left: x, right: x)
            let b = deEssed.process(left: x, right: x)
            if (81_000 + VocalEnhancerDSP.latencyFrames..<87_000 + VocalEnhancerDSP.latencyFrames).contains(frame) {
                reference.append(a.0)
                treated.append(b.0)
            }
        }
        let lowChange = 20 * log10(amplitude(treated, at: 240) / amplitude(reference, at: 240))
        let highChange = 20 * log10(amplitude(treated, at: 8000) / amplitude(reference, at: 8000))
        XCTAssertGreaterThan(lowChange, -1, "De-essing must retain the vocal body")
        XCTAssertLessThan(highChange, -4, "Strong sibilance receives measurable selective attenuation")
    }

    func testDynamicsReduceLevelContrastWithoutChangingStereoImage() {
        var controls = neutral
        controls[4] = 1
        let quiet = VocalEnhancerDSP()
        let loud = VocalEnhancerDSP()
        quiet.configure(controls)
        loud.configure(controls)
        var quietPower: Double = 0
        var loudPower: Double = 0
        for frame in 0..<96_000 {
            let sine = Float(sin(2 * .pi * 220 * Double(frame) / sessionRate))
            let a = quiet.process(left: sine * 0.025, right: sine * 0.00625)
            let b = loud.process(left: sine * 0.4, right: sine * 0.1)
            XCTAssertEqual(a.1, a.0 * 0.25, accuracy: 0.0000001)
            XCTAssertEqual(b.1, b.0 * 0.25, accuracy: 0.0000001)
            if frame >= 48_000 {
                quietPower += Double(a.0 * a.0)
                loudPower += Double(b.0 * b.0)
            }
        }
        let outputContrast = 10 * log10(loudPower / quietPower)
        let inputContrast = 20 * log10(0.4 / 0.025)
        XCTAssertLessThan(outputContrast, inputContrast - 4)
        XCTAssertGreaterThan(quietPower, 0.01, "A quiet voiced signal remains audible")
    }

    func testCleanupAttenuatesLowNoiseWithABoundedFloor() {
        let state = VocalEnhancerDSP()
        var controls = neutral
        controls[0] = 1
        state.configure(controls)
        var sourcePower: Double = 0
        var outputPower: Double = 0
        var seed: UInt64 = 0x1badb002
        for frame in 0..<96_000 {
            seed = seed &* 6364136223846793005 &+ 1442695040888963407
            let x = (Float(seed >> 40) / Float(1 << 24) * 2 - 1) * 0.0004
            let y = state.process(left: x, right: -x)
            if frame >= 48_000 {
                sourcePower += Double(x * x)
                outputPower += Double(y.0 * y.0)
            }
        }
        let gain = sqrt(outputPower / sourcePower)
        XCTAssertLessThan(gain, 0.7)
        XCTAssertGreaterThan(gain, 0.03, "Cleanup uses a soft floor rather than abruptly deleting ambience")
    }

    func testCleanupRetainsAQuietVoicedSustainAfterLearningTheNoiseFloor() {
        let state = VocalEnhancerDSP()
        var controls = neutral
        controls[0] = 1
        state.configure(controls)
        var sourcePower: Double = 0
        var outputPower: Double = 0
        for frame in 0..<192_000 {
            let t = Double(frame) / sessionRate
            let x = Float(sin(2 * .pi * 220 * t)) * 0.002 + Float(sin(2 * .pi * 440 * t)) * 0.0004
            let y = state.process(left: x, right: x)
            if frame >= 144_000 {
                sourcePower += Double(x * x)
                outputPower += Double(y.0 * y.0)
            }
        }
        XCTAssertGreaterThan(
            10 * log10(outputPower / sourcePower), -1.2,
            "A held quiet note must not be learned as noise and expanded away")
    }

    func testCleanupRetainsQuietHighVoicesAcrossTheVocalRange() {
        for frequency in [660.0, 880.0, 1000.0, 1500.0] {
            for cleanup: Float in [0.45, 1] {
                let state = VocalEnhancerDSP()
                var controls = neutral
                controls[0] = cleanup
                state.configure(controls)
                var sourcePower: Double = 0
                var outputPower: Double = 0
                for frame in 0..<240_000 {
                    let phase = 2 * Double.pi * frequency * Double(frame) / sessionRate
                    let x = Float(sin(phase) + 0.5 * sin(2 * phase) + 0.2 * sin(3 * phase)) * 0.002
                    let y = state.process(left: x, right: x)
                    if frame >= 192_000 {
                        sourcePower += Double(x * x)
                        outputPower += Double(y.0 * y.0)
                    }
                }
                XCTAssertGreaterThan(
                    10 * log10(outputPower / sourcePower), -1.2,
                    "Quiet \(frequency) Hz voice must not fade after learning the noise floor (cleanup \(cleanup))")
            }
        }
    }

    func testCleanupStillReducesBroadbandNoiseAtTheQuietVoiceLevel() {
        let state = VocalEnhancerDSP()
        var controls = neutral
        controls[0] = 1
        state.configure(controls)
        var sourcePower: Double = 0
        var outputPower: Double = 0
        var seed: UInt64 = 0x22c075
        for frame in 0..<240_000 {
            seed = seed &* 6364136223846793005 &+ 1442695040888963407
            let x = (Float(seed >> 40) / Float(1 << 24) * 2 - 1) * 0.0028
            let y = state.process(left: x, right: -x)
            if frame >= 192_000 {
                sourcePower += Double(x * x)
                outputPower += Double(y.0 * y.0)
            }
        }
        XCTAssertLessThan(
            10 * log10(outputPower / sourcePower), -4,
            "Protecting quiet pitched content must not disable broadband noise cleanup at the same level")
    }

    func testNeutralTransientIsPreservedAndWetOutputTrimIsAligned() {
        let state = VocalEnhancerDSP()
        var controls = neutral
        controls[7] = -6
        state.configure(controls)
        let scale = pow(Float(10), -6 / 20)
        for frame in 0..<1000 {
            let x: Float = frame == 5 ? 0.7 : 0
            let y = state.process(left: x, right: -x)
            let expected: Float = frame == 5 + VocalEnhancerDSP.latencyFrames ? 0.7 * scale : 0
            XCTAssertEqual(y.0, expected, accuracy: 0.0000002)
            XCTAssertEqual(y.1, -expected, accuracy: 0.0000002)
        }
    }

    func testSilenceAndNonfiniteInputCannotPoisonHistories() {
        let bad = VocalEnhancerDSP()
        let sanitized = VocalEnhancerDSP()
        bad.configure([1, 6, 1, 1, 1, 1, 1, 12, 1])
        sanitized.configure([1, 6, 1, 1, 1, 1, 1, 12, 1])
        for frame in 0..<6000 {
            let sample: Float = frame > 10 ? Float(sin(Double(frame) * 0.12)) * 0.04 : 0
            let l = frame == 3 ? Float.nan : sample
            let r = frame == 7 ? Float.infinity : -sample
            let a = bad.process(left: l, right: r)
            let b = sanitized.process(left: l.isFinite ? l : 0, right: r.isFinite ? r : 0)
            XCTAssertTrue(a.0.isFinite)
            XCTAssertTrue(a.1.isFinite)
            XCTAssertEqual(a.0, b.0)
            XCTAssertEqual(a.1, b.1)
        }
        let silence = VocalEnhancerDSP()
        silence.configure([1, -6, 1, 1, 1, 1, 1, 12, 1])
        for _ in 0..<12_000 {
            let y = silence.process(left: 0, right: 0)
            XCTAssertEqual(y.0, 0)
            XCTAssertEqual(y.1, 0)
        }
    }

    func testCheckpointContinuationAndIndependentStorage() {
        let state = VocalEnhancerDSP()
        state.configure(natural)
        for frame in 0..<4003 {
            let x = Float(sin(Double(frame) * 0.061)) * 0.6
            _ = state.process(left: x, right: x * -0.4)
        }
        let copy = state.deepCopy()
        for frame in 0..<7000 {
            if frame == 517 {
                state.configure([0.7, 2, 0.6, 0.8, 0.8, 0.4, 0.5, -1, 0.8])
                copy.configure([0.7, 2, 0.6, 0.8, 0.8, 0.4, 0.5, -1, 0.8])
            }
            let x = Float(sin(Double(frame) * 0.017)) * 0.3
            let a = state.process(left: x, right: -x * 0.4)
            let b = copy.process(left: x, right: -x * 0.4)
            XCTAssertEqual(a.0, b.0)
            XCTAssertEqual(a.1, b.1)
            XCTAssertEqual(state.lastDryLeft, copy.lastDryLeft)
            XCTAssertEqual(state.reductionDB, copy.reductionDB)
        }
        let untouched = copy.deepCopy()
        state.configure([1, -6, 1, 1, 1, 1, 1, 12, 1])
        for frame in 0..<1000 {
            _ = state.process(left: 3, right: -3)
            let x = Float(frame % 19 - 9) * 0.01
            let a = copy.process(left: x, right: x)
            let b = untouched.process(left: x, right: x)
            XCTAssertEqual(a.0, b.0)
            XCTAssertEqual(a.1, b.1)
        }
    }

    func testSafetyGuardLimitsIndependentlyReconstructedPeaks() {
        for fixture in 0..<4 {
            let state = VocalEnhancerDSP()
            state.configure([0.6, 6, 1, 0.7, 0.8, 1, 1, 12, fixture == 3 ? 0.35 : 1])
            var left = [Float]()
            var right = [Float]()
            for frame in 0..<12_500 {
                let x: Float
                switch fixture {
                case 0: x = Float(sin(2 * .pi * 17000 * Double(frame) / sessionRate)) * 1.8
                case 1: x = frame % 401 == 0 ? 4 : 0
                default: x = Float(sin(Double(frame) * 0.11) + sin(Double(frame) * 1.93)) * 1.6
                }
                let input = frame < 12_000 ? x : 0
                let y = state.process(left: input, right: input * -0.63)
                left.append(y.0)
                right.append(y.1)
            }
            let peak = max(independentTruePeak(left), independentTruePeak(right))
            XCTAssertLessThanOrEqual(
                20 * log10(peak), -0.485, "Fixture \(fixture) must remain below the fixed safety ceiling")
        }
    }

    func testRustReferenceMatchesAcrossAutomatedControlsAndFlush() {
        // Independent Rust implementation checkpoints; dyadic input avoids
        // platform differences in sine generation. Both engines use the same
        // source-time changes, including warm/output/mix latency alignment.
        let expected: [Int: (Float, Float)] = [
            143: (-0.000532157, -0.000400381), 144: (-0.065350316, -0.049016543),
            145: (-0.063091286, -0.046941914), 511: (-0.060315836, -0.033522986),
            1023: (0.013525319, 0.018826062), 1168: (-0.034652032, -0.025320491),
            2048: (0.015872864, 0.020928826), 2192: (-0.011578474, -0.008509722),
            4096: (0.014452861, 0.020436611), 4240: (-0.027537454, -0.020545397),
            6144: (0.016535927, 0.023505779), 6288: (-0.010082271, -0.007295184),
            8191: (0.011685637, 0.016186375), 8335: (0.047580063, 0.035795461),
        ]
        let state = VocalEnhancerDSP()
        var values = natural
        state.configure(values)
        for frame in 0..<8336 {
            switch frame {
            case 1024:
                values[2] = 0.8
                values[3] = 0.9
                state.configure(values)
            case 2048:
                values[4] = 0.9
                values[1] = 2
                state.configure(values)
            case 4096:
                values[6] = 0.7
                values[5] = 0.45
                values[7] = 1.5
                values[8] = 0.7
                state.configure(values)
            case 6144:
                values[0] = 0.8
                values[8] = 1
                values[7] = -2
                state.configure(values)
            default: break
            }
            let left: Float = frame >= 8192 ? 0 : (Float(frame * 17 % 1024) - 512) / 1024 * 0.125
            let right: Float = frame >= 8192 ? 0 : left * 0.5 + (Float(frame * 29 % 1024) - 512) / 1024 * 0.03125
            let y = state.process(left: left, right: right)
            if let target = expected[frame] {
                XCTAssertEqual(y.0, target.0, accuracy: 0.0000001, "Left frame \(frame)")
                XCTAssertEqual(y.1, target.1, accuracy: 0.0000001, "Right frame \(frame)")
            }
        }
    }

    private func amplitude(_ samples: [Float], at frequency: Double) -> Double {
        var real: Double = 0
        var imaginary: Double = 0
        for (i, x) in samples.enumerated() {
            let phase = 2 * Double.pi * frequency * Double(i) / sessionRate
            real += Double(x) * cos(phase)
            imaginary += Double(x) * sin(phase)
        }
        return 2 * hypot(real, imaginary) / Double(samples.count)
    }

    private func independentTruePeak(_ samples: [Float]) -> Double {
        // Published BS.1770-5 Annex 2 polyphase convolution, measured outside
        // the processor and including filter flush. No processor meter is used.
        let phases: [[Double]] = [
            [
                0.001708984375, 0.010986328125, -0.0196533203125, 0.033203125, -0.0594482421875, 0.1373291015625,
                0.97216796875, -0.102294921875, 0.047607421875, -0.026611328125, 0.014892578125, -0.00830078125,
            ],
            [
                -0.0291748046875, 0.029296875, -0.0517578125, 0.089111328125, -0.16650390625, 0.465087890625,
                0.77978515625, -0.2003173828125, 0.1015625, -0.0582275390625, 0.0330810546875, -0.0189208984375,
            ],
            [
                -0.0189208984375, 0.0330810546875, -0.0582275390625, 0.1015625, -0.2003173828125, 0.77978515625,
                0.465087890625, -0.16650390625, 0.089111328125, -0.0517578125, 0.029296875, -0.0291748046875,
            ],
            [
                -0.00830078125, 0.014892578125, -0.026611328125, 0.047607421875, -0.102294921875, 0.97216796875,
                0.1373291015625, -0.0594482421875, 0.033203125, -0.0196533203125, 0.010986328125, 0.001708984375,
            ],
        ]
        var peak = Double(samples.map { abs($0) }.max() ?? 0)
        for n in 0..<(samples.count + 12) {
            for phase in phases {
                var value: Double = 0
                for (tap, coefficient) in phase.enumerated() where n >= tap && n - tap < samples.count {
                    value += coefficient * Double(samples[n - tap])
                }
                peak = max(peak, abs(value))
            }
        }
        return peak
    }
}
