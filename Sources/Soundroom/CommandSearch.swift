import SwiftUI
import SoundroomCore

struct EditorCommand: Identifiable {
    let id: String
    let title: String
    let shortcut: String
    let keywords: String
    let enabled: Bool
    let run: () -> Void
}
extension SessionStore {
    var searchableCommands: [EditorCommand] {
        let canEdit = !busy && !isRecording
        return [
            EditorCommand(id: "play", title: "Play / Pause", shortcut: "Space", keywords: "stop transport listen", enabled: !busy && !isRecording, run: { self.togglePlay() }),
            EditorCommand(id: "start", title: "Return to Start", shortcut: "Return", keywords: "rewind transport", enabled: canEdit, run: { self.returnToStart() }),
            EditorCommand(id: "record", title: "Record / Finish Take", shortcut: "R", keywords: "microphone capture", enabled: !busy, run: { self.record() }),
            EditorCommand(id: "deleteTrack", title: "Delete Track", shortcut: "⌘⌫", keywords: "remove track lane", enabled: canDeleteSelectedTrack, run: { self.deleteSelectedTrack() }),
            EditorCommand(id: "split", title: "Split at Playhead", shortcut: "⌘E", keywords: "slice cut clip", enabled: canEdit && currentTrack?.clips.contains(where: { $0.start < playhead && $0.end > playhead }) == true, run: { self.split() }),
            EditorCommand(id: "combine", title: "Combine Selected Clips", shortcut: "⇧⌘J", keywords: "merge join consolidate audio", enabled: canCombineClips, run: { self.combineClips() }),
            EditorCommand(id: "copy", title: "Copy Audio", shortcut: "⌘C", keywords: "clipboard clip range", enabled: canEdit && hasAudioSelection, run: { self.copy() }),
            EditorCommand(id: "cut", title: "Cut Audio", shortcut: "⌘X", keywords: "clipboard delete range", enabled: canEdit && hasAudioSelection, run: { self.cut() }),
            EditorCommand(id: "paste", title: "Paste", shortcut: "⌘V", keywords: "clipboard audio track", enabled: canEdit && canPaste, run: { self.paste() }),
            EditorCommand(id: "trim", title: "Trim to Selection", shortcut: "⌘T", keywords: "clip cut crop", enabled: canEdit && selection != nil, run: { self.trim() }),
            EditorCommand(id: "align", title: "Align Clip to Playhead", shortcut: "⌘L", keywords: "move timing", enabled: canEdit && currentClip != nil, run: { self.alignToPlayhead() }),
            EditorCommand(id: "undo", title: undoTitle, shortcut: "⌘Z", keywords: "restore reverse", enabled: canEdit && canUndo, run: { self.undo() }),
            EditorCommand(id: "redo", title: redoTitle, shortcut: "⇧⌘Z", keywords: "repeat restore", enabled: canEdit && canRedo, run: { self.redo() }),
            EditorCommand(id: "track", title: "Add Audio Track", shortcut: "⇧⌘N", keywords: "new multitrack", enabled: canEdit, run: { self.addTrack() }),
            EditorCommand(id: "copy-track", title: "Copy Track", shortcut: "", keywords: "duplicate clipboard", enabled: canEdit && currentTrack != nil, run: { self.copyTrack() }),
            EditorCommand(id: "copy-track-automation", title: "Copy Track + Automation", shortcut: "", keywords: "duplicate curve clipboard", enabled: canEdit && currentTrack != nil, run: { self.copyTrack(includeAutomation: true) }),
            EditorCommand(id: "volume", title: "Show Volume Automation", shortcut: "", keywords: "fader fade loudness", enabled: canEdit && currentTrack != nil, run: { self.showMixAutomation(.volume, trackID: self.currentTrack!.id) }),
            EditorCommand(id: "pan", title: "Show Pan Automation", shortcut: "", keywords: "stereo left right", enabled: canEdit && currentTrack != nil, run: { self.showMixAutomation(.pan, trackID: self.currentTrack!.id) }),
            EditorCommand(id: "copy-automation", title: "Copy Automation Range", shortcut: "⌘C in lane", keywords: "points ramp curve clipboard", enabled: canEditAutomation && automationSelectionRange != nil, run: { self.copyAutomationRange() }),
            EditorCommand(id: "paste-automation", title: "Paste Automation at Playhead", shortcut: "⌘V in lane", keywords: "points ramp curve clipboard", enabled: canPasteAutomation, run: { self.pasteAutomation() }),
            EditorCommand(id: "duplicate-automation", title: "Duplicate Automation Ramp", shortcut: "", keywords: "points repeat curve", enabled: canEditAutomation && automationSelectionRange != nil, run: { self.duplicateAutomationRange() }),
            EditorCommand(id: "simplify-automation", title: "Simplify Automation Gesture", shortcut: "", keywords: "points reduce dense curve", enabled: canEditAutomation && automationPoints.count > 2, run: { self.simplifyCurrentAutomation() }),
            EditorCommand(id: "designer", title: "Sound Designer", shortcut: "⇧⌘D", keywords: "synth midi notes", enabled: canEdit, run: { self.requestSoundDesigner() }),
            EditorCommand(id: "beats", title: "Drum Loop Designer", shortcut: "⇧⌘B", keywords: "beats percussion acoustic electronic steps swing", enabled: canEdit, run: { self.requestDrumDesigner() }),
            EditorCommand(id: "enhance-vocals", title: "Enhance Vocals…", shortcut: "", keywords: "voice cleanup singing deess clarity", enabled: canUseTrackTools(selectedTrack), run: { self.requestVocalEnhancement() }),
            EditorCommand(id: "separate-source", title: "Separate Source…", shortcut: "", keywords: "isolate vocals bass drums stems remove instrument", enabled: canSeparateSource, run: { self.requestSourceSeparation() }),
            EditorCommand(id: "import", title: "Import Audio", shortcut: "⌘I", keywords: "file wav aiff", enabled: canEdit, run: { self.importAudio() }),
            EditorCommand(id: "save", title: "Save Session", shortcut: "⌘S", keywords: "project file", enabled: !busy, run: { self.requestSave() }),
            EditorCommand(id: "export", title: "Export Mix", shortcut: "⇧⌘E", keywords: "bounce wav stems", enabled: canEdit && session.end > 0, run: { self.showExport = true }),
            EditorCommand(id: "zoom", title: "Zoom to Selection", shortcut: "⌘J", keywords: "timeline range", enabled: canEdit && selectedRange != nil, run: { self.zoomToSelection() }),
            EditorCommand(id: "fit", title: "Fit Session", shortcut: "⌘0", keywords: "timeline zoom whole", enabled: canEdit, run: { self.fitSession() }),
            EditorCommand(id: "reveal", title: "Reveal Playhead", shortcut: "F", keywords: "timeline scroll cursor", enabled: canEdit, run: { self.revealPlayhead() }),
            EditorCommand(id: "snap", title: snap ? "Turn Snap Off" : "Turn Snap On", shortcut: "", keywords: "grid edges timing", enabled: canEdit, run: { self.snap.toggle() }),
            EditorCommand(id: "loop-selection", title: "Loop Selected Clips or Range", shortcut: "", keywords: "repeat transport selection", enabled: canEdit && timelineSelectionRange != nil, run: { self.useSelectionForLoop() }),
            EditorCommand(id: "loop", title: "Toggle Loop", shortcut: "L", keywords: "repeat transport range clips", enabled: canEdit && canToggleLoop, run: { self.loopSelection.toggle() })
        ]
    }
    func filteredCommands(_ query: String) -> [EditorCommand] {
        let words = query.lowercased().split(whereSeparator: \.isWhitespace)
        return searchableCommands.filter { command in
            let terms = "\(command.title) \(command.keywords) \(command.shortcut)".lowercased()
            return words.allSatisfy { terms.contains($0) }
        }
    }
}

