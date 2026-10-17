import Combine
import Foundation
import SoundroomCore
import SoundroomRemote

/// A clip-bound draft. Rendering never owns the session transport, and only
/// one replacement is in flight; subsequent edits stay private until its ack.
@MainActor final class InlineDrumController: ObservableObject, Identifiable {
    let id = UUID()
    let trackID: UUID
    let clipID: UUID
    @Published private(set) var editor: DrumEditorModel
    @Published private(set) var sending = false
    @Published private(set) var hasChanges = false
    @Published private(set) var failure: String?
    private weak var store: SessionStore?
    private var confirmed: Clip
    private var connectionGeneration: Int?
    private var debounce: Task<Void, Never>?
    private var documentSubscription: AnyCancellable?
    private var subscriptions: Set<AnyCancellable> = []
    private var whenSettled: (() -> Void)?
    private var installing = false

    init(store: SessionStore, trackID: UUID, clip: Clip) {
        self.store = store
        self.trackID = trackID
        clipID = clip.id
        confirmed = clip
        editor = DrumEditorModel(pattern: clip.editableDrumPattern!)
        connectionGeneration = store.remoteGUI?.connectionGeneration
        observeEditor()
        store.$session.dropFirst().sink { [weak self] _ in
            Task { @MainActor [weak self] in self?.synchronize() }
        }.store(in: &subscriptions)
        objectWillChange.sink { [weak store] in store?.objectWillChange.send() }.store(in: &subscriptions)
    }
    var pattern: DrumPattern { editor.pattern }
    var unsettled: Bool { hasChanges || sending }
    var canRetry: Bool { !sending && currentClip == confirmed }
    var canEdit: Bool {
        guard let store, !store.busy, !store.isRecording, currentClip?.editableDrumPattern != nil else { return false }
        guard let remote = store.remoteGUI, case .connected(_, _, let role) = remote.state else { return false }
        return role == .editor || role == .operator
    }
    var currentClip: Clip? { document?.tracks.first { $0.id == trackID }?.clips.first { $0.id == clipID } }
    private var document: Session? { store?.remoteGUI?.authoritativeSession }
    var status: String {
        failure
            ?? (currentClip?.editableDrumPattern == nil
                ? "The source beat clip is unavailable" : unsettled ? "Updating beat…" : "Edits play in the session")
    }

    private func observeEditor() {
        documentSubscription = editor.$document.dropFirst().sink { [weak self] document in
            guard let self, !installing else { return }
            hasChanges = document.patterns[document.selected] != confirmed.editableDrumPattern
            // A render error must allow correction (for example, lowering
            // Level). Changed sources still require explicit recovery.
            if canEdit, currentClip == confirmed, connectionGeneration == store?.remoteGUI?.connectionGeneration {
                failure = nil
            }
            schedule()
        }
    }
    private func schedule() {
        debounce?.cancel()
        guard hasChanges, !sending, failure == nil else { return }
        debounce = Task { [weak self] in
            do { try await Task.sleep(for: .milliseconds(160)) } catch { return }
            self?.flush()
        }
    }
    func flush() {
        debounce?.cancel()
        debounce = nil
        guard !sending, hasChanges, failure == nil, let store else { return }
        guard canEdit, let current = currentClip else {
            fail("Editing is unavailable. Your beat draft is retained.")
            return
        }
        guard current == confirmed, connectionGeneration == store.remoteGUI?.connectionGeneration else {
            fail("This clip or connection changed. Your beat draft is retained; reload to use the current clip.")
            return
        }
        let submitted = pattern
        do { try DrumEditorModel.validate(submitted) } catch {
            fail(error.localizedDescription)
            return
        }
        sending = true
        guard let remote = store.remoteGUI, let payload = try? JSONEncoder().encode(submitted) else {
            sending = false
            fail("The beat could not be sent. Your draft is retained.")
            return
        }
        let source = RemoteWorkspaceSource(remote: remote, trackID: trackID, clipID: clipID, frame: current.start)
        let intent = RemoteEditorIntent(
            operation: "drums.commit", context: source.context, integers: ["start": current.start],
            strings: ["trackID": trackID.uuidString, "clipID": clipID.uuidString], payload: payload)
        remote.commitWorkspace(intent, source: source) { [weak self] reply in
            guard let self else { return }
            sending = false
            guard reply.accepted, let current = currentClip, current.drumPattern == submitted else {
                fail(reply.accepted ? "The rendered clip changed. Your beat draft is retained." : reply.message)
                return
            }
            accepted(current)
        }
    }

