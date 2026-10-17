import Foundation

/// Prepared once off the audio thread. Monotonic rendering advances in constant
/// time; a seek/new graph uses binary search instead of scanning a long Write curve.
final class AutomationCursor {
    private let points: [AutomationPoint]
    private let baseline: Float
    private let axisValues: [Float]
    private let valueScale: AutomationValueScale
    private var lower = -1
    private var previousFrame: Int?
    var hasPoints: Bool { !points.isEmpty }
    init(points: [AutomationPoint], baseline: Float, valueScale: AutomationValueScale = .linear) {
        var unique: [Int: AutomationPoint] = [:]
        unique.reserveCapacity(points.count)
        for point in points {
            let clean = point.relocated(frame: point.frame)
            unique[clean.frame] = clean
        }
        self.points = unique.values.sorted { $0.frame < $1.frame }
        self.axisValues = valueScale == .decibels ? self.points.map { valueScale.axisValue($0.value) } : []
        self.valueScale = valueScale
        self.baseline = baseline.isFinite ? max(0, min(1, baseline)) : 0.5
    }
    func value(at frame: Int) -> Float {
        guard !points.isEmpty else { return baseline }
        if previousFrame == nil || frame < previousFrame!
            || (lower + 2 < points.count && points[lower + 2].frame <= frame)
        {
            var lo = 0
            var hi = points.count
            while lo < hi {
                let middle = lo + (hi - lo) / 2
                if points[middle].frame <= frame { lo = middle + 1 } else { hi = middle }
            }
            lower = lo - 1
        } else {
            while lower + 1 < points.count, points[lower + 1].frame <= frame { lower += 1 }
        }
        previousFrame = frame
        guard lower >= 0 else { return baseline }
        guard lower + 1 < points.count else { return points[lower].value }
        let first = points[lower]
        let next = points[lower + 1]
        if first.value == next.value { return first.value }
        let fraction = AutomationInterpolation.fraction(first, next, at: frame)
        if valueScale == .linear { return first.value * (1 - fraction) + next.value * fraction }
        if fraction <= 0 { return first.value }
        if fraction >= 1 { return next.value }
        return valueScale.normalizedAxis(axisValues[lower] * (1 - fraction) + axisValues[lower + 1] * fraction)
    }
}
