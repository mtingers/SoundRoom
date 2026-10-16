import XCTest
@testable import SoundroomRemote

final class RemoteClientCredentialTests: XCTestCase {
    func testNativeAndPortableFilesLoadWithoutInventingPSK() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let file = directory.appendingPathComponent("pairing.json")
        let native = try RemoteCredential.generate(role: .editor)
        try native.write(to: file)
        XCTAssertEqual(try RemoteClientCredential.load(from: file), .native(native))
        let web = RemoteBrowserCredential(native)
        try RemotePrivateFile.write(JSONEncoder().encode(web), to: file)
        let loaded = try RemoteClientCredential.load(from: file)
        XCTAssertEqual(loaded, .web(web))
        XCTAssertEqual(loaded.credentialID, native.clientID); XCTAssertEqual(loaded.role, .editor)
        XCTAssertThrowsError(try loaded.requireNativeCredential())
        XCTAssertEqual(try RemoteClientCredential.native(native).requireNativeCredential(), native)
        XCTAssertThrowsError(try loaded.connectionName(nil, nativeDefault: "Soundroom TUI"))
        XCTAssertEqual(try loaded.connectionName("Approved portable TUI", nativeDefault: "ignored"), "Approved portable TUI")
        XCTAssertEqual(try RemoteClientCredential.native(native).connectionName(nil, nativeDefault: "Soundroom GUI"), "Soundroom GUI")
    }
    func testPrivateFileAndStrictGrammarValidation() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let file = directory.appendingPathComponent("pairing.json")
        let web = RemoteBrowserCredential(try RemoteCredential.generate())
        let data = try JSONEncoder().encode(web)
        try RemotePrivateFile.write(data, to: file)
        try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: file.path)
        XCTAssertThrowsError(try RemoteClientCredential.load(from: file))
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path)
        let alias = directory.appendingPathComponent("alias.json")
        try FileManager.default.createSymbolicLink(at: alias, withDestinationURL: file)
        XCTAssertThrowsError(try RemoteClientCredential.load(from: alias))
        let original = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        for mutation in ["version": 99, "secret": "unexpected", "clientSecret": Data([1]).base64EncodedString(), "role": "admin"] as [String: Any] {
            var invalid = original; invalid[mutation.key] = mutation.value
            try RemotePrivateFile.write(JSONSerialization.data(withJSONObject: invalid), to: file)
            XCTAssertThrowsError(try RemoteClientCredential.load(from: file), mutation.key)
        }
    }
    func testAuthenticationHasIdenticalNativeAndPortableScopeAndUppercaseUUIDs() throws {
        let native = RemoteCredential(serverID: UUID(uuidString: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")!,
            clientID: UUID(uuidString: "11111111-2222-3333-4444-555555555555")!, secret: Data(repeating: 255, count: 32),
            clientSecret: Data(0..<32), role: .editor)
        let peer = RemotePeer(id: UUID(uuidString: "abcdefab-cdef-abcd-efab-cdefabcdefab")!, name: "Studio TUI", role: .operator)
        let challenge = Data(repeating: 7, count: 32)
        let web = RemoteClientCredential.web(RemoteBrowserCredential(native))
        let proof = web.proof(for: peer, challenge: challenge)
        XCTAssertEqual(proof, "ZKS+wrNJQ8m/9nYWhpEUXvRew0+0T+9gEd3nIr2DZ64=", "Canonical cross-language HMAC vector")
        XCTAssertEqual(proof, native.proof(for: peer, challenge: challenge))
        XCTAssertTrue(native.authenticates(peer: peer, proof: proof, challenge: challenge))
        var other = peer; other.name = "Other TUI"
        XCTAssertFalse(native.authenticates(peer: other, proof: proof, challenge: challenge))
        XCTAssertEqual(web.proof(for: peer, challenge: Data([7])), "")
    }
    func testApprovedNamesArePreservedAndNeverTruncated() throws {
        let name = String(repeating: "é", count: 64)
        XCTAssertNoThrow(try RemoteClientCredential.validateName(name))
        for invalid in [name + "x", "\n", "Studio\u{001b}[0m", "   "] {
            XCTAssertThrowsError(try RemoteClientCredential.validateName(invalid))
        }
    }
    @MainActor func testPortableCredentialCannotStartLegacyConnectionAndLongNamesRemainExact() async throws {
        let web = RemoteBrowserCredential(try RemoteCredential.generate())
        let name = String(repeating: "n", count: 100)
        let client = RemoteClient(credential: .web(web), name: name)
        XCTAssertEqual(client.name, name)
        do { try await client.connect(host: "localhost"); XCTFail("v3-only credential must fail before opening a socket") }
        catch { XCTAssertEqual(client.state, .disconnected) }
    }
}
