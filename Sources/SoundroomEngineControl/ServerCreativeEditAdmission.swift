import Foundation
import SoundroomCore
import SoundroomRemote

/// Creative renders must name and fence the destination they actually mutate.
/// A deleted destination is a conflict, never permission to create a new track.
public enum ServerCreativeEditAdmission {
    public static func validate(_ request: RemoteRequest, in session: Session) throws {
        guard case .editor(let intent) = request.command,
              ["designer.commit", "drums.commit"].contains(intent.operation) else { return }
        func identifier(_ key: String) throws -> UUID? {
            guard let text = intent.strings[key], !text.isEmpty else { return nil }
            guard let id = UUID(uuidString: text) else { throw RemoteError.rejected("The creative edit has an invalid \(key).") }
            return id
        }
        let explicitTrack = try identifier("trackID"), clipID = try identifier("clipID")
        let trackID = intent.operation == "drums.commit" ? explicitTrack ?? intent.context.trackID : explicitTrack
        if let contextTrack = intent.context.trackID, contextTrack != trackID {
            throw RemoteError.rejected("The creative edit context does not match its destination track.")
        }
        if let contextClip = intent.context.clipID, contextClip != clipID {
            throw RemoteError.rejected("The creative edit context does not match its destination clip.")
        }
        if let trackID {
            guard let track = session.tracks.first(where: { $0.id == trackID }) else {
                throw RemoteError.rejected("The destination track is no longer available. Keep the draft or insert it on a new track.")
            }
            guard request.expectedTrackRevisions[trackID] != nil else {
                throw RemoteError.rejected("A creative edit must include its destination track version.")
            }
            if let clipID, !track.clips.contains(where: { $0.id == clipID }) {
                throw RemoteError.rejected("The source clip is no longer available. Keep the draft or insert it on a new track.")
            }
        } else {
            guard clipID == nil else { throw RemoteError.rejected("Replacing a clip requires its destination track.") }
            guard request.expectedRevision != nil else {
                throw RemoteError.rejected("Creating a track requires the opening session version.")
            }
        }
    }
}
