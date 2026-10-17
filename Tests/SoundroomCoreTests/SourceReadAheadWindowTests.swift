import XCTest

@testable import SoundroomCore

final class SourceReadAheadWindowTests: XCTestCase {
    func testLookaheadWrapsAndPinsActualLoopBeginningWithoutLatencyOffset() {
        let loop = 100000..<200000
        let windows = SourceReadAheadWindows.ranges(
            outputFrame: 200000 - 4096 - 128, latency: 4096, horizon: 32768, loop: loop)
        XCTAssertEqual(windows, [199872..<200000, 100000..<132640, 100000..<132768])
        XCTAssertTrue(windows.allSatisfy { $0.lowerBound >= loop.lowerBound && $0.upperBound <= loop.upperBound })
        XCTAssertEqual(
            SourceReadAheadWindows.ranges(outputFrame: 32807, latency: 4096, horizon: 32768, loop: 32760..<32808),
            [32760..<32808])
        XCTAssertEqual(
            SourceReadAheadWindows.ranges(outputFrame: 199999, latency: 4096, horizon: 32768, loop: loop).first,
            104095..<136863)
    }
    func testWindowArithmeticHandlesPrerollLargeLatencyAndIntegerLimit() {
        XCTAssertEqual(
            SourceReadAheadWindows.ranges(outputFrame: 90000, latency: 104096, horizon: 1000, loop: 100000..<200000),
            [194096..<195096, 100000..<101000])
        let nearLimit = (Int.max - 100000)..<Int.max
        let windows = SourceReadAheadWindows.ranges(
            outputFrame: Int.max - 100, latency: 4096, horizon: 32768, loop: nearLimit)
        XCTAssertTrue(
            windows.allSatisfy { $0.lowerBound >= nearLimit.lowerBound && $0.upperBound <= nearLimit.upperBound })
        let negative = SourceReadAheadWindows.ranges(
            outputFrame: Int.min, latency: 4096, horizon: 32768, loop: nearLimit)
        XCTAssertTrue(
            negative.allSatisfy { $0.lowerBound >= nearLimit.lowerBound && $0.upperBound <= nearLimit.upperBound })
        XCTAssertEqual(
            SourceReadAheadWindows.ranges(outputFrame: Int.max - 10, latency: 4096, horizon: 32768, loop: nil), [])
        XCTAssertEqual(SourceReadAheadWindows.ranges(outputFrame: 0, latency: 4096, horizon: 0, loop: 0..<100), [])
    }
    func testPitchLatencyShortLoopNearTileBoundaryLoadsActualFirstAndLastSamples() throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            .appendingPathExtension("wav")
        defer { try? FileManager.default.removeItem(at: url) }
        let values = (0..<45000).map { Float($0 % 1024) / 2048 }
        try WaveFile.encode(AudioSamples(channels: [values]), float: true).write(to: url)
        let audio = try AudioSamples(mappedWaveFile: url)
        let id = UUID()
        let loop = 32760..<32808
        var session = Session()
        session.tracks[0].clips = [Clip(assetID: id, name: "Loop", start: 0, length: audio.count)]
        session.tracks[0].realtimeEffects = [RealtimeEffect(kind: .pitchCorrection)]
        let cache = RealtimeSourceCache(
            session: session, assets: [id: audio], startFrame: loop.upperBound - 1, loopRange: loop)
        for frame in [loop.lowerBound, loop.upperBound - 8] {
            var copied = 0
            cache.withSamples(id: id, sourceFrame: frame, count: 8) { left, _, offset, count in
                copied += count
                for index in 0..<count { XCTAssertEqual(left[index], values[frame + offset + index]) }
            }
            XCTAssertEqual(copied, 8)
        }
        XCTAssertEqual(cache.missedBlocks, 0)
    }
}
