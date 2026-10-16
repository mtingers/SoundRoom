import Foundation
import CryptoKit

/// Client authentication material. A v3 credential deliberately carries no
/// native TLS PSK and can only be used with a verified WSS endpoint.
public enum RemoteClientCredential: Sendable, Equatable {
    case native(RemoteCredential)
    case web(RemoteBrowserCredential)

    public var serverID: UUID {
        switch self { case .native(let value): value.serverID; case .web(let value): value.serverID }
    }
    public var credentialID: UUID {
        switch self { case .native(let value): value.clientID; case .web(let value): value.credentialID }
    }
    public var role: RemoteRole {
        switch self { case .native(let value): value.role; case .web(let value): value.role }
    }
    public func validate() throws {
        switch self { case .native(let value): try value.validate(); case .web(let value): try value.validate() }
    }
    public func requireNativeCredential() throws -> RemoteCredential {
        guard case .native(let value) = self else {
            throw RemoteError.rejected("This credential requires a secure WSS connection. Use --connect-wss or --wss with the approved client name.")
        }
        try value.validate(); return value
    }
    public static func validateName(_ name: String) throws {
        guard !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, name.utf8.count <= 128,
              !name.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) else {
            throw RemoteError.rejected("Client name must contain 1–128 UTF-8 bytes without control characters.")
        }
    }
    /// A portable host binds authentication to the operator-approved name.
    /// Never guess or truncate that identity when loading its credential.
    public func connectionName(_ approvedName: String?, nativeDefault: String) throws -> String {
        let name: String
        if let approvedName { name = approvedName }
        else if case .native = self { name = nativeDefault }
        else { throw RemoteError.rejected("Use --client-name with the exact name approved when this credential was created.") }
        try Self.validateName(name); return name
    }
    public static func load(from url: URL) throws -> Self {
        let data = try RemotePrivateFile.read(url)
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              let version = object["version"] as? Int else { throw RemoteError.invalidCredential }
        let result: Self
        if version == 3 {
            try RemoteV3ModelValidator.validateJSON(data, type: "RemoteBrowserCredential")
            result = .web(try JSONDecoder().decode(RemoteBrowserCredential.self, from: data))
        } else if version == RemoteProtocol.credentialVersion {
            try RemoteV3ModelValidator.validateJSON(data, type: "RemoteCredential")
            result = .native(try JSONDecoder().decode(RemoteCredential.self, from: data))
        } else { throw RemoteError.invalidCredential }
        try result.validate(); return result
    }
    func proof(for peer: RemotePeer, challenge: Data) -> String {
        guard challenge.count == 32 else { return "" }
        let key: Data
        switch self { case .native(let value): key = value.clientSecret; case .web(let value): key = value.clientSecret }
        let message = RemoteCredential.authenticationMessage(serverID: serverID, credentialID: credentialID, peer: peer, challenge: challenge)
        return Data(HMAC<SHA256>.authenticationCode(for: message, using: SymmetricKey(data: key))).base64EncodedString()
    }
}
