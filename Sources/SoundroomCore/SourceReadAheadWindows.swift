import Foundation

/// Source-time windows for a latency-compensated looping track. Worker-only.
/// Pin the actual loop beginning and split future reads at the exclusive end.
enum SourceReadAheadWindows {
    static func ranges(outputFrame: Int, latency: Int, horizon: Int, loop: Range<Int>?) -> [Range<Int>] {
        guard horizon > 0 else { return [] }
        guard let loop, !loop.isEmpty, loop.lowerBound >= 0 else {
            let shifted = outputFrame.addingReportingOverflow(max(0, latency))
            let start = shifted.overflow ? Int.max : shifted.partialValue
            let end = start.addingReportingOverflow(horizon)
            let upper = end.overflow ? Int.max : end.partialValue
            return upper > start ? [start..<upper] : []
        }
        let width = loop.count
        if width <= horizon { return [loop] }
        let frameRemainder = outputFrame % width
        let normalizedFrame = frameRemainder < 0 ? frameRemainder + width : frameRemainder
        let lowerRemainder = loop.lowerBound % width
        let normalized =
            normalizedFrame >= lowerRemainder
            ? normalizedFrame - lowerRemainder : width - (lowerRemainder - normalizedFrame)
        let offset = max(0, latency) % width
        // Modular addition avoids overflowing when a loop ends near Int.max.
        let shifted = normalized >= width - offset ? normalized - (width - offset) : normalized + offset
        let start = loop.lowerBound + shifted
        let first = min(horizon, loop.upperBound - start)
        var ranges = [start..<(start + first)]
        if first < horizon { ranges.append(loop.lowerBound..<(loop.lowerBound + horizon - first)) }
        let pin = loop.lowerBound..<(loop.lowerBound + horizon)
        if !ranges.contains(pin) { ranges.append(pin) }
        return ranges
    }
}
