import Foundation
import SoundroomCore
import SoundroomRemote

/// Track-control changes retain the shared clip arrays. Only changed clip content
/// rebuilds terminal display records; transport and presence do not project clips.
struct TUIProjectionCache {
    private struct Entry {
        var sourceClips: [Clip]
        var clips: [TUIClip]
        var ids: Set<UUID>
    }
    private var entries: [UUID: Entry] = [:]
    private(set) var rebuiltClips = 0
    mutating func project(_ state: RemoteSnapshot) -> TUISession {
        let live = Set(state.session.tracks.map(\.id))
        for id in entries.keys where !live.contains(id) { entries.removeValue(forKey: id) }
        var tracks: [TUITrack] = []; tracks.reserveCapacity(state.session.tracks.count)
        for source in state.session.tracks {
            let clips: [TUIClip]
            if let cached = entries[source.id], cached.sourceClips == source.clips { clips = cached.clips }
            else {
                clips = source.clips.map { .init(id: $0.id, name: $0.name, start: $0.start, length: $0.length) }
                entries[source.id] = Entry(sourceClips: source.clips, clips: clips, ids: Set(source.clips.map(\.id)))
                rebuiltClips += clips.count
            }
            tracks.append(TUITrack(id: source.id, name: source.name, clips: clips, muted: source.muted,
                solo: source.solo, armed: source.armed, gain: Double(source.gain), pan: Double(source.pan),
                effects: source.realtimeEffects.map { $0.kind.rawValue }, color: source.color, inserts: source.realtimeEffects,
                input: "\(source.inputDeviceName ?? state.devices.first(where: { $0.uid == source.inputDeviceUID })?.name ?? source.inputDeviceUID ?? "Default input") · ch \(source.inputChannel + 1)\(source.stereoInput ? "–\(source.inputChannel + 2)" : "")", clipIDSet: entries[source.id]?.ids))
        }
        return TUISession(name: state.session.name, tracks: tracks,
            revision: UInt64(max(0, state.revision)), frame: state.transport.frame,
            playing: state.transport.isPlaying, recording: state.transport.isRecording,
            timing: state.session.timing,
            devices: state.devices.map { .init(id: $0.id, name: $0.name, inputs: $0.inputChannels, outputs: $0.outputChannels) },
            outputID: state.outputDeviceID,
            loopStart: state.transport.loopStart, loopEnd: state.transport.loopEnd,
            operatorName: state.peers.first(where: { $0.id == state.transport.operatorID })?.name
                ?? (state.transport.operatorID == nil ? nil : "another client"))
    }
}

extension TUIIntent {
    /// Separate bounded transport queue: a long file job cannot trap Space/Stop
    /// behind that client's own pending edits. The server still authorizes it.
    var isUrgentTransport: Bool {
        switch self {
        case .togglePlayback, .play, .stop, .record, .startRecording, .seek: return true
        case .command(let line): return ["takeover", "cancel", "play-track", "loop"].contains(line.split(whereSeparator: \.isWhitespace).first?.lowercased() ?? "")
        default: return false
        }
    }
}
