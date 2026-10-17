import CryptoKit
import Foundation

public struct RemoteBrowserCredential: Codable, Sendable, Equatable {
    public var version = 3
    public let serverID: UUID
    public let credentialID: UUID
    public let clientSecret: Data
    public let role: RemoteRole
    public init(serverID: UUID, credentialID: UUID, clientSecret: Data, role: RemoteRole) {
        self.serverID = serverID
        self.credentialID = credentialID
        self.clientSecret = clientSecret
        self.role = role
    }
    public func validate() throws {
        guard version == 3, clientSecret.count == 32 else { throw RemoteError.invalidCredential }
    }
    init(_ credential: RemoteCredential) {
        serverID = credential.serverID
        credentialID = credential.clientID
        clientSecret = credential.clientSecret
        role = credential.role
    }
}

public struct RemoteWebPairingGrant: Codable, Sendable, Equatable {
    public let code: String
    public let name: String
    public let role: RemoteRole
    public let expiresInSeconds: Int
}

/// Public operator inventory contains identity/scope, never any credential key.
public struct RemoteWebClientRecord: Codable, Sendable, Equatable, Identifiable {
    public let id: UUID
    public let name: String
    public let role: RemoteRole
    public init(id: UUID, name: String, role: RemoteRole) {
        self.id = id
        self.name = name
        self.role = role
    }
}

struct RemoteWebCredentialFile: Codable {
    let credential: RemoteCredential
    let record: RemoteWebClientRecord
    func write(to url: URL) throws {
        try validate()
        try RemotePrivateFile.write(JSONEncoder().encode(self), to: url)
    }
    func validate() throws {
        try credential.validate()
        guard record.id == credential.clientID, record.role == credential.role,
            !record.name.isEmpty, record.name.utf8.count <= 128,
            !record.name.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) })
        else { throw RemoteError.invalidCredential }
    }
    static func load(from url: URL) throws -> Self {
        let data = try RemotePrivateFile.read(url)
        let file: Self
        if let stored = try? JSONDecoder().decode(Self.self, from: data) {
            file = stored
        } else {
            let credential = try JSONDecoder().decode(RemoteCredential.self, from: data)
            file = .init(
                credential: credential,
                record: .init(
                    id: credential.clientID, name: "Browser \(credential.clientID.uuidString.prefix(8))",
                    role: credential.role))
        }
        try file.validate()
        return file
    }
}

/// Grants can only be created through an authenticated operator. Possessing a
/// code grants precisely the role/name approved there, once, for the operator-selected bounded lifetime.
@MainActor final class RemoteWebPairingStore {
    private struct Pending {
        let digest: Data
        let name: String
        let role: RemoteRole
        let expires: TimeInterval
    }
    private var pending: [Pending] = []
    private let clock: () -> TimeInterval
    init(clock: @escaping () -> TimeInterval = { ProcessInfo.processInfo.systemUptime }) { self.clock = clock }
    func issue(name: String, role: RemoteRole, expiresInSeconds: Int = 120) throws -> RemoteWebPairingGrant {
        guard (120...3600).contains(expiresInSeconds) else { throw RemoteError.invalidFrame }
        let name = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, name.utf8.count <= 128,
            !name.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) })
        else { throw RemoteError.invalidFrame }
        pending.removeAll { $0.expires <= clock() }
        guard pending.count < 8 else { throw RemoteError.queueFull }
        let code = try RemoteCredential.authenticationNonce().base64EncodedString().replacingOccurrences(
            of: "+", with: "-"
        ).replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: "=", with: "")
        pending.append(
            .init(
                digest: Data(SHA256.hash(data: Data(code.utf8))), name: name, role: role,
                expires: clock() + Double(expiresInSeconds)))
        return .init(code: code, name: name, role: role, expiresInSeconds: expiresInSeconds)
    }
    func redeem(code: String, name: String) throws -> RemoteRole {
        guard code.utf8.count == 43, name.utf8.count <= 128 else { throw RemoteError.invalidCredential }
        pending.removeAll { $0.expires <= clock() }
        let digest = Data(SHA256.hash(data: Data(code.utf8)))
        guard let index = pending.firstIndex(where: { $0.digest == digest && $0.name == name }) else {
            throw RemoteError.invalidCredential
        }
        return pending.remove(at: index).role
    }
    func clear() { pending.removeAll() }
}
