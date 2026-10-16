import XCTest
@testable import SoundroomCore

final class RealtimeEffectTests: XCTestCase {
    private func fixture(count: Int = 30_000) -> (Session, [UUID: AudioSamples]) {
        var session = Session(); let id = UUID()
        let left = (0..<count).map { i in Float(sin(Double(i) * 0.04)) * 0.4 }
        let right = (0..<count).map { i in Float(cos(Double(i) * 0.027)) * 0.25 }
        session.tracks[0].clips = [Clip(assetID: id, name: "Signal", start: 0, length: count, edgeFadeFrames: 0)]
        return (session, [id: AudioSamples(channels: [left, right])])
    }
    private func render(_ mixer: RealtimeMixer, count: Int) -> AudioSamples {
        var left = Array(repeating: Float(0), count: count), right = left
        left.withUnsafeMutableBufferPointer { l in
            right.withUnsafeMutableBufferPointer { r in
                mixer.render(intoLeft: l.baseAddress!, intoRight: r.baseAddress!, frameCount: count)
            }
        }
        return AudioSamples(channels: [left, right])
    }
    func testAutomationUsesSessionTimeAndInterpolatesBetweenPoints() {
        let effect = RealtimeEffect(kind: .gain, amount: 0.3, automation: [
            AutomationPoint(frame: 20, value: 1), AutomationPoint(frame: 10, value: 0)
        ])
        XCTAssertEqual(effect.value(at: 0), 0.3)
        XCTAssertEqual(effect.value(at: 9), 0.3)
        XCTAssertEqual(effect.value(at: 10), 0)
        XCTAssertEqual(effect.value(at: 15), 0.5)
        XCTAssertEqual(effect.value(at: 20), 1)
        XCTAssertEqual(effect.value(at: 999), 1)
        let invalid = RealtimeEffect(kind: .echo, amount: .nan, automation: [
            AutomationPoint(frame: -5, value: -2), AutomationPoint(frame: 0, value: 3)
        ])
        XCTAssertEqual(invalid.value(at: 0), 1)
        XCTAssertEqual(AutomationPoint(frame: -10, value: -2).frame, 0)
    }
    func testDisabledEffectsAndIdentityGainPreserveSource() {
        var (session, assets) = fixture()
        let dry = Mixer.render(session: session, assets: assets, range: 0..<session.end)
        session.tracks[0].realtimeEffects = RealtimeEffectKind.allCases.map { RealtimeEffect(kind: $0, enabled: false) }
        XCTAssertEqual(Mixer.render(session: session, assets: assets, range: 0..<session.end), dry)
        session.tracks[0].realtimeEffects = [RealtimeEffect(kind: .gain, amount: 0.5)]
        XCTAssertEqual(Mixer.render(session: session, assets: assets, range: 0..<session.end), dry)
        XCTAssertEqual(assets.values.first!.channels[0][7], dry.channels[0][7])
    }
    func testArbitraryBlockSizesKeepFilterAndEchoHistory() {
        var (session, assets) = fixture()
        session.tracks[0].realtimeEffects = [
            RealtimeEffect(kind: .lowPass, automation: [AutomationPoint(frame: 0, value: 0.1), AutomationPoint(frame: 28_000, value: 0.9)]),
            RealtimeEffect(kind: .echo, amount: 0.7), RealtimeEffect(kind: .warmth, amount: 0.2)
        ]
        let whole = Mixer.render(session: session, assets: assets, range: 0..<session.end)
        let mixer = RealtimeMixer(session: session, assets: assets)
        var chunks = [Float](), other = [Float]()
        for size in [1, 127, 512, 10_000, 8192, 11_168] {
            let block = render(mixer, count: size)
            chunks += block.channels[0]; other += block.channels[1]
        }
        XCTAssertEqual(chunks.count, 30_000)
        XCTAssertEqual(AudioSamples(channels: [chunks, other]), whole)
        XCTAssertEqual(mixer.position, session.end); XCTAssertTrue(mixer.completed)
    }
    func testSeekingAndRangeExportWarmTheSameHistoryAsPlayback() {
        var (session, assets) = fixture()
        session.tracks[0].realtimeEffects = [RealtimeEffect(kind: .echo, amount: 0.8), RealtimeEffect(kind: .lowPass, amount: 0.8)]
        let complete = Mixer.render(session: session, assets: assets, range: 0..<session.end)
        let range = 12_050..<23_000
        let segment = Mixer.render(session: session, assets: assets, range: range)
        XCTAssertEqual(segment, complete.slice(range))
        XCTAssertEqual(render(RealtimeMixer(session: session, assets: assets, startFrame: range.lowerBound), count: range.count), segment)
    }
    func testUpdateAppliesParametersWithoutResettingEchoHistory() {
        var (session, assets) = fixture(count: 13_000)
        let asset = Array(assets.keys)[0]
        assets[asset] = AudioSamples(channels: [Array(repeating: 0, count: 13_000)])
        assets[asset]!.channels[0][0] = 1
        session.tracks[0].realtimeEffects = [RealtimeEffect(kind: .echo, amount: 0.2)]
        let mixer = RealtimeMixer(session: session, assets: assets)
        _ = render(mixer, count: 12_000)
        session.tracks[0].realtimeEffects[0].amount = 0.8
        mixer.update(session: session, assets: assets)
        let next = render(mixer, count: 1)
        XCTAssertEqual(next.channels[0][0], 0.8 * 0.65, accuracy: 0.00001)
        session.tracks[0].realtimeEffects = [RealtimeEffect(kind: .gain, amount: 1)]
        mixer.update(session: session, assets: assets)
        XCTAssertEqual(render(mixer, count: 2).channels[0], [0, 0])
    }
    func testTrackGainPanAndMuteOperateAfterRealtimeEffects() {
        var (session, assets) = fixture(count: 100)
        session.tracks[0].realtimeEffects = [RealtimeEffect(kind: .gain, amount: 0.75)]
        let wet = Mixer.render(session: session, assets: assets, range: 0..<100)
        session.tracks[0].gain = 0.2; session.tracks[0].pan = 1
        let panned = Mixer.render(session: session, assets: assets, range: 0..<100)
        XCTAssertEqual(panned.channels[0], Array(repeating: 0, count: 100))
        XCTAssertEqual(panned.channels[1][3], wet.channels[1][3] * 0.2, accuracy: 0.00001)
        session.tracks[0].muted = true
        XCTAssertEqual(Mixer.render(session: session, assets: assets, range: 0..<100).channels[1], Array(repeating: 0, count: 100))
    }
    func testRealtimeMetadataRoundTripsAndOldTracksRemainDry() throws {
        let old = Data("{\"version\":1,\"name\":\"Old\",\"tracks\":[{\"name\":\"Audio 1\",\"color\":0}]}".utf8)
        let decoded = try JSONDecoder().decode(Session.self, from: old)
        XCTAssertEqual(decoded.version, 1)
        XCTAssertTrue(decoded.tracks[0].realtimeEffects.isEmpty)
        XCTAssertEqual(decoded.tracks[0].gain, 1)
        var (session, _) = fixture(count: 10)
        session.tracks[0].realtimeEffects = [RealtimeEffect(kind: .gain, automation: [AutomationPoint(frame: 3, value: 0.8)])]
        XCTAssertEqual(try JSONDecoder().decode(Session.self, from: JSONEncoder().encode(session)), session)
        XCTAssertEqual(session.version, 5)
    }
    func testInvalidSamplesAndParametersStayFinite() {
        var (session, assets) = fixture(count: 100)
        let id = Array(assets.keys)[0]
        assets[id] = AudioSamples(channels: [[.nan, .infinity, -.infinity, 0.5] + Array(repeating: 0, count: 96)])
        session.tracks[0].realtimeEffects = RealtimeEffectKind.allCases.map { RealtimeEffect(kind: $0, amount: .nan) }
        XCTAssertTrue(Mixer.render(session: session, assets: assets, range: 0..<100).channels.flatMap { $0 }.allSatisfy(\.isFinite))
        session.tracks[0].gain = .infinity; session.tracks[0].pan = .nan
        XCTAssertTrue(Mixer.render(session: session, assets: assets, range: 0..<100).channels.flatMap { $0 }.allSatisfy(\.isFinite))
    }
    func testSilenceBeyondSessionEndAndNegativeRangeAlignment() {
        var (session, assets) = fixture(count: 3)
        session.tracks[0].realtimeEffects = [RealtimeEffect(kind: .echo, amount: 1)]
        let dry = assets.values.first!
        XCTAssertEqual(Mixer.render(session: session, assets: assets, range: -2..<5).channels[0], [0, 0] + dry.channels[0] + [0, 0])
        XCTAssertEqual(Mixer.render(session: session, assets: assets, range: 100..<110).channels[0], Array(repeating: 0, count: 10))
    }
    func testStructuralSlicesKeepEditableInstrumentSourceMetadata() {
        var (session, _) = fixture(count: 100)
        let pattern = SoundDesignPattern()
        session.tracks[0].clips[0].soundDesign = pattern
        let trackID = session.tracks[0].id, clipID = session.tracks[0].clips[0].id
        XCTAssertEqual(session.fragments(trackID: trackID, range: 0..<100)[0].soundDesign, pattern)
        XCTAssertEqual(session.fragments(trackID: trackID, range: 20..<80)[0].soundDesign, pattern)
        var split = session
        split.split(trackID: trackID, clipID: clipID, at: 50)
        XCTAssertTrue(split.tracks[0].clips.allSatisfy { $0.soundDesign == pattern })
        session.delete(trackID: trackID, range: 20..<80)
        XCTAssertTrue(session.tracks[0].clips.allSatisfy { $0.soundDesign == pattern })
    }
}
