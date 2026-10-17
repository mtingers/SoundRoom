import SoundroomCore
import SwiftUI

struct VocalEnhancementRequest: Identifiable {
    let id = UUID()
    let trackName: String
    let target: EffectWorkspaceTarget
}

extension SessionStore {
    func canUseTrackTools(_ trackID: UUID?) -> Bool {
        guard !busy, !isRecording, let trackID, session.tracks.contains(where: { $0.id == trackID }) else {
            return false
        }
        guard let remoteGUI, remoteGUI.connected, !remoteGUI.hasPendingEdits(for: trackID),
            case .connected(_, _, let role) = remoteGUI.state
        else { return false }
        return role == .editor || role == .operator
    }
    var canSeparateSource: Bool {
        canUseTrackTools(selectedTrack) && hasAudioSelection && selectedRange != nil
            && (selection.map { $0.range.isEmpty || $0.trackID == selectedTrack } ?? true)
    }

    func requestVocalEnhancement(trackID: UUID? = nil) {
        guard let id = trackID ?? selectedTrack, canUseTrackTools(id),
            let track = session.tracks.first(where: { $0.id == id })
        else { return }
        if selectedTrack != id { choose(track: id) }
        effectMode = .realtime
        showEffects = true
        if let effect = track.realtimeEffects.first(where: { $0.kind == .vocalEnhancer }) {
            selectedRealtimeEffect = effect.id
        } else {
            addRealtimeEffect(.vocalEnhancer)
        }
        guard
            let effect = currentTrack?.realtimeEffects.first(where: {
                $0.id == selectedRealtimeEffect && $0.kind == .vocalEnhancer
            })
        else { return }
        vocalEnhancementRequest = .init(
            trackName: track.name, target: .realtime(trackID: id, effectID: effect.id, kind: .vocalEnhancer))
    }
    func requestSourceSeparation(trackID: UUID? = nil, wholeTrack: Bool = false) {
        let id = trackID ?? selectedTrack
        guard canUseTrackTools(id), let id,
            let track = session.tracks.first(where: { $0.id == id })
        else { return }
        if wholeTrack {
            guard let end = track.clips.map(\.end).max(), end > 0 else { return }
            choose(track: id)
            selection = EditSelection(trackID: id, range: 0..<end)
        } else if selectedTrack != id {
            choose(track: id)
        }
        effectMode = .realtime
        sourceSeparationRequest = SourceSeparationController.capture(store: self)
    }
}

struct VocalEnhancementSheet: View {
    @ObservedObject var store: SessionStore
    let request: VocalEnhancementRequest
    @Environment(\.dismiss) private var dismiss
    private var effect: RealtimeEffect? { request.target.effect(store) }
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack {
                Label("Enhance vocals", systemImage: "waveform.and.mic").font(.system(size: 20, weight: .semibold))
                Spacer()
                Button("Done") { dismiss() }.keyboardShortcut(.cancelAction)
            }
            Text(request.trackName).font(.system(size: 13, weight: .medium)).lineLimit(1)
            Text(
                "Start with a preset, listen, then adjust. Changes stay live on this track; Undo restores earlier settings."
            )
            .font(.system(size: 12)).foregroundStyle(Theme.secondary)
            if let effect {
                HStack {
                    Picker(
                        "Compare",
                        selection: Binding(
                            get: { effect.enabled },
                            set: { enabled in
                                EffectWorkspaceFocus.finishNumberEntry {
                                    guard request.target.matches(store), !store.busy, !store.isRecording else { return }
                                    store.changeRealtimeEffect(effect.id, name: "Compare vocal enhancer") {
                                        $0.enabled = enabled
                                    }
                                }
                            })
                    ) {
                        Text("Before").tag(false)
                        Text("Enhanced").tag(true)
                    }.pickerStyle(.segmented)
                    Text("Other inserts remain live").font(.system(size: 11)).foregroundStyle(Theme.secondary)
                }.disabled(store.busy || store.isRecording)
                ScrollView { EffectWorkspaceControls(store: store, target: request.target).padding(12) }
                    .frame(maxHeight: .infinity).background(Theme.well, in: RoundedRectangle(cornerRadius: 8))
                    .disabled(store.busy || store.isRecording)
                EffectAuditionControls(store: store)
                if case let .realtime(trackID, effectID, _) = request.target {
                    HStack {
                        Text("Keep it live, or render it into audio.").font(.system(size: 12)).foregroundStyle(
                            Theme.secondary)
                        Spacer()
                        ApplyLiveEffectsButton(store: store, trackID: trackID, effectID: effectID)
                    }
                }
            } else {
                Text("This track or insert changed. Close this window and reopen Enhance vocals for the current track.")
                    .font(.system(size: 13)).foregroundStyle(Color.orange)
                Spacer()
            }
        }.padding(24).frame(width: 620, height: 720).background(Theme.paper).foregroundStyle(Theme.ink)
            .preferredColorScheme(.dark)
    }
}
