import Combine
import SoundroomCore
import XCTest

@testable import Soundroom

final class WaveformCacheTests: XCTestCase {
    func testCacheUsesByteBudgetAndKeepsHotEnvelopeWhenEvicting() {
        let cache = WaveformCache(maximumBytes: 300, maximumEnvelopeBytes: 64)
        let audio = AudioSamples(channels: [[Float](repeating: 0.5, count: 256)])
        let a = UUID()
        let b = UUID()
        let c = UUID()
        _ = cache.envelope(id: a, audio: audio)
        _ = cache.envelope(id: b, audio: audio)
        _ = cache.envelope(id: a, audio: audio)
        _ = cache.envelope(id: c, audio: audio)
        XCTAssertLessThanOrEqual(cache.residentBytes, 300)
        XCTAssertEqual(cache.cachedAssetCount, 2)
        // A cache hit ignores a hypothetical source replacement under the same immutable ID.
        XCTAssertEqual(cache.envelope(id: a, audio: AudioSamples(channels: [[-1]])).values.first?.1, 0.5)
        XCTAssertEqual(cache.envelope(id: b, audio: AudioSamples(channels: [[-1]])).values.first?.0, -1)
        cache.removeAll()
        XCTAssertEqual(cache.residentBytes, 0)
    }
    func testLongAssetUsesBoundedEnvelopeAndPreservesStereoExtremes() {
        let cache = WaveformCache(maximumBytes: 1024, maximumEnvelopeBytes: 64)
        var left = Array(repeating: Float(0), count: 10000)
        var right = left
        left[600] = -0.8
        right[9000] = 0.7
        right[4] = .nan
        let envelope = cache.envelope(id: UUID(), audio: AudioSamples(channels: [left, right]))
        XCTAssertGreaterThan(envelope.framesPerBin, 128)
        XCTAssertLessThanOrEqual(envelope.count * MemoryLayout<(Float, Float)>.stride, 64)
        XCTAssertEqual(envelope[600 / envelope.framesPerBin].0, -0.8)
        XCTAssertEqual(envelope[9000 / envelope.framesPerBin].1, 0.7)
        XCTAssertEqual(envelope.channelValues.count, 2)
        XCTAssertEqual(envelope.channelValues[0].map(\.1).max(), 0)
        XCTAssertEqual(envelope.channelValues[1].map(\.0).min(), 0)
        XCTAssertEqual(envelope.channelValues[0][600 / envelope.framesPerBin].0, -0.8)
        XCTAssertEqual(envelope.channelValues[1][9000 / envelope.framesPerBin].1, 0.7)
        XCTAssertLessThanOrEqual(envelope.byteCount, 64)
        XCTAssertLessThanOrEqual(cache.residentBytes, 1024)
    }
    @MainActor func testAsynchronousStreamedRangeEnvelopePublishesStereoBins() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        var left = Array(repeating: Float(0), count: 32768)
        var right = left
        left[64] = -1  // Outside the retained source range.
        left[8192] = -0.6
        right[24576] = 0.8
        let file = directory.appendingPathComponent("Take.wav")
        try StreamingAudioFile.writeFloat(AudioSamples(channels: [left, right]), to: file)
        let mapped = try AudioSamples(mappedWaveFile: file)
        let audio = try XCTUnwrap(mapped.view(range: 4096..<28672, channels: [0, 1]))
        XCTAssertTrue(audio.isStreamed)
        let cache = WaveformCache(maximumBytes: 1024, maximumEnvelopeBytes: 64)
        let id = UUID()
        let published = expectation(description: "Worker envelope publication")
        let observation = cache.objectWillChange.sink { published.fulfill() }
        XCTAssertNil(cache.cachedEnvelope(id: id, audio: audio))
        await fulfillment(of: [published], timeout: 5)
        let envelope = try XCTUnwrap(cache.cachedEnvelope(id: id, audio: audio))
        XCTAssertEqual(envelope[(8192 - 4096) / envelope.framesPerBin].0, -0.6)
        XCTAssertEqual(envelope[(24576 - 4096) / envelope.framesPerBin].1, 0.8)
        XCTAssertEqual(envelope.values.map(\.0).min(), -0.6)
        XCTAssertEqual(envelope.channelValues[0].map(\.1).max(), 0)
        XCTAssertEqual(envelope.channelValues[1].map(\.0).min(), 0)
        XCTAssertLessThanOrEqual(cache.residentBytes, 1024)
        withExtendedLifetime(observation) {}
    }

    func testServerCoarseRangesReuseImmutableAssetPeaksAndFineRangesRemainExact() {
        let cache = WaveformCache(maximumBytes: 16_384, maximumEnvelopeBytes: 8192)
        let id = UUID()
        var left = Array(repeating: Float(0), count: 16_384)
        var right = left
        left[5123] = -0.8
        right[5987] = 0.9
        right[6000] = .nan
        let audio = AudioSamples(channels: [left, right])
        let coarse = cache.rangeEnvelope(id: id, audio: audio, range: 4096..<12_288, points: 16)
        XCTAssertEqual(coarse.firstFrame, 4096)
        XCTAssertEqual(coarse.framesPerBin, 512)
        XCTAssertEqual(coarse.values.map(\.0).min(), -0.8)
        XCTAssertEqual(coarse.values.map(\.1).max(), 0.9)
        XCTAssertEqual(coarse.channelValues[0].map(\.1).max(), 0)
        XCTAssertEqual(coarse.channelValues[1].map(\.0).min(), 0)
        XCTAssertEqual(cache.cachedAssetCount, 1)
        // IDs identify immutable media: changed source data proves this query
        // uses resident peaks instead of rescanning PCM for each scroll.
        let changed = AudioSamples(channels: [Array(repeating: Float(1), count: 16_384)])
        let reused = cache.rangeEnvelope(id: id, audio: changed, range: 4608..<12_800, points: 16)
        XCTAssertEqual(reused.values.map(\.0).min(), -0.8)
        XCTAssertEqual(reused.values.map(\.1).max(), 0.9)
        let fine = cache.rangeEnvelope(id: id, audio: audio, range: 5122..<5125, points: 3)
        XCTAssertEqual(fine.framesPerBin, 1)
        XCTAssertEqual(fine.values.map(\.0), [0, -0.8, 0])
        XCTAssertEqual(fine.values.map(\.1), [0, 0, 0])
        XCTAssertEqual(fine.channelValues[0].map(\.0), [0, -0.8, 0])
        XCTAssertEqual(fine.channelValues[1].map(\.0), [0, 0, 0])
    }

    func testSilentLeftAndAudibleRightRemainSeparateAtSampleZoomAndAfterTrimming() throws {
        let cache = WaveformCache()
        let id = UUID()
        let audio = AudioSamples(channels: [[0, 0, 0, 0, 0, 0], [1, 0, -0.7, 0.8, 0, -1]])
        let trimmed = try XCTUnwrap(audio.view(range: 1..<5, channels: [0, 1]))
        let envelope = cache.rangeEnvelope(id: id, audio: trimmed, range: 0..<4, points: 4)
        XCTAssertEqual(envelope.firstFrame, 0)
        XCTAssertEqual(envelope.channelValues[0].map(\.0), [0, 0, 0, 0])
        XCTAssertEqual(envelope.channelValues[0].map(\.1), [0, 0, 0, 0])
        XCTAssertEqual(envelope.channelValues[1].map(\.0), [0, -0.7, 0, 0])
        XCTAssertEqual(envelope.channelValues[1].map(\.1), [0, 0, 0.8, 0])
    }

    func testMonoUsesOneEnvelopeAndIdenticalStereoRetainsTwoChannels() {
        let cache = WaveformCache()
        let samples: [Float] = [-0.5, 0.8]
        let mono = cache.envelope(id: UUID(), audio: AudioSamples(channels: [samples]))
        XCTAssertTrue(mono.channelValues.isEmpty)
        let stereo = cache.envelope(id: UUID(), audio: AudioSamples(channels: [samples, samples]))
        XCTAssertEqual(stereo.channelValues.count, 2)
        XCTAssertEqual(stereo.channelValues[0].map(\.0), stereo.channelValues[1].map(\.0))
        XCTAssertEqual(stereo.byteCount, 3 * mono.byteCount)
    }

    func testConcurrentOverviewAndDetailShareBoundedAssetEnvelope() async {
        let cache = WaveformCache(maximumBytes: 64_000, maximumEnvelopeBytes: 32_000)
        let id = UUID()
        var samples = Array(repeating: Float(0.1), count: 1_000_000)
        samples[513_000] = -0.9
        let audio = AudioSamples(channels: [samples])
        await withTaskGroup(of: WaveformEnvelope.self) { group in
            for index in 0..<4 {
                group.addTask {
                    cache.rangeEnvelope(
                        id: id, audio: audio, range: (index * 100_000)..<(index * 100_000 + 600_000), points: 512)
                }
            }
            for await result in group {
                XCTAssertEqual(result.values.map(\.0).min(), -0.9)
                XCTAssertEqual(result.values.map(\.1).max(), 0.1)
            }
        }
        XCTAssertEqual(cache.cachedAssetCount, 1)
        XCTAssertLessThanOrEqual(cache.residentBytes, 64_000)
    }
}
