import Foundation
import SoundroomCore
import SoundroomRemote

@MainActor final class TUIConnection {
    let client: RemoteClient
    private var projectionCache = TUIProjectionCache()
    var snapshot: RemoteSnapshot? { client.snapshot }
    var onChange: ((RemoteEvent) -> Void)?
    var onState: ((RemoteConnectionState) -> Void)?
    convenience init(credential: RemoteCredential, clientID: UUID = UUID()) {
        self.init(credential: .native(credential), clientID: clientID)
    }
    init(credential: RemoteClientCredential, clientID: UUID = UUID(), clientName: String = "Soundroom TUI") {
        client = RemoteClient(credential: credential, name: clientName, clientID: clientID)
        client.autoReconnect = false
        client.onEvent = { [weak self] event in self?.receive(event) }
        client.onState = { [weak self] state in self?.onState?(state) }
    }
    func connect(host: String, port: UInt16) async throws {
        try await client.connect(host: host, port: port)
        try await awaitSnapshot()
    }
    func connect(websocketURL: URL, origin: URL) async throws {
        try await client.connect(websocketURL: websocketURL, origin: origin)
        try await awaitSnapshot()
    }
    private func awaitSnapshot() async throws {
        let deadline = ProcessInfo.processInfo.systemUptime + 10
        while snapshot == nil {
            guard ProcessInfo.processInfo.systemUptime < deadline else {
                client.disconnect()
                throw RemoteError.timeout
            }
            try await Task.sleep(nanoseconds: 50_000_000)
        }
    }
    private func receive(_ event: RemoteEvent) { onChange?(event) }
    func perform(_ intent: TUIIntent, model: TUIModel) async throws -> String {
        guard let snapshot else { throw RemoteError.connectionLost }
        let plan = try TUICommandAdapter.plan(intent, model: model, snapshot: snapshot)
        guard let command = plan.command else { return plan.message ?? "" }
        var revisions: [UUID: Int] = [:]
        for id in plan.trackIDs { revisions[id] = snapshot.trackRevisions[id] ?? 0 }
        let timeout: Double
        switch command {
        case .exportMix, .openProject, .saveProject: timeout = 600
        case .editor(let e) where e.operation == "clips.combine": timeout = 600
        default: timeout = 30
        }
        let reply = try await client.request(command, expectedTrackRevisions: revisions, timeout: timeout)
        guard reply.accepted else { throw TUICommandAdapter.CommandError("\(reply.status.rawValue): \(reply.message)") }
        if let payload = reply.payload, let string = String(data: payload, encoding: .utf8) { return string }
        return reply.message.isEmpty ? "Accepted · revision \(reply.revision)" : reply.message
    }
    var projection: TUISession? {
        guard let state = snapshot else { return nil }
        return projectionCache.project(state)
    }
}

