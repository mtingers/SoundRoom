import Darwin
import Foundation
import Security
import SoundroomRemote

struct ServerLaunchConfiguration {
    let host: String
    let port: UInt16
    let credentialFile: URL
    let remoteOnly: Bool
    let clientWebSocket: String?
    let clientOrigin: String?
    let clientName: String?
    private let requestedWebClient: Bool
    private let requestedClientName: Bool
    private let unsupportedOptions: [String]
    let rustServer: URL
    let rustDirectory: URL
    let rustPort: UInt16
    /// Extra Rust server flags, including the software callback used by tests.
    let rustServerFlags: [String]

    static func value(_ flag: String, in arguments: [String]) -> String? {
        guard let index = arguments.firstIndex(of: flag), index + 1 < arguments.count else { return nil }
        return arguments[index + 1]
    }
    init(arguments: [String] = CommandLine.arguments) {
        let removed = [
            "--standalone", "--swift-server", "--server", "--data-directory",
            "--web-origin", "--web-p12", "--web-password-file", "--web-keychain-label",
            "--web-assets", "--web-wss-port", "--diagnose-output", "--diagnostic-json",
        ]
        unsupportedOptions = removed.filter(arguments.contains)
        remoteOnly = arguments.contains("--connect") || arguments.contains("--connect-wss")
        requestedWebClient = arguments.contains("--connect-wss")
        requestedClientName = arguments.contains("--client-name")
        clientWebSocket = Self.value("--connect-wss", in: arguments)
        clientOrigin = Self.value("--client-origin", in: arguments)
        clientName = Self.value("--client-name", in: arguments)
        let clientURL = clientWebSocket.flatMap(URL.init(string:))
        host = Self.value("--connect", in: arguments) ?? clientURL?.host ?? "127.0.0.1"
        port =
            clientURL.flatMap { UInt16(exactly: $0.port ?? 443) }
            ?? Self.value("--port", in: arguments).flatMap(UInt16.init) ?? RemoteProtocol.defaultPort
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        rustDirectory =
            Self.value("--rust-data-directory", in: arguments).map { URL(fileURLWithPath: $0, isDirectory: true) }
            ?? support.appendingPathComponent("Soundroom/RustServer", isDirectory: true)
        credentialFile =
            Self.value("--credential-file", in: arguments).map { URL(fileURLWithPath: $0) }
            ?? rustDirectory.appendingPathComponent("gui-credential.json")
        rustServer =
            Self.value("--rust-server", in: arguments).map { URL(fileURLWithPath: $0) }
            ?? ProcessInfo.processInfo.environment["SOUNDROOM_RUST_SERVER"].map { URL(fileURLWithPath: $0) }
            ?? Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/soundroom-server")
        rustPort = Self.value("--rust-port", in: arguments).flatMap(UInt16.init) ?? 49160
        rustServerFlags = arguments.indices.filter { arguments[$0] == "--rust-server-flag" && $0 + 1 < arguments.count }
            .map { arguments[$0 + 1] }
    }
    func validate() throws {
        guard unsupportedOptions.isEmpty else {
            throw RemoteError.rejected(
                "Soundroom is a server client. Removed options: \(unsupportedOptions.joined(separator: ", ")). Use the Rust soundroom-server executable for audio hosting."
            )
        }
        _ = try clientWebEndpoint()
    }
    func clientWebEndpoint() throws -> (url: URL, origin: URL)? {
        guard unsupportedOptions.isEmpty else {
            throw RemoteError.rejected("Legacy standalone and Swift-server modes have been removed.")
        }
        if requestedClientName {
            guard remoteOnly, let clientName else {
                throw RemoteError.rejected("Use --client-name NAME in remote client mode.")
            }
            try RemoteClientCredential.validateName(clientName)
        }
        guard requestedWebClient || clientOrigin != nil else { return nil }
        guard remoteOnly, let raw = clientWebSocket, let value = clientOrigin,
            let url = URL(string: raw), let origin = URL(string: value)
        else {
            throw RemoteError.rejected(
                "Use --connect-wss WSS_URL with --client-origin HTTPS_ORIGIN in remote client mode.")
        }
        try RemoteClient.validateWebEndpoint(websocketURL: url, origin: origin)
        return (url, origin)
    }
    func validatedClientName(for credential: RemoteClientCredential) throws -> String {
        if requestedClientName {
            guard remoteOnly, let clientName else {
                throw RemoteError.rejected("Use --client-name NAME in remote client mode.")
            }
            try RemoteClientCredential.validateName(clientName)
        }
        if clientWebSocket == nil { _ = try credential.requireNativeCredential() }
        return try credential.connectionName(clientName, nativeDefault: "Soundroom GUI")
    }
}

