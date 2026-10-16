import XCTest
@testable import SoundroomCore

final class DSPSeekCacheTests: XCTestCase {
    private func fixture() -> (Session, [UUID: AudioSamples]) {
        var session = Session(); let id = UUID(), count = 30000
        session.tracks[0].clips = [Clip(assetID: id, name: "History", start: 0, length: count)]
        session.tracks[0].realtimeEffects = [RealtimeEffect(kind: .echo, parameters: RealtimeEffectKind.echo.defaultParameters), RealtimeEffect(kind: .room), RealtimeEffect(kind: .compressor)]
        session.tracks[0].realtimeEffects[0].parameterAutomation["mix"] = [.init(frame: 0, value: 0.1), .init(frame: 24000, value: 0.9)]
        return (session, [id: AudioSamples(channels: [(0..<count).map { Float(sin(Double($0) * 0.02) * 0.3) }])])
    }
    func testCachedSeekRestoresIndependentStateAndMatchesUncachedSamples() {
        let (session, assets) = fixture(), cache = DSPSeekCache(intervalFrames: 4096)
        _ = RealtimeMixer(session: session, assets: assets, startFrame: 25000, seekCache: cache)
        let cached = RealtimeMixer(session: session, assets: assets, startFrame: 22000, seekCache: cache)
        let uncached = RealtimeMixer(session: session, assets: assets, startFrame: 22000)
        XCTAssertEqual(cached.restoredCheckpointFrame, 20480); XCTAssertEqual(cached.seekWarmupFrames, 1520)
        for count in [17, 512, 2000] {
            let a = render(cached, count: count), b = render(uncached, count: count)
            for (left, right) in zip(a, b) { XCTAssertEqual(left, right, accuracy: 0.0000001) }
        }
        // Rendering one restored graph must not alter another restoration's history.
        let again = RealtimeMixer(session: session, assets: assets, startFrame: 22000, seekCache: cache)
        let reference = RealtimeMixer(session: session, assets: assets, startFrame: 22000)
        XCTAssertEqual(render(again, count: 512), render(reference, count: 512))
    }
    func testDocumentOrSourceChangeInvalidatesCheckpointAndMemoryIsBounded() {
        var (session, assets) = fixture()
        let cache = DSPSeekCache(intervalFrames: 2048, maximumBytes: 2 * 1024 * 1024)
        _ = RealtimeMixer(session: session, assets: assets, startFrame: 25000, seekCache: cache)
        XCTAssertGreaterThan(cache.checkpointCount, 0); XCTAssertLessThanOrEqual(cache.residentBytes, cache.maximumBytes)
        session.tracks[0].gain = 0.5
        let changed = RealtimeMixer(session: session, assets: assets, startFrame: 20000, seekCache: cache)
        XCTAssertNil(changed.restoredCheckpointFrame)
        let id = assets.keys.first!; assets[id] = AudioSamples(channels: [Array(repeating: 0, count: 30000)])
        let replaced = RealtimeMixer(session: session, assets: assets, startFrame: 20000, seekCache: cache)
        XCTAssertNil(replaced.restoredCheckpointFrame)
        cache.removeAll(); XCTAssertEqual(cache.residentBytes, 0)
    }
    private func render(_ mixer: RealtimeMixer, count: Int) -> [Float] {
        var left = Array(repeating: Float(0), count: count), right = left
        left.withUnsafeMutableBufferPointer { l in right.withUnsafeMutableBufferPointer { r in mixer.render(intoLeft: l.baseAddress!, intoRight: r.baseAddress!, frameCount: count) } }
        return left
    }
}
