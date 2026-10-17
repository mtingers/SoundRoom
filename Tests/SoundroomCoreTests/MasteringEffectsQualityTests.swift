import XCTest

@testable import SoundroomCore

/// Signal-level acceptance checks use independent output reconstruction rather
/// than trusting the effect's own peak or gain-reduction telemetry.
final class MasteringEffectsQualityTests: XCTestCase {
    func testLimiterReconstructedOutputHonorsCeilingForDifficultSignals() {
        let ceiling: Float = -1
        for kind in 0..<5 {
            let state = LimiterDSP()
            state.configure([6, ceiling, 80, 1, 1])
            var left = [Float]()
            var right = [Float]()
            var seed: UInt64 = 0x12a45
            left.reserveCapacity(49_200)
            right.reserveCapacity(49_200)
            for frame in 0..<49_200 {
                let t = Double(frame) / sessionRate
                let sample: Float
                switch kind {
                case 0: sample = Float(sin(2 * .pi * 17000 * t)) * 1.6
                case 1: sample = frame % 499 == 0 ? 3 : 0
                case 2: sample = frame % 2 == 0 ? 1.8 : -1.8
                case 3:
                    seed = seed &* 6364136223846793005 &+ 1442695040888963407
                    sample = (Float(seed >> 40) / Float(1 << 24) * 2 - 1) * 2
                default:
                    sample = Float(sin(2 * .pi * 113 * t) + sin(2 * .pi * 7011 * t) + sin(2 * .pi * 18233 * t))
                }
                let input = frame < 48_000 ? sample : 0
                let output = state.process(left: input, right: input * (kind == 3 ? -0.63 : 0.5))
                left.append(output.0)
                right.append(output.1)
            }
            let measured = 20 * log10(max(independentTruePeak(left), independentTruePeak(right)))
            print("Limiter output fixture \(kind): \(measured) dBTP")
            XCTAssertLessThanOrEqual(measured, Double(ceiling) + 0.015)
        }
    }

    func testLimiterQuietSignalAndAutomatedDryHistoryAreUnchanged() {
        let state = LimiterDSP()
        state.configure([0, -1, 150, 1, 1])
        var input = [Float]()
        for frame in 0..<12_000 {
            let x = Float(sin(Double(frame) * 0.013)) * 0.04
            input.append(x)
            let output = state.process(left: x, right: -x)
            if frame >= LimiterDSP.latencyFrames {
                XCTAssertEqual(output.0, input[frame - LimiterDSP.latencyFrames])
                XCTAssertEqual(output.1, -input[frame - LimiterDSP.latencyFrames])
            }
        }
        let automated = LimiterDSP()
        input.removeAll(keepingCapacity: true)
        for frame in 0..<4000 {
            automated.configure([Float(frame % 37) - 12, -1, 150, 1, 1])
            let x = Float(sin(Double(frame) * 0.071)) * 0.23
            input.append(x)
            _ = automated.process(left: x, right: -x)
            if frame >= LimiterDSP.latencyFrames {
                XCTAssertEqual(
                    automated.lastDryLeft, input[frame - LimiterDSP.latencyFrames],
                    "Bypass retains original float samples even when wet input gain changes")
                XCTAssertEqual(automated.lastDryRight, -input[frame - LimiterDSP.latencyFrames])
            }
        }
    }

    func testLimiterStereoLinkAndCheckpointContinuation() {
        let state = LimiterDSP()
        state.configure([8, -2, 120, 1, 1])
        for frame in 0..<3000 {
            let x = Float(sin(Double(frame) * 0.09))
            let output = state.process(left: x, right: x * 0.25)
            XCTAssertEqual(output.1, output.0 * 0.25)
        }
        let copy = state.deepCopy()
        for frame in 0..<3000 {
            if frame == 300 {
                state.configure([-3, -4, 35, 0, 1])
                copy.configure([-3, -4, 35, 0, 1])
            }
            let x = Float(sin(Double(frame) * 0.17))
            let a = state.process(left: x, right: -x * 0.7)
            let b = copy.process(left: x, right: -x * 0.7)
            XCTAssertEqual(a.0, b.0)
            XCTAssertEqual(a.1, b.1)
            XCTAssertEqual(state.lastDryLeft, copy.lastDryLeft)
        }
        XCTAssertEqual(state.loudness.momentary, copy.loudness.momentary)
    }

