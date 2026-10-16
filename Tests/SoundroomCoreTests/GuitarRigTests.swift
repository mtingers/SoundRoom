import XCTest
@testable import SoundroomCore

final class GuitarRigTests: XCTestCase {
    private var referenceURL: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("portable/crates/engine/tests/fixtures/guitar-rig-reference.json")
    }
    private func signal(frames: Int) -> AudioSamples {
        let left = (0..<frames).map { i -> Float in
            let t = Double(i) / 48000
            return Float(0.17 * sin(t * 2 * .pi * 220) + 0.03 * sin(t * 2 * .pi * 1760))
        }
        let right = (0..<frames).map { Float(0.13 * sin(Double($0) / 48000 * 2 * .pi * 330)) }
        return AudioSamples(channels: [left, right])
    }
    func testFactoryRigsMatchPortableEngineReference() throws {
        let data = try Data(contentsOf: referenceURL)
        let reference = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        let presets = try XCTUnwrap(reference["presets"] as? [[String: Any]])
        XCTAssertEqual(presets.count, GuitarRigDesign.presets.count)
        let frames = try XCTUnwrap(reference["frames"] as? Int), stride = try XCTUnwrap(reference["stride"] as? Int)
        let input = signal(frames: frames)
        for fixture in presets {
            let name = try XCTUnwrap(fixture["name"] as? String)
            let preset = try XCTUnwrap(GuitarRigDesign.presets.first { $0.name == name })
            let output = RealtimeEffect(kind: .guitarRig, parameters: preset.values).process(input)
            let samples = try XCTUnwrap(fixture["samples"] as? [[NSNumber]])
            XCTAssertEqual(output.count, frames)
            for (bin, sample) in samples.enumerated() {
                for channel in 0..<2 {
                    XCTAssertEqual(output.sample(channel: channel, frame: bin * stride), sample[channel].floatValue, accuracy: 0.0005, "\(name), channel \(channel), sample \(bin * stride)")
                }
            }
        }
    }
    func testEmptyRigAndDryParallelMixPreserveStereoSamples() throws {
        let input = signal(frames: 1024)
        let empty = try XCTUnwrap(GuitarRigDesign.presets.first { $0.name.hasPrefix("Empty rig") })
        let effect = RealtimeEffect(kind: .guitarRig, parameters: empty.values)
        XCTAssertEqual(effect.latencyFrames, 32)
        XCTAssertEqual(effect.tailFrames, 0)
        let direct = effect.process(input)
        for channel in 0..<2 { for i in 0..<input.count { XCTAssertEqual(direct.sample(channel: channel, frame: i), input.sample(channel: channel, frame: i), accuracy: 0.000001) } }
        var dry = RealtimeEffect(kind: .guitarRig); dry.parameters["mix"] = 0
        XCTAssertEqual(dry.process(input).channels, input.channels)
        XCTAssertEqual(BuiltInEffect.guitarRig.realtimeKind, .guitarRig)
        let applied = BuiltInEffect.guitarRig.process(input, parameters: empty.values)
        XCTAssertEqual(applied.channels, direct.channels)
    }
    func testCheckpointCopiesKeepIndependentWetHistory() {
        let original = GuitarRigDSP()
        var values = RealtimeEffectKind.guitarRig.parameters.map { $0.defaultValue }
        values[18] = 1; values[23] = 20; values[25] = 0.4; values[27] = 0.3
        original.configure(values)
        let source = signal(frames: 16000)
        for i in 0..<12000 { _ = original.process(left: source.sample(channel: 0, frame: i), right: source.sample(channel: 1, frame: i)) }
        let copy = original.deepCopy()
        for i in 12000..<15000 {
            let a = original.process(left: source.sample(channel: 0, frame: i), right: source.sample(channel: 1, frame: i))
            let b = copy.process(left: source.sample(channel: 0, frame: i), right: source.sample(channel: 1, frame: i))
            XCTAssertEqual(a.0, b.0); XCTAssertEqual(a.1, b.1)
        }
        XCTAssertLessThan(original.storageBytes, 1024 * 1024)
    }
    func testMonoDoesNotInventRightChannelWithoutSpatialEffects() {
        var values = RealtimeEffectKind.guitarRig.defaultParameters
        values["pedal"] = 3; values["amp"] = 4; values["cabinet"] = 4; values["reverbMix"] = 0
        let input = signal(frames: 3000)
        let mono = AudioSamples(channels: [input.channels[0], Array(repeating: 0, count: input.count)])
        let output = RealtimeEffect(kind: .guitarRig, parameters: values).process(mono)
        XCTAssertTrue(output.channels[1].allSatisfy { $0 == 0 })
        XCTAssertGreaterThan(output.channels[0].map(abs).max() ?? 0, 0.01)
    }
    func testApplyingRigToMonoDIPreservesItsStereoChorus() {
        let samples = signal(frames: 4096).channels[0]
        var values = RealtimeEffectKind.guitarRig.defaultParameters
        values["modulation"] = 1; values["reverbMix"] = 0
        let effect = RealtimeEffect(kind: .guitarRig, parameters: values)
        let mono = effect.process(AudioSamples(channels: [samples]))
        let stereo = effect.process(AudioSamples(channels: [samples, samples]))
        XCTAssertEqual(mono.channelCount, 2)
        XCTAssertEqual(mono.channels, stereo.channels)
        XCTAssertTrue(zip(mono.channels[0], mono.channels[1]).contains { abs($0 - $1) > 0.001 })
    }
    func testGuitarRigRequiresVersionElevenIncludingMasterAndPresetRoundtrip() throws {
        var session = Session()
        session.masterTrack.realtimeEffects = [RealtimeEffect(kind: .guitarRig)]
        XCTAssertEqual(session.requiredProjectVersion, 11)
        let effect = RealtimeEffect(kind: .guitarRig, parameters: GuitarRigDesign.presets[5].values)
        let decoded = try JSONDecoder().decode(RealtimeEffect.self, from: JSONEncoder().encode(effect))
        XCTAssertEqual(decoded, effect)
        XCTAssertEqual(RealtimeEffectKind.guitarRig.parameters.count, 30)
        XCTAssertTrue(GuitarRigDesign.presets.allSatisfy { $0.values.count == 30 })
    }
    func testTailBudgetIncludesCascadedDelayAndRoomAndParameterAutomation() {
        var effect = RealtimeEffect(kind: .guitarRig)
        effect.parameters["delayMix"] = 0; effect.parameters["delayTime"] = 1200; effect.parameters["delayFeedback"] = 0.7
        effect.parameterAutomation["delayMix"] = [.init(frame: 48000, value: 0.5)]
        effect.parameters["reverbMix"] = 0.3; effect.parameters["reverbDecay"] = 5
        XCTAssertGreaterThan(effect.tailFrames, 20 * 48000)
        XCTAssertLessThanOrEqual(effect.tailFrames, 30 * 48000)
        effect.enabled = false; XCTAssertEqual(effect.tailFrames, 0)
    }
    func testDistortionRejectsTheFoldedThirdHarmonic() {
        let frames = 24000
        let samples = (0..<frames).map { Float(0.7 * sin(2 * Double.pi * 9000 * Double($0) / 48000)) }
        var values = RealtimeEffectKind.guitarRig.defaultParameters
        values["gate"] = -90; values["amp"] = 3; values["ampDrive"] = 24; values["cabinet"] = 0
        values["master"] = 0; values["sag"] = 0; values["reverbMix"] = 0
        let output = RealtimeEffect(kind: .guitarRig, parameters: values).process(AudioSamples(channels: [samples])).channels[0]
        let naive = samples.map { tanh($0 * pow(Float(10), 25.2 / 20)) }
        func amplitude(_ samples: [Float], _ hz: Double) -> Double {
            var real: Double = 0, imaginary: Double = 0
            for i in 4000..<samples.count {
                let phase = 2 * Double.pi * hz * Double(i) / 48000
                real += Double(samples[i]) * cos(phase); imaginary -= Double(samples[i]) * sin(phase)
            }
            return hypot(real, imaginary)
        }
        let actual = amplitude(output, 21000) / max(0.000001, amplitude(output, 9000))
        let aliased = amplitude(naive, 21000) / max(0.000001, amplitude(naive, 9000))
        print("Guitar rig alias/fundamental: \(20 * log10(actual)) dB, naive \(20 * log10(aliased)) dB")
        XCTAssertLessThan(actual, aliased * 0.12)
    }
    func testMaximumControlsAndRepeatedModelEditsStayFinite() {
        let state = GuitarRigDSP()
        var values = RealtimeEffectKind.guitarRig.parameters.map { $0.maximum }
        state.configure(values)
        for i in 0..<12000 {
            if i.isMultiple(of: 257) {
                values[2] = Float((i / 257) % 4); values[6] = Float((i / 257) % 5); values[14] = Float((i / 257 + 2) % 5)
                state.configure(values)
            }
            let input: Float = i == 0 ? .nan : sin(Float(i) * 0.04)
            let out = state.process(left: input, right: -input)
            XCTAssertTrue(out.0.isFinite && out.1.isFinite)
        }
    }
}
