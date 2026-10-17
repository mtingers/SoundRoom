import XCTest

@testable import SoundroomCore

final class EffectQualityTests: XCTestCase {
    func testOversamplingSuppressesFoldedHarmonic() {
        let state = WarmthDSP()
        let count = 48_000
        let frequency: Float = 9000
        var clean = [Float]()
        var naive = [Float]()
        for i in 0..<count {
            let sample: Float = 0.7 * sin(2 * .pi * frequency * Float(i) / Float(sessionRate))
            let output = state.process(
                left: sample, right: sample, driveDB: 24, tone: 20_000, mix: 1, outputDB: 0, matchLevel: false)
            if i >= 4000 {
                clean.append(output.0)
                naive.append(tanh(sample * pow(10, 24.0 / 20)))
            }
        }
        let cleanAlias = amplitude(clean, frequency: 21_000) / amplitude(clean, frequency: 9000)
        let naiveAlias = amplitude(naive, frequency: 21_000) / amplitude(naive, frequency: 9000)
        print(
            String(
                format: "Warmth alias/fundamental: oversampled %.1f dB, naive %.1f dB", 20 * log10(cleanAlias),
                20 * log10(naiveAlias)))
        XCTAssertLessThan(
            cleanAlias, naiveAlias * 0.12, "Oversampling must reject the 27k third harmonic that folds to 21k")
    }
    func testDryAndWetLatencyAndCopyAreExact() {
        let state = WarmthDSP()
        var impulse = [Float]()
        for i in 0..<100 {
            let output = state.process(
                left: i == 0 ? 0.01 : 0, right: 0, driveDB: 0, tone: 20_000, mix: 0, outputDB: 0, matchLevel: false)
            impulse.append(output.0)
        }
        XCTAssertEqual(impulse[WarmthDSP.latencyFrames], 0.01)
        XCTAssertEqual(impulse.enumerated().filter { $0.element != 0 }.map(\.offset), [16])
        for i in 0..<137 {
            _ = state.process(
                left: Float(sin(Double(i) * 0.11)) * 0.5, right: 0.2, driveDB: 12, tone: 7000, mix: 0.6, outputDB: -4,
                matchLevel: true)
        }
        let clone = state.deepCopy()
        for i in 0..<1000 {
            let sample = Float(sin(Double(i) * 0.13)) * 0.4
            let a = state.process(
                left: sample, right: -sample, driveDB: 12, tone: 7000, mix: 0.6, outputDB: -4, matchLevel: true)
            let b = clone.process(
                left: sample, right: -sample, driveDB: 12, tone: 7000, mix: 0.6, outputDB: -4, matchLevel: true)
            XCTAssertEqual(a.0, b.0)
            XCTAssertEqual(a.1, b.1)
        }
        XCTAssertGreaterThan(state.storageBytes, 700)
    }
    func testOutputControlsFollowDelayedSignal() {
        let state = WarmthDSP()
        for i in 0..<64 {
            let output = state.process(
                left: 0.1, right: 0.1, driveDB: 0, tone: 20_000, mix: 0, outputDB: i < 24 ? 0 : -6, matchLevel: false)
            if (16..<40).contains(i) { XCTAssertEqual(output.0, 0.1, accuracy: 0.000001) }
            if i >= 40 { XCTAssertEqual(output.0, 0.1 * pow(10, -6.0 / 20), accuracy: 0.000001) }
        }
    }
    func testLevelMatchConvergesWithoutChangingOutputTrim() {
        let state = WarmthDSP()
        var inputPower: Double = 0
        var outputPower: Double = 0
        for i in 0..<144_000 {
            let sample = Float(sin(2 * .pi * 440 * Double(i) / sessionRate)) * 0.18
            let output = state.process(
                left: sample, right: sample, driveDB: 18, tone: 12_000, mix: 1, outputDB: -3, matchLevel: true
            ).0
            if i > 96_000 {
                inputPower += Double(sample * sample)
                outputPower += Double(output * output)
            }
            XCTAssertTrue(output.isFinite)
        }
        XCTAssertEqual(10 * log10(outputPower / inputPower), -3, accuracy: 0.25)
    }
    func testAnalyticEQMatchesMeasuredAudio() {
        // Read actual descriptor ids rather than letting an incorrect fixture silently use defaults.
        let ids = RealtimeEffectKind.equalizer.parameters.map(\.id)
        let actual = Dictionary(uniqueKeysWithValues: zip(ids, [Float(7), 1500, -8, 1.7, 4, -2]))
        for frequency in [100.0, 1500, 8000] {
            let samples = (0..<48_000).map { Float(sin(2 * .pi * frequency * Double($0) / sessionRate)) * 0.05 }
            let output = RealtimeEffect(kind: .equalizer, parameters: actual).process(AudioSamples(channels: [samples]))
                .channels[0]
            let measured =
                20
                * log10(
                    amplitude(Array(output.dropFirst(4000)), frequency: frequency)
                        / amplitude(Array(samples.dropFirst(4000)), frequency: frequency))
            XCTAssertEqual(measured, EQResponse.decibels(at: frequency, parameters: actual), accuracy: 0.03)
        }
    }
    func testCompressorMeterMeasuresReductionAndMakeupSeparately() {
        var parameters = RealtimeEffectKind.compressor.defaultParameters
        parameters["threshold"] = -18
        parameters["ratio"] = 4
        parameters["attack"] = 0.1
        parameters["knee"] = 0
        parameters["makeup"] = 6
        let processor = EffectProcessor(effect: RealtimeEffect(kind: .compressor, parameters: parameters))
        var left = [Float](repeating: 0.5, count: 4800)
        var right = left
        left.withUnsafeMutableBufferPointer { l in
            right.withUnsafeMutableBufferPointer { r in
                processor.process(left: l.baseAddress!, right: r.baseAddress!, count: l.count, position: 0)
            }
        }
        let reading = processor.meter.snapshot()
        XCTAssertEqual(reading.inputPeak, 0.5)
        XCTAssertEqual(reading.inputRMS, 0.5)
        XCTAssertEqual(reading.gainReductionDB, (18 - 6.0206) * 0.75, accuracy: 0.01)
        XCTAssertGreaterThan(reading.outputPeak, reading.outputRMS)
        XCTAssertEqual(reading.frameCount, 4800)
    }
    func testWarmthBypassRetainsDryAlignmentAndLegacyIsUnchanged() {
        let processor = EffectProcessor(
            effect: RealtimeEffect(
                kind: .warmth, enabled: false, parameters: RealtimeEffectKind.warmth.defaultParameters))
        var left = [Float](repeating: 0, count: 64)
        var right = left
        left[0] = 0.3
        left.withUnsafeMutableBufferPointer { l in
            right.withUnsafeMutableBufferPointer { r in
                processor.process(left: l.baseAddress!, right: r.baseAddress!, count: l.count, position: 0)
            }
        }
        XCTAssertEqual(left[16], 0.3)
        XCTAssertEqual(left[0], 0)
        let legacy = RealtimeEffect(kind: .warmth, amount: 0.4).process(AudioSamples(channels: [[0.3, -0.2]]))
        XCTAssertEqual(legacy.channels[0][0], tanh(0.3 * 4.2) / tanh(4.2), accuracy: 0.000001)
    }
    func testWarmthCheckpointRestoresFIRAndLevelMatchHistory() {
        var session = Session()
        let id = UUID()
        session.tracks[0].clips = [Clip(assetID: id, name: "Warmth", start: 0, length: 12_000, edgeFadeFrames: 0)]
        var parameters = RealtimeEffectKind.warmth.defaultParameters
        parameters["match"] = 1
        session.tracks[0].realtimeEffects = [RealtimeEffect(kind: .warmth, parameters: parameters)]
        let samples = (0..<12_000).map { Float(sin(Double($0) * 0.037)) * 0.2 }
        let assets = [id: AudioSamples(channels: [samples])]
        let cache = DSPSeekCache(intervalFrames: 2048)
        _ = RealtimeMixer(session: session, assets: assets, startFrame: 10_000, seekCache: cache)
        let cached = RealtimeMixer(session: session, assets: assets, startFrame: 7000, seekCache: cache)
        let uncached = RealtimeMixer(session: session, assets: assets, startFrame: 7000)
        XCTAssertEqual(cached.restoredCheckpointFrame, 6144)
        func render(_ mixer: RealtimeMixer) -> [Float] {
            var left = [Float](repeating: 0, count: 1024)
            var right = left
            left.withUnsafeMutableBufferPointer { l in
                right.withUnsafeMutableBufferPointer { r in
                    mixer.render(intoLeft: l.baseAddress!, intoRight: r.baseAddress!, frameCount: l.count)
                }
            }
            return left
        }
        XCTAssertEqual(render(cached), render(uncached))
        let again = RealtimeMixer(session: session, assets: assets, startFrame: 7000, seekCache: cache)
        let reference = RealtimeMixer(session: session, assets: assets, startFrame: 7000)
        XCTAssertEqual(render(again), render(reference))
    }
    private func amplitude(_ samples: [Float], frequency: Double) -> Double {
        var real: Double = 0
        var imaginary: Double = 0
        for (i, sample) in samples.enumerated() {
            let phase = 2 * .pi * frequency * Double(i) / sessionRate
            real += Double(sample) * cos(phase)
            imaginary -= Double(sample) * sin(phase)
        }
        return 2 * hypot(real, imaginary) / Double(samples.count)
    }
}
