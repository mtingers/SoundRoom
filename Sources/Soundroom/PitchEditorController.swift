import Combine
import Foundation
import SoundroomCore
import SoundroomRemote

/// One editor per document. The measured contour is ephemeral; target curves
/// belong to the live insert and therefore participate in project save and Undo.
@MainActor
final class PitchEditorController: ObservableObject {
    private weak var owner: SessionStore?
    @Published private(set) var before: [PitchContourPoint] = []
    @Published private(set) var after: [PitchContourPoint]?
    @Published private(set) var visibleRegions: [PitchEditRegion] = []
    @Published private(set) var scope = 0..<0
    @Published private(set) var reference: Float = 440
    @Published private(set) var error: String?
    @Published private(set) var targetsAreDraft = false
    /// A successful audio commit ends the expanded editing session. Failure,
    /// cancellation and Undo must not dismiss an editor the user is reviewing.
    @Published private(set) var completedBakeID: UUID?
    private var trackID: UUID?
    private(set) var effectID: UUID?
    private var inputTrack: Track?
    private var sourceIdentities: [UUID: UUID] = [:]
    private var lastEffect: RealtimeEffect?
    private var generation = UUID()
    private var editKey: String?
    private var editName: String?
    /// The exact remote source a request or review is bound to: target binding,
    /// authored track version and connection. Equal numbers on another
    /// connection, or a newer foreign version, never confirm a result.
    struct RemoteSource: Equatable {
        let trackID: UUID
        let effectID: UUID?
        let scope: Range<Int>
        let version: Int
        let epoch: Int
    }
    /// Counts local target changes; a reply never replaces newer private targets.
    private var regionEdits = 0
    /// Bumped whenever the expanded sheet is presented, so a late Apply from an
    /// earlier presentation cannot dismiss a reopened editor.
    private var presentation = UUID()
    @Published private(set) var sourceNotice: String?

    init(owner: SessionStore) { self.owner = owner }

    var parameters: [String: Float] {
        activeEffect?.parametersForEditing ?? owner?.effectParameters
            ?? RealtimeEffectKind.pitchCorrection.defaultParameters
    }
    var activeEffect: RealtimeEffect? {
        owner?.session.tracks.first { $0.id == trackID }?.realtimeEffects.first { $0.id == effectID }
    }
    var bakePlan: PitchAudioCommit.Plan? {
        guard let owner, let trackID, let effectID else { return nil }
        return PitchAudioCommit.plan(session: owner.session, trackID: trackID, effectID: effectID)
    }
    var context: String {
        if targetsAreDraft { return "Showing the sung pitch · pick a style or drag a note to start tuning" }
        if let plan = bakePlan {
            let mode =
                activeEffect?.pitchEditsOnly == true
                ? "Tuning plays live" : "Tuning plays live · automatic outside your edits"
            return "\(mode) · \(plan.label)"
        }
        return "Analyze, then pick a style or drag a note to start tuning"
    }

