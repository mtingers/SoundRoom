import AppKit
import CoreAudio
import SoundroomCore
import SwiftUI
import UniformTypeIdentifiers

struct SoundDesignerSheet: View {
    @State private var pattern: SoundDesignPattern
    @State private var selectedNotes: Set<UUID>
    @State private var octave: Int
    @State private var grid = 0.25
    @State private var previewing = false
    @State private var designingInstrument = false
    @State private var previewStarted = 0.0
    @State private var workspace = 0
    @State private var viewport = DesignerViewport()
    @State private var previewTask: Task<Void, Never>?
    @State private var liveKeys = DesignerHeldKeys()
    @State private var showingKeyboard = false
    @State private var keyboardVelocity = 102.0
    @State private var undoHistory: [SoundDesignPattern] = []
    @State private var redoHistory: [SoundDesignPattern] = []
    @State private var previousPattern: SoundDesignPattern
    @State private var historyPlayback = false
    @State private var groupingEdit = false
    @State private var groupSnapshot: SoundDesignPattern?
    @State private var gesturePattern: SoundDesignPattern?
    @State private var copiedNotes: [SoundDesignNote] = []
    @State private var insertionBeat = 0.0
    @State private var keyboardFocus = UUID()
    @State private var designerWindow = DesignerWindowReference()
    @State private var status = ""
    @State private var importError: String?
    @State private var importReview: DesignerMIDIReviewRequest?
    @State private var importingMIDI = false
    @State private var importTask: Task<Void, Never>?
    @State private var controllerLane = SoundDesignControllerKind.sustain
    @State private var layersExpanded = true
    @State private var humanizePass: UInt64 = 17
    @State private var controllerValues: [SoundDesignControllerKind: Double] = [:]
    @State private var recordingMIDI = false
    @State private var recordStart = 0.0
    @State private var recordingTask: Task<Void, Never>?
    @State private var heldNotes: [Int: (beat: Double, velocity: Double)] = [:]
    @StateObject private var midi = DesignerMIDIInput()
    let remotePerformance: RemoteDesignerPerformance
    let previewPosition: (() -> Double)?
    let previewReady: (() -> Bool)?
    let outputDevice: AudioDeviceID
    let isEditing: Bool
    let onPreview: (SoundDesignPattern) -> Void
    let onStop: () -> Void
    let onCommit: (SoundDesignPattern) -> Void
    let onCancel: () -> Void
    let commitPending: Bool
    let commitMessage: String
    let onDraftChanged: () -> Void
    let onInsertNewTrack: ((SoundDesignPattern) -> Void)?