    func testLoudnessStereoReferenceAndSilence() {
        let loudness = LoudnessDSP()
        for frame in 0..<192_000 {
            // Stereo 1 kHz sine with −20 dBFS peak. BS.1770 K-weighting
            // gives −19.993 LUFS at 48 kHz for this reference waveform.
            let sample = Float(sin(2 * .pi * 1000 * Double(frame) / sessionRate)) * 0.1
            loudness.process(sample, sample)
        }
        XCTAssertEqual(loudness.momentary, -19.993, accuracy: 0.04)
        XCTAssertEqual(loudness.shortTerm, -19.993, accuracy: 0.04)
        for _ in 0..<192_000 { loudness.process(0, 0) }
        XCTAssertLessThan(loudness.momentary, -90)
        XCTAssertLessThan(loudness.shortTerm, -90)
    }

    func testResonanceNeutralAndDryMixHaveCorrectLatency() {
        let variants: [[Float]] = [
            [0, 1, 0.6, 500, 16000, 1, 100, 0, 1, 0], [12, 1, 0.6, 500, 16000, 1, 100, 0, 0, 0],
        ]
        for values in variants {
            let state = ResonanceDSP()
            state.configure(values)
            var input = [Float]()
            for frame in 0..<16_000 {
                let x = Float(sin(Double(frame) * 0.13)) * 0.25 + Float(sin(Double(frame) * 0.77)) * 0.07
                input.append(x)
                let output = state.process(left: x, right: -x)
                let expected = frame >= ResonanceDSP.latencyFrames ? input[frame - ResonanceDSP.latencyFrames] : 0
                XCTAssertEqual(output.0, expected, accuracy: 0.000002)
                XCTAssertEqual(output.1, -expected, accuracy: 0.000002)
            }
        }
    }

    func testResonanceAttenuatesInRangePeakAndRetainsOutOfRangeTone() {
        let state = ResonanceDSP()
        state.configure([12, 1, 0.6, 1000, 6000, 1, 100, 0, 1, 0])
        var input = [Float]()
        var output = [Float]()
        for frame in 0..<96_000 {
            let t = Double(frame) / sessionRate
            let x = Float(sin(2 * .pi * 3000 * t) + sin(2 * .pi * 400 * t)) * 0.2
            let y = state.process(left: x, right: x)
            if frame >= 48_000 {
                let alignedTime = Double(frame - ResonanceDSP.latencyFrames) / sessionRate
                input.append(Float(sin(2 * .pi * 3000 * alignedTime) + sin(2 * .pi * 400 * alignedTime)) * 0.2)
                output.append(y.0)
            }
        }
        let reduction = 20 * log10(amplitude(output, 3000) / amplitude(input, 3000))
        let retained = 20 * log10(amplitude(output, 400) / amplitude(input, 400))
        print("Resonance selected tone \(reduction) dB; outside range \(retained) dB")
        XCTAssertLessThan(reduction, -8)
        XCTAssertGreaterThan(reduction, -12.2)
        XCTAssertEqual(retained, 0, accuracy: 0.04)
        XCTAssertGreaterThan(state.display.bands.max() ?? 0, 8)
    }

    func testResonanceDeltaReconstructsRemovedSignalAndCopiesExactly() {
        let wet = ResonanceDSP()
        let delta = ResonanceDSP()
        wet.configure([8, 0.8, 0.7, 600, 16000, 5, 140, 0.3, 0.7, 0])
        delta.configure([8, 0.8, 0.7, 600, 16000, 5, 140, 0.3, 0.7, 1])
        for frame in 0..<11_017 {
            let x = Float(sin(Double(frame) * 0.39)) * 0.2
            let a = wet.process(left: x, right: -x)
            let b = delta.process(left: x, right: -x)
            XCTAssertEqual(a.0 + b.0, wet.lastDryLeft, accuracy: 0.0000002)
            XCTAssertEqual(a.1 + b.1, wet.lastDryRight, accuracy: 0.0000002)
        }
        let copy = wet.deepCopy()
        for frame in 0..<7000 {
            let x = Float(sin(Double(frame) * 0.23)) * 0.3
            let a = wet.process(left: x, right: x * 0.2)
            let b = copy.process(left: x, right: x * 0.2)
            XCTAssertEqual(a.0, b.0)
            XCTAssertEqual(a.1, b.1)
        }
        XCTAssertEqual(wet.display, copy.display)
    }

