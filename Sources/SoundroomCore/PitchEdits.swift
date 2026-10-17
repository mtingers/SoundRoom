import Foundation

/// A target in source timeline frames. Notes retain fractional semitones; the
/// outgoing interpolation belongs to this point, as with ordinary automation.
public struct PitchEditPoint: Codable, Equatable, Sendable, Identifiable {
    public var id: UUID
    public var frame: Int
    public var note: Float
    public var curve: AutomationCurve
    public var curveStart: Float
    public var curveEnd: Float
    public init(
        id: UUID = UUID(), frame: Int, note: Float, curve: AutomationCurve = .linear,
        curveStart: Float = 0, curveEnd: Float = 1
    ) {
        self.id = id
        self.frame = PitchEditRange.clampedFrame(frame)
        self.note = note.isFinite ? max(0, min(95, note)) : 69
        self.curve = curve
        self.curveStart = curveStart.isFinite ? max(0, min(1, curveStart)) : 0
        self.curveEnd = curveEnd.isFinite ? max(self.curveStart, min(1, curveEnd)) : 1
    }
    private enum CodingKeys: String, CodingKey { case id, frame, note, curve, curveStart, curveEnd }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            id: try c.decodeIfPresent(UUID.self, forKey: .id) ?? UUID(),
            frame: try c.decode(Int.self, forKey: .frame), note: try c.decode(Float.self, forKey: .note),
            curve: try c.decodeIfPresent(AutomationCurve.self, forKey: .curve) ?? .linear,
            curveStart: try c.decodeIfPresent(Float.self, forKey: .curveStart) ?? 0,
            curveEnd: try c.decodeIfPresent(Float.self, forKey: .curveEnd) ?? 1)
    }
    var automationPoint: AutomationPoint {
        .init(frame: frame, value: note / 95, curve: curve, curveStart: curveStart, curveEnd: curveEnd)
    }
}

/// Half-open source timeline range, also used for latency-aligned baked bypass.
public struct PitchEditRange: Codable, Equatable, Sendable {
    public var startFrame: Int
    public var endFrame: Int
    public var range: Range<Int> { startFrame..<max(startFrame, endFrame) }
    public init(startFrame: Int, endFrame: Int) {
        self.startFrame = Self.clampedFrame(startFrame)
        self.endFrame = max(self.startFrame, Self.clampedFrame(endFrame))
    }
    public init(_ range: Range<Int>) { self.init(startFrame: range.lowerBound, endFrame: range.upperBound) }
    // Leave ample headroom for latency, frame differences and export arithmetic.
    static func clampedFrame(_ frame: Int) -> Int { max(0, min(Int.max / 4, frame)) }
    private enum CodingKeys: String, CodingKey { case startFrame, endFrame }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            startFrame: try c.decode(Int.self, forKey: .startFrame), endFrame: try c.decode(Int.self, forKey: .endFrame)
        )
    }
}

