import Foundation
import SoundroomCore

public enum RemoteProtocol {
    public static let version = 2
    /// Pairing files remain usable when the connection protocol changes.
    public static let credentialVersion = 1
    public static let defaultPort: UInt16 = 49152
    public static let maximumFrameBytes = 8 * 1024 * 1024
    public static let maximumQueuedBytes = 32 * 1024 * 1024
    public static let maximumClients = 16
    public static let maximumPendingRequests = 64
    /// Native clients leave these slots available when ordinary requests stall.
    public static let reservedPendingInterruptRequests = 8
    public static let maximumUploadChunkBytes = 256 * 1024
}

public enum RemoteRole: String, Codable, Sendable { case viewer, editor, `operator` }

public struct RemotePeer: Codable, Sendable, Equatable, Identifiable {
    public var id: UUID
    public var name: String
    public var role: RemoteRole
    public init(id: UUID, name: String, role: RemoteRole = .operator) {
        self.id = id
        self.name = name
        self.role = role
    }
}

public struct RemoteDevice: Codable, Sendable, Equatable, Identifiable {
    public var id: UInt32
    public var uid: String
    public var name: String
    public var inputChannels: Int
    public var outputChannels: Int
    public init(id: UInt32, uid: String, name: String, inputChannels: Int, outputChannels: Int) {
        self.id = id
        self.uid = uid
        self.name = name
        self.inputChannels = inputChannels
        self.outputChannels = outputChannels
    }
}

public struct RemoteTransport: Codable, Sendable, Equatable {
    public var frame: Int
    public var isPlaying: Bool
    public var isRecording: Bool
    public var isPreparing: Bool
    public var loopStart: Int?
    public var loopEnd: Int?
    public var hostTime: Double
    public var operatorID: UUID?
    public var recordingStartFrame: Int?
    public init(
        frame: Int = 0, isPlaying: Bool = false, isRecording: Bool = false, isPreparing: Bool = false,
        loopStart: Int? = nil, loopEnd: Int? = nil, hostTime: Double = ProcessInfo.processInfo.systemUptime,
        operatorID: UUID? = nil, recordingStartFrame: Int? = nil
    ) {
        self.frame = frame
        self.isPlaying = isPlaying
        self.isRecording = isRecording
        self.isPreparing = isPreparing
        self.loopStart = loopStart
        self.loopEnd = loopEnd
        self.hostTime = hostTime
        self.operatorID = operatorID
        self.recordingStartFrame = recordingStartFrame
    }
}

/// Display data only. Remote clients never receive PCM or open Core Audio devices.
public struct RemoteAsset: Codable, Sendable, Equatable, Identifiable {
    public var id: UUID
    public var frames: Int
    public var channels: Int
    public var peaks: [Float]
    /// Per-channel min/max pairs (L then R). Optional for older hosts; `peaks`
    /// remains the combined envelope for clients that draw a single waveform.
    public var channelPeaks: [[Float]]?
    public var waveformStartFrame: Int
    public var framesPerBin: Int?
    public init(
        id: UUID, frames: Int, channels: Int, peaks: [Float] = [], waveformStartFrame: Int = 0,
        framesPerBin: Int? = nil, channelPeaks: [[Float]]? = nil
    ) {
        self.id = id
        self.frames = frames
        self.channels = channels
        self.peaks = peaks
        self.waveformStartFrame = waveformStartFrame
        self.framesPerBin = framesPerBin
        self.channelPeaks = channelPeaks
    }
}

