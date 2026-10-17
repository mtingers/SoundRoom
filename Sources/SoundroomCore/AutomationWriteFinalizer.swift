import Foundation

/// Worker input from an actual callback receipt. A requested UI value alone is
/// not an applied ramp and must never be supplied here.
public struct AutomationWriteRamp: Equatable, Sendable {
    public let frame: Int
    public let start: Float
    public let target: Float
    public let duration: Int
    public init(frame: Int, start: Float, target: Float, duration: Int) {
        self.frame = frame
        self.start = start
        self.target = target
        self.duration = duration
    }
}
public struct AutomationWriteRelease: Equatable, Sendable {
    public let lastFrame: Int
    public let lastValue: Float
    /// First actual lookup of the untouched original Read lane after release.
    public let nextReadFrame: Int
    public init(lastFrame: Int, lastValue: Float, nextReadFrame: Int) {
        self.lastFrame = lastFrame
        self.lastValue = lastValue
        self.nextReadFrame = nextReadFrame
    }
}
public enum AutomationWriteAccuracy: Sendable {
    /// Derive normalized tolerance conservatively from physical descriptor
    /// bounds, never from UI screen coordinates.
    case normalized(Float)
    case decibels(Float)
}
public enum AutomationWriteFinalizerError: Error, Equatable {
    case invalidCapture, captureLimit, pointLimit, workLimit, cancelled
}
/// Worker-only curve construction; does not authenticate, author, infer a clock,
/// open a device, or change a live processor. Keep the caller's full receipts
/// available for a reviewable draft if any bound or proof fails.
public enum AutomationWriteFinalizer {
    public static let maximumCapturedRamps = 65_536
    public static let maximumWrittenPoints = 4_096
    public static let maximumLanePoints = 100_000
    public static let maximumDurationFrames = 28_800_000
    private static let maximumWork = 8_000_000
    private static let maximumFrame = 4_147_200_000
    public static func finalize(
        original: [AutomationPoint], baseline: Float, scale: AutomationValueScale,
        ramps: [AutomationWriteRamp], released: AutomationWriteRelease?,
        accuracy: AutomationWriteAccuracy, cancelled: () -> Bool = { false }
    ) throws -> [AutomationPoint] {
        guard !ramps.isEmpty else { return original }
        guard ramps.count <= maximumCapturedRamps, original.count <= maximumLanePoints else {
            throw AutomationWriteFinalizerError.captureLimit
        }
        guard Set(original.map(\.frame)).count == original.count,
            valid(baseline),
            original.allSatisfy({
                valid($0.value) && $0.frame >= 0 && $0.frame <= maximumFrame && valid($0.curveStart)
                    && valid($0.curveEnd) && $0.curveStart <= $0.curveEnd
            }),
            ramps.allSatisfy({
                valid($0.start) && valid($0.target) && $0.frame >= 0 && (1...480).contains($0.duration)
                    && $0.frame <= maximumFrame - $0.duration
            }),
            zip(ramps, ramps.dropFirst()).allSatisfy({ $0.frame < $1.frame }),
            let released, valid(released.lastValue), released.lastFrame >= ramps.last!.frame,
            released.lastFrame < maximumFrame, released.nextReadFrame == released.lastFrame + 1,
            released.nextReadFrame - ramps[0].frame <= maximumDurationFrames
        else { throw AutomationWriteFinalizerError.invalidCapture }
        let tolerance: Float
        switch accuracy {
        case .normalized(let value):
            guard value.isFinite, value > 0, value <= 1 else { throw AutomationWriteFinalizerError.invalidCapture }
            tolerance = scale == .decibels ? value / (log(10) / 20) : value
        case .decibels(let value):
            guard scale == .decibels, value.isFinite, value > 0, value <= 1 else {
                throw AutomationWriteFinalizerError.invalidCapture
            }
            tolerance = value
        }
        var raw: [Int: Float] = [:]
        for (index, ramp) in ramps.enumerated() {
            if cancelled() { throw AutomationWriteFinalizerError.cancelled }
            let until = index + 1 < ramps.count ? ramps[index + 1].frame : released.lastFrame
            raw[ramp.frame] = ramp.start
            let end = min(ramp.frame + ramp.duration, until)
            raw[end] = scale.interpolate(
                ramp.start, ramp.target, fraction: Float(end - ramp.frame) / Float(ramp.duration))
        }
        raw = raw.filter { $0.key <= released.lastFrame }
        raw[released.lastFrame] = released.lastValue
        let first = ramps[0].frame
        raw[released.nextReadFrame] = AutomationInterpolation.value(
            original, at: released.nextReadFrame, baseline: baseline, scale: scale)
        let reduced = try simplify(
            raw.sorted { $0.key < $1.key }.map { ($0.key, $0.value) }, scale: scale, tolerance: tolerance,
            cancelled: cancelled)
        guard reduced.count - 1 <= maximumWrittenPoints else { throw AutomationWriteFinalizerError.pointLimit }
        var indexed: [Int: AutomationPoint] = [:]
        for point in original { indexed[point.frame] = point }
        var result = indexed.values.sorted { $0.frame < $1.frame }
        if first > 0 { insertBoundary(&result, at: first - 1, baseline: baseline, scale: scale) }
        insertBoundary(&result, at: released.nextReadFrame, baseline: baseline, scale: scale)
        result.removeAll { $0.frame >= first && $0.frame < released.nextReadFrame }
        result += reduced.filter { $0.0 < released.nextReadFrame }.map { AutomationPoint(frame: $0.0, value: $0.1) }
        result.sort { $0.frame < $1.frame }
        guard result.count <= maximumLanePoints else { throw AutomationWriteFinalizerError.pointLimit }
        return result
    }
    private static func valid(_ value: Float) -> Bool { value.isFinite && (0...1).contains(value) }
    private static func insertBoundary(
        _ points: inout [AutomationPoint], at frame: Int, baseline: Float, scale: AutomationValueScale
    ) {
        guard !points.contains(where: { $0.frame == frame }) else { return }
        let index = points.firstIndex(where: { $0.frame > frame }) ?? points.count
        let value = AutomationInterpolation.value(points, at: frame, baseline: baseline, scale: scale)
        var inserted = AutomationPoint(frame: frame, value: value)
        if index == 0 && !points.isEmpty {
            inserted.curve = .hold
        } else if index > 0 && index < points.count {
            let lower = points[index - 1]
            let middle =
                lower.curveStart + (lower.curveEnd - lower.curveStart) * Float(frame - lower.frame)
                / Float(points[index].frame - lower.frame)
            inserted.curve = lower.curve
            inserted.curveStart = middle
            inserted.curveEnd = lower.curveEnd
            points[index - 1].curveEnd = middle
        }
        points.insert(inserted, at: index)
    }
    private static func simplify(
        _ points: [(Int, Float)], scale: AutomationValueScale, tolerance: Float,
        cancelled: () -> Bool
    ) throws -> [(Int, Float)] {
        guard points.count > 2 else { return points }
        var keep = Array(repeating: false, count: points.count)
        keep[0] = true
        keep[points.count - 1] = true
        if scale == .decibels {
            for i in points.indices where points[i].1 == 0 {
                keep[i] = true
                if i > 0 { keep[i - 1] = true }
                if i + 1 < points.count { keep[i + 1] = true }
            }
        }
        let required = keep.indices.filter { keep[$0] }
        var stack = Array(zip(required, required.dropFirst()))
        var work = 0
        while let (first, last) = stack.popLast() {
            if cancelled() { throw AutomationWriteFinalizerError.cancelled }
            guard last > first + 1 else { continue }
            var worst = tolerance
            var split: Int?
            for i in (first + 1)..<last {
                work += 1
                if work > maximumWork { throw AutomationWriteFinalizerError.workLimit }
                if work % 256 == 0 && cancelled() { throw AutomationWriteFinalizerError.cancelled }
                let fraction = Double(points[i].0 - points[first].0) / Double(points[last].0 - points[first].0)
                let prediction =
                    Double(scale.axisValue(points[first].1)) * (1 - fraction) + Double(scale.axisValue(points[last].1))
                    * fraction
                let error = Float(abs(Double(scale.axisValue(points[i].1)) - prediction))
                if error > worst {
                    worst = error
                    split = i
                }
            }
            if let split {
                keep[split] = true
                stack.append((first, split))
                stack.append((split, last))
            }
        }
        return points.indices.filter { keep[$0] }.map { points[$0] }
    }
}