public struct PitchEditRegion: Codable, Equatable, Sendable, Identifiable {
    public var id: UUID
    public var startFrame: Int
    public var endFrame: Int
    public var points: [PitchEditPoint]
    public var range: Range<Int> { PitchEditRange(startFrame: startFrame, endFrame: endFrame).range }
    public init(id: UUID = UUID(), startFrame: Int, endFrame: Int, points: [PitchEditPoint]) {
        self.id = id
        let scope = PitchEditRange(startFrame: startFrame, endFrame: endFrame)
        self.startFrame = scope.startFrame
        self.endFrame = scope.endFrame
        var unique: [Int: PitchEditPoint] = [:]
        for point in points {
            var clean = PitchEditPoint(
                id: point.id, frame: point.frame, note: point.note, curve: point.curve,
                curveStart: point.curveStart, curveEnd: point.curveEnd)
            clean.frame = max(scope.startFrame, min(scope.endFrame, clean.frame))
            unique[clean.frame] = clean
        }
        self.points = unique.values.sorted { $0.frame < $1.frame }
    }
    public init(id: UUID = UUID(), range: Range<Int>, points: [PitchEditPoint]) {
        self.init(id: id, startFrame: range.lowerBound, endFrame: range.upperBound, points: points)
    }
    private enum CodingKeys: String, CodingKey { case id, startFrame, endFrame, points }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            id: try c.decodeIfPresent(UUID.self, forKey: .id) ?? UUID(),
            startFrame: try c.decode(Int.self, forKey: .startFrame),
            endFrame: try c.decode(Int.self, forKey: .endFrame),
            points: try c.decodeIfPresent([PitchEditPoint].self, forKey: .points) ?? [])
    }
    /// Suitable for UI inspection; playback uses a prepared, indexed cursor.
    public func note(at frame: Int) -> Float? {
        guard range.contains(frame), let first = points.first else { return nil }
        return AutomationInterpolation.value(points.map(\.automationPoint), at: frame, baseline: first.note / 95) * 95
    }
    /// Cut both ends without changing the retained curve, including nonlinear domains.
    public func sliced(to requested: Range<Int>, id: UUID? = nil) -> PitchEditRegion? {
        let start = max(startFrame, requested.lowerBound)
        let end = min(endFrame, requested.upperBound)
        guard start < end, let first = points.first else { return nil }
        if start == startFrame && end == endFrame {
            var unchanged = self
            if let id { unchanged.id = id }
            return unchanged
        }
        var curve = points.map(\.automationPoint)
        curve = AutomationCurveEditing.insertingBoundary(curve, at: start, baseline: first.note / 95)
        curve = AutomationCurveEditing.insertingBoundary(curve, at: end, baseline: first.note / 95)
        let sliced = curve.filter { $0.frame >= start && $0.frame <= end }.map { point in
            PitchEditPoint(
                id: points.first(where: { $0.frame == point.frame })?.id ?? UUID(), frame: point.frame,
                note: point.value * 95, curve: point.curve, curveStart: point.curveStart, curveEnd: point.curveEnd)
        }
        return .init(id: id ?? self.id, startFrame: start, endFrame: end, points: sliced)
    }
}

