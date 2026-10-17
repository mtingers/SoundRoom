import Foundation
import SoundroomCore

extension SessionStore {
    var canCombineClips: Bool {
        !busy && !isRecording
            && session.tracks.contains { $0.clips.filter { selectedClips.contains($0.id) }.count >= 2 }
    }
    func combineClips() { _ = remoteRoute("clips.combine") }
}
