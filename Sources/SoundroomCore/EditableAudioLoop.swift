import Foundation

/// Callback-owned PCM player. Replacement buffers are prepared off the render
/// thread. The caller retires the returned old buffer away from that thread.
public final class EditableAudioLoop {
    public struct Prepared: Sendable {
        public let channels: [[Float]]
        public let beats: Double
        public var count: Int { channels[0].count }
        public init(audio: AudioSamples, beats: Double) {
            let count = max(1, audio.count)
            var left =
                audio.count > 0 && audio.channelCount > 0
                ? audio.readFrames(0..<audio.count, channels: [0])[0].map { $0.isFinite ? $0 : 0 }
                : [Float](repeating: 0, count: count)
            var right =
                audio.count > 0 && audio.channelCount > 1
                ? audio.readFrames(0..<audio.count, channels: [1])[0].map { $0.isFinite ? $0 : 0 } : left
            // Tail folding can sum across the loop boundary. One stereo gain
            // preserves the waveform and pan balance instead of hard clipping.
            let peak = max(left.lazy.map { abs($0) }.max() ?? 0, right.lazy.map { abs($0) }.max() ?? 0)
            if peak > 0.98 {
                let gain: Float = 0.98 / peak
                for i in left.indices {
                    left[i] *= gain
                    right[i] *= gain
                }
            }
            channels = [left, right]
            self.beats = beats.isFinite ? max(0.25, min(256, beats)) : 4
        }
    }
    private var current: Prepared?
    private var previous: Prepared?
    private var frame = 0, previousFrame = 0, transition = 480
    private var level: Float = 0
    private var playing = false
    public var readyForReplacement: Bool { current == nil || transition >= 480 }
    public var beat: Double { current.map { Double(frame) / Double($0.count) * $0.beats } ?? 0 }
    public init() {}
    @discardableResult public func replace(_ prepared: Prepared, restart: Bool = false) -> Prepared? {
        let retired = previous
        previous = current
        previousFrame = frame
        if let current, !restart {
            // Keep the same musical beat when tempo/length changes.
            let beat = Double(frame) / Double(current.count) * current.beats
            frame =
                Int(
                    (beat.truncatingRemainder(dividingBy: prepared.beats) / prepared.beats * Double(prepared.count))
                        .rounded()) % prepared.count
        } else {
            frame = 0
        }
        current = prepared
        transition = 0
        playing = true
        return retired
    }
    public func stop() { playing = false }
    public func render(left: UnsafeMutablePointer<Float>, right: UnsafeMutablePointer<Float>, count: Int) {
        guard count > 0 else { return }
        for i in 0..<count {
            level = playing ? min(1, level + 1 / 240) : max(0, level - 1 / 240)
            guard let current else {
                left[i] = 0
                right[i] = 0
                continue
            }
            let weight = Float(0.5 - 0.5 * cos(Double(min(480, transition)) / 480 * .pi))
            for c in 0..<2 {
                let next = current.channels[c][frame]
                let old = transition < 480 ? previous?.channels[c][previousFrame] ?? next : next
                let sample = (old + (next - old) * weight) * level
                if c == 0 { left[i] = sample } else { right[i] = sample }
            }
            if playing || level > 0 {
                frame = (frame + 1) % current.count
                if let previous { previousFrame = (previousFrame + 1) % previous.count }
            }
            transition = min(480, transition + 1)
        }
    }
}