public enum PitchEditRegions {
    /// Later input regions replace earlier overlaps. Retained fragments preserve
    /// their exact target interpolation and never bridge an unvoiced gap.
    public static func sanitized(_ regions: [PitchEditRegion]) -> [PitchEditRegion] {
        let clean = regions.map {
            PitchEditRegion(id: $0.id, startFrame: $0.startFrame, endFrame: $0.endFrame, points: $0.points)
        }
        .filter { $0.startFrame < $0.endFrame && !$0.points.isEmpty }
        let sorted = clean.sorted { $0.startFrame < $1.startFrame }
        // Analyzed voiced runs and normal editor data are already disjoint. Avoid
        // repeated insertion scans when preparing thousands of short phrases.
        if zip(sorted, sorted.dropFirst()).allSatisfy({ pair in pair.0.endFrame <= pair.1.startFrame }) {
            return uniquelyIdentified(sorted)
        }
        var result: [PitchEditRegion] = []
        for region in clean { result = replacingClean(result, with: region) }
        return uniquelyIdentified(result)
    }
    public static func replacing(_ regions: [PitchEditRegion], with replacement: PitchEditRegion) -> [PitchEditRegion] {
        uniquelyIdentified(replacingClean(sanitized(regions), with: replacement))
    }
    private static func replacingClean(_ regions: [PitchEditRegion], with replacement: PitchEditRegion)
        -> [PitchEditRegion]
    {
        let clean = PitchEditRegion(
            id: replacement.id, startFrame: replacement.startFrame, endFrame: replacement.endFrame,
            points: replacement.points)
        guard clean.startFrame < clean.endFrame, !clean.points.isEmpty else { return regions }
        var output: [PitchEditRegion] = []
        for region in regions {
            if region.endFrame <= clean.startFrame || region.startFrame >= clean.endFrame {
                output.append(region)
                continue
            }
            if region.startFrame < clean.startFrame,
                let before = region.sliced(to: region.startFrame..<clean.startFrame)
            {
                output.append(before)
            }
            if clean.endFrame < region.endFrame,
                let after = region.sliced(
                    to: clean.endFrame..<region.endFrame,
                    id: region.startFrame < clean.startFrame ? UUID() : region.id)
            {
                output.append(after)
            }
        }
        output.append(clean)
        return output.sorted { $0.startFrame < $1.startFrame }
    }
    private static func uniquelyIdentified(_ regions: [PitchEditRegion]) -> [PitchEditRegion] {
        var regionIDs: Set<UUID> = []
        var pointIDs: Set<UUID> = []
        return regions.map { original in
            var region = original
            if !regionIDs.insert(region.id).inserted {
                region.id = UUID()
                regionIDs.insert(region.id)
            }
            region.points = region.points.map { original in
                var point = original
                if !pointIDs.insert(point.id).inserted {
                    point.id = UUID()
                    pointIDs.insert(point.id)
                }
                return point
            }
            return region
        }
    }
    public static func slicing(_ regions: [PitchEditRegion], to range: Range<Int>) -> [PitchEditRegion] {
        sanitized(regions).compactMap { $0.sliced(to: range) }
    }
    public static func removing(_ regions: [PitchEditRegion], in range: Range<Int>) -> [PitchEditRegion] {
        let removed = PitchEditRange(range)
        guard removed.startFrame < removed.endFrame else { return sanitized(regions) }
        var result: [PitchEditRegion] = []
        for region in sanitized(regions) {
            if region.endFrame <= removed.startFrame || region.startFrame >= removed.endFrame {
                result.append(region)
                continue
            }
            let hasBefore = region.startFrame < removed.startFrame
            if hasBefore, let before = region.sliced(to: region.startFrame..<removed.startFrame) {
                result.append(before)
            }
            if removed.endFrame < region.endFrame,
                let after = region.sliced(to: removed.endFrame..<region.endFrame, id: hasBefore ? UUID() : region.id)
            {
                result.append(after)
            }
        }
        return uniquelyIdentified(result)
    }
    public static func removing(_ ranges: [PitchEditRange], in range: Range<Int>) -> [PitchEditRange] {
        let removed = PitchEditRange(range)
        var result: [PitchEditRange] = []
        for scope in sanitizedRanges(ranges) {
            if scope.endFrame <= removed.startFrame || scope.startFrame >= removed.endFrame
                || removed.startFrame == removed.endFrame
            {
                result.append(scope)
                continue
            }
            if scope.startFrame < removed.startFrame {
                result.append(.init(startFrame: scope.startFrame, endFrame: removed.startFrame))
            }
            if removed.endFrame < scope.endFrame {
                result.append(.init(startFrame: removed.endFrame, endFrame: scope.endFrame))
            }
        }
        return result
    }
    /// Move source-timed edits. At timeline bounds, slice the original curve
    /// before translating; clamping individual anchors would distort its shape.
    public static func relocated(_ regions: [PitchEditRegion], by delta: Int, renewIDs: Bool = false)
        -> [PitchEditRegion]
    {
        let limit = Int.max / 4
        guard delta > -limit && delta < limit else { return [] }
        let lower = delta < 0 ? -delta : 0
        let upper = delta > 0 ? limit - delta : limit
        return sanitized(regions).compactMap { region in
            let retained: PitchEditRegion
            if region.startFrame >= lower && region.endFrame <= upper {
                retained = region
            } else {
                guard let slice = region.sliced(to: lower..<upper) else { return nil }
                retained = slice
            }
            let points = retained.points.map { point in
                PitchEditPoint(
                    id: renewIDs ? UUID() : point.id, frame: point.frame + delta, note: point.note,
                    curve: point.curve, curveStart: point.curveStart, curveEnd: point.curveEnd)
            }
            return PitchEditRegion(
                id: renewIDs ? UUID() : retained.id, startFrame: retained.startFrame + delta,
                endFrame: retained.endFrame + delta, points: points)
        }
    }
    public static func relocated(_ ranges: [PitchEditRange], by delta: Int) -> [PitchEditRange] {
        let limit = Int.max / 4
        guard delta > -limit && delta < limit else { return [] }
        let lower = delta < 0 ? -delta : 0
        let upper = delta > 0 ? limit - delta : limit
        return sanitizedRanges(ranges).compactMap { range in
            let start = max(lower, range.startFrame)
            let end = min(upper, range.endFrame)
            guard start < end else { return nil }
            return .init(startFrame: start + delta, endFrame: end + delta)
        }
    }
    public static func relocatedRanges(_ ranges: [PitchEditRange], by delta: Int) -> [PitchEditRange] {
        relocated(ranges, by: delta)
    }
    public static func sanitizedRanges(_ ranges: [PitchEditRange]) -> [PitchEditRange] {
        let sorted = ranges.map { PitchEditRange(startFrame: $0.startFrame, endFrame: $0.endFrame) }
            .filter { $0.startFrame < $0.endFrame }.sorted { $0.startFrame < $1.startFrame }
        var output: [PitchEditRange] = []
        for range in sorted {
            if let last = output.last, range.startFrame <= last.endFrame {
                output[output.count - 1].endFrame = max(last.endFrame, range.endFrame)
            } else {
                output.append(range)
            }
        }
        return output
    }
}