    /// Changes to audio or preceding inserts invalidate measured input. Changes to
    /// this insert invalidate only measured output, keeping a stable editing view.
    func synchronize(track: Track?, effectID requested: UUID?, range: Range<Int>?, assets: [UUID: AudioSamples]) {
        guard let track else {
            clear()
            return
        }
        let selected = requested.flatMap { id in
            track.realtimeEffects.first { $0.id == id && $0.kind == .pitchCorrection }
        }
        let index = selected.flatMap { effect in track.realtimeEffects.firstIndex { $0.id == effect.id } }
        let nextRange = range ?? (0..<(track.clips.map(\.end).max() ?? 0))
        let nextInput = Self.bus(track, effects: index.map { Array(track.realtimeEffects.prefix($0)) } ?? [])
        let identities = Dictionary(
            uniqueKeysWithValues: track.referencedAssetIDs.compactMap { id in assets[id].map { (id, $0.cacheIdentity) }
            })
        let changedInput =
            trackID != track.id || inputTrack != nextInput || scope != nextRange || sourceIdentities != identities
        let changedInsert = self.effectID != selected?.id
        if changedInput || changedInsert {
            generation = UUID()
            if owner?.isManualPitchPreview == true { owner?.stopTransport() }
            before = []
            after = nil
            error = nil
            visibleRegions = PitchEditRegions.slicing(selected?.pitchEdits ?? [], to: nextRange)
            targetsAreDraft = false
        } else if lastEffect != selected {
            after = nil
            if lastEffect?.pitchEdits != selected?.pitchEdits {
                if targetsAreDraft, !visibleRegions.isEmpty {
                    // A foreign edit cannot silently replace unconfirmed private targets.
                    sourceNotice =
                        "Targets changed on the server. Your private draft is kept; move a point to replace them or Analyze to start over."
                } else {
                    visibleRegions = PitchEditRegions.slicing(selected?.pitchEdits ?? [], to: nextRange)
                    targetsAreDraft = false
                }
            }
        }
        trackID = track.id
        self.effectID = selected?.id
        scope = nextRange
        inputTrack = nextInput
        sourceIdentities = identities
        lastEffect = selected
        let raw = parameters["reference"] ?? 440
        reference = raw.isFinite && (420...460).contains(raw) ? raw : 440
    }
    private func clear() {
        generation = UUID()
        trackID = nil
        effectID = nil
        inputTrack = nil
        lastEffect = nil
        before = []
        after = nil
        visibleRegions = []
        scope = 0..<0
        sourceIdentities = [:]
        error = nil
        targetsAreDraft = false
        sourceNotice = nil
    }
    func presented() { presentation = UUID() }
    func remoteSource() -> RemoteSource? {
        guard let remote = owner?.remoteGUI, remote.connected, let trackID, let version = remote.trackRevisions[trackID]
        else { return nil }
        return .init(
            trackID: trackID, effectID: effectID, scope: scope, version: version, epoch: remote.connectionGeneration)
    }
    func isCurrent(_ source: RemoteSource) -> Bool { remoteSource() == source }
    static func bus(_ track: Track, effects: [RealtimeEffect]) -> Track {
        var result = track
        result.muted = false
        result.solo = false
        result.gain = 1
        result.pan = 0
        result.mixAutomation = [:]
        result.realtimeEffects = effects
        result.routing = nil
        return result
    }
    static func inputDocument(_ original: Session, track: Track) -> Session {
        var result = (try? original.isolating(trackID: track.id)) ?? original
        var track = track
        track.routing = result.tracks.first?.routing
        result.tracks[0] = track
        return result
    }
    func beginEdit(_ name: String) {
        guard let owner, !owner.busy, !owner.isRecording else { return }
        if owner.isManualPitchPreview { owner.stopTransport() }
        error = nil
        after = nil
        editKey = "pitch.curve.\(trackID?.uuidString ?? "")"
        editName = name
        owner.beginContinuousEdit(editKey!)
    }
    func endEdit() {
        owner?.endContinuousEdit()
        editKey = nil
        editName = nil
    }

