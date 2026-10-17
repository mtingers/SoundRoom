import XCTest

@testable import SoundroomCore

final class EffectLatencyTests: XCTestCase {
    private func source() -> AudioSamples {
        var samples = (0..<2048).map { Float(sin(Double($0) * 0.07) * 0.2) }
        samples[0] = 0.37
        samples[samples.count - 1] = -0.41
        return AudioSamples(channels: [samples, samples.map { -$0 }])
    }
    private func warmth(enabled: Bool = true) -> RealtimeEffect {
        var values = RealtimeEffectKind.warmth.defaultParameters
        values["mix"] = 0
        return RealtimeEffect(kind: .warmth, enabled: enabled, parameters: values)
    }
    func testSelectionProcessingPreservesFirstAndLastSamplesAndBypassAlignment() {
        let audio = source()
        for effect in [warmth(), warmth(enabled: false)] {
            XCTAssertEqual(effect.latencyFrames, 16)
            XCTAssertEqual(effect.process(audio), audio)
        }
        XCTAssertEqual(RealtimeEffect(kind: .warmth, amount: 0.4).latencyFrames, 0)
    }
    func testMixedTrackChainAndAutomationStayOnSourceTimeline() {
        let asset = UUID()
        let audio = source()
        let gain = RealtimeEffect(
            kind: .gain, parameters: ["gain": 0],
            parameterAutomation: ["gain": [.init(frame: 0, value: 0.5), .init(frame: 2047, value: 0.65)]])
        var reference = Session()
        reference.tracks[0].clips = [
            Clip(assetID: asset, name: "Endpoints", start: 0, length: audio.count, edgeFadeFrames: 0)
        ]
        reference.tracks[0].realtimeEffects = [gain]
        let expected = Mixer.render(session: reference, assets: [asset: audio], range: 0..<audio.count)
        for chain in [[gain, warmth()], [warmth(), gain], [warmth(), gain, warmth(enabled: false)]] {
            var session = reference
            session.tracks[0].realtimeEffects = chain
            let rendered = Mixer.render(session: session, assets: [asset: audio], range: 0..<audio.count)
            XCTAssertEqual(rendered, expected)
            let range = 931..<audio.count
            XCTAssertEqual(Mixer.render(session: session, assets: [asset: audio], range: range), rendered.slice(range))
        }
    }
    func testCheckpointPreservesWarmthFilterAndAlignedDryHistory() {
        let audio = source()
        let asset = UUID()
        var session = Session()
        session.tracks[0].clips = [Clip(assetID: asset, name: "Tone", start: 0, length: audio.count, edgeFadeFrames: 0)]
        session.tracks[0].realtimeEffects = [
            RealtimeEffect(kind: .warmth, parameters: RealtimeEffectKind.warmth.defaultParameters)
        ]
        let cache = DSPSeekCache(intervalFrames: 480, maximumBytes: 1_048_576)
        _ = RealtimeMixer(session: session, assets: [asset: audio], startFrame: 1500, seekCache: cache)
        let fromCheckpoint = RealtimeMixer(session: session, assets: [asset: audio], startFrame: 1511, seekCache: cache)
        let reference = RealtimeMixer(session: session, assets: [asset: audio], startFrame: 1511)
        XCTAssertNotNil(fromCheckpoint.restoredCheckpointFrame)
        var left = [Float](repeating: 0, count: 128)
        var right = left
        var otherLeft = left
        var otherRight = left
        left.withUnsafeMutableBufferPointer { l in
            right.withUnsafeMutableBufferPointer { r in
                fromCheckpoint.render(intoLeft: l.baseAddress!, intoRight: r.baseAddress!, frameCount: 128)
            }
        }
        otherLeft.withUnsafeMutableBufferPointer { l in
            otherRight.withUnsafeMutableBufferPointer { r in
                reference.render(intoLeft: l.baseAddress!, intoRight: r.baseAddress!, frameCount: 128)
            }
        }
        XCTAssertEqual(left, otherLeft)
        XCTAssertEqual(right, otherRight)
    }
}
