import XCTest
@testable import SoundroomCore

final class ChorusDitherTests: XCTestCase {
    func testChorusFractionalDelayAndFeedback() {
        let audio = AudioSamples(channels: [[1] + Array(repeating: 0, count: 199)])
        let params: [String: Float] = ["rate": 1, "depth": 0, "delay": 1.0104167,
            "width": 0, "feedback": 0, "mix": 1]
        let result = BuiltInEffect.chorus.process(audio, parameters: params)
        XCTAssertEqual(result.channels[0][48], 0.5, accuracy: 0.00001)
        XCTAssertEqual(result.channels[0][49], 0.5, accuracy: 0.00001)
        XCTAssertEqual(result.channels[0].reduce(0, +), 1, accuracy: 0.00001)
        var feedback = params; feedback["delay"] = 1; feedback["feedback"] = 0.5
        let echoes = BuiltInEffect.chorus.process(audio, parameters: feedback)
        XCTAssertEqual(echoes.channels[0][48], 1, accuracy: 0.00001)
        XCTAssertEqual(echoes.channels[0][96], 0.5, accuracy: 0.00001)
        XCTAssertEqual(echoes.channels[0][144], 0.25, accuracy: 0.00001)
    }

    func testChorusStereoWidthDryMixAndTail() {
        let samples = (0..<6000).map { Float(sin(Double($0) * 0.06)) * 0.3 }
        let audio = AudioSamples(channels: [samples, samples])
        var params = RealtimeEffectKind.chorus.defaultParameters
        params["width"] = 0
        let mono = BuiltInEffect.chorus.process(audio, parameters: params)
        XCTAssertEqual(mono.channels[0], mono.channels[1])
        params["width"] = 1
        let wide = BuiltInEffect.chorus.process(audio, parameters: params)
        XCTAssertNotEqual(wide.channels[0], wide.channels[1])
        params["mix"] = 0
        XCTAssertEqual(BuiltInEffect.chorus.process(audio, parameters: params), audio)
        params["depth"] = 0; params["delay"] = 1; params["mix"] = 1; params["feedback"] = 0
        let impulse = RealtimeEffect(kind: .chorus, parameters: params).process(.init(channels: [[1]]), includeTail: true)
        XCTAssertEqual(impulse.count, 49)
        XCTAssertEqual(impulse.channels[0][48], 1, accuracy: 0.00001)
    }

    func testDitherTPDFStatisticsIndependenceAndDeterminism() {
        let count = 100_000, step = pow(Float(2), -15)
        let effect = RealtimeEffect(kind: .dither, parameters: ["bits": 16, "amount": 1, "quantize": 0])
        let audio = AudioSamples(channels: [Array(repeating: 0, count: count), Array(repeating: 0, count: count)])
        let first = effect.process(audio), second = effect.process(audio)
        XCTAssertEqual(first, second)
        let l = first.channels[0], r = first.channels[1]
        let mean = l.reduce(0, +) / Float(count)
        let variance = l.reduce(0) { $0 + $1 * $1 } / Float(count)
        let cross = zip(l, r).reduce(Float(0)) { $0 + $1.0 * $1.1 } / Float(count)
        XCTAssertLessThan(abs(mean), step * 0.01)
        XCTAssertEqual(variance / (step * step), 1 / 6, accuracy: 0.003)
        XCTAssertLessThan(abs(cross / (step * step)), 0.003)
        XCTAssertTrue(l.allSatisfy { abs($0) < step })
        XCTAssertNotEqual(l, r)
        XCTAssertNotEqual(first, RealtimeEffect(kind: .dither, parameters: effect.parameters).process(audio))
    }

    func testDitherQuantizationAndZeroNoise() {
        let samples: [Float] = [-0.9999, -0.03, 0, 0.03001, 0.9999]
        let audio = AudioSamples(channels: [samples])
        XCTAssertEqual(BuiltInEffect.dither.process(audio, parameters: ["bits": 16, "amount": 0, "quantize": 0]), audio)
        let reduced = BuiltInEffect.dither.process(audio, parameters: ["bits": 8, "amount": 1, "quantize": 1])
        for sample in reduced.channels[0] {
            XCTAssertEqual(sample * 128, (sample * 128).rounded())
        }
        let mode = RealtimeEffectKind.dither.parameters[2]
        XCTAssertEqual(mode.display(0), "Dither only")
        XCTAssertEqual(mode.physical(0.6), 1)
    }

    func testChorusAndDitherMatchBlocksAndSeek() {
        let id = UUID(), count = 5000
        let input = AudioSamples(channels: [(0..<count).map { Float(sin(Double($0) * 0.04)) * 0.25 }])
        for kind in [RealtimeEffectKind.chorus, .dither] {
            var session = Session()
            session.tracks[0].clips = [.init(assetID: id, name: "Signal", start: 0, length: count)]
            session.tracks[0].realtimeEffects = [.init(kind: kind, parameters: kind.defaultParameters)]
            let assets = [id: input]
            let whole = Mixer.render(session: session, assets: assets, range: 0..<count)
            XCTAssertEqual(Mixer.render(session: session, assets: assets, range: 2345..<4000), whole.slice(2345..<4000))
            let mixer = RealtimeMixer(session: session, assets: assets)
            var l = [Float](repeating: 0, count: count), r = l
            l.withUnsafeMutableBufferPointer { lp in r.withUnsafeMutableBufferPointer { rp in
                var offset = 0
                for size in [1, 47, 513, 99, 4340] {
                    mixer.render(intoLeft: lp.baseAddress!.advanced(by: offset), intoRight: rp.baseAddress!.advanced(by: offset), frameCount: size)
                    offset += size
                }
            } }
            XCTAssertEqual(whole, AudioSamples(channels: [l, r]))
        }
    }
}
