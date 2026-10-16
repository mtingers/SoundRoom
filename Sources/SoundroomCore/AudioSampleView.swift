import Foundation

/// Immutable channel/range view retains its source without copying a long take.
/// Leading alignment silence is virtual; saving/exporting writes bounded blocks.
final class AudioSampleView: @unchecked Sendable {
    let source: AudioSamples
    let channels: [Int]
    let start: Int
    let frames: Int
    let padding: Int
    init(source: AudioSamples, channels: [Int], range: Range<Int>, padding: Int) {
        self.source = source; self.channels = channels; start = range.lowerBound
        frames = range.count + padding; self.padding = padding
    }
    func sample(channel: Int, frame: Int) -> Float {
        guard channels.indices.contains(channel), frame >= padding, frame < frames else { return 0 }
        return source.sample(channel: channels[channel], frame: start + frame - padding)
    }
    func readFrames(_ range: Range<Int>, channels indices: [Int]) -> [[Float]] {
        var output = indices.map { _ in Array(repeating: Float(0), count: range.count) }
        let lo = max(padding, range.lowerBound), hi = min(frames, range.upperBound)
        guard hi > lo else { return output }
        let valid = indices.map { channels.indices.contains($0) ? channels[$0] : -1 }
        let read = source.readFrames((start + lo - padding)..<(start + hi - padding), channels: valid)
        for channel in output.indices { output[channel].replaceSubrange((lo - range.lowerBound)..<(hi - range.lowerBound), with: read[channel]) }
        return output
    }
}

extension AudioSamples {
    public func view(range: Range<Int>, channels: [Int], padding: Int = 0) -> AudioSamples? {
        guard range.lowerBound >= 0, range.upperBound <= count, !range.isEmpty, padding >= 0,
              padding <= Int.max - range.count, !channels.isEmpty,
              channels.allSatisfy({ $0 >= 0 && $0 < channelCount }) else { return nil }
        return AudioSamples(view: AudioSampleView(source: self, channels: channels, range: range, padding: padding))
    }
}
