import XCTest

@testable import SoundroomCore

final class ChannelEqualizerTests: XCTestCase {
    func testShelvesHaveIndependentEndGainsAndCutsAreButterworth() {
        let low: [String: Float] = ["lowShape": 1, "lowGain": 6, "lowFrequency": 120]
        XCTAssertEqual(EQResponse.decibels(at: 0, parameters: low), 6, accuracy: 1e-7)
        XCTAssertEqual(EQResponse.decibels(at: 24_000, parameters: low), 0, accuracy: 1e-7)
        let high: [String: Float] = ["highShape": 1, "highGain": -9, "highFrequency": 9000]
        XCTAssertEqual(EQResponse.decibels(at: 0, parameters: high), 0, accuracy: 1e-7)
        XCTAssertEqual(EQResponse.decibels(at: 24_000, parameters: high), -9, accuracy: 1e-7)
        for (enabled, frequency) in [("highPassEnabled", "highPassFrequency"), ("lowPassEnabled", "lowPassFrequency")] {
            let cutoff: Float = enabled == "highPassEnabled" ? 80 : 8000
            XCTAssertEqual(
                EQResponse.decibels(at: Double(cutoff), parameters: [enabled: 1, frequency: cutoff]), -3.0103,
                accuracy: 0.00001)
            XCTAssertEqual(EQResponse.decibels(at: Double(cutoff), parameters: [frequency: cutoff]), 0, accuracy: 1e-8)
        }
    }
    func testMeasuredAudioMatchesCurveForShelvesCutsAndExtremeLowBand() {
        let settings: [[String: Float]] = [
            RealtimeEffectKind.equalizer.presets.first { $0.name == "Air & weight" }!.values,
            RealtimeEffectKind.equalizer.presets.first { $0.name == "Dark guitar" }!.values,
            [
                "lowShape": 1, "lowFrequency": 20, "lowGain": 18, "lowQ": 8, "highShape": 1, "highFrequency": 18_000,
                "highGain": -12, "output": -3,
            ],
        ]
        for parameters in settings {
            for frequency in [40.0, 100, 1000, 8000, 16_000] {
                let input = (0..<96_000).map { Float(sin(2 * .pi * frequency * Double($0) / sessionRate)) * 0.01 }
                let output = RealtimeEffect(kind: .equalizer, parameters: parameters).process(
                    AudioSamples(channels: [input])
                ).channels[0]
                XCTAssertTrue(output.allSatisfy(\.isFinite))
                let measured =
                    20
                    * log10(
                        amplitude(Array(output.suffix(48_000)), frequency)
                            / amplitude(Array(input.suffix(48_000)), frequency))
                XCTAssertEqual(
                    measured, EQResponse.decibels(at: frequency, parameters: parameters), accuracy: 0.04,
                    "\(frequency) Hz: \(parameters)")
            }
        }
    }
    func testOldSixParameterSettingsRetainTheirThreeBellResponse() {
        let parameters: [String: Float] = [
            "lowGain": 7, "midFrequency": 1600, "midGain": -6, "midQ": 1.7, "highGain": 3, "output": -2,
        ]
        let input = (0..<24_000).map { Float(sin(Double($0) * 0.041) + sin(Double($0) * 0.31)) * 0.1 }
        var filters = [Biquad(), Biquad(), Biquad()]
        filters[0].configure(frequency: 100, q: 0.707, gain: 7, mode: .equalizer)
        filters[1].configure(frequency: 1600, q: 1.7, gain: -6, mode: .equalizer)
        filters[2].configure(frequency: 8000, q: 0.707, gain: 3, mode: .equalizer)
        let output = RealtimeEffect(kind: .equalizer, parameters: parameters).process(AudioSamples(channels: [input]))
            .channels[0]
        var largest: Float = 0
        for i in input.indices {
            var expected = input[i]
            for j in filters.indices { expected = filters[j].filter(expected) }
            largest = max(largest, abs(output[i] - expected * pow(10, -2.0 / 20)))
        }
        XCTAssertLessThan(largest, 0.0001, "Only double-precision rounding may differ from the original coefficients")
    }
    func testShapeAndCutChangesCrossfadeWarmFiltersAndCopyIsExact() {
        let state = EqualizerDSP()
        var values = RealtimeEffectKind.equalizer.parameters.map(\.defaultValue)
        values[0] = 6
        state.configure(values)
        for _ in 0..<48_000 { _ = state.process(left: 0.2, right: -0.1) }
        values[8] = 1
        values[13] = 1
        state.configure(values)
        let copy = state.deepCopy()
        var previous: Float = 0.2
        var largest: Float = 0
        for _ in 0..<2000 {
            let a = state.process(left: 0.2, right: -0.1)
            let b = copy.process(left: 0.2, right: -0.1)
            largest = max(largest, abs(a.0 - previous))
            previous = a.0
            XCTAssertEqual(a.0, b.0)
            XCTAssertEqual(a.1, b.1)
        }
        XCTAssertLessThan(largest, 0.003)
        XCTAssertEqual(previous, 0, accuracy: 0.000001)
    }
    func testExpandedAutomationAndCheckpointRenderingRemainSampleExact() {
        let id = UUID()
        let count = 24_000
        var session = Session()
        session.tracks[0].clips = [Clip(assetID: id, name: "EQ", start: 0, length: count, edgeFadeFrames: 0)]
        var effect = RealtimeEffect(kind: .equalizer, parameters: RealtimeEffectKind.equalizer.presets[3].values)
        effect.parameterAutomation = [
            "lowFrequency": [.init(frame: 0, value: 0.1), .init(frame: count, value: 0.9)],
            "highShape": [.init(frame: 0, value: 0), .init(frame: 12_000, value: 1)],
            "lowPassEnabled": [.init(frame: 0, value: 0), .init(frame: count, value: 1)],
        ]
        session.tracks[0].realtimeEffects = [effect]
        let assets = [id: AudioSamples(channels: [(0..<count).map { Float(sin(Double($0) * 0.037)) * 0.1 }])]
        let cache = DSPSeekCache(intervalFrames: 2048)
        _ = RealtimeMixer(session: session, assets: assets, startFrame: 20_000, seekCache: cache)
        let mixer = RealtimeMixer(session: session, assets: assets, startFrame: 9000, seekCache: cache)
        XCTAssertEqual(mixer.restoredCheckpointFrame, 8192)
        var left = [Float](repeating: 0, count: 6000)
        var right = left
        left.withUnsafeMutableBufferPointer { l in
            right.withUnsafeMutableBufferPointer { r in
                mixer.render(intoLeft: l.baseAddress!, intoRight: r.baseAddress!, frameCount: 17)
                mixer.render(intoLeft: l.baseAddress! + 17, intoRight: r.baseAddress! + 17, frameCount: l.count - 17)
            }
        }
        XCTAssertEqual(
            AudioSamples(channels: [left, right]), Mixer.render(session: session, assets: assets, range: 9000..<15_000))
    }
    private func amplitude(_ samples: [Float], _ frequency: Double) -> Double {
        var real = 0.0
        var imaginary = 0.0
        for (i, sample) in samples.enumerated() {
            let phase = 2 * .pi * frequency * Double(i) / sessionRate
            real += Double(sample) * cos(phase)
            imaginary += Double(sample) * sin(phase)
        }
        return 2 * hypot(real, imaginary) / Double(samples.count)
    }
}
