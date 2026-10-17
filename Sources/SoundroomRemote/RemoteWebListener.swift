import Foundation
import Network
import Security

public struct RemoteWebConfiguration: @unchecked Sendable {
    public let identity: SecIdentity
    public let assetDirectory: URL
    public let origin: URL
    public let websocketPort: UInt16
    public let allowedOrigins: Set<String>
    public var httpPort: UInt16 { UInt16(origin.port ?? 443) }
    public init(
        identity: SecIdentity, assetDirectory: URL, origin: URL, websocketPort: UInt16,
        allowedOrigins: Set<String> = []
    ) throws {
        guard let canonical = RemoteWebPolicy.canonicalOrigin(origin.absoluteString),
            websocketPort > 0, Int(websocketPort) != (origin.port ?? 443),
            (origin.port ?? 443) > 0, (origin.port ?? 443) <= 65535
        else { throw RemoteError.invalidFrame }
        let extra = try allowedOrigins.map { value -> String in
            guard let normalized = RemoteWebPolicy.canonicalOrigin(value) else { throw RemoteError.invalidFrame }
            return normalized
        }
        self.identity = identity
        self.assetDirectory = assetDirectory
        self.origin = URL(string: canonical)!
        self.websocketPort = websocketPort
        self.allowedOrigins = Set(extra).union([canonical])
    }
    public var websocketURL: String {
        var value = URLComponents(url: origin, resolvingAgainstBaseURL: false)!
        value.scheme = "wss"
        value.port = Int(websocketPort)
        value.path = "/"
        return value.string!
    }
    /// Imports into process memory on modern macOS, never the user's login keychain.
    /// Older hosts can pass an existing SecIdentity to the initializer instead.
    public static func loadIdentity(pkcs12: URL, password: String) throws -> SecIdentity {
        guard #available(macOS 15, *) else {
            throw RemoteError.rejected(
                "PKCS#12 memory import requires macOS 15. On macOS 14, use a provisioned Keychain identity.")
        }
        let data = try RemotePrivateFile.read(pkcs12, maximumBytes: 1024 * 1024)
        guard data.count > 0, data.count <= 1024 * 1024 else { throw RemoteError.invalidCredential }
        var imported: CFArray?
        let options =
            [kSecImportExportPassphrase as String: password, kSecImportToMemoryOnly as String: true] as [String: Any]
        let status = SecPKCS12Import(data as CFData, options as CFDictionary, &imported)
        guard status == errSecSuccess, let items = imported as? [[String: Any]],
            let raw = items.first?[kSecImportItemIdentity as String],
            CFGetTypeID(raw as CFTypeRef) == SecIdentityGetTypeID()
        else {
            throw RemoteError.rejected("The web TLS identity could not be opened (Security \(status)).")
        }
        return unsafeBitCast(raw as CFTypeRef, to: SecIdentity.self)
    }
    public static func keychainIdentity(label: String) throws -> SecIdentity {
        var raw: CFTypeRef?
        let query =
            [
                kSecClass as String: kSecClassIdentity, kSecAttrLabel as String: label,
                kSecReturnRef as String: true, kSecMatchLimit as String: kSecMatchLimitOne,
            ] as [String: Any]
        let status = SecItemCopyMatching(query as CFDictionary, &raw)
        guard status == errSecSuccess, let raw, CFGetTypeID(raw) == SecIdentityGetTypeID() else {
            throw RemoteError.rejected("The provisioned web TLS identity was not found (Security \(status)).")
        }
        return unsafeBitCast(raw, to: SecIdentity.self)
    }
}

