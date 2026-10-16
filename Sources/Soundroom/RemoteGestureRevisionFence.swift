import Foundation
import SoundroomRemote

/// A drag owns its opening versions. Receipt revisions may advance them only
/// after matching authored metadata arrives; foreign changes never become a
/// new basis for an old private draft.
@MainActor final class RemoteGestureRevisionFence {
    enum Resolution: Equatable { case waiting, ready, conflict }
    let id = UUID()
    private(set) var versions: [UUID: Int]
    private(set) var invalidated = false
    var draft: RemoteTrackEdit?

    init(versions: [UUID: Int]) { self.versions = versions }
    func invalidate() { invalidated = true }

    func isCurrent(targets: Set<UUID>, observed: [UUID: Int]) -> Bool {
        !invalidated && targets.allSatisfy { versions[$0] == observed[$0] }
    }

    func resolve(reply: RemoteReply, targets: Set<UUID>, observedRevision: Int,
                 observed: [UUID: Int], verifiedUnchanged: Set<UUID>, removedTargets: Set<UUID> = []) -> Resolution {
        guard !invalidated, reply.accepted else { invalidate(); return .conflict }
        guard observedRevision >= reply.revision else { return .waiting }
        var next = versions
        for id in targets {
            if observed[id] == reply.revision {
                next[id] = reply.revision
            } else if removedTargets.contains(id), observed[id] == nil, versions[id] != nil, observedRevision == reply.revision {
                next.removeValue(forKey: id)
            } else if observed[id] == versions[id], versions[id] != nil, verifiedUnchanged.contains(id) {
                // No-op controls retain their target version even if an
                // unrelated authored edit has advanced the session revision.
            } else {
                invalidate(); return .conflict
            }
        }
        versions = next
        return .ready
    }
}
