import AVFoundation
import Combine
import Foundation
import SoundroomCore
import SoundroomRemote

struct EditSelection: Equatable {
    var trackID: UUID
    var range: Range<Int>
}
struct RenameRequest: Identifiable {
    let id: UUID
    let name: String
}
enum EffectMode: String, CaseIterable, Identifiable {
    case selection = "Selection"
    case realtime = "Real-time"
    case master = "Master"
    var id: Self { self }
}
struct SoundDesignRequest: Identifiable {
    let id: UUID
    let clipID: UUID?
    let trackID: UUID?
    let pattern: SoundDesignPattern
    let frame: Int
    let source: RemoteWorkspaceSource?
    init(
        id: UUID = UUID(), clipID: UUID?, trackID: UUID?, pattern: SoundDesignPattern, frame: Int,
        source: RemoteWorkspaceSource? = nil
    ) {
        self.id = id
        self.clipID = clipID
        self.trackID = trackID
        self.pattern = pattern
        self.frame = frame
        self.source = source
    }
}
private enum EditorClipboard {
    case audio(clips: [Clip], assets: [UUID: AudioSamples])
    case track(Track, assets: [UUID: AudioSamples], includesAutomation: Bool)
}

@MainActor final class SessionStore: ObservableObject {
    let headless: Bool
    private let presentation: (any SessionPresentation)?
    var remoteGUI: RemoteGUIController?