/// Pure HTTP/origin policy, independently testable without listening sockets.
enum RemoteWebPolicy {
    static let maximumHeaderBytes = 16 * 1024
    static func canonicalOrigin(_ value: String) -> String? {
        guard let url = URLComponents(string: value), url.scheme?.lowercased() == "https",
            let host = url.host, !host.isEmpty, url.user == nil, url.password == nil,
            url.query == nil, url.fragment == nil, url.path.isEmpty || url.path == "/",
            url.port == nil || (1...65535).contains(url.port!)
        else { return nil }
        var normalized = URLComponents()
        normalized.scheme = "https"
        normalized.host = host.lowercased()
        normalized.port = url.port == 443 ? nil : url.port
        return normalized.string
    }
    static func acceptsWebSocket(protocols: [String], headers: [(name: String, value: String)], origins: Set<String>)
        -> Bool
    {
        let originHeaders = headers.filter { $0.name.lowercased() == "origin" }
        guard protocols.contains("soundroom.v3"), originHeaders.count == 1,
            let canonical = canonicalOrigin(originHeaders[0].value), origins.contains(canonical)
        else { return false }
        return true
    }
    struct Request: Equatable {
        var path: String
        var head: Bool
    }
    static func request(_ bytes: Data, expectedHost: String) throws -> Request {
        guard bytes.count <= maximumHeaderBytes, let text = String(data: bytes, encoding: .utf8),
            text.hasSuffix("\r\n\r\n"), !text.contains("\0")
        else { throw RemoteError.invalidFrame }
        let lines = text.components(separatedBy: "\r\n")
        let parts = lines[0].split(separator: " ", omittingEmptySubsequences: false)
        guard parts.count == 3, parts[0] == "GET" || parts[0] == "HEAD", parts[2] == "HTTP/1.1",
            parts[1].hasPrefix("/"), !parts[1].contains("?"), !parts[1].contains("#"),
            let path = String(parts[1]).removingPercentEncoding,
            !path.contains("\\"), !path.contains("\0"), !path.split(separator: "/").contains("..")
        else { throw RemoteError.invalidFrame }
        var headers: [String: String] = [:]
        for line in lines.dropFirst().filter({ !$0.isEmpty }) {
            guard let colon = line.firstIndex(of: ":"), !line.hasPrefix(" "), !line.hasPrefix("\t") else {
                throw RemoteError.invalidFrame
            }
            let key = line[..<colon].lowercased()
            let value = line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)
            guard headers[key] == nil else { throw RemoteError.invalidFrame }
            headers[key] = value
        }
        guard headers["host"]?.lowercased() == expectedHost.lowercased(), headers["transfer-encoding"] == nil,
            headers["content-length"] == nil || headers["content-length"] == "0"
        else { throw RemoteError.invalidFrame }
        return Request(path: path == "/" ? "/index.html" : path, head: parts[0] == "HEAD")
    }
}

/// HTTPS asset hosting and WSS share a TLS identity. Separate listeners allow
/// Apple's websocket implementation to own HTTP upgrade/framing completely.
final class RemoteWebListener: @unchecked Sendable {
    private let configuration: RemoteWebConfiguration
    private let queue = DispatchQueue(label: "soundroom.web.listener", qos: .utility)
    private var http: NWListener?
    private var websocket: NWListener?
    private let httpGate = RemoteAdmissionGate(capacity: 16)
    private var assets: [String: (Data, String)] = [:]
    private var requests: [UUID: RemoteHTTPConnection] = [:]
    var onConnection: (@Sendable (NWConnection) -> Void)?
    var onError: (@Sendable (String) -> Void)?
    init(configuration: RemoteWebConfiguration) throws {
        self.configuration = configuration
        let root = configuration.assetDirectory.standardizedFileURL.resolvingSymlinksInPath()
        guard
            let files = FileManager.default.enumerator(
                at: root, includingPropertiesForKeys: [.isRegularFileKey, .isSymbolicLinkKey],
                options: [.skipsHiddenFiles])
        else {
            throw RemoteError.rejected("The browser client assets are missing. Build web/ first.")
        }
        var total = 0
        for case let file as URL in files {
            let values = try file.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey])
            guard values.isSymbolicLink != true else { throw RemoteError.invalidFrame }
            guard values.isRegularFile == true else { continue }
            let canonical = file.standardizedFileURL.resolvingSymlinksInPath()
            guard canonical.path.hasPrefix(root.path + "/") else { throw RemoteError.invalidFrame }
            let size = try file.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
            guard size <= 16 * 1024 * 1024, total <= 32 * 1024 * 1024 - size else { throw RemoteError.oversizedFrame }
            total += size
            let path = String(canonical.path.dropFirst(root.path.count))
            assets[path] = (try Data(contentsOf: canonical), Self.mime(file.pathExtension))
        }
        guard assets["/index.html"] != nil else {
            throw RemoteError.rejected("The browser client index.html is missing.")
        }
        assets["/soundroom-config.json"] = (
            try JSONSerialization.data(withJSONObject: ["version": 3, "wssURL": configuration.websocketURL]),
            "application/json"
        )
    }
    private func parameters() throws -> NWParameters {
        guard let identity = sec_identity_create(configuration.identity) else { throw RemoteError.invalidCredential }
        let tls = NWProtocolTLS.Options()
        sec_protocol_options_set_min_tls_protocol_version(tls.securityProtocolOptions, .TLSv12)
        sec_protocol_options_set_local_identity(tls.securityProtocolOptions, identity)
        let tcp = NWProtocolTCP.Options()
        tcp.noDelay = true
        let value = NWParameters(tls: tls, tcp: tcp)
        value.allowLocalEndpointReuse = true
        return value
    }
    func start() throws {
        let secureHTTP = try NWListener(using: parameters(), on: NWEndpoint.Port(rawValue: configuration.httpPort)!)
        let secureWSParameters = try parameters()
        let options = NWProtocolWebSocket.Options()
        options.autoReplyPing = true
        options.maximumMessageSize = RemoteProtocol.maximumFrameBytes
        let origins = configuration.allowedOrigins
        options.setClientRequestHandler(queue) { protocols, headers in
            let accepted = RemoteWebPolicy.acceptsWebSocket(protocols: protocols, headers: headers, origins: origins)
            return .init(status: accepted ? .accept : .reject, subprotocol: accepted ? "soundroom.v3" : nil)
        }
        secureWSParameters.defaultProtocolStack.applicationProtocols.insert(options, at: 0)
        let secureWS = try NWListener(
            using: secureWSParameters, on: NWEndpoint.Port(rawValue: configuration.websocketPort)!)
        http = secureHTTP
        websocket = secureWS
        secureWS.newConnectionHandler = { [weak self] connection in
            guard let self, let handler = self.onConnection else {
                connection.cancel()
                return
            }
            handler(connection)
        }
        secureHTTP.newConnectionHandler = { [weak self] connection in
            guard let self, let permit = self.httpGate.acquire() else {
                connection.cancel()
                return
            }
            let id = UUID()
            let host = String(self.configuration.origin.absoluteString.dropFirst("https://".count))
            let request = RemoteHTTPConnection(
                connection: connection, queue: self.queue, permit: permit,
                host: host, assets: self.assets, websocketURL: self.configuration.websocketURL)
            self.requests[id] = request
            request.onClose = { [weak self] in self?.requests.removeValue(forKey: id) }
            request.start()
        }
        for listener in [secureHTTP, secureWS] {
            listener.stateUpdateHandler = { [weak self] state in
                if case .failed(let error) = state { self?.onError?(error.localizedDescription) }
            }
            listener.start(queue: queue)
        }
    }
    func stop() {
        queue.async { [self] in
            httpGate.close()
            http?.cancel()
            websocket?.cancel()
            http = nil
            websocket = nil
            let active = Array(requests.values)
            requests.removeAll()
            active.forEach { $0.close() }
        }
    }
    private static func mime(_ suffix: String) -> String {
        switch suffix.lowercased() {
        case "html": return "text/html; charset=utf-8"
        case "js": return "text/javascript; charset=utf-8"
        case "css": return "text/css; charset=utf-8"
        case "json": return "application/json"
        case "svg": return "image/svg+xml"
        case "png": return "image/png"
        case "woff2": return "font/woff2"
        default: return "application/octet-stream"
        }
    }
}

