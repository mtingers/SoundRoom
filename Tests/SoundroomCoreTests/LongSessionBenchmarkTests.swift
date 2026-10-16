import XCTest
import Darwin
@testable import SoundroomCore

/// Opt-in release benchmark: SOUNDROOM_LONG_SESSION_BENCHMARK=1 swift test -c release
/// -debug-info-format none --filter LongSessionBenchmarkTests. Default runs skip the allocation and long seek.
final class LongSessionBenchmarkTests: XCTestCase {
    func testHourTimelineWithTwoTenMinuteStereoAssets() throws {
        guard ProcessInfo.processInfo.environment["SOUNDROOM_LONG_SESSION_BENCHMARK"] == "1" else {
            throw XCTSkip("Opt-in long-session memory/seek benchmark")
        }
        let seconds = 600, frames = Int(sessionRate) * seconds
        let timelineFrames = frames * 6, seekFrame = timelineFrames - Int(sessionRate)
        let baselineRSS = peakRSS(), allocateStart = ProcessInfo.processInfo.systemUptime
        // Non-silent stereo media: bounded triangles keep source creation cheap and
        // touch every allocated page. Separate arrays/assets avoid COW deduplication.
        let first = AudioSamples(channels: [triangle(frames, period: 109, amplitude: 0.2), triangle(frames, period: 173, amplitude: 0.15)])
        let second = AudioSamples(channels: [triangle(frames, period: 211, amplitude: 0.12), triangle(frames, period: 257, amplitude: 0.1)])
        let sourceIDs = [UUID(), UUID()], assets = [sourceIDs[0]: first, sourceIDs[1]: second]
        var session = Session(); session.tracks = []
        for index in 0..<2 {
            var track = Track(name: "Benchmark \(index + 1)", color: index)
            for repeatIndex in 0..<6 { track.clips.append(Clip(assetID: sourceIDs[index], name: "Ten-minute source", start: repeatIndex * frames, length: frames, edgeFadeFrames: 0)) }
            if index == 0 { track.realtimeEffects = [RealtimeEffect(kind: .lowPass, amount: 0.4)] }
            else { track.realtimeEffects = [RealtimeEffect(kind: .echo, amount: 0.35)] }
            session.tracks.append(track)
        }
        let allocationSeconds = ProcessInfo.processInfo.systemUptime - allocateStart
        let allocatedRSS = peakRSS(), seekStart = ProcessInfo.processInfo.systemUptime
        // Both effects require state history. This deliberately measures the current
        // full warm-up from zero, rather than an easier stateless late-timeline seek.
        let mixer = RealtimeMixer(session: session, assets: assets, startFrame: seekFrame)
        let seekSeconds = ProcessInfo.processInfo.systemUptime - seekStart
        XCTAssertEqual(mixer.position, seekFrame); XCTAssertEqual(mixer.end, timelineFrames)
        var left = Array(repeating: Float(0), count: 512), right = left
        let renderStart = ProcessInfo.processInfo.systemUptime
        left.withUnsafeMutableBufferPointer { l in right.withUnsafeMutableBufferPointer { r in
            mixer.render(intoLeft: l.baseAddress!, intoRight: r.baseAddress!, frameCount: 512)
        } }
        let firstBlockSeconds = ProcessInfo.processInfo.systemUptime - renderStart
        XCTAssertTrue(left.allSatisfy(\.isFinite)); XCTAssertTrue(right.allSatisfy(\.isFinite))
        XCTAssertGreaterThan(left.map(abs).max()!, 0.01)
        let updateStart = ProcessInfo.processInfo.systemUptime
        session.tracks[0].gain = 0.8; mixer.update(session: session, assets: assets)
        let updateSeconds = ProcessInfo.processInfo.systemUptime - updateStart
        let finalRSS = peakRSS()
        print(String(format: "SOUNDROOM_BENCHMARK timeline_seconds=3600 tracks=2 clips=12 unique_stereo_assets=2 source_seconds_each=600 sample_rate=48000 raw_pcm_mib=%.2f source_setup_seconds=%.6f stateful_seek_frame=%d stateful_seek_seconds=%.6f first_512_frame_render_ms=%.6f update_ms=%.6f peak_rss_baseline_mib=%.2f peak_rss_after_assets_mib=%.2f peak_rss_final_mib=%.2f", Double(frames * 2 * 2 * MemoryLayout<Float>.size) / 1_048_576, allocationSeconds, seekFrame, seekSeconds, firstBlockSeconds * 1000, updateSeconds * 1000, Double(baselineRSS) / 1_048_576, Double(allocatedRSS) / 1_048_576, Double(finalRSS) / 1_048_576))
        // No performance threshold: these are measurements, not promises for all hardware.
    }
    private func triangle(_ count: Int, period: Int, amplitude: Float) -> [Float] {
        var samples = Array(repeating: Float(0), count: count)
        for index in samples.indices {
            let phase = Float(index % period) / Float(period)
            samples[index] = (1 - 4 * abs(phase - 0.5)) * amplitude
        }
        return samples
    }
    private func peakRSS() -> Int64 {
        var usage = rusage(); getrusage(RUSAGE_SELF, &usage)
        // Darwin ru_maxrss reports bytes, unlike Linux's KiB.
        return Int64(usage.ru_maxrss)
    }
}