    let effectPresets = EffectPresetLibrary()
    let timelineEditing = TimelineEditingState()
    let automationEditing = AutomationEditingState()
    let activity = WorkspaceActivity()
    let fileTransfer = FileTransferProgress()
    let waveformDisplay = WaveformDisplay()
    @Published var trackSize = TrackSize.comfortable {
        didSet { if !headless { UserDefaults.standard.set(trackSize.rawValue, forKey: "soundroom.trackSize") } }
    }
    @Published var effectsDrawerWidth = 360.0 {
        didSet {
            if !headless { UserDefaults.standard.set(effectsDrawerWidth, forKey: "soundroom.effectsDrawerWidth") }
        }
    }
    @Published var sourceSeparationRequest: SourceSeparationController?
    @Published var vocalEnhancementRequest: VocalEnhancementRequest?
    @Published var showCommandSearch = false
    @Published var session = Session() {
        didSet {
            synchronizeAutomationSelectionContext()
            if oldValue.timing != session.timing { recordingTempo = session.timing.tempo }
        }
    }
    @Published var assets: [UUID: AudioSamples] = [:]
    @Published var selection: EditSelection?
    @Published var selectedTrack: UUID? {
        didSet {
            if selectedTrack != oldValue {
                restoreLiveEffectDestination()
                selectedRealtimeEffect = currentTrack?.realtimeEffects.first?.id
                if selectedTrack == Session.masterTrackID && automationTarget == .pan { automationTarget = .volume }
            }
            synchronizeAutomationSelectionContext()
        }
    }
    @Published var selectedClip: UUID?
    @Published var selectedClips: Set<UUID> = []
    @Published var followPlayhead = true
    /// Playback region is independent of the audio edit selection and selected track.
    var definingLoopRange = false
    @Published var loopRange: Range<Int>?
    @Published var loopSelection = false {
        didSet {
            guard remoteGUI?.applyingServerState != true else { return }
            if loopSelection && !oldValue && !definingLoopRange { loopRange = timelineSelectionRange ?? loopRange }
            remoteGUI?.send(
                .setLoop(
                    start: loopSelection ? loopRange?.lowerBound : nil, end: loopSelection ? loopRange?.upperBound : nil
                ))
        }
    }
    @Published var visibleTimelineWidth: Double = 780
    @Published var revealFrame: Int?
    @Published var revealRequestSerial: UInt64 = 0
    /// Keyboard zoom: the timeline view anchors it on the playhead or viewport center.
    var timelineZoomFactor: Double = 1
    /// Reveal only when the frame is offscreen (Tab navigation), not re-anchor.
    var revealOnlyIfHidden = false
    /// Where the next ⌘V lands while the cursor, selection and track are as the
    /// last copy/paste left them: right after the copied or pasted audio.
    private var pasteChain:
        (origin: Int, pasted: [(ids: Set<UUID>, end: Int)], track: UUID?, playhead: Int, selection: EditSelection?)?
    /// A held ⌘V must not queue a paste fenced on a track version the previous
    /// paste is about to replace: the server would reject it and roll it back.
    var pasteAwaitsServer: Bool {
        guard let remoteGUI, let track = selectedTrack else { return false }
        return remoteGUI.hasPendingEdits(for: track)
    }
    @Published var timelineZoomSerial: UInt64 = 0
    var timelineScrollOffset: Double = 0
    @Published var timelineScrollSerial: UInt64 = 0
    @Published var compareOriginal = false
    @Published var effectParameters: [String: Float] = [:]
    @Published var includeEffectTail = false
    @Published var automationWrite = false
    @Published var automationTarget = AutomationTarget.effect {
        didSet { synchronizeAutomationSelectionContext() }
    }
    @Published var exportTracks = false
    @Published var exportIncludeTails = true
    @Published var exportTrackIDs: Set<UUID> = []
    @Published var moveAutomationWithClips = false
    @Published var cancelWork = false
    let playbackPosition = PlaybackPosition()
    let playbackLevel = PlaybackLevel()
    @Published private(set) var transportExtent = 0
    private var clientEditCursorFrame = 0
    var playhead: Int {
        get { clientEditCursorFrame }
        set {
            clientEditCursorFrame = max(0, newValue)
            if !isPlaying { playbackPosition.update(newValue) }
            // Grow recording/tail space in chunks, instead of relaying out the
            // entire timeline on every render-clock tick beyond the last clip.
            if newValue > session.end {
                if newValue > transportExtent { transportExtent = newValue + Int(sessionRate * 5) }
            } else if transportExtent != 0 {
                transportExtent = 0
            }
        }
    }
    @Published var auditionTrackID: UUID?
    @Published var isPlaying = false
    @Published var isRecording = false
    @Published var workProgress: Double?
    @Published var busy = false
    @Published var status = "Ready" {
        didSet { if status != oldValue { activity.record(status) } }
    }
    @Published var zoom: Double = 85
    @Published var snap = true
    @Published var inputDevice: AudioDeviceID
    @Published var outputDevice: AudioDeviceID
    @Published var devices: [AudioDevice] = []
    let inputActivity = InputActivity()
    var inputPeaks: [Float] {
        get { inputActivity.peaks }
        set { inputActivity.peaks = newValue }
    }
    var liveInputHistory: LivePeakHistory {
        get { inputActivity.history }
        set { inputActivity.history = newValue }
    }
    @Published var recordingTempo: Double = 120
    @Published var countInBars = 0
    @Published var preRollSeconds: Double = 0
    @Published var punchRecording = false
    /// The transport click, heard during playback and recording. The server
    /// owns the sound; this client re-asserts its choice after connecting.
    @Published var metronome = false {
        didSet {
            guard metronome != oldValue else { return }
            if !headless { UserDefaults.standard.set(metronome, forKey: "soundroom.metronome") }
            syncMetronome()
        }
    }
    let trackDragging = TrackDragState()
    let inputVolume = InputVolumeModel()
    @Published var showRecordingSetup = false
    @Published var showTakeBrowser = false
    @Published var recordingOffsetMS: Double = 0
    @Published var inputMonitoring = false {
        didSet {
            if remoteGUI?.applyingServerState != true {
                _ = remoteRoute("recording.settings", flags: ["inputMonitoring": inputMonitoring])
            }
        }
    }
    var recordingStartFrame: Int { remoteGUI?.recordingStartFrame ?? clientEditCursorFrame }
    var workTask: Task<Void, Never>?
    var isFinishingRecording: Bool { false }
    var canCancelBackgroundWork: Bool { busy }
    @Published var clipMovePreview: (ids: Set<UUID>, shift: Int, guide: Int?)?
    var meter: Float {
        get { playbackLevel.peak }
        set { playbackLevel.update(newValue) }
    }
    @Published var projectURL: URL?
    @Published var revision = UUID()
    @Published var renameRequest: RenameRequest?
    @Published var sessionRenameDraft: SessionRenameDraft?
    @Published var soundDesignRequest: SoundDesignRequest?
    @Published var soundDesignCommitPending = false
    @Published var soundDesignCommitMessage = ""
    var soundDesignDraftVersion = 0
    private var soundDesignCommitID: UUID?
    @Published var drumDesignRequest: DrumDesignRequest?
    lazy var drumDesigner = DrumDesignerController(owner: self)
    @Published var inlineDrums: InlineDrumController?
    @Published var inlineDrumExpanded = false
    @Published var inlineSound: InlineSoundController?
    @Published var inlineSoundExpanded = false
    lazy var pitchEditor = PitchEditorController(owner: self)
    @Published var automationLaneVisible = false
    @Published var effectMode = EffectMode.realtime
    @Published var selectedRealtimeEffect: UUID? {
        didSet {
            synchronizeAutomationSelectionContext()
            if remoteGUI?.applyingServerState != true {
                remoteGUI?.route(
                    .init(
                        operation: "meters.subscribe",
                        payload: try? JSONEncoder().encode(selectedRealtimeEffect.map { [$0] } ?? [])))
            }
        }
    }
    @Published var effect = BuiltInEffect.normalize
    @Published var amount: Float = 0.5
    @Published var showEffects = true
    @Published var showDevices = false
    @Published var showExport = false
    @Published var exportFormat = "wav"
    @Published var exportSelection = false
    private var savedRevision: UUID?
    @Published private var clipboard: EditorClipboard?
    var isManualPitchPreview: Bool { false }
    func effectMeterSnapshot(_ id: UUID) -> EffectMeterSnapshot? { remoteGUI?.effectMeters[id] }
    var designerPreviewBeat: Double { remoteGUI?.designerBeat ?? 0 }
    @Published var designerPreviewing = false
    var canToggleTransport: Bool { !busy || isPlaying || isRecording || remoteGUI?.transportPreparing == true }
    var canToggleRecording: Bool { !busy || isRecording || (remoteGUI?.transportPreparing == true) }
    var recordingInProgress: Bool { isRecording || (remoteGUI?.transportPreparing == true) }
    var automationTransportCursor: (frame: Int, serial: Int?) { (playhead, nil) }
    var automationPlaybackLoopRange: Range<Int>? { isPlaying && loopSelection ? loopRange : nil }
    @Published private var previewing = false
    private var continuousEditKey: String?
    private var lastContinuousChange = Date.distantPast
    private var draggingEditKey: String?
    private var automationSelectionContextID: String?
    private var restoringAutomationSelection = false
    var mixWriteFrames: [String: Int] = [:]
    var canUndo: Bool {
        ((inlineDrums?.hasChanges == true && inlineDrums?.failure == nil)
            || (inlineSound?.hasChanges == true && inlineSound?.failure == nil) || (remoteGUI?.canUndo == true))
            && !busy && !isRecording
    }
    var canRedo: Bool { (remoteGUI?.canRedo == true) && !busy && !isRecording }
    var undoTitle: String { remoteGUI?.undoName ?? "Undo" }
    var redoTitle: String { remoteGUI?.redoName ?? "Redo" }
    var canPaste: Bool { clipboard != nil && !busy && !isRecording }
    var canPasteTrack: Bool {
        guard !busy, !isRecording, case .track = clipboard else { return false }
        return true
    }
    var isPreviewing: Bool { previewing }
    var dirty: Bool { savedRevision != revision }
    func recordingInputChannels(for track: Track) -> [Int] { track.deviceInputChannels }
    var inputChannels: Int { devices.first(where: { $0.id == inputDevice })?.inputChannels ?? 0 }
    var selectedRange: Range<Int>? {
        if let selection, !selection.range.isEmpty { return selection.range }
        if let clip = currentClip { return clip.start..<clip.end }
        return nil
    }
    var currentTrack: Track? {
        selectedTrack == Session.masterTrackID ? session.masterTrack : session.tracks.first { $0.id == selectedTrack }
    }
    /// Selection is the single destination for both inserts and automation.
    /// Audio edits temporarily use selection mode while their sheet is open.
    func restoreLiveEffectDestination() { effectMode = selectedTrack == Session.masterTrackID ? .master : .realtime }
    var currentClip: Clip? { currentTrack?.clips.first { $0.id == selectedClip } }
    var currentRealtimeEffect: RealtimeEffect? {
        currentTrack?.realtimeEffects.first { $0.id == selectedRealtimeEffect } ?? currentTrack?.realtimeEffects.first
    }
    func showsAutomation(_ track: Track) -> Bool {
        (effectMode == .realtime || effectMode == .master) && automationLaneVisible && track.id == selectedTrack
            && (automationTarget != .effect || currentRealtimeEffect != nil)
    }
    var hasAudioSelection: Bool {
        guard let id = selectedTrack, let range = selectedRange else { return false }
        return !session.fragments(trackID: id, range: range).isEmpty
    }
    var timelineSeconds: Double { max(20, Double(max(session.end, transportExtent)) / sessionRate + 8) }
    var inputName: String { devices.first { $0.id == inputDevice }?.name ?? "Choose input" }
    var outputName: String { devices.first { $0.id == outputDevice }?.name ?? "Choose output" }

