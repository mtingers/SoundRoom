import SwiftUI
import AppKit
import SoundroomCore

struct EffectInspector: View {
    @ObservedObject var store: SessionStore
    var isExpanded = false
    var onExpand: (() -> Void)? = nil
    var onClose: (() -> Void)? = nil
    @State private var audioEdits = false
    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 12) {
                HStack {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Effects").font(.system(size: 17, weight: .semibold))
                        HStack(spacing: 6) {
                            Circle().fill(store.selectedTrack == Session.masterTrackID ? Theme.ink : Theme.track(store.currentTrack?.color ?? 0)).frame(width: 7, height: 7).accessibilityHidden(true)
                            Text(store.currentTrack?.name ?? "Select a track").font(.system(size: 12, weight: .medium)).lineLimit(1)
                        }.accessibilityElement(children: .combine).accessibilityLabel("Effects for \(store.currentTrack?.name ?? "no selected track")")
                    }
                    Spacer()
                    if let onExpand {
                        Button(action: onExpand) { Image(systemName: isExpanded ? "arrow.right.to.line" : "arrow.left.to.line").frame(width: 24, height: 24).contentShape(Rectangle()) }
                            .buttonStyle(.plain).help(isExpanded ? "Compact effects drawer" : "Expand effects drawer")
                            .accessibilityLabel(isExpanded ? "Compact effects drawer" : "Expand effects drawer")
                    }
                    if let onClose {
                        Button(action: onClose) { Image(systemName: "xmark").frame(width: 24, height: 24).contentShape(Rectangle()) }
                            .buttonStyle(.plain).help("Close effects drawer").accessibilityLabel("Close effects drawer")
                    }
                }
            }.padding([.horizontal, .top], 16)
            RealtimeEffectInspector(store: store, onAudioEdits: store.effectMode == .master ? nil : {
                store.effectMode = .selection; if store.effect.realtimeKind != nil { store.chooseEffect(.normalize) }; audioEdits = true
            })
        }.background(Theme.paper)
            .sheet(isPresented: $audioEdits, onDismiss: { store.stopTransport(); store.restoreLiveEffectDestination(); store.focusEditor() }) {
                VStack { HStack { Text("Audio edits").font(.headline); Spacer(); Button("Done") { audioEdits = false }.disabled(store.busy) }.padding(); SelectionEffectInspector(store: store); if store.busy { HStack { ProgressView(value: store.workProgress ?? 0); Button("Cancel processing") { store.cancelBackgroundWork() } }.padding() } }.frame(width: 440, height: 650).background(Theme.paper).interactiveDismissDisabled(store.busy).disabled(false).environment(\.isEnabled, true)
            }
    }
}

private enum RealtimeInspectorPanel: String, CaseIterable, Identifiable {
    case parameters = "Parameters", automation = "Automation"
    var id: Self { self }
}