@MainActor final class TUIController {
    private let terminal = Terminal()
    private let connection: TUIConnection
    private let host: String
    private var model = TUIModel()
    private var dirty = true
    private var lastFrame: TUIFrame?
    private let color: Bool
    private let mouse: Bool
    private var redraw: DispatchSourceTimer?
    private var pending: [(TUIIntent, TUIModel)] = []
    private var draining = false
    private var urgent: [(TUIIntent, TUIModel)] = []
    private var urgentDraining = false
    private var finished: CheckedContinuation<Void, Never>?
    private var closed = false
    init(connection: TUIConnection, host: String, color: Bool = true, mouse: Bool = true, model: TUIModel = TUIModel())
    {
        self.connection = connection
        self.host = host
        self.color = color
        self.mouse = mouse
        self.model = model
    }
    func run() async throws {
        terminal.mouseEnabled = mouse
        try terminal.start()
        if let snapshot = connection.projection { model.update(snapshot) }
        model.connected = true
        model.role = connection.client.credential.role.rawValue
        model.status = "Navigation stays local. Enter seeks shared transport; ? opens help."
        terminal.onKey = { [weak self] key in
            guard let self else { return }
            let intents = self.model.handle(key)
            self.dirty = true
            self.submit(intents)
        }
        terminal.onMouse = { [weak self] event in
            guard let self, let frame = self.lastFrame else { return }
            let intents = self.model.handleMouse(event, frame: frame)
            self.dirty = true
            self.submit(intents)
        }
        terminal.onPaste = { [weak self] text in
            guard let self else { return }
            if self.model.mode == .command {
                let safe = TUIRenderer.safe(text)
                var remaining = max(0, 4096 - self.model.commandText.utf8.count)
                for character in safe {
                    let bytes = String(character).utf8.count
                    guard bytes <= remaining else { break }
                    remaining -= bytes
                    self.model.commandText.append(character)
                }
            } else {
                self.model.status = "Paste text after : to enter a command. Use p to paste audio clips."
            }
            self.dirty = true
        }
        terminal.onQuit = { [weak self] in self?.close() }
        terminal.onResize = { [weak self] in self?.dirty = true }
        connection.onChange = { [weak self] event in self?.receive(event) }
        connection.onState = { [weak self] state in
            guard let self else { return }
            switch state {
            case .connected(_, _, let role):
                self.model.connected = true
                self.model.role = role.rawValue
            case .disconnected:
                self.pending = []
                self.urgent = []
                self.model.connected = false
                self.model.status = "Disconnected. Server audio continues; q detaches."
            case .failed(let error):
                self.pending = []
                self.urgent = []
                self.model.connected = false
                self.model.status = error
            case .connecting: self.model.status = "Connecting…"
            case .reconnecting(let attempt):
                self.model.connected = false
                self.model.status = "Reconnecting (\(attempt))…"
            }
            self.dirty = true
        }
        let timer = DispatchSource.makeTimerSource(queue: .main)
        timer.schedule(deadline: .now(), repeating: .milliseconds(100), leeway: .milliseconds(10))
        timer.setEventHandler { [weak self] in
            guard let self, self.dirty else { return }
            self.dirty = false
            let size = self.terminal.dimensions
            let frame = TUIRenderer.frame(
                self.model, host: self.host, width: size.width, height: size.height, color: self.color)
            self.lastFrame = frame
            self.terminal.output(frame.output)
        }
        timer.resume()
        redraw = timer
        await withCheckedContinuation { continuation in
            if closed { continuation.resume() } else { finished = continuation }
        }
    }
    private func submit(_ intents: [TUIIntent]) {
        for intent in intents {
            if intent == .quit {
                close()
                return
            }
            guard model.connected else {
                model.status = "Disconnected. q detaches; restart to reconnect."
                continue
            }
            if intent.isUrgentTransport {
                guard urgent.count < 8 else {
                    model.status = "Transport queue full; wait for the server."
                    continue
                }
                urgent.append((intent, model))
                drainUrgent()
                continue
            }
            guard pending.count < 32 else {
                model.status = "Command queue full; wait for current edits."
                continue
            }
            pending.append((intent, model))
        }
        drain()
    }
    private func receive(_ event: RemoteEvent) {
        switch event {
        case .snapshot, .delta:
            if let state = connection.projection { model.update(state) }
            dirty = true
        case .transport(let transport):
            model.session.frame = transport.frame
            model.session.playing = transport.isPlaying
            model.session.recording = transport.isRecording
            model.session.operatorName =
                connection.snapshot?.peers.first(where: { $0.id == transport.operatorID })?.name
                ?? (transport.operatorID == nil ? nil : "another client")
            dirty = true
        case .presence:
            model.session.operatorName =
                connection.snapshot?.peers.first(where: { $0.id == connection.snapshot?.transport.operatorID })?.name
            dirty = true
        case .status(let message), .job(_, _, let message):
            model.status = message
            dirty = true
        case .projects(let projects):
            model.status = projects.joined(separator: " · ")
            dirty = true
        default: break
        }
    }
    private func drain() {
        guard !draining, !pending.isEmpty, !closed else { return }
        draining = true
        Task { @MainActor [weak self] in
            guard let self else { return }
            while !self.pending.isEmpty && !self.closed {
                let (intent, context) = self.pending.removeFirst()
                do { self.model.status = try await self.connection.perform(intent, model: context) } catch {
                    self.model.status = String(describing: error)
                }
                self.dirty = true
            }
            self.draining = false
        }
    }
    private func drainUrgent() {
        guard !urgentDraining, !urgent.isEmpty, !closed else { return }
        urgentDraining = true
        Task { @MainActor [weak self] in
            guard let self else { return }
            while !self.urgent.isEmpty && !self.closed {
                let (intent, context) = self.urgent.removeFirst()
                do { self.model.status = try await self.connection.perform(intent, model: context) } catch {
                    self.model.status = String(describing: error)
                }
                self.dirty = true
            }
            self.urgentDraining = false
        }
    }
    private func close() {
        guard !closed else { return }
        closed = true
        redraw?.cancel()
        redraw = nil
        pending = []
        urgent = []
        connection.client.disconnect()
        terminal.stop()
        finished?.resume()
        finished = nil
    }
}
