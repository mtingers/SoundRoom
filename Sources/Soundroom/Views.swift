import SwiftUI
import AppKit
import SoundroomCore

enum Theme {
    static let background = Color(red: 0.075, green: 0.085, blue: 0.105)
    static let paper = Color(red: 0.115, green: 0.13, blue: 0.15)
    static let raised = Color(red: 0.16, green: 0.18, blue: 0.20)
    static let ink = Color(red: 0.91, green: 0.94, blue: 0.94)
    static let secondary = Color(red: 0.61, green: 0.66, blue: 0.69)
    /// Blue identifies interaction/selection. Signal and transport state use
    /// their own colors, paired with labels, shapes and accessibility values.
    static let accent = Color(red: 0.48, green: 0.70, blue: 1.0)
    static let playback = Color(red: 0.38, green: 0.82, blue: 0.57)
    static let success = playback
    static let loop = Color(red: 0.98, green: 0.75, blue: 0.35)
    static let warning = Color(red: 0.98, green: 0.70, blue: 0.32)
    static let recording = Color(red: 0.98, green: 0.38, blue: 0.37)
    static let automation = Color(red: 0.78, green: 0.65, blue: 1.0)
    static let line = Color(red: 0.23, green: 0.26, blue: 0.29)
    /// Layered surfaces so neighbouring sections read as separate places:
    /// a sunken well for scrolling controls, raised cards inside it, a
    /// stronger edge between sections, and a neutral action bar.
    static let well = Color(red: 0.082, green: 0.093, blue: 0.112)
    static let card = Color(red: 0.148, green: 0.166, blue: 0.188)
    static let edge = Color(red: 0.29, green: 0.33, blue: 0.36)
    static let actionBar = Color(red: 0.13, green: 0.15, blue: 0.18)
    static func sectionTitle(_ text: String) -> some View {
        Text(text.uppercased()).font(.system(size: 11, weight: .semibold)).tracking(0.7).foregroundStyle(secondary)
    }
    static let tracks = [Color(red: 0.45, green: 0.81, blue: 0.67), Color(red: 0.48, green: 0.69, blue: 0.93), Color(red: 0.94, green: 0.64, blue: 0.44), Color(red: 0.76, green: 0.60, blue: 0.91), Color(red: 0.83, green: 0.78, blue: 0.45)]
    static func track(_ color: Int) -> Color { tracks[abs(color % tracks.count)] }
}

struct RoomButtonStyle: ButtonStyle {
    var prominent = false
    @Environment(\.isEnabled) private var enabled
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .foregroundStyle(prominent ? Theme.background : Theme.ink)
            .padding(.horizontal, 12).padding(.vertical, 9)
            .background(prominent ? Theme.accent : configuration.isPressed ? Theme.line : Theme.raised, in: RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6).stroke(prominent ? Color.clear : Theme.line, lineWidth: 1))
            .contentShape(Rectangle()).opacity(enabled ? (configuration.isPressed ? 0.8 : 1) : 0.35)
    }
}

struct ActionButton: View {
    var label: String
    var icon: String
    var action: () -> Void
    var body: some View {
        Button(action: action) { Label(label, systemImage: icon).font(.system(size: 12, weight: .medium)) }
            .buttonStyle(RoomButtonStyle())
    }
}

