import XCTest
@testable import SoundroomCore

final class PitchEffectIntegrationTests: XCTestCase {
    private func source(count: Int = 72_000) -> AudioSamples {
        AudioSamples(channels: [(0..<count).map { i in
            let phase = 2 * Float.pi * 450 * Float(i) / 48_000
            return 0.32 * sin(phase) + 0.1 * sin(phase * 2)
        }])
    }
    private func hardTune() -> [String: Float] {
        RealtimeEffectKind.pitchCorrection.presets.first { $0.name == "Hard tune" }!.values
    }
    private func frequency(_ audio: AudioSamples) -> Float {
        let input = audio.channels[0]
        var crossings: [Double] = []
        for i in 36_001..<60_000 where input[i - 1] <= 0 && input[i] > 0 {
            crossings.append(Double(i - 1) - Double(input[i - 1] / (input[i] - input[i - 1])))
        }
        return Float(Double(crossings.count - 1) * 48_000 / (crossings.last! - crossings.first!))
    }
    func testDescriptorsPresetsAndProjectRoundTripPreserveDiscreteCustomTargets() throws {
        let kind = RealtimeEffectKind.pitchCorrection
        XCTAssertEqual(kind.parameters.map(\.id), ["key", "scale", "mask", "reference", "retune", "strength", "humanize", "tolerance", "vibrato", "formant", "mix", "manualTarget"])
        XCTAssertEqual(kind.parameters.first { $0.id == "mask" }!.clamped(123.6), 124)
        XCTAssertEqual(kind.parameters.first { $0.id == "mask" }!.clamped(8000), 4095)
        XCTAssertEqual(kind.parameters.first { $0.id == "scale" }!.physical(0.52), 2)
        let manual = kind.parameters.first { $0.id == "manualTarget" }!
        XCTAssertEqual(manual.defaultValue, -1)
        XCTAssertEqual(manual.clamped(.nan), -1)
        XCTAssertEqual(manual.clamped(.infinity), -1)
        XCTAssertEqual(manual.clamped(-900), -1)
        XCTAssertEqual(manual.clamped(900), 95)
        XCTAssertEqual(manual.clamped(69.6), 69.6)
        for preset in kind.presets {
            XCTAssertEqual(Set(preset.values.keys), Set(kind.parameters.map(\.id)))
            for parameter in kind.parameters { XCTAssertEqual(parameter.clamped(preset.values[parameter.id]!), preset.values[parameter.id]!) }
        }
        var values = hardTune(); values["scale"] = 3; values["mask"] = 145; values["formant"] = 2
        let original = RealtimeEffect(kind: kind, parameters: values, parameterAutomation: ["retune": [.init(frame: 900, value: 0.4, curve: .hold)]])
        XCTAssertEqual(try JSONDecoder().decode(RealtimeEffect.self, from: JSONEncoder().encode(original)), original)
        XCTAssertEqual(try JSONDecoder().decode(RealtimeEffectKind.self, from: JSONEncoder().encode(kind)), kind)
        XCTAssertEqual(BuiltInEffect.pitchCorrection.realtimeKind, kind)
    }
    func testSelectionCorrectionPreservesDurationAndDryFirstLastSamples() {
        let input = source()
        let tuned = BuiltInEffect.pitchCorrection.process(input, parameters: hardTune())
        XCTAssertEqual(tuned.count, input.count); XCTAssertEqual(tuned.channelCount, input.channelCount)
        XCTAssertEqual(frequency(tuned), 440, accuracy: 0.6)
        var dry = hardTune(); dry["mix"] = 0
        XCTAssertEqual(BuiltInEffect.pitchCorrection.process(input, parameters: dry), input)
        XCTAssertEqual(RealtimeEffect(kind: .pitchCorrection, enabled: false, parameters: hardTune()).process(input), input)
    }
    func testDownstreamAutomationAndMultiTrackRenderingStayOnSourceTimeline() {
        let input = source(count: 12_000), asset = UUID()
        var dry = hardTune(); dry["mix"] = 0
        let pitch = RealtimeEffect(kind: .pitchCorrection, parameters: dry)
        let gain = RealtimeEffect(kind: .gain, parameters: ["gain": 0], parameterAutomation: ["gain": [.init(frame: 0, value: 0.5), .init(frame: 11_999, value: 0.65)]])
        var session = Session(); session.tracks[0].clips = [Clip(assetID: asset, name: "Source", start: 0, length: input.count, edgeFadeFrames: 0)]
        session.tracks[0].realtimeEffects = [gain]
        let reference = Mixer.render(session: session, assets: [asset: input], range: 0..<input.count)
        for chain in [[pitch, gain], [gain, pitch], [pitch, gain, pitch]] {
            session.tracks[0].realtimeEffects = chain
            XCTAssertEqual(Mixer.render(session: session, assets: [asset: input], range: 0..<input.count), reference)
            XCTAssertEqual(Mixer.render(session: session, assets: [asset: input], range: 3811..<12_000), reference.slice(3811..<12_000))
        }
    }
    func testPitchSeekCheckpointCopiesFullAnalysisAndDelayedControls() {
        let input = source(count: 16_000), asset = UUID()
        var session = Session(); session.tracks[0].clips = [Clip(assetID: asset, name: "Voice", start: 0, length: input.count, edgeFadeFrames: 0)]
        session.tracks[0].realtimeEffects = [RealtimeEffect(kind: .pitchCorrection, parameters: hardTune())]
        let cache = DSPSeekCache(intervalFrames: 2000, maximumBytes: 12 * 1024 * 1024)
        _ = RealtimeMixer(session: session, assets: [asset: input], startFrame: 7000, seekCache: cache)
        let restored = RealtimeMixer(session: session, assets: [asset: input], startFrame: 7111, seekCache: cache)
        let reference = RealtimeMixer(session: session, assets: [asset: input], startFrame: 7111)
        XCTAssertNotNil(restored.restoredCheckpointFrame)
        var l = [Float](repeating: 0, count: 128), r = l, otherL = l, otherR = l
        l.withUnsafeMutableBufferPointer { lp in r.withUnsafeMutableBufferPointer { rp in restored.render(intoLeft: lp.baseAddress!, intoRight: rp.baseAddress!, frameCount: 128) } }
        otherL.withUnsafeMutableBufferPointer { lp in otherR.withUnsafeMutableBufferPointer { rp in reference.render(intoLeft: lp.baseAddress!, intoRight: rp.baseAddress!, frameCount: 128) } }
        XCTAssertEqual(l, otherL); XCTAssertEqual(r, otherR)
    }
    func testOldPitchParametersRemainAutomaticAndAbsoluteTargetRoundTrips() throws {
        let input = source()
        var explicitAutomatic = hardTune()
        var legacy = explicitAutomatic; legacy.removeValue(forKey: "manualTarget")
        explicitAutomatic["manualTarget"] = -1
        XCTAssertEqual(BuiltInEffect.pitchCorrection.process(input, parameters: legacy),
                       BuiltInEffect.pitchCorrection.process(input, parameters: explicitAutomatic))
        var manual = hardTune(); manual["manualTarget"] = 81
        let original = RealtimeEffect(kind: .pitchCorrection, parameters: manual)
        let restored = try JSONDecoder().decode(RealtimeEffect.self, from: JSONEncoder().encode(original))
        XCTAssertEqual(restored.parameters["manualTarget"], 81)
        XCTAssertEqual(frequency(BuiltInEffect.pitchCorrection.process(input, parameters: restored.parameters)), 880, accuracy: 1)
    }
}