    func testWarmthCharactersRejectAsymmetryDCAndRemainDistinct() {
        var signatures = [Double]()
        for model in 0..<5 {
            let state = WarmthDSP()
            var sum: Double = 0
            var energy: Double = 0
            for frame in 0..<96_000 {
                let x = Float(sin(2 * .pi * 80 * Double(frame) / sessionRate)) * 0.5
                let y = state.process(
                    left: x, right: -x, driveDB: 18, tone: 12000, mix: 1, outputDB: 0, matchLevel: false,
                    character: model, bias: 0.8)
                if frame >= 48_000 {
                    sum += Double(y.0)
                    energy += Double(y.0 * y.0)
                }
                XCTAssertTrue(y.0.isFinite)
                XCTAssertTrue(y.1.isFinite)
            }
            XCTAssertLessThan(abs(sum / 48_000), 0.0001, "Model \(model) must not introduce sustained DC")
            signatures.append(sqrt(energy / 48_000))
        }
        for a in 0..<5 { for b in (a + 1)..<5 { XCTAssertGreaterThan(abs(signatures[a] - signatures[b]), 0.002) } }
    }

    func testAllNewWarmthModelsSuppressFoldedHighFrequencyHarmonics() {
        for model in 1..<5 {
            let state = WarmthDSP()
            let drive = pow(Float(10), 24 / 20)
            var oversampled = [Float]()
            var naive = [Float]()
            for frame in 0..<48_000 {
                let x = Float(sin(2 * .pi * 9000 * Double(frame) / sessionRate)) * 0.7
                let y = state.process(
                    left: x, right: x, driveDB: 24, tone: 20000, mix: 1, outputDB: 0, matchLevel: false,
                    character: model, bias: 0.6)
                if frame >= 4000 {
                    oversampled.append(y.0)
                    naive.append(
                        WarmthDSP.shape(
                            x * drive, drive: drive, normalization: tanh(drive), character: model, bias: 0.6))
                }
            }
            let filteredRatio = amplitude(oversampled, 21000) / amplitude(oversampled, 9000)
            let naiveRatio = amplitude(naive, 21000) / amplitude(naive, 9000)
            XCTAssertLessThan(
                filteredRatio, naiveRatio * 0.12,
                "Character \(model) must reject harmonics above the base-rate Nyquist limit")
        }
    }

    func testWarmthClassicCompatibilityAndModelSwitchCheckpoint() {
        let classic = WarmthDSP()
        let explicit = WarmthDSP()
        for frame in 0..<3000 {
            let x = Float(sin(Double(frame) * 0.071)) * 0.6
            let a = classic.process(
                left: x, right: -x, driveDB: 13, tone: 8000, mix: 0.6, outputDB: -3, matchLevel: true)
            let b = explicit.process(
                left: x, right: -x, driveDB: 13, tone: 8000, mix: 0.6, outputDB: -3, matchLevel: true, character: 0,
                bias: 0.9, emphasisDB: 12)
            XCTAssertEqual(a.0, b.0)
            XCTAssertEqual(a.1, b.1)
        }
        // Clone in the middle of overlapping model changes, including DC and
        // emphasis blending, rather than only a settled character.
        for _ in 0..<73 {
            _ = classic.process(
                left: 0.2, right: -0.1, driveDB: 12, tone: 12000, mix: 1, outputDB: 0, matchLevel: false, character: 2,
                bias: 0.8, emphasisDB: 8)
        }
        let copy = classic.deepCopy()
        var previous: Float = 0
        for frame in 0..<1000 {
            let model = frame < 31 ? 4 : frame < 112 ? 1 : 0
            let a = classic.process(
                left: 0.2, right: -0.1, driveDB: 12, tone: 12000, mix: 1, outputDB: 0, matchLevel: false,
                character: model, bias: 0.8, emphasisDB: 8)
            let b = copy.process(
                left: 0.2, right: -0.1, driveDB: 12, tone: 12000, mix: 1, outputDB: 0, matchLevel: false,
                character: model, bias: 0.8, emphasisDB: 8)
            XCTAssertEqual(a.0, b.0)
            XCTAssertEqual(a.1, b.1)
            if frame > 20 {
                XCTAssertLessThan(
                    abs(a.0 - previous), 0.04, "Character changes crossfade shape, presence and DC removal")
            }
            previous = a.0
        }
    }

