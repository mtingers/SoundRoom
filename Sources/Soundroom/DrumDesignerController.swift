import AppKit
import SoundroomCore
import SoundroomRemote

struct DrumDesignRequest: Identifiable {
    let id: UUID
    let clipID: UUID?
    let trackID: UUID?
    let frame: Int
    let pattern: DrumPattern
    let source: RemoteWorkspaceSource?
    init(id: UUID = UUID(), clipID: UUID?, trackID: UUID?, frame: Int, pattern: DrumPattern, source: RemoteWorkspaceSource? = nil) {
        self.id = id; self.clipID = clipID; self.trackID = trackID; self.frame = frame; self.pattern = pattern; self.source = source
    }
}
@MainActor final class DrumDesignerController: ObservableObject {
    weak var owner: SessionStore?
    private(set) var isLoopingPreview = false
    @Published var previewing = false
    @Published var preparing = false
    @Published var message = ""
    @Published var working = false
    var elapsed: Double { remoteElapsed + (previewing ? max(0, ProcessInfo.processInfo.systemUptime - remoteReceived) : 0) }
    private var remoteElapsed = 0.0
    private var remoteReceived = ProcessInfo.processInfo.systemUptime
    func installRemoteAudition(playing: Bool, preparing: Bool, elapsed: Double) {
        remoteElapsed = max(0, elapsed); remoteReceived = ProcessInfo.processInfo.systemUptime
        if previewing != playing { previewing = playing }; if self.preparing != preparing { self.preparing = preparing }
        if !playing && !preparing { isLoopingPreview = false }
    }
    private var generation = 0
    private var draftVersion = 0
    func draftChanged() { draftVersion &+= 1 }
    init(owner: SessionStore) { self.owner = owner }
    func stop() {
        _ = owner?.remoteRoute("drums.stop")
        generation += 1; isLoopingPreview = false; previewing = false; preparing = false; working = false
    }
    func preview(_ pattern: DrumPattern, audition: DrumVoice? = nil, velocity: Double = 0.9) {
        guard let owner else { return }

        guard let remote = owner.remoteGUI else { message = "Connect to the audio server before previewing."; previewing = false; preparing = false; return }
        generation += 1; let token = generation
        let payload = try? JSONEncoder().encode(pattern)
        let continuing = previewing && isLoopingPreview && audition == nil
        isLoopingPreview = audition == nil
        if !continuing { previewing = false }; preparing = true; message = ""
        remote.route(.init(operation: "drums.preview", context: owner.remoteContext, numbers: audition == nil ? [:] : ["velocity": min(1, max(0, velocity))], strings: audition.map { ["audition": $0.rawValue] } ?? [:], payload: payload)) { [weak self] reply in
            guard let self, token == generation else { return }
            preparing = false; previewing = reply.accepted
            if !reply.accepted { message = reply.message }
        }
        return
    }
    func commit(_ pattern: DrumPattern, request: DrumDesignRequest) {
        guard let owner else { return }

        guard !working, !pattern.steps.isEmpty, owner.drumDesignRequest?.id == request.id else { return }
        guard let remote = owner.remoteGUI, let source = request.source else { message = "Connect and reopen the source before inserting. Your draft is retained."; return }
        guard let payload = try? JSONEncoder().encode(pattern) else { message = "The loop could not be encoded. Your draft is retained."; return }
        generation += 1; let token = generation, submittedVersion = draftVersion
        working = true; message = "Creating drum loop on the server…"
        var strings: [String: String] = [:]
        if let id = request.clipID { strings["clipID"] = id.uuidString }; if let id = request.trackID { strings["trackID"] = id.uuidString }
        remote.commitWorkspace(.init(operation: "drums.commit", context: source.context, integers: ["start": request.frame], strings: strings, payload: payload), source: source) { [weak self, weak owner, weak remote] reply in
            guard let self, let owner, token == generation, owner.drumDesignRequest?.id == request.id else { return }
            working = false
            guard reply.accepted, let remote else { message = reply.message; return }
            previewing = false; preparing = false
            if submittedVersion == draftVersion { owner.drumDesignRequest = nil; message = "Drum loop inserted"; return }
            if let data = reply.payload, let result = try? JSONDecoder().decode(RemoteEditorContext.self, from: data),
               let trackID = result.trackID, remote.trackRevisions[trackID] == reply.revision {
                let next = RemoteWorkspaceSource(remote: remote, trackID: trackID, clipID: result.clipID, frame: result.cursorFrame)
                owner.drumDesignRequest = .init(id: request.id, clipID: result.clipID, trackID: trackID, frame: result.cursorFrame, pattern: pattern, source: next)
            }
            message = "The earlier loop was inserted. Your newer private draft remains open."
        }
        return
    }

    func insertOnNewTrack(_ pattern: DrumPattern, request: DrumDesignRequest) {
        guard !working, let owner, owner.drumDesignRequest?.id == request.id, let remote = owner.remoteGUI else { return }
        let source = RemoteWorkspaceSource(remote: remote, trackID: nil, clipID: nil, frame: request.frame)
        let detached = DrumDesignRequest(id: request.id, clipID: nil, trackID: nil, frame: request.frame, pattern: pattern, source: source)
        owner.drumDesignRequest = detached
        commit(pattern, request: detached)
    }
    func close() {
        stop(); owner?.drumDesignRequest = nil; owner?.focusEditor()
    }
}
extension SessionStore {
    func requestDrumDesigner(editClip: Bool = false) {
        guard !busy, !isRecording else { return }
        if inlineSound?.unsettled == true { status = "Finish or reload the current sound draft before opening a designer."; return }
        if let inlineDrums, inlineDrums.unsettled, inlineDrums.failure == nil {
            if editClip, currentClip?.id == inlineDrums.clipID { inlineDrums.openDesigner() }
            else { status = "Finish the current beat update before opening another designer." }
            return
        }
        endContinuousEdit()
        let clip = editClip ? currentClip : nil
        var pattern = clip?.editableDrumPattern ?? DrumPattern()
        if clip?.editableDrumPattern == nil {
            pattern.apply(.backbeat); pattern.kit = .jazzMaple
            pattern.variationCycles = 4; pattern.velocityHumanize = 0.035; pattern.humanize = 0.003
        }
        drumDesigner.message = ""
        let trackID = selectedTrack == Session.masterTrackID || currentTrack?.acceptsClips == false ? nil : selectedTrack
        let source = remoteGUI.map { RemoteWorkspaceSource(remote: $0, trackID: trackID, clipID: clip?.id, frame: playhead) }
        drumDesignRequest = DrumDesignRequest(clipID: clip?.id, trackID: trackID, frame: playhead, pattern: pattern, source: source)
    }
}
