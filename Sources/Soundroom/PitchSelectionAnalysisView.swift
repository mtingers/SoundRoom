import AppKit
import SoundroomCore
import SwiftUI

/// Both inspectors host the document's same persisted pitch editor.
struct PitchSelectionAnalysisView: View {
    @ObservedObject var store: SessionStore
    @ObservedObject private var editor: PitchEditorController
    var effectID: UUID?
    private var expanded: Bool
    @State private var showExpanded = false
    @Environment(\.dismiss) private var dismiss
    init(store: SessionStore, effectID: UUID? = nil, expanded: Bool = false) {
        self.store = store
        self.effectID = effectID
        self.expanded = expanded
        self.editor = store.pitchEditor
    }
    private var requestedEffect: UUID? {
        if let effectID { return effectID }
        let effects = store.currentTrack?.realtimeEffects ?? []
        return effects.first { $0.id == store.selectedRealtimeEffect && $0.kind == .pitchCorrection }?.id
            ?? effects.first { $0.kind == .pitchCorrection }?.id
    }
    private func synchronize() {
        editor.synchronize(
            track: store.currentTrack, effectID: requestedEffect,
            range: store.selectedRange, assets: store.assets)
    }
    private var pointEditor: some View {
        PitchPointEditorView(
            regions: Binding(get: { editor.visibleRegions }, set: { editor.setRegions($0) }),
            before: editor.before, after: editor.after, scope: editor.scope, reference: editor.reference,
            busy: store.busy || store.isRecording, isPreviewing: store.isManualPitchPreview,
            canBake: editor.bakePlan != nil && !editor.targetsAreDraft,
            context: editor.context,
            bakeTitle: editor.bakePlan?.fullTrack == true ? "Apply full track" : "Apply to audio", error: editor.error,
            onBeginEdit: { editor.beginEdit($0) },
            onEndEdit: { editor.endEdit() }, onAnalyze: { editor.analyze() },
            onRecheck: { editor.render(preview: false) }, onAudition: { editor.render(preview: true) },
            onAuditionOriginal: { editor.render(preview: true, auditionOriginal: true) },
            onStop: { store.stopTransport() }, onBake: { editor.bake() },
            onResetTargets: { editor.resetTargets($0) }, expanded: expanded,
            canUndo: store.canUndo, canRedo: store.canRedo,
            onUndo: { store.undo() }, onRedo: { store.redo() }, onExpand: { showExpanded = true })
    }
    private func finishNumberEntry(_ action: @escaping () -> Void) {
        EffectWorkspaceFocus.finishNumberEntry(action)
    }
    var body: some View {
        Group {
            if expanded {
                VStack(spacing: 0) {
                    HStack {
                        Text("Pitch editor").font(.headline)
                        Spacer()
                        Button {
                            finishNumberEntry { store.undo() }
                        } label: {
                            Image(systemName: "arrow.uturn.backward")
                        }
                        .accessibilityLabel("Undo pitch edit").disabled(!store.canUndo || store.busy)
                        Button {
                            finishNumberEntry { store.redo() }
                        } label: {
                            Image(systemName: "arrow.uturn.forward")
                        }
                        .accessibilityLabel("Redo pitch edit").disabled(!store.canRedo || store.busy)
                        Button("Done") { dismiss() }
                    }.padding(16)
                    ScrollView { pointEditor.frame(maxWidth: .infinity, alignment: .topLeading) }
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                }.frame(
                    minWidth: 760, idealWidth: 900, maxWidth: .infinity,
                    minHeight: 650, idealHeight: 820, maxHeight: .infinity
                ).background(Theme.background)
            } else {
                pointEditor
            }
        }
        .sheet(isPresented: $showExpanded) {
            PitchSelectionAnalysisView(store: store, effectID: effectID, expanded: true)
        }
        .onAppear {
            if expanded { editor.presented() }
            synchronize()
        }
        .onChange(of: store.session) { _, _ in synchronize() }
        .onChange(of: store.selectedTrack) { _, _ in synchronize() }
        .onChange(of: store.selectedRange) { _, _ in synchronize() }
        .onChange(of: requestedEffect) { _, _ in synchronize() }
        .onChange(of: store.assets.mapValues(\.cacheIdentity)) { _, _ in synchronize() }
        .onChange(of: store.effectParameters) { _, _ in synchronize() }
        .onChange(of: editor.completedBakeID) { _, completed in
            if expanded, completed != nil { dismiss() }
        }
        .onDisappear {
            editor.endEdit()
            if store.isManualPitchPreview { store.stopTransport() }
        }
    }
}
