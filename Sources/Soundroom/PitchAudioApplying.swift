import Foundation
import SoundroomCore

extension SessionStore {
    func applyPitchEdits(trackID: UUID, effectID: UUID, onApplied: (() -> Void)? = nil) {
        guard let remoteGUI else { status = "Connect to the audio server before applying effects."; return }
        remoteGUI.route(.init(operation: "pitch.apply", context: remoteContext, strings: ["trackID": trackID.uuidString, "effectID": effectID.uuidString])) { reply in
            if reply.accepted { onApplied?() }
        }
    }
}
