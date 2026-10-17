import XCTest

@testable import SoundroomCore

final class AtomicReadAheadTests: XCTestCase {
    private func fixture(_ body: (AudioSamples) throws -> Void) throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            .appendingPathExtension("wav")
        defer { try? FileManager.default.removeItem(at: url) }
        let values = (0..<600000).map { Float($0 % 1024) / 2048 }
        try WaveFile.encode(AudioSamples(channels: [values]), float: true).write(to: url)
        try body(AudioSamples(mappedWaveFile: url))
    }
    func testHeldReaderSurvivesPublishingAndBoundsRetirementWithoutBlockingOtherReaders() throws {
        try fixture { audio in
            var session = Session()
            session.tracks = []
            var assets: [UUID: AudioSamples] = [:]
            let ids = (0..<24).map { _ in UUID() }
            for (index, id) in ids.enumerated() {
                assets[id] = audio
                var track = Track(name: "Read-ahead \(index)", color: index)
                track.clips = [Clip(assetID: id, name: "Source", start: 0, length: audio.count)]
                session.tracks.append(track)
            }
            let cache = RealtimeSourceCache(session: session, assets: assets, startFrame: 0)
            let entered = DispatchSemaphore(value: 0)
            let release = DispatchSemaphore(value: 0)
            let finished = DispatchSemaphore(value: 0)
            let reader = DispatchQueue(label: "Soundroom.test-held-reader")
            reader.async {
                cache.withSamples(id: ids[0], sourceFrame: 100, count: 32) { left, _, _, count in
                    entered.signal()
                    _ = release.wait(timeout: .now() + 10)
                    for index in 0..<count { XCTAssertEqual(left[index], Float((100 + index) % 1024) / 2048) }
                }
                finished.signal()
            }
            XCTAssertEqual(entered.wait(timeout: .now() + 2), .success)
            XCTAssertEqual(cache.activeReaders, 1)
            cache.advance(to: 220000)
            waitUntil { cache.publicationGeneration >= 2 }
            XCTAssertEqual(cache.publicationGeneration, 2)
            // The old snapshot is pinned. Other readers can use the published
            // replacement immediately instead of emitting a contention zero.
            for _ in 0..<100 {
                var copied = 0
                cache.withSamples(id: ids[1], sourceFrame: 220000, count: 512) { left, _, offset, count in
                    copied += count
                    for index in 0..<count {
                        XCTAssertEqual(left[index], Float((220000 + offset + index) % 1024) / 2048)
                    }
                }
                XCTAssertEqual(copied, 512)
            }
            cache.advance(to: 440000)
            Thread.sleep(forTimeInterval: 0.02)
            XCTAssertEqual(
                cache.publicationGeneration, 2,
                "A long reader must defer further publication rather than retain unbounded snapshots")
            release.signal()
            XCTAssertEqual(finished.wait(timeout: .now() + 2), .success)
            waitUntil { cache.publicationGeneration >= 3 }
            XCTAssertEqual(cache.activeReaders, 0)
            XCTAssertEqual(cache.missedBlocks, 0)
        }
    }
    func testDenseCallbackReadsRemainExactWhileWorkerPublishesFutureTiles() throws {
        try fixture { audio in
            var session = Session()
            let id = UUID()
            session.tracks[0].clips = [Clip(assetID: id, name: "Source", start: 0, length: audio.count)]
            let cache = RealtimeSourceCache(
                session: session, assets: [id: audio], startFrame: 0, loopRange: 0..<audio.count)
            let group = DispatchGroup()
            group.enter()
            DispatchQueue.global(qos: .userInitiated).async {
                for frame in stride(from: 100000, to: 500000, by: 10000) {
                    cache.advance(to: frame)
                    Thread.sleep(forTimeInterval: 0.002)
                }
                group.leave()
            }
            for block in 0..<2000 {
                let frame = block % 1000
                var copied = 0
                cache.withSamples(id: id, sourceFrame: frame, count: 128) { left, _, offset, count in
                    copied += count
                    for index in 0..<count {
                        XCTAssertEqual(left[index], Float((frame + offset + index) % 1024) / 2048)
                    }
                }
                XCTAssertEqual(copied, 128)
            }
            XCTAssertEqual(group.wait(timeout: .now() + 5), .success)
            XCTAssertGreaterThan(cache.publicationGeneration, 1)
            XCTAssertEqual(cache.missedBlocks, 0)
        }
    }
    private func waitUntil(_ predicate: () -> Bool, file: StaticString = #filePath, line: UInt = #line) {
        let deadline = Date().addingTimeInterval(5)
        while !predicate() && Date() < deadline { Thread.sleep(forTimeInterval: 0.001) }
        XCTAssertTrue(predicate(), file: file, line: line)
    }
}