public struct RemoteSnapshot: Codable, Sendable, Equatable {
    public var serverID: UUID
    public var serverName: String
    public var session: Session
    public var revision: Int
    public var trackRevisions: [UUID: Int]
    public var transport: RemoteTransport
    public var devices: [RemoteDevice]
    public var inputDeviceID: UInt32
    public var outputDeviceID: UInt32
    public var assets: [RemoteAsset]
    public var status: String
    public var projectName: String?
    public var projectID: UUID?
    public var peers: [RemotePeer]
    public init(
        serverID: UUID, serverName: String, session: Session, revision: Int = 0, trackRevisions: [UUID: Int] = [:],
        transport: RemoteTransport = .init(), devices: [RemoteDevice] = [], inputDeviceID: UInt32 = 0,
        outputDeviceID: UInt32 = 0, assets: [RemoteAsset] = [], status: String = "", projectName: String? = nil,
        projectID: UUID? = nil, peers: [RemotePeer] = []
    ) {
        self.serverID = serverID
        self.serverName = serverName
        self.session = session
        self.revision = revision
        self.trackRevisions = trackRevisions
        self.transport = transport
        self.devices = devices
        self.inputDeviceID = inputDeviceID
        self.outputDeviceID = outputDeviceID
        self.assets = assets
        self.status = status
        self.projectName = projectName
        self.projectID = projectID
        self.peers = peers
    }
}

/// Track deltas carry only changed entities; the master uses Session.masterTrackID.
public struct RemotePitchPoint: Codable, Sendable, Equatable {
    public var frame: Int
    public var frequency: Float
    public var confidence: Float
    public init(frame: Int, frequency: Float, confidence: Float) {
        self.frame = frame
        self.frequency = frequency
        self.confidence = confidence
    }
}
public struct RemotePitchAnalysis: Codable, Sendable, Equatable {
    public var points: [RemotePitchPoint]
    public var regions: [PitchEditRegion]
    public var isOutput: Bool
    public var targetsAreDraft: Bool
    public var error: String?
    public init(
        points: [RemotePitchPoint] = [], regions: [PitchEditRegion] = [], isOutput: Bool = false,
        targetsAreDraft: Bool = false, error: String? = nil
    ) {
        self.points = points
        self.regions = regions
        self.isOutput = isOutput
        self.targetsAreDraft = targetsAreDraft
        self.error = error
    }
}

/// Lightweight mixer/rack changes leave clip/note arrays on the client untouched.
public struct RemoteTrackControls: Codable, Sendable, Equatable {
    public var trackID: UUID
    public var name: String
    public var color: Int
    public var gain: Float
    public var pan: Float
    public var muted: Bool
    public var solo: Bool
    public var armed: Bool
    public var inputDeviceUID: String?
    public var inputDeviceName: String?
    public var inputChannel: Int
    public var stereoInput: Bool
    public var volumeAutomationScale: AutomationValueScale?
    public var mixAutomation: [String: [AutomationPoint]]
    public var routing: TrackRouting?
    public var realtimeEffects: [RealtimeEffect]
    public init(track: Track) {
        routing = track.routing
        trackID = track.id
        name = track.name
        color = track.color
        gain = track.gain
        pan = track.pan
        muted = track.muted
        solo = track.solo
        armed = track.armed
        inputDeviceUID = track.inputDeviceUID
        inputDeviceName = track.inputDeviceName
        inputChannel = track.inputChannel
        stereoInput = track.stereoInput
        volumeAutomationScale = track.volumeAutomationScale
        mixAutomation = track.mixAutomation
        realtimeEffects = track.realtimeEffects
    }
    public func apply(to track: inout Track) {
        guard track.id == trackID else { return }
        track.routing = routing
        track.name = name
        track.color = color
        track.gain = gain
        track.pan = pan
        track.muted = muted
        track.solo = solo
        track.armed = armed
        track.inputDeviceUID = inputDeviceUID
        track.inputDeviceName = inputDeviceName
        track.inputChannel = inputChannel
        track.stereoInput = stereoInput
        track.volumeAutomationScale = volumeAutomationScale
        track.mixAutomation = mixAutomation
        track.realtimeEffects = realtimeEffects
    }
}