struct MainView: View {
    @ObservedObject var store: SessionStore
    private var drawerWidth: Double { store.effectsDrawerWidth }
    @State private var resizeOrigin: Double?
    var body: some View {
        VStack(spacing: 0) {
            transport
            editBar
            GeometryReader { geometry in
                let width = EffectDrawerLayout.width(preferred: drawerWidth, available: geometry.size.width)
                HStack(spacing: 0) {
                    timeline
                    if store.showEffects {
                        drawerHandle(width: width, available: geometry.size.width)
                        EffectInspector(store: store, isExpanded: width >= 400,
                            onExpand: geometry.size.width >= 1026 ? { store.effectsDrawerWidth = width >= 400 ? 320 : 420; store.focusEditor() } : nil,
                            onClose: { store.showEffects = false; store.focusEditor() })
                            .frame(width: width)
                    }
                }
            }
            if store.loopSelection && store.loopRange != nil { LoopRangeControls(store: store) }
            SelectionPrecisionBar(store: store)
            WorkspaceStatusBar(store: store)
        }
        .foregroundStyle(Theme.ink).background(Theme.background)
        .tint(Theme.accent).preferredColorScheme(.dark)
        .frame(minWidth: 950, minHeight: 620)
        .sheet(isPresented: Binding(get: { store.remoteGUI?.showConnection ?? false }, set: { store.remoteGUI?.showConnection = $0 })) {
            if let remote = store.remoteGUI { RemoteConnectionSheet(remote: remote) }
        }
        .sheet(isPresented: Binding(get: { store.remoteGUI?.showProjectPicker ?? false }, set: { store.remoteGUI?.showProjectPicker = $0 })) {
            if let remote = store.remoteGUI { RemoteProjectPicker(remote: remote) }
        }
        .sheet(isPresented: $store.showDevices) { DeviceSheet(store: store) }
        .sheet(isPresented: $store.showRecordingSetup) { RecordingSetupView(store: store) }
        .sheet(isPresented: $store.showTakeBrowser) { TakeBrowserView(store: store) }
        .sheet(isPresented: $store.showExport) { ExportSheet(store: store) }
        .sheet(isPresented: $store.showCommandSearch, onDismiss: { store.focusEditor() }) { CommandSearchView(store: store) }
        .sheet(item: $store.renameRequest) { request in RenameTrackSheet(store: store, request: request) }
        .sheet(item: $store.sessionRenameDraft) { draft in SessionRenameSheet(store: store, draft: draft) }
        .sheet(item: $store.sourceSeparationRequest) { request in SourceSeparationSheet(store: store, request: request) }
        .sheet(item: $store.vocalEnhancementRequest) { request in VocalEnhancementSheet(store: store, request: request) }
        .sheet(item: $store.soundDesignRequest) { request in
            SoundDesignerSheet(pattern: request.pattern, isEditing: request.clipID != nil, remotePerformance: store.remoteDesignerPerformance, outputDevice: store.outputDevice, previewReady: { store.designerPreviewing }, previewPosition: { store.designerPreviewBeat },
                onPreview: { if store.soundDesignRequest?.id == request.id { store.previewSoundDesign($0) } },
                onStop: { if store.soundDesignRequest == nil || store.soundDesignRequest?.id == request.id { store.stopSoundDesignPreview() } },
                onCommit: { if let current = store.soundDesignRequest, current.id == request.id { store.commitSoundDesign($0, request: current) } },
                onCancel: { if store.soundDesignRequest?.id == request.id { store.closeSoundDesigner() } },
                commitPending: store.soundDesignCommitPending, commitMessage: store.soundDesignCommitMessage,
                onDraftChanged: { if store.soundDesignRequest?.id == request.id { store.soundDesignDraftVersion &+= 1 } },
                onInsertNewTrack: { if let current = store.soundDesignRequest, current.id == request.id { store.insertSoundDesignOnNewTrack($0, request: current) } })
                .onDisappear { if store.soundDesignRequest == nil || store.soundDesignRequest?.id == request.id { store.stopSoundDesignPreview(); store.focusEditor() } }
        }
        .sheet(item: $store.drumDesignRequest) { request in
            DrumDesignerSheet(pattern: request.pattern, controller: store.drumDesigner, isEditing: request.clipID != nil,
                onCommit: { if let current = store.drumDesignRequest, current.id == request.id { store.drumDesigner.commit($0, request: current) } },
                onInsertNewTrack: { if let current = store.drumDesignRequest, current.id == request.id { store.drumDesigner.insertOnNewTrack($0, request: current) } },
                isCurrentWorkspace: { store.drumDesignRequest == nil || store.drumDesignRequest?.id == request.id })
                .onDisappear { if store.drumDesignRequest == nil || store.drumDesignRequest?.id == request.id { store.drumDesigner.stop(); store.focusEditor() } }
        }
        .onDrop(of: [.fileURL], isTargeted: nil) { providers in
            guard !store.busy, !store.isRecording else { return false }
            let group = DispatchGroup(); let lock = NSLock(); var urls: [URL] = []
            for provider in providers {
                group.enter()
                _ = provider.loadObject(ofClass: URL.self) { url, _ in
                    if let url { lock.lock(); urls.append(url); lock.unlock() }; group.leave()
                }
            }
            group.notify(queue: .main) { if !urls.isEmpty { store.importAudio(urls: urls) } }
            return true
        }
    }
    private var transport: some View {
        GeometryReader { geometry in
            transportRow(compact: geometry.size.width < 1120)
                .frame(width: geometry.size.width, height: geometry.size.height)
        }.frame(height: 36)
            .padding(.horizontal, 16).padding(.vertical, 9).background(Theme.paper)
            .overlay(alignment: .bottom) { Theme.line.frame(height: 1) }
    }
    private func transportRow(compact: Bool) -> some View {
        HStack(spacing: compact ? 12 : 16) {
            HStack(spacing: 6) {
                Image(systemName: "waveform").font(.system(size: 18, weight: .medium)).foregroundStyle(Theme.accent)
                Button { store.requestSessionRename() } label: {
                    HStack(spacing: 4) {
                        Text(store.session.name).font(.system(size: 12, weight: .medium)).lineLimit(1)
                        Image(systemName: "pencil").font(.system(size: 9)).foregroundStyle(Theme.secondary)
                    }
                }.buttonStyle(.plain).disabled(!store.canRenameSession)
                    .help("Rename session · \(store.session.name)").accessibilityLabel("Rename session: \(store.session.name)")
            }.frame(minWidth: 80, maxWidth: 150, alignment: .leading)
            Spacer(minLength: 8)
            HStack(spacing: compact ? 7 : 9) {
                Button { store.returnToStart() } label: { Image(systemName: "backward.end.fill").font(.system(size: 16)).frame(width: 24, height: 32) }.buttonStyle(.plain).help("Return to start (Return)").accessibilityLabel("Return to start")
                    .disabled(store.busy || store.isRecording)
                Button { store.togglePlay() } label: {
                    Image(systemName: store.isPlaying ? "pause.fill" : "play.fill").font(.system(size: 19)).frame(width: 40, height: 32)
                        .foregroundStyle(store.isPlaying ? Theme.background : Theme.ink)
                        .background(store.isPlaying ? Theme.playback : Theme.raised, in: RoundedRectangle(cornerRadius: 9))
                        .overlay(RoundedRectangle(cornerRadius: 9).stroke(store.isPlaying ? Theme.playback : Theme.edge))
                }.buttonStyle(.plain).help("Play / pause (Space)").accessibilityLabel(store.isPlaying ? "Pause" : "Play").disabled(!store.canToggleTransport)
                Button { store.record() } label: {
                    Image(systemName: store.recordingInProgress ? "stop.fill" : "circle.fill").font(.system(size: 19)).foregroundStyle(Theme.recording)
                        .frame(width: 34, height: 32).background(store.recordingInProgress ? Theme.recording.opacity(0.12) : Theme.paper, in: RoundedRectangle(cornerRadius: 9))
                        .overlay(RoundedRectangle(cornerRadius: 9).stroke(Theme.line, lineWidth: 1))
                }.buttonStyle(.plain).help("Record / finish recording (R)").accessibilityLabel(store.recordingInProgress ? "Finish recording" : "Record").disabled(!store.canToggleRecording)
                Button { store.showRecordingSetup = true } label: { Image(systemName: "slider.horizontal.3") }.buttonStyle(.plain).help(store.recordingSetupSummary).accessibilityLabel("Recording setup").disabled(store.busy || store.isRecording)
                TransportPositionReadout(position: store.playbackPosition, playing: store.isPlaying, recording: store.isRecording, timing: store.session.timing, settings: store.timelineEditing)
                OutputLevelReadout(level: store.playbackLevel, compact: compact)
                ProjectTempoControls(store: store)
                Button { store.loopSelection.toggle() } label: { Image(systemName: "repeat").foregroundStyle(store.loopSelection ? Theme.loop : Theme.secondary) }
                    .buttonStyle(RoomButtonStyle()).help("Loop selected clips or range (L). Shift-click clip names to select several.").accessibilityLabel("Loop selection").accessibilityValue(store.loopSelection ? "On" : "Off").disabled(!store.canToggleLoop || store.isRecording || store.busy)
                    .overlay(RoundedRectangle(cornerRadius: 6).stroke(store.loopSelection ? Theme.loop : Color.clear))
            }.fixedSize()
            Spacer(minLength: 8)
            HStack(spacing: 9) {
                if !compact { Button { store.refreshDevices(); store.showDevices = true } label: {
                    HStack(spacing: 5) {
                        Image(systemName: "mic"); Text(store.inputName).lineLimit(1).frame(maxWidth: 88)
                        Image(systemName: "arrow.right").font(.system(size: 8))
                        Image(systemName: "speaker.wave.2"); Text(store.outputName).lineLimit(1).frame(maxWidth: 88)
                    }.font(.system(size: 11)).foregroundStyle(Theme.secondary)
                    Image(systemName: "chevron.down").font(.system(size: 9)).padding(.leading, 8)
                }.buttonStyle(.plain).frame(maxWidth: 230, alignment: .trailing).disabled(store.isPlaying || store.busy).help("Choose input and output devices")
                }
                if compact {
                    Button { store.refreshDevices(); store.showDevices = true } label: { Image(systemName: "speaker.wave.2").frame(width: 26, height: 30) }.buttonStyle(.plain).disabled(store.isPlaying || store.busy).help("Audio connections").accessibilityLabel("Audio connections")
                }
                ActionButton(label: "Save", icon: "square.and.arrow.down") { store.requestSave() }.disabled(store.busy || store.remoteGUI?.isSavingProject == true).help(store.remoteGUI?.projectSaveHelp ?? "Save this project")
                ActionButton(label: "Export", icon: "square.and.arrow.up") { store.showExport = true }.disabled(store.session.end == 0 || store.busy || store.isRecording)
            }.fixedSize()
        }
    }
    private func drawerHandle(width: CGFloat, available: CGFloat) -> some View {
        Rectangle().fill(Theme.edge).frame(width: EffectDrawerLayout.dividerWidth)
            .overlay { Capsule().fill(Theme.secondary.opacity(0.5)).frame(width: 2, height: 28) }
            .contentShape(Rectangle())
            .gesture(DragGesture(coordinateSpace: .global).onChanged { drag in
                if resizeOrigin == nil { resizeOrigin = Double(width) }
                store.effectsDrawerWidth = Double(EffectDrawerLayout.width(preferred: (resizeOrigin ?? Double(width)) - drag.translation.width, available: available))
            }.onEnded { _ in resizeOrigin = nil; store.focusEditor() })
            .accessibilityLabel("Effects drawer width")
            .accessibilityValue("\(Int(width)) points")
            .accessibilityAdjustableAction { direction in
                store.effectsDrawerWidth = Double(EffectDrawerLayout.width(preferred: Double(width) + (direction == .increment ? 20 : -20), available: available))
            }.help("Drag to resize the effects drawer")
    }
    private var editBar: some View {
        GeometryReader { geometry in
            editingToolbar(compact: geometry.size.width < 1200)
                .frame(width: geometry.size.width, height: geometry.size.height)
        }.frame(height: 38).padding(.horizontal, 16).padding(.vertical, 6).disabled(store.busy || store.isRecording)
    }
    private func editingToolbar(compact: Bool) -> some View {
        HStack(spacing: compact ? 6 : 8) {
            HStack(spacing: compact ? 6 : 8) {
                Menu {
                    Button("Audio track") { store.addTrack() }
                    Button("Group bus") { store.addMixTrack(role: "bus") }
                    Button("Effect return") { store.addMixTrack(role: "return") }
                } label: { Label("Track", systemImage: "plus") }.menuStyle(.borderlessButton).fixedSize()
                ActionButton(label: "Import", icon: "arrow.down.doc") { store.importAudio() }
                if !compact {
                    ActionButton(label: "Designer", icon: "pianokeys") { store.requestSoundDesigner() }.help("Create notes with a built-in sound designer")
                    ActionButton(label: "Beats", icon: "square.grid.3x3") { store.requestDrumDesigner() }.help("Drum loop designer · ⇧⌘B")
                }
            }.fixedSize()
            Spacer(minLength: 12)
            HStack(spacing: compact ? 6 : 8) {
                Button { store.undo() } label: { Image(systemName: "arrow.uturn.backward").font(.system(size: 12)).frame(width: 14) }.buttonStyle(RoomButtonStyle()).disabled(!store.canUndo).help("\(store.undoTitle) (⌘Z)").accessibilityLabel(store.undoTitle)
                Button { store.redo() } label: { Image(systemName: "arrow.uturn.forward").font(.system(size: 12)).frame(width: 14) }.buttonStyle(RoomButtonStyle()).disabled(!store.canRedo).help("\(store.redoTitle) (⇧⌘Z)").accessibilityLabel(store.redoTitle)
                ActionButton(label: "Split", icon: "scissors") { store.split() }.help("Split at playhead (⌘E)")
                ActionButton(label: "Combine", icon: "rectangle.compress.vertical") { store.combineClips() }.disabled(!store.canCombineClips).help("Combine selected clips on each track (⇧⌘J). Shift-click clip names to select several.")
                if compact {
                    Menu {
                        Button("Sound designer…") { store.requestSoundDesigner() }
                        Button("Beat designer…") { store.requestDrumDesigner() }
                        Divider()
                        Button("Trim to selection") { store.trim() }.disabled(!store.hasAudioSelection || store.selection == nil)
                        Button("Align to playhead") { store.alignToPlayhead() }.disabled(store.currentClip == nil)
                    } label: { Label("More", systemImage: "ellipsis").font(.system(size: 12)) }
                        .menuStyle(.borderlessButton).fixedSize().help("More creation and editing tools")
                } else {
                    ActionButton(label: "Trim", icon: "crop") { store.trim() }.disabled(!store.hasAudioSelection || store.selection == nil).help("Keep only the selected audio on this track (⌘T)")
                    ActionButton(label: "Align", icon: "align.horizontal.left") { store.alignToPlayhead() }.disabled(store.currentClip == nil).help("Align selected clip start to playhead")
                }
            }.fixedSize()
            Spacer(minLength: 12)
            HStack(spacing: compact ? 6 : 8) {
                Button { store.snap.toggle() } label: {
                    Label("Snap", systemImage: "arrow.right.to.line").font(.system(size: 11)).foregroundStyle(store.snap ? Theme.accent : Theme.secondary)
                }.buttonStyle(RoomButtonStyle()).help("Snap moved clips to the playhead and clip edges").accessibilityValue(store.snap ? "On" : "Off")
                Button {
                    store.requestTimelineZoom(1 / 1.5); store.focusEditor()
                } label: {
                    Image(systemName: "minus.magnifyingglass").font(.system(size: 11)).frame(width: 26, height: 26).contentShape(Rectangle())
                }.buttonStyle(.plain).foregroundStyle(Theme.secondary).disabled(store.zoom <= SessionStore.zoomRange.lowerBound).help("Zoom out (⌘−)").accessibilityLabel("Zoom out")
                // Logarithmic: every part of the slider zooms by the same ratio.
                RoomSlider(value: Binding(get: { log(store.zoom) }, set: { store.zoom = SessionStore.clampedZoom(exp($0)) }),
                           in: log(SessionStore.zoomRange.lowerBound)...log(SessionStore.zoomRange.upperBound), label: "Timeline zoom").frame(width: 82).help("Timeline zoom")
                Button {
                    store.requestTimelineZoom(1.5); store.focusEditor()
                } label: {
                    Image(systemName: "plus.magnifyingglass").font(.system(size: 11)).frame(width: 26, height: 26).contentShape(Rectangle())
                }.buttonStyle(.plain).foregroundStyle(Theme.secondary).disabled(store.zoom >= SessionStore.zoomRange.upperBound).help("Zoom in (⌘=) · pinch or ⌘-scroll to zoom").accessibilityLabel("Zoom in")
                effectsControl.padding(.leading, 6)
            }.fixedSize()
        }
    }
    private var effectsControl: some View {
        HStack(spacing: 0) {
            Button { store.showEffects.toggle(); store.focusEditor() } label: {
                Label("Effects", systemImage: "slider.horizontal.3")
                    .padding(.leading, 10).padding(.trailing, 8).frame(height: 34).contentShape(Rectangle())
            }.buttonStyle(.plain).help("Show or hide effects")
                .accessibilityValue(store.showEffects ? "Open" : "Closed")
            Theme.line.frame(width: 1, height: 18)
            Menu {
                Button(store.showEffects ? "Hide effect controls" : "Show effect controls") { store.showEffects.toggle(); store.focusEditor() }
                Divider()
                Button("Enhance vocals…") { store.requestVocalEnhancement() }.disabled(!store.canUseTrackTools(store.selectedTrack))
                Button("Separate source…") { store.requestSourceSeparation() }.disabled(!store.canSeparateSource)
            } label: {
                Image(systemName: "chevron.down").font(.system(size: 9, weight: .semibold)).frame(width: 26, height: 34).contentShape(Rectangle())
            }.menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
                .help("Effects: vocal enhancement and source separation").accessibilityLabel("Effects actions")
        }.font(.system(size: 12, weight: .medium))
            .background(store.showEffects ? Theme.accent.opacity(0.12) : Theme.raised, in: RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6).stroke(Theme.line))
    }
    private var timeline: some View { TimelineView(store: store) }

}

