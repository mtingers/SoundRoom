import Foundation
import SoundroomCore

/// Display-only scope for the server's Apply to Audio operation.
/// A scoped first pitch stage can be removed after every region is baked; a pitch
/// automatic or later pitch stage requires an explicit full-track chain bounce.
enum PitchAudioCommit {
    struct Plan: Equatable, Sendable {
        var range: Range<Int>
        var prefixCount: Int
        var fullTrack: Bool
        var label: String
    }
    static func plan(session: Session, trackID: UUID, effectID: UUID) -> Plan? {
        guard let track = session.tracks.first(where: { $0.id == trackID }),
              let index = track.realtimeEffects.firstIndex(where: { $0.id == effectID }),
              track.realtimeEffects[index].enabled, !track.clips.isEmpty,
              track.clips.allSatisfy({ $0.start >= 0 && $0.length > 0 && $0.start <= Int.max / 4 - $0.length }) else { return nil }
        let pitch = track.realtimeEffects[index]
        let end = track.clips.map(\.end).max() ?? 0
        guard end > 0 else { return nil }
        if index == 0, pitch.kind == .pitchCorrection, pitch.pitchEditsOnly {
            let edits = PitchEditRegions.sanitized(pitch.pitchEdits)
            guard let start = edits.map(\.startFrame).min(), let last = edits.map(\.endFrame).max(),
                  start < end,
                  edits.contains(where: { region in track.clips.contains { $0.start < region.endFrame && $0.end > region.startFrame } }) else { return nil }
            // Out-of-source scopes cannot contribute audio; do not allocate their empty future.
            let upper = min(end, last)
            guard upper > start else { return nil }
            return Plan(range: start..<upper, prefixCount: 1, fullTrack: false,
                        label: "Apply all edited regions")
        }
        let limit = Int(30 * sessionRate)
        // A tail estimate is a -60 dB bound, not an exact zero. Retaining a live
        // suffix after truncating its prefix loses small continuing contributions.
        // Full-track mode therefore bakes every insert, with one global tail cap.
        let count = track.realtimeEffects.count
        let estimatedTail = track.realtimeEffects.reduce(0) { $0 + $1.tailFrames }
        let tail = min(limit, estimatedTail)
        guard end <= Int.max / 4 - tail else { return nil }
        return Plan(range: 0..<(end + tail), prefixCount: count, fullTrack: true,
                    label: estimatedTail > limit ? "Full track + capped tails · entire \(count)-effect chain"
                        : "Full track + tails · entire \(count)-effect chain")
    }


}
