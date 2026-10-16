import XCTest
@testable import SoundroomCore

final class PreparedMixerTests: XCTestCase {
    private func render(_ mixer: RealtimeMixer, count: Int) -> AudioSamples {
        var left = Array(repeating: Float(0), count: count), right = left
        left.withUnsafeMutableBufferPointer { l in right.withUnsafeMutableBufferPointer { r in
            mixer.render(intoLeft: l.baseAddress!, intoRight: r.baseAddress!, frameCount: count)
        } }
        return AudioSamples(channels: [left, right])
    }
    func testPreparedMetadataUpdatesPreserveEveryEffectStateAndAutomationExactly() {
        let id = UUID(), count = 20_000
        var session = Session(); session.tracks[0].clips = [.init(assetID: id, name: "Signal", start: 0, length: count, edgeFadeFrames: 0)]
        session.tracks[0].realtimeEffects = RealtimeEffectKind.allCases.map { kind in
            var effect = RealtimeEffect(kind: kind, parameters: kind.defaultParameters)
            effect.parameterAutomation[kind.parameters[0].id] = [.init(frame: 0, value: 0.3), .init(frame: count - 1, value: 0.7)]
            return effect
        }
        let assets = [id: AudioSamples(channels: [(0..<count).map { Float(sin(Double($0) * 0.029)) * 0.2 }])]
        let expected = Mixer.render(session: session, assets: assets, range: 0..<count)
        let mixer = RealtimeMixer(session: session, assets: assets)
        var result = AudioSamples(channels: [[], []])
        for (index, size) in [17, 495, 8192, 11_296].enumerated() {
            session.tracks[0].name = "Renamed \(index)"
            let graph = RealtimeMixer.prepareUpdate(session: session, assets: assets)
            XCTAssertTrue(mixer.applyPreparedUpdate(graph))
            XCTAssertFalse(mixer.applyPreparedUpdate(graph), "Handoffs are single-use")
            let block = render(mixer, count: size)
            for channel in 0..<2 { result.channels[channel] += block.channels[channel] }
            graph.releaseRetiredResources()
        }
        XCTAssertEqual(result, expected)
    }
    func testAddingRemovingAndReorderingIdentityInsertRetainsEchoTail() {
        let id = UUID(), count = 240
        let audio = AudioSamples(channels: [[1] + Array(repeating: 0, count: count - 1)])
        var session = Session(); session.tracks[0].clips = [.init(assetID: id, name: "Impulse", start: 0, length: count, edgeFadeFrames: 0)]
        let echo = RealtimeEffect(kind: .echo, parameters: ["time": 1, "feedback": 0.5, "mix": 1, "damping": 20_000])
        session.tracks[0].realtimeEffects = [echo]
        let assets = [id: audio], expected = Mixer.render(session: session, assets: [id: audio], range: 0..<count)
        let mixer = RealtimeMixer(session: session, assets: assets)
        let gain = RealtimeEffect(kind: .gain, parameters: ["gain": 0])
        var left: [Float] = [], right: [Float] = []
        for chain in [[echo], [gain, echo], [echo, gain], [echo]] {
            session.tracks[0].realtimeEffects = chain
            let graph = RealtimeMixer.prepareUpdate(session: session, assets: assets)
            mixer.applyPreparedUpdate(graph)
            let block = render(mixer, count: 60)
            left += block.channels[0]; right += block.channels[1]
            graph.releaseRetiredResources()
        }
        XCTAssertEqual(AudioSamples(channels: [left, right]), expected)
    }
    func testPreparedSourceAndTrackChangesBecomeVisibleOnlyAtHandoff() {
        let id = UUID()
        var session = Session(); session.tracks[0].clips = [.init(assetID: id, name: "Source", start: 0, length: 32, edgeFadeFrames: 0)]
        let mixer = RealtimeMixer(session: session, assets: [id: AudioSamples(channels: [Array(repeating: 0.25, count: 32)])])
        session.tracks[0].gain = 0.5
        let prepared = RealtimeMixer.prepareUpdate(session: session, assets: [id: AudioSamples(channels: [Array(repeating: 0.75, count: 32)])])
        XCTAssertEqual(render(mixer, count: 8).channels[0], Array(repeating: 0.25, count: 8))
        mixer.applyPreparedUpdate(prepared)
        XCTAssertEqual(render(mixer, count: 8).channels[0], Array(repeating: 0.375, count: 8))
        prepared.releaseRetiredResources()
    }
    func testDenseAutomationCursorMatchesPublicCurveAndSupportsBackwardSeek() {
        let points = (0..<12_000).map { AutomationPoint(frame: $0 * 7 + 20, value: Float($0 % 101) / 100) }
        let effect = RealtimeEffect(kind: .gain, amount: 0.3, automation: points)
        let cursor = AutomationCursor(points: points, baseline: 0.3)
        for frame in [-10, 0, 19, 20, 21, 5000, 50_000, 100, 84_012, 100_000, 20, 0] {
            XCTAssertEqual(cursor.value(at: frame), effect.value(at: frame), "Absolute-frame curve at \(frame)")
        }
        let duplicate = AutomationCursor(points: [.init(frame: 0, value: 0), .init(frame: 10, value: 0.2), .init(frame: 10, value: 0.8)], baseline: 0)
        XCTAssertEqual(duplicate.value(at: 5), 0.4, accuracy: 0.00001)
        XCTAssertEqual(duplicate.value(at: 10), 0.8)
    }
    func testPartialParameterCurvesKeepLegacyAmountAutomation() {
        for kind in [RealtimeEffectKind.lowPass, .echo, .warmth] {
            var effect = RealtimeEffect(kind: kind, amount: 0.2,
                                        automation: [.init(frame: 0, value: 0.1), .init(frame: 100, value: 0.9)])
            let secondary = kind == .lowPass ? "resonance" : kind == .echo ? "time" : "tone"
            effect.parameterAutomation[secondary] = [.init(frame: 0, value: 0.5)]
            let processor = EffectProcessor(effect: effect)
            let primary = kind == .lowPass ? "frequency" : kind == .echo ? "mix" : "drive"
            let index = processor.descriptors.firstIndex { $0.id == primary }!
            for frame in [0, 1, 33, 75, 100, 101] {
                var left: Float = 0.1, right: Float = 0.1
                withUnsafeMutablePointer(to: &left) { l in withUnsafeMutablePointer(to: &right) { r in
                    processor.process(left: l, right: r, count: 1, position: frame)
                } }
                XCTAssertEqual(processor.values[index], effect.parameterValue(primary, at: frame), accuracy: 0.0001)
            }
        }
    }
    func testLiveGainAndBypassTransitionsBoundDiscontinuitiesAndFinishInTenMilliseconds() {
        let id = UUID(), count = 4000
        var session = Session(); session.tracks[0].clips = [.init(assetID: id, name: "DC", start: 0, length: count, edgeFadeFrames: 0)]
        session.tracks[0].realtimeEffects = [.init(kind: .gain, parameters: ["gain": 0])]
        let assets = [id: AudioSamples(channels: [Array(repeating: Float(0.1), count: count)])]
        let mixer = RealtimeMixer(session: session, assets: assets)
        XCTAssertEqual(render(mixer, count: 16).channels[0], Array(repeating: 0.1, count: 16))
        session.tracks[0].realtimeEffects[0].parameters["gain"] = 20
        let change = RealtimeMixer.prepareUpdate(session: session, assets: assets)
        mixer.applyPreparedUpdate(change, smoothManualChanges: true)
        let ramp = render(mixer, count: LiveRamp.frameCount + 16).channels[0]
        XCTAssertLessThan(abs(ramp[0] - 0.1), 0.001)
        XCTAssertTrue(zip(ramp, ramp.dropFirst()).allSatisfy { abs($0.0 - $0.1) < 0.01 })
        XCTAssertEqual(ramp[LiveRamp.frameCount - 1], 1, accuracy: 0.000001)
        XCTAssertEqual(ramp.last!, 1, accuracy: 0.000001)
        change.releaseRetiredResources()
        session.tracks[0].realtimeEffects[0].enabled = false
        let bypass = RealtimeMixer.prepareUpdate(session: session, assets: assets)
        mixer.applyPreparedUpdate(bypass, smoothManualChanges: true)
        let bypassed = render(mixer, count: LiveRamp.frameCount + 16).channels[0]
        XCTAssertLessThan(abs(bypassed[0] - 1), 0.003)
        XCTAssertTrue(zip(bypassed, bypassed.dropFirst()).allSatisfy { abs($0.0 - $0.1) < 0.003 })
        XCTAssertEqual(bypassed[LiveRamp.frameCount - 1], 0.1)
        XCTAssertEqual(bypassed.last!, 0.1)
        bypass.releaseRetiredResources()
    }
    func testLiveTrackGainAndPanRampAndAutomationRemainsExact() {
        let id = UUID(), count = 2000
        var session = Session(); session.tracks[0].clips = [.init(assetID: id, name: "DC", start: 0, length: count, edgeFadeFrames: 0)]
        let assets = [id: AudioSamples(channels: [Array(repeating: Float(1), count: count)])]
        let mixer = RealtimeMixer(session: session, assets: assets)
        _ = render(mixer, count: 16)
        session.tracks[0].gain = 0.5; session.tracks[0].pan = 1
        let update = RealtimeMixer.prepareUpdate(session: session, assets: assets)
        mixer.applyPreparedUpdate(update, smoothManualChanges: true)
        let changed = render(mixer, count: LiveRamp.frameCount + 1)
        XCTAssertGreaterThan(changed.channels[0][0], 0.99)
        XCTAssertGreaterThan(changed.channels[1][0], 0.99)
        XCTAssertEqual(changed.channels[0].last!, 0)
        XCTAssertEqual(changed.channels[1].last!, 0.5)
        update.releaseRetiredResources()

        var automated = Session(); automated.tracks[0].clips = session.tracks[0].clips
        var effect = RealtimeEffect(kind: .gain, parameters: ["gain": 0])
        effect.parameterAutomation["gain"] = [.init(frame: 0, value: 0.5), .init(frame: 100, value: 0.9)]
        automated.tracks[0].realtimeEffects = [effect]
        let reference = Mixer.render(session: automated, assets: assets, range: 0..<count)
        let live = RealtimeMixer(session: automated, assets: assets)
        var result = render(live, count: 30)
        automated.tracks[0].realtimeEffects[0].parameters["gain"] = -20
        let graph = RealtimeMixer.prepareUpdate(session: automated, assets: assets)
        live.applyPreparedUpdate(graph, smoothManualChanges: true)
        let rest = render(live, count: count - 30)
        for i in 0..<2 { result.channels[i] += rest.channels[i] }
        XCTAssertEqual(result, reference, "A base control change must not modify its existing automation curve")
        graph.releaseRetiredResources()
    }
    func testBypassingAnAudibleTailDoesNotEndBeforeItsLiveRamp() {
        let id = UUID()
        var session = Session(); session.tracks[0].clips = [.init(assetID: id, name: "Impulse", start: 0, length: 1, edgeFadeFrames: 0)]
        session.tracks[0].realtimeEffects = [.init(kind: .echo, parameters: ["time": 1, "feedback": 0.9, "mix": 1, "damping": 20_000])]
        let assets = [id: AudioSamples(channels: [[1]])]
        let mixer = RealtimeMixer(session: session, assets: assets, includeEffectTails: true)
        _ = render(mixer, count: 100)
        session.tracks[0].realtimeEffects[0].enabled = false
        let graph = RealtimeMixer.prepareUpdate(session: session, assets: assets)
        mixer.applyPreparedUpdate(graph, smoothManualChanges: true)
        XCTAssertEqual(mixer.end, 100 + LiveRamp.frameCount)
        XCTAssertFalse(mixer.completed)
        let decay = render(mixer, count: LiveRamp.frameCount).channels[0]
        XCTAssertTrue(decay.contains { abs($0) > 0.001 })
        XCTAssertEqual(decay.last!, 0)
        XCTAssertTrue(mixer.completed)
        graph.releaseRetiredResources()
    }
    func testExplicitRenderEndpointSurvivesLiveGraphUpdatesAndEndsExactly() {
        let id = UUID()
        var session = Session(); session.tracks[0].clips = [.init(assetID: id, name: "Loop", start: 0, length: 100, edgeFadeFrames: 0)]
        let assets = [id: AudioSamples(channels: [Array(repeating: Float(0.25), count: 100)])]
        let mixer = RealtimeMixer(session: session, assets: assets, startFrame: 10, renderEndFrame: 30)
        XCTAssertEqual(render(mixer, count: 8).channels[0], Array(repeating: 0.25, count: 8))
        session.tracks[0].realtimeEffects = [.init(kind: .echo)]
        let graph = RealtimeMixer.prepareUpdate(session: session, assets: assets)
        mixer.applyPreparedUpdate(graph, smoothManualChanges: true)
        XCTAssertEqual(mixer.end, 30)
        let block = render(mixer, count: 20).channels[0]
        XCTAssertEqual(Array(block.prefix(12)), Array(repeating: 0.25, count: 12))
        XCTAssertEqual(Array(block.suffix(8)), Array(repeating: 0, count: 8))
        XCTAssertEqual(mixer.position, 38)
        XCTAssertTrue(mixer.completed)
        graph.releaseRetiredResources()
        let beyondSources = RealtimeMixer(session: session, assets: assets, renderEndFrame: 200)
        _ = render(beyondSources, count: 200)
        XCTAssertEqual(beyondSources.end, 200)
        XCTAssertTrue(beyondSources.completed)
    }
}