struct TrackControls: View {
    @ObservedObject var store: SessionStore
    let track: Track
    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            HStack(spacing: 7) {
                RoundedRectangle(cornerRadius: 2).fill(Theme.track(track.color)).frame(width: 4, height: 17)
                Button { store.choose(track: track.id) } label: { Text(track.name).font(.system(size: 12, weight: .semibold)).lineLimit(1).frame(maxWidth: .infinity, alignment: .leading).contentShape(Rectangle()) }.buttonStyle(.plain).help(track.name)
                    .accessibilityLabel("Select \(track.name) track").accessibilityAddTraits(store.selectedTrack == track.id ? .isSelected : [])
                TrackRoutingButton(store: store, track: track)
                Button { store.requestRename(track.id) } label: { Image(systemName: "pencil").font(.system(size: 10)).foregroundStyle(Theme.secondary).frame(width: 18, height: 22).contentShape(Rectangle()) }.buttonStyle(.plain).help("Rename track").accessibilityLabel("Rename \(track.name)")
                if track.routing?.role == nil || track.routing?.role == "audio" { Button { store.updateTrack(track.id, actionName: "Arm track") { $0.armed.toggle() } } label: { Image(systemName: track.armed ? "record.circle.fill" : "record.circle").font(.system(size: 15)).foregroundStyle(track.armed ? Theme.recording : Theme.secondary).frame(width: 24, height: 24).contentShape(Rectangle()) }.buttonStyle(.plain).help("Arm this track for recording").accessibilityLabel("Record arm \(track.name)").accessibilityValue(track.armed ? "Armed" : "Off") }
            }
            HStack(spacing: 6) {
                smallToggle("M", active: track.muted, help: "Mute", color: Theme.warning) { store.updateTrack(track.id, actionName: "Mute") { $0.muted.toggle() } }
                smallToggle("S", active: track.solo, help: "Solo", color: Theme.loop) { store.updateTrack(track.id, actionName: "Solo") { $0.solo.toggle() } }
                TrackAutomationToggle(store: store, trackID: track.id)
                TrackMixControl(store: store, track: track, parameter: .volume)
            }
            if track.armed && store.isRecording {
                RecordingInputMeter(activity: store.inputActivity, channels: store.recordingInputChannels(for: track), trackName: track.name)
            }
            HStack(spacing: 6) {
                if track.routing?.role == nil || track.routing?.role == "audio" { TrackInputControls(store: store, track: track) }
                // The readout after the slider (C, L38, R16) labels it as pan.
                TrackMixControl(store: store, track: track, parameter: .pan)
            }
            if !store.isRecording, track.clips.contains(where: { $0.editableDrumPattern != nil }) {
                Button { store.toggleInlineDrums(trackID: track.id) } label: {
                    Label("Edit beats", systemImage: "square.grid.3x3").font(.system(size: 10)).frame(maxWidth: .infinity, alignment: .leading)
                        .foregroundStyle(store.inlineDrumExpanded && store.inlineDrums?.trackID == track.id ? Theme.accent : Theme.secondary)
                }.buttonStyle(.plain).help("Show or hide the inline beat editor").accessibilityLabel("Edit beats in \(track.name)")
            }
            if !store.isRecording, track.clips.contains(where: { $0.editableSoundDesign != nil }) {
                Button { store.toggleInlineSound(trackID: track.id) } label: {
                    Label("Edit notes", systemImage: "pianokeys").font(.system(size: 10)).frame(maxWidth: .infinity, alignment: .leading)
                        .foregroundStyle(store.inlineSoundExpanded && store.inlineSound?.trackID == track.id ? Theme.accent : Theme.secondary)
                }.buttonStyle(.plain).help("Show or hide the inline sound editor").accessibilityLabel("Edit notes in \(track.name)")
            }
        }.padding(.horizontal, 14).frame(maxWidth: .infinity, maxHeight: .infinity)
        // The selected track reads at a glance: a tinted row with an accent
        // edge, and a firm border where the track column meets the timeline.
        .background(store.selectedTrack == track.id ? Theme.accent.opacity(0.13) : Color.clear)
        .overlay(alignment: .leading) { if store.selectedTrack == track.id { Theme.accent.frame(width: 3) } }
        .overlay(alignment: .bottom) { Theme.line.frame(height: 1) }
        .overlay(alignment: .trailing) { Theme.edge.frame(width: 1) }
        .contextMenu {
            Button("Enhance vocals…") { store.requestVocalEnhancement(trackID: track.id) }.disabled(!store.canUseTrackTools(track.id))
            Button("Separate source…") { store.requestSourceSeparation(trackID: track.id, wholeTrack: true) }.disabled(!store.canUseTrackTools(track.id) || track.clips.isEmpty)
            Divider()
            Button("Rename track…") { store.requestRename(track.id) }
            Button("Show takes…") { store.choose(track: track.id); store.showTakeBrowser = true }.disabled(track.recordingTakes.isEmpty)
            Button("Copy track") { store.copyTrack(track.id) }
            Button("Volume automation") { store.showMixAutomation(.volume, trackID: track.id) }
            Button("Pan automation") { store.showMixAutomation(.pan, trackID: track.id) }
            Divider()
            Button("Copy track + automation") { store.copyTrack(track.id, includeAutomation: true) }
            Button("Paste track") { store.pasteTrack(after: track.id) }.disabled(!store.canPasteTrack)
            Divider()
            Button("Delete track", role: .destructive) { store.removeTrack(track.id) }
        }
        .disabled(store.busy || store.isRecording)
    }
    private func smallToggle(_ title: String, active: Bool, help: String, color: Color, action: @escaping () -> Void) -> some View {
        Button(action: action) { Text(title).font(.system(size: 10, weight: .semibold)).frame(width: 24, height: 24).background(active ? color : Theme.raised, in: RoundedRectangle(cornerRadius: 4)).foregroundStyle(active ? Theme.background : Theme.secondary).contentShape(Rectangle()) }
            .buttonStyle(.plain).help("\(help) \(track.name)").accessibilityLabel("\(help) \(track.name)")
            .accessibilityValue(active ? "On" : "Off").accessibilityAddTraits(active ? .isSelected : [])
    }
}

