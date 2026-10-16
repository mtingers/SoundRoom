import Foundation
import SoundroomCore

/// Negotiated host facts. Device/backend/codec support is supplied by the host,
/// never inferred from client OS or from a successfully compiled binary.
public struct RemoteCapabilities: Codable, Equatable, Sendable {
    public var protocolVersions: [Int] = [2, 3]
    public var projectVersions: [Int] = Array(1...11)
    public var commands: [String] = RemoteV3Descriptor.commands.keys.sorted()
    public var editorOperations: [String] = RemoteAutomationWriteGrammar.defaultOperations
    public var sampleRate: Double
    public var deviceBackends: [String]
    public var codecs: [String]
    public var snapshotPagination: Bool
    public var maxFrameBytes = RemoteProtocol.maximumFrameBytes
    public var maxUploadChunkBytes = RemoteProtocol.maximumUploadChunkBytes
    public var maxUploadBytes = 2_147_483_648
    public var maxTimelineFrame: Int
    public var features: [String]
    public var effects: [RemoteEffectMetadata]
    public var selectionEffects: [RemoteSelectionEffectMetadata]
    public var instruments: [RemoteInstrumentMetadata]
    public var designerPresets: [RemoteDesignerPresetMetadata]
    public var drumKits: [RemoteDrumKitMetadata]
    public var drumPresets: [RemoteDrumPresetMetadata]
    public init(sampleRate: Double = 48000, deviceBackends: [String] = [], codecs: [String] = [], snapshotPagination: Bool = false, features: [String] = []) {
        self.sampleRate = sampleRate; self.deviceBackends = deviceBackends; self.codecs = codecs; self.snapshotPagination = snapshotPagination
        maxTimelineFrame = sampleRate.isFinite && sampleRate > 0 && sampleRate <= 768000 ? Int(sampleRate * 24 * 60 * 60) : 0
        self.features = features
        effects = RealtimeEffectKind.allCases.map(RemoteEffectMetadata.init)
        selectionEffects = BuiltInEffect.allCases.map(RemoteSelectionEffectMetadata.init)
        instruments = MusicalInstrument.allCases.map(RemoteInstrumentMetadata.init)
        designerPresets = SoundDesignPreset.allCases.map(RemoteDesignerPresetMetadata.init) + OrganicPatch.allCases.map(RemoteDesignerPresetMetadata.init)
        drumKits = DrumKit.allCases.map(RemoteDrumKitMetadata.init)
        drumPresets = DrumPatternPreset.allCases.map(RemoteDrumPresetMetadata.init)
    }
    public func supports(_ command: RemoteCommand) -> Bool {
        guard commands.contains(command.capabilityName) else { return false }
        if case .editor(let intent) = command { return editorOperations.contains(intent.operation) }
        return true
    }
    /// Declare new event grammar only to a host that explicitly advertises it.
    /// Adding fields to Hello would break older strict v3 servers before pairing.
    public func clientFeatureDeclaration() -> RemoteCommand? {
        guard features.contains("compact-control-patches"), commands.contains("editor"),
              editorOperations.contains("client.features") else { return nil }
        let payload = Data(#"["compact-control-patches"]"#.utf8)
        return .editor(.init(operation: "client.features", payload: payload))
    }
}

public extension RemoteCommand {
    /// Exhaustive native tags, without encoding a large edit on the UI actor.
    var capabilityName: String {
        switch self {
        case .snapshot: return "snapshot"
        case .play: return "play"
        case .stop: return "stop"
        case .seek: return "seek"
        case .record: return "record"
        case .takeTransport: return "takeTransport"
        case .setLoop: return "setLoop"
        case .updateTrackControls: return "updateTrackControls"
        case .setTrackName: return "setTrackName"
        case .setTrackGain: return "setTrackGain"
        case .setTrackPan: return "setTrackPan"
        case .setTrackMute: return "setTrackMute"
        case .setTrackSolo: return "setTrackSolo"
        case .setTrackArm: return "setTrackArm"
        case .setDevices: return "setDevices"
        case .setTrackInput: return "setTrackInput"
        case .addTrack: return "addTrack"
        case .removeTrack: return "removeTrack"
        case .splitClip: return "splitClip"
        case .deleteClips: return "deleteClips"
        case .moveClip: return "moveClip"
        case .replaceTracks: return "replaceTracks"
        case .editTracks: return "editTracks"
        case .editor: return "editor"
        case .undo: return "undo"
        case .redo: return "redo"
        case .listProjects: return "listProjects"
        case .openProject: return "openProject"
        case .saveProject: return "saveProject"
        case .exportMix: return "exportMix"
        case .beginUpload: return "beginUpload"
        case .uploadChunk: return "uploadChunk"
        case .finishUpload: return "finishUpload"
        case .downloadArtifact: return "downloadArtifact"
        case .readArtifact: return "readArtifact"
        case .waveform: return "waveform"
        case .ping: return "ping"
        }
    }
}
public struct RemoteEffectMetadata: Codable, Equatable, Sendable {
    public var id: String
    public var name: String
    public var parameters: [RemoteParameterMetadata]
    public var factoryPresets: [RemoteEffectPresetMetadata]
    public var modes = ["realtime", "applyToAudio"]
    public init(_ kind: RealtimeEffectKind) {
        id = kind.rawValue; name = kind.name; parameters = kind.parameters.map(RemoteParameterMetadata.init)
        factoryPresets = kind.presets.map { RemoteEffectPresetMetadata(name: $0.name, values: $0.values) }
    }
}
public struct RemoteParameterMetadata: Codable, Equatable, Sendable {
    public var id: String
    public var name: String
    public var minimum: Float
    public var maximum: Float
    public var defaultValue: Float
    public var unit: String
    public var logarithmic: Bool
    public var integer: Bool
    public var choices: [String]?
    public var automationUnit = "normalized"
    public init(_ parameter: EffectParameter) {
        id = parameter.id; name = parameter.name; minimum = parameter.minimum; maximum = parameter.maximum
        defaultValue = parameter.defaultValue; unit = parameter.unit; logarithmic = parameter.logarithmic
        integer = parameter.integer; choices = parameter.choices
    }
}

public struct RemoteEffectPresetMetadata: Codable, Equatable, Sendable {
    public var name: String
    public var values: [String: Float]
    public init(name: String, values: [String: Float]) { self.name = name; self.values = values }
}
public struct RemoteSelectionEffectMetadata: Codable, Equatable, Sendable {
    public var id: String
    public var name: String
    public var realtime: Bool
    public init(_ effect: BuiltInEffect) { id = effect.rawValue; name = effect.name; realtime = RealtimeEffectKind(rawValue: effect.rawValue) != nil }
}
public struct RemoteInstrumentMetadata: Codable, Equatable, Sendable {
    public var id: String
    public var name: String
    public var characterLabel: String
    public init(_ instrument: MusicalInstrument) { id = instrument.rawValue; name = instrument.title; characterLabel = instrument.characterLabel }
}
public struct RemoteDesignerPresetMetadata: Codable, Equatable, Sendable {
    public var id: String
    public var name: String
    public var category: String
    public var sound: SoundDesignPattern
    public init(_ preset: SoundDesignPreset) {
        id = "factory." + preset.rawValue; name = preset.rawValue; category = "instrument"
        sound = DesignerSoundPreset(pattern: preset.applying(to: SoundDesignPattern())).sound
    }
    public init(_ patch: OrganicPatch) {
        id = "organic." + patch.rawValue; name = patch.rawValue; category = "organic"
        sound = DesignerSoundPreset(pattern: patch.applying(to: SoundDesignPattern())).sound
    }
}
public struct RemoteDrumKitMetadata: Codable, Equatable, Sendable {
    public var id: String
    public var name: String
    public var voices: [String]
    public var articulations: [String: [String]]
    public init(_ kit: DrumKit) {
        id = kit.rawValue; name = kit.title; voices = DrumVoice.allCases.map(\.rawValue)
        articulations = Dictionary(uniqueKeysWithValues: DrumVoice.allCases.map { ($0.rawValue, DrumArticulation.available(for: $0).map(\.rawValue)) })
    }
}
public struct RemoteDrumPresetMetadata: Codable, Equatable, Sendable {
    public var id: String
    public var name: String
    public var pattern: DrumPattern
    public init(_ preset: DrumPatternPreset) { id = preset.rawValue; name = preset.rawValue; pattern = DrumPattern(); pattern.apply(preset) }
}