    init(
        pattern: SoundDesignPattern, isEditing: Bool, remotePerformance: RemoteDesignerPerformance,
        outputDevice: AudioDeviceID = 0, previewReady: (() -> Bool)? = nil, previewPosition: (() -> Double)? = nil,
        onPreview: @escaping (SoundDesignPattern) -> Void, onStop: @escaping () -> Void,
        onCommit: @escaping (SoundDesignPattern) -> Void, onCancel: @escaping () -> Void,
        commitPending: Bool = false, commitMessage: String = "", onDraftChanged: @escaping () -> Void = {},
        onInsertNewTrack: ((SoundDesignPattern) -> Void)? = nil
    ) {
        var editable = pattern
        editable.bpm = pattern.bpm.isFinite ? max(30, min(300, pattern.bpm)) : 120
        editable.beats = pattern.beats.isFinite ? max(0.25, min(64, pattern.beats)) : 4
        editable.notes = pattern.notes.prefix(256).filter {
            $0.startBeat.isFinite && $0.durationBeats.isFinite && $0.startBeat >= 0 && $0.startBeat < editable.beats
                && $0.durationBeats > 0
        }.map { note in
            var result = note
            result.pitch = max(0, min(127, note.pitch))
            result.velocity = note.velocity.isFinite ? max(0, min(1, note.velocity)) : 0.8
            result.durationBeats = min(editable.beats - note.startBeat, note.durationBeats)
            return result
        }
        _pattern = State(initialValue: editable)
        _previousPattern = State(initialValue: editable)
        _selectedNotes = State(initialValue: Set(editable.notes.prefix(1).map(\.id)))
        _octave = State(initialValue: max(0, min(10, (editable.notes.first?.pitch ?? 60) / 12)))
        self.remotePerformance = remotePerformance
        _midi = StateObject(wrappedValue: DesignerMIDIInput())
        self.previewPosition = previewPosition
        self.previewReady = previewReady
        self.outputDevice = outputDevice
        self.isEditing = isEditing
        self.onPreview = onPreview
        self.onStop = onStop
        self.onCommit = onCommit
        self.onCancel = onCancel
        self.commitPending = commitPending
        self.commitMessage = commitMessage
        self.onDraftChanged = onDraftChanged
        self.onInsertNewTrack = onInsertNewTrack
    }
    private var selectedIndex: Int? { pattern.notes.firstIndex { selectedNotes.contains($0.id) } }
    private var midiSource: Binding<UInt32> {
        Binding(
            get: { midi.connected },
            set: {
                stopMIDIRecording()
                allDesignerNotesOff()
                midi.connect($0)
            })
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            header
            HStack {
                Picker("Workspace", selection: $workspace) {
                    Label("Sequence", systemImage: "pianokeys").tag(0)
                    Label("Sound & layers", systemImage: "waveform.path").tag(1)
                }.pickerStyle(.segmented).labelsHidden().frame(width: 280).onChange(of: workspace) { focusDesigner() }
                Spacer()
                Text(pattern.instrument.title).font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.secondary)
                if previewing {
                    Label("Live", systemImage: "waveform").font(.system(size: 11)).foregroundStyle(Theme.accent)
                }
            }
            patternControls
            if workspace == 0 {
                VStack(spacing: 8) {
                    gridControls
                    viewportControls
                    noteGrid
                    noteControls
                    controllerControls
                }
                Spacer(minLength: 0)
            } else {
                ScrollView {
                    VStack(spacing: 12) {
                        organicControls
                        instrumentControls
                        layerControls
                    }.padding(.trailing, 4)
                }
            }
            Divider().overlay(Theme.line)
            footer
        }
        .padding(16).frame(width: 960, height: 740)
        .foregroundStyle(Theme.ink).background(Theme.paper).tint(Theme.accent).preferredColorScheme(.dark)
        .background(
            DesignerKeyboardBridge(
                onKey: handleKey, onRelease: releaseKeyboardNotes, focusRequest: keyboardFocus,
                windowReference: designerWindow)
        )
        .overlay(alignment: .bottom) {
            if showingKeyboard {
                DesignerVirtualKeyboard(
                    octave: $octave, velocity: $keyboardVelocity,
                    active: liveKeys.pitches, recording: recordingMIDI,
                    onNote: receivePointerNote,
                    onRecord: { if recordingMIDI { stopMIDIRecording() } else { startMIDIRecording() } },
                    onClose: { showingKeyboard = false }
                )
                .frame(width: 542)
                .clipShape(RoundedRectangle(cornerRadius: 12))
                .overlay(RoundedRectangle(cornerRadius: 12).stroke(Theme.line))
                .shadow(color: .black.opacity(0.45), radius: 16, y: 6)
                .padding(.bottom, 76)
                .onAppear { focusDesigner() }
                .onDisappear {
                    releaseKeyboardNotes()
                    focusDesigner()
                }
            }
        }
        .onChange(of: pattern) { _, new in
            onDraftChanged()
            if new != previousPattern && !historyPlayback && !groupingEdit {
                undoHistory.append(previousPattern)
                trimHistory()
                redoHistory.removeAll()
            }
            historyPlayback = false
            previousPattern = new
            viewport = viewport.fitted(to: new.beats)
            if previewing && !recordingMIDI { onPreview(new) }
            if midi.connected != 0 { remotePerformance.updateMIDI(new) }
        }
        .onChange(of: pattern.bpm) { _, value in
            let safe = value.isFinite ? max(30, min(300, value)) : 120
            if safe != value { pattern.bpm = safe }
        }
        .onChange(of: midi.connected) { _, value in
            if value == 0 {
                stopMIDIRecording()
                allDesignerNotesOff()
            }
            heldNotes.removeAll()
            liveKeys.clear()
        }
        .onAppear {
            midi.onNote = receiveMIDINote
            midi.onController = receiveMIDIController
            midi.onAllNotesOff = {
                allDesignerNotesOff()
                stopMIDIRecording()
                liveKeys.clear()
            }
            remotePerformance.attachMIDI(midi, pattern)
            focusDesigner()
        }
        .onDisappear {
            importTask?.cancel()
            stopMIDIRecording()
            stopPreview()
            releaseKeyboardNotes()
            midi.close()
            remotePerformance.detachMIDI()
        }
        .alert(
            "Designer could not load the file",
            isPresented: Binding(
                get: { importError != nil },
                set: {
                    if !$0 {
                        importError = nil
                        focusDesigner()
                    }
                })
        ) {
            Button("OK", role: .cancel) {
                importError = nil
                focusDesigner()
            }
        } message: {
            Text(importError ?? "")
        }
        .sheet(isPresented: $designingInstrument, onDismiss: focusDesigner) {
            InstrumentDesignerSheet(
                pattern: $pattern, playing: previewing, onPlay: preview, onClose: { designingInstrument = false })
        }
        .sheet(item: $importReview, onDismiss: focusDesigner) { request in
            DesignerMIDIReviewSheet(request: request) { imported in
                acceptMIDI(imported)
                importReview = nil
            } onCancel: {
                importReview = nil
            }
        }
    }

    private var header: some View {
        HStack {
            Text("Sound designer").font(.system(size: 21, weight: .semibold, design: .rounded))
            Spacer()
            Button {
                undo()
            } label: {
                Image(systemName: "arrow.uturn.backward")
            }
            .buttonStyle(RoomButtonStyle()).disabled(undoHistory.isEmpty || recordingMIDI).help("Undo · ⌘Z")
            .accessibilityLabel("Undo designer edit")
            Button {
                redo()
            } label: {
                Image(systemName: "arrow.uturn.forward")
            }
            .buttonStyle(RoomButtonStyle()).disabled(redoHistory.isEmpty || recordingMIDI).help("Redo · ⇧⌘Z")
            .accessibilityLabel("Redo designer edit")
            Menu("Preset") {
                ForEach(SoundDesignPreset.allCases) { preset in
                    Button(preset.rawValue) { pattern = preset.applying(to: pattern) }
                }
                Divider()
                ForEach(OrganicPatch.allCases) { patch in
                    Button(patch.rawValue) {
                        pattern = patch.applying(to: pattern)
                        focusDesigner()
                    }
                }
                Divider()
                Button("Save sound…", action: saveSound)
                Button("Load sound…", action: loadSound)
            }.frame(width: 120).disabled(recordingMIDI)
        }
    }
    private var organicControls: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack {
                Label("Organic patch bank", systemImage: "sparkles").font(.system(size: 13, weight: .semibold))
                Spacer()
                Text("Preserves your notes").font(.system(size: 10)).foregroundStyle(Theme.secondary)
            }
            HStack(spacing: 8) {
                ForEach(OrganicPatch.allCases) { patch in
                    Button {
                        pattern = patch.applying(to: pattern)
                        layersExpanded = true
                        focusDesigner()
                    } label: {
                        VStack(alignment: .leading, spacing: 7) {
                            Text(patch.rawValue).font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.ink)
                            Text(patch.detail).font(.system(size: 10)).foregroundStyle(Theme.secondary).lineLimit(3)
                                .frame(height: 38, alignment: .topLeading)
                        }.frame(maxWidth: .infinity, alignment: .leading).padding(12)
                            .background(Theme.raised, in: RoundedRectangle(cornerRadius: 7))
                            .overlay(RoundedRectangle(cornerRadius: 7).stroke(Theme.accent.opacity(0.3)))
                    }.buttonStyle(.plain).accessibilityLabel("Load \(patch.rawValue) sound")
                }
            }
            HStack(spacing: 24) {
                parameter(
                    "Tuning drift", value: $pattern.organic.drift, range: 0...1,
                    display: String(format: "±%.1f ct", pattern.organic.drift * 6)
                )
                .help("Slow independent pitch movement; maximum ±6 cents. Zero bypasses drift.")
                parameter(
                    "Timbre variation", value: $pattern.organic.timbre, range: 0...1,
                    display: percent(pattern.organic.timbre)
                )
                .help("Repeatable differences in attack, tone and instrument character between notes.")
                parameter(
                    "Breathing", value: $pattern.organic.breath, range: 0...1, display: percent(pattern.organic.breath)
                )
                .help("Slow level movement, up to ±6%, independent for each layer.")
                Button("New variation") {
                    pattern.organic.seed &+= 1
                    for i in pattern.layers.indices {
                        pattern.layers[i].organicSeed = (pattern.layers[i].organicSeed ?? 97) &+ 1
                    }
                    focusDesigner()
                }
                .buttonStyle(RoomButtonStyle()).help("Choose a new repeatable variation without changing the notes")
            }
        }.padding(12).background(Theme.background, in: RoundedRectangle(cornerRadius: 8)).disabled(recordingMIDI)
    }
    private func saveSound() {
        releaseKeyboardNotes()
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.json]
        panel.nameFieldStringValue = "My sound.soundroom-preset.json"
        panel.title = "Save Designer sound"
        panel.prompt = "Save sound"
        guard let window = designerWindow.view?.window ?? NSApp.keyWindow else { return }
        panel.beginSheetModal(for: window) { response in
            defer { focusDesigner() }
            guard response == .OK, let url = panel.url else { return }
            do {
                let encoder = JSONEncoder()
                encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
                try encoder.encode(DesignerSoundPreset(pattern: pattern)).write(to: url, options: .atomic)
                status = "Sound saved."
            } catch { importError = error.localizedDescription }
        }
    }
    private func loadSound() {
        releaseKeyboardNotes()
        let panel = DesignerSoundFilePanel.makeOpenPanel()
        panel.title = "Load Designer sound"
        panel.prompt = "Load sound"
        guard let window = designerWindow.view?.window ?? NSApp.keyWindow else { return }
        panel.beginSheetModal(for: window) { response in
            defer { focusDesigner() }
            guard response == .OK, let url = panel.url else { return }
            do {
                let scoped = url.startAccessingSecurityScopedResource()
                defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
                guard (attributes[.size] as? NSNumber)?.intValue ?? 0 <= 1_000_000 else {
                    throw SoundroomError.message("This sound preset is too large.")
                }
                let preset = try JSONDecoder().decode(DesignerSoundPreset.self, from: Data(contentsOf: url))
                guard preset.version == 1 else {
                    throw SoundroomError.message("This sound preset needs a newer version of Soundroom.")
                }
                pattern = preset.applying(to: pattern)
                workspace = 1
                layersExpanded = true
                status = "Sound loaded."
            } catch { importError = error.localizedDescription }
        }
    }
    private var patternControls: some View {
        HStack(spacing: 18) {
            HStack(spacing: 6) {
                Text("BPM").foregroundStyle(Theme.secondary)
                CommittedNumberField(
                    "BPM", value: $pattern.bpm, range: 30...300, onEditingChanged: groupEdit, onSubmit: focusDesigner
                )
                .textFieldStyle(.roundedBorder).frame(width: 58).accessibilityLabel("Tempo in beats per minute")
                Stepper("Tempo", value: $pattern.bpm, in: 30...300).labelsHidden()
            }
            Picker("Length", selection: lengthBinding) {
                ForEach([1.0, 2, 4, 8, 16, 32, 64], id: \.self) { Text("\(Int($0)) beats").tag($0) }
                if ![1.0, 2, 4, 8, 16, 32, 64].contains(pattern.beats) {
                    Text("\(pattern.beats.formatted()) beats").tag(pattern.beats)
                }
            }.frame(width: 156)
            Picker("Grid", selection: $grid) {
                Text("1/4").tag(1.0)
                Text("1/8").tag(0.5)
                Text("1/16").tag(0.25)
                Text("1/32").tag(0.125)
            }.frame(width: 118)
            Spacer()
            Menu {
                Button("Import MIDI…", action: importMIDI).disabled(importingMIDI)
                Button("Export MIDI…", action: exportMIDI).disabled(pattern.notes.isEmpty)
            } label: {
                Label("MIDI", systemImage: "square.and.arrow.down")
            }.frame(width: 100)
        }.font(.system(size: 12)).disabled(recordingMIDI)
    }
    private var instrumentControls: some View {
        VStack(spacing: 0) {
            VStack(spacing: 12) {
                HStack {
                    Picker(
                        "Instrument",
                        selection: Binding(
                            get: { pattern.instrument },
                            set: {
                                pattern = $0.applying(to: pattern)
                                focusDesigner()
                            })
                    ) {
                        ForEach(MusicalInstrument.allCases) { Text($0.title).tag($0) }
                    }.accessibilityLabel("Designer instrument")
                    if previewing { Label("Live", systemImage: "waveform").foregroundStyle(Theme.accent) }
                }
                if pattern.instrument.isRecorded {
                    HStack(spacing: 12) {
                        Picker(
                            "Sound",
                            selection: Binding(
                                get: { pattern.instrumentShape != nil },
                                set: { designed in
                                    pattern.instrumentShape =
                                        designed ? (pattern.instrumentShape ?? SoundDesignInstrumentShape()) : nil
                                })
                        ) {
                            Text("Recording").tag(false)
                            Text("Designed").tag(true)
                        }.pickerStyle(.segmented).frame(width: 220)
                            .help(
                                "Recording plays the real instrument; Designed generates it from measurements so you can shape it"
                            )
                        Button("Instrument designer…") {
                            if pattern.instrumentShape == nil { pattern.instrumentShape = SoundDesignInstrumentShape() }
                            designingInstrument = true
                        }
                        .buttonStyle(RoomButtonStyle())
                        Spacer()
                    }
                }
                if pattern.instrument != .synthesizer && !pattern.instrument.isRecorded {
                    HStack(spacing: 18) {
                        parameter(
                            pattern.instrument.characterLabel, value: $pattern.character, range: 0...1,
                            display: percent(pattern.character))
                        parameter(
                            "Resonance", value: $pattern.resonance, range: 0...1, display: percent(pattern.resonance))
                    }
                }
                if pattern.instrument == .synthesizer {
                    HStack(spacing: 18) {
                        Picker("Oscillator 1", selection: $pattern.waveform) {
                            ForEach(SoundDesignWaveform.allCases, id: \.self) { Text($0.title).tag($0) }
                        }
                        Picker("Oscillator 2", selection: $pattern.secondWaveform) {
                            ForEach(SoundDesignWaveform.allCases, id: \.self) { Text($0.title).tag($0) }
                        }
                        parameter(
                            "Blend", value: $pattern.oscillatorMix, range: 0...1,
                            display: percent(pattern.oscillatorMix))
                        parameter(
                            "Detune", value: $pattern.detune, range: -50...50, display: "\(Int(pattern.detune)) cents")
                    }
                }
                // Recorded instruments keep their own envelope and tone; they
                // are shaped in the instrument designer instead.
                if !pattern.instrument.isRecorded {
                    DesignerEnvelopePreview(
                        attack: pattern.attack, decay: pattern.decay, sustain: pattern.sustain, release: pattern.release
                    )
                    HStack(spacing: 18) {
                        parameter("Attack", value: $pattern.attack, range: 0...2, display: milliseconds(pattern.attack))
                        parameter("Decay", value: $pattern.decay, range: 0...2, display: milliseconds(pattern.decay))
                        parameter("Sustain", value: $pattern.sustain, range: 0...1, display: percent(pattern.sustain))
                        parameter(
                            "Release", value: $pattern.release, range: 0...2, display: milliseconds(pattern.release))
                    }
                }
                HStack(spacing: 18) {
                    parameter(
                        "Fine tune", value: $pattern.fineTune, range: -50...50,
                        display: "\(Int(pattern.fineTune)) cents")
                    if !pattern.instrument.isRecorded {
                        parameter(
                            "Tone", value: $pattern.tone, range: 0...1,
                            display: "\(Int(250 * pow(64, pattern.tone))) Hz")
                    }
                    if pattern.instrument == .synthesizer {
                        parameter(
                            "Pulse width", value: $pattern.pulseWidth, range: 0.05...0.95,
                            display: percent(pattern.pulseWidth))
                        parameter("Sub", value: $pattern.subLevel, range: 0...1, display: percent(pattern.subLevel))
                        parameter(
                            "Noise", value: $pattern.noiseLevel, range: 0...1, display: percent(pattern.noiseLevel))
                    }
                    if !pattern.instrument.isRecorded {
                        parameter(
                            "Drive", value: $pattern.drive, range: 1...8,
                            display: String(format: "%.1f×", pattern.drive))
                    }
                }
            }.padding(12)
        }.background(Theme.background, in: RoundedRectangle(cornerRadius: 8)).disabled(recordingMIDI)
    }
    private var gridControls: some View {
        HStack {
            Picker("Octave", selection: $octave) {
                ForEach(0...10, id: \.self) { Text($0 == 10 ? "C9 – G9" : "C\($0 - 1) – B\($0 - 1)").tag($0) }
            }.frame(width: 158).help("Octave down/up · Z / X")
            Text("\(pattern.notes.count) notes").foregroundStyle(Theme.secondary)
            Button {
                addNote(
                    pitch: selectedIndex.map { pattern.notes[$0].pitch } ?? octave * 12,
                    beat: min(insertionBeat, max(0, pattern.beats - grid)))
            } label: {
                Label("Add note", systemImage: "plus")
            }
            .buttonStyle(.plain).disabled(recordingMIDI || pattern.notes.count >= 256).help(
                "Add a note at the selected beat · N")
            Spacer()
            Button("Quantize") {
                pattern.notes = SoundDesignEditing.quantized(pattern.notes, grid: grid, patternBeats: pattern.beats)
            }
            .buttonStyle(.plain).disabled(pattern.notes.isEmpty || recordingMIDI).help(
                "Snap note starts and lengths to the grid")
            Button("Humanize") {
                let ids: Set<UUID>? = selectedNotes.isEmpty ? nil : selectedNotes
                pattern.notes = SoundDesignEditing.humanized(
                    pattern.notes, ids: ids, patternBeats: pattern.beats, seed: humanizePass)
                humanizePass &+= 1
                focusDesigner()
            }.buttonStyle(.plain).disabled(pattern.notes.isEmpty || recordingMIDI).help(
                "Vary selected note timing ±0.03 beats and velocity ±8%; all notes when none are selected")
            Button("Clear") {
                pattern.notes.removeAll()
                pattern.controllers.removeAll()
                selectedNotes.removeAll()
            }
            .buttonStyle(.plain).disabled(pattern.notes.isEmpty || recordingMIDI)
            Menu {
                Text("Space — Play / Stop")
                Text("N — Add note; ⌘+/⌘− — Zoom; ⌘0 — Fit")
                Text("R — Record / Stop keyboard or MIDI notes")
                Text("A–K, W E T Y U — Play notes")
                Text("Z / X — Octave down / up")
                Text("Arrows — Move notes")
                Text("⌥↑ / ⌥↓ — Transpose an octave")
                Text("⇧← / ⇧→ — Resize notes")
                Text("⌘A / ⌘C / ⌘V — Select / Copy / Paste")
                Text("⌘D — Duplicate notes")
                Text("⇧Q — Quantize; ⌥H — Humanize")
                Text("Delete — Delete notes")
                Text("⌘Z / ⇧⌘Z — Undo / Redo")
            } label: {
                Image(systemName: "keyboard")
            }.menuStyle(.borderlessButton).frame(width: 26).accessibilityLabel("Designer keyboard shortcuts")
        }.font(.system(size: 12))
    }
    private var visibleRange: DesignerViewport { viewport.fitted(to: pattern.beats) }
    private var viewportControls: some View {
        HStack(spacing: 8) {
            Button {
                panViewport(-1)
            } label: {
                Image(systemName: "chevron.left")
            }.disabled(visibleRange.start == 0).accessibilityLabel("Previous beats")
            GeometryReader { geometry in
                Canvas { context, size in
                    for note in pattern.notes {
                        let rect = CGRect(
                            x: note.startBeat / pattern.beats * size.width,
                            y: Double(127 - note.pitch) / 127 * (size.height - 3),
                            width: max(2, note.durationBeats / pattern.beats * size.width), height: 2)
                        context.fill(Path(rect), with: .color(Theme.accent.opacity(0.55)))
                    }
                    let window = CGRect(
                        x: visibleRange.start / pattern.beats * size.width, y: 1,
                        width: visibleRange.span / pattern.beats * size.width, height: size.height - 2)
                    context.fill(Path(window), with: .color(Theme.accent.opacity(0.12)))
                    context.stroke(Path(window), with: .color(Theme.accent), lineWidth: 1)
                }.contentShape(Rectangle()).gesture(
                    DragGesture(minimumDistance: 0).onChanged { value in
                        viewport = DesignerViewport(
                            start: value.location.x / max(1, geometry.size.width) * pattern.beats - visibleRange.span
                                / 2,
                            span: visibleRange.span
                        ).fitted(to: pattern.beats)
                    }
                ).overlay(alignment: .topLeading) {
                    DesignerPlayhead(
                        viewport: DesignerViewport(span: pattern.beats), bpm: pattern.bpm, beats: pattern.beats,
                        playing: previewing, recording: recordingMIDI,
                        started: recordingMIDI ? recordStart : previewStarted,
                        position: previewPosition, width: geometry.size.width, height: 24)
                }.accessibilityLabel("Sequence overview").accessibilityValue(
                    "Beats \(visibleRange.start + 1) through \(visibleRange.start + visibleRange.span)"
                )
                .accessibilityAdjustableAction { direction in
                    if direction == .increment { panViewport(1) } else if direction == .decrement { panViewport(-1) }
                }
                .help("Click or drag to navigate the sequence")
            }.frame(height: 24).background(Theme.background, in: RoundedRectangle(cornerRadius: 4))
            Button {
                panViewport(1)
            } label: {
                Image(systemName: "chevron.right")
            }.disabled(visibleRange.start + visibleRange.span >= pattern.beats).accessibilityLabel("Next beats")
            Text(
                "\((visibleRange.start + 1).formatted(.number.precision(.fractionLength(0...1))))–\((visibleRange.start + visibleRange.span).formatted(.number.precision(.fractionLength(0...1))))"
            )
            .font(.system(size: 10, design: .monospaced)).foregroundStyle(Theme.secondary).frame(width: 65)
            Button {
                zoomViewport(2)
            } label: {
                Image(systemName: "minus.magnifyingglass")
            }.accessibilityLabel("Zoom out sequence").help("Zoom out · ⌘−")
            Button {
                zoomViewport(0.5)
            } label: {
                Image(systemName: "plus.magnifyingglass")
            }.accessibilityLabel("Zoom in sequence").help("Zoom in · ⌘+")
            Button("Fit") { viewport = DesignerViewport(span: pattern.beats) }.help("Fit entire sequence · ⌘0")
            Button("Selection") { revealSelectedNotes() }.disabled(selectedNotes.isEmpty).help("Zoom to selected notes")
        }.buttonStyle(.plain).font(.system(size: 11))
    }
    private func panViewport(_ direction: Double) {
        viewport = DesignerViewport(
            start: visibleRange.start + direction * visibleRange.span * 0.75, span: visibleRange.span
        ).fitted(to: pattern.beats)
        focusDesigner()
    }
    private func zoomViewport(_ factor: Double) {
        let anchor =
            insertionBeat >= visibleRange.start && insertionBeat <= visibleRange.start + visibleRange.span
            ? insertionBeat : visibleRange.start + visibleRange.span / 2
        viewport = visibleRange.zoomed(by: factor, around: anchor, beats: pattern.beats)
        focusDesigner()
    }
    private func ensureInsertionVisible() {
        if insertionBeat < visibleRange.start || insertionBeat >= visibleRange.start + visibleRange.span {
            viewport = DesignerViewport(start: insertionBeat - visibleRange.span / 4, span: visibleRange.span).fitted(
                to: pattern.beats)
        }
    }
    private func revealSelectedNotes() {
        let notes = pattern.notes.filter { selectedNotes.contains($0.id) }
        guard let first = notes.map(\.startBeat).min(), let last = notes.map({ $0.startBeat + $0.durationBeats }).max()
        else { return }
        viewport = DesignerViewport(start: max(0, first - grid), span: max(1, last - first + grid * 2)).fitted(
            to: pattern.beats)
        if let pitch = notes.first?.pitch { octave = pitch / 12 }
        focusDesigner()
    }
    private var noteGrid: some View {
        HStack(alignment: .top, spacing: 0) {
            VStack(spacing: 0) {
                Color.clear.frame(height: 20)
                ForEach((0..<12).reversed(), id: \.self) { pitchClass in
                    let pitch = octave * 12 + pitchClass
                    Button(pitch <= 127 ? SoundDesignNote(pitch: pitch, startBeat: 0).name : "—") {
                        audition(pitch: pitch)
                    }
                    .buttonStyle(.plain).font(.system(size: 10)).foregroundStyle(Theme.secondary)
                    .frame(width: 42, height: 19).background(
                        [1, 3, 6, 8, 10].contains(pitchClass) ? Theme.paper : Theme.raised
                    )
                    .disabled(pitch > 127).accessibilityLabel(
                        pitch <= 127 ? "Play \(SoundDesignNote(pitch: pitch, startBeat: 0).name)" : "Outside MIDI range"
                    )
                }
            }.frame(width: 42)
            GeometryReader { geometry in
                let width = geometry.size.width
                let cellWidth = width / visibleRange.span * grid
                let rowHeight = 19.0
                VStack(spacing: 0) {
                    Canvas { context, size in
                        let step = max(1, Int(ceil(visibleRange.span / 16)))
                        for beat in stride(
                            from: Int(ceil(visibleRange.start)),
                            through: Int(floor(visibleRange.start + visibleRange.span)), by: step)
                        {
                            context.draw(
                                Text(String(beat + 1)).font(.system(size: 9, design: .monospaced)).foregroundColor(
                                    Theme.secondary),
                                at: CGPoint(
                                    x: (Double(beat) - visibleRange.start) / visibleRange.span * size.width + 3, y: 10),
                                anchor: .leading)
                        }
                    }.frame(height: 20).accessibilityHidden(true)
                    ZStack(alignment: .topLeading) {
                        Canvas { context, size in
                            for row in 0..<12 {
                                let pitch = 11 - row
                                context.fill(
                                    Path(
                                        CGRect(x: 0, y: Double(row) * rowHeight, width: size.width, height: rowHeight)),
                                    with: .color([1, 3, 6, 8, 10].contains(pitch) ? Theme.paper : Theme.raised))
                            }
                            var lines = Path()
                            for row in 0...12 {
                                lines.move(to: CGPoint(x: 0, y: Double(row) * rowHeight))
                                lines.addLine(to: CGPoint(x: size.width, y: Double(row) * rowHeight))
                            }
                            let spacing = cellWidth < 7 ? max(grid, 1) : grid
                            for index in Int(
                                floor(visibleRange.start / spacing))...Int(
                                    ceil((visibleRange.start + visibleRange.span) / spacing))
                            {
                                let x = (Double(index) * spacing - visibleRange.start) / visibleRange.span * size.width
                                lines.move(to: CGPoint(x: x, y: 0))
                                lines.addLine(to: CGPoint(x: x, y: size.height))
                            }
                            context.stroke(lines, with: .color(Theme.background), lineWidth: 0.7)
                        }.contentShape(Rectangle()).onTapGesture { location in
                            guard !recordingMIDI else { return }
                            let beat = floor(visibleRange.beat(at: location.x / max(1, width)) / grid) * grid
                            let pitch = octave * 12 + 11 - min(11, max(0, Int(location.y / rowHeight)))
                            if beat < pattern.beats { addNote(pitch: pitch, beat: beat) }
                        }.accessibilityLabel("Piano roll").accessibilityHint(
                            "Click to add notes. Use the arrow keys to move selected notes.")
                        ForEach(pattern.notes.filter { $0.pitch / 12 == octave && visibleRange.includes($0) }) { note in
                            noteView(note, cellWidth: cellWidth, rowHeight: rowHeight)
                        }
                        DesignerPlayhead(
                            viewport: visibleRange, bpm: pattern.bpm, beats: pattern.beats, playing: previewing,
                            recording: recordingMIDI, started: recordingMIDI ? recordStart : previewStarted,
                            position: previewPosition,
                            width: width, height: rowHeight * 12)
                    }.frame(height: rowHeight * 12).clipped()
                    velocityLane(width: width).clipped()
                }
            }
        }.padding(8).frame(height: 320).background(Theme.background, in: RoundedRectangle(cornerRadius: 8))
    }
    private func noteView(_ note: SoundDesignNote, cellWidth: Double, rowHeight: Double) -> some View {
        let width = max(8, note.durationBeats / grid * cellWidth - 2)
        return RoundedRectangle(cornerRadius: 3)
            .fill(Theme.accent.opacity(0.45 + note.velocity * 0.5))
            .overlay(
                RoundedRectangle(cornerRadius: 3).stroke(
                    selectedNotes.contains(note.id) ? Theme.ink : Color.clear, lineWidth: 1.5)
            )
            .overlay(alignment: .trailing) {
                // The visible grip stays 5 pt; the grab zone is wider so length
                // drags don't miss and fall through to moving the note.
                Rectangle().fill(Theme.ink.opacity(0.5)).frame(width: 5).padding(.vertical, 3)
                    .frame(width: min(12, max(6, width * 0.3)), alignment: .trailing)
                    .contentShape(Rectangle()).highPriorityGesture(
                        noteDrag(note, cellWidth: cellWidth, rowHeight: rowHeight, resizing: true)
                    )
                    .onHover { inside in if inside { NSCursor.resizeLeftRight.push() } else { NSCursor.pop() } }
                    .help("Drag to change note length")
            }
            .frame(width: width, height: rowHeight - 3)
            .contentShape(Rectangle())
            .onTapGesture {
                select(note.id)
                insertionBeat = note.startBeat
            }
            .gesture(noteDrag(note, cellWidth: cellWidth, rowHeight: rowHeight, resizing: false))
            .offset(
                x: (note.startBeat - visibleRange.start) / grid * cellWidth + 1,
                y: Double(11 - note.pitch % 12) * rowHeight + 1
            )
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(
                "\(note.name), beat \((note.startBeat + 1).formatted()), \(note.durationBeats.formatted()) beats"
            )
            .accessibilityValue(selectedNotes.contains(note.id) ? "Selected" : "Note")
            .accessibilityAddTraits(.isButton)
            .accessibilityAction { select(note.id) }
            .contextMenu {
                Button("Copy note") {
                    selectedNotes = [note.id]
                    copyNotes()
                }
                Button("Duplicate note") {
                    selectedNotes = [note.id]
                    duplicateNotes()
                }
                Button("Delete note", role: .destructive) {
                    pattern.notes.removeAll { $0.id == note.id }
                    selectedNotes.remove(note.id)
                }
            }
            .allowsHitTesting(!recordingMIDI)
    }
    private var noteControls: some View {
        HStack(spacing: 18) {
            if let index = selectedIndex {
                Text(selectedNotes.count == 1 ? pattern.notes[index].name : "\(selectedNotes.count) notes")
                    .font(.system(size: 12, weight: .semibold)).frame(width: 65, alignment: .leading)
                parameter(
                    "Velocity", value: noteValue(\.velocity), range: 0.05...1,
                    display: percent(pattern.notes[index].velocity))
                parameter(
                    "Length", value: noteLength, range: 1...max(1, pattern.beats / grid),
                    display: "\(pattern.notes[index].durationBeats.formatted()) beats")
                Picker(
                    "Articulation",
                    selection: Binding(
                        get: { pattern.notes[index].articulation ?? .normal },
                        set: { value in
                            pattern.notes = pattern.notes.map { original in
                                var note = original
                                if selectedNotes.contains(note.id) { note.articulation = value }
                                return note
                            }
                            focusDesigner()
                        })
                ) { ForEach(SoundDesignArticulation.allCases) { Text($0.title).tag($0) } }.labelsHidden().frame(
                    width: 110
                ).accessibilityLabel("Note articulation")
                Button {
                    deleteNotes()
                } label: {
                    Image(systemName: "trash")
                }
                .buttonStyle(RoomButtonStyle()).help("Delete selected notes · Delete").accessibilityLabel(
                    "Delete selected notes")
            } else {
                Text("Select a note").font(.system(size: 12)).foregroundStyle(Theme.secondary)
                Spacer()
            }
        }.frame(height: 38).disabled(recordingMIDI)
    }
    private var layerControls: some View {
        DisclosureGroup("Layers", isExpanded: $layersExpanded) {
            VStack(spacing: 9) {
                HStack {
                    parameter("Base gain", value: $pattern.baseGain, range: 0...1, display: percent(pattern.baseGain))
                    parameter(
                        "Base pan", value: $pattern.basePan, range: -1...1,
                        display: pattern.basePan == 0
                            ? "Center" : "\(Int(abs(pattern.basePan) * 100))% \(pattern.basePan < 0 ? "L" : "R")")
                    Button("Add layer") {
                        if pattern.layers.count < 6 {
                            pattern.layers.append(SoundDesignLayer())
                            focusDesigner()
                        }
                    }
                    .buttonStyle(RoomButtonStyle()).disabled(pattern.layers.count >= 6)
                }
                ForEach(Array(pattern.layers.enumerated()), id: \.element.id) { index, layer in
                    VStack(spacing: 6) {
                        HStack {
                            Toggle("Layer \(index + 1)", isOn: layerBinding(index, \.enabled)).font(.system(size: 11))
                                .frame(width: 84)
                            Picker("Voice", selection: layerBinding(index, \.preset)) {
                                ForEach(SoundDesignPreset.allCases) { Text($0.rawValue).tag($0) }
                            }.frame(width: 166)
                            Stepper(
                                "Transpose \(layer.transpose) st", value: layerBinding(index, \.transpose), in: -24...24
                            ).font(.system(size: 11)).frame(width: 166)
                            Spacer()
                            Button {
                                pattern.layers.remove(at: index)
                                focusDesigner()
                            } label: {
                                Image(systemName: "trash")
                            }.buttonStyle(.plain)
                        }
                        HStack(spacing: 12) {
                            parameter(
                                "Gain", value: layerBinding(index, \.gain), range: 0...1, display: percent(layer.gain))
                            parameter(
                                "Pan", value: layerBinding(index, \.pan), range: -1...1,
                                display: layer.pan == 0
                                    ? "Center" : "\(Int(abs(layer.pan) * 100))% \(layer.pan < 0 ? "L" : "R")")
                            parameter(
                                "Min velocity", value: layerBinding(index, \.minimumVelocity), range: 0...1,
                                display: percent(layer.minimumVelocity))
                            parameter(
                                "Max velocity", value: layerBinding(index, \.maximumVelocity), range: 0...1,
                                display: percent(layer.maximumVelocity))
                        }
                        DisclosureGroup("Tune & timing") {
                            let voice = layer.instrument(for: pattern)
                            HStack(spacing: 18) {
                                parameter(
                                    "Fine tune", value: layerSoundBinding(index, \.fineTune, fallback: 0),
                                    range: -50...50, display: "\(Int(layer.fineTune ?? 0)) cents")
                                parameter(
                                    "Delay", value: layerSoundBinding(index, \.delay, fallback: 0), range: 0...0.5,
                                    display: milliseconds(layer.onsetDelay))
                                parameter(
                                    "Decay", value: layerSoundBinding(index, \.decayOverride, fallback: voice.decay),
                                    range: 0...2, display: milliseconds(voice.decay))
                                parameter(
                                    "Sustain",
                                    value: layerSoundBinding(index, \.sustainOverride, fallback: voice.sustain),
                                    range: 0...1, display: percent(voice.sustain))
                            }.padding(.top, 8)
                            HStack {
                                Picker("Key zone low", selection: layerPitchBinding(index, low: true)) {
                                    ForEach(0...127, id: \.self) { pitch in
                                        Text(SoundDesignNote(pitch: pitch, startBeat: 0).name).tag(pitch)
                                    }
                                }
                                Picker("Key zone high", selection: layerPitchBinding(index, low: false)) {
                                    ForEach(0...127, id: \.self) { pitch in
                                        Text(SoundDesignNote(pitch: pitch, startBeat: 0).name).tag(pitch)
                                    }
                                }
                                Button("Reset tuning") {
                                    pattern.layers[index].fineTune = nil
                                    pattern.layers[index].delay = nil
                                    pattern.layers[index].lowestPitch = nil
                                    pattern.layers[index].highestPitch = nil
                                    pattern.layers[index].decayOverride = nil
                                    pattern.layers[index].sustainOverride = nil
                                }.buttonStyle(.plain)
                            }.padding(.top, 8)
                        }.font(.system(size: 11)).padding(.top, 4)
                        DisclosureGroup("Tone & envelope") {
                            let voice = layer.instrument(for: pattern)
                            HStack(spacing: 18) {
                                parameter(
                                    "Tone", value: layerSoundBinding(index, \.toneOverride, fallback: voice.tone),
                                    range: 0...1, display: percent(voice.tone))
                                parameter(
                                    voice.instrument.characterLabel,
                                    value: layerSoundBinding(index, \.characterOverride, fallback: voice.character),
                                    range: 0...1, display: percent(voice.character))
                                parameter(
                                    "Attack", value: layerSoundBinding(index, \.attackOverride, fallback: voice.attack),
                                    range: 0...2, display: milliseconds(voice.attack))
                                parameter(
                                    "Release",
                                    value: layerSoundBinding(index, \.releaseOverride, fallback: voice.release),
                                    range: 0...2, display: milliseconds(voice.release))
                            }.padding(.top, 8)
                            HStack {
                                parameter(
                                    "Velocity crossfade", value: layerSoundBinding(index, \.velocityFade, fallback: 0),
                                    range: 0...0.5, display: percent(layer.velocityFade ?? 0)
                                )
                                .help("Fade in and out inside the velocity zone instead of switching layers abruptly.")
                                Spacer(minLength: 40)
                                Button("Reset voice") {
                                    pattern.layers[index].toneOverride = nil
                                    pattern.layers[index].characterOverride = nil
                                    pattern.layers[index].attackOverride = nil
                                    pattern.layers[index].releaseOverride = nil
                                    focusDesigner()
                                }.buttonStyle(.plain).font(.system(size: 11))
                            }.padding(.top, 8)
                        }.font(.system(size: 11)).padding(.top, 4)
                    }.padding(10).background(Theme.raised, in: RoundedRectangle(cornerRadius: 6))
                }
            }.padding(.top, 8)
        }.font(.system(size: 12)).padding(10).background(Theme.background, in: RoundedRectangle(cornerRadius: 8))
            .disabled(recordingMIDI)
    }
    private func layerPitchBinding(_ index: Int, low: Bool) -> Binding<Int> {
        Binding(
            get: {
                guard pattern.layers.indices.contains(index) else { return low ? 0 : 127 }
                return low ? pattern.layers[index].lowestPitch ?? 0 : pattern.layers[index].highestPitch ?? 127
            },
            set: { value in
                guard pattern.layers.indices.contains(index) else { return }
                if low {
                    pattern.layers[index].lowestPitch = value
                    pattern.layers[index].highestPitch = max(value, pattern.layers[index].highestPitch ?? 127)
                } else {
                    pattern.layers[index].highestPitch = value
                    pattern.layers[index].lowestPitch = min(value, pattern.layers[index].lowestPitch ?? 0)
                }
            })
    }
    private func layerSoundBinding(_ index: Int, _ key: WritableKeyPath<SoundDesignLayer, Double?>, fallback: Double)
        -> Binding<Double>
    {
        Binding(
            get: {
                pattern.layers.indices.contains(index) ? pattern.layers[index][keyPath: key] ?? fallback : fallback
            },
            set: { value in
                guard pattern.layers.indices.contains(index) else { return }
                pattern.layers[index][keyPath: key] = value
            })
    }
    private func layerBinding<Value>(_ index: Int, _ key: WritableKeyPath<SoundDesignLayer, Value>) -> Binding<Value> {
        Binding(
            get: {
                pattern.layers.indices.contains(index)
                    ? pattern.layers[index][keyPath: key] : SoundDesignLayer()[keyPath: key]
            },
            set: { value in
                guard pattern.layers.indices.contains(index) else { return }
                pattern.layers[index][keyPath: key] = value
                if key == \SoundDesignLayer.minimumVelocity {
                    pattern.layers[index].maximumVelocity = max(
                        pattern.layers[index].minimumVelocity, pattern.layers[index].maximumVelocity)
                }
                if key == \SoundDesignLayer.maximumVelocity {
                    pattern.layers[index].minimumVelocity = min(
                        pattern.layers[index].minimumVelocity, pattern.layers[index].maximumVelocity)
                }
            })
    }
    private func velocityLane(width: Double) -> some View {
        ZStack(alignment: .topLeading) {
            Rectangle().fill(Theme.paper)
            Text("Velocity").font(.system(size: 9)).foregroundStyle(Theme.secondary).padding(3)
            ForEach(
                pattern.notes.filter {
                    $0.startBeat >= visibleRange.start && $0.startBeat < visibleRange.start + visibleRange.span
                }
            ) { note in
                let height = max(2, note.velocity * 40)
                Capsule().fill(selectedNotes.contains(note.id) ? Theme.ink : Theme.accent).frame(
                    width: 7, height: height
                )
                .offset(x: (note.startBeat - visibleRange.start) / visibleRange.span * width, y: 52 - height)
                .contentShape(Rectangle())
                .gesture(
                    DragGesture(minimumDistance: 0, coordinateSpace: .named("designerVelocity")).onChanged { value in
                        if !groupingEdit {
                            groupEdit(true)
                            selectedNotes = [note.id]
                        }
                        guard let index = pattern.notes.firstIndex(where: { $0.id == note.id }) else { return }
                        pattern.notes[index].velocity = max(0.05, min(1, (52 - value.location.y) / 40))
                    }.onEnded { _ in groupEdit(false) }
                )
                .accessibilityLabel("\(note.name) velocity").accessibilityValue("\(Int(note.velocity * 127))")
            }
        }.frame(width: width, height: 56).coordinateSpace(name: "designerVelocity").allowsHitTesting(!recordingMIDI)
    }
    private var controllerControls: some View {
        VStack(spacing: 6) {
            HStack {
                Picker("Controller", selection: $controllerLane) {
                    ForEach(SoundDesignControllerKind.allCases) { Text($0.title).tag($0) }
                }.frame(width: 180)
                Text("\(pattern.controllers.filter { $0.kind == controllerLane }.count) points").font(.system(size: 11))
                    .foregroundStyle(Theme.secondary)
                Spacer()
                Button("Clear lane") {
                    pattern.controllers.removeAll { $0.kind == controllerLane }
                    focusDesigner()
                }.buttonStyle(.plain).font(.system(size: 11))
            }
            GeometryReader { geometry in
                Canvas { context, size in
                    let events = pattern.controllers.filter {
                        $0.kind == controllerLane && $0.beat.isFinite && $0.value.isFinite
                    }.sorted { $0.beat < $1.beat }
                    var path = Path()
                    var lastY = (1 - controllerLane.defaultValue) * (size.height - 8) + 4
                    path.move(to: CGPoint(x: 0, y: lastY))
                    for event in events {
                        if event.beat < visibleRange.start {
                            lastY = (1 - max(0, min(1, event.value))) * (size.height - 8) + 4
                            path = Path()
                            path.move(to: CGPoint(x: 0, y: lastY))
                            continue
                        }
                        if event.beat > visibleRange.start + visibleRange.span { break }
                        let x = max(
                            0, min(size.width, (event.beat - visibleRange.start) / visibleRange.span * size.width))
                        let y = (1 - max(0, min(1, event.value))) * (size.height - 8) + 4
                        path.addLine(to: CGPoint(x: x, y: lastY))
                        path.addLine(to: CGPoint(x: x, y: y))
                        lastY = y
                        context.fill(
                            Path(ellipseIn: CGRect(x: x - 2, y: y - 2, width: 4, height: 4)), with: .color(Theme.ink))
                    }
                    path.addLine(to: CGPoint(x: size.width, y: lastY))
                    context.stroke(path, with: .color(Theme.accent), lineWidth: 1.5)
                }.contentShape(Rectangle()).gesture(
                    DragGesture(minimumDistance: 0).onChanged { value in
                        if !groupingEdit { groupEdit(true) }
                        let beat = max(
                            0,
                            min(
                                pattern.beats,
                                (visibleRange.beat(at: value.location.x / max(1, geometry.size.width)) / grid).rounded()
                                    * grid))
                        let raw = max(0, min(1, 1 - (value.location.y - 4) / max(1, geometry.size.height - 8)))
                        let level = controllerLane == .sustain ? (raw >= 0.5 ? 1.0 : 0.0) : raw
                        pattern.controllers.removeAll { $0.kind == controllerLane && abs($0.beat - beat) < 0.000001 }
                        if pattern.controllers.count < 4096 {
                            pattern.controllers.append(.init(kind: controllerLane, beat: beat, value: level))
                        }
                    }.onEnded { _ in groupEdit(false) })
            }.frame(height: 42).background(Theme.paper, in: RoundedRectangle(cornerRadius: 4))
                .help(
                    "Click or draw controller points on the beat grid. Sustain holds notes; modulation adds vibrato; expression changes level; pitch bend spans ±2 semitones."
                )
                .accessibilityLabel("\(controllerLane.title) controller lane")
        }.padding(10).background(Theme.background, in: RoundedRectangle(cornerRadius: 8)).disabled(recordingMIDI)
    }
    private var footer: some View {
        VStack(spacing: 6) {
            HStack(spacing: 10) {
                Button(action: preview) {
                    Label(previewing ? "Stop" : "Play loop", systemImage: previewing ? "stop.fill" : "play.fill")
                }
                .buttonStyle(RoomButtonStyle()).disabled(pattern.notes.isEmpty && !recordingMIDI && !previewing).help(
                    "Play / Stop · Space"
                ).accessibilityLabel(previewing ? "Stop Designer loop" : "Play Designer loop")
                Picker("MIDI input", selection: midiSource) {
                    Text("Computer keyboard").tag(UInt32(0))
                    ForEach(midi.sources) { Text($0.name).tag($0.id) }
                }.labelsHidden().frame(width: 170).font(.system(size: 11))
                Button {
                    if recordingMIDI { stopMIDIRecording() } else { startMIDIRecording() }
                } label: {
                    Label(
                        recordingMIDI ? "Stop notes" : "Record notes",
                        systemImage: recordingMIDI ? "stop.fill" : "record.circle")
                }.buttonStyle(RoomButtonStyle()).help("Record computer keyboard or MIDI notes · R")
                Button {
                    showingKeyboard.toggle()
                } label: {
                    Label("Keyboard", systemImage: "pianokeys")
                }
                .buttonStyle(RoomButtonStyle()).help("Show keyboard · ⇧⌘K")
                .accessibilityLabel("Show virtual keyboard")
                Spacer()
                Button("Cancel") {
                    stopMIDIRecording()
                    stopPreview()
                    onCancel()
                }
                .buttonStyle(RoomButtonStyle()).keyboardShortcut(.cancelAction)
                if let onInsertNewTrack {
                    Button("Insert as new track") {
                        stopMIDIRecording()
                        stopPreview()
                        onInsertNewTrack(pattern)
                    }
                    .buttonStyle(RoomButtonStyle()).disabled(pattern.notes.isEmpty || commitPending)
                }
                Button(commitPending ? "Inserting…" : isEditing ? "Update sound" : "Insert sound") {
                    stopMIDIRecording()
                    stopPreview()
                    onCommit(pattern)
                }.buttonStyle(RoomButtonStyle(prominent: true)).keyboardShortcut(.defaultAction).disabled(
                    pattern.notes.isEmpty || commitPending)
            }
            Text(commitMessage.isEmpty ? (status.isEmpty ? (midi.error ?? "") : status) : commitMessage).font(
                .system(size: 10)
            ).foregroundStyle(Theme.secondary)
                .frame(maxWidth: .infinity, alignment: .leading).frame(height: 12)
        }
    }

    private var lengthBinding: Binding<Double> {
        Binding(
            get: { pattern.beats },
            set: { value in
                var next = pattern
                next.beats = value
                next.notes = next.notes.filter { $0.startBeat < value }.map { source in
                    var note = source
                    note.durationBeats = min(note.durationBeats, value - note.startBeat)
                    return note
                }
                pattern = next
                insertionBeat = min(insertionBeat, max(0, value - grid))
            })
    }
    private func parameter(_ title: String, value: Binding<Double>, range: ClosedRange<Double>, display: String)
        -> some View
    {
        VStack(alignment: .leading, spacing: 5) {
            HStack {
                Text(title)
                Spacer()
                Text(display).foregroundStyle(Theme.secondary).monospacedDigit()
            }
            .font(.system(size: 10))
            RoomSlider(value: value, in: range, label: title, onEditingChanged: groupEdit).accessibilityLabel(title)
        }.frame(maxWidth: .infinity)
    }
    private func percent(_ value: Double) -> String { "\(Int(value * 100))%" }
    private func milliseconds(_ value: Double) -> String { "\(Int(value * 1_000)) ms" }
    private func groupEdit(_ editing: Bool) {
        if editing && !groupingEdit {
            groupSnapshot = pattern
            undoHistory.append(pattern)
            trimHistory()
            redoHistory.removeAll()
            groupingEdit = true
        } else if !editing {
            if let original = groupSnapshot, original == pattern { _ = undoHistory.popLast() }
            groupSnapshot = nil
            groupingEdit = false
            previousPattern = pattern
            focusDesigner()
        }
    }
    private func trimHistory() { if undoHistory.count > 100 { undoHistory.removeFirst(undoHistory.count - 100) } }
    private func undo() {
        guard !recordingMIDI, let old = undoHistory.popLast() else { return }
        redoHistory.append(pattern)
        historyPlayback = true
        pattern = old
        restoreNoteSelection(old)
    }
    private func redo() {
        guard !recordingMIDI, let next = redoHistory.popLast() else { return }
        undoHistory.append(pattern)
        historyPlayback = true
        pattern = next
        restoreNoteSelection(next)
    }
    private func restoreNoteSelection(_ next: SoundDesignPattern) {
        selectedNotes.formIntersection(Set(next.notes.map(\.id)))
        if selectedNotes.isEmpty, let first = next.notes.first { selectedNotes = [first.id] }
        if let first = next.notes.first(where: { selectedNotes.contains($0.id) }) {
            insertionBeat = first.startBeat
            octave = first.pitch / 12
            ensureInsertionVisible()
        }
    }
    private func focusDesigner() { keyboardFocus = UUID() }
    private func select(_ id: UUID) {
        focusDesigner()
        if NSApp.currentEvent?.modifierFlags.contains(.shift) == true {
            if selectedNotes.contains(id) { selectedNotes.remove(id) } else { selectedNotes.insert(id) }
        } else {
            selectedNotes = [id]
        }
    }
    private func addNote(pitch: Int, beat: Double) {
        guard pattern.notes.count < 256, pitch <= 127 else {
            status = "Pattern limit: 256 notes."
            return
        }
        let note = SoundDesignNote(pitch: pitch, startBeat: beat, durationBeats: min(grid, pattern.beats - beat))
        pattern.notes.append(note)
        selectedNotes = [note.id]
        insertionBeat = beat
        focusDesigner()
    }
    private func deleteNotes() {
        pattern.notes.removeAll { selectedNotes.contains($0.id) }
        selectedNotes.removeAll()
    }
    private func copyNotes() {
        copiedNotes = pattern.notes.filter { selectedNotes.contains($0.id) }
        status = "\(copiedNotes.count) notes copied."
    }
    private func pasteNotes(at target: Double? = nil) {
        guard let first = copiedNotes.map(\.startBeat).min() else { return }
        let origin = max(0, target ?? insertionBeat)
        let additions = copiedNotes.compactMap { source -> SoundDesignNote? in
            let start = origin + source.startBeat - first
            guard start < pattern.beats else { return nil }
            var note = source
            note.id = UUID()
            note.startBeat = start
            note.durationBeats = min(source.durationBeats, pattern.beats - start)
            return note
        }
        guard pattern.notes.count + additions.count <= 256 else {
            status = "Pattern limit: 256 notes."
            return
        }
        pattern.notes.append(contentsOf: additions)
        selectedNotes = Set(additions.map(\.id))
        insertionBeat = origin
        ensureInsertionVisible()
        focusDesigner()
    }
    private func duplicateNotes() {
        copyNotes()
        let end = copiedNotes.map { $0.startBeat + $0.durationBeats }.max() ?? insertionBeat
        pasteNotes(at: end)
    }
    private var noteLength: Binding<Double> {
        Binding(
            get: { selectedIndex.map { pattern.notes[$0].durationBeats / grid } ?? 1 },
            set: { value in
                pattern.notes = pattern.notes.map { note in
                    selectedNotes.contains(note.id)
                        ? SoundDesignEditing.resized(note, to: value * grid, patternBeats: pattern.beats, grid: grid)
                        : note
                }
            })
    }
    private func noteValue(_ path: WritableKeyPath<SoundDesignNote, Double>) -> Binding<Double> {
        Binding(
            get: { selectedIndex.map { pattern.notes[$0][keyPath: path] } ?? 0.8 },
            set: { value in
                for index in pattern.notes.indices where selectedNotes.contains(pattern.notes[index].id) {
                    pattern.notes[index][keyPath: path] = value
                }
            })
    }
    private func noteDrag(_ note: SoundDesignNote, cellWidth: Double, rowHeight: Double, resizing: Bool) -> some Gesture
    {
        DragGesture(minimumDistance: 3).onChanged { value in
            if gesturePattern == nil {
                if !selectedNotes.contains(note.id) { selectedNotes = [note.id] }
                gesturePattern = pattern
                groupEdit(true)
            }
            guard var original = gesturePattern else { return }
            let beats = Double(value.translation.width) / cellWidth * grid
            if resizing, let index = original.notes.firstIndex(where: { $0.id == note.id }) {
                original.notes[index] = SoundDesignEditing.resized(
                    original.notes[index], to: original.notes[index].durationBeats + beats, patternBeats: pattern.beats,
                    grid: grid)
            } else {
                // Keep the initiating note view mounted until gesture completion.
                // Crossing octaves is available through the keyboard transpose controls.
                let visible = original.notes.filter { selectedNotes.contains($0.id) && $0.pitch / 12 == octave }
                let low = visible.map(\.pitch).min() ?? note.pitch
                let high = visible.map(\.pitch).max() ?? note.pitch
                let desired = -Int((Double(value.translation.height) / rowHeight).rounded())
                let semitones = max(octave * 12 - low, min(octave * 12 + 11 - high, desired))
                original.notes = SoundDesignEditing.moved(
                    original.notes, ids: selectedNotes, by: beats,
                    semitones: semitones, patternBeats: pattern.beats, grid: grid)
            }
            pattern = original
        }.onEnded { _ in
            gesturePattern = nil
            groupEdit(false)
        }
    }
    private func releaseKeyboardNotes() {
        for pitch in liveKeys.releaseLocal() { endLiveNote(pitch) }
    }
    private func handleKey(_ event: NSEvent) -> Bool {
        if event.type == .keyUp {
            let source = DesignerHeldKeys.Source.computer(event.keyCode)
            guard liveKeys.pitch(for: source) != nil else { return false }
            if let pitch = liveKeys.release(source) { endLiveNote(pitch) }
            return true
        }
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let command = flags.contains(.command)
        let shift = flags.contains(.shift)
        let option = flags.contains(.option)
        let key = event.charactersIgnoringModifiers?.lowercased() ?? ""
        if command {
            if key == "k" && shift {
                if !event.isARepeat { showingKeyboard.toggle() }
                return true
            }
            guard !recordingMIDI else { return ["z", "a", "c", "x", "v", "d"].contains(key) }
            switch key {
            case "=", "+": zoomViewport(0.5)
            case "-": zoomViewport(2)
            case "0": viewport = DesignerViewport(span: pattern.beats)
            case "z": shift ? redo() : undo()
            case "a": selectedNotes = Set(pattern.notes.map(\.id))
            case "c": copyNotes()
            case "x":
                copyNotes()
                deleteNotes()
            case "v": pasteNotes()
            case "d": duplicateNotes()
            default: return false
            }
            return true
        }
        if key == "q" && shift && !recordingMIDI {
            pattern.notes = SoundDesignEditing.quantized(pattern.notes, grid: grid, patternBeats: pattern.beats)
            return true
        }
        if key == "h" && option && !recordingMIDI {
            pattern.notes = SoundDesignEditing.humanized(
                pattern.notes, ids: selectedNotes.isEmpty ? nil : selectedNotes, patternBeats: pattern.beats,
                seed: humanizePass)
            humanizePass &+= 1
            return true
        }
        if event.keyCode == 53 && showingKeyboard {
            showingKeyboard = false
            return true
        }
        if event.keyCode == 49 {
            if !event.isARepeat { preview() }
            return true
        }
        if [51, 117].contains(event.keyCode) {
            if !recordingMIDI { deleteNotes() }
            return true
        }
        if [123, 124, 125, 126].contains(event.keyCode) {
            guard !recordingMIDI else { return true }
            if shift && [123, 124].contains(event.keyCode) {
                pattern.notes = pattern.notes.map { source in
                    selectedNotes.contains(source.id)
                        ? SoundDesignEditing.resized(
                            source, to: source.durationBeats + (event.keyCode == 123 ? -grid : grid),
                            patternBeats: pattern.beats, grid: grid) : source
                }
            } else {
                let beats = event.keyCode == 123 ? -grid : event.keyCode == 124 ? grid : 0
                let semitones =
                    event.keyCode == 125 ? (option ? -12 : -1) : event.keyCode == 126 ? (option ? 12 : 1) : 0
                pattern.notes = SoundDesignEditing.moved(
                    pattern.notes, ids: selectedNotes, by: beats, semitones: semitones, patternBeats: pattern.beats,
                    grid: grid)
                if let index = selectedIndex {
                    insertionBeat = pattern.notes[index].startBeat
                    octave = pattern.notes[index].pitch / 12
                    ensureInsertionVisible()
                }
            }
            return true
        }
        if key == "n" && !recordingMIDI {
            addNote(
                pitch: selectedIndex.map { pattern.notes[$0].pitch } ?? octave * 12,
                beat: min(insertionBeat, max(0, pattern.beats - grid)))
            return true
        }
        if key == "r" {
            if recordingMIDI { stopMIDIRecording() } else { startMIDIRecording() }
            return true
        }
        if key == "z" {
            octave = max(0, octave - 1)
            return true
        }
        if key == "x" {
            octave = min(10, octave + 1)
            return true
        }
        if DesignerTypingKeyboard.keys.contains(key) {
            if !event.isARepeat, let pitch = DesignerTypingKeyboard.pitch(for: key, octave: octave),
                liveKeys.press(.computer(event.keyCode), pitch: pitch)
            {
                beginLiveNote(pitch, velocity: Int(keyboardVelocity))
            }
            return true
        }
        return false
    }
    private func allDesignerNotesOff() {
        remotePerformance.allNotesOff()
    }
    private func releaseDesignerNote(_ pitch: Int) {
        remotePerformance.noteOff(pitch)
    }
    private func preview() {
        focusDesigner()
        if recordingMIDI {
            stopMIDIRecording()
            stopPreview()
            return
        }
        if previewing {
            stopPreview()
            return
        }
        allDesignerNotesOff()
        liveKeys.clear()
        previewStarted = ProcessInfo.processInfo.systemUptime
        status = previewReady == nil ? "" : "Preparing sound…"
        onPreview(pattern)
        previewing = true
        schedulePreviewEnd(duration: pattern.beats * 60 / max(30, pattern.bpm) + pattern.performanceRelease)
    }
    private func audition(
        pitch: Int, velocity: Double = 0.8, hold: Bool = false, restoreFocus: Bool = true,
        performanceBeat: Double? = nil
    ) {
        if restoreFocus { focusDesigner() }
        remotePerformance.noteOn(pattern, pitch, velocity, performanceBeat)
    }
    private func schedulePreviewEnd(duration: Double) {
        previewTask?.cancel()
        let end = max(0.1, min(130, duration))
        let requested = ProcessInfo.processInfo.systemUptime
        previewTask = Task { @MainActor in
            if let ready = previewReady {
                while !ready() && !Task.isCancelled {
                    if ProcessInfo.processInfo.systemUptime - requested > 30 {
                        status = "Sound preview did not start. Check the selected audio output."
                        stopPreview()
                        return
                    }
                    try? await Task.sleep(for: .milliseconds(50))
                }
            }
            guard !Task.isCancelled else { return }
            status = ""
            let started = ProcessInfo.processInfo.systemUptime
            while !Task.isCancelled {
                // Engine scheduling provides a short safety lead before its first buffer.
                let elapsed = max(0, ProcessInfo.processInfo.systemUptime - started - (previewReady == nil ? 0 : 0.12))
                if let ready = previewReady {
                    if !ready() {
                        stopPreview()
                        return
                    }
                } else if elapsed >= end {
                    stopPreview()
                    return
                }
                try? await Task.sleep(for: .milliseconds(40))
            }
        }
    }
    private func stopPreview() {
        previewTask?.cancel()
        previewTask = nil
        if previewing { onStop() }
        previewing = false
        allDesignerNotesOff()
        liveKeys.clear()
    }
    private func startMIDIRecording() {
        stopPreview()
        liveKeys.clear()
        groupEdit(true)
        heldNotes.removeAll()
        for kind in SoundDesignControllerKind.allCases {
            pattern.controllers.removeAll { $0.kind == kind && $0.beat == 0 }
            pattern.controllers.append(.init(kind: kind, beat: 0, value: controllerValues[kind] ?? kind.defaultValue))
        }
        recordingMIDI = true
        recordStart = ProcessInfo.processInfo.systemUptime
        status = "Recording keyboard / MIDI notes. Space stops."
        recordingTask = Task { @MainActor in
            while !Task.isCancelled && recordingMIDI {
                try? await Task.sleep(for: .milliseconds(40))
                guard !Task.isCancelled else { return }
                let beat = min(pattern.beats, (ProcessInfo.processInfo.systemUptime - recordStart) * pattern.bpm / 60)
                if beat >= pattern.beats {
                    stopMIDIRecording()
                    return
                }
            }
        }
    }
    private func receiveMIDIController(_ kind: SoundDesignControllerKind, _ value: Double) {
        controllerValues[kind] = value
        guard recordingMIDI, pattern.controllers.count < 4096 else { return }
        let beat = min(pattern.beats, max(0, (ProcessInfo.processInfo.systemUptime - recordStart) * pattern.bpm / 60))
        if let last = pattern.controllers.last, last.kind == kind, abs(last.value - value) < 1 / 127,
            beat - last.beat < 0.04
        {
            return
        }
        pattern.controllers.append(SoundDesignController(kind: kind, beat: beat, value: value))
        controllerLane = kind
    }
    private func receivePointerNote(_ pitch: Int, _ on: Bool) {
        if on {
            if liveKeys.press(.pointer(pitch), pitch: pitch) { beginLiveNote(pitch, velocity: Int(keyboardVelocity)) }
        } else if let released = liveKeys.release(.pointer(pitch)) {
            endLiveNote(released)
        }
    }
    private func receiveMIDINote(_ pitch: Int, _ velocity: Int, _ on: Bool) {
        if on && velocity > 0 {
            if liveKeys.press(.midi(pitch), pitch: pitch) {
                beginLiveNote(pitch, velocity: velocity, auditionNote: false)
            }
        } else if let released = liveKeys.release(.midi(pitch)) {
            endLiveNote(released, releaseAudio: false)
        }
    }
    private func endLiveNote(_ pitch: Int, releaseAudio: Bool = true) {
        if releaseAudio { releaseDesignerNote(pitch) }
        if recordingMIDI { finishMIDINote(pitch) }
    }
    private func beginLiveNote(_ pitch: Int, velocity: Int, auditionNote: Bool = true) {
        if recordingMIDI {
            let beat = (ProcessInfo.processInfo.systemUptime - recordStart) * pattern.bpm / 60
            if beat >= pattern.beats {
                stopMIDIRecording()
                return
            }
            if heldNotes[pitch] != nil { finishMIDINote(pitch) }
            heldNotes[pitch] = (beat, Double(velocity) / 127)
        }
        if auditionNote {
            audition(
                pitch: pitch, velocity: Double(velocity) / 127, hold: true, restoreFocus: false,
                performanceBeat: recordingMIDI ? heldNotes[pitch]?.beat : nil)
        }
    }
    private func finishMIDINote(_ pitch: Int) {
        guard let start = heldNotes.removeValue(forKey: pitch), pattern.notes.count < 256 else { return }
        let end = min(pattern.beats, (ProcessInfo.processInfo.systemUptime - recordStart) * pattern.bpm / 60)
        guard end > start.beat else { return }
        pattern.notes.append(
            SoundDesignNote(
                pitch: pitch, velocity: start.velocity, startBeat: start.beat,
                durationBeats: end - start.beat))
        selectedNotes = [pattern.notes.last!.id]
    }
    private func stopMIDIRecording() {
        guard recordingMIDI else { return }
        recordingTask?.cancel()
        recordingTask = nil
        for pitch in Array(heldNotes.keys) { finishMIDINote(pitch) }
        for kind in SoundDesignControllerKind.allCases {
            if kind == .sustain && (pattern.controllers.last(where: { $0.kind == kind })?.value ?? 0) >= 0.5 {
                pattern.controllers.append(
                    .init(
                        kind: .sustain,
                        beat: min(
                            pattern.beats, (ProcessInfo.processInfo.systemUptime - recordStart) * pattern.bpm / 60),
                        value: 0))
            }
        }
        allDesignerNotesOff()
        liveKeys.clear()
        recordingMIDI = false
        groupEdit(false)
        if !selectedNotes.isEmpty { revealSelectedNotes() }
        status = "Notes recorded."
    }
    private func acceptMIDI(_ imported: SoundDesignPattern) {
        let next = DesignerMIDIImport.replacingSequence(in: pattern, with: imported)
        pattern = next
        selectedNotes = Set(next.notes.prefix(1).map(\.id))
        octave = max(0, min(10, (next.notes.first?.pitch ?? 60) / 12))
        insertionBeat = 0
        viewport = DesignerViewport().fitted(to: next.beats)
        workspace = 0
        status =
            "Imported \(next.notes.count) \(next.notes.count == 1 ? "note" : "notes") · \(next.beats.formatted(.number.precision(.fractionLength(0...2)))) \(next.beats == 1 ? "beat" : "beats") · \(next.bpm.formatted(.number.precision(.fractionLength(0...1)))) BPM"
        focusDesigner()
    }
    private func importMIDI() {
        stopPreview()
        releaseKeyboardNotes()
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.midi]
        panel.allowsMultipleSelection = false
        panel.title = "Import MIDI notes"
        panel.prompt = "Import"
        guard let window = designerWindow.view?.window ?? NSApp.keyWindow else {
            importError = "The designer window is unavailable. Close and reopen the designer, then import again."
            return
        }
        panel.beginSheetModal(for: window) { response in
            guard response == .OK, let url = panel.url else {
                focusDesigner()
                return
            }
            importingMIDI = true
            status = "Reading MIDI…"
            importTask = Task { @MainActor in
                do {
                    let content = try await Task.detached(priority: .userInitiated) {
                        let scoped = url.startAccessingSecurityScopedResource()
                        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                        return try MIDIFile.inspect(Data(contentsOf: url, options: .mappedIfSafe))
                    }.value
                    guard !Task.isCancelled else { return }
                    importingMIDI = false
                    if content.parts.count == 1, let imported = try? content.pattern() {
                        acceptMIDI(imported)
                    } else {
                        status = "Choose a MIDI part and beat range."
                        importReview = DesignerMIDIReviewRequest(name: url.lastPathComponent, content: content)
                    }
                } catch {
                    guard !Task.isCancelled else { return }
                    importingMIDI = false
                    status = ""
                    importError = error.localizedDescription
                }
            }
        }
    }
    private func exportMIDI() {
        stopPreview()
        releaseKeyboardNotes()
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.midi]
        panel.nameFieldStringValue = "Sound design.mid"
        guard let window = designerWindow.view?.window ?? NSApp.keyWindow else { return }
        panel.beginSheetModal(for: window) { response in
            guard response == .OK, let url = panel.url else { return }
            do {
                try MIDIFile.encode(pattern).write(to: url, options: .atomic)
                status = "MIDI exported."
            } catch { status = error.localizedDescription }
        }
    }
}

