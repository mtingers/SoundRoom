import AppKit
import SoundroomCore
import SwiftUI

/// Pin an inspector to its source so a delayed number/preset callback cannot edit
/// another track, replacement insert, or newly selected audio range.
enum EffectWorkspaceTarget: Equatable, Identifiable {
    case selection(trackID: UUID?, kind: BuiltInEffect, range: Range<Int>?)
    case realtime(trackID: UUID, effectID: UUID, kind: RealtimeEffectKind)

    var id: String {
        switch self {
        case let .selection(track, kind, range):
            return
                "selection.\(track?.uuidString ?? "none").\(kind.rawValue).\(range?.lowerBound ?? -1).\(range?.upperBound ?? -1)"
        case let .realtime(track, effect, kind): return "realtime.\(track).\(effect).\(kind.rawValue)"
        }
    }
    func matches(
        mode: EffectMode, selectedTrackID: UUID?, selectionKind: BuiltInEffect,
        selectionRange: Range<Int>?, effects: [RealtimeEffect]
    ) -> Bool {
        switch self {
        case let .selection(track, kind, range):
            return mode == .selection && selectedTrackID == track && selectionKind == kind && selectionRange == range
        case let .realtime(track, id, kind):
            return (mode == .realtime || mode == .master) && selectedTrackID == track
                && effects.contains { $0.id == id && $0.kind == kind }
        }
    }
    @MainActor func matches(_ store: SessionStore) -> Bool {
        matches(
            mode: store.effectMode, selectedTrackID: store.selectedTrack, selectionKind: store.effect,
            selectionRange: store.selectedRange, effects: store.currentTrack?.realtimeEffects ?? [])
    }
    var isSelection: Bool {
        if case .selection = self { return true }
        return false
    }
    var kind: RealtimeEffectKind? {
        switch self {
        case let .selection(_, kind, _): return kind.realtimeKind
        case let .realtime(_, _, kind): return kind
        }
    }
    var trackID: UUID? {
        switch self {
        case let .selection(track, _, _): return track
        case let .realtime(track, _, _): return track
        }
    }
    var rawValue: String {
        switch self {
        case let .selection(_, kind, _): return kind.rawValue
        case let .realtime(_, _, kind): return kind.rawValue
        }
    }
    var name: String {
        switch self {
        case let .selection(_, kind, _): return kind.name
        case let .realtime(_, _, kind): return kind.name
        }
    }
    @MainActor func effect(_ store: SessionStore) -> RealtimeEffect? {
        guard matches(store), case let .realtime(_, id, _) = self else { return nil }
        return store.currentTrack?.realtimeEffects.first { $0.id == id }
    }
}

@MainActor enum EffectWorkspaceFocus {
    /// Blurring a committed numeric field stages its edit; dispatch after the
    /// next main-loop turn so presets, previews, apply and Undo see that edit.
    static func finishNumberEntry(_ action: @escaping () -> Void) {
        finishNumberEntry(in: NSApp.keyWindow, action)
    }
    static func finishNumberEntry(in window: NSWindow?, _ action: @escaping () -> Void) {
        guard let window, window.firstResponder is NSTextView else {
            action()
            return
        }
        // NSHostingView need not accept first responder. Resign to the window
        // itself instead, so FocusState ends the draft before reset or preview.
        window.makeFirstResponder(nil)
        DispatchQueue.main.async { action() }
    }
}

