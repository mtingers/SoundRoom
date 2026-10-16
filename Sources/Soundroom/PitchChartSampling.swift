import Foundation
import SoundroomCore

enum PitchChartSampling {
    static func anchors(_ points: [PitchEditPoint], frames: Range<Int>, pixels: Int) -> [PitchEditPoint] {
        let start = lowerBound(points, frame: frames.lowerBound, time: { $0.frame })
        let end = lowerBound(points, frame: frames.upperBound + 1, time: { $0.frame })
        return extrema(points[start..<end], frames: frames, pixels: pixels, time: { $0.frame }, value: { $0.note })
    }
    static func extrema<T>(_ values: ArraySlice<T>, frames: Range<Int>, pixels: Int,
                           time: (T) -> Int, value: (T) -> Float) -> [T] {
        guard !frames.isEmpty else { return [] }
        let width = max(1, pixels)
        if values.count <= width * 4 { return Array(values) }
        var result: [T] = [], bucket = -1
        var first: T?, last: T?, low: T?, high: T?
        func flush() {
            guard let first, let last, let low, let high else { return }
            var unique: [Int: T] = [:]
            for item in [first, low, high, last] { unique[time(item)] = item }
            result.append(contentsOf: unique.values.sorted { time($0) < time($1) })
        }
        for item in values {
            let pixel = max(0, min(width - 1, Int(Double(time(item) - frames.lowerBound) / Double(frames.count) * Double(width))))
            if bucket != pixel || first == nil {
                flush(); first = item; last = item; low = item; high = item; bucket = pixel
            } else {
                last = item
                if value(item) < value(low!) { low = item }
                if value(item) > value(high!) { high = item }
            }
        }
        flush()
        return result
    }
    static func lowerBound<T>(_ values: [T], frame: Int, time: (T) -> Int) -> Int {
        var lo = 0, hi = values.count
        while lo < hi {
            let middle = lo + (hi - lo) / 2
            if time(values[middle]) < frame { lo = middle + 1 } else { hi = middle }
        }
        return lo
    }
    /// Cull by binary lookup, then preserve first/last and extrema per horizontal pixel.
    /// Unvoiced runs are retained as separators, including gaps within a pixel.
    static func contour(_ points: [PitchContourPoint], frames: Range<Int>, pixels: Int) -> [PitchContourPoint] {
        guard !points.isEmpty, !frames.isEmpty else { return [] }
        let start = max(0, lowerBound(points, frame: frames.lowerBound, time: { $0.frame }) - 1)
        let end = min(points.count, lowerBound(points, frame: frames.upperBound, time: { $0.frame }) + 1)
        guard start < end else { return [] }
        let width = max(1, pixels)
        var output: [PitchContourPoint] = []
        output.reserveCapacity(min(end - start, width * 4 + 16))
        var bucket = -1
        var first: PitchContourPoint?, last: PitchContourPoint?, low: PitchContourPoint?, high: PitchContourPoint?
        func flush() {
            guard let first, let last, let low, let high else { return }
            var unique: [Int: PitchContourPoint] = [:]
            for point in [first, low, high, last] { unique[point.frame] = point }
            output.append(contentsOf: unique.values.sorted { $0.frame < $1.frame })
        }
        for point in points[start..<end] {
            guard point.isVoiced else {
                flush(); first = nil; last = nil; low = nil; high = nil; bucket = -1
                if output.last?.isVoiced != false { output.append(point) }
                continue
            }
            let pixel = max(0, min(width - 1, Int(Double(point.frame - frames.lowerBound) / Double(frames.count) * Double(width))))
            if first == nil || bucket != pixel {
                flush(); first = point; last = point; low = point; high = point; bucket = pixel
            } else {
                last = point
                if point.frequency < low!.frequency { low = point }
                if point.frequency > high!.frequency { high = point }
            }
        }
        flush()
        return output
    }
}

struct PreparedPitchChartRegion {
    let range: Range<Int>
    let points: [PitchEditPoint]
    private let automation: [AutomationPoint]
    init(_ region: PitchEditRegion) {
        range = region.range
        points = region.points
        automation = points.map { AutomationPoint(frame: $0.frame, value: $0.note / 95,
            curve: $0.curve, curveStart: $0.curveStart, curveEnd: $0.curveEnd) }
    }
    /// One initial binary search; adjacent pixels reuse the current segment.
    /// Sampling includes knots (and the sample before hold steps), so steep changes
    /// do not turn into false long glides when the viewport gets wider.
    func samples(frames: Range<Int>, pixels: Int) -> [(frame: Int, note: Float)] {
        let start = max(range.lowerBound, frames.lowerBound), end = min(range.upperBound - 1, frames.upperBound - 1)
        guard start <= end, !points.isEmpty else { return [] }
        let count = max(1, pixels * 2)
        let hop = max(1, (end - start) / count)
        var times = Array(stride(from: start, through: end, by: hop))
        times.append(end)
        let lo = PitchChartSampling.lowerBound(points, frame: start, time: { $0.frame })
        let hi = PitchChartSampling.lowerBound(points, frame: end + 1, time: { $0.frame })
        for index in lo..<hi {
            let frame = points[index].frame
            times.append(frame)
            if index > 0, points[index - 1].curve == .hold, frame > start { times.append(frame - 1) }
        }
        times.sort()
        var segment = max(0, lo - 1), previous: Int?
        var result: [(frame: Int, note: Float)] = []
        result.reserveCapacity(times.count)
        for frame in times where frame != previous {
            while segment + 1 < automation.count, automation[segment + 1].frame <= frame { segment += 1 }
            let value: Float
            if frame < automation[0].frame { value = automation[0].value }
            else if segment + 1 < automation.count {
                value = AutomationInterpolation.between(automation[segment], automation[segment + 1], at: frame)
            } else { value = automation[segment].value }
            result.append((frame, value * 95)); previous = frame
        }
        return PitchChartSampling.extrema(result[...], frames: start..<(end + 1), pixels: max(1, pixels),
            time: { $0.frame }, value: { $0.note })
    }
}