struct RealtimeEffectInspector: View {
    @ObservedObject var store: SessionStore
    @State private var panel: RealtimeInspectorPanel
    @State private var chainExpanded = false
    @State private var browsing = false
    @State private var precisionTarget: EffectWorkspaceTarget?
    var onAudioEdits: (() -> Void)?
    init(store: SessionStore, onAudioEdits: (() -> Void)? = nil) {
        self.store = store; self.onAudioEdits = onAudioEdits
        self._panel = State(initialValue: store.automationTarget == .effect ? .parameters : .automation)
    }
    var body: some View {
        GeometryReader { inspectorGeometry in
        VStack(spacing: 0) {
        VStack(alignment: .leading, spacing: 10) {
            if store.effectMode == .master { MasteringAssistantControls(store: store) }
            else if let track = store.currentTrack { TrackRoutingButton(store: store, track: track, expandedLabel: true) }
            if inspectorGeometry.size.height < 470 {
                Button { chainExpanded.toggle() } label: {
                    HStack {
                        Image(systemName: chainExpanded ? "chevron.down" : "chevron.right")
                        Text("\(chainExpanded ? "Hide" : "Show") effect chain")
                        Spacer()
                        Text("Inserts: \(store.currentTrack?.realtimeEffects.count ?? 0)").foregroundStyle(Theme.secondary)
                    }.font(.system(size: 12)).padding(.vertical, 4)
                }.buttonStyle(.plain).help("Expand the chain to add, select, reorder or remove inserts")
            }
            if inspectorGeometry.size.height >= 470 || chainExpanded {
            HStack(spacing: 6) {
                Theme.sectionTitle("Effect chain")
            }
            HStack(spacing: 8) {
                EffectChainControls(store: store)
                Spacer(minLength: 0)
                Button { browsing = true } label: { Label("Add", systemImage: "plus").font(.system(size: 12)) }
                    .buttonStyle(RoomButtonStyle()).disabled(store.currentTrack == nil).accessibilityLabel("Add real-time effect")
                    .popover(isPresented: $browsing, arrowEdge: .leading) {
                        EffectBrowser(entries: (store.effectMode == .master ? RealtimeEffectKind.allCases.filter { $0 != .pitchCorrection && $0 != .dither } : RealtimeEffectKind.allCases).map { .init(id: $0.rawValue, name: $0.name) }) { id in
                            if let kind = RealtimeEffectKind(rawValue: id) { store.addRealtimeEffect(kind); panel = .parameters }
                        }.onDisappear { restoreEditorAfterEffectBrowser(store) }
                    }
            }
            if let effects = store.currentTrack?.realtimeEffects, !effects.isEmpty {
                ScrollView {
                    VStack(spacing: 4) { ForEach(effects) { effect in
                        RealtimeEffectRow(store: store, effect: effect, onExpand: {
                            guard let trackID = store.selectedTrack else { return }
                            store.selectedRealtimeEffect = effect.id
                            precisionTarget = .realtime(trackID: trackID, effectID: effect.id, kind: effect.kind)
                        })
                    } }
                }.frame(height: CGFloat(min(4, effects.count)) * 39)
            }
            }
            HStack {
                Theme.sectionTitle(store.currentRealtimeEffect.map { "\($0.kind.name) settings" } ?? "Settings")
                Spacer()
                if store.currentRealtimeEffect?.enabled == false {
                    Text("Bypassed").font(.system(size: 9.5, weight: .semibold)).foregroundStyle(Theme.background)
                        .padding(.horizontal, 6).padding(.vertical, 2).background(Theme.secondary, in: Capsule())
                }
            }.padding(.top, 4)
            // A sunken well: the scrolling settings read as one place, apart
            // from the chain above and the listen/apply bar below.
            GeometryReader { geometry in
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                        if let effect = store.currentRealtimeEffect { parameterControls(effect) }
                        else { Text("Add an effect to this track.").font(.system(size: 11)).foregroundStyle(Theme.secondary) }
                }.padding(10).frame(width: geometry.size.width, alignment: .topLeading)
            }.id("\(store.selectedTrack?.uuidString ?? "none").\(store.currentRealtimeEffect?.id.uuidString ?? "none").\(panel.rawValue)")
                .frame(maxHeight: .infinity)
            }
            .background(Theme.well, in: RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).stroke(Theme.line))
            .clipShape(RoundedRectangle(cornerRadius: 10))
        }.padding(16)
            actionBar
        }.disabled(store.busy || store.isRecording)
            .sheet(item: $precisionTarget) { target in
                EffectPrecisionSheet(store: store, target: target, onAutomation: {
                    guard target.matches(store), case let .realtime(_, id, _) = target else { return }
                    store.selectedRealtimeEffect = id; store.automationTarget = .effect; store.automationLaneVisible = true
                    store.focusEditor()
                })
            }
            .onChange(of: store.automationTarget) { _, value in if value != .effect { panel = .automation } }
            .onChange(of: store.selectedRealtimeEffect) { _, _ in if store.automationTarget == .effect { panel = .parameters } }
        }
    }

    /// Listening and committing are actions on the whole track, not settings
    /// of one effect: they live in their own tinted bar along the bottom.
    private var actionBar: some View {
        VStack(alignment: .leading, spacing: 8) {
            Theme.sectionTitle("Listen & apply")
            EffectAuditionControls(store: store)
            HStack(spacing: 8) {
                if store.selectedTrack != Session.masterTrackID, let trackID = store.selectedTrack, let effect = store.currentRealtimeEffect {
                    ApplyLiveEffectsButton(store: store, trackID: trackID, effectID: effect.id)
                    if effect.kind == .sourceIsolator, store.effectMode != .master {
                        Button("Separate source…") { store.requestSourceSeparation() }
                            .buttonStyle(RoomButtonStyle())
                            .disabled(!store.hasAudioSelection || store.remoteGUI?.hasPendingEdits(for: trackID) == true)
                            .help("Use local learned separation on the selected audio")
                    }
                }
                if let onAudioEdits {
                    Button("Audio edits…", action: onAudioEdits).buttonStyle(RoomButtonStyle())
                        .disabled(!store.hasAudioSelection || store.busy)
                        .help(store.hasAudioSelection ? "Process the selected audio (normalize, fades, effects)" : "Select audio on the timeline first")
                }
                Spacer(minLength: 0)
            }.font(.system(size: 11))
        }.padding(.horizontal, 16).padding(.vertical, 12).frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.actionBar).overlay(alignment: .top) { Theme.edge.frame(height: 1) }
    }
    private func parameterControls(_ effect: RealtimeEffect) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            if effect.kind == .pitchCorrection {
                PitchSelectionAnalysisView(store: store, effectID: effect.id)
                PitchFeedbackView(snapshot: { store.effectMeterSnapshot(effect.id) },
                    running: store.isPlaying && !store.isPreviewing, bypassed: !effect.enabled).id(effect.id)
            }
            if effect.kind == .pitchCorrection {
            EffectParameterControls(library: store.effectPresets, kind: effect.kind, values: effect.parametersForEditing, onChange: { key, value in
                store.changeRealtimeEffect(effect.id, name: "Effect \(key)", coalescingKey: "\(effect.id).\(key)") { item in
                    if item.parameters.isEmpty { item.enableParameterControls(); item.selectedAutomationParameter = item.kind.parameters.first?.id }
                    item.parameters[key] = value
                }
            }, onEditingChanged: { key, editing in
                if editing { store.beginContinuousEdit("\(effect.id).\(key)") }
                else { store.endContinuousEdit() }
            }, onNumericSubmit: { store.focusEditor() }, onPreset: { parameters in
                store.changeRealtimeEffect(effect.id, name: "Effect preset") { $0.parameters = parameters; if $0.selectedAutomationParameter == nil { $0.selectedAutomationParameter = $0.kind.parameters.first?.id } }
                store.focusEditor()
            }, onSavedPreset: { preset in
                store.applyRealtimePreset(preset, to: effect.id)
            }).id(effect.id)
            } else if let trackID = store.selectedTrack {
                EffectWorkspaceControls(store: store, target: .realtime(trackID: trackID, effectID: effect.id, kind: effect.kind), onNumericSubmit: { store.focusEditor() })
            }
        }
    }

}