/// Drawer and precision sheet share these callbacks and read current document
/// values. Expanded controls are an explicit upgrade for legacy processors.
struct EffectWorkspaceControls: View {
    @ObservedObject var store: SessionStore
    @ObservedObject private var library: EffectPresetLibrary
    let target: EffectWorkspaceTarget
    var onNumericSubmit: () -> Void = {}
    @State private var dialog: EffectPresetDialog?
    init(store: SessionStore, target: EffectWorkspaceTarget, onNumericSubmit: @escaping () -> Void = {}) {
        self.store = store
        self.target = target
        self.onNumericSubmit = onNumericSubmit
        self.library = store.effectPresets
    }
    private var effect: RealtimeEffect? { target.effect(store) }
    private var legacy: Bool {
        target.isSelection
            ? store.effectParameters.isEmpty && store.effect.usesLegacyAmountProcessor
            : effect?.usesLegacyProcessor == true
    }
    private var amount: Float { target.isSelection ? store.amount : effect?.amount ?? 0.5 }
    private var values: [String: Float] {
        target.isSelection ? store.selectionParametersForEditing : effect?.parametersForEditing ?? [:]
    }
    var body: some View {
        if target.matches(store), let kind = target.kind {
            VStack(alignment: .leading, spacing: 12) {
                if legacy {
                    HStack {
                        Text("Legacy processor").font(.system(size: 11, weight: .medium))
                        Spacer()
                        Menu("Presets") {
                            Button("Default") {
                                EffectWorkspaceFocus.finishNumberEntry { apply(kind.defaultParameters) }
                            }
                            ForEach(kind.presets) { preset in
                                Button(preset.name) { EffectWorkspaceFocus.finishNumberEntry { apply(preset.values) } }
                            }
                            if !library.presets(for: kind).isEmpty {
                                Section("My Presets") {
                                    ForEach(library.presets(for: kind)) { preset in
                                        Button(preset.name) {
                                            EffectWorkspaceFocus.finishNumberEntry { applySaved(preset) }
                                        }
                                    }
                                }
                            }
                            Divider()
                            Button("Save Preset…") {
                                EffectWorkspaceFocus.finishNumberEntry { if target.matches(store) { dialog = .save } }
                            }.disabled(library.loadError != nil)
                            Button("Manage Presets…") {
                                EffectWorkspaceFocus.finishNumberEntry { if target.matches(store) { dialog = .manage } }
                            }
                        }.menuStyle(.borderlessButton).accessibilityLabel("\(kind.name) presets")
                    }
                    HStack {
                        Text("Amount")
                        Spacer()
                        Text(kind.parameterDisplay(amount)).monospacedDigit()
                    }.font(.system(size: 11))
                    RoomSlider(
                        value: Binding(get: { Double(amount) }, set: { setAmount(Float($0)) }), in: 0...1,
                        label: "\(kind.name) legacy amount",
                        onEditingChanged: { editing in continuous("amount", editing: editing) })
                    Button("Use expanded controls") {
                        guard target.matches(store), !store.busy, !store.isRecording else { return }
                        if target.isSelection {
                            store.enableSelectionParameterControls()
                        } else if let effect {
                            store.changeRealtimeEffect(effect.id, name: "Expand effect controls") {
                                $0.enableParameterControls()
                            }
                        }
                        onNumericSubmit()
                    }.buttonStyle(RoomButtonStyle()).font(.system(size: 11))
                    Text("Keeps the original processor until you upgrade or choose a preset.")
                        .font(.system(size: 10)).foregroundStyle(Theme.secondary).fixedSize(
                            horizontal: false, vertical: true
                        )
                        .help(
                            "Expanded controls use the current processor and can change the sound. Live insert upgrades can be undone; selection settings are temporary."
                        )
                } else {
                    EffectParameterControls(
                        library: library, kind: kind, values: values, onChange: setParameter,
                        onEditingChanged: { continuous($0, editing: $1) }, onNumericSubmit: onNumericSubmit,
                        onPreset: apply, onGraphChange: setGraphParameters, onSavedPreset: applySaved
                    ).id(target.id)
                    if let effect,
                        [.compressor, .gate, .limiter, .resonance, .vocalEnhancer, .sourceIsolator].contains(
                            effect.kind)
                    {
                        ProfessionalEffectMeterView(
                            kind: effect.kind, snapshot: { store.effectMeterSnapshot(effect.id) })
                    }
                }
            }.sheet(item: $dialog) { action in
                switch action {
                case .save:
                    SaveEffectPresetSheet(
                        library: library, kind: kind, values: values, legacyAmount: legacy ? amount : nil)
                case .manage:
                    ManageEffectPresetsSheet(
                        library: library, kind: kind, values: values, legacyAmount: legacy ? amount : nil)
                }
            }
        }
    }
    private func setParameter(_ key: String, _ value: Float) {
        guard target.matches(store), !store.busy, !store.isRecording else { return }
        if target.isSelection {
            store.setSelectionParameter(key, value: value)
        } else if let effect {
            store.changeRealtimeEffect(effect.id, name: "Effect \(key)", coalescingKey: "\(effect.id).\(key)") {
                $0.enableParameterControls()
                $0.parameters[key] = value
            }
        }
    }
    private func setGraphParameters(_ edits: [String: Float]) {
        guard target.matches(store), !store.busy, !store.isRecording else { return }
        if target.isSelection {
            store.effectParameters = values.merging(edits) { _, new in new }
            if store.isPreviewing { store.refreshEffectPreview() }
        } else if let effect {
            store.changeRealtimeEffect(
                effect.id, name: target.kind == .soundPutty ? "Sculpt Sound Putty" : "EQ graph",
                coalescingKey: "\(effect.id).graph"
            ) {
                $0.enableParameterControls()
                $0.parameters.merge(edits) { _, new in new }
            }
        }
    }
    private func setAmount(_ value: Float) {
        guard target.matches(store), !store.busy, !store.isRecording else { return }
        if target.isSelection {
            store.amount = value
            if store.isPreviewing { store.refreshEffectPreview() }
        } else if let effect {
            store.changeRealtimeEffect(effect.id, name: "Effect amount", coalescingKey: "\(effect.id).amount") {
                $0.amount = value
            }
        }
    }
    private func continuous(_ key: String, editing: Bool) {
        if !editing {
            store.endContinuousEdit()
            return
        }
        guard target.matches(store), let effect else { return }
        store.beginContinuousEdit("\(effect.id).\(key)")
    }
    private func apply(_ parameters: [String: Float]) {
        guard target.matches(store), !store.busy, !store.isRecording else { return }
        store.endContinuousEdit()
        if target.isSelection {
            store.effectParameters = parameters
            if store.isPreviewing { store.refreshEffectPreview() }
        } else if let effect {
            store.changeRealtimeEffect(effect.id, name: "Effect preset") {
                $0.parameters = parameters
                if $0.selectedAutomationParameter == nil {
                    $0.selectedAutomationParameter = $0.kind.parameters.first?.id
                }
            }
        }
        onNumericSubmit()
    }
    private func applySaved(_ preset: SavedEffectPreset) {
        guard target.matches(store), !store.busy, !store.isRecording else { return }
        store.endContinuousEdit()
        if target.isSelection {
            store.applySelectionPreset(preset)
        } else if let effect {
            store.applyRealtimePreset(preset, to: effect.id)
        }
        onNumericSubmit()
    }
}

