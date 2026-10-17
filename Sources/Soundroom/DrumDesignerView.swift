import AppKit
import SoundroomCore
import SwiftUI
import UniformTypeIdentifiers

struct DrumDesignerSheet: View {
    @StateObject private var editor: DrumEditorModel
    @ObservedObject var controller: DrumDesignerController
    let isEditing: Bool
    let onCommit: (DrumPattern) -> Void
    let onInsertNewTrack: ((DrumPattern) -> Void)?
    let isCurrentWorkspace: () -> Bool
    @State private var focus = UUID()
    @State private var status = ""
    init(
        pattern: DrumPattern, controller: DrumDesignerController, isEditing: Bool,
        onCommit: @escaping (DrumPattern) -> Void, onInsertNewTrack: ((DrumPattern) -> Void)? = nil,
        isCurrentWorkspace: @escaping () -> Bool = { true }
    ) {
        _editor = StateObject(wrappedValue: DrumEditorModel(pattern: pattern))
        self.controller = controller
        self.isEditing = isEditing
        self.onCommit = onCommit
        self.onInsertNewTrack = onInsertNewTrack
        self.isCurrentWorkspace = isCurrentWorkspace
    }
    private var pattern: DrumPattern { editor.pattern }
    private var lane: DrumLane { pattern.lanes[editor.selectedVoice.rawValue] ?? DrumLane(kit: pattern.kit) }
    private var currentKitTitle: String {
        if let id = pattern.customKitID { return kitLibrary.kits.first { $0.id == id }?.name ?? "Your kit" }
        return pattern.kit.title
    }
    @StateObject private var kitLibrary = DrumKitLibrary(owner: nil)
    @State private var designingKit = false
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            header
            patternControls
            timingControls
            variationControls
            grid
            selectedStepControls
            laneControls
            Divider().overlay(Theme.line)
            footer
        }
        .padding(18).frame(width: 1080, height: 770)
        .sheet(isPresented: $designingKit) {
            DrumKitDesignerView(
                library: kitLibrary, controller: controller,
                editing: pattern.customKitID.flatMap { id in kitLibrary.kits.first { $0.id == id } },
                onUse: { design in
                    editor.change { $0.use(design) }
                    designingKit = false
                },
                onClose: { designingKit = false })
        }
        .onAppear {
            kitLibrary.owner = controller.owner
            kitLibrary.refresh()
        }
        .foregroundStyle(Theme.ink).background(Theme.paper).tint(Theme.accent).preferredColorScheme(.dark)
        .background(DrumKeyboardBridge(onKey: handleKey, focusRequest: focus))
        .onChange(of: editor.document) { _, _ in
            guard isCurrentWorkspace() else { return }
            controller.draftChanged()
            if controller.working { return }
            if controller.previewing && controller.isLoopingPreview {
                controller.preview(pattern)
            } else {
                controller.stop()
            }
        }
        .onDisappear { if isCurrentWorkspace() { controller.stop() } }
    }
    private var header: some View {
        HStack {
            Text("Drum loop designer").font(.system(size: 21, weight: .semibold, design: .rounded))
            Spacer()
            Button {
                editor.undo()
                refocus()
            } label: {
                Image(systemName: "arrow.uturn.backward")
            }.help("Undo · ⌘Z").disabled(editor.undoHistory.isEmpty)
            Button {
                editor.redo()
                refocus()
            } label: {
                Image(systemName: "arrow.uturn.forward")
            }.help("Redo · ⇧⌘Z").disabled(editor.redoHistory.isEmpty)
            Button {
                controller.stop()
                designingKit = true
            } label: {
                Label("Kit designer…", systemImage: "slider.horizontal.3")
            }
            .help("Design your own kit piece by piece; saved kits appear in the Kit menu")
            Menu("Grooves") {
                ForEach(DrumPatternPreset.Category.allCases) { category in
                    Section(category.rawValue) {
                        ForEach(DrumPatternPreset.allCases.filter { $0.category == category }) { preset in
                            Button(preset.rawValue) {
                                editor.change { $0.apply(preset) }
                                refocus()
                            }
                        }
                    }
                }
            }.frame(width: 110).help("Load a groove into the current pattern")
        }.buttonStyle(RoomButtonStyle()).disabled(controller.working)
    }
    private var variationControls: some View {
        HStack(spacing: 20) {
            Picker(
                "Performance",
                selection: Binding(
                    get: { pattern.variationCycles ?? 1 },
                    set: { value in
                        editor.change { $0.variationCycles = value }
                        refocus()
                    })
            ) {
                Text("One pass").tag(1)
                Text("2 variations").tag(2)
                Text("4 variations").tag(4)
                Text("8 variations").tag(8)
            }.frame(width: 200).help(
                "Render a repeatable performance with different strikes, dynamics and timing on each pass")
            VStack(spacing: 2) {
                Text("Velocity variation \(Int((pattern.velocityHumanize ?? 0) * 100))%")
                RoomSlider(
                    value: Binding(
                        get: { pattern.velocityHumanize ?? 0 },
                        set: { value in editor.change { $0.velocityHumanize = value } }), in: 0...0.2,
                    label: "Velocity variation", onEditingChanged: gesture)
            }.frame(width: 180)
            Spacer()
            Text("8 strike variations · continuous velocity").font(.system(size: 11)).foregroundStyle(Theme.secondary)
        }.font(.system(size: 12)).disabled(controller.working)
    }
    private var patternControls: some View {
        HStack(spacing: 12) {
            // Your patterns in this bank (switch, add, save); grooves are presets on the right.
            Menu {
                ForEach(Array(editor.document.patterns.enumerated()), id: \.offset) { index, value in
                    Button {
                        editor.selectPattern(index)
                        refocus()
                    } label: {
                        if index == editor.document.selected {
                            Label(value.name, systemImage: "checkmark")
                        } else {
                            Text(value.name)
                        }
                    }
                }
                Divider()
                Button("New pattern") {
                    editor.newPattern()
                    refocus()
                }
                Button("Duplicate pattern") {
                    editor.newPattern(duplicate: true)
                    refocus()
                }
                Divider()
                Button("Save patterns…", action: savePatterns)
                Button("Open patterns…", action: openPatterns)
            } label: {
                Label(
                    "\(pattern.name) (\(editor.document.selected + 1)/\(editor.document.patterns.count))",
                    systemImage: "square.stack")
            }.frame(width: 190).help("Your patterns: switch, add, duplicate, save or open")
            TextField(
                "Name",
                text: Binding(
                    get: { pattern.name }, set: { new in editor.change { $0.name = String(new.prefix(120)) } })
            )
            .textFieldStyle(.roundedBorder).frame(width: 164)
            Menu {
                ForEach(DrumKit.allCases) { kit in
                    Button {
                        editor.change {
                            $0.kit = kit
                            $0.lanes = [:]
                            $0.variationCycles = kit.isAcoustic ? 4 : 1
                            $0.velocityHumanize = kit.isAcoustic ? 0.035 : 0
                        }
                        refocus()
                    } label: {
                        if pattern.customKitID == nil && pattern.kit == kit {
                            Label(kit.title, systemImage: "checkmark")
                        } else {
                            Text(kit.title)
                        }
                    }
                }
                Section("Your kits") {
                    ForEach(kitLibrary.kits) { design in
                        Button {
                            editor.change { $0.use(design) }
                            refocus()
                        } label: {
                            if pattern.customKitID == design.id {
                                Label(design.name, systemImage: "checkmark")
                            } else {
                                Text(design.name)
                            }
                        }
                    }
                    Button("Kit designer…") {
                        controller.stop()
                        designingKit = true
                    }
                }
            } label: {
                Text(currentKitTitle)
            }
            .frame(width: 215).help("Set all sounds to a kit. Individual sounds can use a different kit below.")
            Spacer()
            Button("Copy", action: copyPattern).help("Copy pattern · ⌘C")
            Button("Paste", action: pastePattern).help("Paste pattern · ⌘V")
        }.font(.system(size: 12)).disabled(controller.working)
    }
    private var timingControls: some View {
        HStack(spacing: 16) {
            Text("BPM").foregroundStyle(Theme.secondary)
            CommittedNumberField(
                "BPM", value: parameter(\.bpm, range: 30...300), range: 30...300, onEditingChanged: gesture,
                onSubmit: refocus
            )
            .textFieldStyle(.roundedBorder).frame(width: 55)
            Picker(
                "Length",
                selection: Binding(
                    get: { pattern.bars },
                    set: { value in
                        editor.change { $0.resizeBars(value) }
                        refocus()
                    })
            ) {
                ForEach([1, 2, 4, 8], id: \.self) { Text("\($0) \($0 == 1 ? "bar" : "bars")").tag($0) }
            }.frame(width: 112)
            Picker("Grid", selection: Binding(get: { pattern.stepsPerBeat }, set: changeGrid)) {
                Text("1/8").tag(2)
                Text("1/16").tag(4)
                Text("1/32").tag(8)
            }.frame(width: 112)
            VStack(spacing: 2) {
                Text("Swing \(Int(pattern.swing * 100))%")
                RoomSlider(
                    value: parameter(\.swing, range: 0...0.7), in: 0...0.7, label: "Swing", onEditingChanged: gesture)
            }.frame(width: 124)
            VStack(spacing: 2) {
                Text("Humanize \(Int(pattern.humanize * 1000)) ms")
                RoomSlider(
                    value: parameter(\.humanize, range: 0...0.025), in: 0...0.025, label: "Humanize timing",
                    onEditingChanged: gesture)
            }.frame(width: 128)
            VStack(spacing: 2) {
                Text("Level \(Int(pattern.masterGain * 100))%")
                RoomSlider(
                    value: parameter(\.masterGain, range: 0...1), in: 0...1, label: "Drum master level",
                    onEditingChanged: gesture)
            }.frame(width: 124)
        }.font(.system(size: 11)).disabled(controller.working)
    }
    private var grid: some View {
        SwiftUI.TimelineView(.animation(minimumInterval: 1.0 / 30)) { _ in
            let playStep =
                controller.previewing
                ? Int(
                    controller.elapsed.truncatingRemainder(dividingBy: pattern.duration) / pattern.duration
                        * Double(pattern.stepCount)) : -1
            HStack(spacing: 8) {
                VStack(spacing: 5) {
                    Text("Audition").font(.system(size: 10)).foregroundStyle(Theme.secondary).frame(height: 18)
                    ForEach(DrumVoice.allCases) { voice in
                        HStack(spacing: 5) {
                            Button {
                                editor.selectedVoice = voice
                                controller.preview(pattern, audition: voice)
                                refocus()
                            } label: {
                                Image(systemName: "speaker.wave.2").frame(width: 18)
                            }.buttonStyle(.plain).help("Audition \(voice.title)")
                            Button(voice.title) {
                                editor.selectedVoice = voice
                                refocus()
                            }.buttonStyle(.plain).frame(width: 82, alignment: .leading)
                        }.font(.system(size: 12, weight: editor.selectedVoice == voice ? .semibold : .regular))
                            .foregroundStyle(editor.selectedVoice == voice ? Theme.accent : Theme.ink).frame(height: 27)
                    }
                }.frame(width: 112)
                ScrollView(.horizontal) {
                    VStack(spacing: 5) {
                        HStack(spacing: 5) {
                            ForEach(0..<pattern.stepCount, id: \.self) { index in
                                Text(index % pattern.stepsPerBeat == 0 ? "\(index / pattern.stepsPerBeat + 1)" : "·")
                                    .font(.system(size: 10)).foregroundStyle(
                                        index == playStep ? Theme.accent : Theme.secondary
                                    ).frame(width: 27, height: 18)
                            }
                        }
                        ForEach(DrumVoice.allCases) { voice in
                            HStack(spacing: 5) {
                                ForEach(0..<pattern.stepCount, id: \.self) { index in stepButton(voice, index, playStep)
                                }
                            }
                        }
                    }.padding(.bottom, 6)
                }
            }
        }.frame(height: 325).disabled(controller.working)
    }
    private func stepButton(_ voice: DrumVoice, _ index: Int, _ playStep: Int) -> some View {
        let step = pattern.steps.first { $0.voice == voice && $0.index == index }
        let selected = editor.selectedVoice == voice && editor.selectedStep == index
        return Button {
            if NSEvent.modifierFlags.contains(.option) {
                editor.selectedVoice = voice
                editor.selectedStep = index
            } else {
                editor.toggle(voice: voice, index: index)
            }
            refocus()
        } label: {
            RoundedRectangle(cornerRadius: 4)
                .fill(
                    step == nil
                        ? (index % pattern.stepsPerBeat == 0 ? Theme.line : Theme.raised)
                        : Theme.accent.opacity(0.35 + (step?.velocity ?? 0) * 0.65)
                )
                .overlay(alignment: .bottom) {
                    if let step {
                        Capsule().fill(Theme.background.opacity(0.6)).frame(
                            width: 17, height: max(2, step.velocity * 15)
                        ).padding(.bottom, 3)
                    }
                }
                .overlay(
                    RoundedRectangle(cornerRadius: 4).stroke(
                        selected ? Theme.ink : index == playStep ? Theme.accent : Color.clear, lineWidth: 1.5)
                )
                .frame(width: 27, height: 27)
        }.buttonStyle(.plain).help(
            "\(voice.title), step \(index + 1). Click toggles. Option-click selects for editing."
        )
        .accessibilityLabel("\(voice.title) step \(index + 1)")
        .accessibilityValue(step == nil ? "Off" : "On, velocity \(Int((step?.velocity ?? 0) * 127))")
        .contextMenu {
            Button("Select") {
                editor.selectedVoice = voice
                editor.selectedStep = index
                refocus()
            }
            ForEach([0.3, 0.6, 0.8, 1.0], id: \.self) { velocity in
                Button("Velocity \(Int(velocity * 127))") {
                    editor.selectedVoice = voice
                    editor.selectedStep = index
                    if step == nil { editor.toggle(voice: voice, index: index) }
                    editor.setStep { $0.velocity = velocity }
                    refocus()
                }
            }
            Button("Remove") {
                editor.selectedVoice = voice
                editor.selectedStep = index
                editor.removeSelected()
                refocus()
            }.disabled(step == nil)
        }
    }
    private var selectedStepControls: some View {
        HStack(spacing: 14) {
            Text("\(editor.selectedVoice.title) · Step \(editor.selectedStep + 1)").font(
                .system(size: 12, weight: .semibold)
            ).frame(width: 125, alignment: .leading)
            Picker(
                "Articulation",
                selection: Binding(
                    get: { editor.currentStep?.articulation ?? .center },
                    set: { value in
                        editor.setStep { $0.articulation = value }
                        refocus()
                    })
            ) {
                ForEach(DrumArticulation.available(for: editor.selectedVoice)) { Text($0.title).tag($0) }
            }.frame(width: 160)
            Text("Velocity \(Int((editor.currentStep?.velocity ?? 0) * 127))").font(.system(size: 11)).frame(width: 78)
            RoomSlider(value: stepParameter(\.velocity), in: 0...1, label: "Step velocity", onEditingChanged: gesture)
                .frame(width: 120)
            Text("Pan").font(.system(size: 11))
            RoomSlider(value: stepParameter(\.pan), in: -1...1, label: "Step pan", onEditingChanged: gesture).frame(
                width: 80)
            Text("Chance \(Int((editor.currentStep?.probability ?? 1) * 100))%").font(.system(size: 11))
            RoomSlider(
                value: stepParameter(\.probability), in: 0...1, label: "Step probability", onEditingChanged: gesture
            ).frame(width: 95)
            Button {
                editor.removeSelected()
                refocus()
            } label: {
                Image(systemName: "trash")
            }.buttonStyle(.plain)
        }.disabled(editor.currentStep == nil || controller.working)
    }
    private var laneControls: some View {
        HStack(spacing: 12) {
            Text("Sound").font(.system(size: 12, weight: .semibold))
            Picker(
                "Kit",
                selection: Binding(
                    get: { lane.kit },
                    set: { value in
                        editor.setLane { $0.kit = value }
                        refocus()
                    })
            ) {
                ForEach(DrumKit.allCases) { kit in Text(kit.title).tag(kit) }
            }.frame(width: 198)
            VStack(spacing: 2) {
                Text("Damping \(Int((lane.damping ?? 0.5) * 100))%").font(.system(size: 11))
                RoomSlider(
                    value: Binding(
                        get: { lane.damping ?? 0.5 }, set: { value in editor.setLane { $0.damping = value } }),
                    in: 0...1, label: "Drum damping", onEditingChanged: gesture)
            }.frame(width: 110)
            Text("Tune \(lane.tuning.formatted(.number.precision(.fractionLength(1)))) st").font(.system(size: 11))
                .frame(width: 86)
            RoomSlider(value: laneParameter(\.tuning), in: -12...12, label: "Lane tuning", onEditingChanged: gesture)
                .frame(width: 112)
            Text("Gain").font(.system(size: 11))
            RoomSlider(value: laneParameter(\.gain), in: 0...1.5, label: "Lane gain", onEditingChanged: gesture).frame(
                width: 104)
            Text("Pan").font(.system(size: 11))
            RoomSlider(value: laneParameter(\.pan), in: -1...1, label: "Lane pan", onEditingChanged: gesture).frame(
                width: 92)
            Toggle(
                "Mute",
                isOn: Binding(
                    get: { lane.muted },
                    set: { value in
                        editor.setLane { $0.muted = value }
                        refocus()
                    })
            ).font(.system(size: 11))
        }.disabled(controller.working)
    }
    private var footer: some View {
        HStack(spacing: 12) {
            Button {
                (controller.previewing || controller.preparing) ? controller.stop() : controller.preview(pattern)
                refocus()
            } label: {
                Label(
                    controller.previewing || controller.preparing ? "Stop" : "Play loop",
                    systemImage: controller.previewing || controller.preparing ? "stop.fill" : "play.fill")
            }.buttonStyle(RoomButtonStyle()).help("Play / Stop · Space").disabled(
                pattern.steps.isEmpty || controller.working)
            Button("Export WAV…", action: exportWAV).buttonStyle(RoomButtonStyle()).disabled(
                pattern.steps.isEmpty || controller.working)
            Text(controller.message.isEmpty ? status : controller.message).font(.system(size: 11)).foregroundStyle(
                Theme.secondary
            ).lineLimit(2)
            Spacer()
            Button("Cancel") { if isCurrentWorkspace() { controller.close() } }.buttonStyle(RoomButtonStyle())
            if let onInsertNewTrack {
                Button("Insert as new track") { onInsertNewTrack(pattern) }.buttonStyle(RoomButtonStyle()).disabled(
                    pattern.steps.isEmpty || controller.working)
            }
            Button(isEditing ? "Update loop" : "Insert loop") { onCommit(pattern) }.buttonStyle(
                RoomButtonStyle(prominent: true)
            ).disabled(pattern.steps.isEmpty || controller.working)
        }
    }
    private func parameter(_ key: WritableKeyPath<DrumPattern, Double>, range: ClosedRange<Double>) -> Binding<Double> {
        Binding(
            get: { pattern[keyPath: key] },
            set: { value in
                editor.change {
                    $0[keyPath: key] =
                        value.isFinite ? min(range.upperBound, max(range.lowerBound, value)) : range.lowerBound
                }
            })
    }
    private func stepParameter(_ key: WritableKeyPath<DrumStep, Double>) -> Binding<Double> {
        Binding(
            get: { editor.currentStep?[keyPath: key] ?? 0 },
            set: { value in editor.setStep { $0[keyPath: key] = value } })
    }
    private func laneParameter(_ key: WritableKeyPath<DrumLane, Double>) -> Binding<Double> {
        Binding(get: { lane[keyPath: key] }, set: { value in editor.setLane { $0[keyPath: key] = value } })
    }
    private func gesture(_ changing: Bool) {
        if changing {
            editor.beginGesture()
        } else {
            editor.endGesture()
            refocus()
        }
    }
    private func refocus() { focus = UUID() }
    private func changeGrid(_ divisions: Int) {
        editor.change { pattern in
            let old = pattern.stepsPerBeat
            pattern.stepsPerBeat = divisions
            var remapped: [String: DrumStep] = [:]
            for var step in pattern.steps {
                step.index = Int((Double(step.index * divisions) / Double(old)).rounded())
                guard step.index < pattern.stepCount else { continue }
                let key = "\(step.voice.rawValue)-\(step.index)"
                if let existing = remapped[key], existing.velocity > step.velocity { continue }
                remapped[key] = step
            }
            pattern.steps = remapped.values.sorted { $0.index < $1.index }
        }
        refocus()
    }
    private func copyPattern() {
        do {
            let data = try editor.copyPattern()
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setData(data, forType: .init("org.soundroom.drum-pattern"))
            status = "Pattern copied"
        } catch { status = error.localizedDescription }
        refocus()
    }
    private func pastePattern() {
        do {
            guard let data = NSPasteboard.general.data(forType: .init("org.soundroom.drum-pattern")) else {
                status = "Copy a drum pattern first"
                return
            }
            try editor.pastePattern(data)
            status = "Pattern pasted"
        } catch { status = error.localizedDescription }
        refocus()
    }
    private func savePatterns() {
        controller.stop()
        let panel = NSSavePanel()
        panel.allowedContentTypes = [UTType(filenameExtension: "soundroomdrums") ?? .json]
        panel.nameFieldStringValue = pattern.name + ".soundroomdrums"
        guard let window = NSApp.keyWindow else { return }
        panel.beginSheetModal(for: window) { response in
            guard response == .OK, let url = panel.url else { return }
            do {
                try editor.saveDocument().write(to: url, options: .atomic)
                status = "Patterns saved"
            } catch { status = error.localizedDescription }
            refocus()
        }
    }
    private func openPatterns() {
        controller.stop()
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [UTType(filenameExtension: "soundroomdrums") ?? .json, .json]
        panel.allowsMultipleSelection = false
        guard let window = NSApp.keyWindow else { return }
        panel.beginSheetModal(for: window) { response in
            guard response == .OK, let url = panel.url else { return }
            do {
                guard (try url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0) <= 2_000_000 else {
                    throw DrumDocumentError.invalid
                }
                try editor.loadDocument(Data(contentsOf: url))
                status = "Patterns opened"
            } catch { status = error.localizedDescription }
            refocus()
        }
    }
    private func exportWAV() {
        controller.stop()
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.wav]
        panel.nameFieldStringValue = pattern.name + ".wav"
        guard let window = NSApp.keyWindow else { return }
        let pattern = pattern
        panel.beginSheetModal(for: window) { response in
            guard response == .OK, let url = panel.url else { return }
            guard let remote = controller.owner?.remoteGUI else {
                status = "Connect to the audio server before exporting."
                return
            }
            remote.exportDrumPattern(pattern, to: url) { message in
                status = message
                refocus()
            }
        }
    }
    private func handleKey(_ event: NSEvent) -> Bool {
        guard event.type == .keyDown, !controller.working else { return false }
        let modifiers = event.modifierFlags.intersection([.command, .control, .option, .shift])
        if modifiers.contains(.command) {
            switch event.keyCode {
            case 6: modifiers.contains(.shift) ? editor.redo() : editor.undo()
            case 8: copyPattern()
            case 9: pastePattern()
            case 2: editor.newPattern(duplicate: true)
            case 1: savePatterns()
            default: return false
            }
            return true
        }
        guard modifiers.intersection([.command, .control, .option]).isEmpty else { return false }
        switch event.keyCode {
        case 49: (controller.previewing || controller.preparing) ? controller.stop() : controller.preview(pattern)
        case 51, 117: editor.removeSelected()
        case 123: editor.selectedStep = max(0, editor.selectedStep - 1)
        case 124: editor.selectedStep = min(pattern.stepCount - 1, editor.selectedStep + 1)
        case 125, 126:
            let current = DrumVoice.allCases.firstIndex(of: editor.selectedVoice) ?? 0
            editor.selectedVoice =
                DrumVoice.allCases[min(DrumVoice.allCases.count - 1, max(0, current + (event.keyCode == 125 ? 1 : -1)))]
        case 36: editor.toggle(voice: editor.selectedVoice, index: editor.selectedStep)
        case 53: controller.stop()
        default: return false
        }
        return true
    }
}
private struct DrumKeyboardBridge: NSViewRepresentable {
    let onKey: (NSEvent) -> Bool
    let focusRequest: UUID
    final class Coordinator {
        weak var view: NSView?
        var onKey: (NSEvent) -> Bool
        var monitor: Any?
        var lastFocus: UUID?
        init(_ onKey: @escaping (NSEvent) -> Bool) { self.onKey = onKey }
        func install() {
            monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
                guard let self, let window = self.view?.window,
                    DesignerKeyboardRouting.accepts(
                        eventWindow: event.window, designerWindow: window, isKey: window.isKeyWindow,
                        hasNestedSheet: window.attachedSheet != nil, responder: window.firstResponder)
                else { return event }
                return self.onKey(event) ? nil : event
            }
        }
    }
    func makeCoordinator() -> Coordinator { Coordinator(onKey) }
    func makeNSView(context: Context) -> NSView {
        let view = DrumFocusView()
        context.coordinator.view = view
        context.coordinator.install()
        return view
    }
    func updateNSView(_ view: NSView, context: Context) {
        context.coordinator.onKey = onKey
        if context.coordinator.lastFocus != focusRequest {
            context.coordinator.lastFocus = focusRequest
            DispatchQueue.main.async { [weak view] in
                if let view, let window = view.window, window.attachedSheet == nil { window.makeFirstResponder(view) }
            }
        }
    }
    static func dismantleNSView(_ view: NSView, coordinator: Coordinator) {
        if let monitor = coordinator.monitor { NSEvent.removeMonitor(monitor) }
    }
}
private final class DrumFocusView: NSView { override var acceptsFirstResponder: Bool { true } }