struct CommandSearchView: View {
    @ObservedObject var store: SessionStore
    @Environment(\.dismiss) private var dismiss
    @State private var query = ""
    @State private var selected = 0
    @FocusState private var searching: Bool
    private var results: [EditorCommand] { store.filteredCommands(query) }
    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Image(systemName: "magnifyingglass").foregroundStyle(Theme.secondary)
                TextField("Find a command…", text: $query).textFieldStyle(.plain).focused($searching)
                    .onSubmit { executeSelected() }
                    .onKeyPress(.downArrow) { selected = min(max(0, results.count - 1), selected + 1); return .handled }
                    .onKeyPress(.upArrow) { selected = max(0, selected - 1); return .handled }
                Button("Close") { dismiss() }.keyboardShortcut(.cancelAction)
            }.padding(16)
            Divider()
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(spacing: 2) {
                        ForEach(Array(results.enumerated()), id: \.element.id) { index, command in
                            Button { execute(command) } label: {
                                HStack { Text(command.title); Spacer(); Text(command.shortcut).foregroundStyle(Theme.secondary).font(.system(size: 12, design: .monospaced)) }
                                    .padding(.horizontal, 12).padding(.vertical, 9).contentShape(Rectangle())
                                    .background(index == selected ? Theme.accent.opacity(0.18) : Color.clear, in: RoundedRectangle(cornerRadius: 5))
                            }.buttonStyle(.plain).disabled(!command.enabled).opacity(command.enabled ? 1 : 0.45).id(index)
                        }
                        if results.isEmpty { Text("No matching commands").foregroundStyle(Theme.secondary).padding(24) }
                    }.padding(8)
                }.onChange(of: selected) { _, value in proxy.scrollTo(value) }
            }
            Divider()
            Text("↑ ↓ choose · Return run · Escape close").font(.system(size: 11)).foregroundStyle(Theme.secondary).padding(10)
        }.frame(width: 520, height: 440).background(Theme.paper).preferredColorScheme(.dark)
            .onAppear { searching = true }.onChange(of: query) { _, _ in selected = 0 }
    }
    private func executeSelected() {
        guard results.indices.contains(selected) else { return }; execute(results[selected])
    }
    private func execute(_ command: EditorCommand) {
        guard command.enabled else { return }
        dismiss()
        DispatchQueue.main.async { store.focusEditor(); command.run() }
    }
}