struct SelectionUtilityCard: View {
    let effect: BuiltInEffect
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(effect.detail).font(.system(size: 12)).foregroundStyle(Theme.secondary).fixedSize(
                horizontal: false, vertical: true)
            if effect == .fadeIn || effect == .fadeOut {
                Canvas { context, size in
                    var line = Path()
                    line.move(to: CGPoint(x: 0, y: effect == .fadeIn ? size.height - 4 : 4))
                    line.addLine(to: CGPoint(x: size.width, y: effect == .fadeIn ? 4 : size.height - 4))
                    context.stroke(line, with: .color(Theme.accent), lineWidth: 2)
                }.frame(height: 52).accessibilityLabel(
                    effect == .fadeIn
                        ? "Linear envelope from silence to full level" : "Linear envelope from full level to silence")
                HStack {
                    Text(effect == .fadeIn ? "Silence" : "Full level")
                    Spacer()
                    Text(effect == .fadeIn ? "Full level" : "Silence")
                }.font(.system(size: 10)).foregroundStyle(Theme.secondary)
            } else if effect == .normalize {
                HStack {
                    Text("Peak target")
                    Spacer()
                    Text("−1 dBFS").foregroundStyle(Theme.accent).monospacedDigit()
                }.font(.system(size: 12))
                Text("One gain change across every selected channel; dynamics stay intact.").font(.system(size: 11))
                    .foregroundStyle(Theme.secondary).fixedSize(horizontal: false, vertical: true)
            } else if effect == .reverse {
                Label("End → start", systemImage: "arrow.left").font(.system(size: 13)).foregroundStyle(Theme.accent)
                Text("Keeps the selection’s position and duration.").font(.system(size: 11)).foregroundStyle(
                    Theme.secondary)
            }
        }.padding(14).frame(maxWidth: .infinity, alignment: .leading).background(
            Theme.raised, in: RoundedRectangle(cornerRadius: 8))
    }
}