private struct DesignerMIDIReviewRequest: Identifiable {
    let id = UUID()
    let name: String
    let content: MIDIFileContent
}

private struct DesignerMIDIReviewSheet: View {
    let request: DesignerMIDIReviewRequest
    let onImport: (SoundDesignPattern) -> Void
    let onCancel: () -> Void
    @State private var partID: Int
    @State private var startBeat = 0.0
    @State private var length: Double
    init(
        request: DesignerMIDIReviewRequest, onImport: @escaping (SoundDesignPattern) -> Void,
        onCancel: @escaping () -> Void
    ) {
        self.request = request
        self.onImport = onImport
        self.onCancel = onCancel
        _partID = State(initialValue: request.content.parts.first?.id ?? -1)
        _length = State(initialValue: min(64, request.content.beats))
    }
    private var result: Result<SoundDesignPattern, Error> {
        Result { try request.content.pattern(partID: partID < 0 ? nil : partID, startBeat: startBeat, length: length) }
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Import MIDI notes").font(.system(size: 20, weight: .semibold))
            Text(request.name).font(.system(size: 12)).foregroundStyle(Theme.secondary).lineLimit(1)
            Picker("Part", selection: $partID) {
                ForEach(request.content.parts) { part in Text("\(part.name) · \(part.notes.count) notes").tag(part.id) }
                if request.content.parts.count > 1 { Text("All parts").tag(-1) }
            }.accessibilityLabel("MIDI import part")
            HStack(spacing: 18) {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Start beat (from zero)").font(.system(size: 11))
                    CommittedNumberField(
                        "Start beat", value: $startBeat, range: 0...max(0, request.content.beats - 0.25),
                        fractionDigits: 2
                    )
                    .textFieldStyle(.roundedBorder).accessibilityLabel("MIDI import start beat")
                }
                VStack(alignment: .leading, spacing: 6) {
                    Text("Length in beats").font(.system(size: 11))
                    CommittedNumberField(
                        "Length", value: $length, range: 0.25...min(64, request.content.beats), fractionDigits: 2
                    )
                    .textFieldStyle(.roundedBorder).accessibilityLabel("MIDI import length in beats")
                }
            }
            Text("The current instrument plays the imported notes. MIDI instrument programs do not replace your sound.")
                .font(.system(size: 12)).foregroundStyle(Theme.secondary).fixedSize(horizontal: false, vertical: true)
            Group {
                switch result {
                case .success(let pattern):
                    Text(
                        "\(pattern.notes.count) \(pattern.notes.count == 1 ? "note" : "notes") · \(pattern.bpm.formatted(.number.precision(.fractionLength(0...1)))) BPM"
                    ).foregroundStyle(Theme.accent)
                case .failure(let error): Text(error.localizedDescription).foregroundStyle(Theme.secondary)
                }
            }.font(.system(size: 12)).fixedSize(horizontal: false, vertical: true).frame(
                minHeight: 34, alignment: .topLeading)
            HStack {
                Spacer()
                Button("Cancel", action: onCancel).buttonStyle(RoomButtonStyle()).keyboardShortcut(.cancelAction)
                Button("Replace notes") { if case .success(let pattern) = result { onImport(pattern) } }
                    .buttonStyle(RoomButtonStyle(prominent: true)).keyboardShortcut(.defaultAction)
                    .disabled(
                        {
                            if case .success = result { return false }
                            return true
                        }())
            }
        }.padding(20).frame(width: 490).background(Theme.paper).foregroundStyle(Theme.ink).preferredColorScheme(.dark)
            .onChange(of: startBeat) { _, value in length = min(length, request.content.beats - value) }
    }
}