    func setRegions(_ value: [PitchEditRegion], name: String = "Edit pitch targets") {
        guard let owner, let trackID, !owner.busy, !owner.isRecording, !scope.isEmpty else { return }
        let clean = PitchEditRegions.slicing(value, to: scope)
        guard clean != visibleRegions || (!clean.isEmpty && (activeEffect == nil || targetsAreDraft)) else { return }
        if owner.isManualPitchPreview { owner.stopTransport() }
        let key = editKey
        var chosen = activeEffect
        if chosen == nil {
            var effect = RealtimeEffect(kind: .pitchCorrection, parameters: parameters)
            effect.pitchEditsOnly = true
            chosen = effect
        }
        guard var effect = chosen else { return }
        var retained: [PitchEditRegion] = []
        for region in effect.pitchEdits {
            if region.endFrame <= scope.lowerBound || region.startFrame >= scope.upperBound {
                retained.append(region)
            } else {
                if region.startFrame < scope.lowerBound,
                    let part = region.sliced(to: region.startFrame..<scope.lowerBound)
                {
                    retained.append(part)
                }
                if region.endFrame > scope.upperBound,
                    let part = region.sliced(
                        to: scope.upperBound..<region.endFrame,
                        id: region.startFrame < scope.lowerBound ? UUID() : region.id)
                {
                    retained.append(part)
                }
            }
        }
        effect.pitchEdits = PitchEditRegions.sanitized(retained + clean)
        regionEdits += 1
        sourceNotice = nil
        let id = effect.id
        owner.updateTrack(trackID, actionName: editName ?? name, coalescingKey: key) { track in
            if let index = track.realtimeEffects.firstIndex(where: { $0.id == id }) {
                track.realtimeEffects[index] = effect
            } else {
                track.realtimeEffects.insert(effect, at: 0)
            }
        }
        // Adopt the newly created insert without throwing away its raw analysis.
        effectID = id
        lastEffect = effect
        visibleRegions = clean
        after = nil
        targetsAreDraft = false
        owner.selectedRealtimeEffect = id
    }
    func resetTargets(_ preset: PitchCurvePreset) {
        guard let owner, !before.isEmpty, !owner.busy, !owner.isRecording else { return }
        owner.stopTransport()
        endEdit()
        generation = UUID()
        error = nil
        let token = generation
        let measured = before
        let range = scope
        let pitchReference = reference
        let mask = PitchPointTargets.allowedPitchClasses(parameters: parameters)
        let original = owner.session
        let assets = owner.assets
        owner.busy = true
        owner.cancelWork = false
        owner.status = "Preparing pitch targets…"
        owner.workTask = Task {
            defer {
                owner.busy = false
                owner.workTask = nil
            }
            let worker = Task.detached(priority: .userInitiated) {
                try Task.checkCancellation()
                let targets = PitchPointTargets.seed(
                    contour: measured, scope: range, reference: pitchReference,
                    preset: preset, allowedPitchClasses: mask)
                try Task.checkCancellation()
                return targets
            }
            do {
                let targets = try await withTaskCancellationHandler(
                    operation: { try await worker.value }, onCancel: { worker.cancel() })
                guard token == generation, !Task.isCancelled, !owner.cancelWork,
                    owner.session == original, identitiesMatch(assets)
                else {
                    owner.status = "Pitch preset discarded"
                    return
                }
                owner.busy = false
                beginEdit("Pitch curve preset")
                setRegions(targets, name: "Pitch curve preset")
                endEdit()
                owner.status = "Pitch targets · \(preset.rawValue)"
            } catch is CancellationError { owner.status = "Pitch preset cancelled" } catch {
                self.error = error.localizedDescription
                owner.status = "Could not prepare pitch targets"
            }
        }
    }
    func setParameter(_ key: String, value: Float) {
        guard let owner, !owner.busy, !owner.isRecording else { return }
        if let effectID {
            owner.changeRealtimeEffect(effectID, name: "Pitch \(key)", coalescingKey: "\(effectID).\(key)") { item in
                item.enableParameterControls()
                item.parameters[key] = value
            }
        } else {
            owner.setSelectionParameter(key, value: value)
        }
        after = nil
        if owner.isManualPitchPreview { owner.stopTransport() }
    }
    func setParameters(_ values: [String: Float]) {
        guard let owner, !owner.busy, !owner.isRecording else { return }
        if let effectID {
            owner.changeRealtimeEffect(effectID, name: "Pitch parameter preset") { $0.parameters = values }
        } else {
            owner.effectParameters = values
        }
        after = nil
        owner.focusEditor()
        if owner.isManualPitchPreview { owner.stopTransport() }
    }

