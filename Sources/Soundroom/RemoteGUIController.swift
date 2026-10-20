import Combine
import CryptoKit
import Foundation
import Security
import SoundroomCore
import SoundroomEngineControl
import SoundroomRemote

/// UI drafts and navigation live in SessionStore. This adapter alone sends
/// their explicit targets to the hardware host; no client owns an audio clock.
@MainActor final class RemoteGUIController: ObservableObject {
    typealias CommandSender = @MainActor (RemoteCommand, [UUID: Int], Int?, Double) async throws -> RemoteReply
    weak var store: SessionStore?
    let client: RemoteClient
    private let commandSender: CommandSender?
    func requestFile(_ command: RemoteCommand, timeout: Double = 30) async throws -> RemoteReply {
        if let commandSender { return try await commandSender(command, [:], nil, timeout) }
        return try await client.request(command, timeout: timeout)
    }
    func requestUploadBatch(id: UUID, offset: Int, chunks: [Data]) async throws -> [RemoteReply] {
        if let commandSender {
            let generation = connectionGeneration
            var replies: [RemoteReply] = []
            var position = offset
            for chunk in chunks {
                try requireImportConnection(generation)
                replies.append(
                    try await commandSender(.uploadChunk(id: id, offset: position, data: chunk), [:], nil, 30))
                try requireImportConnection(generation)
                position += chunk.count
            }
            return replies
        }
        return try await client.uploadChunks(id: id, offset: offset, chunks: chunks)
    }
    func requireImportConnection(_ generation: Int) throws {
        guard connected, connectionGeneration == generation else { throw RemoteError.connectionLost }
    }
    @Published private(set) var state = RemoteConnectionState.disconnected
    @Published private(set) var serverName = "Connecting…"
    @Published private(set) var peers: [RemotePeer] = []
    @Published var showConnection = false { didSet { store?.objectWillChange.send() } }
    @Published private(set) var lastError: String?
    @Published private(set) var projectSaveStatus = ProjectSaveStatus.unverified
    private var savedServerRevision: Int?
    private var pendingSavedDocument: (document: Session, revision: Int)?
    private(set) var serverProjectName: String?
    private(set) var projectFileURL: URL?
    private var projectServerID: UUID?
    private var currentProjectID: UUID?
    private(set) var projectNavigationGeneration = 0
    @Published var isSavingProject = false { didSet { store?.objectWillChange.send() } }
    @Published private(set) var activeWork: String?
    @Published private(set) var canUndo = false
    @Published private(set) var canRedo = false
    @Published private(set) var undoName = "Undo"
    @Published private(set) var redoName = "Redo"
    let host: String
    let port: UInt16
    let isLocalServer: Bool
    let webEndpoint: (url: URL, origin: URL)?
    /// Private CA of a local Rust host, anchored for this connection only.
    var pinnedCA: SecCertificate?
    var pairingFile: URL?
    private(set) var revision = 0
    private(set) var trackRevisions: [UUID: Int] = [:]
    private var waveformCache = RemoteWaveformCache()
    private var waveformQueue = RemoteWaveformQueue()
    private var waveformGeneration = 0
    private var mediaRequests = 0
    private var pending: [(RemoteCommand, [UUID: Int], ((RemoteReply) -> Void)?, RemoteGestureRevisionFence?, Int?)] =
        []
    private var commandTask: Task<Void, Never>?
    private(set) var authoritativeSession: Session?
    private var authoritativeServerID: UUID?
    private var activeGesture: (key: String, fence: RemoteGestureRevisionFence)?
    private var inFlightGesture: RemoteGestureRevisionFence?
    private var inFlightCommand: RemoteCommand?
    private(set) var connectionGeneration = 0
    /// The connection whose first snapshot already received this client's
    /// server-held preferences (the transport click).
    private var preferencesSentGeneration: Int?
    private struct MetadataWait {
        let fence: RemoteGestureRevisionFence
        let command: RemoteCommand
        let reply: RemoteReply
        let continuation: CheckedContinuation<Bool, Never>
    }
    private var metadataWait: MetadataWait?
    private var metadataTimeout: Task<Void, Never>?
    @Published private(set) var unsentEdit: RemoteTrackEdit?
    private(set) var effectMeters: [UUID: EffectMeterSnapshot] = [:]
    private var urgentRequests = 0
    private weak var midiReceiver: DesignerMIDIInput?
    private var midiSources: [RemoteMIDISource] = []
    private var connectionTask: Task<Void, Never>?
    private(set) var applyingServerState = false
    private(set) var recordingStartFrame = 0
    private(set) var transportPreparing = false
    var designerBeat: Double {
        designerServerBeat
            + (store?.designerPreviewing == true
                ? max(0, ProcessInfo.processInfo.systemUptime - designerReceived) * designerBPM / 60 : 0)
    }
    private var designerServerBeat = 0.0
    private var designerReceived = ProcessInfo.processInfo.systemUptime
    var designerPreviewStart: Double?
    var designerBPM: Double = 120
    @Published var showProjectPicker = false { didSet { store?.objectWillChange.send() } }
    @Published var projects: [String] = []