    private func accepted(_ clip: Clip) {
        confirmed = clip
        hasChanges = pattern != clip.drumPattern
        if hasChanges {
            schedule()
        } else {
            let action = whenSettled
            whenSettled = nil
            action?()
        }
    }
    private func fail(_ message: String) {
        failure = message
        debounce?.cancel()
        debounce = nil
        whenSettled = nil
        store?.activity.record(message, kind: .failure)
    }
    func synchronize() {
        guard !sending else { return }
        guard let current = currentClip, let pattern = current.editableDrumPattern else {
            if hasChanges {
                fail("The source beat clip is unavailable. Copy or discard your retained draft to edit another clip.")
            }
            return
        }
        if hasChanges {
            if current != confirmed {
                fail("This clip changed elsewhere. Your beat draft is retained; reload to use the current clip.")
            }
            return
        }
        guard current != confirmed else { return }
        install(current, pattern: pattern)
    }
    func reload() {
        guard !sending, let clip = currentClip, let pattern = clip.editableDrumPattern else { return }
        install(clip, pattern: pattern)
    }
    func discardDraft() {
        guard !sending else { return }
        if currentClip?.editableDrumPattern != nil {
            reload()
        } else {
            store?.inlineDrumExpanded = false
            store?.inlineDrums = nil
        }
    }
    func retry() {
        guard canRetry else { return }
        failure = nil
        connectionGeneration = store?.remoteGUI?.connectionGeneration
        flush()
    }
    private func install(_ clip: Clip, pattern: DrumPattern) {
        debounce?.cancel()
        debounce = nil
        confirmed = clip
        failure = nil
        hasChanges = false
        whenSettled = nil
        connectionGeneration = store?.remoteGUI?.connectionGeneration
        let voice = editor.selectedVoice
        let step = min(editor.selectedStep, pattern.stepCount - 1)
        installing = true
        editor = DrumEditorModel(pattern: pattern)
        editor.selectedVoice = voice
        editor.selectedStep = step
        installing = false
        observeEditor()
    }
    /// History and the full designer wait for the newest accepted render, so
    /// a late auto-save cannot overwrite Undo or a newer modal draft.
    func afterSettling(_ action: @escaping () -> Void) -> Bool {
        guard unsettled else { return false }
        guard failure == nil else { return false }
        whenSettled = action
        flush()
        return true
    }
    func openDesigner() {
        if afterSettling({ [weak self] in self?.openDesigner() }) { return }
        guard let store, currentClip?.editableDrumPattern != nil else { return }
        store.choose(track: trackID)
        store.selectedClip = clipID
        store.selectedClips = [clipID]
        store.inlineDrumExpanded = false
        store.requestDrumDesigner(editClip: true)
    }
}

enum InlineDrumLayout {
    static let controls = 76.0, ruler = 38.0, row = 24.0, gap = 4.0, footer = 34.0
    static var grid: Double { Double(DrumVoice.allCases.count) * (row + gap) }
    static var height: Double { controls + ruler + grid + footer + 8 }
}

extension SessionStore {
    func toggleInlineDrums(trackID: UUID, clipID: UUID? = nil) {
        let target = clipID ?? selectedClip ?? (selectedClips.count == 1 ? selectedClips.first : nil)
        if let inlineDrums, inlineDrums.trackID == trackID, inlineDrums.currentClip != nil,
            target == nil || inlineDrums.clipID == target
        {
            inlineDrumExpanded.toggle()
            return
        }
        guard inlineDrums?.unsettled != true else {
            inlineDrumExpanded = true
            status = "Finish or reload the current beat draft before opening another clip."
            return
        }
        guard canUseTrackTools(trackID), let track = session.tracks.first(where: { $0.id == trackID }),
            let clip = track.clips.first(where: { $0.id == target && $0.editableDrumPattern != nil })
                ?? track.clips.first(where: { $0.editableDrumPattern != nil })
        else { return }
        inlineDrums = InlineDrumController(store: self, trackID: trackID, clip: clip)
        inlineDrumExpanded = true
    }
    func inlineDrumHeight(for track: Track) -> Double {
        inlineDrumExpanded && inlineDrums?.trackID == track.id ? InlineDrumLayout.height : 0
    }
}
