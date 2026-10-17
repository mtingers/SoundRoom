import Darwin
import XCTest

@testable import SoundroomCore

/// Opt-in release qualification with real disk-backed PCM, no session-sized arrays.
final class StreamedSessionBenchmarkTests: XCTestCase {
    func testHourDiskBackedSessionMemorySeekAndCancellation() async throws {
        guard ProcessInfo.processInfo.environment["SOUNDROOM_STREAMED_BENCHMARK"] == "1" else {
            throw XCTSkip("Opt-in hour-long streamed-session qualification")
        }
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let baseline = peakRSS()
        let setupStart = ProcessInfo.processInfo.systemUptime
        let sourceFrames = Int(sessionRate * 600)
        let timelineFrames = sourceFrames * 6
        let ids = [UUID(), UUID()]
        var assets: [UUID: AudioSamples] = [:]
        for index in 0..<2 {
            let url = folder.appendingPathComponent("source-\(index).wav")
            try writeFixture(url: url, frames: sourceFrames, period: 1024 * (index + 1))
            assets[ids[index]] = try AudioSamples(mappedWaveFile: url)
        }
        var session = Session()
        session.tracks = []
        for index in 0..<2 {
            var track = Track(name: "Disk-backed \(index)", color: index)
            track.clips = (0..<6).map {
                Clip(assetID: ids[index], name: "Ten-minute source", start: $0 * sourceFrames, length: sourceFrames)
            }
            track.realtimeEffects = [RealtimeEffect(kind: index == 0 ? .lowPass : .echo, amount: 0.35)]
            session.tracks.append(track)
        }
        let setupSeconds = ProcessInfo.processInfo.systemUptime - setupStart
        let mappedRSS = peakRSS()
        let cache = DSPSeekCache()
        let firstStart = ProcessInfo.processInfo.systemUptime
        let first = RealtimeMixer(
            session: session, assets: assets, startFrame: timelineFrames - 48000, seekCache: cache)
        let initialSeek = ProcessInfo.processInfo.systemUptime - firstStart
        XCTAssertEqual(first.position, timelineFrames - 48000)
        let cachedStart = ProcessInfo.processInfo.systemUptime
        let cached = RealtimeMixer(
            session: session, assets: assets, startFrame: timelineFrames - 24000, seekCache: cache)
        let cachedSeek = ProcessInfo.processInfo.systemUptime - cachedStart
        let restoredFrame = try XCTUnwrap(cached.restoredCheckpointFrame)
        XCTAssertGreaterThan(restoredFrame, 0)
        XCTAssertLessThanOrEqual(cached.seekWarmupFrames, cache.intervalFrames)
        XCTAssertLessThanOrEqual(cache.residentBytes, cache.maximumBytes)
        cached.prepareRealtimeSources(session: session, assets: assets)
        var left = Array(repeating: Float(0), count: 512)
        var right = left
        left.withUnsafeMutableBufferPointer { l in
            right.withUnsafeMutableBufferPointer { r in
                cached.render(intoLeft: l.baseAddress!, intoRight: r.baseAddress!, frameCount: 512)
            }
        }
        XCTAssertTrue(left.allSatisfy(\.isFinite))
        XCTAssertGreaterThan(left.map(abs).max()!, 0.01)
        XCTAssertEqual(cached.sourceReadUnderruns, 0)
        let cancelStart = ProcessInfo.processInfo.systemUptime
        let cancelSession = session
        let cancelAssets = assets
        let task = Task.detached {
            let mixer = RealtimeMixer(session: cancelSession, assets: cancelAssets, startFrame: timelineFrames - 1)
            try Task.checkCancellation()
            return mixer.position
        }
        try await Task.sleep(nanoseconds: 20_000_000)
        task.cancel()
        do {
            _ = try await task.value
            XCTFail("Cancelled preparation returned a completed mixer")
        } catch is CancellationError {}
        let cancellation = ProcessInfo.processInfo.systemUptime - cancelStart
        XCTAssertLessThan(cancellation, 2)
        XCTAssertLessThan(
            peakRSS() - baseline, 128 * 1024 * 1024, "Disk-backed seeks must stay below source-sized PCM residency")
        print(
            String(
                format:
                    "SOUNDROOM_STREAMED_BENCHMARK timeline_seconds=3600 tracks=2 clips=12 source_pcm_mib=%.2f setup_seconds=%.6f first_seek_seconds=%.6f cached_seek_seconds=%.6f checkpoint_frame=%d cached_warm_frames=%d checkpoint_mib=%.2f cancellation_ms=%.3f peak_rss_baseline_mib=%.2f peak_rss_mapped_mib=%.2f peak_rss_final_mib=%.2f",
                Double(sourceFrames * 16) / 1048576, setupSeconds, initialSeek, cachedSeek, restoredFrame,
                cached.seekWarmupFrames, Double(cache.residentBytes) / 1048576, cancellation * 1000,
                Double(baseline) / 1048576, Double(mappedRSS) / 1048576, Double(peakRSS()) / 1048576))
    }
    private func writeFixture(url: URL, frames: Int, period: Int) throws {
        let blockFrames = 8192
        let source = AudioSamples(channels: [
            (0..<blockFrames).map { (1 - 4 * abs(Float($0 % period) / Float(period) - 0.5)) * 0.15 },
            (0..<blockFrames).map { (1 - 4 * abs(Float($0 % period) / Float(period) - 0.5)) * 0.1 },
        ])
        let encoded = WaveFile.encode(source, float: true)
        var header = Data(encoded.prefix(44))
        func set(_ value: UInt32, at offset: Int) {
            var value = value.littleEndian
            withUnsafeBytes(of: &value) { header.replaceSubrange(offset..<(offset + 4), with: $0) }
        }
        set(UInt32(frames * 8 + 36), at: 4)
        set(UInt32(frames * 8), at: 40)
        try header.write(to: url)
        let output = try FileHandle(forWritingTo: url)
        defer { try? output.close() }
        try output.seekToEnd()
        let payload = Data(encoded.dropFirst(44))
        var remaining = frames
        while remaining > 0 {
            let count = min(remaining, blockFrames)
            try output.write(contentsOf: payload.prefix(count * 8))
            remaining -= count
        }
    }
    private func peakRSS() -> Int64 {
        var usage = rusage()
        getrusage(RUSAGE_SELF, &usage)
        return Int64(usage.ru_maxrss)
    }
}