/// Time-ruler spacing that keeps labels readable at every zoom.
enum TimelineTicks {
    /// The shortest round step (seconds) whose ticks are at least `spacing` points apart.
    static func step(pixelsPerSecond: Double, spacing: Double = 64) -> Double {
        let steps: [Double] = [0.1, 0.2, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 1200]
        return steps.first { $0 * pixelsPerSecond >= spacing } ?? 1800
    }
    /// Unlabeled subdivisions between labeled ticks.
    static func minor(_ step: Double) -> Double { step == 15 || step == 30 ? step / 3 : step == 2 || step == 0.2 ? step / 2 : step / 5 }
    static func label(_ seconds: Double, step: Double) -> String {
        let whole = Int(seconds.rounded(.down))
        if step >= 1 { return String(format: "%d:%02d", whole / 60, whole % 60) }
        return seconds < 60 ? String(format: "%.1f s", seconds) : String(format: "%d:%04.1f", whole / 60, seconds - Double(whole / 60 * 60))
    }
}

struct RulerView: View {
    @ObservedObject var store: SessionStore
    @ObservedObject var settings: TimelineEditingState
    var visiblePixels: Range<Double>?
    init(store: SessionStore, visiblePixels: Range<Double>? = nil) { self.store = store; self.settings = store.timelineEditing; self.visiblePixels = visiblePixels }
    @State private var proposedLoop: Range<Int>?
    @State private var loopDragAnchor: Double?
    @State private var loopHandleAnchor: Double?
    @Environment(\.timelineScrollInteraction) private var scrollInteraction
    private func updateLoopDrag(at x: Double) {
        guard let anchor = loopDragAnchor else { return }
        let start = store.snappedFrame(Int(anchor / store.zoom * sessionRate), useSelection: false).frame
        let end = store.snappedFrame(Int(max(0, x) / store.zoom * sessionRate), useSelection: false).frame
        proposedLoop = start == end ? nil : min(start, end)..<max(start, end)
    }
    var body: some View {
        Canvas { context, size in
            if let range = proposedLoop ?? (store.loopSelection ? store.loopRange : nil) {
                let x = Double(range.lowerBound) / sessionRate * store.zoom
                let width = Double(range.count) / sessionRate * store.zoom
                context.fill(Path(CGRect(x: x, y: 0, width: width, height: size.height)), with: .color(Theme.loop.opacity(0.14)))
                context.fill(Path(CGRect(x: x, y: 30, width: width, height: 4)), with: .color(Theme.loop))
            }
            let firstPixel = max(0, visiblePixels?.lowerBound ?? 0)
            let lastPixel = min(size.width, visiblePixels?.upperBound ?? size.width)
            if settings.timebase == .beats {
                let start = Int(firstPixel / store.zoom * sessionRate), end = Int(lastPixel / store.zoom * sessionRate)
                for tick in store.session.timing.rulerTicks(start: start, end: end, pixelsPerSecond: store.zoom) {
                    let x = Double(tick.frame) / sessionRate * store.zoom
                    var path = Path(); path.move(to: CGPoint(x: x, y: tick.bar ? 24 : 28)); path.addLine(to: CGPoint(x: x, y: 34))
                    context.stroke(path, with: .color(tick.bar ? Theme.accent.opacity(0.65) : Theme.line), lineWidth: 1)
                    context.draw(Text(tick.label).font(.system(size: 10, design: .monospaced)).foregroundStyle(tick.bar ? Theme.ink : Theme.secondary), at: CGPoint(x: x + 5, y: 14), anchor: .leading)
                }
            } else {
                let step = TimelineTicks.step(pixelsPerSecond: store.zoom), minor = TimelineTicks.minor(step)
                if minor * store.zoom >= 8 {
                    var ticks = Path()
                    for seconds in stride(from: floor(firstPixel / store.zoom / minor) * minor, through: lastPixel / store.zoom, by: minor) {
                        ticks.move(to: CGPoint(x: seconds * store.zoom, y: 31)); ticks.addLine(to: CGPoint(x: seconds * store.zoom, y: 34))
                    }
                    context.stroke(ticks, with: .color(Theme.line), lineWidth: 1)
                }
                let first = floor(firstPixel / store.zoom / step) * step
                for seconds in stride(from: first, through: lastPixel / store.zoom, by: step) {
                    let x = seconds * store.zoom
                    var tick = Path(); tick.move(to: CGPoint(x: x, y: 24)); tick.addLine(to: CGPoint(x: x, y: 34))
                    context.stroke(tick, with: .color(Theme.secondary.opacity(0.6)), lineWidth: 1)
                    context.draw(Text(TimelineTicks.label(seconds, step: step)).font(.system(size: 10, design: .monospaced)).foregroundStyle(Theme.secondary), at: CGPoint(x: x + 4, y: 14), anchor: .leading)
                }
            }

        }.background(Theme.paper).contentShape(Rectangle())
            .coordinateSpace(name: "loopRuler")
            .gesture(DragGesture(minimumDistance: 0).onChanged { value in
                guard !store.busy, !store.isRecording else { return }
                if loopDragAnchor == nil { loopDragAnchor = value.startLocation.x }
                if abs(value.translation.width) > 3 {
                    scrollInteraction?.beginDrag { x in updateLoopDrag(at: x) }
                    if let scrollInteraction { scrollInteraction.updateDragPosition(contentX: value.location.x) }
                    else { updateLoopDrag(at: value.location.x) }
                }
            }.onEnded { value in
                scrollInteraction?.endDrag()
                if let range = proposedLoop { store.setLoopRange(range) }
                else if abs(value.translation.width) <= 3 { store.seek(store.snappedFrame(Int(value.location.x / store.zoom * sessionRate), useSelection: false).frame) }
                proposedLoop = nil; loopDragAnchor = nil; store.focusEditor()
            })
            .onDisappear { scrollInteraction?.endDrag() }
            .overlay(alignment: .topLeading) {
                if store.loopSelection, let range = store.loopRange {
                    loopHandle(range: range, start: true)
                    loopHandle(range: range, start: false)
                }
            }
            .accessibilityLabel("Timeline ruler. Click to seek; drag to set a loop across tracks.")
    }
    private func updateLoopHandle(range: Range<Int>, start: Bool, contentX: Double) {
        guard let anchor = loopHandleAnchor, !store.busy, !store.isRecording else { scrollInteraction?.endDrag(); return }
        let original = start ? range.lowerBound : range.upperBound
        let frame = store.snappedFrame(original + Int((contentX - anchor) / store.zoom * sessionRate), useSelection: false).frame
        proposedLoop = start ? min(frame, range.upperBound - 1)..<range.upperBound : range.lowerBound..<max(range.lowerBound + 1, frame)
    }
    private func loopHandle(range: Range<Int>, start: Bool) -> some View {
        let preview = proposedLoop ?? range
        return Image(systemName: start ? "arrowtriangle.right.fill" : "arrowtriangle.left.fill")
            .font(.system(size: 10)).foregroundStyle(Theme.accent)
            .frame(width: 18, height: 20).contentShape(Rectangle())
            .position(x: max(9, Double(start ? preview.lowerBound : preview.upperBound) / sessionRate * store.zoom), y: 25)
            .highPriorityGesture(DragGesture(minimumDistance: 1, coordinateSpace: .named("loopRuler")).onChanged { value in
                guard !store.busy, !store.isRecording else { return }
                if loopHandleAnchor == nil { loopHandleAnchor = value.startLocation.x }
                scrollInteraction?.beginDrag { x in updateLoopHandle(range: range, start: start, contentX: x) }
                if let scrollInteraction { scrollInteraction.updateDragPosition(contentX: value.location.x) }
                else { updateLoopHandle(range: range, start: start, contentX: value.location.x) }
            }.onEnded { _ in
                scrollInteraction?.endDrag()
                if store.loopRange == range, let proposedLoop { store.setLoopRange(proposedLoop) }
                proposedLoop = nil; loopHandleAnchor = nil; store.focusEditor()
            }).help(start ? "Drag loop start" : "Drag loop end")
            .accessibilityLabel(start ? "Loop start handle" : "Loop end handle")
    }

}