    init(headless: Bool = false, presentation: (any SessionPresentation)? = nil) {
        self.headless = headless
        self.presentation = headless ? nil : presentation
        if !headless {
            trackSize =
                TrackSize(rawValue: UserDefaults.standard.string(forKey: "soundroom.trackSize") ?? "") ?? .comfortable
            let width = UserDefaults.standard.double(forKey: "soundroom.effectsDrawerWidth")
            if width.isFinite, width >= 280 { effectsDrawerWidth = min(600, width) }
            metronome = UserDefaults.standard.bool(forKey: "soundroom.metronome")
        }
        inputDevice = 0
        outputDevice = 0
        selectedTrack = session.tracks.first?.id
        savedRevision = revision
        zoom = Self.clampedZoom(UserDefaults.standard.double(forKey: "timelineZoom").nonZero(or: 85))
        exportFormat = UserDefaults.standard.string(forKey: "exportFormat") ?? "wav"
    }
    /// Point frames identify knots only within one track/lane/insert/parameter.
    /// Keep the clipboard, but never carry a selected range into another context.
    private func synchronizeAutomationSelectionContext() {
        guard !restoringAutomationSelection else { return }
        let context = automationContextID
        let previous = automationSelectionContextID
        automationSelectionContextID = context
        if let previous, previous != context { automationEditing.resetSelection() }
    }
    func report(_ error: Error) { status = error.localizedDescription }
    func refreshDevices() { remoteGUI?.send(.snapshot) }

    func beginContinuousEdit(_ key: String) {
        draggingEditKey = key
        mixWriteFrames = [:]
        remoteGUI?.beginGesture(key)
    }
    func endContinuousEdit(finishingAutomation: Bool = true) {
        if finishingAutomation { finishAutomationTouches() }
        continuousEditKey = nil
        draggingEditKey = nil
        mixWriteFrames = [:]
        remoteGUI?.endGesture()
    }
    func isContinuingEdit(_ key: String) -> Bool {
        continuousEditKey == key && (draggingEditKey == key || Date().timeIntervalSince(lastContinuousChange) < 0.45)
    }
    func focusEditor() {
        guard !headless else { return }
        presentation?.focusEditor()
    }
    func edit(_ name: String = "Edit", _ change: () -> Void) {
        guard !isRecording, !busy else { return }
        let before = session
        change()
        session.version = session.requiredProjectVersion
        remoteCommitMetadata(before: before, after: session, name: name, gestureID: draggingEditKey)
    }
    func undo() {
        if inlineSound?.afterSettling({ [weak self] in self?.undo() }) == true { return }
        if inlineDrums?.afterSettling({ [weak self] in self?.undo() }) == true { return }
        remoteGUI?.send(.undo)
    }
    func redo() {
        if inlineSound?.afterSettling({ [weak self] in self?.redo() }) == true { return }
        if inlineDrums?.afterSettling({ [weak self] in self?.redo() }) == true { return }
        remoteGUI?.send(.redo)
    }