    convenience init(
        store: SessionStore, credential: RemoteCredential, host: String, port: UInt16 = RemoteProtocol.defaultPort,
        isLocalServer: Bool = false,
        webEndpoint: (url: URL, origin: URL)? = nil
    ) {
        self.init(
            store: store, credential: .native(credential), host: host, port: port, isLocalServer: isLocalServer,
            webEndpoint: webEndpoint)
    }
    init(
        store: SessionStore, credential: RemoteClientCredential, host: String,
        port: UInt16 = RemoteProtocol.defaultPort, isLocalServer: Bool = false,
        clientName: String = "Soundroom GUI",
        webEndpoint: (url: URL, origin: URL)? = nil, commandSender: CommandSender? = nil
    ) {
        self.store = store
        self.host = host
        self.port = port
        self.isLocalServer = isLocalServer
        self.commandSender = commandSender
        self.webEndpoint = webEndpoint
        client = RemoteClient(credential: credential, name: clientName)
        client.onEvent = { [weak self] event in self?.receive(event) }
        client.onState = { [weak self] state in self?.receiveState(state) }
    }
    func receiveState(_ state: RemoteConnectionState) {
        if case .connected = state {} else { invalidateConnectionEdits() }
        self.state = state
        if case let .connected(_, name, _) = state {
            serverName = name
            lastError = nil
        }
        if case let .failed(message) = state {
            lastError = message
            store?.recordFailure(message)
        }
        store?.objectWillChange.send()
    }
    private func request(
        _ command: RemoteCommand, expecting versions: [UUID: Int] = [:], expectedRevision: Int? = nil,
        timeout: Double = 30
    ) async throws -> RemoteReply {
        if let commandSender { return try await commandSender(command, versions, expectedRevision, timeout) }
        let id: UUID
        if case .editor(let intent) = command, intent.flags["separateSource"] == true,
            ["effects.preview", "effects.apply"].contains(intent.operation),
            let text = intent.strings["separationJobID"], let jobID = UUID(uuidString: text)
        {
            id = jobID
        } else {
            id = UUID()
        }
        return try await client.request(
            command, expectedRevision: expectedRevision, expectedTrackRevisions: versions, id: id, timeout: timeout)
    }
    var connected: Bool {
        if case .connected = state { return true }
        return false
    }
    var projectSaveHelp: String {
        if let projectFileURL { return "Save to \(projectFileURL.path)" }
        if let serverProjectName { return "Save \(serverProjectName) on \(serverName)" }
        return isLocalServer ? "Choose where to save this project" : "Save a named project on \(serverName)"
    }
    func adoptProjectDestination(serverName: String?, file: URL?) {
        serverProjectName = serverName.map { $0.hasSuffix(".soundroom") ? $0 : $0 + ".soundroom" }
        projectFileURL = file
        store?.projectURL = file
    }
    func openedProjectDestination(serverName: String?, file: URL?, projectID: UUID? = nil) {
        projectNavigationGeneration &+= 1
        savedServerRevision = nil
        projectSaveStatus = .unverified
        pendingSavedDocument = nil
        if let projectID { currentProjectID = projectID }
        adoptProjectDestination(serverName: serverName, file: file)
    }
    func confirmProjectFileSaved(_ document: Session, at target: URL) {
        let matches = authoritativeSession == document && store?.session == document && pending.isEmpty
        savedServerRevision = matches ? revision : nil
        projectSaveStatus = matches ? .saved : .modified
        pendingSavedDocument = matches ? nil : (document, revision)
        store?.rememberSavedProject(target)
        let message =
            matches ? "Saved: \(target.lastPathComponent)" : "Project saved. The session has newer unsaved changes."
        store?.status = message
        store?.activity.record(message, kind: .success)
    }
    private func confirmPendingSaveMetadata() {
        guard let saved = pendingSavedDocument else { return }
        if revision == saved.revision && authoritativeSession == saved.document && store?.session == saved.document
            && pending.isEmpty
        {
            savedServerRevision = revision
            projectSaveStatus = .saved
            pendingSavedDocument = nil
        } else if revision > saved.revision {
            pendingSavedDocument = nil
        }
    }
    var connectionLabel: String {
        switch state {
        case .connected: return isLocalServer ? "Local server" : serverName
        case .connecting: return "Connecting…"
        case .reconnecting: return "Reconnecting…"
        case .failed, .disconnected: return "Disconnected"
        }
    }
    func connect() {
        connectionTask?.cancel()
        connectionTask = Task { [weak self] in
            guard let self else { return }
            do {
                if let webEndpoint, let pinnedCA {
                    try await client.connect(
                        websocketURL: webEndpoint.url, origin: webEndpoint.origin, pinnedCA: pinnedCA)
                } else if let webEndpoint {
                    try await client.connect(websocketURL: webEndpoint.url, origin: webEndpoint.origin)
                } else {
                    try await client.connect(host: host, port: port)
                }
            } catch {
                lastError = error.localizedDescription
                store?.status = error.localizedDescription
            }
        }
    }
    func beginGesture(_ key: String) {
        guard activeGesture?.key != key else { return }
        activeGesture = (key, RemoteGestureRevisionFence(versions: trackRevisions))
    }
    func endGesture() { activeGesture = nil }
    func discardUnsentEdit() { unsentEdit = nil }
    func copyUnsentTrack(_ id: UUID) {
        guard let store, let track = unsentEdit?.tracks.first(where: { $0.id == id }) else { return }
        store.copyUnsentTrack(track)
    }
    private func preserve(_ fence: RemoteGestureRevisionFence?) {
        guard let fence else { return }
        fence.invalidate()
        // Keep one latest draft; dense clips/curves stay shared through COW.
        if let draft = fence.draft { unsentEdit = draft }
    }
    private func invalidateConnectionEdits() {
        savedServerRevision = nil
        pendingSavedDocument = nil
        projectSaveStatus = .unverified
        activeWork = nil
        transportPreparing = false
        connectionGeneration &+= 1
        preserve(inFlightGesture)
        preserve(activeGesture?.fence)
        let waiting = pending
        pending.removeAll()
        for item in waiting { preserve(item.3) }
        finishMetadataWait(false)
        authoritativeSession = nil
        authoritativeServerID = nil
        trackRevisions = [:]
        revision = 0
        let failed = RemoteReply(
            requestID: UUID(), status: .rejected, revision: revision,
            message: "Connection changed. Unsent edits were not replayed.")
        waiting.forEach { $0.2?(failed) }
    }
    func disconnect() {
        connectionTask?.cancel()
        commandTask?.cancel()
        invalidateConnectionEdits()
        client.disconnect()
    }
    func send(
        _ command: RemoteCommand, expecting versions: [UUID: Int] = [:], gesture: RemoteGestureRevisionFence? = nil,
        expectedRevision: Int? = nil, onReply: ((RemoteReply) -> Void)? = nil
    ) {
        guard connected else {
            lastError = "Connect to the audio server before editing."
            store?.status = lastError!
            preserve(gesture)
            onReply?(.init(requestID: UUID(), status: .rejected, revision: revision, message: lastError!))
            return
        }
        guard pending.count < RemoteProtocol.maximumPendingRequests else {
            let message = "The server command queue is full. Wait for pending edits."
            store?.status = message
            preserve(gesture)
            onReply?(.init(requestID: UUID(), status: .rejected, revision: revision, message: message))
            return
        }
        if !Self.affectedTracks(command).isEmpty, projectSaveStatus == .saved { projectSaveStatus = .modified }
        if let key = Self.coalescingKey(command),
            let index = pending.lastIndex(where: { Self.coalescingKey($0.0) == key && $0.3?.id == gesture?.id })
        {
            pending[index].0 = command
            pending[index].2 = onReply
        } else {
            pending.append((command, versions, onReply, gesture, expectedRevision))
        }
        startCommandDrain()
    }
    private func startCommandDrain() {
        guard commandTask == nil else { return }
        commandTask = Task { [weak self] in
            guard let self else { return }
            defer {
                self.commandTask = nil
                if !self.pending.isEmpty { self.startCommandDrain() }
            }
            while !pending.isEmpty && !Task.isCancelled {
                let (command, capturedVersions, replyHandler, fence, expectedRevision) = pending.removeFirst()
                let targets = Set(Self.affectedTracks(command))
                guard fence?.isCurrent(targets: targets, observed: trackRevisions) != false else {
                    rejectGesture(fence)
                    replyHandler?(
                        .init(
                            requestID: UUID(), status: .conflict, revision: revision,
                            message: lastError ?? "Track changed."))
                    continue
                }
                var versions = capturedVersions
                if let fence {
                    for id in targets { versions[id] = fence.versions[id] }
                }
                let generation = connectionGeneration
                let navigation = projectNavigationGeneration
                if Self.requestTimeout(command) >= 600 {
                    activeWork = "Processing on audio server"
                    store?.workProgress = nil
                }
                var savedCandidate: Session?
                if case .saveProject(let name, let replace, let title) = command {
                    savedCandidate = authoritativeSession
                    if let title {
                        savedCandidate?.name = title
                    } else if replace != true {
                        savedCandidate?.name = (try? SessionName.projectTitle(name)) ?? name
                    }
                }
                inFlightGesture = fence
                inFlightCommand = command
                defer {
                    inFlightGesture = nil
                    inFlightCommand = nil
                    activeWork = nil
                }
                do {
                    let reply = try await request(
                        command, expecting: versions, expectedRevision: expectedRevision,
                        timeout: Self.requestTimeout(command))
                    guard generation == connectionGeneration, !Task.isCancelled else {
                        preserve(fence)
                        replyHandler?(
                            .init(
                                requestID: reply.requestID, status: .rejected, revision: revision,
                                message: "Connection changed. Review the unsent edit."))
                        continue
                    }
                    if let fence, reply.accepted {
                        let confirmed = await confirmMetadata(fence, command: command, reply: reply)
                        guard generation == connectionGeneration, !Task.isCancelled else {
                            preserve(fence)
                            replyHandler?(
                                .init(
                                    requestID: reply.requestID, status: .rejected, revision: revision,
                                    message: "Connection changed. Review the unsent edit."))
                            continue
                        }
                        if !confirmed {
                            rejectGesture(fence)
                            replyHandler?(
                                .init(
                                    requestID: reply.requestID, status: .conflict, revision: revision,
                                    message: lastError ?? "Track changed."))
                            continue
                        }
                    }
                    if !reply.accepted {
                        preserve(fence)
                        lastError = reply.message
                        store?.recordFailure(reply.message)
                        _ = try? await request(.snapshot)
                    } else {
                        if case .openProject(let id) = command {
                            let payload = reply.payload.flatMap {
                                try? JSONSerialization.jsonObject(with: $0) as? [String: String]
                            }
                            openedProjectDestination(
                                serverName: id, file: nil,
                                projectID: payload?["projectID"].flatMap(UUID.init(uuidString:)))
                        }
                        if case .editor(let intent) = command, intent.operation == "project.new" {
                            openedProjectDestination(serverName: nil, file: nil)
                        }
                        if let savedCandidate {
                            if navigation == projectNavigationGeneration, case .saveProject(let name, _, _) = command {
                                adoptProjectDestination(serverName: name, file: nil)
                            }
                            let matches =
                                authoritativeSession == savedCandidate && revision == reply.revision
                                && !pending.contains(where: { !Self.affectedTracks($0.0).isEmpty })
                            savedServerRevision = matches ? revision : nil
                            projectSaveStatus = matches ? .saved : .modified
                            pendingSavedDocument = matches ? nil : (savedCandidate, reply.revision)
                            store?.activity.record(
                                matches
                                    ? "Session saved on the audio server."
                                    : "A project copy was saved. The current session has newer changes.", kind: .success
                            )
                        }
                        if !reply.message.isEmpty { store?.status = reply.message }
                        if let fence, !pending.contains(where: { $0.3?.id == fence.id }) { fence.draft = nil }
                    }
                    replyHandler?(reply)
                } catch {
                    guard generation == connectionGeneration, !Task.isCancelled else {
                        preserve(fence)
                        replyHandler?(
                            .init(
                                requestID: UUID(), status: .rejected, revision: revision,
                                message: "Connection changed. Review the unsent edit."))
                        continue
                    }
                    preserve(fence)
                    lastError = error.localizedDescription
                    store?.recordFailure(error.localizedDescription)
                    let failed = RemoteReply(
                        requestID: UUID(), status: .rejected, revision: revision, message: error.localizedDescription)
                    let waiting = pending
                    pending.removeAll()
                    replyHandler?(failed)
                    waiting.forEach {
                        self.preserve($0.3)
                        $0.2?(failed)
                    }
                    break
                }
            }
        }
    }
    private func rejectGesture(_ fence: RemoteGestureRevisionFence?) {
        preserve(fence)
        lastError = "The track changed while you were editing. Review the unsent edit in Server connection."
        store?.recordFailure(lastError!)
        if let authoritativeSession {
            store?.session = authoritativeSession
            sanitizeSelection()
        }
    }
    private func metadataResolution(_ fence: RemoteGestureRevisionFence, command: RemoteCommand, reply: RemoteReply)
        -> RemoteGestureRevisionFence.Resolution
    {
        let targets = Set(Self.affectedTracks(command))
        return fence.resolve(
            reply: reply, targets: targets, observedRevision: revision,
            observed: trackRevisions, verifiedUnchanged: verifiedUnchangedTargets(command),
            removedTargets: {
                if case .editTracks(let edit) = command { return Set(edit.removedTrackIDs) }
                return []
            }())
    }
    private func confirmMetadata(_ fence: RemoteGestureRevisionFence, command: RemoteCommand, reply: RemoteReply) async
        -> Bool
    {
        switch metadataResolution(fence, command: command, reply: reply) {
        case .ready: return true
        case .conflict: return false
        case .waiting: break
        }
        return await withCheckedContinuation { continuation in
            metadataWait = MetadataWait(fence: fence, command: command, reply: reply, continuation: continuation)
            metadataTimeout = Task { [weak self] in
                do { try await Task.sleep(nanoseconds: 30_000_000_000) } catch { return }
                self?.finishMetadataWait(false)
            }
        }
    }
    private func finishMetadataWait(_ success: Bool) {
        guard let wait = metadataWait else { return }
        metadataWait = nil
        metadataTimeout?.cancel()
        metadataTimeout = nil
        wait.continuation.resume(returning: success)
    }
    private func resolveMetadataWait() {
        guard let wait = metadataWait else { return }
        switch metadataResolution(wait.fence, command: wait.command, reply: wait.reply) {
        case .waiting: break
        case .ready: finishMetadataWait(true)
        case .conflict: finishMetadataWait(false)
        }
    }
    private func verifiedUnchangedTargets(_ command: RemoteCommand) -> Set<UUID> {
        guard let document = authoritativeSession else { return [] }
        func track(_ id: UUID) -> Track? {
            id == Session.masterTrackID ? document.masterTrack : document.tracks.first { $0.id == id }
        }
        switch command {
        case .setTrackGain(let id, let value): return track(id)?.gain == value ? [id] : []
        case .setTrackPan(let id, let value): return track(id)?.pan == value ? [id] : []
        case .setTrackName(let id, let value): return track(id)?.name == value ? [id] : []
        case .setTrackMute(let id, let value): return track(id)?.muted == value ? [id] : []
        case .setTrackSolo(let id, let value): return track(id)?.solo == value ? [id] : []
        case .setTrackArm(let id, let value): return track(id)?.armed == value ? [id] : []
        case .updateTrackControls(let controls, _, _):
            return track(controls.trackID).map { RemoteTrackControls(track: $0) == controls ? [controls.trackID] : [] }
                ?? []
        case .editTracks(let patch):
            var result = Set(patch.tracks.filter { track($0.id) == $0 }.map(\.id))
            if let order = patch.trackOrder, document.tracks.map(\.id) == order {
                result.formUnion(Set(order).subtracting(patch.tracks.map(\.id)))
            }
            return result
        case .editor(let intent) where intent.operation == "effects.preview" && intent.flags["separateSource"] == true:
            return intent.context.trackID.map { [$0] } ?? []
        default: return []
        }
    }
    private static func affectedTracks(_ command: RemoteCommand) -> [UUID] {
        switch command {
        case .editTracks(let patch): return patch.tracks.map(\.id) + patch.removedTrackIDs + (patch.trackOrder ?? [])
        case .updateTrackControls(let controls, _, _): return [controls.trackID]
        case .setTrackGain(let id, _), .setTrackPan(let id, _), .setTrackName(let id, _), .setTrackMute(let id, _),
            .setTrackSolo(let id, _), .setTrackArm(let id, _), .setTrackInput(let id, _, _, _):
            return [id]
        case .editor(let intent) where ["designer.commit", "drums.commit"].contains(intent.operation):
            return intent.context.trackID.map { [$0] } ?? []
        case .editor(let intent) where intent.operation == "master.applyProposal": return [Session.masterTrackID]
        case .editor(let intent)
        where intent.flags["separateSource"] == true && ["effects.apply", "effects.preview"].contains(intent.operation):
            return intent.context.trackID.map { [$0] } ?? []
        case .editor(let intent) where ["pitch.analyze", "pitch.render", "pitch.apply"].contains(intent.operation):
            return intent.strings["trackID"].flatMap(UUID.init(uuidString:)).map { [$0] } ?? []
        default: return []
        }
    }
    /// True while an edit touching this track is queued or awaiting authored
    /// metadata; source-bound work must wait instead of fencing a stale version.
    func hasPendingEdits(for trackID: UUID) -> Bool {
        let edits = { (command: RemoteCommand) in
            Self.affectedTracks(command).contains(trackID) && !Self.isReadOnlySourceRequest(command)
        }
        return inFlightCommand.map(edits) == true || pending.contains { edits($0.0) }
    }
    /// While a drag streams edits, the server confirms each one after the next
    /// is already queued. Showing that older state would pull the control
    /// back for a moment (a jittery slider), so tracks with queued edits keep
    /// their local, newer version; the last edit's confirmation then matches.
    private func overlayingQueuedEdits(_ document: Session, local: Session) -> Session {
        let ahead = Set(pending.filter { !Self.isReadOnlySourceRequest($0.0) }.flatMap { Self.affectedTracks($0.0) })
        guard !ahead.isEmpty else { return document }
        var result = document
        for id in ahead {
            if id == Session.masterTrackID {
                result.masterTrack = local.masterTrack
            } else if let index = result.tracks.firstIndex(where: { $0.id == id }),
                let mine = local.tracks.first(where: { $0.id == id })
            {
                result.tracks[index] = mine
            }
        }
        return result
    }
    private static func isReadOnlySourceRequest(_ command: RemoteCommand) -> Bool {
        if case .editor(let intent) = command {
            return ["pitch.analyze", "pitch.render"].contains(intent.operation)
                || intent.operation == "effects.preview" && intent.flags["separateSource"] == true
        }
        return false
    }
    private static func coalescingKey(_ command: RemoteCommand) -> String? {
        switch command {
        case .setTrackGain(let id, _): return "\(id).gain"
        case .setTrackPan(let id, _): return "\(id).pan"
        case .updateTrackControls(_, _, let gesture): return gesture
        case .editTracks(let edit): return edit.gestureID
        default: return nil
        }
    }
    static func controlCommand(before: Track, after: Track, name: String, gesture: String?) -> RemoteCommand {
        var test = before
        test.gain = after.gain
        if test == after { return .setTrackGain(trackID: after.id, gain: after.gain) }
        test = before
        test.pan = after.pan
        if test == after { return .setTrackPan(trackID: after.id, pan: after.pan) }
        test = before
        test.name = after.name
        if test == after { return .setTrackName(trackID: after.id, name: after.name) }
        test = before
        test.muted = after.muted
        if test == after { return .setTrackMute(trackID: after.id, muted: after.muted) }
        test = before
        test.solo = after.solo
        if test == after { return .setTrackSolo(trackID: after.id, solo: after.solo) }
        test = before
        test.armed = after.armed
        if test == after { return .setTrackArm(trackID: after.id, armed: after.armed) }
        return .updateTrackControls(controls: .init(track: after), name: name, gestureID: gesture)
    }
    private static func requestTimeout(_ command: RemoteCommand) -> Double {
        if case .editor(let intent) = command, intent.flags["separateSource"] == true { return 3900 }
        switch command {
        case .openProject, .saveProject, .exportMix, .finishUpload: return 600
        default: break
        }
        if case .editor(let intent) = command,
            [
                "effects.apply", "designer.commit", "drums.commit", "drums.export", "clips.combine", "pitch.apply",
                "pitch.manualApply", "pitch.analyze", "pitch.render", "master.analyze", "project.export",
                "project.package", "server.shutdown", "drums.saveKit",
            ].contains(intent.operation)
        {
            return 600
        }
        return 30
    }
    func route(_ intent: RemoteEditorIntent, onReply: ((RemoteReply) -> Void)? = nil) {
        let intent =
            ["transport.stop", "transport.pause", "transport.finishRecording"].contains(intent.operation)
            ? RemoteEditorIntent(operation: intent.operation, context: .init(cursorFrame: intent.context.cursorFrame))
            : intent
        switch intent.operation {
        case "designer.stop":
            cancelPendingAuditions(["designer.preview", "designer.configure", "designer.noteOn", "designer.controller"])
        case "designer.allNotesOff": cancelPendingAuditions(["designer.noteOn", "designer.controller"])
        case "drums.stop": cancelPendingAuditions(["drums.preview", "drums.configure"])
        case "take.stop": cancelPendingAuditions(["take.preview", "take.configure"])
        default: break
        }
        if RemoteCommand.editor(intent).canInterruptServerJob {
            sendUrgent(.editor(intent), onReply: onReply)
            return
        }
        // Only the newest queued preview matters: replace, do not pile up.
        if ["designer.preview", "drums.preview"].contains(intent.operation),
            let index = pending.lastIndex(where: {
                if case .editor(let queued) = $0.0 { return queued.operation == intent.operation }
                return false
            })
        {
            pending[index].2?(
                .init(requestID: UUID(), status: .rejected, revision: revision, message: "Preview replaced."))
            pending[index] = (.editor(intent), [:], onReply, nil, nil)
            return
        }
        let sessionRevision =
            ["session.timing", "session.rename", "project.new"].contains(intent.operation) ? revision : nil
        send(
            .editor(intent),
            expecting: intent.context.trackID.flatMap { id in trackRevisions[id].map { [id: $0] } } ?? [:],
            expectedRevision: sessionRevision, onReply: onReply)
    }
    func commitWorkspace(
        _ intent: RemoteEditorIntent, source: RemoteWorkspaceSource, onReply: @escaping (RemoteReply) -> Void
    ) {
        func identifierMatches(_ key: String, _ id: UUID?) -> Bool {
            guard let text = intent.strings[key], !text.isEmpty else { return id == nil }
            return UUID(uuidString: text) == id
        }
        guard ["designer.commit", "drums.commit"].contains(intent.operation),
            identifierMatches("trackID", source.trackID), identifierMatches("clipID", source.clipID),
            source.ready(in: self), intent.context.trackID == source.trackID, intent.context.clipID == source.clipID,
            intent.context.cursorFrame == source.frame
        else {
            onReply(
                .init(
                    requestID: UUID(), status: .conflict, revision: revision,
                    message:
                        "The source changed. Your draft is retained; insert it on a new track or reopen its source."))
            return
        }
        send(.editor(intent), gesture: source.fence, expectedRevision: source.expectedSessionRevision, onReply: onReply)
    }
    @discardableResult
    func cancelQueuedSeparation(fence: RemoteGestureRevisionFence) -> Bool {
        func matches(_ entry: (RemoteCommand, [UUID: Int], ((RemoteReply) -> Void)?, RemoteGestureRevisionFence?, Int?))
            -> Bool
        {
            guard entry.3?.id == fence.id, case .editor(let intent) = entry.0 else { return false }
            return intent.flags["separateSource"] == true
                && ["effects.preview", "effects.apply"].contains(intent.operation)
        }
        let cancelled = pending.filter(matches)
        guard !cancelled.isEmpty else { return false }
        pending.removeAll(where: matches)
        let reply = RemoteReply(
            requestID: UUID(), status: .rejected, revision: revision,
            message: "Source separation cancelled before processing.")
        cancelled.forEach { $0.2?(reply) }
        return true
    }
    private func cancelPendingAuditions(_ operations: Set<String>) {
        func matches(_ command: RemoteCommand) -> Bool {
            if case .editor(let intent) = command { return operations.contains(intent.operation) }
            return false
        }
        let cancelled = pending.filter { matches($0.0) }
        pending.removeAll { matches($0.0) }
        let reply = RemoteReply(requestID: UUID(), status: .rejected, revision: revision, message: "Preview cancelled.")
        cancelled.forEach { $0.2?(reply) }
    }
    private func sendUrgent(_ command: RemoteCommand, onReply: ((RemoteReply) -> Void)? = nil) {
        guard connected, urgentRequests < 32 else {
            let message = "Live control is waiting for the server."
            store?.status = message
            onReply?(.init(requestID: UUID(), status: .rejected, revision: revision, message: message))
            return
        }
        urgentRequests += 1
        Task { [weak self] in
            guard let self else { return }
            defer { urgentRequests -= 1 }
            do {
                let reply = try await request(command)
                if !reply.accepted { store?.status = reply.message }
                onReply?(reply)
            } catch {
                store?.status = error.localizedDescription
                onReply?(
                    .init(requestID: UUID(), status: .rejected, revision: revision, message: error.localizedDescription)
                )
            }
        }
    }
    func attachMIDI(_ input: DesignerMIDIInput, pattern: SoundDesignPattern) {
        midiReceiver = input
        input.configureRemote(sources: midiSources.map { .init(id: $0.id, name: $0.name) }) { [weak self] source in
            self?.route(
                .init(
                    operation: "designer.midiConnect", integers: ["sourceID": Int(source)],
                    payload: try? JSONEncoder().encode(pattern)))
        }
        route(.init(operation: "designer.midiSources")) { [weak self] reply in
            guard let self, reply.accepted, let payload = reply.payload,
                let sources = try? JSONDecoder().decode([RemoteMIDISource].self, from: payload)
            else { return }
            midiSources = sources
            midiReceiver?.updateRemoteSources(sources.map { .init(id: $0.id, name: $0.name) })
        }
    }
    func detachMIDI() { midiReceiver = nil }
    func commit(before: Session, after: Session, name: String, gestureID: String?) {
        guard !applyingServerState, before != after else { return }
        let old = Dictionary(uniqueKeysWithValues: before.tracks.map { ($0.id, $0) })
        var changed = after.tracks.filter { old[$0.id] != $0 }
        if before.masterTrack != after.masterTrack { changed.append(after.masterTrack) }
        let removed = before.tracks.filter { oldTrack in !after.tracks.contains { $0.id == oldTrack.id } }.map(\.id)
        let oldOrder = before.tracks.map(\.id)
        let newOrder = after.tracks.map(\.id)
        let targets = Set(changed.map(\.id) + removed + (oldOrder != newOrder ? oldOrder : []))
        let fence: RemoteGestureRevisionFence
        if let gestureID, let activeGesture, activeGesture.key == gestureID {
            fence = activeGesture.fence
        } else {
            fence = RemoteGestureRevisionFence(versions: trackRevisions)
        }
        let patch = RemoteTrackEdit(
            tracks: changed, removedTrackIDs: removed, trackOrder: oldOrder != newOrder ? newOrder : nil, name: name,
            gestureID: gestureID == nil ? nil : fence.id.uuidString)
        fence.draft = patch
        // An existing target without an opening version cannot be turned into
        // an unfenced edit. New tracks alone legitimately have no version yet.
        guard targets.allSatisfy({ id in (id != Session.masterTrackID && old[id] == nil) || fence.versions[id] != nil })
        else {
            rejectGesture(fence)
            return
        }
        let versions = Dictionary(
            uniqueKeysWithValues: targets.compactMap { id in fence.versions[id].map { (id, $0) } })
        if changed.count == 1, let track = changed.first,
            let original = track.id == Session.masterTrackID ? before.masterTrack : old[track.id],
            removed.isEmpty, oldOrder == newOrder, original.clips == track.clips,
            original.recordingTakes == track.recordingTakes, original.activeTakeID == track.activeTakeID
        {
            send(
                Self.controlCommand(before: original, after: track, name: name, gesture: patch.gestureID),
                expecting: versions, gesture: fence)
            return
        }
        send(.editTracks(patch), expecting: versions, gesture: fence)
    }
    func waveform(_ id: UUID, range: Range<Int>? = nil, pixels: Int = 2048) -> WaveformEnvelope? {
        guard let frames = store?.assets[id]?.count, frames > 0 else { return nil }
        let wanted = range ?? 0..<frames
        let lower = max(0, min(frames - 1, wanted.lowerBound))
        let upper = min(frames, max(lower + 1, wanted.upperBound))
        let visible = lower..<upper
        let envelope = waveformCache.envelope(id: id, range: visible)
        if connected {
            let overview = RemoteWaveformRequest.overview(id: id, frames: frames, points: waveformCache.overviewPoints)
            if !waveformCache.contains(overview) { waveformQueue.enqueue(overview) }
            if let detail = RemoteWaveformRequest.refinement(
                id: id, frames: frames, range: visible, pixels: pixels, envelope: envelope),
                !waveformCache.contains(detail)
            {
                waveformQueue.enqueue(detail)
            }
            drainWaveforms()
        }
        return envelope
    }
    private func drainWaveforms() {
        // Two utility requests let the current view refine while an overview is
        // measured. The queue is coalesced; transport uses its separate lane.
        while connected, mediaRequests < 2, let request = waveformQueue.next() {
            mediaRequests += 1
            let generation = waveformGeneration
            Task { [weak self] in
                guard let self else { return }
                defer {
                    mediaRequests -= 1
                    if generation == waveformGeneration { waveformQueue.complete(request) }
                    drainWaveforms()
                }
                do {
                    var reply: RemoteReply?
                    for attempt in 0..<6 {
                        guard connected, generation == waveformGeneration else { return }
                        let result = try await requestFile(
                            .waveform(
                                assetID: request.id, start: request.start, end: request.end, points: request.points),
                            timeout: 600)
                        reply = result
                        if result.status != .busy { break }
                        try await Task.sleep(nanoseconds: UInt64(min(1000, 100 << attempt)) * 1_000_000)
                    }
                    guard let reply else { return }
                    if reply.status == .busy, generation == waveformGeneration {
                        // Revisit only still-visible requests after the bounded
                        // backoff; another peer's long scan must not leave a
                        // stationary client permanently blank.
                        store?.waveformDisplay.invalidate()
                    }
                    if reply.accepted, let payload = reply.payload {
                        let asset = await Task.detached(priority: .utility) {
                            try? JSONDecoder().decode(RemoteAsset.self, from: payload)
                        }.value
                        if generation == waveformGeneration, let asset, install(asset) {
                            store?.waveformDisplay.invalidate()
                        }
                    }
                } catch { /* Reconnection preserves measurements; obsolete views never accumulate. */  }
            }
        }
    }
    @discardableResult private func install(_ asset: RemoteAsset) -> Bool {
        guard asset.frames >= 0, (1...64).contains(asset.channels) else { return false }
        if store?.assets[asset.id]?.count != asset.frames || store?.assets[asset.id]?.channelCount != asset.channels {
            store?.assets[asset.id] = AudioSamples(
                displayFrames: asset.frames, channels: asset.channels, identity: asset.id)
        }
        guard let envelope = WaveformEnvelope(asset: asset) else { return false }
        return waveformCache.install(id: asset.id, frames: asset.frames, envelope: envelope)
    }
    func receive(_ event: RemoteEvent) {
        guard let store else { return }
        applyingServerState = true
        defer {
            applyingServerState = false
            resolveMetadataWait()
            switch event {
            case .snapshot, .delta: confirmPendingSaveMetadata()
            default: break
            }
        }
        switch event {
        case .snapshot(let snapshot):
            if let serverID = authoritativeServerID {
                if serverID != snapshot.serverID {
                    invalidateConnectionEdits()
                } else if snapshot.revision < revision {
                    return
                }
            }
            authoritativeServerID = snapshot.serverID
            let incomingProject = snapshot.projectName.map { $0.hasSuffix(".soundroom") ? $0 : $0 + ".soundroom" }
            if projectServerID != nil
                && (projectServerID != snapshot.serverID || serverProjectName != incomingProject
                    || (currentProjectID != nil && currentProjectID != snapshot.projectID))
            {
                projectFileURL = nil
                store.projectURL = nil
                savedServerRevision = nil
                projectSaveStatus = .unverified
                pendingSavedDocument = nil
                projectNavigationGeneration &+= 1
            }
            projectServerID = snapshot.serverID
            serverProjectName = incomingProject
            currentProjectID = snapshot.projectID
            if let savedServerRevision, savedServerRevision != snapshot.revision { projectSaveStatus = .modified }
            serverName = snapshot.serverName
            revision = snapshot.revision
            trackRevisions = snapshot.trackRevisions
            peers = snapshot.peers
            authoritativeSession = snapshot.session
            store.session = overlayingQueuedEdits(snapshot.session, local: store.session)
            // Asset IDs are immutable. Reconnects must not blank already drawn
            // waveforms; retain only IDs still present in the new document.
            waveformCache.retainAssets(Set(snapshot.assets.map(\.id)))
            waveformCache.configure(assetCount: snapshot.assets.count)
            // Install the inventory as one publication. Publishing an empty
            // dictionary and then every individual source made a large open
            // notify the entire editor thousands of times on the UI actor.
            var inventory: [UUID: AudioSamples] = [:]
            for asset in snapshot.assets where asset.frames >= 0 && (1...64).contains(asset.channels) {
                inventory[asset.id] = AudioSamples(
                    displayFrames: asset.frames, channels: asset.channels, identity: asset.id)
            }
            store.assets = inventory
            waveformGeneration &+= 1
            waveformQueue.removeAll()
            snapshot.assets.forEach { install($0) }
            installDevices(snapshot.devices, input: snapshot.inputDeviceID, output: snapshot.outputDeviceID)
            installTransport(snapshot.transport)
            store.status = snapshot.status
            sanitizeSelection()
            // The server starts with the click off; only an enabled click needs asserting.
            if preferencesSentGeneration != connectionGeneration {
                preferencesSentGeneration = connectionGeneration
                if store.metronome { store.syncMetronome() }
            }
        case .delta(let delta):
            guard let current = authoritativeSession, delta.revision >= revision,
                delta.hasValidControlPatches(in: current)
            else { return }
            revision = delta.revision
            trackRevisions.merge(delta.trackRevisions) { _, new in new }
            if let savedServerRevision, savedServerRevision != revision { projectSaveStatus = .modified }
            for id in delta.removedTrackIDs { trackRevisions.removeValue(forKey: id) }
            var document = current
            document.tracks.removeAll { delta.removedTrackIDs.contains($0.id) }
            for track in delta.tracks {
                if track.id == Session.masterTrackID {
                    document.masterTrack = track
                } else if let index = document.tracks.firstIndex(where: { $0.id == track.id }) {
                    document.tracks[index] = track
                } else {
                    document.tracks.append(track)
                }
            }
            for controls in delta.controls {
                if controls.trackID == Session.masterTrackID {
                    controls.apply(to: &document.masterTrack)
                } else if let index = document.tracks.firstIndex(where: { $0.id == controls.trackID }) {
                    controls.apply(to: &document.tracks[index])
                }
            }
            for patch in delta.controlPatches ?? [] {
                if patch.trackID == Session.masterTrackID {
                    patch.apply(to: &document.masterTrack)
                } else if let index = document.tracks.firstIndex(where: { $0.id == patch.trackID }) {
                    patch.apply(to: &document.tracks[index])
                }
            }
            if let order = delta.trackOrder {
                let positions = Dictionary(uniqueKeysWithValues: order.enumerated().map { ($1, $0) })
                document.tracks.sort { (positions[$0.id] ?? Int.max) < (positions[$1.id] ?? Int.max) }
            }
            if let timing = delta.timing { document.timing = timing }
            if let name = delta.name { document.name = name }
            if delta.metadataUpdated { document.masteringAnalysis = delta.masteringAnalysis }
            document.version = delta.sessionVersion ?? document.requiredProjectVersion
            authoritativeSession = document
            store.session = overlayingQueuedEdits(document, local: store.session)
            if !delta.assets.isEmpty {
                waveformCache.configure(assetCount: Set(store.assets.keys).union(delta.assets.map(\.id)).count)
            }
            delta.assets.forEach { install($0) }
            if let status = delta.status { store.status = status }
            // Fader/pan/effect-control deltas cannot remove regions. Walking
            // every clip and republishing selectedClips on each one made a
            // tiny mixer update scale with the size of the arrangement.
            if !delta.tracks.isEmpty || !delta.removedTrackIDs.isEmpty { sanitizeSelection() }
        case .transport(let transport): installTransport(transport)
        case .meters(let left, let right, let input):
            store.meter = max(left, right)
            if store.isRecording || store.inputMonitoring {
                store.inputActivity.update([input], recording: store.isRecording)
            }
        case .devices(let devices, let input, let output): installDevices(devices, input: input, output: output)
        case .presence(let clients): peers = clients
        case .status(let status): store.status = status
        case .waveform(let asset): if install(asset) { store.waveformDisplay.invalidate() }
        case .job(_, let progress, let message):
            if store.workProgress != progress { store.workProgress = progress }
            if store.status != message { store.status = message }
        case .history(let undo, let redo, let undoLabel, let redoLabel):
            canUndo = undo
            canRedo = redo
            undoName = undoLabel.map { "Undo " + $0 } ?? "Undo"
            redoName = redoLabel.map { "Redo " + $0 } ?? "Redo"
            store.objectWillChange.send()
        case .effectMeters(let snapshots): effectMeters = snapshots
        case .audition(let kind, let owner, let playing, let preparing, let position):
            let owned = owner == client.clientID
            if kind == "designer" {
                designerServerBeat = max(0, position)
                designerReceived = ProcessInfo.processInfo.systemUptime
                if store.designerPreviewing != (owned && playing) { store.designerPreviewing = owned && playing }
            } else if kind == "drums" {
                store.drumDesigner.installRemoteAudition(
                    playing: owned && playing, preparing: owned && preparing, elapsed: position)
            }
        case .midiSources(let sources):
            midiSources = sources
            midiReceiver?.updateRemoteSources(sources.map { .init(id: $0.id, name: $0.name) })
        case .midiNote(let pitch, let velocity, let on): midiReceiver?.onNote?(pitch, velocity, on)
        case .midiController(let kind, let value): midiReceiver?.onController?(kind, value)
        case .midiAllNotesOff: midiReceiver?.onAllNotesOff?()
        case .analysis: break
        case .projects(let names): projects = names
        case .automationWrite(let update): store.status = update.reason
        }
    }
    private func installTransport(_ transport: RemoteTransport) {
        guard let store else { return }
        if transportPreparing != transport.isPreparing {
            transportPreparing = transport.isPreparing
            store.objectWillChange.send()
        }
        let wasActive = store.isPlaying || store.isRecording
        if transport.isRecording && !store.isRecording { store.inputActivity.reset() }
        if store.isPlaying != transport.isPlaying { store.isPlaying = transport.isPlaying }
        if store.isRecording != transport.isRecording { store.isRecording = transport.isRecording }
        if let start = transport.recordingStartFrame { recordingStartFrame = start }
        if transport.isPlaying || transport.isRecording || wasActive { store.clientDisplayTransport(transport.frame) }
        let loopRange = transport.loopStart.flatMap { start in
            transport.loopEnd.flatMap { end in end > start ? start..<end : nil }
        }
        if store.loopRange != loopRange { store.loopRange = loopRange }
        if store.loopSelection != (loopRange != nil) { store.loopSelection = loopRange != nil }
        if store.busy != transport.isPreparing { store.busy = transport.isPreparing }
    }
    private func installDevices(_ devices: [RemoteDevice], input: UInt32, output: UInt32) {
        store?.devices = devices.map {
            AudioDevice(
                id: $0.id, name: $0.name, uid: $0.uid, inputChannels: $0.inputChannels,
                outputChannels: $0.outputChannels)
        }
        store?.inputDevice = input
        store?.outputDevice = output
    }
    private func sanitizeSelection() {
        guard let store else { return }
        let clips = Set(store.session.tracks.flatMap(\.clips).map(\.id))
        store.selectedClips.formIntersection(clips)
        if let id = store.selectedClip, !clips.contains(id) { store.selectedClip = nil }
        if store.selectedTrack != Session.masterTrackID
            && !store.session.tracks.contains(where: { $0.id == store.selectedTrack })
        {
            store.selectedTrack = store.session.tracks.first?.id
            store.selection = nil
        }
    }
}