/// Negotiated scalar edits leave even dense effect/automation arrays untouched.
/// Servers emit these only after the client declares compact-control-patches.
public struct RemoteTrackControlPatch: Codable, Sendable, Equatable {
    public var trackID: UUID
    public var gain: Float?
    public var pan: Float?
    public var muted: Bool?
    public var solo: Bool?
    public var armed: Bool?
    public init(
        trackID: UUID, gain: Float? = nil, pan: Float? = nil, muted: Bool? = nil, solo: Bool? = nil, armed: Bool? = nil
    ) {
        self.trackID = trackID
        self.gain = gain
        self.pan = pan
        self.muted = muted
        self.solo = solo
        self.armed = armed
    }
    public func apply(to track: inout Track) {
        guard track.id == trackID else { return }
        if let gain { track.gain = gain }
        if let pan { track.pan = pan }
        if let muted { track.muted = muted }
        if let solo { track.solo = solo }
        if let armed { track.armed = armed }
    }
    public func hasValidValues() -> Bool {
        (gain != nil || pan != nil || muted != nil || solo != nil || armed != nil)
            && (gain == nil || gain!.isFinite && (0...4).contains(gain!))
            && (pan == nil || pan!.isFinite && (-1...1).contains(pan!))
    }
}

public struct RemoteSessionDelta: Codable, Sendable, Equatable {
    public var revision: Int
    public var tracks: [Track]
    public var controls: [RemoteTrackControls]
    public var controlPatches: [RemoteTrackControlPatch]?
    public var removedTrackIDs: [UUID]
    public var trackOrder: [UUID]?
    public var trackRevisions: [UUID: Int]
    public var name: String?
    public var masteringAnalysis: MasteringAnalysis?
    public var metadataUpdated: Bool
    public var timing: MusicalTiming?
    public var sessionVersion: Int?
    public var assets: [RemoteAsset]
    public var status: String?
    public init(
        revision: Int, tracks: [Track] = [], controls: [RemoteTrackControls] = [],
        controlPatches: [RemoteTrackControlPatch]? = nil, removedTrackIDs: [UUID] = [], trackOrder: [UUID]? = nil,
        trackRevisions: [UUID: Int] = [:], name: String? = nil, masteringAnalysis: MasteringAnalysis? = nil,
        metadataUpdated: Bool = false, sessionVersion: Int? = nil, timing: MusicalTiming? = nil,
        assets: [RemoteAsset] = [], status: String? = nil
    ) {
        self.revision = revision
        self.tracks = tracks
        self.controls = controls
        self.controlPatches = controlPatches
        self.removedTrackIDs = removedTrackIDs
        self.trackOrder = trackOrder
        self.trackRevisions = trackRevisions
        self.name = name
        self.masteringAnalysis = masteringAnalysis
        self.metadataUpdated = metadataUpdated
        self.sessionVersion = sessionVersion
        self.timing = timing
        self.assets = assets
        self.status = status
    }
    /// Preflight the entire sparse set before changing any mirrored metadata.
    public func hasValidControlPatches(in session: Session) -> Bool {
        guard let controlPatches else { return true }
        guard controlPatches.count <= 1024 else { return false }
        let removed = Set(removedTrackIDs)
        let available = Set(session.tracks.map(\.id) + tracks.map(\.id) + [Session.masterTrackID])
        var seen = Set<UUID>()
        return controlPatches.allSatisfy { patch in
            patch.hasValidValues() && seen.insert(patch.trackID).inserted
                && available.contains(patch.trackID) && !removed.contains(patch.trackID)
        }
    }
}

public struct RemoteEditorContext: Codable, Sendable, Equatable {
    public var trackID: UUID?
    public var clipID: UUID?
    public var clipIDs: [UUID]
    public var cursorFrame: Int
    public var rangeStart: Int?
    public var rangeEnd: Int?
    public var effectID: UUID?
    public var automationTarget: String?
    public init(
        trackID: UUID? = nil, clipID: UUID? = nil, clipIDs: [UUID] = [], cursorFrame: Int = 0,
        rangeStart: Int? = nil, rangeEnd: Int? = nil, effectID: UUID? = nil, automationTarget: String? = nil
    ) {
        self.trackID = trackID
        self.clipID = clipID
        self.clipIDs = clipIDs
        self.cursorFrame = cursorFrame
        self.rangeStart = rangeStart
        self.rangeEnd = rangeEnd
        self.effectID = effectID
        self.automationTarget = automationTarget
    }
}