struct LaneView: View {
    @ObservedObject private var waveformCache = WaveformCache.shared
    @ObservedObject private var waveformDisplay: WaveformDisplay
    @ObservedObject var store: SessionStore
    @ObservedObject private var settings: TimelineEditingState
    let track: Track
    var visiblePixels: Range<Double>? = nil
    var viewportPixels: Range<Double>? = nil
    init(store: SessionStore, track: Track, visiblePixels: Range<Double>? = nil, viewportPixels: Range<Double>? = nil) {
        self.store = store; self.settings = store.timelineEditing; self.waveformDisplay = store.waveformDisplay
        self.track = track; self.visiblePixels = visiblePixels; self.viewportPixels = viewportPixels
    }
    @State private var moving: Clip?
    @State private var moveDelta: Int = 0
    @State private var dragStarted = false
    @State private var initialClipSelection: Set<UUID> = []
    @State private var extendingClipSelection = false
    @State private var duplicatingClipDrag = false
    @State private var dragAnchorX: Double = 0
    @Environment(\.timelineScrollInteraction) private var scrollInteraction
    private func updateDrag(at contentX: Double) {
        guard !store.busy, !store.isRecording else { scrollInteraction?.endDrag(); return }
        let delta = contentX - dragAnchorX
        if let clip = moving {
            moveDelta = Int(delta / store.zoom * sessionRate)
            store.clipMovePreview = store.clipMoveProposal(primary: clip.id, to: clip.start + moveDelta)
        } else if abs(delta) > 2 {
            let start = max(0, Int(dragAnchorX / store.zoom * sessionRate))
            let end = max(0, Int(contentX / store.zoom * sessionRate))
            if store.timelineEditing.grid == .beat {
                store.selectTimelineDrag(track: track.id, start: store.snappedFrame(start, useSelection: false).frame, end: store.snappedFrame(end, useSelection: false).frame)
            } else { store.selectTimelineDrag(track: track.id, start: start, end: end) }
        }
    }
    /// Double-click a Designer or Beats clip to edit its part.
    private func openEditorOnDoubleClick(_ clip: Clip) {
        guard !extendingClipSelection, (NSApp.currentEvent?.clickCount ?? 1) == 2 else { return }
        if store.selectedClip != clip.id { store.choose(track: track.id, clip: clip.id) }
        if clip.editableSoundDesign != nil { store.toggleInlineSound(trackID: track.id, clipID: clip.id) }
        else if clip.editableDrumPattern != nil { store.toggleInlineDrums(trackID: track.id, clipID: clip.id) }
    }
    var body: some View {
        Canvas { context, size in
            let lane = AudioLaneGeometry(height: size.height)
            context.fill(Path(CGRect(origin: .zero, size: size)), with: .color(store.selectedTrack == track.id ? Theme.paper.opacity(0.75) : Theme.background))
            var grid = Path()
            let gridPeriod = store.zoom * TimelineTicks.step(pixelsPerSecond: store.zoom)
            let gridStart = floor(max(0, visiblePixels?.lowerBound ?? 0) / gridPeriod) * gridPeriod
            let gridEnd = min(size.width, visiblePixels?.upperBound ?? size.width)
            if settings.timebase == .time {
                for x in stride(from: gridStart, through: gridEnd, by: gridPeriod) { grid.move(to: CGPoint(x: x, y: 0)); grid.addLine(to: CGPoint(x: x, y: size.height)) }
            }
            context.stroke(grid, with: .color(Theme.line.opacity(0.45)), lineWidth: 0.5)
            for clip in track.clips {
                let start = store.clipMovePreview?.ids.contains(clip.id) == true ? clip.start + (store.clipMovePreview?.shift ?? 0) : clip.start
                let x = Double(start) / sessionRate * store.zoom, width = max(2, Double(clip.length) / sessionRate * store.zoom)
                if let visiblePixels, x + width < visiblePixels.lowerBound || x > visiblePixels.upperBound { continue }
                let rect = CGRect(x: x, y: 8, width: width, height: lane.bottom - 8)
                let color = Theme.track(track.color)
                context.fill(Path(roundedRect: rect, cornerRadius: 5), with: .color(color.opacity(track.muted ? 0.08 : store.selectedClips.contains(clip.id) ? 0.28 : 0.15)))
                context.stroke(Path(roundedRect: rect, cornerRadius: 5), with: .color(color.opacity(store.selectedClips.contains(clip.id) ? 1 : 0.45)), lineWidth: store.selectedClips.contains(clip.id) ? 2 : 1)
                var titleContext = context
                let headerLeft = max(x, viewportPixels?.lowerBound ?? x)
                let headerRight = min(x + width, viewportPixels?.upperBound ?? (x + width))
                let headerWidth = max(0, headerRight - headerLeft)
                let channelLabel = AudioChannelPresentation.label(channels: store.assets[clip.assetID]?.channelCount)
                let badgeWidth: Double = channelLabel == "Mono" ? 38 : 46
                let showsBadge = channelLabel != nil && headerWidth >= badgeWidth + 24
                if showsBadge, let channelLabel {
                    let badgeRect = CGRect(x: headerRight - badgeWidth - 6, y: 13, width: badgeWidth, height: 16)
                    context.fill(Path(roundedRect: badgeRect, cornerRadius: 4), with: .color(Theme.background.opacity(0.8)))
                    context.draw(Text(channelLabel).font(.system(size: 9, weight: .semibold)).foregroundStyle(color), at: CGPoint(x: badgeRect.midX, y: badgeRect.midY))
                }
                titleContext.clip(to: Path(CGRect(x: headerLeft + 5, y: 10, width: max(0, headerWidth - 10 - (showsBadge ? badgeWidth + 8 : 0)), height: 23)))
                // Designer and Beats clips carry an editable part: badge them
                // with the toolbar's icons (double-click opens the editor).
                let badge = clip.editableSoundDesign != nil ? "pianokeys" : clip.editableDrumPattern != nil ? "square.grid.3x3" : nil
                let title = badge.map { Text(Image(systemName: $0)).font(.system(size: 10, weight: .semibold)) + Text("  ") + Text(clip.name) } ?? Text(clip.name)
                titleContext.draw(title.font(.system(size: 12, weight: .medium)).foregroundStyle(color), at: CGPoint(x: headerLeft + 8, y: 21), anchor: .leading)
                let firstPixel = max(0, Int(floor((visiblePixels?.lowerBound ?? x) - x)))
                let lastPixel = min(Int(ceil(width)), Int(ceil((visiblePixels?.upperBound ?? (x + width)) - x)))
                let framesPerPixel = sessionRate / store.zoom
                let sourceStart = clip.offset + max(0, min(clip.length, Int(Double(firstPixel) * framesPerPixel)))
                let sourceEnd = clip.offset + max(0, min(clip.length, Int(Double(max(firstPixel, lastPixel)) * framesPerPixel)))
                if lastPixel > firstPixel, sourceEnd > sourceStart, let peaks = store.waveformEnvelope(clip.assetID, range: sourceStart..<sourceEnd, pixels: lastPixel - firstPixel) {
                    ClipWaveformDrawing.draw(context: context, envelope: peaks, source: clip.offset..<(clip.offset + clip.length),
                                             rect: CGRect(x: x, y: 36, width: width, height: lane.bottom - 36), pixels: firstPixel..<lastPixel,
                                             color: color.opacity(track.muted ? 0.3 : 0.9), labelX: headerLeft + 7)
                }
            }
            if let selection = store.selection, selection.trackID == track.id {
                let x = Double(selection.range.lowerBound) / sessionRate * store.zoom
                let width = Double(selection.range.count) / sessionRate * store.zoom
                let rect = CGRect(x: x, y: 0, width: width, height: size.height)
                context.fill(Path(rect), with: .color(Theme.accent.opacity(0.15)))
                context.stroke(Path(rect), with: .color(Theme.accent.opacity(0.8)), lineWidth: 1)
            }
            if store.loopSelection, let range = store.loopRange {
                var boundaries = Path()
                for frame in [range.lowerBound, range.upperBound] {
                    let x = Double(frame) / sessionRate * store.zoom
                    boundaries.move(to: CGPoint(x: x, y: 0)); boundaries.addLine(to: CGPoint(x: x, y: size.height))
                }
                context.stroke(boundaries, with: .color(Theme.loop.opacity(0.85)), style: StrokeStyle(lineWidth: 1, dash: [3, 3]))
            }
            if let guide = store.clipMovePreview?.guide {
                let x = Double(guide) / sessionRate * store.zoom
                var guideLine = Path(); guideLine.move(to: CGPoint(x: x, y: 0)); guideLine.addLine(to: CGPoint(x: x, y: size.height))
                context.stroke(guideLine, with: .color(Theme.accent), style: StrokeStyle(lineWidth: 2, dash: [4, 3]))
            }
            var bottom = Path(); bottom.move(to: CGPoint(x: 0, y: size.height)); bottom.addLine(to: CGPoint(x: size.width, y: size.height)); context.stroke(bottom, with: .color(Theme.line), lineWidth: 1)
        }
        .accessibilityRepresentation { TrackClipAccessibility(store: store, track: track, visiblePixels: visiblePixels) }
        .contentShape(Rectangle())
        .gesture(DragGesture(minimumDistance: 0).onChanged { value in
            guard !store.isRecording, !store.busy else { return }
            let start = max(0, Int(value.startLocation.x / store.zoom * sessionRate))
            if !dragStarted {
                dragStarted = true; dragAnchorX = value.startLocation.x; initialClipSelection = store.selectedClips
                let flags = NSApp.currentEvent?.modifierFlags ?? NSEvent.modifierFlags
                extendingClipSelection = flags.contains(.shift); duplicatingClipDrag = flags.contains(.option)
                let clip = track.clips.last { $0.start <= start && $0.end > start }
                if value.startLocation.y >= 8 && value.startLocation.y < 33, let clip {
                    moving = clip
                    if extendingClipSelection { store.choose(track: track.id, clip: clip.id, extending: true) }
                    else if !store.selectedClips.contains(clip.id) { store.choose(track: track.id, clip: clip.id) }
                } else { moving = nil; store.choose(track: track.id); store.seek(start) }
            }
            if abs(value.translation.width) > 2 {
                scrollInteraction?.beginDrag { contentX in updateDrag(at: contentX) }
            }
            if abs(value.translation.width) > 2, let scrollInteraction { scrollInteraction.updateDragPosition(contentX: value.location.x) }
            else { updateDrag(at: value.location.x) }
        }.onEnded { value in
            scrollInteraction?.endDrag()
            if let clip = moving {
                if abs(value.translation.width) > 2 || abs(value.translation.height) > 40 { store.moveSelectedClips(primary: clip.id, to: clip.start + moveDelta, targetTrack: store.trackForVerticalMove(from: track.id, delta: value.translation.height), duplicate: duplicatingClipDrag) }
                else { openEditorOnDoubleClick(clip) }
            }
            else if abs(value.translation.width) <= 2 {
                let frame = max(0, Int(value.location.x / store.zoom * sessionRate))
                let clip = track.clips.last { $0.start <= frame && $0.end > frame }
                if extendingClipSelection { store.selectedClips = initialClipSelection }
                store.choose(track: track.id, clip: clip?.id, extending: extendingClipSelection); store.seek(frame)
                if let clip { openEditorOnDoubleClick(clip) }
            }
            moving = nil; moveDelta = 0; dragStarted = false; store.clipMovePreview = nil
        })
        .onDisappear { scrollInteraction?.endDrag() }
        .overlay {
            if store.isRecording && track.armed { RecordingLaneOverlay(store: store, track: track) }
        }
        .overlay { ClipHandleLayer(store: store, track: track) }
        .overlay { MusicalLaneGrid(store: store, settings: store.timelineEditing, visiblePixels: visiblePixels).allowsHitTesting(false) }
        .background(ClipContextPointer(store: store, trackID: track.id).allowsHitTesting(false))
        .onDrop(of: [.text], isTargeted: nil) { providers, location in
            guard !store.busy, !store.isRecording, let provider = providers.first else { return false }
            _ = provider.loadObject(ofClass: String.self) { value, _ in
                guard let value, let id = UUID(uuidString: value) else { return }
                Task { @MainActor in store.moveSelectedClips(primary: id, to: max(0, Int(location.x / store.zoom * sessionRate)), targetTrack: track.id) }
            }
            return true
        }
        .contextMenu {
            Button("Enhance vocals…") { store.requestVocalEnhancement(trackID: track.id) }.disabled(!store.canUseTrackTools(track.id))
            Button("Separate source…") { store.requestSourceSeparation() }.disabled(!store.canSeparateSource)
            Divider()
            if let clip = store.clipForContextMenu(track: track) {
                ClipHistoryMenu(store: store, clip: clip, trackID: track.id)
                Divider()
            }
            Button("Split at playhead") { store.split() }
            Button("Combine selected clips") { store.combineClips() }.disabled(!store.canCombineClips)
            Button("Copy") { store.copy() }.disabled(!store.hasAudioSelection)
            Button("Cut") { store.cut() }.disabled(!store.hasAudioSelection)
            Button("Paste") { store.paste() }
            if store.currentClip?.editableSoundDesign != nil {
                Button("Edit notes inline") { store.toggleInlineSound(trackID: track.id, clipID: store.currentClip?.id) }
                Button("Full sound designer…") { store.requestSoundDesigner(editClip: true) }
            }
            if store.currentClip?.editableDrumPattern != nil {
                Button("Edit beat inline") { store.toggleInlineDrums(trackID: track.id, clipID: store.currentClip?.id) }
                Button("Full drum designer…") { store.requestDrumDesigner(editClip: true) }
            }
            Button("Align clip to playhead") { store.alignToPlayhead() }.disabled(store.currentClip == nil)
            Menu("Move clip to track") {
                ForEach(store.session.tracks.filter { $0.id != store.selectedTrack && $0.acceptsClips }) { destination in
                    Button(destination.name) { store.moveClipToTrack(destination.id) }
                }
            }.disabled(store.currentClip == nil)
            Toggle("Move automation with clips", isOn: $store.moveAutomationWithClips)
            Button("Delete selected audio", role: .destructive) { store.deleteSelectedClips() }.disabled(!store.hasAudioSelection)
        }
        .accessibilityElement(children: .contain).accessibilityLabel("\(track.name) timeline")
    }
}

