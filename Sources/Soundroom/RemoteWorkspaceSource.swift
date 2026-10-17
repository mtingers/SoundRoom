import Foundation
import SoundroomCore
import SoundroomRemote

/// Opening ownership of a private creative draft. Navigation is intentionally
/// absent: changing the selected track or cursor cannot retarget this source.
@MainActor final class RemoteWorkspaceSource {
    let trackID: UUID?
    let clipID: UUID?
    let frame: Int
    let connectionGeneration: Int
    let sessionRevision: Int
    let fence: RemoteGestureRevisionFence
    private let hasBaseline: Bool

    init(remote: RemoteGUIController, trackID: UUID?, clipID: UUID?, frame: Int) {
        self.trackID = trackID
        self.clipID = clipID
        self.frame = frame
        connectionGeneration = remote.connectionGeneration
        sessionRevision = remote.revision
        fence = RemoteGestureRevisionFence(versions: remote.trackRevisions)
        hasBaseline = remote.connected && remote.authoritativeSession != nil
    }
    var context: RemoteEditorContext {
        .init(trackID: trackID, clipID: clipID, clipIDs: clipID.map { [$0] } ?? [], cursorFrame: frame)
    }
    var expectedSessionRevision: Int? { trackID == nil ? sessionRevision : nil }
    func ready(in remote: RemoteGUIController) -> Bool {
        guard hasBaseline, remote.connected, connectionGeneration == remote.connectionGeneration,
            let document = remote.authoritativeSession, !fence.invalidated
        else { return false }
        guard let trackID else { return clipID == nil && remote.revision == sessionRevision }
        guard fence.versions[trackID] != nil, fence.isCurrent(targets: [trackID], observed: remote.trackRevisions),
            let track = document.tracks.first(where: { $0.id == trackID })
        else { return false }
        return clipID == nil || track.clips.contains { $0.id == clipID }
    }
}
