import SwiftUI
import SoundroomCore

extension SessionStore {
    func addMixTrack(role: String) {
        edit(role == "bus" ? "Add group bus" : "Add effect return") {
            var track = Track(name: role == "bus" ? "Group" : "Reverb return", color: role == "bus" ? 2 : 3)
            track.routing = TrackRouting(role: role)
            if role == "return" { var room = RealtimeEffect(kind: .room); room.parameters = RealtimeEffectKind.room.defaultParameters; room.parameters["mix"] = 1; room.parameters["character"] = 1; track.realtimeEffects = [room] }
            session.tracks.append(track); selectedTrack = track.id; selectedClip = nil; selectedClips = []; selection = nil
        }
    }
    func setRouting(_ trackID: UUID, coalescingKey: String? = nil, change: (inout TrackRouting) -> Void) {
        guard let index = session.tracks.firstIndex(where: { $0.id == trackID }) else { return }
        var route = session.tracks[index].routing ?? TrackRouting(); change(&route)
        var candidate = session; candidate.tracks[index].routing = route
        do { _ = try TrackRoutingPlan(session: candidate) }
        catch { status = error.localizedDescription; return }
        updateTrack(trackID, actionName: "Track routing", coalescingKey: coalescingKey) { $0.routing = route }
    }
}

struct TrackRoutingButton: View {
    @ObservedObject var store: SessionStore
    let track: Track
    var expandedLabel = false
    @State private var showing = false
    var body: some View {
        Button { showing = true } label: {
            if expandedLabel { Label("Routing · \(track.routing?.role == "bus" ? "Group bus" : track.routing?.role == "return" ? "Effect return" : "Audio")",systemImage:"point.3.connected.trianglepath.dotted").font(.system(size:12)).lineLimit(1) }
            else { Image(systemName:"point.3.connected.trianglepath.dotted").font(.system(size:12)).frame(width:18,height:22).contentShape(Rectangle()) }
        }.buttonStyle(.plain).foregroundStyle(Theme.secondary)
            .accessibilityLabel("Routing for \(track.name)").help("Output, post-fader sends, and compressor sidechain")
            .popover(isPresented: $showing) {
                if let current = store.session.tracks.first(where: { $0.id == track.id }) { TrackRoutingEditor(store: store, track: current) }
            }
    }
}

private struct TrackRoutingEditor: View {
    @ObservedObject var store: SessionStore
    let track: Track
    var route: TrackRouting { track.routing ?? TrackRouting() }
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("\(track.name) · Routing").font(.headline)
            Picker("Output", selection: Binding<UUID?>(get: { route.outputID }, set: { id in store.setRouting(track.id) { $0.outputID = id } })) {
                Text("Master").tag(Optional<UUID>.none)
                ForEach(store.session.tracks.filter { $0.id != track.id && $0.routing?.role == "bus" }) { target in Text(target.name).tag(Optional(target.id)) }
            }
            Text("Sends · post-fader").font(.subheadline)
            let returns = store.session.tracks.filter { $0.id != track.id && $0.routing?.role == "return" }
            if returns.isEmpty { Text("Add an Effect return from the Track menu. Shared reverb starts at 100% wet.").font(.caption).foregroundStyle(Theme.secondary).fixedSize(horizontal:false,vertical:true) }
            ForEach(returns) { target in
                let send = route.sends.first { $0.trackID == target.id }
                HStack {
                    Toggle(target.name, isOn: Binding(get: { send != nil }, set: { enabled in store.setRouting(track.id) { routing in routing.sends.removeAll { $0.trackID == target.id }; if enabled { routing.sends.append(TrackSend(trackID: target.id)) } } })).lineLimit(1).frame(width:120,alignment:.leading).help(target.name)
                    if let send {
                        Slider(value: Binding(get: { Double(send.level) }, set: { level in store.setRouting(track.id,coalescingKey:"\(track.id).send.\(target.id)") { routing in if let i = routing.sends.firstIndex(where: { $0.trackID == target.id }) { routing.sends[i].level = Float(level) } } }), in: 0...2,onEditingChanged: { editing in if editing { store.beginContinuousEdit("\(track.id).send.\(target.id)") } else { store.endContinuousEdit() } })
                            .accessibilityLabel("Send level to \(target.name)")
                        Text(send.level == 0 ? "−∞ dB" : String(format: "%+.1f dB", 20 * log10(send.level))).font(.caption.monospacedDigit()).frame(width: 64)
                    }
                }
            }
            Picker("Compressor sidechain", selection: Binding<UUID?>(get: { route.sidechainSourceID }, set: { id in store.setRouting(track.id) { $0.sidechainSourceID = id } })) {
                Text("Internal · this track").tag(Optional<UUID>.none)
                ForEach(store.session.tracks.filter { $0.id != track.id }) { source in Text(source.name).tag(Optional(source.id)) }
            }
            Text("The selected post-fader signal controls every Compressor on this track. It is not mixed into the track. Mute also mutes the key.")
                .font(.caption).foregroundStyle(Theme.secondary).fixedSize(horizontal:false,vertical:true)
            if route.sidechainSourceID != nil && !track.realtimeEffects.contains(where: { $0.kind == .compressor }) { Text("Add a Compressor to use this sidechain.").font(.caption).foregroundStyle(Theme.warning) }
        }.padding(18).frame(width: 390).disabled(store.busy || store.isRecording)
    }
}
