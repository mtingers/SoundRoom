import XCTest

@testable import SoundroomCore

final class StereoRoomTests: XCTestCase {
    private func impulse(_ overrides: [String: Float] = [:], frames: Int = 96_000) -> AudioSamples {
        var parameters = RealtimeEffectKind.room.defaultParameters
        parameters.merge(["predelay": 0, "mix": 1, "lowCut": 0]) { _, new in new }
        parameters.merge(overrides) { _, new in new }
        return RealtimeEffect(kind: .room, parameters: parameters).process(
            AudioSamples(channels: [
                [1] + [Float](repeating: 0, count: frames - 1), [Float](repeating: 0, count: frames),
            ]))
    }
    func testDenseTailBuildsDensityAndDecorrelatesHardPannedInput() {
        let classic = impulse(["character": 0])
        let dense = impulse()
        let range = 4800..<14400
        func density(_ audio: AudioSamples) -> Double {
            Double(audio.channels[0][range].filter { abs($0) > 0.00001 }.count) / Double(range.count)
        }
        XCTAssertEqual(classic.channels[1].reduce(0) { $0 + abs($1) }, 0)
        XCTAssertGreaterThan(density(dense), density(classic) + 0.2)
        XCTAssertGreaterThan(density(dense), 0.9)
        let l = dense.channels[0][range]
        let r = dense.channels[1][range]
        let cross = zip(l, r).reduce(0.0) { $0 + Double($1.0 * $1.1) }
        let energyL = l.reduce(0.0) { $0 + Double($1 * $1) }
        let energyR = r.reduce(0.0) { $0 + Double($1 * $1) }
        let correlation = cross / sqrt(energyL * energyR)
        XCTAssertGreaterThan(energyR, 0.001)
        XCTAssertLessThan(abs(correlation), 0.6)
        print("REVERB_DENSITY classic=\(density(classic)) dense=\(density(dense)) stereoCorrelation=\(correlation)")
    }
    func testWidthLowCutDecayAndEarlyBalanceHaveMeasurableEffects() {
        let mono = impulse(["width": 0])
        XCTAssertEqual(mono.channels[0], mono.channels[1])
        let early = impulse(["early": 1])
        XCTAssertTrue(early.channels[0].suffix(80_000).allSatisfy { $0 == 0 })
        let short = impulse(["decay": 0.3, "early": 0])
        let long = impulse(["decay": 3, "early": 0])
        func energy(_ audio: AudioSamples, _ range: Range<Int>) -> Double {
            audio.channels[0][range].reduce(0) { $0 + Double($1 * $1) }
        }
        XCTAssertLessThan(energy(short, 24_000..<48_000), energy(long, 24_000..<48_000) * 0.001)
        let frames = 48_000
        let source = (0..<frames).map { Float(sin(2 * .pi * 40 * Double($0) / sessionRate)) * 0.1 }
        let audio = AudioSamples(channels: [source, source])
        var values = RealtimeEffectKind.room.defaultParameters
        values["mix"] = 1
        values["early"] = 1
        values["lowCut"] = 0
        let uncut = RealtimeEffect(kind: .room, parameters: values).process(audio)
        values["lowCut"] = 400
        let cut = RealtimeEffect(kind: .room, parameters: values).process(audio)
        XCTAssertLessThan(energy(cut, 24_000..<48_000), energy(uncut, 24_000..<48_000) * 0.02)
    }
    func testOriginalFourParameterSettingsAndOlderEmptyProjectsStayClassic() throws {
        let parameters: [String: Float] = ["decay": 1.4, "predelay": 0, "damping": 6000, "mix": 0.2]
        let source = (0..<24_000).map { Float(sin(Double($0) * 0.043)) * 0.1 }
        let effect = RealtimeEffect(kind: .room, parameters: parameters)
        let output = effect.process(AudioSamples(channels: [source])).channels[0]
        var reference = OriginalRoomReference()
        for i in source.indices {
            XCTAssertEqual(output[i], source[i] * 0.8 + reference.process(source[i]) * 0.2, accuracy: 0.000001)
        }
        let decoded = try JSONDecoder().decode(RealtimeEffect.self, from: Data("{\"kind\":\"room\"}".utf8))
        XCTAssertEqual(decoded.parameterValue("character", at: 0), 0)
        XCTAssertEqual(RealtimeEffect(kind: .room).parameterValue("character", at: 0), 1)
        XCTAssertEqual(try JSONDecoder().decode(RealtimeEffect.self, from: JSONEncoder().encode(effect)), effect)
    }
    func testPredelayIncludingFractionalSampleNeverReadsAnOldWriteSlot() {
        let zero = impulse(["early": 1], frames: 6000)
        let delayed = impulse(["predelay": 10, "early": 1], frames: 6000)
        for i in 480..<6000 { XCTAssertEqual(delayed.channels[0][i], zero.channels[0][i - 480], accuracy: 0.000001) }
        let fractional = impulse(["predelay": 1000 / 96_000, "early": 1], frames: 6000)
        for i in 1..<6000 {
            XCTAssertEqual(
                fractional.channels[0][i], (zero.channels[0][i] + zero.channels[0][i - 1]) * 0.5, accuracy: 0.000001)
        }
    }
    func testAutomationBlockSplitsAndCheckpointHistoryAreSampleExact() {
        let id = UUID()
        let count = 30_000
        var session = Session()
        session.tracks[0].clips = [Clip(assetID: id, name: "Reverb", start: 0, length: count, edgeFadeFrames: 0)]
        var effect = RealtimeEffect(kind: .room)
        for key in ["decay", "diffusion", "width", "lowCut", "early", "character"] {
            effect.parameterAutomation[key] = [.init(frame: 0, value: 0.2), .init(frame: count, value: 0.8)]
        }
        session.tracks[0].realtimeEffects = [effect]
        let assets = [id: AudioSamples(channels: [(0..<count).map { Float(sin(Double($0) * 0.031)) * 0.1 }])]
        let cache = DSPSeekCache(intervalFrames: 2048)
        _ = RealtimeMixer(session: session, assets: assets, startFrame: 24_000, seekCache: cache)
        let mixer = RealtimeMixer(session: session, assets: assets, startFrame: 9000, seekCache: cache)
        XCTAssertEqual(mixer.restoredCheckpointFrame, 8192)
        var left = [Float](repeating: 0, count: 12_000)
        var right = left
        left.withUnsafeMutableBufferPointer { l in
            right.withUnsafeMutableBufferPointer { r in
                mixer.render(intoLeft: l.baseAddress!, intoRight: r.baseAddress!, frameCount: 17)
                mixer.render(intoLeft: l.baseAddress! + 17, intoRight: r.baseAddress! + 17, frameCount: l.count - 17)
            }
        }
        XCTAssertEqual(
            AudioSamples(channels: [left, right]), Mixer.render(session: session, assets: assets, range: 9000..<21_000))
    }
    func testDenseExtremeParametersRemainFiniteAndOutputIncludesDryBranch() {
        for preset in RealtimeEffectKind.room.presets {
            var values = preset.values
            values["decay"] = 10
            values["output"] = 12
            values["diffusion"] = 1
            let audio = AudioSamples(channels: [
                [1] + [Float](repeating: 0, count: 48_000), [Float](repeating: 0, count: 48_001),
            ])
            let result = RealtimeEffect(kind: .room, parameters: values).process(audio)
            XCTAssertTrue(result.channels.flatMap { $0 }.allSatisfy(\.isFinite))
        }
        var values = RealtimeEffectKind.room.defaultParameters
        values["mix"] = 0
        values["output"] = -6
        let result = RealtimeEffect(kind: .room, parameters: values).process(AudioSamples(channels: [[1, -1]]))
        XCTAssertEqual(result.channels[0][0], pow(10, -6.0 / 20), accuracy: 0.000001)
        XCTAssertEqual(result.channels[0][1], -pow(10, -6.0 / 20), accuracy: 0.000001)
    }
    func testAlgorithmCrossfadeAndCopiedTailRemainContinuousAndIndependent() {
        let state = StereoRoomDSP()
        var values = RealtimeEffectKind.room.parameters.map { RealtimeEffectKind.room.defaultParameters[$0.id]! }
        state.configure(values)
        for i in 0..<12_000 {
            _ = state.process(left: Float(sin(Double(i) * 0.041)) * 0.1, right: 0, classicLeft: 0.2, classicRight: 0.1)
        }
        values[4] = 0
        state.configure(values)
        let copy = state.deepCopy()
        var previous = state.deepCopy().process(left: 0, right: 0, classicLeft: 0.2, classicRight: 0.1).0
        for _ in 0..<1000 {
            let a = state.process(left: 0, right: 0, classicLeft: 0.2, classicRight: 0.1)
            let b = copy.process(left: 0, right: 0, classicLeft: 0.2, classicRight: 0.1)
            XCTAssertEqual(a.0, b.0)
            XCTAssertEqual(a.1, b.1)
            XCTAssertLessThan(abs(a.0 - previous), 0.03)
            previous = a.0
        }
        XCTAssertEqual(previous, 0.2)
        let independent = copy.deepCopy()
        for _ in 0..<100 { _ = state.process(left: 1, right: 1, classicLeft: 0.2, classicRight: 0.1) }
        values[4] = 1
        copy.configure(values)
        independent.configure(values)
        for _ in 0..<1000 {
            let a = copy.process(left: 0, right: 0, classicLeft: 0, classicRight: 0)
            let b = independent.process(left: 0, right: 0, classicLeft: 0, classicRight: 0)
            XCTAssertEqual(a.0, b.0)
            XCTAssertEqual(a.1, b.1)
        }
    }
}

