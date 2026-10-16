import XCTest
@testable import SoundroomRemote

final class RemoteWebPolicyTests: XCTestCase {
    func testBrowserOriginMustBeExplicitHTTPSAndUnique() {
        let origins: Set<String> = ["https://studio.local:49154"]
        XCTAssertTrue(RemoteWebPolicy.acceptsWebSocket(protocols: ["soundroom.v3"], headers: [("Origin", "https://studio.local:49154")], origins: origins))
        for value in ["null", "http://studio.local:49154", "https://attacker.local", "https://studio.local:49154/path", "https://studio.local:49154?token=secret"] {
            XCTAssertFalse(RemoteWebPolicy.acceptsWebSocket(protocols: ["soundroom.v3"], headers: [("Origin", value)], origins: origins))
        }
        XCTAssertFalse(RemoteWebPolicy.acceptsWebSocket(protocols: [], headers: [("Origin", "https://studio.local:49154")], origins: origins))
        XCTAssertFalse(RemoteWebPolicy.acceptsWebSocket(protocols: ["soundroom.v3"], headers: [], origins: origins))
        XCTAssertFalse(RemoteWebPolicy.acceptsWebSocket(protocols: ["soundroom.v3"], headers: [("Origin", "https://studio.local:49154"), ("origin", "https://studio.local:49154")], origins: origins))
    }
    func testStaticRequestsRejectTraversalBodiesDuplicateHostAndForeignHost() throws {
        func request(_ target: String, method: String = "GET", headers: String = "Host: studio.local:49154\r\n") -> Data {
            Data("\(method) \(target) HTTP/1.1\r\n\(headers)\r\n".utf8)
        }
        XCTAssertEqual(try RemoteWebPolicy.request(request("/"), expectedHost: "studio.local:49154"), .init(path: "/index.html", head: false))
        XCTAssertEqual(try RemoteWebPolicy.request(request("/assets/client.js", method: "HEAD"), expectedHost: "studio.local:49154"), .init(path: "/assets/client.js", head: true))
        for target in ["/../operator.json", "/%2e%2e/operator.json", "/assets/%5csecret", "/file%00", "/?token=secret", "https://studio.local/"] {
            XCTAssertThrowsError(try RemoteWebPolicy.request(request(target), expectedHost: "studio.local:49154"))
        }
        for headers in ["Host: other.local\r\n", "Host: studio.local:49154\r\nHost: studio.local:49154\r\n", "Host: studio.local:49154\r\nContent-Length: 1\r\n", "Host: studio.local:49154\r\nTransfer-Encoding: chunked\r\n"] {
            XCTAssertThrowsError(try RemoteWebPolicy.request(request("/", headers: headers), expectedHost: "studio.local:49154"))
        }
        XCTAssertThrowsError(try RemoteWebPolicy.request(request("/", method: "POST"), expectedHost: "studio.local:49154"))
        XCTAssertThrowsError(try RemoteWebPolicy.request(Data(repeating: 65, count: 16_385), expectedHost: "studio.local:49154"))
    }
    func testSecretReaderRejectsWorldReadableFilesAndLinks() throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let secret = folder.appendingPathComponent("secret")
        try Data("private".utf8).write(to: secret)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: secret.path)
        XCTAssertEqual(try RemotePrivateFile.read(secret), Data("private".utf8))
        let link = folder.appendingPathComponent("link")
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: secret)
        XCTAssertThrowsError(try RemotePrivateFile.read(link))
        XCTAssertThrowsError(try RemotePrivateFile.read(secret, maximumBytes: 2))
        try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: secret.path)
        XCTAssertThrowsError(try RemotePrivateFile.read(secret))
    }
}