struct SelectionEffectActions: View {
    @ObservedObject var store: SessionStore
    var valid = true
    var onPreview: (() -> Void)? = nil
    var isValid: () -> Bool = { true }
    var beforeAction: (@escaping () -> Void) -> Void = { EffectWorkspaceFocus.finishNumberEntry($0) }
    var body: some View {
        VStack(spacing: 10) {
            EditingScopeView(scope: store.editingScope)
            HStack {
                Toggle(
                    "Original",
                    isOn: Binding(
                        get: { store.compareOriginal },
                        set: { value in
                            beforeAction {
                                guard valid && isValid() else { return }
                                store.compareOriginal = value
                                store.refreshEffectPreview()
                            }
                        })
                ).toggleStyle(.checkbox)
                    .help("Compare the selected source audio without this selection effect")
                Spacer()
                if [BuiltInEffect.echo, .room, .chorus].contains(store.effect) {
                    Toggle(
                        "Tail",
                        isOn: Binding(
                            get: { store.includeEffectTail },
                            set: { value in
                                beforeAction {
                                    guard valid && isValid() else { return }
                                    store.includeEffectTail = value
                                    store.refreshEffectPreview()
                                }
                            })
                    ).toggleStyle(.checkbox)
                        .disabled(store.effectParameters.isEmpty && store.effect.usesLegacyAmountProcessor)
                        .help(
                            store.effectParameters.isEmpty && store.effect.usesLegacyAmountProcessor
                                ? "Use expanded controls to include decay; the legacy processor keeps the selection length."
                                : "Include the effect’s decay after the selection")
                }
            }.font(.system(size: 11))
            HStack(spacing: 8) {
                Button {
                    beforeAction {
                        if valid && isValid() { if let onPreview { onPreview() } else { store.previewEffect() } }
                    }
                } label: {
                    Label(
                        store.isPreviewing ? "Stop" : "Preview",
                        systemImage: store.isPreviewing ? "stop.fill" : "play.fill"
                    ).frame(maxWidth: .infinity)
                }.buttonStyle(RoomButtonStyle())
                Button {
                    beforeAction { if valid && isValid() { store.applyEffect() } }
                } label: {
                    Text("Apply to audio").frame(maxWidth: .infinity)
                }.buttonStyle(RoomButtonStyle(prominent: true)).help(
                    "Process selected audio. Undo and clip history retain the original.")
            }.font(.system(size: 11)).disabled(!valid || !store.hasAudioSelection || store.busy || store.isRecording)
        }.disabled(!valid || store.busy || store.isRecording)
    }
}

struct EffectAutomationStatus: View {
    let effect: RealtimeEffect
    var mode: AutomationRecordMode = .read
    private var count: Int { effect.automatedParameterIDs.count }
    var body: some View {
        HStack(spacing: 5) {
            if mode != .read {
                Label("\(mode.rawValue) enabled", systemImage: "record.circle").foregroundStyle(Theme.accent)
            } else if count > 0 {
                Label("\(count) automated", systemImage: "point.3.connected.trianglepath.dotted").foregroundStyle(
                    Theme.secondary)
            } else {
                Text("Baseline controls").foregroundStyle(Theme.secondary)
            }
        }.font(.system(size: 10)).help(
            "Controls and response plots show baseline settings. Automation lanes can override them during playback and export."
        )
    }
}