struct DeviceSheet: View {
    @ObservedObject var store: SessionStore
    @Environment(\.dismiss) var dismiss
    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            Text("Audio devices").font(.system(size: 20, weight: .semibold))
            Picker("Record from", selection: Binding(get: { store.inputDevice }, set: { value in
                store.inputDevice = value; store.remoteGUI?.send(.setDevices(inputID: value, outputID: nil))
            })) {
                ForEach(store.devices.filter { $0.inputChannels > 0 }) { device in Text("\(device.name) · \(device.inputChannels) ch").tag(device.id) }
            }
            Picker("Play through", selection: Binding(get: { store.outputDevice }, set: { value in
                store.outputDevice = value; store.remoteGUI?.send(.setDevices(inputID: nil, outputID: value))
            })) {
                ForEach(store.devices.filter { $0.outputChannels > 0 }) { device in Text(device.name).tag(device.id) }
            }
            Button("Recording setup…") { store.showDevices = false; DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) { store.showRecordingSetup = true } }
            Text("Choose a device and channels on each track.").font(.system(size: 12)).foregroundStyle(Theme.secondary)
            HStack {
                Text("Recording offset (ms)")
                CommittedNumberField("Offset", value: $store.recordingOffsetMS, range: -500...500, fractionDigits: 1).textFieldStyle(.roundedBorder).frame(width: 80).accessibilityLabel("Recording offset milliseconds")
            }.font(.system(size: 12))
            Toggle("Monitor input · Use headphones", isOn: $store.inputMonitoring)
            HStack {
                Button("Refresh devices") { store.refreshDevices() }
                Spacer()
                Button("Done") { NSApp.keyWindow?.makeFirstResponder(nil); store.rememberPreferences(); dismiss() }.buttonStyle(RoomButtonStyle(prominent: true)).keyboardShortcut(.defaultAction)
            }
        }.padding(28).frame(width: 480).background(Theme.paper).foregroundStyle(Theme.ink).tint(Theme.accent).preferredColorScheme(.dark)
    }
}