    func addTrack() {
        edit("Add track") {
            let track = Track(name: "Audio \(session.tracks.count + 1)", color: session.tracks.count % 5)
            session.tracks.append(track)
            selectedTrack = track.id
            selectedClip = nil
            selectedClips = []
            selection = nil
        }
    }
    func removeTrack(_ id: UUID) {
        let name = session.tracks.first { $0.id == id }?.name
        edit("Delete track") {
            session.tracks.removeAll { $0.id == id }
            for i in session.tracks.indices {
                if session.tracks[i].routing?.outputID == id { session.tracks[i].routing?.outputID = nil }
                if session.tracks[i].routing?.sidechainSourceID == id {
                    session.tracks[i].routing?.sidechainSourceID = nil
                }
                session.tracks[i].routing?.sends.removeAll { $0.trackID == id }
            }
            if selectedTrack == id {
                selectedTrack = session.tracks.first?.id
                selection = nil
                selectedClip = nil
            }
        }
        if let name, !session.tracks.contains(where: { $0.id == id }) { status = "Deleted \(name). ⌘Z to undo." }
    }
    var canDeleteSelectedTrack: Bool {
        !busy && !isRecording && selectedTrack.map { id in session.tracks.contains { $0.id == id } } == true
    }
    /// ⌘⌫: delete the selected track (not the master). Undoable.
    func deleteSelectedTrack() {
        guard canDeleteSelectedTrack, let id = selectedTrack else { return }
        removeTrack(id)
    }
    func requestRename(_ id: UUID) {
        guard !busy, !isRecording, let track = session.tracks.first(where: { $0.id == id }) else { return }
        endContinuousEdit()
        renameRequest = RenameRequest(id: id, name: track.name)
    }
    func renameTrack(_ id: UUID, to name: String) {
        let name = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { return }
        updateTrack(id, actionName: "Rename track") { $0.name = name }
    }
    func updateTrack(
        _ id: UUID, actionName: String = "Track settings", coalescingKey: String? = nil, _ change: (inout Track) -> Void
    ) {
        guard !isRecording, !busy else { return }
        let isMaster = id == Session.masterTrackID
        let index = session.tracks.firstIndex(where: { $0.id == id })
        guard isMaster || index != nil else { return }
        let before = session
        let old = isMaster ? session.masterTrack : session.tracks[index!]
        var changed = old
        change(&changed)
        guard changed != old else { return }
        if changed.realtimeEffects.count != old.realtimeEffects.count
            || changed.realtimeEffects.reduce(0, { $0 + $1.latencyFrames })
                != old.realtimeEffects.reduce(0, { $0 + $1.latencyFrames })
        {
            var candidate = session
            if isMaster { candidate.masterTrack = changed } else { candidate.tracks[index!] = changed }
            do { _ = try TrackRoutingPlan(session: candidate) } catch {
                status = error.localizedDescription
                return
            }
        }
        var next = session
        if isMaster { next.masterTrack = changed } else { next.tracks[index!] = changed }
        next.version = next.requiredProjectVersion
        session = next
        remoteCommitMetadata(before: before, after: next, name: actionName, gestureID: coalescingKey)
    }
    func seek(_ frame: Int) {
        playhead = max(0, frame)
        focusEditor()
    }
    func choose(track: UUID, clip: UUID? = nil, extending: Bool = false) {
        guard !isRecording, !busy, track == Session.masterTrackID || session.tracks.contains(where: { $0.id == track })
        else { return }
        automationEditing.hasFocus = false
        endContinuousEdit()
        focusEditor()
        selectedTrack = track
        selectedClip = clip
        selection = nil
        restoreLiveEffectDestination()
        if extending, let clip {
            if selectedClips.contains(clip) { selectedClips.remove(clip) } else { selectedClips.insert(clip) }
        } else {
            selectedClips = clip.map { [$0] } ?? []
        }
    }
    func select(track: UUID, start: Int, end: Int) {
        automationEditing.hasFocus = false
        guard !isRecording, !busy else { return }
        endContinuousEdit()
        focusEditor()
        stopTransport()
        selectedTrack = track
        selectedClip = nil
        selectedClips = []
        selection = EditSelection(trackID: track, range: max(0, min(start, end))..<max(0, max(start, end)))
        playhead = max(0, min(start, end))
    }
    func moveClip(_ id: UUID, on trackID: UUID, to frame: Int) {
        guard let t = session.tracks.firstIndex(where: { $0.id == trackID }),
            let c = session.tracks[t].clips.firstIndex(where: { $0.id == id })
        else { return }
        var target = max(0, frame)
        if snap {
            let anchors =
                [0, playhead] + session.tracks.flatMap(\.clips).filter { $0.id != id }.flatMap { [$0.start, $0.end] }
            let threshold = Int(sessionRate * 8 / zoom)
            if let nearest = anchors.min(by: { abs($0 - target) < abs($1 - target) }), abs(nearest - target) < threshold
            {
                target = nearest
            }
        }
        guard target != session.tracks[t].clips[c].start else { return }
        edit("Move clip") {
            session.tracks[t].clips[c].start = target
            selection = nil
            selectedClip = id
            selectedTrack = trackID
        }
        status = "Clip moved to \(timeString(target))."
    }
    func nudge(_ direction: Int, large: Bool = false) {
        guard let clip = currentClip else {
            seek(playhead + direction * Int(sessionRate * (large ? 1 : 0.01)))
            return
        }
        let old = snap
        snap = false
        moveSelectedClips(primary: clip.id, to: clip.start + direction * Int(sessionRate * (large ? 1 : 0.01)))
        snap = old
    }
    func alignToPlayhead() {
        guard let clip = currentClip else { return }
        moveSelectedClips(primary: clip.id, to: playhead)
    }
    func moveClipToTrack(_ targetID: UUID) {
        guard let clip = currentClip, targetID != selectedTrack else { return }
        moveSelectedClips(primary: clip.id, to: clip.start, targetTrack: targetID)
    }
    func split() {
        guard !busy, !isRecording, let track = currentTrack else { return }
        let ids = track.clips.filter { $0.start < playhead && $0.end > playhead }.map(\.id)
        guard !ids.isEmpty else {
            status = "Place the playhead inside a clip to split it."
            return
        }
        edit("Split") {
            let existingIDs = Set(track.clips.map(\.id))
            for id in ids { session.split(trackID: track.id, clipID: id, at: playhead) }
            let newIDs = Set(session.tracks.first(where: { $0.id == track.id })?.clips.map(\.id) ?? []).subtracting(
                existingIDs)
            selectedClips = Set(ids).union(newIDs)
            selectedClip = nil
            selection = nil
        }
        status = "Split at \(timeString(playhead))."
    }
    func copy() {
        guard !busy, !isRecording else { return }
        let clips: [Clip]
        if selectedClips.count > 1 {
            let selected = session.tracks.flatMap(\.clips).filter { selectedClips.contains($0.id) }
            let first = selected.map(\.start).min() ?? 0
            clips = selected.map { original in
                var clip = original
                clip.start -= first
                return clip
            }
        } else {
            guard let trackID = selectedTrack, let range = selectedRange else { return }
            clips = session.fragments(trackID: trackID, range: range)
        }
        guard !clips.isEmpty else { return }
        clipboard = .audio(
            clips: clips, assets: assets.filter { key, _ in clips.contains { $0.referencedAssetIDs.contains(key) } })
        status = "Audio copied"
        let end =
            selectedClips.count > 1
            ? session.tracks.flatMap(\.clips).filter { selectedClips.contains($0.id) }.map(\.end).max()
            : selectedRange?.upperBound
        pasteChain = end.map { ($0, [], selectedTrack, playhead, selection) }
    }
    func copyTrack(_ id: UUID? = nil, includeAutomation: Bool = false) {
        guard !busy, !isRecording, let trackID = id ?? selectedTrack,
            var track = session.tracks.first(where: { $0.id == trackID })
        else { return }
        if !includeAutomation {
            track.mixAutomation = [:]
            for i in track.realtimeEffects.indices {
                track.realtimeEffects[i].automation = []
                track.realtimeEffects[i].parameterAutomation = [:]
                track.realtimeEffects[i].pitchEdits = []
                track.realtimeEffects[i].pitchEditsOnly = false
                // These ranges protect already rendered source audio from a
                // second application of this insert. They belong to the media.
            }
        }
        let used = track.referencedAssetIDs
        clipboard = .track(
            track, assets: assets.filter { used.contains($0.key) }, includesAutomation: includeAutomation)
        endContinuousEdit()
        focusEditor()
        status = includeAutomation ? "Track copied with automation" : "Track copied"
    }
    func copyUnsentTrack(_ track: Track) {
        let used = track.referencedAssetIDs
        clipboard = .track(track, assets: assets.filter { used.contains($0.key) }, includesAutomation: true)
        status = "Unsent track copied with automation. Paste to create a separate track."
    }
    func cut() {
        copy()
        pasteChain = nil
        deleteSelectedClips(actionName: "Cut")
    }
    func paste() {
        guard !busy, !isRecording, let clipboard else { return }
        if case .track = clipboard {
            pasteTrack()
            return
        }
        guard case let .audio(clips, copiedAssets) = clipboard else { return }
        if currentTrack?.acceptsClips == false || selectedTrack == Session.masterTrackID {
            status = "Choose an audio or instrument track before pasting clips."
            return
        }
        // Until the user places the cursor, repeated ⌘V lays copies end to end
        // after the source instead of stacking them on one position.
        let destination: Int
        var chained = false
        if let chain = pasteChain, chain.track == selectedTrack, chain.selection == selection {
            chained = chain.playhead == playhead
            // A cursor moved since then (e.g. a ruler click) wins over the stale range.
            // Otherwise continue after the latest paste that still exists: a
            // paste the server rejected and rolled back must not leave a gap.
            let present = Set(currentTrack?.clips.map(\.id) ?? [])
            destination =
                chain.playhead == playhead
                ? chain.pasted.last { !$0.ids.isDisjoint(with: present) }?.end ?? chain.origin : playhead
        } else {
            destination = selection?.range.lowerBound ?? playhead
        }
        var pastedIDs: Set<UUID> = []
        edit("Paste") {
            if selectedTrack == nil {
                let track = Track(name: "Audio \(session.tracks.count + 1)", color: session.tracks.count % 5)
                session.tracks.append(track)
                selectedTrack = track.id
            }
            guard let t = session.tracks.firstIndex(where: { $0.id == selectedTrack }) else { return }
            assets.merge(copiedAssets) { current, _ in current }
            for var clip in clips {
                clip.id = UUID()
                clip.start += destination
                pastedIDs.insert(clip.id)
                session.tracks[t].clips.append(clip)
            }
            selectedClips = pastedIDs
            selectedClip = nil
            if let end = clips.map(\.end).max() {
                selection = EditSelection(trackID: session.tracks[t].id, range: destination..<(destination + end))
            } else {
                selection = nil
            }
        }
        if let end = clips.map(\.end).max() {
            let earlier = chained ? pasteChain : nil
            let anchors = (earlier?.pasted ?? []).suffix(63) + [(pastedIDs, destination + end)]
            pasteChain = (earlier?.origin ?? destination, Array(anchors), selectedTrack, playhead, selection)
        }
        status = "Pasted at \(timeString(destination))."
    }
    func pasteTrack(after trackID: UUID? = nil) {
        guard !busy, !isRecording, case let .track(source, copiedAssets, includesAutomation) = clipboard else { return }
        let destination = max(0, playhead)
        let origin = source.clips.map(\.start).min() ?? 0
        let shift = destination - origin
        var pasted = source
        pasted.id = UUID()
        pasted.armed = false
        if var routing = pasted.routing {
            if !session.tracks.contains(where: { $0.id == routing.outputID && $0.routing?.role == "bus" }) {
                routing.outputID = nil
            }
            routing.sends.removeAll { send in
                !session.tracks.contains(where: { $0.id == send.trackID && $0.routing?.role == "return" })
            }
            if !session.tracks.contains(where: { $0.id == routing.sidechainSourceID }) {
                routing.sidechainSourceID = nil
            }
            pasted.routing = routing
        }
        let baseName = "\(source.name) copy"
        let names = Set(session.tracks.map(\.name))
        pasted.name = baseName
        var suffix = 2
        while names.contains(pasted.name) {
            pasted.name = "\(baseName) \(suffix)"
            suffix += 1
        }
        for i in pasted.clips.indices {
            let (start, overflow) = destination.addingReportingOverflow(source.clips[i].start - origin)
            guard !overflow, start <= Int.max - pasted.clips[i].length else {
                status = "The pasted track exceeds the timeline limit"
                return
            }
            pasted.clips[i].id = UUID()
            pasted.clips[i].start = start
        }
        let activeTakeIndex = source.recordingTakes.firstIndex { $0.id == source.activeTakeID }
        for index in pasted.recordingTakes.indices {
            pasted.recordingTakes[index].id = UUID()
            var shifted: [Clip] = []
            for var clip in source.recordingTakes[index].clips {
                let (start, overflow) = clip.start.addingReportingOverflow(shift)
                guard !overflow, start <= Int.max - clip.length else {
                    status = "The pasted takes exceed the timeline limit"
                    return
                }
                if start < 0 {
                    let trim = -start
                    guard trim < clip.length else { continue }
                    clip.offset += trim
                    clip.length -= trim
                    clip.fadeReferenceLength = clip.fadeReferenceLength ?? (clip.length + trim)
                    clip.fadeReferenceOffset = (clip.fadeReferenceOffset ?? 0) + trim
                }
                clip.id = UUID()
                clip.start = max(0, start)
                shifted.append(clip)
            }
            pasted.recordingTakes[index].clips = shifted
        }
        pasted.activeTakeID = activeTakeIndex.map { pasted.recordingTakes[$0].id }
        for parameter in TrackAutomationParameter.allCases {
            let curve = source.mixAutomation[parameter.rawValue] ?? []
            guard
                let moved = AutomationCurveEditing.relocating(
                    curve, by: shift, baseline: source.baseValue(for: parameter),
                    scale: parameter == .volume ? source.volumeAutomationScale ?? .linear : .linear)
            else {
                status = "The pasted automation exceeds the timeline limit"
                return
            }
            if !curve.isEmpty { pasted.mixAutomation[parameter.rawValue] = moved }
        }
        for i in pasted.realtimeEffects.indices {
            let original = source.realtimeEffects[i]
            pasted.realtimeEffects[i].id = UUID()
            pasted.realtimeEffects[i].pitchEdits = PitchEditRegions.relocated(
                original.pitchEdits, by: shift, renewIDs: true)
            pasted.realtimeEffects[i].bakedRanges = PitchEditRegions.relocatedRanges(original.bakedRanges, by: shift)
            guard
                let moved = AutomationCurveEditing.relocating(original.automation, by: shift, baseline: original.amount)
            else {
                status = "The pasted automation exceeds the timeline limit"
                return
            }
            pasted.realtimeEffects[i].automation = moved
            for (key, curve) in original.parameterAutomation {
                guard
                    let moved = AutomationCurveEditing.relocating(
                        curve, by: shift, baseline: original.value(at: 0, parameter: key))
                else {
                    status = "The pasted automation exceeds the timeline limit"
                    return
                }
                pasted.realtimeEffects[i].parameterAutomation[key] = moved
            }
        }
        let afterID = trackID ?? selectedTrack
        edit(includesAutomation ? "Paste track + automation" : "Paste track") {
            assets.merge(copiedAssets) { current, _ in current }
            let index = session.tracks.firstIndex { $0.id == afterID }.map { $0 + 1 } ?? session.tracks.count
            session.tracks.insert(pasted, at: index)
            selectedTrack = pasted.id
            selectedClip = pasted.clips.first?.id
            selectedClips = Set(pasted.clips.map(\.id))
            selection = nil
            selectedRealtimeEffect = pasted.realtimeEffects.first?.id
            playhead = destination
        }
        status = "Track pasted at \(timeString(destination))"
    }
    func deleteSelection(actionName: String = "Delete audio") {
        guard let id = selectedTrack, let range = selectedRange, hasAudioSelection else { return }
        edit(actionName) {
            session.delete(trackID: id, range: range, operation: actionName)
            selection = nil
            selectedClip = nil
            playhead = range.lowerBound
        }
        status = "Audio removed"
    }
    func trim() {
        guard let id = selectedTrack, let range = selectedRange, selection != nil else { return }
        edit("Trim") {
            guard let t = session.tracks.firstIndex(where: { $0.id == id }) else { return }
            session.tracks[t].clips = session.fragments(trackID: id, range: range).map { part in
                var clip = part
                clip.start += range.lowerBound
                return clip
            }
            selectedClip = nil
        }
        status = "Track trimmed to the selection."
    }
    func selectAll() {
        guard let track = currentTrack, let start = track.clips.map(\.start).min(),
            let end = track.clips.map(\.end).max()
        else { return }
        select(track: track.id, start: start, end: end)
    }
    func togglePlay() {
        if isPlaying || isRecording || remoteGUI?.transportPreparing == true {
            _ = remoteRoute("transport.pause")
        } else {
            _ = remoteRoute("transport.toggle")
        }
    }

