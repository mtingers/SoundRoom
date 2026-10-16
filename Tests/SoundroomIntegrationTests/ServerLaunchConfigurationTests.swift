import XCTest
import SoundroomRemote
@testable import Soundroom

final class ServerLaunchConfigurationTests: XCTestCase {
    func testCommonWebClientIsRemoteOnlyAndRequiresCompleteSecureEndpoint() throws {
        let configuration = ServerLaunchConfiguration(arguments: ["Soundroom", "--connect-wss", "wss://studio.local:49155/", "--client-origin", "https://studio.local:49154"])
        XCTAssertTrue(configuration.remoteOnly)
        let endpoint = try XCTUnwrap(configuration.clientWebEndpoint())
        XCTAssertEqual(endpoint.url.host, "studio.local"); XCTAssertEqual(endpoint.origin.scheme, "https")
        for arguments in [["--connect-wss"], ["--connect-wss", "wss://studio.local/"],
                          ["--client-origin", "https://studio.local"],
                          ["--connect-wss", "ws://studio.local/", "--client-origin", "https://studio.local"],
                          ["--connect-wss", "wss://studio.local:70000/", "--client-origin", "https://studio.local"],
                          ["--connect-wss", "wss://studio.local/?token=secret", "--client-origin", "https://studio.local"],
                          ["--standalone", "--connect-wss", "wss://studio.local/", "--client-origin", "https://studio.local"]] {
            XCTAssertThrowsError(try ServerLaunchConfiguration(arguments: ["Soundroom"] + arguments).clientWebEndpoint())
        }
    }
    func testDefaultLaunchRequiresTheRustServer() throws {
        let configuration = ServerLaunchConfiguration(arguments: ["Soundroom"])
        XCTAssertFalse(configuration.remoteOnly); XCTAssertNil(try configuration.clientWebEndpoint())
        XCTAssertEqual(configuration.host, "127.0.0.1")
    }
    func testRetiredModesAreRejectedBeforeStartup() {
        for flag in ["--standalone", "--swift-server", "--server", "--diagnose-output", "--data-directory", "--web-origin"] {
            XCTAssertThrowsError(try ServerLaunchConfiguration(arguments: ["Soundroom", flag]).validate(), flag)
        }
    }
    @MainActor func testMissingRustServerCannotFallBackToAnInProcessEngine() async {
        let store = SessionStore(headless: true)
        let configuration = ServerLaunchConfiguration(arguments: ["Soundroom", "--rust-server", "/nonexistent/soundroom-server"])
        do {
            _ = try await ServerBootstrap.connect(store: store, configuration: configuration)
            XCTFail("A missing Rust server must fail explicitly")
        } catch {
            XCTAssertTrue(error.localizedDescription.contains("Rust audio server is missing"))
            XCTAssertNil(store.remoteGUI)
            XCTAssertFalse(store.isPlaying)
            XCTAssertFalse(store.isRecording)
        }
    }
    func testPortableCredentialRequiresWSSAndExactApprovedName() throws {
        let web = RemoteClientCredential.web(RemoteBrowserCredential(serverID: UUID(), credentialID: UUID(), clientSecret: Data(repeating: 7, count: 32), role: .editor))
        let flags = ["Soundroom", "--connect-wss", "wss://studio.local/control", "--client-origin", "https://studio.local"]
        XCTAssertThrowsError(try ServerLaunchConfiguration(arguments: flags).validatedClientName(for: web))
        let configuration = ServerLaunchConfiguration(arguments: flags + ["--client-name", "Studio GUI"])
        XCTAssertEqual(try configuration.validatedClientName(for: web), "Studio GUI")
        XCTAssertThrowsError(try ServerLaunchConfiguration(arguments: ["Soundroom", "--connect", "studio.local", "--client-name", "Studio GUI"]).validatedClientName(for: web))
        XCTAssertThrowsError(try ServerLaunchConfiguration(arguments: flags + ["--client-name", "\n"]).validatedClientName(for: web))
        XCTAssertThrowsError(try ServerLaunchConfiguration(arguments: flags + ["--client-name"]).clientWebEndpoint())
        XCTAssertThrowsError(try ServerLaunchConfiguration(arguments: ["Soundroom", "--client-name", "Local"]).clientWebEndpoint())
    }
}
