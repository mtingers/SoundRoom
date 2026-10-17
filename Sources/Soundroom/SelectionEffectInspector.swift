import SoundroomCore
import SwiftUI

struct SelectionEffectInspector: View {
    @ObservedObject var store: SessionStore
    @State private var browsing = false
    @State private var precisionTarget: EffectWorkspaceTarget?
    private var target: EffectWorkspaceTarget {
        .selection(trackID: store.selectedTrack, kind: store.effect, range: store.selectedRange)
    }
    var body: some View {
        let currentTarget = target
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 7) {
                Image(systemName: store.hasAudioSelection ? "waveform" : "cursorarrow")
                Text(
                    store.hasAudioSelection
                        ? "\(store.currentTrack?.name ?? "Audio") · \(timeString(store.selectedRange?.count ?? 0))"
                        : "Select audio to apply"
                )
                .lineLimit(1)
            }.font(.system(size: 11)).foregroundStyle(Theme.secondary)
            HStack(spacing: 6) {
                Button {
                    browsing = true
                } label: {
                    HStack {
                        EffectIdentity(name: store.effect.name, rawValue: store.effect.rawValue)
                        Spacer()
                        Image(systemName: "chevron.up.chevron.down").font(.system(size: 10)).foregroundStyle(
                            Theme.secondary)
                    }.frame(maxWidth: .infinity).contentShape(Rectangle())
                }.buttonStyle(RoomButtonStyle()).help(store.effect.detail).accessibilityLabel(
                    "Selection effect: \(store.effect.name)"
                )
                .popover(isPresented: $browsing, arrowEdge: .leading) {
                    EffectBrowser(
                        entries: BuiltInEffect.allCases.filter { $0.realtimeKind == nil }.map {
                            .init(id: $0.rawValue, name: $0.name)
                        }, selected: store.effect.rawValue
                    ) { id in
                        if let effect = BuiltInEffect(rawValue: id) { store.chooseEffect(effect) }
                    }.onDisappear { restoreEditorAfterEffectBrowser(store) }
                }
                if store.effect != .pitchCorrection {
                    Button {
                        precisionTarget = target
                    } label: {
                        Image(systemName: "arrow.up.left.and.arrow.down.right").frame(width: 24, height: 24)
                    }
                    .buttonStyle(.plain).help("Open precision editor").accessibilityLabel(
                        "Open \(store.effect.name) precision editor")
                }
            }
            Text(
                store.effect == .pitchCorrection ? "Tune live · apply when ready" : "Preview · apply to selected audio"
            )
            .font(.system(size: 10)).foregroundStyle(Theme.secondary)
            GeometryReader { geometry in
                ScrollView {
                    VStack(alignment: .leading, spacing: 12) {
                        if let kind = store.effect.realtimeKind {
                            if kind == .pitchCorrection { PitchSelectionAnalysisView(store: store) }
                            if kind == .pitchCorrection {
                                EffectParameterControls(
                                    library: store.effectPresets, kind: kind,
                                    values: kind == .pitchCorrection
                                        ? store.pitchEditor.parameters : store.effectParameters,
                                    onChange: { key, value in
                                        guard store.effect.realtimeKind == kind else { return }
                                        if kind == .pitchCorrection {
                                            store.pitchEditor.setParameter(key, value: value)
                                        } else {
                                            store.setSelectionParameter(key, value: value)
                                        }
                                    },
                                    onEditingChanged: { key, editing in
                                        guard kind == .pitchCorrection, let id = store.pitchEditor.effectID else {
                                            return
                                        }
                                        if editing {
                                            store.beginContinuousEdit("\(id).\(key)")
                                        } else {
                                            store.endContinuousEdit()
                                        }
                                    }, onNumericSubmit: { store.focusEditor() },
                                    onPreset: { values in
                                        if kind == .pitchCorrection {
                                            store.pitchEditor.setParameters(values)
                                        } else {
                                            store.effectParameters = values
                                        }
                                        if store.isPreviewing { store.refreshEffectPreview() }
                                        store.focusEditor()
                                    },
                                    onSavedPreset: { preset in
                                        if kind == .pitchCorrection {
                                            store.pitchEditor.setParameters(preset.parameters)
                                        } else {
                                            store.applySelectionPreset(preset)
                                        }
                                    }
                                ).id(kind)
                            } else {
                                EffectWorkspaceControls(
                                    store: store, target: target, onNumericSubmit: { store.focusEditor() })
                            }
                        } else {
                            SelectionUtilityCard(effect: store.effect)
                        }
                    }.frame(width: geometry.size.width, alignment: .topLeading)
                }.id(store.effect)
            }.frame(maxHeight: .infinity)
            if store.effect != .pitchCorrection {
                Divider()
                SelectionEffectActions(store: store, isValid: { currentTarget.matches(store) })
            }
        }.padding(16).background(Theme.paper).disabled(store.busy || store.isRecording)
            .sheet(item: $precisionTarget) { EffectPrecisionSheet(store: store, target: $0) }
    }
}