extension SessionStore {
    var remoteContext: RemoteEditorContext {
        .init(
            trackID: selectedTrack, clipID: selectedClip, clipIDs: Array(selectedClips), cursorFrame: playhead,
            rangeStart: selectedRange?.lowerBound, rangeEnd: selectedRange?.upperBound,
            effectID: selectedRealtimeEffect, automationTarget: automationTarget.rawValue)
    }
    @discardableResult func remoteRoute(
        _ operation: String, integers: [String: Int] = [:], numbers: [String: Double] = [:],
        strings: [String: String] = [:], flags: [String: Bool] = [:], payload: Data? = nil
    ) -> Bool {
        guard let remoteGUI else {
            status = "Connecting to the audio server…"
            return true
        }
        if operation == "designer.stop" {
            designerPreviewing = false
            remoteGUI.designerPreviewStart = nil
        }
        let capturedContext = remoteContext
        remoteGUI.route(
            .init(
                operation: operation, context: capturedContext, integers: integers, numbers: numbers, strings: strings,
                flags: flags, payload: payload)
        ) { [weak self] reply in
            if operation == "designer.preview", reply.accepted, let self {
                // Publishing an unchanged value re-renders the open designer and
                // cancels an in-progress note or slider drag.
                if !designerPreviewing { designerPreviewing = true }
                if remoteGUI.designerPreviewStart == nil {
                    remoteGUI.designerPreviewStart = ProcessInfo.processInfo.systemUptime
                }
                if let payload, let pattern = try? JSONDecoder().decode(SoundDesignPattern.self, from: payload) {
                    remoteGUI.designerBPM = pattern.bpm
                }
            }
            guard reply.accepted, let payload = reply.payload,
                let result = try? JSONDecoder().decode(RemoteEditorContext.self, from: payload), let self
            else { return }
            if ["transport.toggle", "transport.pause", "transport.stop"].contains(operation), !isPlaying, !isRecording {
                playhead = result.cursorFrame
            }
            // Stop replies contain the authoritative cursor, not editor targets.
            if ["transport.stop", "transport.pause", "transport.finishRecording"].contains(operation) { return }
            let current = remoteContext
            guard current.trackID == capturedContext.trackID, current.clipID == capturedContext.clipID,
                Set(current.clipIDs) == Set(capturedContext.clipIDs), current.rangeStart == capturedContext.rangeStart,
                current.rangeEnd == capturedContext.rangeEnd
            else { return }
            selectedTrack = result.trackID
            selectedClip = result.clipID
            selectedClips = Set(result.clipIDs)
            if let start = result.rangeStart, let end = result.rangeEnd, let track = result.trackID, end > start {
                selection = EditSelection(trackID: track, range: start..<end)
            } else {
                selection = nil
            }
            if let effectID = result.effectID { selectedRealtimeEffect = effectID }
        }
        return true
    }
    func remoteCommitMetadata(before: Session, after: Session, name: String, gestureID: String? = nil) {
        remoteGUI?.commit(before: before, after: after, name: name, gestureID: gestureID)
    }
    func waveformEnvelope(_ id: UUID, range: Range<Int>? = nil, pixels: Int = 2048) -> WaveformEnvelope? {
        if let remoteGUI { return remoteGUI.waveform(id, range: range, pixels: pixels) }
        return assets[id].flatMap { WaveformCache.shared.cachedEnvelope(id: id, audio: $0) }
    }
}