/// Built off the callback. A block jump or backwards loop seek performs a binary
/// lookup; adjacent samples reuse the current region and automation segment.
final class PitchEditCursor {
    struct Sample {
        var note: Float
        var scopeBlend: Float
    }
    static let boundaryRampFrames = Int(sessionRate * 0.005)
    private let regions: [PitchEditRegion]
    private let cursors: [AutomationCursor]
    private let gateStarts: [Int]
    private let gateEnds: [Int]
    private var current = -1
    init(regions: [PitchEditRegion]) {
        self.regions = PitchEditRegions.sanitized(regions)
        cursors = self.regions.map { region in
            AutomationCursor(
                points: region.points.map(\.automationPoint), baseline: (region.points.first?.note ?? 69) / 95)
        }
        var starts = [Int](repeating: 0, count: self.regions.count)
        var ends = starts
        var first = 0
        while first < self.regions.count {
            var last = first
            while last + 1 < self.regions.count {
                let before = self.regions[last]
                let after = self.regions[last + 1]
                guard before.endFrame == after.startFrame else { break }
                let curve = before.points.map(\.automationPoint)
                // A held segment's left limit differs from its endpoint jump.
                let held = before.points.last(where: { $0.frame < before.endFrame })?.curve == .hold
                let leftFrame = held ? before.endFrame - 1 : before.endFrame
                let left =
                    AutomationInterpolation.value(curve, at: leftFrame, baseline: before.points[0].note / 95) * 95
                let right =
                    AutomationInterpolation.value(
                        after.points.map(\.automationPoint), at: after.startFrame,
                        baseline: after.points[0].note / 95) * 95
                guard abs(left - right) < 0.001 else { break }
                last += 1
            }
            for index in first...last {
                starts[index] = self.regions[first].startFrame
                ends[index] = self.regions[last].endFrame
            }
            first = last + 1
        }
        gateStarts = starts
        gateEnds = ends
    }
    func note(at frame: Int) -> Float? { sample(at: frame)?.note }
    func sample(at frame: Int) -> Sample? {
        if current >= 0 {
            let region = regions[current]
            if frame >= region.startFrame && frame < region.endFrame { return currentSample(at: frame) }
            if frame >= region.endFrame && (current + 1 == regions.count || frame < regions[current + 1].startFrame) {
                return nil
            }
        } else if regions.isEmpty || frame < regions[0].startFrame {
            return nil
        }
        var low = 0
        var high = regions.count
        while low < high {
            let middle = low + (high - low) / 2
            if regions[middle].startFrame <= frame { low = middle + 1 } else { high = middle }
        }
        current = low - 1
        guard current >= 0, frame < regions[current].endFrame else { return nil }
        return currentSample(at: frame)
    }
    private func currentSample(at frame: Int) -> Sample {
        let edgeDistance = min(frame - gateStarts[current], gateEnds[current] - 1 - frame)
        let blend: Float
        if edgeDistance >= Self.boundaryRampFrames {
            blend = 1
        } else {
            let fraction = Float(max(0, edgeDistance)) / Float(Self.boundaryRampFrames)
            blend = 0.5 - 0.5 * cos(Float.pi * fraction)
        }
        return .init(note: cursors[current].value(at: frame) * 95, scopeBlend: blend)
    }
}

final class PitchBakedRangeCursor {
    private let ranges: [PitchEditRange]
    private var current = -1
    init(ranges: [PitchEditRange]) { self.ranges = PitchEditRegions.sanitizedRanges(ranges) }
    func contains(_ frame: Int) -> Bool {
        if current >= 0 {
            let range = ranges[current]
            if frame >= range.startFrame && frame < range.endFrame { return true }
            if frame >= range.endFrame && (current + 1 == ranges.count || frame < ranges[current + 1].startFrame) {
                return false
            }
        } else if ranges.isEmpty || frame < ranges[0].startFrame {
            return false
        }
        var low = 0
        var high = ranges.count
        while low < high {
            let middle = low + (high - low) / 2
            if ranges[middle].startFrame <= frame { low = middle + 1 } else { high = middle }
        }
        current = low - 1
        return current >= 0 && frame < ranges[current].endFrame
    }
}
