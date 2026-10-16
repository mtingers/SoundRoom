import XCTest
@testable import SoundroomRemote

@MainActor final class RemoteWebPairingTests: XCTestCase {
    func testGrantIsNameBoundSingleUseAndRoleComesFromApproval() throws {
        let store = RemoteWebPairingStore()
        let grant = try store.issue(name: "Studio browser", role: .viewer)
        XCTAssertEqual(grant.code.count, 43)
        XCTAssertEqual(grant.expiresInSeconds, 120)
        XCTAssertThrowsError(try store.redeem(code: grant.code, name: "Different browser"))
        XCTAssertEqual(try store.redeem(code: grant.code, name: "Studio browser"), .viewer)
        XCTAssertThrowsError(try store.redeem(code: grant.code, name: "Studio browser"))
    }
    func testExpiryAndPendingAdmissionAreBounded() throws {
        var now: TimeInterval = 0
        let store = RemoteWebPairingStore(clock: { now })
        let grant = try store.issue(name: "Browser", role: .editor)
        for index in 1..<8 { _ = try store.issue(name: "Browser \(index)", role: .viewer) }
        XCTAssertThrowsError(try store.issue(name: "Too many", role: .operator))
        now = 120
        XCTAssertThrowsError(try store.redeem(code: grant.code, name: "Browser"))
        XCTAssertNoThrow(try store.issue(name: "Fresh", role: .operator))
    }
    func testOperatorCanChooseLongerCodeLifetime() throws {
        var now: TimeInterval = 0
        let store = RemoteWebPairingStore(clock: { now })
        let grant = try store.issue(name: "Browser", role: .editor, expiresInSeconds: 1800)
        XCTAssertEqual(grant.expiresInSeconds, 1800)
        now = 300
        XCTAssertEqual(try store.redeem(code: grant.code, name: "Browser"), .editor)
        let expired = try store.issue(name: "Browser", role: .viewer, expiresInSeconds: 1800)
        now += 1800
        XCTAssertThrowsError(try store.redeem(code: expired.code, name: "Browser"))
        XCTAssertThrowsError(try store.issue(name: "Browser", role: .viewer, expiresInSeconds: 0))
        XCTAssertThrowsError(try store.issue(name: "Browser", role: .viewer, expiresInSeconds: 3601))
    }
    func testBrowserCredentialNeverContainsNativeTLSPairingSecret() throws {
        let credential = try RemoteCredential.generate()
        let data = try JSONEncoder().encode(RemoteBrowserCredential(credential))
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertNil(object["secret"])
        XCTAssertEqual(object["clientSecret"] as? String, credential.clientSecret.base64EncodedString())
        XCTAssertEqual(object["credentialID"] as? String, credential.clientID.uuidString)
        XCTAssertThrowsError(try RemoteWebPairingStore().issue(name: "\n", role: .operator))
    }
    func testNamedRegistryIsPrivatePersistentAndRejectsMismatchedScope() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let credential = try RemoteCredential.generate(role: .editor)
        let file = directory.appendingPathComponent("browser.json")
        let record = RemoteWebClientRecord(id: credential.clientID, name: "Studio browser", role: .editor)
        let stored = RemoteWebCredentialFile(credential: credential, record: record)
        try stored.write(to: file)
        XCTAssertEqual(try RemoteWebCredentialFile.load(from: file).record, record)
        let attributes = try FileManager.default.attributesOfItem(atPath: file.path)
        XCTAssertEqual((attributes[.posixPermissions] as? NSNumber)?.intValue, 0o600)
        XCTAssertThrowsError(try RemoteWebCredentialFile(credential: credential, record: .init(id: credential.clientID, name: record.name, role: .operator)).write(to: file))
        XCTAssertEqual(try RemoteWebCredentialFile.load(from: file).credential, credential, "Rejected replacement must retain existing credential")
        try credential.write(to: file)
        XCTAssertEqual(try RemoteWebCredentialFile.load(from: file).record.id, credential.clientID, "Legacy browser records remain revocable")
        let publicRecord = try JSONSerialization.jsonObject(with: JSONEncoder().encode(record)) as! [String: Any]
        XCTAssertEqual(Set(publicRecord.keys), ["id", "name", "role"])
    }
}
