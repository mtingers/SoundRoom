import Foundation
import SoundroomCore

extension SessionStore {
    /// The extent of the selected clips, including gaps and clips on other tracks.
    /// Destructive audio edits continue to use the existing single-track selectedRange.
    var timelineSelectionRange: Range<Int>? {
        if let selection, !selection.range.isEmpty { return selection.range }
        let clips = session.tracks.flatMap(\.clips).filter { selectedClips.contains($0.id) && $0.length > 0 }
        if let first = clips.map(\.start).min(), let last = clips.map(\.end).max() { return first..<last }
        return selectedRange
    }

    var canToggleLoop: Bool { loopSelection || timelineSelectionRange != nil || loopRange != nil }

    func useSelectionForLoop() {
        guard let range = timelineSelectionRange else { return }
        setLoopRange(range)
    }

    func setLoopRange(_ range: Range<Int>) {
        guard let remoteGUI else {
            status = "Connect to the audio server before changing the loop."
            return
        }

        guard !range.isEmpty, !busy, !isRecording else { return }
        remoteGUI.send(.setLoop(start: max(0, range.lowerBound), end: max(1, range.upperBound)))
        return
    }

    func setLoopEdge(start: Bool, seconds: Double) {
        guard let range = loopRange, seconds.isFinite else { return }
        let frame = Int(min(Double(Int.max / 4), max(0, seconds * sessionRate)).rounded())
        setLoopRange(
            start
                ? min(frame, range.upperBound - 1)..<range.upperBound
                : range.lowerBound..<max(range.lowerBound + 1, frame))
    }
}
