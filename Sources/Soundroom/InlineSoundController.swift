import Foundation
import Combine
import SoundroomCore
import SoundroomRemote

/// One clip-bound instrument draft, with at most one render awaiting acceptance.
@MainActor final class InlineSoundController: ObservableObject, Identifiable {
    let id = UUID()
    let trackID: UUID
    let clipID: UUID
    @Published private(set) var pattern: SoundDesignPattern
    @Published var selectedNotes: Set<UUID> = []
    @Published var grid = 0.25
    @Published var octave: Int
    @Published private(set) var sending = false
    @Published private(set) var hasChanges = false
    @Published private(set) var failure: String?
    private weak var store: SessionStore?
    private var confirmed: Clip
    private var connectionGeneration: Int?
    private var debounce: Task<Void, Never>?
    private var subscriptions: Set<AnyCancellable> = []
    private var whenSettled: (() -> Void)?
    private var gesture: SoundDesignPattern?
    private var copiedNotes: [SoundDesignNote] = []

    init(store: SessionStore, trackID: UUID, clip: Clip) {
        self.store = store; self.trackID = trackID; clipID = clip.id; confirmed = clip
        let editable = clip.editableSoundDesign!
        pattern = editable
        octave = max(0, min(10, (editable.notes.first?.pitch ?? 60) / 12))
        connectionGeneration = store.remoteGUI?.connectionGeneration
        store.$session.dropFirst().sink { [weak self] _ in
            Task { @MainActor [weak self] in self?.synchronize() }
        }.store(in: &subscriptions)
        // Selection, octave and pointer previews belong to this panel. Only
        // changes to history availability need to invalidate the document UI.
        $hasChanges.removeDuplicates().dropFirst().sink { [weak store] _ in store?.objectWillChange.send() }.store(in: &subscriptions)
        $failure.removeDuplicates().dropFirst().sink { [weak store] _ in store?.objectWillChange.send() }.store(in: &subscriptions)
    }
    var unsettled: Bool { hasChanges || sending }
    var canRetry: Bool { !sending && currentClip == confirmed }
    var canEdit: Bool {
        guard let store, !store.busy, !store.isRecording, store.soundDesignRequest == nil,
              currentClip?.editableSoundDesign != nil else { return false }
        guard let remote = store.remoteGUI, case .connected(_, _, let role) = remote.state else { return false }
        return role == .editor || role == .operator
    }
    var currentClip: Clip? { document?.tracks.first { $0.id == trackID }?.clips.first { $0.id == clipID } }
    private var document: Session? { store?.remoteGUI?.authoritativeSession }
    var status: String { failure ?? (currentClip == nil ? "The instrument clip is unavailable" : unsettled ? "Updating sound…" : "Edits play in the session") }
    var selected: SoundDesignNote? { pattern.notes.first { selectedNotes.contains($0.id) } }

    func change(_ edit: (inout SoundDesignPattern) -> Void) {
        guard canEdit else { return }
        var next = pattern; edit(&next)
        guard next != pattern else { return }
        pattern = next; hasChanges = next != confirmed.editableSoundDesign
        selectedNotes.formIntersection(Set(next.notes.map(\.id)))
        if currentClip == confirmed, connectionGeneration == store?.remoteGUI?.connectionGeneration { failure = nil }
        schedule()
    }
    func select(_ id: UUID, extending: Bool = false) {
        if extending { if !selectedNotes.insert(id).inserted { selectedNotes.remove(id) } }
        else { selectedNotes = [id] }
    }
    func addNote(pitch: Int, beat: Double) {
        guard beat.isFinite, beat >= 0, beat < pattern.beats, (0...127).contains(pitch), pattern.notes.count < 256 else { return }
        let note = SoundDesignNote(pitch: pitch, startBeat: floor(beat / grid) * grid,
                                   durationBeats: min(grid, pattern.beats - floor(beat / grid) * grid))
        change { $0.notes.append(note) }; if pattern.notes.contains(where: { $0.id == note.id }) { selectedNotes = [note.id] }
    }
    func deleteNotes() { change { p in p.notes.removeAll { selectedNotes.contains($0.id) } } }
    func copyNotes() { copiedNotes = pattern.notes.filter { selectedNotes.contains($0.id) } }
    func pasteNotes(at beat: Double) {
        guard beat.isFinite, let first = copiedNotes.map(\.startBeat).min() else { return }
        let copies = copiedNotes.compactMap { original -> SoundDesignNote? in
            var note = original; note.id = UUID(); note.startBeat = max(0, (beat / grid).rounded() * grid) + original.startBeat - first
            guard note.startBeat < pattern.beats else { return nil }
            note.durationBeats = min(note.durationBeats, pattern.beats - note.startBeat); return note
        }
        let room = max(0, 256 - pattern.notes.count), inserted = Array(copies.prefix(room))
        change { $0.notes.append(contentsOf: inserted) }; selectedNotes = Set(inserted.map(\.id))
    }
    func beginGesture() { guard gesture == nil else { return }; gesture = pattern; debounce?.cancel() }
    func drag(by beats: Double, semitones: Int, resizing: Bool) {
        guard let original = gesture else { return }
        change { p in
            if resizing { p.notes = original.notes.map { selectedNotes.contains($0.id) ? SoundDesignEditing.resized($0, to: $0.durationBeats + beats, patternBeats: p.beats, grid: grid) : $0 } }
            else { p.notes = SoundDesignEditing.moved(original.notes, ids: selectedNotes, by: beats, semitones: semitones, patternBeats: p.beats, grid: grid) }
        }
    }
    func endGesture() { gesture = nil; schedule() }
    func resizeSequence(_ beats: Double) {
        guard beats.isFinite, (0.25...64).contains(beats) else { return }
        change { p in
            p.beats = beats; p.notes = p.notes.filter { $0.startBeat < beats }.map { note in var n = note; n.durationBeats = min(n.durationBeats, beats - n.startBeat); return n }
            p.controllers.removeAll { $0.beat > beats }
        }
    }
    private func schedule() {
        debounce?.cancel()
        guard gesture == nil, hasChanges, !sending, failure == nil else { return }
        debounce = Task { [weak self] in
            do { try await Task.sleep(for: .milliseconds(180)) } catch { return }
            self?.flush()
        }
    }
    func flush() {
        debounce?.cancel(); debounce = nil
        guard gesture == nil, !sending, hasChanges, failure == nil, let store else { return }
        guard canEdit, let current = currentClip else { fail("Editing is unavailable. Your sound draft is retained."); return }
        guard current == confirmed, connectionGeneration == store.remoteGUI?.connectionGeneration else {
            fail("This clip or connection changed. Your sound draft is retained; reload to use the current clip."); return
        }
        let submitted = pattern; sending = true
        guard let remote = store.remoteGUI, let payload = try? JSONEncoder().encode(submitted) else { sending = false; fail("The sound could not be sent. Your draft is retained."); return }
        let source = RemoteWorkspaceSource(remote: remote, trackID: trackID, clipID: clipID, frame: current.start)
        let intent = RemoteEditorIntent(operation: "designer.commit", context: source.context, integers: ["frame": current.start],
                                        strings: ["trackID": trackID.uuidString, "clipID": clipID.uuidString], payload: payload)
        remote.commitWorkspace(intent, source: source) { [weak self] reply in
            guard let self else { return }; sending = false
            guard reply.accepted, let current = currentClip, current.soundDesign == submitted else {
                fail(reply.accepted ? "The rendered clip changed. Your sound draft is retained." : reply.message); return
            }
            accepted(current)
        }
    }