    func analyze() {
        guard let owner, let remote = owner.remoteGUI else {
            error = "Connect to the audio server before analyzing."
            return
        }

        guard let trackID, !scope.isEmpty, !owner.busy, !owner.isRecording else { return }
        requestRemoteAnalysis(
            remote, owner: owner, operation: "pitch.analyze", trackID: trackID, preview: false, original: false)
        return
    }
    private func identitiesMatch(_ assets: [UUID: AudioSamples]) -> Bool {
        sourceIdentities.allSatisfy {
            assets[$0.key]?.cacheIdentity == $0.value && owner?.assets[$0.key]?.cacheIdentity == $0.value
        }
    }
    func render(preview: Bool, auditionOriginal: Bool = false) {
        guard let owner, let remote = owner.remoteGUI else {
            error = "Connect to the audio server before rendering."
            return
        }

        guard let trackID, !scope.isEmpty, !owner.busy, !owner.isRecording else { return }
        requestRemoteAnalysis(
            remote, owner: owner, operation: "pitch.render", trackID: trackID, preview: preview,
            original: auditionOriginal)
        return
    }
    private func requestRemoteAnalysis(
        _ remote: RemoteGUIController, owner: SessionStore, operation: String, trackID: UUID, preview: Bool,
        original: Bool
    ) {
        guard let source = remoteSource() else {
            error = "Refresh the server snapshot before analyzing this track."
            return
        }
        guard !remote.hasPendingEdits(for: trackID) else {
            error = "Finishing your pitch edit on the server. Try again in a moment."
            return
        }
        generation = UUID()
        let token = generation
        let edits = regionEdits
        error = nil
        var strings = ["trackID": trackID.uuidString]
        if let effectID { strings["effectID"] = effectID.uuidString }
        owner.status =
            operation == "pitch.analyze" ? "Analyzing pitch on the server…" : "Preparing pitch output on the server…"
        let intent = RemoteEditorIntent(
            operation: operation, context: owner.remoteContext,
            integers: ["start": scope.lowerBound, "end": scope.upperBound],
            strings: strings, flags: ["preview": preview, "original": original])
        let versions = [trackID: source.version]
        remote.send(.editor(intent), expecting: versions, gesture: RemoteGestureRevisionFence(versions: versions)) {
            [weak self, weak owner] reply in
            // Rejection keeps the existing binding, analysis and private targets.
            guard let self, token == self.generation else { return }
            guard reply.accepted else {
                self.error = reply.message
                return
            }
            guard self.isCurrent(source) else {
                self.error =
                    "The track changed while the server analyzed it. Your private targets are kept; analyze again."
                return
            }
            guard let data = reply.payload, let result = try? JSONDecoder().decode(RemotePitchAnalysis.self, from: data)
            else { return }
            let points = result.points.map {
                PitchContourPoint(frame: $0.frame, frequency: $0.frequency, confidence: $0.confidence)
            }
            if result.isOutput {
                self.after = points
            } else {
                self.before = points
                self.after = nil
                if self.regionEdits == edits {
                    self.visibleRegions = result.regions
                    self.targetsAreDraft = result.targetsAreDraft
                    self.sourceNotice = nil
                }
            }
            self.error = result.error
            owner?.status = result.isOutput ? "Pitch output measured" : "Pitch analyzed"
        }
    }
    func bake() {
        guard let owner, let trackID, let effectID, !targetsAreDraft, bakePlan != nil else { return }
        let shown = presentation
        guard let remote = owner.remoteGUI else {
            error = "Connect to the audio server before applying."
            return
        }

        // Apply exactly the reviewed authored targets: fence their version and
        // confirm completion only by this request's own observed metadata.
        guard let source = remoteSource() else {
            error = "Refresh the server snapshot before applying."
            return
        }
        guard !remote.hasPendingEdits(for: trackID) else {
            error = "Finishing your pitch edit on the server. Apply again in a moment."
            return
        }
        let versions = [trackID: source.version]
        let intent = RemoteEditorIntent(
            operation: "pitch.apply", context: owner.remoteContext,
            strings: ["trackID": trackID.uuidString, "effectID": effectID.uuidString])
        owner.status = "Applying pitch to audio on the server…"
        error = nil
        remote.send(.editor(intent), expecting: versions, gesture: RemoteGestureRevisionFence(versions: versions)) {
            [weak self, weak remote] reply in
            guard let self, let remote else { return }
            guard reply.accepted else {
                self.error = reply.message
                return
            }
            guard remote.connectionGeneration == source.epoch, remote.trackRevisions[trackID] == reply.revision else {
                self.error = "The track changed after Apply. Review the current audio before continuing."
                return
            }
            if shown == self.presentation, self.trackID == trackID { self.completedBakeID = UUID() }
        }
        return
    }
}
