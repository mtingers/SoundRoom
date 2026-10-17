import SoundroomCore
import SwiftUI

struct ApplyLiveEffectsButton: View {
    @ObservedObject var store: SessionStore
    let trackID: UUID
    let effectID: UUID
    @State private var reviewing = false
    var onApplied: () -> Void = {}
    private var plan: PitchAudioCommit.Plan? {
        PitchAudioCommit.plan(session: store.session, trackID: trackID, effectID: effectID)
    }
    var body: some View {
        Button("Apply to audio…") { reviewing = true }
            .buttonStyle(RoomButtonStyle()).disabled(plan == nil || store.busy || store.isRecording)
            .help("Bake live inserts into audio, retaining Undo and clip history")
            .sheet(isPresented: $reviewing) {
                VStack(alignment: .leading, spacing: 16) {
                    Text("Apply to audio").font(.system(size: 18, weight: .semibold))
                    if let plan {
                        EditingScopeView(
                            scope: .init(
                                trackName: store.session.tracks.first(where: { $0.id == trackID })?.name
                                    ?? "Track unavailable",
                                detail: plan.fullTrack ? "Entire track · render live inserts" : "Edited pitch regions",
                                range: nil))
                        Text(plan.label).font(.system(size: 13, weight: .medium))
                        Text(
                            plan.fullTrack
                                ? "Renders the entire track with its insert automation and decay, then removes the baked inserts. Track volume, pan and Master processing remain live."
                                : "Renders the edited pitch regions and removes that pitch stage. Other inserts remain live."
                        )
                        .font(.system(size: 12)).foregroundStyle(Theme.secondary)
                        Text("Undo and clip history retain the original audio.").font(.system(size: 11))
                            .foregroundStyle(Theme.secondary)
                        HStack {
                            Spacer()
                            Button("Cancel") { reviewing = false }.keyboardShortcut(.cancelAction).disabled(store.busy)
                            Button("Apply") {
                                store.applyPitchEdits(trackID: trackID, effectID: effectID) {
                                    reviewing = false
                                    onApplied()
                                }
                            }.buttonStyle(RoomButtonStyle(prominent: true)).disabled(store.busy).keyboardShortcut(
                                .defaultAction)
                        }
                        if store.busy {
                            HStack {
                                ProgressView(value: store.workProgress ?? 0).frame(maxWidth: .infinity)
                                Button("Cancel processing") { store.cancelBackgroundWork() }
                            }
                        }
                    } else {
                        Text("This insert or its source audio has changed.")
                        Button("Done") { reviewing = false }
                    }
                }.padding(24).frame(width: 440).background(Theme.paper).interactiveDismissDisabled(store.busy).disabled(
                    false
                ).environment(\.isEnabled, true)
            }
    }
}
