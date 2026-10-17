import Foundation
import SoundroomCore

extension SessionStore {
    func moveRealtimeEffect(_ id: UUID, offset: Int) {
        guard !busy, !isRecording, let trackID = selectedTrack else { return }
        updateTrack(trackID, actionName: "Reorder effects") { track in
            guard let index = track.realtimeEffects.firstIndex(where: { $0.id == id }),
                track.realtimeEffects.indices.contains(index + offset)
            else { return }
            track.realtimeEffects.swapAt(index, index + offset)
        }
        focusEditor()
    }
}