    func toggleEffectAudition(trackID: UUID?) {
        _ = remoteRoute("transport.audition", strings: trackID.map { ["trackID": $0.uuidString] } ?? [:])
    }
    func play(trackOnly: UUID? = nil) {
        _ = remoteRoute("transport.play", strings: trackOnly.map { ["trackID": $0.uuidString] } ?? [:])
    }

    func pause() { _ = remoteRoute("transport.pause") }
    func stopTransport() { _ = remoteRoute("transport.stop") }
    func returnToStart() {
        guard !isRecording else { return }
        if isPlaying, let remoteGUI {
            let start = max(0, playhead)
            remoteGUI.send(.seek(frame: start))
            requestTimelineReveal(start)
            return
        }
        playhead = 0
        selection = nil
        requestTimelineReveal(0)
    }
    func record() {
        // Finish is an interrupt, not another start request. Route it before
        // permission checks and busy admission so saving/edit work cannot hide Stop.
        if isRecording {
            finishRecording()
            return
        }
        if remoteGUI?.transportPreparing == true {
            stopTransport()
            return
        }
        // The local host records this Mac's microphone on the app's behalf; ask
        // before starting capture so a take never starts silent.
        if remoteGUI?.isLocalServer == true, !isRecording,
            AVCaptureDevice.authorizationStatus(for: .audio) != .authorized
        {
            switch AVCaptureDevice.authorizationStatus(for: .audio) {
            case .notDetermined:
                status = "Waiting for microphone permission…"
                Task { @MainActor [weak self] in
                    if await AVCaptureDevice.requestAccess(for: .audio) {
                        self?.record()
                    } else {
                        self?.report(
                            SoundroomError.message(
                                "Microphone access is off. Enable Soundroom in System Settings → Privacy & Security → Microphone, then try again."
                            ))
                    }
                }
            default:
                report(
                    SoundroomError.message(
                        "Microphone access is off. Enable Soundroom in System Settings → Privacy & Security → Microphone, then try again."
                    ))
            }
            return
        }
        _ = remoteRoute(
            "transport.record", integers: ["countInBars": countInBars],
            numbers: ["tempo": recordingTempo, "preRollSeconds": preRollSeconds, "offsetMS": recordingOffsetMS],
            flags: ["punch": punchRecording, "metronome": metronome, "inputMonitoring": inputMonitoring])
    }
    func finishRecording(message: String? = nil, automaticPunch: Bool = false) {
        _ = remoteRoute("transport.finishRecording")
    }
    func previewEffect() {
        _ = remoteRoute(
            "effects.preview", numbers: ["amount": Double(amount)], strings: ["kind": effect.rawValue],
            payload: try? JSONEncoder().encode(effectParameters))
    }

