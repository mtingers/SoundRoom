import Foundation
import SoundroomCore

/// Coarsens the entire input history rather than stretching only recent samples
/// across the take. Each bin covers the same number of input callbacks.
struct LivePeakHistory: Equatable {
    var channels: [[Float]] = []
    private var pending: [Float] = []
    private var pendingCount = 0
    private var callbacksPerBin = 1
    mutating func append(_ peaks: [Float]) {
        guard !peaks.isEmpty else { return }
        if channels.count != peaks.count {
            channels = peaks.map { _ in [] }
            pending = Array(repeating: 0, count: peaks.count)
            pendingCount = 0
            callbacksPerBin = 1
        }
        for c in peaks.indices { pending[c] = max(pending[c], peaks[c].isFinite ? abs(peaks[c]) : 0) }
        pendingCount += 1
        guard pendingCount >= callbacksPerBin else { return }
        for c in peaks.indices {
            channels[c].append(pending[c])
            pending[c] = 0
        }
        pendingCount = 0
        if (channels.first?.count ?? 0) >= 4096 {
            for c in channels.indices {
                channels[c] = stride(from: 0, to: channels[c].count - 1, by: 2).map {
                    max(channels[c][$0], channels[c][$0 + 1])
                }
            }
            callbacksPerBin *= 2
        }
    }
    func peaks(for track: Track) -> [Float] {
        peaks(channels: track.deviceInputChannels)
    }
    func channelPeaks(channels routed: [Int]) -> [[Float]] {
        routed.prefix(2).map { channels.indices.contains($0) ? channels[$0] : [] }
    }
    func peaks(channels routed: [Int]) -> [Float] {
        guard let first = routed.first, channels.indices.contains(first) else { return [] }
        var result = channels[first]
        for channel in routed.dropFirst() where channels.indices.contains(channel) {
            result = zip(result, channels[channel]).map { max($0, $1) }
        }
        return result
    }
}