struct AutomationLaneHeader: View {
    @ObservedObject var store: SessionStore
    @State private var editing = false
    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Picker("Lane", selection: $store.automationTarget) {
                ForEach(AutomationTarget.allCases.filter { store.selectedTrack != Session.masterTrackID || $0 != .pan }) { Text($0.rawValue).tag($0) }
            }.font(.system(size: 10)).onChange(of: store.automationTarget) { _, _ in store.focusEditor() }
            if store.automationTarget == .effect, let effect = store.currentRealtimeEffect {
                Picker("Parameter", selection: Binding(get: { store.currentRealtimeEffect?.selectedAutomationParameter ?? "" }, set: { key in
                    store.changeRealtimeEffect(effect.id, name: "Automation parameter") { item in
                        if !key.isEmpty { item.enableParameterControls() }; item.selectedAutomationParameter = key.isEmpty ? nil : key
                    }; store.focusEditor()
                })) {
                    if effect.parameters.isEmpty { Text("Legacy amount").tag("") }
                    ForEach(effect.kind.parameters) { Text($0.name).tag($0.id) }
                }.font(.system(size: 10))
            }
            HStack {
                Picker("Automation mode", selection: Binding(get: { store.automationWrite ? .write : store.automationEditing.mode }, set: store.setAutomationMode)) {
                    ForEach(store.availableAutomationRecordModes) { Text($0.rawValue).tag($0) }
                }.labelsHidden().font(.system(size: 10)).frame(maxWidth: .infinity)
                Button { editing = true } label: { Image(systemName: "slider.horizontal.3") }.buttonStyle(.plain).accessibilityLabel("Edit automation points").help("Edit points, curves and ranges")
            }
        }.frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 12).background(Theme.paper)
            .overlay(alignment: .bottom) { Theme.line.frame(height: 1) }
            .popover(isPresented: $editing) { ScrollView { AutomationControls(store: store).padding(16) }.frame(width: 350, height: 540).background(Theme.paper) }
    }
}