    /// Starting values for an explicit upgrade; older selection processors keep
    /// their original algorithms until expanded controls are enabled.
    var selectionParametersForEditing: [String: Float] {
        guard let kind = effect.realtimeKind else { return [:] }
        guard effectParameters.isEmpty else { return effectParameters }
        let strength = amount.isFinite ? max(0, min(1, amount)) : kind.defaultAmount
        var parameters = RealtimeEffect(kind: kind, amount: strength).parametersForEditing
        switch kind {
        case .highPass: parameters["frequency"] = 30 * pow(100, strength)
        case .compressor:
            parameters["threshold"] = -6 - strength * 24
            parameters["ratio"] = 4
            parameters["attack"] = 5
            parameters["release"] = 100
            parameters["knee"] = 0
        default: break
        }
        return parameters
    }
    func enableSelectionParameterControls() {
        guard !busy, !isRecording, effect.realtimeKind != nil, effectParameters.isEmpty else { return }
        effectParameters = selectionParametersForEditing
        if isPreviewing { refreshEffectPreview() }
    }
    func setSelectionParameter(_ key: String, value: Float) {
        guard !busy, !isRecording, let parameter = effect.parameters.first(where: { $0.id == key }) else { return }
        if effectParameters.isEmpty { effectParameters = selectionParametersForEditing }
        effectParameters[key] = parameter.clamped(value)
        if isPreviewing { refreshEffectPreview() }
    }
    func applySelectionPreset(_ preset: SavedEffectPreset) {
        guard !busy, !isRecording, effect.realtimeKind == preset.kind else { return }
        effectParameters = preset.legacyAmount == nil ? preset.parameters : [:]
        if let amount = preset.legacyAmount { self.amount = amount }
        if isPreviewing { refreshEffectPreview() }
        focusEditor()
    }
    func applyRealtimePreset(_ preset: SavedEffectPreset, to id: UUID) {
        guard !busy, !isRecording, currentTrack?.realtimeEffects.first(where: { $0.id == id })?.kind == preset.kind
        else { return }
        changeRealtimeEffect(id, name: "Preset \(preset.name)") { effect in
            if let amount = preset.legacyAmount {
                effect.amount = amount
                effect.parameters = [:]
                effect.selectedAutomationParameter = nil
            } else {
                effect.parameters = preset.parameters
                if effect.selectedAutomationParameter == nil {
                    effect.selectedAutomationParameter = effect.kind.parameters.first?.id
                }
            }
        }
        focusEditor()
    }
    func refreshEffectPreview() {
        _ = remoteRoute(
            "effects.preview", numbers: ["amount": Double(amount)], strings: ["kind": effect.rawValue],
            payload: try? JSONEncoder().encode(effectParameters))
    }