/// Sheet-local interception keeps musical shortcuts away from the main editor, while
/// leaving text fields and nested file panels to normal AppKit key handling.
private struct DesignerKeyboardBridge: NSViewRepresentable {
    let onKey: (NSEvent) -> Bool
    let onRelease: () -> Void
    let focusRequest: UUID
    let windowReference: DesignerWindowReference
    final class Coordinator {
        weak var view: NSView?
        var onKey: (NSEvent) -> Bool
        var onRelease: () -> Void
        var resignationObserver: Any?
        var monitor: Any?
        var lastFocus: UUID?
        init(_ onKey: @escaping (NSEvent) -> Bool, onRelease: @escaping () -> Void) {
            self.onKey = onKey
            self.onRelease = onRelease
        }
        func install() {
            monitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .keyUp]) { [weak self] event in
                guard let self, let window = self.view?.window else { return event }
                // Release already-held keys even after focus moves into a text field.
                if event.type == .keyUp { return self.onKey(event) ? nil : event }
                guard
                    DesignerKeyboardRouting.accepts(
                        eventWindow: event.window, designerWindow: window,
                        isKey: window.isKeyWindow, hasNestedSheet: window.attachedSheet != nil,
                        responder: window.firstResponder)
                else { return event }
                return self.onKey(event) ? nil : event
            }
            resignationObserver = NotificationCenter.default.addObserver(
                forName: NSWindow.didResignKeyNotification, object: nil, queue: .main
            ) { [weak self] notification in
                guard let self, let window = notification.object as? NSWindow, window === self.view?.window else {
                    return
                }
                self.onRelease()
            }
        }
    }
    func makeCoordinator() -> Coordinator { Coordinator(onKey, onRelease: onRelease) }
    func makeNSView(context: Context) -> NSView {
        let view = DesignerFocusView()
        context.coordinator.view = view
        windowReference.view = view
        context.coordinator.install()
        return view
    }
    func updateNSView(_ nsView: NSView, context: Context) {
        context.coordinator.onKey = onKey
        context.coordinator.onRelease = onRelease
        if context.coordinator.lastFocus != focusRequest {
            context.coordinator.lastFocus = focusRequest
            DispatchQueue.main.async { [weak nsView] in
                guard let nsView, let window = nsView.window, window.attachedSheet == nil else { return }
                window.makeFirstResponder(nsView)
            }
        }
    }
    static func dismantleNSView(_ nsView: NSView, coordinator: Coordinator) {
        if let monitor = coordinator.monitor { NSEvent.removeMonitor(monitor) }
        coordinator.monitor = nil
        if let observer = coordinator.resignationObserver { NotificationCenter.default.removeObserver(observer) }
        coordinator.resignationObserver = nil
    }
}

private final class DesignerFocusView: NSView {
    override var acceptsFirstResponder: Bool { true }
    // This invisible view owns keyboard focus, never mouse tracking in the
    // editor or its floating keyboard.
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        DispatchQueue.main.async { [weak self] in
            guard let self, let window = self.window, window.attachedSheet == nil else { return }
            window.makeFirstResponder(self)
        }
    }
}

/// AppKit identities keep a main-window event, a text editor, or a nested file dialog
/// from triggering musical commands. Kept separate so these focus regressions are testable.
enum DesignerKeyboardRouting {
    static func accepts(
        eventWindow: NSWindow?, designerWindow: NSWindow?, isKey: Bool,
        hasNestedSheet: Bool, responder: NSResponder?
    ) -> Bool {
        guard let designerWindow, let eventWindow else { return false }
        return isKey && eventWindow === designerWindow && !hasNestedSheet && !(responder is NSTextView)
    }
}

private final class DesignerWindowReference { weak var view: NSView? }