struct EffectPrecisionSheet: View {
    @ObservedObject var store: SessionStore
    let target: EffectWorkspaceTarget
    var onAutomation: () -> Void = {}
    @Environment(\.dismiss) private var dismiss
    @State private var ownsTransport = false
    private var valid: Bool { target.matches(store) }
    private var effect: RealtimeEffect? { target.effect(store) }
    private var trackName: String {
        if target.trackID == Session.masterTrackID { return "Master" }
        return store.session.tracks.first { $0.id == target.trackID }?.name ?? "Audio"
    }
    private var editorHeight: (minimum: CGFloat, preferred: CGFloat) {
        switch target.kind {
        case .gain: return (400, 460)
        case .lowPass, .highPass: return (500, 600)
        case .dither: return (520, 620)
        case nil: return (380, 440)
        default: return (580, 720)
        }
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 10) {
                EffectIdentity(name: target.name, rawValue: target.rawValue)
                Spacer()
                Button {
                    finishNumberEntry { if store.canUndo && !store.busy && !store.isRecording { store.undo() } }
                } label: {
                    Image(systemName: "arrow.uturn.backward")
                }
                .disabled(!store.canUndo || store.busy || store.isRecording).help("Undo document edit (⌘Z)")
                .accessibilityLabel("Undo")
                Button {
                    finishNumberEntry { if store.canRedo && !store.busy && !store.isRecording { store.redo() } }
                } label: {
                    Image(systemName: "arrow.uturn.forward")
                }
                .disabled(!store.canRedo || store.busy || store.isRecording).help("Redo document edit (⇧⌘Z)")
                .accessibilityLabel("Redo")
                Button("Done") {
                    finishNumberEntry {
                        cleanup()
                        dismiss()
                    }
                }.keyboardShortcut(.cancelAction).disabled(store.busy)
            }.font(.system(size: 16, weight: .medium))
            VStack(alignment: .leading, spacing: 5) {
                Text(target.isSelection ? "Selection · \(trackName)" : "Live insert · \(trackName)").font(
                    .system(size: 12, weight: .medium))
                Text(
                    target.isSelection
                        ? "Preview first, then apply to selected audio."
                        : "Processes playback and export. Changes are saved with the session."
                )
                .font(.system(size: 11)).foregroundStyle(Theme.secondary)
                if let effect {
                    EffectAutomationStatus(
                        effect: effect, mode: store.automationWrite ? .write : store.automationEditing.mode)
                }
            }
            Divider()
            ScrollView {
                if valid {
                    if target.kind != nil {
                        EffectWorkspaceControls(store: store, target: target, onNumericSubmit: focusSheet).padding(
                            .bottom, 8)
                    } else if case let .selection(_, kind, _) = target {
                        SelectionUtilityCard(effect: kind)
                    }
                } else {
                    ContentUnavailableView(
                        "Source changed", systemImage: "arrow.triangle.2.circlepath",
                        description: Text("Close this editor and reopen the effect for the current track or selection.")
                    )
                }
            }.frame(maxWidth: .infinity, maxHeight: .infinity).disabled(store.busy || store.isRecording)
            Divider()
            if store.busy {
                HStack {
                    if let progress = store.workProgress {
                        ProgressView(value: progress).frame(maxWidth: .infinity).accessibilityLabel(
                            "Effect processing progress")
                    } else {
                        ProgressView().controlSize(.small).accessibilityLabel("Effect processing progress")
                    }
                    Text(store.status).font(.system(size: 11)).foregroundStyle(Theme.secondary).lineLimit(1)
                    if store.canCancelBackgroundWork { Button("Cancel") { store.cancelBackgroundWork() } }
                }
            }
            if target.isSelection {
                SelectionEffectActions(
                    store: store, valid: valid, onPreview: preview, isValid: { target.matches(store) },
                    beforeAction: finishNumberEntry)
                Text("Selection settings are temporary. Undo restores applied audio edits.").font(.system(size: 10))
                    .foregroundStyle(Theme.secondary)
            } else {
                HStack {
                    if let effect {
                        Button {
                            finishNumberEntry { bypass() }
                        } label: {
                            Label(effect.enabled ? "Bypass" : "Enable", systemImage: "power")
                        }.buttonStyle(RoomButtonStyle())
                    }
                    if let id = target.trackID, id != Session.masterTrackID {
                        Button {
                            finishNumberEntry {
                                ownsTransport = true
                                store.toggleEffectAudition(trackID: id)
                            }
                        } label: {
                            Label(
                                store.isPlaying && store.auditionTrackID == id ? "Pause track" : "Play track",
                                systemImage: "headphones")
                        }.buttonStyle(RoomButtonStyle())
                    }
                    Button {
                        finishNumberEntry { playMix() }
                    } label: {
                        Label(
                            store.isPlaying && store.auditionTrackID == nil ? "Pause mix" : "Play mix",
                            systemImage: store.isPlaying ? "pause.fill" : "play.fill")
                    }.buttonStyle(RoomButtonStyle(prominent: true))
                    if let id = target.trackID, id != Session.masterTrackID, let effect {
                        ApplyLiveEffectsButton(
                            store: store, trackID: id, effectID: effect.id,
                            onApplied: {
                                cleanup()
                                dismiss()
                            })
                    }
                    Spacer()
                    Button("Automation…") {
                        finishNumberEntry {
                            guard valid else { return }
                            cleanup()
                            dismiss()
                            DispatchQueue.main.async { onAutomation() }
                        }
                    }
                }.font(.system(size: 12)).disabled(!valid || store.busy || store.isRecording)
            }
        }.padding(24).frame(
            minWidth: 560, idealWidth: 640, minHeight: editorHeight.minimum, idealHeight: editorHeight.preferred
        )
        .background(Theme.paper).foregroundStyle(Theme.ink).tint(Theme.accent).preferredColorScheme(.dark)
        .background(
            EffectPrecisionKeyboardBridge(
                onSpace: {
                    if valid {
                        if target.isSelection { preview() } else if store.isPlaying { store.pause() } else { playMix() }
                    }
                },
                onUndo: { if store.canUndo && !store.busy && !store.isRecording { store.undo() } },
                onRedo: { if store.canRedo && !store.busy && !store.isRecording { store.redo() } }
            ).frame(width: 0, height: 0)
        )
        .onAppear { DispatchQueue.main.async { focusSheet() } }
        .onDisappear { cleanup() }
        // The drawer is disabled during a worker; Cancel in this sheet
        // remains reachable, while local controls keep their own guards.
        .disabled(false).environment(\.isEnabled, true)
    }
    private func focusSheet() {
        guard let window = NSApp.keyWindow, window.sheetParent != nil else { return }
        window.makeFirstResponder(nil)
    }
    private func finishNumberEntry(_ action: @escaping () -> Void) { EffectWorkspaceFocus.finishNumberEntry(action) }
    private func preview() {
        guard valid, !store.busy, !store.isRecording else { return }
        if !store.isPreviewing { ownsTransport = true }
        store.previewEffect()
    }
    private func playMix() {
        guard valid, !store.busy, !store.isRecording else { return }
        if !store.isPlaying { ownsTransport = true }
        ownsTransport = true
        store.toggleEffectAudition(trackID: nil)
    }
    private func bypass() {
        guard valid, let effect, !store.busy, !store.isRecording else { return }
        store.endContinuousEdit()
        store.changeRealtimeEffect(effect.id, name: "Bypass effect") { $0.enabled.toggle() }
    }
    private func cleanup() {
        store.endContinuousEdit()
        if ownsTransport && (store.isPlaying || store.isPreviewing) { store.stopTransport() }
        ownsTransport = false
    }
}