    func testNewEffectAutomationIsIndependentOfCallbackBlockSize() {
        for kind in [RealtimeEffectKind.limiter, .resonance, .warmth] {
            var values = kind.defaultParameters
            if kind == .warmth { values["character"] = 2 }
            let first = kind.parameters[0]
            let curve = [
                AutomationPoint(frame: 0, value: first.normalized(first.defaultValue)),
                AutomationPoint(frame: 7500, value: 0.85),
            ]
            let effect = RealtimeEffect(kind: kind, parameters: values, parameterAutomation: [first.id: curve])
            func render(blocks: [Int]) -> [Float] {
                let processor = EffectProcessor(effect: effect)
                var left = (0..<12_000).map { Float(sin(Double($0) * 0.117)) * 0.31 }
                var right = left.map { $0 * -0.7 }
                var position = 0
                var block = 0
                left.withUnsafeMutableBufferPointer { l in
                    right.withUnsafeMutableBufferPointer { r in
                        while position < l.count {
                            let size = min(blocks[block % blocks.count], l.count - position)
                            processor.process(
                                left: l.baseAddress! + position, right: r.baseAddress! + position, count: size,
                                position: position)
                            position += size
                            block += 1
                        }
                    }
                }
                return left + right
            }
            XCTAssertEqual(
                render(blocks: [12_000]), render(blocks: [1, 17, 127, 512, 31, 2048]),
                "\(kind) automation and latency must not change with callback partitioning")
        }
    }

    func testDisabledLimiterReturnsOriginalAudioDespiteInputGainAutomation() {
        let effect = RealtimeEffect(
            kind: .limiter, enabled: false, parameters: RealtimeEffectKind.limiter.defaultParameters,
            parameterAutomation: [
                "input": [AutomationPoint(frame: 0, value: 0), AutomationPoint(frame: 3000, value: 1)]
            ])
        let processor = EffectProcessor(effect: effect)
        let source = (0..<5000).map { Float(sin(Double($0) * 0.07)) * 0.23 }
        var left = source
        var right = source.map { -$0 }
        left.withUnsafeMutableBufferPointer { l in
            right.withUnsafeMutableBufferPointer { r in
                processor.process(left: l.baseAddress!, right: r.baseAddress!, count: l.count, position: 0)
            }
        }
        XCTAssertEqual(
            Array(left.prefix(LimiterDSP.latencyFrames)), [Float](repeating: 0, count: LimiterDSP.latencyFrames))
        XCTAssertEqual(
            Array(left.dropFirst(LimiterDSP.latencyFrames)),
            Array(source.prefix(source.count - LimiterDSP.latencyFrames)))
        XCTAssertEqual(
            Array(right.dropFirst(LimiterDSP.latencyFrames)),
            Array(source.prefix(source.count - LimiterDSP.latencyFrames)).map { -$0 })
    }

    private func amplitude(_ samples: [Float], _ frequency: Double) -> Double {
        var a: Double = 0
        var b: Double = 0
        for (i, x) in samples.enumerated() {
            let phase = 2 * .pi * frequency * Double(i) / sessionRate
            a += Double(x) * cos(phase)
            b += Double(x) * sin(phase)
        }
        return 2 * hypot(a, b) / Double(samples.count)
    }

    private func independentTruePeak(_ samples: [Float]) -> Double {
        // BS.1770-5 Annex 2 coefficients; independent offline convolution of
        // every phase, including filter flush. No TruePeakDSP or meter is used.
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
                for (tap, c) in phase.enumerated() where n >= tap && n - tap < samples.count {
                    value += c * Double(samples[n - tap])
                }
                peak = max(peak, abs(value))
            }
        }
        return peak
    }
}