struct ExportSheet: View {
    @ObservedObject var store: SessionStore
    @Environment(\.dismiss) var dismiss
    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            Text(store.exportTracks ? "Export tracks" : "Export mix").font(.system(size: 20, weight: .semibold))
            EditingScopeView(scope: .init(trackName: store.exportTracks ? "Selected tracks" : "Entire mix", detail: store.exportSelection ? "Selected time range" : "Whole session", range: store.exportSelection ? store.selectedRange : 0..<store.session.end))
            Picker("Format", selection: $store.exportFormat) {
                Text("WAV · 24-bit, 48 kHz").tag("wav")
                Text("AIFF · 24-bit, 48 kHz").tag("aiff")
                Text("AAC · 256 kbps (.m4a)").tag("m4a")
            }
            Toggle("Export tracks as separate files", isOn: $store.exportTracks)
            if store.exportTracks {
                ScrollView {
                    VStack(alignment: .leading) {
                        ForEach(store.session.tracks) { track in
                            Toggle(track.name, isOn: Binding(get: { store.exportTrackIDs.contains(track.id) }, set: { selected in
                                if selected { store.exportTrackIDs.insert(track.id) } else { store.exportTrackIDs.remove(track.id) }
                            }))
                        }
                    }
                }.frame(maxHeight: 150)
            }
            Toggle("Include effect tails", isOn: $store.exportIncludeTails)
            Toggle("Export selected time range only", isOn: $store.exportSelection).disabled(store.selectedRange == nil)
            Text(store.exportTracks ? "Aligned stereo files · Track effects included · Master processing excluded" : "Stereo mix · Track and Master processing included").font(.system(size: 11)).foregroundStyle(Theme.secondary)
            HStack { Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction); Spacer(); Button("Choose destination…") { store.exportMix() }.buttonStyle(RoomButtonStyle(prominent: true)).keyboardShortcut(.defaultAction) }
        }.padding(28).frame(width: 450).background(Theme.paper).foregroundStyle(Theme.ink).tint(Theme.accent).preferredColorScheme(.dark)
        .onAppear { if store.selectedRange == nil { store.exportSelection = false }; if store.exportTrackIDs.isEmpty { store.exportTrackIDs = Set(store.session.tracks.map(\.id)) } }
    }
}

struct RenameTrackSheet: View {
    @ObservedObject var store: SessionStore
    let request: RenameRequest
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @FocusState private var focused: Bool
    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            Text("Rename track").font(.system(size: 18, weight: .semibold))
            TextField("Track name", text: $name).textFieldStyle(.roundedBorder).focused($focused).onSubmit(commit)
            HStack {
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Spacer()
                Button("Rename", action: commit).buttonStyle(RoomButtonStyle(prominent: true)).keyboardShortcut(.defaultAction)
                    .disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }.padding(24).frame(width: 350).background(Theme.paper).foregroundStyle(Theme.ink).preferredColorScheme(.dark)
            .onAppear { name = request.name; focused = true }
            .onDisappear { store.focusEditor() }
    }
    private func commit() {
        guard !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        store.renameTrack(request.id, to: name); dismiss()
    }
}
