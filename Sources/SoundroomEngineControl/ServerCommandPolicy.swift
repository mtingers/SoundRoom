import Foundation
import SoundroomCore
import SoundroomRemote

/// Pure host admission/history/context policy. This library owns no editor,
/// dialogs, focus or audio device; execution still performs domain authorization.
public extension RemoteCommand {
    var isServerReadOnly: Bool {
        switch self { case .snapshot, .ping, .waveform, .listProjects, .downloadArtifact, .readArtifact: return true; case .editor(let intent): return ["meters.subscribe", "designer.midiSources", "master.proposal", "client.features"].contains(intent.operation); default: return false }
    }
    var canInterruptServerJob: Bool {
        switch self { case .stop: return true; case .editor(let intent): return ["transport.stop", "transport.pause", "transport.finishRecording", "jobs.cancel", "designer.noteOn", "designer.noteOff", "designer.allNotesOff", "designer.controller", "designer.configure", "designer.stop", "drums.stop", "take.stop", "audition.stop"].contains(intent.operation); default: return false }
    }
    var recordsServerHistory: Bool {
        switch self {
        case .updateTrackControls, .setTrackName, .setTrackGain, .setTrackPan, .setTrackMute, .setTrackSolo, .setTrackArm, .setTrackInput, .addTrack, .removeTrack, .splitClip, .deleteClips, .moveClip, .replaceTracks, .editTracks, .finishUpload: return true
        case .editor(let intent): return ["session.timing", "session.rename", "effects.apply", "designer.commit", "drums.commit", "clips.combine", "pitch.apply", "pitch.manualApply", "pitch.targets", "master.analyze", "master.applyProposal"].contains(intent.operation)
        default: return false
        }
    }
    var requiresPostRenderPreflight: Bool {
        if case .finishUpload = self { return true }
        if case .editor = self { return recordsServerHistory }
        return false
    }
    var isProjectOperation: Bool {
        switch self { case .openProject, .saveProject, .exportMix: return true; default: return false }
    }
    var isHeavyServerOperation: Bool {
        guard case .editor(let intent) = self else { return false }
        return ["effects.apply", "pitch.analyze", "pitch.render", "pitch.apply", "pitch.manualApply", "pitch.targets", "master.analyze", "master.applyProposal", "clips.combine", "designer.commit", "drums.commit", "drums.export", "project.export", "project.package", "project.new"].contains(intent.operation)
    }
    var serverTrackEdit: RemoteTrackEdit? {
        switch self { case .editTracks(let edit): return edit; case .replaceTracks(let tracks, let name, let gesture): return .init(tracks: tracks, name: name ?? "Edit", gestureID: gesture); default: return nil }
    }
    var changesServerCursor: Bool {
        switch self {
        case .play, .stop, .seek, .record, .takeTransport, .setLoop: return true
        case .editor(let intent): return intent.operation.hasPrefix("transport.")
        default: return false
        }
    }
    var serverGesture: String? { switch self { case .setTrackGain(let id, _): return "gain.\(id)"; case .setTrackPan(let id, _): return "pan.\(id)"; case .editTracks(let edit): return edit.gestureID; case .replaceTracks(_, _, let id): return id; case .updateTrackControls(_, _, let id): return id; default: return nil } }
    var serverTitle: String {
        switch self { case .editTracks(let edit): return edit.name; case .replaceTracks(_, let name, _): return name ?? "Edit"; case .updateTrackControls(_, let name, _): return name; case .setTrackName: return "Rename track"; case .setTrackGain: return "Track volume"; case .setTrackPan: return "Track pan"; case .setTrackMute: return "Mute track"; case .setTrackSolo: return "Solo track"; case .setTrackArm: return "Arm track"; case .editor(let intent): return intent.operation == "session.timing" ? "Tempo and meter" : intent.operation.replacingOccurrences(of: ".", with: " "); default: return "Edit" }
    }
}