/// Only operations explicitly whitelisted by the authoritative command service may execute.
/// This is data, never an Objective-C selector, shell command or server filesystem path.
public struct RemoteEditorIntent: Codable, Sendable, Equatable {
    public var operation: String
    public var context: RemoteEditorContext
    public var integers: [String: Int]
    public var numbers: [String: Double]
    public var strings: [String: String]
    public var flags: [String: Bool]
    public var payload: Data?
    public init(
        operation: String, context: RemoteEditorContext = .init(), integers: [String: Int] = [:],
        numbers: [String: Double] = [:], strings: [String: String] = [:], flags: [String: Bool] = [:],
        payload: Data? = nil
    ) {
        self.operation = operation
        self.context = context
        self.integers = integers
        self.numbers = numbers
        self.strings = strings
        self.flags = flags
        self.payload = payload
    }
}

public struct RemoteTrackEdit: Codable, Sendable, Equatable {
    public var tracks: [Track]
    public var removedTrackIDs: [UUID]
    public var trackOrder: [UUID]?
    public var name: String
    public var gestureID: String?
    public init(
        tracks: [Track] = [], removedTrackIDs: [UUID] = [], trackOrder: [UUID]? = nil, name: String,
        gestureID: String? = nil
    ) {
        self.tracks = tracks
        self.removedTrackIDs = removedTrackIDs
        self.trackOrder = trackOrder
        self.name = name
        self.gestureID = gestureID
    }
}

public enum RemoteCommand: Codable, Sendable, Equatable {
    case snapshot
    case play(trackOnly: UUID?)
    case stop
    case seek(frame: Int)
    case record
    case takeTransport
    case setLoop(start: Int?, end: Int?)
    case updateTrackControls(controls: RemoteTrackControls, name: String, gestureID: String?)
    case setTrackName(trackID: UUID, name: String)
    case setTrackGain(trackID: UUID, gain: Float)
    case setTrackPan(trackID: UUID, pan: Float)
    case setTrackMute(trackID: UUID, muted: Bool)
    case setTrackSolo(trackID: UUID, solo: Bool)
    case setTrackArm(trackID: UUID, armed: Bool)
    case setDevices(inputID: UInt32?, outputID: UInt32?)
    case setTrackInput(trackID: UUID, deviceUID: String?, channel: Int, stereo: Bool)
    case addTrack(name: String)
    case removeTrack(trackID: UUID)
    case splitClip(trackID: UUID, clipID: UUID, frame: Int)
    case deleteClips(trackID: UUID, clipIDs: [UUID])
    case moveClip(trackID: UUID, clipID: UUID, start: Int)
    case replaceTracks(tracks: [Track], name: String?, gestureID: String?)
    case editTracks(RemoteTrackEdit)
    case editor(RemoteEditorIntent)
    case undo
    case redo
    case listProjects
    case openProject(id: String)
    case saveProject(name: String, replaceExisting: Bool? = nil, title: String? = nil)
    case exportMix(name: String)
    case beginUpload(id: UUID, name: String, bytes: Int, sha256: String)
    case uploadChunk(id: UUID, offset: Int, data: Data)
    case finishUpload(id: UUID, trackID: UUID?, frame: Int)
    case downloadArtifact(id: UUID, offset: Int, bytes: Int)
    case readArtifact(id: String, offset: Int, bytes: Int)
    case waveform(assetID: UUID, start: Int, end: Int, points: Int)
    case ping(clientTime: Double)
}

public struct RemoteArtifact: Codable, Sendable, Equatable, Identifiable {
    public var id: UUID
    public var name: String
    public var bytes: Int
    public var sha256: String
    public init(id: UUID, name: String, bytes: Int, sha256: String) {
        self.id = id
        self.name = name
        self.bytes = bytes
        self.sha256 = sha256
    }
}
public struct RemoteArtifactChunk: Codable, Sendable, Equatable {
    public var offset: Int
    public var data: Data
    public var complete: Bool
    public init(offset: Int, data: Data, complete: Bool) {
        self.offset = offset
        self.data = data
        self.complete = complete
    }
}
public struct RemoteMIDISource: Codable, Sendable, Equatable, Identifiable {
    public var id: UInt32
    public var name: String
    public init(id: UInt32, name: String) {
        self.id = id
        self.name = name
    }
}

