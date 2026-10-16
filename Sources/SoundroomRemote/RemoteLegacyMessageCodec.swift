import Foundation

/// A late legacy client receives an actionable failure for a session that needs
/// pagination, rather than an oversized frame or an unexplained dropped socket.
enum RemoteLegacyMessageCodec {
    static func encode(_ message: RemoteWireMessage) throws -> Data {
        let data = try JSONEncoder().encode(message)
        switch message {
        case .event(.snapshot), .event(.delta):
            guard data.count > 6 * 1024 * 1024 else { return data }
            return try JSONEncoder().encode(RemoteWireMessage.failure("This session requires the common WSS connection because its metadata exceeds the legacy document limit. Connect using --connect-wss or --wss."))
        default: return data
        }
    }
}