/// Route only this sheet’s keys, preserving native text undo and control Space.
private struct EffectPrecisionKeyboardBridge: NSViewRepresentable {
    var onSpace: () -> Void
    var onUndo: () -> Void
    var onRedo: () -> Void
    final class Coordinator {
        weak var view: NSView?
        var onSpace: () -> Void
        var onUndo: () -> Void
        var onRedo: () -> Void
        var monitor: Any?
        init(_ bridge: EffectPrecisionKeyboardBridge) {
            onSpace = bridge.onSpace
            onUndo = bridge.onUndo
            onRedo = bridge.onRedo
        }
        func install() {
            monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
                guard let self, let window = self.view?.window,
                    PitchEditorKeyboardRouting.accepts(
                        eventWindow: event.window, editorWindow: window, isKey: window.isKeyWindow,
                        hasNestedSheet: window.attachedSheet != nil, responder: window.firstResponder),
                    let action = PitchEditorKeyboardRouting.action(
                        keyCode: event.keyCode, modifiers: event.modifierFlags, repeated: event.isARepeat),
                    PitchEditorKeyboardRouting.accepts(action, responder: window.firstResponder)
                else { return event }
                switch action {
                case .space: self.onSpace()
                case .undo: self.onUndo()
                case .redo: self.onRedo()
                case .consume: break
                }
                return nil
            }
        }
    }
    func makeCoordinator() -> Coordinator { Coordinator(self) }
    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        context.coordinator.view = view
        context.coordinator.install()
        return view
    }
    func updateNSView(_ view: NSView, context: Context) {
        context.coordinator.onSpace = onSpace
        context.coordinator.onUndo = onUndo
        context.coordinator.onRedo = onRedo
    }
    static func dismantleNSView(_ view: NSView, coordinator: Coordinator) {
        if let monitor = coordinator.monitor { NSEvent.removeMonitor(monitor) }
        coordinator.monitor = nil
    }
}
