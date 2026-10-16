import XCTest
@testable import SoundroomCore

final class AdvancedEffectTests: XCTestCase {
    func testPhysicalParameterConversionAndIndependentCurvesRoundTrip() throws {
        let frequency = RealtimeEffectKind.lowPass.parameters[0]
        XCTAssertEqual(frequency.physical(frequency.normalized(1234)), 1234, accuracy: 0.001)
        XCTAssertEqual(frequency.clamped(.nan), frequency.defaultValue)
        var effect = RealtimeEffect(kind: .echo, parameters: RealtimeEffectKind.echo.defaultParameters,
            parameterAutomation: ["mix": [.init(frame: 10, value: 0), .init(frame: 20, value: 1)], "feedback": [.init(frame: 0, value: 0.8)]])
        effect.selectedAutomationParameter = "mix"
        XCTAssertEqual(effect.activeAutomationAmount, 0.25)
        XCTAssertEqual(effect.parameterValue("mix", at: 15), 0.5)
        XCTAssertEqual(effect.parameterValue("feedback", at: 15), 0.76, accuracy: 0.00001)
        effect.activeAutomation = [.init(frame: 100, value: 0.2)]
        XCTAssertEqual(effect.parameterAutomation["feedback"]?.count, 1)
        XCTAssertTrue(effect.automation.isEmpty)
        XCTAssertEqual(try JSONDecoder().decode(RealtimeEffect.self, from: JSONEncoder().encode(effect)), effect)
        let legacy = Data("{\"kind\":\"echo\",\"amount\":0.8,\"enabled\":true,\"automation\":[]}".utf8)
        let decoded = try JSONDecoder().decode(RealtimeEffect.self, from: legacy)
        XCTAssertTrue(decoded.parameters.isEmpty); XCTAssertTrue(decoded.parameterAutomation.isEmpty)
        XCTAssertEqual(decoded.value(at: 0), 0.8)
    }
    func testEchoTimeFeedbackMixAndTailAreAudibleAndSelectionMatchesRealtime() {
        let audio = AudioSamples(channels: [[1] + Array(repeating: 0, count: 99)])
        let params: [String: Float] = ["time": 1, "feedback": 0, "mix": 1, "damping": 20_000]
        let effect = RealtimeEffect(kind: .echo, parameters: params)
        let processed = effect.process(audio)
        XCTAssertEqual(processed.channels[0][0], 0)
        XCTAssertEqual(processed.channels[0][48], 1, accuracy: 0.00001)
        XCTAssertEqual(processed.channels[0][96], 0)
        XCTAssertEqual(BuiltInEffect.echo.process(audio, parameters: params), processed)
        let short = AudioSamples(channels: [[1]])
        XCTAssertEqual(effect.process(short).count, 1)
        XCTAssertEqual(effect.process(short, includeTail: true).count, 49)
        XCTAssertEqual(effect.process(short, includeTail: true).channels[0][48], 1, accuracy: 0.00001)
    }
    func testCompressorLinksStereoAndHonorsRatioMakeupAndMix() {
        let audio = AudioSamples(channels: [Array(repeating: 1, count: 4000), Array(repeating: 0.5, count: 4000)])
        var params = RealtimeEffectKind.compressor.defaultParameters
        params["threshold"] = -20; params["ratio"] = 4; params["attack"] = 0.1; params["knee"] = 0
        let compressed = BuiltInEffect.compressor.process(audio, parameters: params)
        XCTAssertEqual(compressed.channels[0].last!, pow(Float(10), -15 / 20), accuracy: 0.0001)
        XCTAssertEqual(compressed.channels[1].last!, compressed.channels[0].last! * 0.5, accuracy: 0.00001)
        params["makeup"] = 6
        let louder = BuiltInEffect.compressor.process(audio, parameters: params)
        XCTAssertEqual(louder.channels[0].last! / compressed.channels[0].last!, pow(Float(10), 6 / 20), accuracy: 0.00001)
        params["mix"] = 0
        XCTAssertEqual(BuiltInEffect.compressor.process(audio, parameters: params), audio)
    }
    func testFilterFrequencyResponseAndEqualizerNeutral() {
        let signal = (0..<4800).map { Float(sin(Double($0) * 2 * .pi * 8000 / sessionRate)) }
        let audio = AudioSamples(channels: [signal])
        let low = BuiltInEffect.lowPass.process(audio, parameters: ["frequency": 500, "resonance": 0.707])
        let high = BuiltInEffect.highPass.process(audio, parameters: ["frequency": 500, "resonance": 0.707])
        func rms(_ samples: [Float]) -> Float { sqrt(samples.suffix(4000).reduce(0) { $0 + $1 * $1 } / 4000) }
        XCTAssertLessThan(rms(low.channels[0]), 0.02)
        XCTAssertGreaterThan(rms(high.channels[0]), 0.65)
        let eq = BuiltInEffect.equalizer.process(audio, parameters: BuiltInEffect.equalizer.defaultParameters)
        for i in signal.indices { XCTAssertEqual(eq.channels[0][i], signal[i], accuracy: 0.00002) }
    }
    func testAdvancedEffectsAreIndependentOfRenderBlockBoundaries() {
        let count = 20_000, id = UUID()
        let samples = (0..<count).map { Float(sin(Double($0) * 0.039)) * 0.3 }
        var session = Session()
        session.tracks[0].clips = [.init(assetID: id, name: "Signal", start: 0, length: count, edgeFadeFrames: 0)]
        session.tracks[0].realtimeEffects = RealtimeEffectKind.allCases.map { kind in
            var effect = RealtimeEffect(kind: kind, parameters: kind.defaultParameters)
            let parameter = kind.parameters[0]
            effect.parameterAutomation[parameter.id] = [.init(frame: 0, value: 0.35), .init(frame: count - 1, value: 0.65)]
            return effect
        }
        let assets = [id: AudioSamples(channels: [samples])]
        let expected = Mixer.render(session: session, assets: assets, range: 0..<count)
        let mixer = RealtimeMixer(session: session, assets: assets)
        var left: [Float] = [], right: [Float] = []
        for size in [1, 17, 513, 8192, 11_277] {
            var l = Array(repeating: Float(0), count: size), r = l
            l.withUnsafeMutableBufferPointer { lp in r.withUnsafeMutableBufferPointer { rp in mixer.render(intoLeft: lp.baseAddress!, intoRight: rp.baseAddress!, frameCount: size) } }
            left += l; right += r
        }
        XCTAssertEqual(expected, AudioSamples(channels: [left, right]))
        XCTAssertTrue(left.allSatisfy(\.isFinite))
    }
    func testRoomTailAndMixerTailOptInPreserveDrySessionLength() {
        let id = UUID(), audio = AudioSamples(channels: [[1]])
        let effect = RealtimeEffect(kind: .room, parameters: ["decay": 0.3, "predelay": 0, "damping": 6000, "mix": 1])
        let room = effect.process(audio, includeTail: true)
        XCTAssertGreaterThan(room.count, audio.count)
        XCTAssertTrue(room.channels[0].dropFirst(1000).contains { abs($0) > 0.01 })
        var session = Session(); session.tracks[0].clips = [.init(assetID: id, name: "Impulse", start: 0, length: 1, edgeFadeFrames: 0)]
        session.tracks[0].realtimeEffects = [effect]
        XCTAssertEqual(RealtimeMixer(session: session, assets: [id: audio]).end, 1)
        XCTAssertEqual(RealtimeMixer(session: session, assets: [id: audio], includeEffectTails: true).end, room.count)
        XCTAssertLessThanOrEqual(RealtimeMixer.tailFrames(session: session), Int(30 * sessionRate))
    }
    func testHostileParametersAreClampedAndFinite() {
        let audio = AudioSamples(channels: [[.nan, .infinity, -.infinity, 1] + Array(repeating: 0.1, count: 500)])
        for kind in RealtimeEffectKind.allCases {
            let parameters = Dictionary(uniqueKeysWithValues: kind.parameters.map { ($0.id, Float.nan) })
            let result = RealtimeEffect(kind: kind, parameters: parameters).process(audio)
            XCTAssertTrue(result.channels.flatMap { $0 }.allSatisfy(\.isFinite), kind.name)
        }
    }
    func testSeekingAdvancedWarmthWarmsItsToneFilterHistory() {
        let id = UUID(), samples = (0..<20_000).map { Float(sin(Double($0) * 0.035)) * 0.4 }
        var session = Session(); session.tracks[0].clips = [.init(assetID: id, name: "Signal", start: 0, length: samples.count, edgeFadeFrames: 0)]
        session.tracks[0].realtimeEffects = [.init(kind: .warmth, parameters: ["drive": 12, "tone": 800, "mix": 1, "output": -3])]
        let assets = [id: AudioSamples(channels: [samples])]
        let complete = Mixer.render(session: session, assets: assets, range: 0..<samples.count)
        XCTAssertEqual(Mixer.render(session: session, assets: assets, range: 8000..<9500), complete.slice(8000..<9500))
    }

    func testExplicitLegacyUpgradePreservesGainAndConvertsItsCurve() {
        var effect = RealtimeEffect(kind: .gain, amount: 0.75, automation: [.init(frame: 10, value: 0.25), .init(frame: 20, value: 1)])
        XCTAssertEqual(effect.parametersForEditing["gain"], 12)
        effect.enableParameterControls(); effect.selectedAutomationParameter = "gain"
        XCTAssertEqual(effect.parameterValue("gain", at: 0), 12, accuracy: 0.00001)
        XCTAssertEqual(effect.parameterValue("gain", at: 10), -12, accuracy: 0.00001)
        XCTAssertEqual(effect.parameterValue("gain", at: 15), 6, accuracy: 0.00001)
        XCTAssertEqual(effect.parameterValue("gain", at: 20), 24, accuracy: 0.00001)
        let upgraded = effect; effect.enableParameterControls(); XCTAssertEqual(effect, upgraded)
    }

}
