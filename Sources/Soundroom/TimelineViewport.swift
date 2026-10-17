import Foundation
import SoundroomCore

/// Timeline positions are content pixels; the viewport comes from the scroll
/// view's geometry rather than seconds elapsed or the last reveal request.
struct TimelineViewport: Equatable {
    var offset: Double = 0
    var width: Double = 0
    var contentWidth: Double = 0

    /// Accessibility scrollers can change bounds without a wheel event. Ignore
    /// layout/zoom changes and scrolls for which this view already has a request.
    func isUnrequestedScroll(from previous: TimelineViewport, pendingDestination: Double?) -> Bool {
        pendingDestination == nil && previous.width > 0 && abs(width - previous.width) < 0.5
            && abs(contentWidth - previous.contentWidth) < 0.5 && abs(offset - previous.offset) > 2
    }

    func followDestination(frame: Int, zoom: Double, playing: Bool, enabled: Bool, loop: Range<Int>? = nil) -> Double? {
        guard playing, enabled, width > 0, width.isFinite, offset.isFinite,
            contentWidth.isFinite, zoom.isFinite, zoom > 0
        else { return nil }
        let cursor = Double(max(0, frame)) / sessionRate * zoom
        let left = max(0, offset)
        let right = left + width
        // A loop that is already visible must stay still through its wrap, even
        // when its ends lie inside the usual follow safety margins.
        if let loop, !loop.isEmpty {
            let start = Double(loop.lowerBound) / sessionRate * zoom
            let end = Double(loop.upperBound) / sessionRate * zoom
            if start >= left - 1, end <= right + 1 { return nil }
        }
        let margin = min(40, width * 0.1)
        let safeLeft = left == 0 ? 0 : left + margin
        let safeRight = right - margin
        guard cursor < safeLeft || cursor > safeRight else { return nil }
        let maximum = max(0, contentWidth - width)
        var desired = cursor < safeLeft ? cursor - margin : cursor - width * 0.2
        if let loop, !loop.isEmpty {
            let start = Double(loop.lowerBound) / sessionRate * zoom
            let end = Double(loop.upperBound) / sessionRate * zoom
            if end - start <= width {
                // Reveal a short offscreen loop as one region, so all following
                // iterations fit without alternating between its two edges.
                desired = min(start, max(end - width, start - margin))
            }
        }
        desired = max(0, min(maximum, desired))
        return abs(desired - left) >= 1 ? desired : nil
    }

    /// Continuous edge scrolling, with a gentle inner edge zone and capped speed
    /// outside the viewport. Pointer coordinates are viewport pixels, not content.
    func dragScrollDestination(pointerX: Double, elapsed: Double) -> Double? {
        guard width > 0, width.isFinite, offset.isFinite, contentWidth.isFinite,
            pointerX.isFinite, elapsed.isFinite, elapsed > 0
        else { return nil }
        let edge = min(36, width * 0.1)
        let penetration: Double
        if pointerX < edge {
            penetration = pointerX - edge
        } else if pointerX > width - edge {
            penetration = pointerX - (width - edge)
        } else {
            return nil
        }
        let speed = min(900, abs(penetration) * 12)
        let destination = max(
            0, min(max(0, contentWidth - width), offset + (penetration < 0 ? -speed : speed) * min(0.08, elapsed)))
        return abs(destination - offset) > 0.001 ? destination : nil
    }

    func revealDestination(frame: Int, zoom: Double) -> Double {
        guard zoom.isFinite, zoom > 0 else { return 0 }
        let cursor = Double(max(0, frame)) / sessionRate * zoom
        let maximum = max(0, contentWidth - width)
        return max(0, min(maximum, cursor - min(40, max(0, width) * 0.1)))
    }
}

/// One monotonic time budget shared by pointer events and held-drag ticks.
struct TimelineDragScrollClock {
    private(set) var lastTimestamp: TimeInterval?
    mutating func begin(at timestamp: TimeInterval) { lastTimestamp = timestamp.isFinite ? timestamp : nil }
    mutating func reset() { lastTimestamp = nil }
    mutating func consume(at timestamp: TimeInterval) -> TimeInterval {
        guard timestamp.isFinite else { return 0 }
        guard let last = lastTimestamp else {
            lastTimestamp = timestamp
            return 0
        }
        guard timestamp >= last else { return 0 }
        lastTimestamp = timestamp
        return min(0.08, timestamp - last)
    }
}