public struct RemoteRequest: Codable, Sendable, Equatable, Identifiable {
    public var id: UUID
    public var clientID: UUID
    public var expectedRevision: Int?
    public var expectedTrackRevisions: [UUID: Int]
    public var command: RemoteCommand
    public init(
        id: UUID = UUID(), clientID: UUID, expectedRevision: Int? = nil,
        expectedTrackRevisions: [UUID: Int] = [:], command: RemoteCommand
    ) {
        self.id = id
        self.clientID = clientID
        self.expectedRevision = expectedRevision
        self.expectedTrackRevisions = expectedTrackRevisions
        self.command = command
    }
}

public enum RemoteReplyStatus: String, Codable, Sendable { case accepted, rejected, conflict, busy }
public struct RemoteReply: Codable, Sendable, Equatable {
    public var requestID: UUID
    public var status: RemoteReplyStatus
    public var revision: Int
    public var message: String
    public var payload: Data?
    public var accepted: Bool { status == .accepted }
    public init(
        requestID: UUID, status: RemoteReplyStatus = .accepted, revision: Int, message: String = "",
        payload: Data? = nil
    ) {
        self.requestID = requestID
        self.status = status
        self.revision = revision
        self.message = message
        self.payload = payload
    }
}

public enum RemoteEvent: Codable, Sendable, Equatable {
    case snapshot(RemoteSnapshot)
    case delta(RemoteSessionDelta)
    case transport(RemoteTransport)
    case meters(left: Float, right: Float, input: Float)
    case effectMeters([UUID: EffectMeterSnapshot])
    case audition(kind: String, ownerID: UUID?, playing: Bool, preparing: Bool, position: Double)
    case devices([RemoteDevice], inputID: UInt32, outputID: UInt32)
    case presence([RemotePeer])
    case midiSources([RemoteMIDISource])
    case midiNote(pitch: Int, velocity: Int, on: Bool)
    case midiController(kind: SoundDesignControllerKind, value: Double)
    case midiAllNotesOff
    case history(canUndo: Bool, canRedo: Bool, undoName: String?, redoName: String?)
    case status(String)
    case waveform(RemoteAsset)
    case analysis(RemotePitchAnalysis)
    case projects([String])
    case job(id: UUID, progress: Double, message: String)
    case automationWrite(update: RemoteAutomationWriteLeaseUpdate)
}

public enum RemoteConnectionState: Equatable, Sendable {
    case disconnected
    case connecting
    case connected(serverID: UUID, name: String, role: RemoteRole)
    case reconnecting(attempt: Int)
    case failed(String)
}

public enum RemoteError: Error, LocalizedError, Equatable {
    case invalidCredential, invalidFrame, oversizedFrame, queueFull, connectionLost, timeout
    case rejected(String)
    case protocolMismatch
    public var errorDescription: String? {
        switch self {
        case .invalidCredential: return "The pairing credential is invalid or insecure."
        case .invalidFrame: return "The server sent an invalid control frame."
        case .oversizedFrame: return "The control message exceeds the 8 MiB limit."
        case .queueFull: return "The client is too slow; reconnect for a fresh snapshot."
        case .connectionLost: return "Connection lost. Reconcile the request ID before retrying an edit."
        case .timeout: return "The server did not respond before the timeout."
        case .rejected(let message): return message
        case .protocolMismatch: return "This client and server use incompatible protocol versions."
        }
    }
}

public struct RemoteChallenge: Codable, Sendable {
    public var version = RemoteProtocol.version
    public var serverID: UUID
    public var nonce: Data
}
public struct RemoteHello: Codable, Sendable {
    public var version = RemoteProtocol.version
    public var serverID: UUID
    public var peer: RemotePeer
    public var credentialID: UUID
    public var proof: String
}
public struct RemoteWelcome: Codable, Sendable {
    public var version = RemoteProtocol.version
    public var serverID: UUID
    public var name: String
    public var role: RemoteRole
}
public enum RemoteWireMessage: Codable, Sendable {
    case challenge(RemoteChallenge)
    case hello(RemoteHello)
    case welcome(RemoteWelcome)
    case request(RemoteRequest)
    case reply(RemoteReply)
    case event(RemoteEvent)
    case failure(String)
    case heartbeat(UUID)
    case heartbeatReply(UUID)
}
