import Foundation
import SoundroomCore

/// Device-output envelopes only: source media, edit fades and exported PCM stay intact.
/// The render owner advances this state sample by sample, independent of callback size.
struct PlaybackTransitions {
    static let defaultFrames = Int(sessionRate * 0.005)
    let frames: Int
    private let attackFrames: Int
    private var attackPosition = 0
    private var stopPosition: Int?
    var remainingStopFrames: Int { max(0, frames - (stopPosition ?? 0)) }
    var stopped: Bool { stopPosition.map { $0 >= frames } ?? false }

    init(frames: Int = defaultFrames, availableFrames: Int) {
        self.frames = max(0, frames)
        attackFrames = min(max(0, frames), max(0, availableFrames / 2))
    }
    mutating func requestStop() { if stopPosition == nil { stopPosition = 0 } }
    private func rise(_ position: Int, length: Int) -> Float {
        guard length > 1 else { return position == 0 && length == 1 ? 0 : 1 }
        if position <= 0 { return 0 }
        if position >= length - 1 { return 1 }
        let t = Float(max(0, min(length - 1, position))) / Float(length - 1)
        return 0.5 - 0.5 * cos(.pi * t)
    }
    mutating func gain(remainingFrames: Int?, endRampFrames: Int) -> Float {
        var gain = frames == 0 ? 1 : rise(attackPosition, length: attackFrames)
        attackPosition = min(attackFrames, attackPosition + 1)
        if frames > 0, let remainingFrames {
            gain = min(gain, remainingFrames <= 1 ? 0 : rise(remainingFrames - 1, length: endRampFrames))
        }
        if let position = stopPosition {
            gain *= frames == 0 ? 0 : 1 - rise(position, length: frames)
            stopPosition = min(frames, position + 1)
        }
        return gain
    }
}