    func chooseEffect(_ kind: BuiltInEffect) {
        guard !busy, !isRecording else { return }
        focusEditor()
        endContinuousEdit()
        if isPreviewing { stopTransport() }
        effect = kind
        effectParameters = kind.defaultParameters
    }
    func addRealtimeEffect(_ kind: RealtimeEffectKind) {
        guard let trackID = selectedTrack else { return }
        var effect = RealtimeEffect(kind: kind, parameters: kind.defaultParameters)
        effect.selectedAutomationParameter = kind.parameters.first?.id
        updateTrack(trackID, actionName: "Add real-time effect") { $0.realtimeEffects.append(effect) }
        selectedRealtimeEffect = effect.id
        automationTarget = .effect
        focusEditor()
    }
    func changeRealtimeEffect(
        _ id: UUID, name: String, coalescingKey: String? = nil, _ change: (inout RealtimeEffect) -> Void
    ) {
        guard let trackID = selectedTrack else { return }
        let writing = automationWrites && isPlaying && !previewing
        let previousFrame = coalescingKey.flatMap { isContinuingEdit($0) ? mixWriteFrames[$0] : nil }
        updateTrack(trackID, actionName: name, coalescingKey: coalescingKey) { track in
            guard let index = track.realtimeEffects.firstIndex(where: { $0.id == id }) else { return }
            let before = track.realtimeEffects[index]
            change(&track.realtimeEffects[index])
            if before.amount != track.realtimeEffects[index].amount {
                track.realtimeEffects[index].parameters = [:]
                track.realtimeEffects[index].selectedAutomationParameter = nil
            }
            if writing {
                if before.amount != track.realtimeEffects[index].amount {
                    prepareAutomationTouch(
                        key: coalescingKey ?? "\(before.id).amount", trackID: trackID, effectID: before.id,
                        parameter: "legacy", points: before.automation, baseline: before.amount)
                    if let touch = writeAutomationTouch(
                        key: coalescingKey ?? "\(before.id).amount", value: track.realtimeEffects[index].amount)
                    {
                        track.realtimeEffects[index].automation = touch
                    } else {
                        var curve = track.realtimeEffects[index].automation
                        if !curve.contains(where: { $0.frame == 0 }) {
                            curve.append(.init(frame: 0, value: before.value(at: 0)))
                        }
                        if let previousFrame, previousFrame <= playhead {
                            curve.removeAll { $0.frame > previousFrame && $0.frame < playhead }
                        } else if playhead > 0 {
                            curve = AutomationCurveEditing.insertingBoundary(
                                curve, at: playhead - 1, baseline: before.amount)
                        }
                        curve.removeAll { $0.frame == playhead }
                        curve.append(.init(frame: playhead, value: track.realtimeEffects[index].amount))
                        track.realtimeEffects[index].automation = curve.sorted { $0.frame < $1.frame }
                    }
                } else {
                    for descriptor in before.kind.parameters {
                        let key = descriptor.id
                        let value = track.realtimeEffects[index].parameters[key] ?? descriptor.defaultValue
                        guard value != (before.parametersForEditing[key] ?? descriptor.defaultValue) else { continue }
                        prepareAutomationTouch(
                            key: coalescingKey ?? "\(before.id).\(key)", trackID: trackID, effectID: before.id,
                            parameter: key, points: before.parameterAutomation[key] ?? [],
                            baseline: descriptor.normalized(before.parametersForEditing[key] ?? descriptor.defaultValue)
                        )
                        if let touch = writeAutomationTouch(
                            key: coalescingKey ?? "\(before.id).\(key)", value: descriptor.normalized(value))
                        {
                            track.realtimeEffects[index].parameterAutomation[key] = touch
                        } else {
                            var curve = track.realtimeEffects[index].parameterAutomation[key] ?? []
                            if !curve.contains(where: { $0.frame == 0 }) {
                                curve.append(AutomationPoint(frame: 0, value: before.value(at: 0, parameter: key)))
                            }
                            if let previousFrame, previousFrame <= playhead {
                                curve.removeAll { $0.frame > previousFrame && $0.frame < playhead }
                            } else if playhead > 0 {
                                curve = AutomationCurveEditing.insertingBoundary(
                                    curve, at: playhead - 1,
                                    baseline: descriptor.normalized(
                                        before.parametersForEditing[key] ?? descriptor.defaultValue))
                            }
                            curve.removeAll { $0.frame == playhead }
                            curve.append(AutomationPoint(frame: playhead, value: descriptor.normalized(value)))
                            track.realtimeEffects[index].parameterAutomation[key] = curve.sorted { $0.frame < $1.frame }
                        }
                    }
                }
            }
        }
        if let key = coalescingKey { mixWriteFrames[key] = writing ? playhead : nil }
    }
    func removeRealtimeEffect(_ id: UUID) {
        guard let trackID = selectedTrack else { return }
        updateTrack(trackID, actionName: "Remove real-time effect") { $0.realtimeEffects.removeAll { $0.id == id } }
        if selectedRealtimeEffect == id { selectedRealtimeEffect = nil }
        focusEditor()
    }
    func putAutomationPoint(effectID: UUID, frame: Int, value: Float, replacing originalFrame: Int? = nil) {
        let frame = max(0, frame)
        let value = min(1, max(0, value))
        changeRealtimeEffect(effectID, name: "Automation point") { effect in
            effect.activeAutomation.removeAll { $0.frame == frame || $0.frame == originalFrame }
            effect.activeAutomation.append(AutomationPoint(frame: frame, value: value))
            effect.activeAutomation.sort { $0.frame < $1.frame }
        }
        focusEditor()
    }
    func deleteAutomationPoint(effectID: UUID, frame: Int) {
        changeRealtimeEffect(effectID, name: "Delete automation point") {
            $0.activeAutomation.removeAll { $0.frame == frame }
        }
        focusEditor()
    }
    func requestSoundDesigner(editClip: Bool = false) {
        guard !busy, !isRecording else { return }
        if let inlineSound, inlineSound.unsettled {
            if editClip, currentClip?.id == inlineSound.clipID, inlineSound.failure == nil {
                inlineSound.openDesigner()
            } else {
                status = "Finish or reload the current inline sound draft before opening a designer."
            }
            return
        }
        if inlineDrums?.unsettled == true {
            status = "Finish or reload the current beat draft before opening a designer."
            return
        }
        endContinuousEdit()
        let clip = editClip ? currentClip : nil
        let trackID =
            selectedTrack == Session.masterTrackID || currentTrack?.acceptsClips == false ? nil : selectedTrack
        let source = remoteGUI.map {
            RemoteWorkspaceSource(remote: $0, trackID: trackID, clipID: clip?.id, frame: playhead)
        }
        soundDesignCommitID = nil
        soundDesignCommitPending = false
        soundDesignCommitMessage = ""
        soundDesignDraftVersion = 0
        if clip?.id == inlineSound?.clipID { inlineSoundExpanded = false }
        var pattern = clip?.editableSoundDesign ?? SoundDesignPattern(bpm: max(30, session.timing.tempo))
        if clip == nil { pattern.beats = session.timing.quarterNotesPerBar }
        soundDesignRequest = SoundDesignRequest(
            clipID: clip?.id, trackID: trackID, pattern: pattern, frame: playhead, source: source)
    }
    func stopSoundDesignPreview() { _ = remoteRoute("designer.stop") }
    func previewSoundDesign(_ pattern: SoundDesignPattern) {
        _ = remoteRoute("designer.preview", payload: try? JSONEncoder().encode(pattern))
    }
    func commitSoundDesign(_ pattern: SoundDesignPattern, request: SoundDesignRequest) {
        guard !soundDesignCommitPending, !pattern.notes.isEmpty, soundDesignRequest?.id == request.id else { return }
        guard let remote = remoteGUI, let source = request.source else {
            soundDesignCommitMessage = "Connect and reopen the source before inserting. Your draft is retained."
            return
        }
        guard let payload = try? JSONEncoder().encode(pattern) else {
            soundDesignCommitMessage = "The sound could not be encoded. Your draft is retained."
            return
        }
        let token = UUID()
        let draftVersion = soundDesignDraftVersion
        soundDesignCommitID = token
        soundDesignCommitPending = true
        soundDesignCommitMessage = "Creating sound on the server…"
        var strings: [String: String] = [:]
        if let id = request.trackID { strings["trackID"] = id.uuidString }
        if let id = request.clipID { strings["clipID"] = id.uuidString }
        remote.commitWorkspace(
            .init(
                operation: "designer.commit", context: source.context, integers: ["frame": request.frame],
                strings: strings, payload: payload), source: source
        ) { [weak self, weak remote] reply in
            guard let self, soundDesignCommitID == token, soundDesignRequest?.id == request.id else { return }
            soundDesignCommitPending = false
            soundDesignCommitID = nil
            guard reply.accepted, let remote else {
                soundDesignCommitMessage = reply.message
                return
            }
            if soundDesignDraftVersion == draftVersion {
                soundDesignRequest = nil
                soundDesignCommitMessage = ""
                status = "Sound design inserted"
                return
            }
            // A render never dismisses private edits made after submission.
            if let data = reply.payload, let result = try? JSONDecoder().decode(RemoteEditorContext.self, from: data),
                let trackID = result.trackID, remote.trackRevisions[trackID] == reply.revision
            {
                let next = RemoteWorkspaceSource(
                    remote: remote, trackID: trackID, clipID: result.clipID, frame: result.cursorFrame)
                soundDesignRequest = .init(
                    id: request.id, clipID: result.clipID, trackID: trackID, pattern: pattern,
                    frame: result.cursorFrame, source: next)
            }
            soundDesignCommitMessage = "The earlier sound was inserted. Your newer private draft remains open."
        }
    }
    func closeSoundDesigner() {
        soundDesignCommitID = nil
        soundDesignCommitPending = false
        if busy { cancelBackgroundWork() }
        stopSoundDesignPreview()
        if let request = soundDesignRequest { playhead = request.frame }
        soundDesignRequest = nil
        focusEditor()
        status = "Ready"
    }
    func insertSoundDesignOnNewTrack(_ pattern: SoundDesignPattern, request: SoundDesignRequest) {
        guard !soundDesignCommitPending, soundDesignRequest?.id == request.id, let remote = remoteGUI else { return }
        let source = RemoteWorkspaceSource(remote: remote, trackID: nil, clipID: nil, frame: request.frame)
        let detached = SoundDesignRequest(
            id: request.id, clipID: nil, trackID: nil, pattern: pattern, frame: request.frame, source: source)
        soundDesignRequest = detached
        commitSoundDesign(pattern, request: detached)
    }