private final class RemoteHTTPConnection {
    let connection: NWConnection
    let queue: DispatchQueue
    let permit: RemoteAdmissionGate.Permit
    let host: String
    let assets: [String: (Data, String)]
    let websocketURL: String
    var incoming = Data()
    var closed = false
    var watchdog: DispatchSourceTimer?
    var onClose: (() -> Void)?
    init(
        connection: NWConnection, queue: DispatchQueue, permit: RemoteAdmissionGate.Permit, host: String,
        assets: [String: (Data, String)], websocketURL: String
    ) {
        self.connection = connection
        self.queue = queue
        self.permit = permit
        self.host = host
        self.assets = assets
        self.websocketURL = websocketURL
    }
    func start() {
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now() + 10)
        timer.setEventHandler { [weak self] in self?.close() }
        watchdog = timer
        timer.resume()
        connection.stateUpdateHandler = { [weak self] state in
            guard let self else { return }
            switch state {
            case .ready: self.receive()
            case .failed, .cancelled: self.close()
            default: break
            }
        }
        connection.start(queue: queue)
    }
    func receive() {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 4096) { [weak self] data, _, complete, error in
            guard let self, !self.closed else { return }
            guard error == nil, let data, !data.isEmpty,
                self.incoming.count <= RemoteWebPolicy.maximumHeaderBytes - data.count
            else {
                self.close()
                return
            }
            self.incoming.append(data)
            if let end = self.incoming.range(of: Data("\r\n\r\n".utf8)) {
                // One request per connection. Reject pipelined/body bytes.
                guard end.upperBound == self.incoming.endIndex,
                    let request = try? RemoteWebPolicy.request(self.incoming, expectedHost: self.host)
                else {
                    self.respond(status: "400 Bad Request", body: Data(), mime: "text/plain", head: false)
                    return
                }
                if let asset = self.assets[request.path] {
                    self.respond(status: "200 OK", body: asset.0, mime: asset.1, head: request.head)
                } else {
                    self.respond(
                        status: "404 Not Found", body: Data("Not found".utf8), mime: "text/plain", head: request.head)
                }
            } else if complete {
                self.close()
            } else {
                self.receive()
            }
        }
    }
    func respond(status: String, body: Data, mime: String, head: Bool) {
        let csp =
            "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self' \(websocketURL); img-src 'self' data:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"
        let headers =
            "HTTP/1.1 \(status)\r\nContent-Type: \(mime)\r\nContent-Length: \(body.count)\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: \(csp)\r\nReferrer-Policy: no-referrer\r\n\r\n"
        var response = Data(headers.utf8)
        if !head { response.append(body) }
        connection.send(content: response, completion: .contentProcessed { [weak self] _ in self?.close() })
    }
    func close() {
        guard !closed else { return }
        closed = true
        watchdog?.cancel()
        watchdog = nil
        connection.stateUpdateHandler = nil
        connection.cancel()
        onClose?()
        onClose = nil
    }
    deinit {
        watchdog?.cancel()
        connection.cancel()
    }
}