@MainActor enum ServerBootstrap {
    nonisolated(unsafe) static var launchedRustServer: Process?
    static func connectRust(store: SessionStore, configuration: ServerLaunchConfiguration, executable: URL) async throws
        -> RemoteGUIController
    {
        let directory = configuration.rustDirectory
        try FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        let log = directory.appendingPathComponent("server.log")
        if !FileManager.default.fileExists(atPath: log.path) {
            FileManager.default.createFile(atPath: log.path, contents: nil, attributes: [.posixPermissions: 0o600])
        }
        let handle = try FileHandle(forWritingTo: log)
        try handle.seekToEnd()
        func launch() throws -> Process {
            let child = Process()
            child.executableURL = executable
            child.arguments =
                ["serve-local", directory.path, "--port", String(configuration.rustPort)]
                + configuration.rustServerFlags
            child.standardOutput = handle
            child.standardError = handle
            child.standardInput = FileHandle.nullDevice
            try child.run()
            return child
        }
        // A second launch exits on the server's lock; the running host is reused.
        var child = try launch()
        defer { launchedRustServer = child }
        let credentialURL = directory.appendingPathComponent("gui-credential.json")
        let caURL = directory.appendingPathComponent("tls/ca.crt")
        // Files exist before the listener binds; a refused first attempt would leave
        // Network.framework waiting. Connect only once loopback accepts.
        var listening = false
        for _ in 0..<200 {
            if Self.loopbackAccepts(port: configuration.rustPort) {
                listening = true
                break
            }
            // A previous host may still be releasing the storage lock after it
            // stopped listening; start again once our candidate has exited.
            if !child.isRunning { child = try launch() }
            try await Task.sleep(for: .milliseconds(50))
        }
        guard listening else { throw RemoteError.rejected("The local Rust server is not listening. See \(log.path).") }
        // Startup may replace a revoked credential or regenerate the identity.
        // Read both only after this host has completed initialization and bound.
        var credential: RemoteClientCredential?
        var ca: SecCertificate?
        for _ in 0..<100 {
            credential = try? RemoteClientCredential.load(from: credentialURL)
            if let pem = try? String(contentsOf: caURL, encoding: .utf8),
                let der = Data(
                    base64Encoded: pem.components(separatedBy: .newlines).filter { !$0.hasPrefix("---") }.joined())
            {
                ca = SecCertificateCreateWithData(nil, der as CFData)
            }
            if credential != nil, ca != nil { break }
            try await Task.sleep(for: .milliseconds(100))
        }
        guard let credential, let ca else {
            throw RemoteError.rejected("The local Rust server did not start. See \(log.path).")
        }
        let url = URL(string: "wss://localhost:\(configuration.rustPort)/control")!
        let origin = URL(string: "https://localhost")!
        let controller = RemoteGUIController(
            store: store, credential: credential, host: "localhost", port: configuration.rustPort,
            isLocalServer: true, clientName: "Soundroom GUI", webEndpoint: (url, origin))
        controller.pinnedCA = ca
        controller.pairingFile = credentialURL
        // The listener may still be binding; the client retries until it accepts.
        store.remoteGUI = controller
        controller.connect()
        return controller
    }
    nonisolated static func loopbackAccepts(port: UInt16) -> Bool {
        let fd = socket(AF_INET, SOCK_STREAM, 0)
        guard fd >= 0 else { return false }
        defer { close(fd) }
        var address = sockaddr_in()
        address.sin_family = sa_family_t(AF_INET)
        address.sin_port = port.bigEndian
        address.sin_addr.s_addr = inet_addr("127.0.0.1")
        return withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size)) == 0
            }
        }
    }
    static func connect(store: SessionStore, configuration: ServerLaunchConfiguration) async throws
        -> RemoteGUIController
    {
        try configuration.validate()
        if !configuration.remoteOnly {
            guard FileManager.default.isExecutableFile(atPath: configuration.rustServer.path) else {
                throw RemoteError.rejected(
                    "The Rust audio server is missing. Rebuild the app with scripts/build-app.sh or supply --rust-server PATH."
                )
            }
            return try await connectRust(
                store: store, configuration: configuration, executable: configuration.rustServer)
        }
        let endpoint = try configuration.clientWebEndpoint()
        let credential = try RemoteClientCredential.load(from: configuration.credentialFile)
        let name = try configuration.validatedClientName(for: credential)
        let controller = RemoteGUIController(
            store: store, credential: credential, host: configuration.host, port: configuration.port,
            isLocalServer: false, clientName: name, webEndpoint: endpoint)
        controller.pairingFile = configuration.credentialFile
        store.remoteGUI = controller
        controller.connect()
        return controller
    }
}
