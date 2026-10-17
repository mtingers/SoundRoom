import Darwin
import SoundroomCore
import XCTest

@testable import Soundroom

final class LongSelectionEffectTests: XCTestCase {
    /// Run in a fresh release test process per effect so the RSS high-water marks are comparable.
    @MainActor
    func testPreparedHistoryUsesWorkerAudioWithoutAnotherRender() throws {
        let store = SessionStore()
        let asset = UUID()
        let track = store.session.tracks[0].id
        store.assets[asset] = AudioSamples(channels: [[0.1, 0.2, 0.3, 0.4]])
        store.session.tracks[0].clips = [
            Clip(assetID: asset, name: "A", start: 0, length: 2, edgeFadeFrames: 0),
            Clip(assetID: asset, name: "B", start: 2, offset: 2, length: 2, edgeFadeFrames: 0),
        ]
        store.selectedTrack = track
        let prepared = AudioSamples(channels: [[0.7, 0.6, 0.5, 0.4], [0.7, 0.6, 0.5, 0.4]])
        let result = store.processedClip(
            assetID: UUID(), name: "Processed", range: 0..<4, length: 4,
            source: store.session, operation: "Echo", originalAudio: prepared)
        let prior = try XCTUnwrap(result.previousStates?.last)
        XCTAssertEqual(store.assets[prior.assetID]?.cacheIdentity, prepared.cacheIdentity)
        XCTAssertEqual(result.start, 0)
        XCTAssertEqual(prior.length, 4)
        XCTAssertEqual(prior.operation, "Echo")
    }
    @MainActor

    private func writeFixture(_ url: URL, frames: Int) throws {
        let blockFrames = 8192
        let audio = AudioSamples(channels: [(0..<blockFrames).map { sin(Float($0) * .pi / 128) * 0.2 }])
        let encoded = WaveFile.encode(audio, float: true)
        var header = Data(encoded.prefix(44))
        for (offset, value) in [(4, UInt32(frames * 4 + 36)), (40, UInt32(frames * 4))] {
            var value = value.littleEndian
            withUnsafeBytes(of: &value) { header.replaceSubrange(offset..<(offset + 4), with: $0) }
        }
        try header.write(to: url)
        let file = try FileHandle(forWritingTo: url)
        defer { try? file.close() }
        try file.seekToEnd()
        let payload = Data(encoded.dropFirst(44))
        var remaining = frames
        while remaining > 0 {
            let count = min(remaining, blockFrames)
            try file.write(contentsOf: payload.prefix(count * 4))
            remaining -= count
        }
    }
    private func peakRSS() -> Int64 {
        var usage = rusage()
        getrusage(RUSAGE_SELF, &usage)
        return Int64(usage.ru_maxrss)
    }
}