struct TrackAutomationToggle: View {
    @ObservedObject var store: SessionStore
    let trackID: UUID
    private var active: Bool { store.selectedTrack == trackID && store.automationLaneVisible }
    var body: some View {
        Button {
            let wasActive = active
            store.choose(track: trackID)
            store.automationLaneVisible = !wasActive
            if store.currentRealtimeEffect == nil || trackID == Session.masterTrackID && store.automationTarget == .pan { store.automationTarget = .volume }
            store.focusEditor()
        } label: {
            Text("A").font(.system(size: 10, weight: .semibold)).frame(width: 24, height: 24)
                .background(active ? Theme.automation : Theme.raised, in: RoundedRectangle(cornerRadius: 4)).foregroundStyle(active ? Theme.background : Theme.secondary)
        }.buttonStyle(.plain).help("Show / hide automation").accessibilityLabel("Toggle automation for \(trackID == Session.masterTrackID ? "Master" : store.session.tracks.first { $0.id == trackID }?.name ?? "track")").accessibilityValue(active ? "Shown" : "Hidden")
    }
}

struct AutomationLaneView: View {
    @ObservedObject var store: SessionStore
    @ObservedObject var editing: AutomationEditingState
    @State private var draft: AutomationPoint?
    @State private var originalFrame: Int?
    @State private var dragging = false
    @State private var deleting = false
    @State private var selectingRange = false
    @State private var groupFrames: Set<Int> = []
    @State private var groupDraft: [AutomationPoint] = []
    @State private var frameDelta = 0
    @State private var valueDelta: Float = 0
    init(store: SessionStore) { self.store = store; self.editing = store.automationEditing }
    var body: some View {
        GeometryReader { geometry in
            Canvas { context, size in
                let y: (Float) -> CGFloat = { 9 + CGFloat(1 - $0) * (size.height - 18) }
                var points = store.automationPoints.filter { !groupFrames.contains($0.frame) && $0.frame != originalFrame }
                points += groupDraft
                if let draft { points.removeAll { $0.frame == draft.frame }; points.append(draft) }
                points.sort { $0.frame < $1.frame }
                if let range = editing.range {
                    let start = Double(range.lowerBound) / sessionRate * store.zoom
                    let end = Double(range.upperBound) / sessionRate * store.zoom
                    context.fill(Path(CGRect(x: start, y: 0, width: max(1, end - start), height: size.height)), with: .color(Theme.accent.opacity(0.1)))
                }
                for amount: Float in [0, 0.5, 1] {
                    var grid = Path(); grid.move(to: CGPoint(x: 0, y: y(amount))); grid.addLine(to: CGPoint(x: size.width, y: y(amount)))
                    context.stroke(grid, with: .color(Theme.line.opacity(0.6)), lineWidth: 1)
                }
                var line = Path(); line.move(to: CGPoint(x: 0, y: y(store.automationBaseValue)))
                if let first = points.first {
                    let x = Double(first.frame) / sessionRate * store.zoom
                    if first.frame > 0 { line.addLine(to: CGPoint(x: x, y: y(store.automationBaseValue))) }
                    line.addLine(to: CGPoint(x: x, y: y(first.value)))
                }
                for pair in zip(points, points.dropFirst()) {
                    for sample in 1...24 {
                        let frame = pair.0.frame + (pair.1.frame - pair.0.frame) * sample / 24
                        line.addLine(to: CGPoint(x: Double(frame) / sessionRate * store.zoom, y: y(AutomationInterpolation.between(pair.0, pair.1, at: frame, scale: store.currentAutomationScale))))
                    }
                }
                line.addLine(to: CGPoint(x: size.width, y: y(points.last?.value ?? store.automationBaseValue)))
                context.stroke(line, with: .color(Theme.automation), lineWidth: 1.5)
                for point in points {
                    let x = Double(point.frame) / sessionRate * store.zoom
                    let selected = editing.selectedFrames.contains(point.frame) || groupDraft.contains { $0.frame == point.frame }
                    let circle = Path(ellipseIn: CGRect(x: x - 4, y: y(point.value) - 4, width: 8, height: 8))
                    context.fill(circle, with: .color(selected ? Theme.ink : Theme.automation))
                }

            }.background(Theme.paper.opacity(0.45)).contentShape(Rectangle())
                .gesture(DragGesture(minimumDistance: 0).onChanged { value in
                    guard !store.busy, !store.isRecording else { return }
                    if !dragging {
                        dragging = true
                        let flags = NSApp.currentEvent?.modifierFlags ?? NSEvent.modifierFlags
                        deleting = flags.contains(.option)
                        let point = store.automationPoints.first { point in
                            let x = Double(point.frame) / sessionRate * store.zoom
                            let y = 9 + Double(1 - point.value) * (geometry.size.height - 18)
                            return hypot(x - value.startLocation.x, y - value.startLocation.y) < 10
                        }
                        originalFrame = point?.frame
                        selectingRange = point == nil && flags.contains(.shift)
                        if let point, !deleting {
                            store.selectAutomationPoint(point.frame, extending: flags.contains(.shift))
                            groupFrames = editing.selectedFrames
                        } else if !selectingRange { editing.resetSelection() }
                    }
                    if selectingRange {
                        let first = max(0, Int(min(value.startLocation.x, value.location.x) / store.zoom * sessionRate))
                        let last = max(first + 1, Int(max(value.startLocation.x, value.location.x) / store.zoom * sessionRate))
                        store.selectAutomationRange(first..<last)
                    } else if !groupFrames.isEmpty {
                        let originals = store.automationPoints.filter { groupFrames.contains($0.frame) }
                        let rawDelta = Int(value.translation.width / store.zoom * sessionRate)
                        let shift = max(-(originals.map(\.frame).min() ?? 0), rawDelta)
                        let delta = Float(-value.translation.height / (geometry.size.height - 18))
                        frameDelta = shift
                        valueDelta = max(-(originals.map(\.value).min() ?? 0), min(1 - (originals.map(\.value).max() ?? 1), delta))
                        groupDraft = originals.map { $0.relocated(frame: $0.frame + shift, value: $0.value + valueDelta) }
                        draft = nil
                    } else if !deleting {
                        draft = AutomationPoint(frame: max(0, Int(value.location.x / store.zoom * sessionRate)),
                            value: Float(max(0, min(1, 1 - (value.location.y - 9) / (geometry.size.height - 18)))))
                    }
                }.onEnded { _ in
                    if deleting, let originalFrame { store.deleteCurrentAutomationPoint(frame: originalFrame) }
                    else if !groupFrames.isEmpty { store.moveAutomationGroup(frames: groupFrames, by: frameDelta, valueDelta: valueDelta) }
                    else if let draft { store.putCurrentAutomationPoint(frame: draft.frame, value: draft.value, replacing: originalFrame); editing.selectedFrames = [draft.frame] }
                    editing.hasFocus = true; resetDraft(); store.focusEditor()
                })
                .overlay(alignment: .topLeading) {
                    if dragging, !groupFrames.isEmpty {
                        Text(String(format: "Move %+.4f s · Value %+.1f%%", Double(frameDelta) / sessionRate, valueDelta * 100)).font(.system(size: 10, design: .monospaced)).padding(4).background(Theme.paper).allowsHitTesting(false)
                    }
                }
                .accessibilityLabel("\(store.automationTitle) automation lane. Click to add; drag selected points together. Shift-click selects points; Shift-drag selects a time range; Option-click deletes.")
        }.overlay(alignment: .bottom) { Theme.line.frame(height: 1) }
            .onChange(of: store.automationTitle) { _, _ in resetDraft(); editing.resetSelection() }
            .onChange(of: store.selectedTrack) { _, _ in resetDraft(); editing.resetSelection() }
    }
    private func resetDraft() {
        draft = nil; originalFrame = nil; dragging = false; deleting = false; selectingRange = false; groupFrames = []; groupDraft = []; frameDelta = 0; valueDelta = 0
    }
}