/// Frozen pre-upgrade topology, including its per-sample coefficient formulas.
private struct OriginalRoomReference {
    var lines = [1423, 2137, 3163, 4789].map { [Float](repeating: 0, count: $0) }
    var positions = [0, 0, 0, 0], damping = [Float](repeating: 0, count: 4)
    var allpasses = [347, 113].map { [Float](repeating: 0, count: $0) }, ap = [0, 0]
    mutating func process(_ input: Float) -> Float {
        let coefficient = 1 - exp(-2 * Float.pi * 6000 / Float(sessionRate))
        var result: Float = 0
        for i in 0..<4 {
            let delayed = lines[i][positions[i]]
            damping[i] += coefficient * (delayed - damping[i])
            let feedback = pow(Float(10), -3 * Float(lines[i].count) / (Float(sessionRate) * 1.4))
            lines[i][positions[i]] = input + damping[i] * feedback
            positions[i] = (positions[i] + 1) % lines[i].count
            result += delayed * 0.25
        }
        for i in 0..<2 {
            let delayed = allpasses[i][ap[i]]
            let output = delayed - result * 0.5
            allpasses[i][ap[i]] = result + output * 0.5
            ap[i] = (ap[i] + 1) % allpasses[i].count
            result = output
        }
        return result
    }
}
