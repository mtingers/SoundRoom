import XCTest
@testable import SoundroomCore

final class InteractiveSeekTests: XCTestCase {
    private func fixture() -> (Session, [UUID: AudioSamples]) {
        let id = UUID(), count = 144_000
        var session = Session()
        session.tracks[0].clips = [Clip(assetID: id, name: "Signal", start: 0, length: count, edgeFadeFrames: 0)]
        session.tracks[0].realtimeEffects = [RealtimeEffect(kind: .lowPass, amount: 0.4), RealtimeEffect(kind: .echo, parameters: RealtimeEffectKind.echo.defaultParameters), RealtimeEffect(kind: .room)]
        session.tracks[0].mixAutomation["pan"] = [.init(frame: 0, value: 0), .init(frame: count, value: 1)]
        let audio = AudioSamples(channels: [(0..<count).map { Float(sin(Double($0) * 0.025)) * 0.1 }])
        return (session, [id: audio])
    }
    private func render(_ mixer: RealtimeMixer, count: Int = 512) -> ([Float], [Float]) {
        var left = Array(repeating: Float(0), count: count), right = left
        left.withUnsafeMutableBufferPointer { l in right.withUnsafeMutableBufferPointer { r in mixer.render(intoLeft: l.baseAddress!, intoRight: r.baseAddress!, frameCount: count) } }
        return (left, right)
    }
    func testLateInteractiveSeekHasBoundedWarmupAndExactTimelinePosition() {
        let (session, assets) = fixture()
        for start in [0, 1000, 48_000, 120_000] {
            let mixer = RealtimeMixer(session: session, assets: assets, startFrame: start, seekPolicy: .interactive)
            XCTAssertEqual(mixer.position, start)
            XCTAssertEqual(mixer.seekWarmupFrames, min(start, RealtimeSeekPolicy.interactiveHistoryFrames))
            let output = render(mixer)
            XCTAssertTrue((output.0 + output.1).allSatisfy(\.isFinite))
            XCTAssertGreaterThan(output.0.map(abs).max()!, 0.001)
            XCTAssertEqual(mixer.position, start + 512)
        }
    }
    func testLocalStatesCannotPoisonCanonicalCheckpointsOrRangeExports() {
        let (session, assets) = fixture(), cache = DSPSeekCache(intervalFrames: 4096)
        let local = RealtimeMixer(session: session, assets: assets, startFrame: 120_000, seekCache: cache, seekPolicy: .interactive)
        XCTAssertNil(local.restoredCheckpointFrame)
        XCTAssertEqual(cache.checkpointCount, 0, "Local pre-roll states are not exact song-history checkpoints")
        let exact = RealtimeMixer(session: session, assets: assets, startFrame: 120_000, seekCache: cache)
        XCTAssertEqual(exact.seekWarmupFrames, 120_000)
        let expected = Mixer.render(session: session, assets: assets, range: 120_000..<120_512)
        let result = render(exact)
        XCTAssertEqual(result.0, expected.channels[0]); XCTAssertEqual(result.1, expected.channels[1])
        XCTAssertGreaterThan(cache.checkpointCount, 0)
        let count = cache.checkpointCount
        _ = RealtimeMixer(session: session, assets: assets, startFrame: 120_000, seekCache: cache, seekPolicy: .interactive)
        XCTAssertEqual(cache.checkpointCount, count)
    }
    func testInteractiveLoopWarmupDoesNotReadBeforeLoopStartAndWrapRetainsState() {
        let (session, assets) = fixture(), loop = 48_000..<96_000
        let mixer = RealtimeMixer(session: session, assets: assets, startFrame: 49_000, renderEndFrame: loop.upperBound, loopRange: loop, seekPolicy: .interactive)
        XCTAssertEqual(mixer.seekWarmupFrames, 1000)
        XCTAssertEqual(mixer.position, 49_000)
        XCTAssertEqual(mixer.end, loop.upperBound)
        _ = render(mixer)
        mixer.rewindForLoop(to: loop.lowerBound)
        let wrapped = render(mixer)
        XCTAssertTrue((wrapped.0 + wrapped.1).allSatisfy(\.isFinite))
        XCTAssertGreaterThan(wrapped.0.map(abs).max()!, 0.001)
    }
    func testLatencyCompensationAndAbsoluteAutomationSurviveLocalSeek() {
        let id = UUID(), count = 144_000, start = 96_000
        let audio = AudioSamples(channels: [(0..<count).map { Float(sin(Double($0) * 0.033)) * 0.1 }])
        var session = Session()
        session.tracks[0].clips = [Clip(assetID: id, name: "Aligned", start: 0, length: count, edgeFadeFrames: 0)]
        session.tracks[0].mixAutomation["volume"] = [.init(frame: 0, value: 0), .init(frame: count, value: 1)]
        session.tracks[0].mixAutomation["pan"] = [.init(frame: 0, value: 0), .init(frame: count, value: 1)]
        let dry = render(RealtimeMixer(session: session, assets: [id: audio], startFrame: start))
        var pitch = RealtimeEffect(kind: .pitchCorrection); pitch.enabled = false
        session.tracks[0].realtimeEffects = [pitch]
        let mixer = RealtimeMixer(session: session, assets: [id: audio], startFrame: start, seekPolicy: .interactive)
        XCTAssertLessThanOrEqual(mixer.seekWarmupFrames, RealtimeSeekPolicy.interactiveHistoryFrames)
        let output = render(mixer)
        for (a, b) in zip(output.0, dry.0) { XCTAssertEqual(a, b, accuracy: 0.0000001) }
        for (a, b) in zip(output.1, dry.1) { XCTAssertEqual(a, b, accuracy: 0.0000001) }
    }
}