private struct RealtimeEffectRow: View {
    @ObservedObject var store: SessionStore
    let effect: RealtimeEffect
    var onExpand: () -> Void = {}
    private var bypassTitle: String { "\(effect.enabled ? "Bypass" : "Enable") \(effect.kind.name)" }
    var body: some View {
                        HStack(spacing: 8) {
                            Button {
                                store.changeRealtimeEffect(effect.id, name: "Bypass effect") { $0.enabled.toggle() }; store.focusEditor()
                            } label: { Image(systemName: "power").foregroundStyle(effect.enabled ? Theme.accent : Theme.secondary) }
                                .buttonStyle(.plain).help(bypassTitle)
                                .accessibilityLabel(bypassTitle)
                                .accessibilityValue(effect.enabled ? "On" : "Bypassed")
                            Button { store.selectedRealtimeEffect = effect.id; store.automationTarget = .effect; store.focusEditor() } label: {
                                HStack(spacing: 7) {
                                    Image(systemName: EffectSymbol.of(effect.kind.rawValue)).foregroundStyle(EffectFamily.of(effect.kind.rawValue).color)
                                    Text(effect.kind.name).lineLimit(1)
                                }.frame(maxWidth: .infinity, alignment: .leading).contentShape(Rectangle())
                            }.buttonStyle(.plain).accessibilityAddTraits(store.currentRealtimeEffect?.id == effect.id ? .isSelected : [])
                            if effect.kind != .pitchCorrection {
                                Button(action: onExpand) { Image(systemName: "arrow.up.left.and.arrow.down.right").frame(width: 18, height: 18) }
                                    .buttonStyle(.plain).help("Expand effect").accessibilityLabel("Open \(effect.kind.name) precision editor")
                            }
                            Button { store.moveRealtimeEffect(effect.id, offset: -1) } label: { Image(systemName: "chevron.up").font(.system(size: 9)) }.buttonStyle(.plain)
                                .disabled(store.currentTrack?.realtimeEffects.first?.id == effect.id).help("Process earlier").accessibilityLabel("Move \(effect.kind.name) earlier")
                            Button { store.moveRealtimeEffect(effect.id, offset: 1) } label: { Image(systemName: "chevron.down").font(.system(size: 9)) }.buttonStyle(.plain)
                                .disabled(store.currentTrack?.realtimeEffects.last?.id == effect.id).help("Process later").accessibilityLabel("Move \(effect.kind.name) later")
                            Button { store.removeRealtimeEffect(effect.id) } label: { Image(systemName: "xmark") }.buttonStyle(.plain)
                                .help("Remove effect").accessibilityLabel("Remove \(effect.kind.name)")
                        }.font(.system(size: 12)).padding(10)
                            .background(store.currentRealtimeEffect?.id == effect.id ? Theme.accent.opacity(0.18) : Theme.raised, in: RoundedRectangle(cornerRadius: 5))
                            .overlay(RoundedRectangle(cornerRadius: 5).stroke(store.currentRealtimeEffect?.id == effect.id ? Theme.accent.opacity(0.7) : Color.clear, lineWidth: 1))
    }
}