    private func accepted(_ clip: Clip) {
        confirmed = clip; hasChanges = pattern != clip.soundDesign
        if hasChanges { schedule() }
        else { let action = whenSettled; whenSettled = nil; action?() }
    }
    private func fail(_ message: String) { failure = message; debounce?.cancel(); debounce = nil; whenSettled = nil; store?.activity.record(message, kind: .failure) }
    func synchronize() {
        guard !sending else { return }
        guard let current = currentClip, current.editableSoundDesign != nil else {
            if hasChanges { fail("The source clip is unavailable. Copy or discard your retained draft to edit another clip.") }; return
        }
        if hasChanges { if current != confirmed { fail("This clip changed elsewhere. Your sound draft is retained; reload to use the current clip.") }; return }
        if current != confirmed { install(current) }
    }
    func reload() { guard !sending, let clip = currentClip, clip.editableSoundDesign != nil else { return }; install(clip) }
    private func install(_ clip: Clip) {
        debounce?.cancel(); debounce = nil; confirmed = clip; pattern = clip.editableSoundDesign!
        failure = nil; hasChanges = false; whenSettled = nil; gesture = nil
        connectionGeneration = store?.remoteGUI?.connectionGeneration; selectedNotes.formIntersection(Set(pattern.notes.map(\.id)))
    }
    func discardDraft() {
        guard !sending else { return }
        if currentClip?.editableSoundDesign != nil { reload() }
        else { store?.inlineSoundExpanded = false; store?.inlineSound = nil }
    }
    func retry() { guard canRetry else { return }; failure = nil; connectionGeneration = store?.remoteGUI?.connectionGeneration; flush() }
    func afterSettling(_ action: @escaping () -> Void) -> Bool {
        guard unsettled, failure == nil else { return false }; whenSettled = action; flush(); return true
    }
    func openDesigner() {
        guard failure == nil else { return }
        if afterSettling({ [weak self] in self?.openDesigner() }) { return }
        guard let store, currentClip?.editableSoundDesign != nil else { return }
        store.choose(track: trackID); store.selectedClip = clipID; store.selectedClips = [clipID]
        store.inlineSoundExpanded = false; store.requestSoundDesigner(editClip: true)
    }
}

enum InlineSoundLayout {
    static let controls = 76.0, ruler = 26.0, row = 20.0, footer = 52.0
    static var height: Double { controls + ruler + row * 12 + footer }
}

extension SessionStore {
    func toggleInlineSound(trackID: UUID, clipID: UUID? = nil) {
        let target = clipID ?? selectedClip ?? (selectedClips.count == 1 ? selectedClips.first : nil)
        if let inlineSound, inlineSound.trackID == trackID, inlineSound.currentClip != nil, target == nil || inlineSound.clipID == target {
            inlineSoundExpanded.toggle(); return
        }
        guard inlineSound?.unsettled != true else { inlineSoundExpanded = true; status = "Finish or reload the current sound draft before opening another clip."; return }
        guard canUseTrackTools(trackID), soundDesignRequest == nil, let track = session.tracks.first(where: { $0.id == trackID }),
              let clip = track.clips.first(where: { $0.id == target && $0.editableSoundDesign != nil }) ?? track.clips.first(where: { $0.editableSoundDesign != nil }) else { return }
        inlineSound = InlineSoundController(store: self, trackID: trackID, clip: clip); inlineSoundExpanded = true
    }
    func inlineSoundHeight(for track: Track) -> Double { inlineSoundExpanded && inlineSound?.trackID == track.id ? InlineSoundLayout.height : 0 }
}
