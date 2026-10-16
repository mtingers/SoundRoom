import Foundation

/// A bounded musical viewport, independent of pixel size and quantization.
public struct DesignerViewport: Equatable, Sendable {
    public var start: Double
    public var span: Double
    public init(start: Double = 0, span: Double = 8) { self.start = start; self.span = span }
    public func fitted(to beats: Double) -> Self {
        let length = beats.isFinite ? max(0.25, min(64, beats)) : 4
        let width = span.isFinite ? max(0.25, min(length, span)) : min(8, length)
        return Self(start: start.isFinite ? max(0, min(length - width, start)) : 0, span: width)
    }
    public func zoomed(by factor: Double, around beat: Double, beats: Double) -> Self {
        let old = fitted(to: beats), anchor = beat.isFinite ? max(old.start, min(old.start + old.span, beat)) : old.start
        let fraction = (anchor - old.start) / old.span
        let width = max(0.25, min(beats, old.span * (factor.isFinite && factor > 0 ? factor : 1)))
        return Self(start: anchor - fraction * width, span: width).fitted(to: beats)
    }
    public func includes(_ note: SoundDesignNote) -> Bool { note.startBeat < start + span && note.startBeat + note.durationBeats > start }
    public func beat(at fraction: Double) -> Double { start + max(0, min(1, fraction)) * span }
}
