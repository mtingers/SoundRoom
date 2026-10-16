import Foundation

public enum AutomationCurve: String, Codable, CaseIterable, Identifiable, Sendable {
    case linear, smooth, easeIn, easeOut, hold
    public var id: Self { self }
    public var name: String {
        switch self { case .linear: return "Linear"; case .smooth: return "Smooth"; case .easeIn: return "Ease in"; case .easeOut: return "Ease out"; case .hold: return "Hold" }
    }
    public func fraction(_ fraction: Float) -> Float {
        let f = max(0, min(1, fraction))
        switch self {
        case .linear: return f
        case .smooth: return f * f * (3 - 2 * f)
        case .easeIn: return f * f
        case .easeOut: return 1 - (1 - f) * (1 - f)
        case .hold: return f < 1 ? 0 : 1
        }
    }
}
public enum AutomationValueScale: String, Codable, CaseIterable, Identifiable, Sendable {
    case linear, decibels
    public var id: Self { self }
    public var name: String { self == .linear ? "Linear gain" : "Decibels" }
    /// A normalized zero is true silence at the endpoint. Inside a dB ramp it uses
    /// a -80 dB floor, avoiding log(0) and allowing a musically even fade to silence.
    public func axisValue(_ value: Float) -> Float {
        self == .linear ? value : 20 * log10(max(0.0001, value * 1.5))
    }
    public func normalizedAxis(_ value: Float) -> Float {
        self == .linear ? value : max(0, min(1, pow(10, value / 20) / 1.5))
    }
    public func interpolate(_ first: Float, _ last: Float, fraction: Float) -> Float {
        if first == last { return first }
        guard fraction > 0 else { return first }
        guard fraction < 1 else { return last }
        return normalizedAxis(axisValue(first) * (1 - fraction) + axisValue(last) * fraction)
    }
}
public enum AutomationInterpolation {
    public static func fraction(_ first: AutomationPoint, _ last: AutomationPoint, at frame: Int) -> Float {
        guard last.frame > first.frame else { return 0 }
        let position = max(0, min(1, Float(frame - first.frame) / Float(last.frame - first.frame)))
        if first.curve == .linear { return position }
        if first.curveStart == 0 && first.curveEnd == 1 { return first.curve.fraction(position) }
        let start = first.curve.fraction(first.curveStart), end = first.curve.fraction(first.curveEnd)
        let raw = first.curve.fraction(first.curveStart + (first.curveEnd - first.curveStart) * position)
        return abs(end - start) > 0.000001 ? (raw - start) / (end - start) : position
    }
    public static func between(_ first: AutomationPoint, _ last: AutomationPoint, at frame: Int, scale: AutomationValueScale = .linear) -> Float {
        scale.interpolate(first.value, last.value, fraction: fraction(first, last, at: frame))
    }
    public static func value(_ points: [AutomationPoint], at frame: Int, baseline: Float, scale: AutomationValueScale = .linear) -> Float {
        var lower: AutomationPoint?, upper: AutomationPoint?
        for point in points {
            if point.frame <= frame { if lower == nil || point.frame >= lower!.frame { lower = point } }
            else if upper == nil || point.frame < upper!.frame { upper = point }
        }
        guard let lower else { return baseline }
        guard let upper else { return lower.value }
        return between(lower, upper, at: frame, scale: scale)
    }
}

public enum AutomationSimplifier {
    /// Ramer-Douglas-Peucker in time/value coordinates. Keeps every nonlinear segment,
    /// endpoints and duplicate-time resolution. Linear curves stay within tolerance
    /// at every original knot and between knots in the selected interpolation domain.
    public static func simplify(_ points: [AutomationPoint], tolerance: Float, scale: AutomationValueScale = .linear) -> [AutomationPoint] {
        var unique: [Int: AutomationPoint] = [:]
        for point in points { unique[point.frame] = point }
        let sorted = unique.values.sorted { $0.frame < $1.frame }
        guard sorted.count > 2, tolerance.isFinite, tolerance > 0 else { return sorted }
        var keep: Set<Int> = [0, sorted.count - 1]
        if scale == .decibels {
            for i in sorted.indices where sorted[i].value == 0 { keep.insert(i) }
        }
        for i in 0..<(sorted.count - 1) where sorted[i].curve != .linear { keep.insert(i); keep.insert(i + 1) }
        let required = keep.sorted()
        var stack: [(Int, Int)] = zip(required, required.dropFirst()).map { ($0, $1) }
        while let (first, last) = stack.popLast() {
            guard last - first > 1 else { continue }
            var maximum: Float = 0; var chosen = first
            let a = scale.axisValue(sorted[first].value), b = scale.axisValue(sorted[last].value)
            for i in (first + 1)..<last {
                let fraction = Float(sorted[i].frame - sorted[first].frame) / Float(sorted[last].frame - sorted[first].frame)
                let error = abs(scale.axisValue(sorted[i].value) - (a + (b - a) * fraction))
                if error > maximum { maximum = error; chosen = i }
            }
            if maximum > tolerance { keep.insert(chosen); stack.append((first, chosen)); stack.append((chosen, last)) }
        }
        return keep.sorted().map { sorted[$0] }
    }
}


public extension AutomationPoint {
    func relocated(frame: Int, value: Float? = nil) -> AutomationPoint {
        AutomationPoint(frame: frame, value: value ?? self.value, curve: curve, curveStart: curveStart, curveEnd: curveEnd)
    }
}
public enum AutomationCurveEditing {
    /// Shift a curve and preserve its original nonlinear domain when clipping
    /// before time zero. Nil indicates an integer overflow, before any mutation.
    public static func relocating(_ points: [AutomationPoint], by shift: Int, baseline: Float, scale: AutomationValueScale = .linear) -> [AutomationPoint]? {
        guard shift != Int.min else { return nil }
        let retained = shift < 0 && points.contains(where: { $0.frame < -shift }) ?
            insertingBoundary(points, at: -shift, baseline: baseline, scale: scale) : points
        var result: [AutomationPoint] = []
        for point in retained {
            let (frame, overflow) = point.frame.addingReportingOverflow(shift)
            guard !overflow else { return nil }
            if frame >= 0 { result.append(point.relocated(frame: frame)) }
        }
        return result.sorted { $0.frame < $1.frame }
    }
    /// Split a nonlinear transition without changing any samples outside the edit.
    /// The outgoing point retains the original curve's fractional domain.
    public static func insertingBoundary(_ points: [AutomationPoint], at frame: Int, baseline: Float, scale: AutomationValueScale = .linear) -> [AutomationPoint] {
        var sorted = points.sorted { $0.frame < $1.frame }
        guard !sorted.contains(where: { $0.frame == frame }) else { return sorted }
        let value = AutomationInterpolation.value(sorted, at: frame, baseline: baseline, scale: scale)
        guard let lower = sorted.lastIndex(where: { $0.frame < frame }), lower + 1 < sorted.count else {
            sorted.append(.init(frame: frame, value: value)); return sorted.sorted { $0.frame < $1.frame }
        }
        let original = sorted[lower], next = sorted[lower + 1]
        let fraction = Float(frame - original.frame) / Float(next.frame - original.frame)
        let middle = original.curveStart + (original.curveEnd - original.curveStart) * fraction
        sorted[lower].curveEnd = middle
        sorted.insert(.init(frame: frame, value: value, curve: original.curve, curveStart: middle, curveEnd: original.curveEnd), at: lower + 1)
        return sorted
    }
}
