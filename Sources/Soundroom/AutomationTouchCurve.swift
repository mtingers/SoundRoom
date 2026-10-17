import SoundroomCore

/// Read data stays immutable while a held Touch value previews ahead of the
/// transport. Committed writes retain completed loop passes independently.
struct AutomationTouchCurve {
    let original: [AutomationPoint]
    let baseline: Float
    let scale: AutomationValueScale
    let loop: Range<Int>?
    private(set) var committed: [AutomationPoint]
    private(set) var frame: Int
    private(set) var value: Float
    private var observedFrame: Int
    private var observedSerial: Int?
    private var started = false

    init(
        points: [AutomationPoint], baseline: Float, scale: AutomationValueScale, start: Int, loop: Range<Int>?,
        serial: Int? = nil
    ) {
        original = points
        committed = points
        self.baseline = baseline
        self.scale = scale
        self.loop = loop.flatMap { $0.contains(start) ? $0 : nil }
        frame = start
        observedFrame = start
        observedSerial = serial
        value = AutomationInterpolation.value(points, at: start, baseline: baseline, scale: scale)
    }

    mutating func observe(_ current: Int, serial: Int? = nil) {
        var wraps = current < observedFrame ? 1 : 0
        if let loop, let previous = observedSerial, let serial, serial >= previous,
            loop.contains(observedFrame), serial - previous <= Int.max - (observedFrame - loop.lowerBound)
        {
            wraps = (observedFrame - loop.lowerBound + (serial - previous)) / loop.count
        }
        if started, let loop, loop.contains(current), wraps > 0 {
            replace(
                frame...max(frame, loop.upperBound - 1),
                with: [.init(frame: frame, value: value), .init(frame: loop.upperBound - 1, value: value)])
            if wraps > 1 {
                replace(
                    loop.lowerBound...(loop.upperBound - 1),
                    with: [
                        .init(frame: loop.lowerBound, value: value), .init(frame: loop.upperBound - 1, value: value),
                    ])
            }
            replace(
                loop.lowerBound...current,
                with: [.init(frame: loop.lowerBound, value: value), .init(frame: current, value: value)])
            frame = current
        }
        observedFrame = current
        observedSerial = serial
    }

    mutating func write(at current: Int, value next: Float, serial: Int? = nil) -> [AutomationPoint] {
        observe(current, serial: serial)
        if started, current >= frame {
            replace(frame...current, with: [.init(frame: frame, value: value), .init(frame: current, value: next)])
        } else {
            replace(current...current, with: [.init(frame: current, value: next)])
        }
        started = true
        frame = current
        value = next
        return preview()
    }

    private func preview() -> [AutomationPoint] {
        guard let loop else { return committed.filter { $0.frame <= frame } }
        // A held fader controls the next lap as well as the remaining current lap.
        // The recorded curve is restored from `committed` when Touch is released.
        var result = committed
        if loop.lowerBound > 0 {
            result = AutomationCurveEditing.insertingBoundary(
                result, at: loop.lowerBound - 1, baseline: baseline, scale: scale)
        }
        result = AutomationCurveEditing.insertingBoundary(result, at: loop.upperBound, baseline: baseline, scale: scale)
        result.removeAll { loop.contains($0.frame) }
        result += [
            .init(frame: loop.lowerBound, value: value, curve: .hold), .init(frame: loop.upperBound - 1, value: value),
        ]
        return unique(result)
    }

    mutating func finish(at release: Int, returnFrames: Int = 4800, serial: Int? = nil) -> [AutomationPoint] {
        guard started else { return original }
        observe(release, serial: serial)
        let current = max(frame, release)
        replace(frame...current, with: [.init(frame: frame, value: value), .init(frame: current, value: value)])
        guard let loop else {
            let end = current + min(max(1, returnFrames), Int.max - current)
            let target = AutomationInterpolation.value(committed, at: end, baseline: baseline, scale: scale)
            replace(
                current...end,
                with: [.init(frame: current, value: value, curve: .smooth), .init(frame: end, value: target)])
            return unique(committed)
        }
        guard loop.count > 1 else {
            replace(
                current...current,
                with: [
                    .init(
                        frame: current,
                        value: AutomationInterpolation.value(original, at: current, baseline: baseline, scale: scale))
                ])
            return unique(committed)
        }
        let duration = min(max(1, returnFrames), loop.count - 1)
        let untilBoundary = loop.upperBound - current
        let targetFrame = duration < untilBoundary ? current + duration : loop.lowerBound + (duration - untilBoundary)
        let target = AutomationInterpolation.value(committed, at: targetFrame, baseline: baseline, scale: scale)
        var returning = [
            AutomationPoint(frame: 0, value: value, curve: .smooth), .init(frame: duration, value: target),
        ]
        if duration < untilBoundary {
            replace(current...targetFrame, with: returning.map { $0.relocated(frame: current + $0.frame) })
        } else {
            returning = AutomationCurveEditing.insertingBoundary(
                returning, at: untilBoundary - 1, baseline: value, scale: scale)
            returning = AutomationCurveEditing.insertingBoundary(
                returning, at: untilBoundary, baseline: value, scale: scale)
            replace(
                current...(loop.upperBound - 1),
                with: returning.filter { $0.frame < untilBoundary }.map { $0.relocated(frame: current + $0.frame) })
            replace(
                loop.lowerBound...targetFrame,
                with: returning.filter { $0.frame >= untilBoundary }.map {
                    $0.relocated(frame: loop.lowerBound + ($0.frame - untilBoundary))
                })
        }
        return unique(committed)
    }

    private mutating func replace(_ range: ClosedRange<Int>, with replacement: [AutomationPoint]) {
        if range.lowerBound > 0 {
            committed = AutomationCurveEditing.insertingBoundary(
                committed, at: range.lowerBound - 1, baseline: baseline, scale: scale)
        }
        if range.upperBound < Int.max {
            committed = AutomationCurveEditing.insertingBoundary(
                committed, at: range.upperBound + 1, baseline: baseline, scale: scale)
        }
        committed.removeAll { range.contains($0.frame) }
        committed += replacement
        committed = unique(committed)
    }
    private func unique(_ points: [AutomationPoint]) -> [AutomationPoint] {
        var indexed: [Int: AutomationPoint] = [:]
        for point in points { indexed[point.frame] = point }
        return indexed.values.sorted { $0.frame < $1.frame }
    }
}
