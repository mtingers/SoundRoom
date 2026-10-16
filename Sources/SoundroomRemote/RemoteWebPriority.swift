import Foundation

extension RemoteWireMessage {
    /// Authentication, rejection, receipts and liveness must remain serviceable
    /// while another peer's ordinary document is being prepared.
    var usesPriorityWebEncoding: Bool {
        switch self {
        case .challenge, .welcome, .failure, .reply, .heartbeat, .heartbeatReply: return true
        default: return false
        }
    }
}
