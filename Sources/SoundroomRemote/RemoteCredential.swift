import Foundation
import CryptoKit
import Security
import Darwin

/// Pairing is an explicit file transfer from the hardware host. The file is a
/// bearer credential: store it with mode 0600, never put its contents in argv.
public struct RemoteCredential: Codable, Sendable, Equatable {
    public var version: Int = RemoteProtocol.credentialVersion
    public var serverID: UUID
    /// Credential identity; each attached client has an independent runtime ID.
    public var clientID: UUID
    public var secret: Data
    public var clientSecret: Data
    public var role: RemoteRole
    public init(serverID: UUID = UUID(), clientID: UUID = UUID(), secret: Data, clientSecret: Data, role: RemoteRole = .operator) {
        self.serverID = serverID; self.clientID = clientID; self.secret = secret; self.clientSecret = clientSecret; self.role = role
    }
    public static func generate(serverID: UUID = UUID(), role: RemoteRole = .operator) throws -> Self {
        .init(serverID: serverID, secret: try randomSecret(), clientSecret: try randomSecret(), role: role)
    }
    public func paired(role: RemoteRole = .operator) throws -> Self {
        .init(serverID: serverID, secret: secret, clientSecret: try Self.randomSecret(), role: role)
    }
    public func validate() throws {
        guard version == RemoteProtocol.credentialVersion, secret.count == 32, clientSecret.count == 32 else { throw RemoteError.invalidCredential }
    }
    public func write(to url: URL) throws {
        try validate()
        let parent = url.deletingLastPathComponent()
        try FileManager.default.createDirectory(at: parent, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        let temporary = parent.appendingPathComponent(".soundroom-pairing-\(UUID().uuidString)")
        let fd = Darwin.open(temporary.path, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, S_IRUSR | S_IWUSR)
        guard fd >= 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
        defer { Darwin.close(fd); try? FileManager.default.removeItem(at: temporary) }
        let data = try JSONEncoder().encode(self)
        try data.withUnsafeBytes { bytes in
            var offset = 0
            while offset < bytes.count {
                let written = Darwin.write(fd, bytes.baseAddress!.advanced(by: offset), bytes.count - offset)
                if written < 0 { if errno == EINTR { continue }; throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
                offset += written
            }
        }
        guard fsync(fd) == 0, Darwin.rename(temporary.path, url.path) == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    }
    public static func load(from url: URL) throws -> Self {
        let fd = Darwin.open(url.path, O_RDONLY | O_NOFOLLOW | O_CLOEXEC)
        guard fd >= 0 else { throw RemoteError.invalidCredential }
        defer { Darwin.close(fd) }
        var info = stat()
        guard fstat(fd, &info) == 0, (info.st_mode & S_IFMT) == S_IFREG,
              info.st_uid == getuid(), (info.st_mode & 0o077) == 0, info.st_size > 0, info.st_size <= 16_384 else { throw RemoteError.invalidCredential }
        var data = Data(count: Int(info.st_size))
        try data.withUnsafeMutableBytes { bytes in
            var offset = 0
            while offset < bytes.count {
                let count = Darwin.read(fd, bytes.baseAddress!.advanced(by: offset), bytes.count - offset)
                if count < 0 { if errno == EINTR { continue }; throw RemoteError.invalidCredential }
                guard count > 0 else { throw RemoteError.invalidCredential }; offset += count
            }
        }
        let value = try JSONDecoder().decode(Self.self, from: data); try value.validate(); return value
    }
    private static func randomSecret() throws -> Data {
        var bytes = [UInt8](repeating: 0, count: 32)
        guard SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes) == errSecSuccess else { throw RemoteError.invalidCredential }
        return Data(bytes)
    }
    static func authenticationNonce() throws -> Data { try randomSecret() }
    func proof(for peer: RemotePeer, challenge: Data) -> String {
        guard challenge.count == 32 else { return "" }
        let message = authenticationMessage(peer: peer, challenge: challenge)
        return Data(HMAC<SHA256>.authenticationCode(for: message, using: SymmetricKey(data: clientSecret))).base64EncodedString()
    }
    func authenticates(peer: RemotePeer, proof: String, challenge: Data) -> Bool {
        guard challenge.count == 32, let received = Data(base64Encoded: proof), received.count == 32 else { return false }
        let message = authenticationMessage(peer: peer, challenge: challenge)
        return HMAC<SHA256>.isValidAuthenticationCode(received, authenticating: message, using: SymmetricKey(data: clientSecret))
    }
    private func authenticationMessage(peer: RemotePeer, challenge: Data) -> Data {
        Self.authenticationMessage(serverID: serverID, credentialID: clientID, peer: peer, challenge: challenge)
    }
    static func authenticationMessage(serverID: UUID, credentialID: UUID, peer: RemotePeer, challenge: Data) -> Data {
        Data("soundroom-auth-v2|\(serverID.uuidString)|\(credentialID.uuidString)|\(peer.id.uuidString)|\(peer.name)|\(challenge.base64EncodedString())".utf8)
    }
}
