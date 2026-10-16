import XCTest
@testable import SoundroomCore

final class OfflineEffectBlockTests: XCTestCase {
    func testBoundedOfflineReadsMatchContinuousReferenceIncludingLatencyAndTails() throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        for channelCount in [1, 2, 3] {
            let input = AudioSamples(channels: (0..<channelCount).map { channel in
                (0..<17_123).map { sin(Float($0) * Float(channel + 1) * 0.0288) * 0.17 }
            })
            let url = folder.appendingPathComponent("\(channelCount).wav")
            let source: AudioSamples
            if channelCount <= 2 {
                try WaveFile.encode(input, float: true).write(to: url)
                source = try AudioSamples(mappedWaveFile: url)
            } else { source = input } // WAV export is mono/stereo; resident odd pairs are also supported.
            for kind in RealtimeEffectKind.allCases {
                var parameters = kind.defaultParameters
                if kind == .room { parameters["decay"] = 0.1 }
                let effect = RealtimeEffect(kind: kind, parameters: parameters)
                for tail in [false, true] {
                    let expected = reference(input, effect: effect, includeTail: tail)
                    let actual = effect.process(source, includeTail: tail)
                    XCTAssertEqual(actual.channelCount, channelCount, "\(kind) tail=\(tail)")
                    XCTAssertEqual(actual.count, expected.count, "\(kind) tail=\(tail)")
                    XCTAssertTrue(actual.channels == expected.channels, "\(kind) channels=\(channelCount) tail=\(tail)")
                }
            }
        }
    }
    func testResidentBlockReadsPreserveSilenceOutsideSourceAndInvalidChannels() {
        let audio = AudioSamples(channels: [[0.1, 0.2, 0.3], [0.4, 0.5, 0.6]])
        XCTAssertEqual(audio.readFrames(1..<3), [[0.2, 0.3], [0.5, 0.6]])
        XCTAssertEqual(audio.readFrames(-1..<4, channels: [0, 9]), [[0, 0.1, 0.2, 0.3, 0], [0, 0, 0, 0, 0]])
    }
    func testProgressIsBoundedMonotonicAndCompletesForOddChannelPairs() {
        let input = AudioSamples(channels: Array(repeating: Array(repeating: Float(0.1), count: 180_001), count: 3))
        let recorder = ProgressRecorder()
        _ = RealtimeEffect(kind: .echo).process(input, progress: { recorder.append($0) })
        let values = recorder.values
        XCTAssertEqual(values.last, 1)
        XCTAssertGreaterThan(values.count, 2); XCTAssertLessThanOrEqual(values.count, 102)
        XCTAssertTrue(values.allSatisfy { (0...1).contains($0) })
        XCTAssertTrue(zip(values, values.dropFirst()).allSatisfy { $0 <= $1 })
    }
    func testCancellationDuringProcessingReturnsOriginalRatherThanPartialAudio() async {
        let input = AudioSamples(channels: [Array(repeating: Float(0.1), count: 200_000)])
        let result = await Task.detached {
            RealtimeEffect(kind: .room).process(input, progress: { fraction in
                if fraction > 0.05 { withUnsafeCurrentTask { $0?.cancel() } }
            })
        }.value
        XCTAssertEqual(result.cacheIdentity, input.cacheIdentity)
    }
    private func reference(_ audio: AudioSamples, effect: RealtimeEffect, includeTail: Bool) -> AudioSamples {
        let outputCount = audio.count + (includeTail ? effect.tailFrames : 0), latency = effect.latencyFrames
        let input = audio.channels
        var output: [[Float]] = []
        for pair in stride(from: 0, to: input.count, by: 2) {
            var left = input[pair] + Array(repeating: Float(0), count: outputCount + latency - audio.count)
            var right = input[min(pair + 1, input.count - 1)] + Array(repeating: Float(0), count: outputCount + latency - audio.count)
            let processor = EffectProcessor(effect: effect)
            left.withUnsafeMutableBufferPointer { l in right.withUnsafeMutableBufferPointer { r in
                for offset in stride(from: 0, to: l.count, by: 8192) {
                    processor.process(left: l.baseAddress!.advanced(by: offset), right: r.baseAddress!.advanced(by: offset), count: min(8192, l.count - offset), position: offset)
                }
            } }
            output.append(Array(left[latency..<(latency + outputCount)]))
            if pair + 1 < input.count { output.append(Array(right[latency..<(latency + outputCount)])) }
        }
        return AudioSamples(channels: output)
    }
}

private final class ProgressRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var recorded: [Double] = []
    func append(_ value: Double) { lock.lock(); recorded.append(value); lock.unlock() }
    var values: [Double] { lock.lock(); defer { lock.unlock() }; return recorded }
}