    func applyEffect() {
        _ = remoteRoute(
            "effects.apply", numbers: ["amount": Double(amount)], strings: ["kind": effect.rawValue],
            flags: ["tail": includeEffectTail], payload: try? JSONEncoder().encode(effectParameters))
    }
    func importAudio(urls: [URL]? = nil) { remoteGUI?.importAudio(urls: urls) }
    func confirmAbandon() -> Bool {
        if remoteGUI?.isSavingProject == true {
            status = "Wait for the project save to finish before closing."
            return false
        }
        return true
    }
    func newSession() { _ = remoteRoute("project.new") }
    @discardableResult func save(as: Bool = false) -> Bool {
        remoteGUI?.saveProject(saveAs: `as`)
        return remoteGUI?.connected == true
    }
    func openSession(url: URL? = nil) { remoteGUI?.openProject(url: url) }
    func exportMix() { remoteGUI?.exportMix() }
}

func timeString(_ frames: Int) -> String {
    let seconds = Double(max(0, frames)) / sessionRate
    return String(format: "%02d:%05.2f", Int(seconds) / 60, seconds.truncatingRemainder(dividingBy: 60))
}

private extension Double {
    func nonZero(or fallback: Double) -> Double { isFinite && self != 0 ? self : fallback }
}

extension SessionStore {
    func confirmAbandonAsync() async -> Bool { confirmAbandon() }
    func requestNew() { _ = remoteRoute("project.new") }
    func rememberPreferences() {
        UserDefaults.standard.set(zoom, forKey: "timelineZoom")
        UserDefaults.standard.set(exportFormat, forKey: "exportFormat")
        UserDefaults.standard.set(recordingOffsetMS, forKey: "recordingOffsetMS")
    }
    /// Open Recent entries: local project folders, and (with an audio server)
    /// projects saved on the server, stored as "server:<name>".
    enum RecentProject: Equatable {
        case file(URL)
        case server(String)
        var key: String {
            switch self {
            case .file(let url): url.path
            case .server(let name): "server:" + name
            }
        }
        init(key: String) {
            self = key.hasPrefix("server:") ? .server(String(key.dropFirst(7))) : .file(URL(fileURLWithPath: key))
        }
    }
    var recentProjectEntries: [RecentProject] {
        (UserDefaults.standard.stringArray(forKey: "recentProjects") ?? []).map(RecentProject.init(key:)).filter {
            if case .file(let url) = $0 { return FileManager.default.fileExists(atPath: url.path) }
            return true
        }
    }
    var recentProjects: [URL] { recentProjectEntries.compactMap { if case .file(let url) = $0 { url } else { nil } } }
    func rememberRecent(_ entry: RecentProject) {
        var keys = (UserDefaults.standard.stringArray(forKey: "recentProjects") ?? []).filter { $0 != entry.key }
        keys.insert(entry.key, at: 0)
        UserDefaults.standard.set(Array(keys.prefix(12)), forKey: "recentProjects")
    }
    func rememberSavedProject(_ url: URL) {
        rememberRecent(.file(url))
        presentation?.rememberSavedProject(url)
    }
    func chooseProjectSaveDestination() -> URL? { presentation?.chooseProjectToSave(name: session.name) }
    func requestSave(as saveAs: Bool = false) { remoteGUI?.saveProject(saveAs: saveAs) }
    func requestOpen(url: URL? = nil) { remoteGUI?.openProject(url: url) }
    func cancelBackgroundWork() { _ = remoteRoute("jobs.cancel") }

    func prepareToQuit() async {
        rememberPreferences()
        remoteGUI?.disconnect()
    }
}

@MainActor extension SessionStore {
    func clientDisplayTransport(_ frame: Int) {
        playbackPosition.update(frame)
        if frame > session.end {
            if frame > transportExtent { transportExtent = frame + Int(sessionRate * 5) }
        } else if transportExtent != 0 {
            transportExtent = 0
        }
    }
}
