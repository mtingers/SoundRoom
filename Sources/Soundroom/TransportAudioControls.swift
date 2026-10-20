import Foundation
import SoundroomRemote
import SwiftUI

enum InputVolumeLevel: Equatable {
    /// 0…1, as System Settings shows it.
    case adjustable(Double)
    /// The device has no software-controllable input level.
    case fixed
    case unavailable(String)
}

/// Hardware input levels by device UID; a missing entry is not yet read.
/// Kept off SessionStore so a slider drag redraws only the volume rows.
@MainActor final class InputVolumeModel: ObservableObject {
    @Published var levels: [String: InputVolumeLevel] = [:]
    /// Per device: whether a request is in flight, and the newest slider value
    /// waiting behind it, so a drag never queues a backlog.
    var requests: [String: (inFlight: Bool, next: Double?)] = [:]
}

private struct InputVolumeReply: Decodable {
    let uid: String
    let volume: Double?
}

extension SessionStore {
    func toggleMetronome() { metronome.toggle() }
    func syncMetronome() {
        guard let remoteGUI, remoteGUI.connected else { return }
        let enabled = metronome
        remoteGUI.route(.init(operation: "transport.metronome", flags: ["enabled": enabled])) { [weak self] reply in
            // Show the click as it is heard: a refused switch-on stays off.
            guard let self, !reply.accepted, enabled, metronome else { return }
            metronome = false
            status = reply.message.isEmpty ? "The metronome could not be switched on." : reply.message
        }
    }

    /// The session input first, then any other device a track records from.
    var inputVolumeDevices: [AudioDevice] {
        let inputs = devices.filter { $0.inputChannels > 0 && !$0.uid.isEmpty }
        var result = inputs.filter { $0.id == inputDevice }
        for uid in session.tracks.compactMap(\.inputDeviceUID) where !result.contains(where: { $0.uid == uid }) {
            if let device = inputs.first(where: { $0.uid == uid }) { result.append(device) }
        }
        return result
    }
    func loadInputVolume(uid: String) {
        guard inputVolume.requests[uid]?.inFlight != true else { return }
        sendInputVolume(uid: uid, volume: nil)
    }
    func setInputVolume(_ volume: Double, uid: String) {
        let volume = min(1, max(0, volume))
        inputVolume.levels[uid] = .adjustable(volume)
        if inputVolume.requests[uid]?.inFlight == true {
            inputVolume.requests[uid]?.next = volume
        } else {
            sendInputVolume(uid: uid, volume: volume)
        }
    }
    private func sendInputVolume(uid: String, volume: Double?) {
        guard let remoteGUI, remoteGUI.connected else {
            inputVolume.levels[uid] = .unavailable("Connect to the audio server to adjust the input level.")
            return
        }
        inputVolume.requests[uid] = (inFlight: true, next: nil)
        remoteGUI.route(
            .init(
                operation: "devices.inputVolume", numbers: volume.map { ["volume": $0] } ?? [:],
                strings: ["uid": uid])
        ) { [weak self] reply in
            guard let self else { return }
            let next = inputVolume.requests[uid]?.next
            inputVolume.requests[uid] = nil
            if let next {
                sendInputVolume(uid: uid, volume: next)
                return
            }
            if reply.accepted, let payload = reply.payload,
                let result = try? JSONDecoder().decode(InputVolumeReply.self, from: payload)
            {
                inputVolume.levels[result.uid] = result.volume.map { .adjustable(min(1, max(0, $0))) } ?? .fixed
            } else {
                inputVolume.levels[uid] = .unavailable(
                    reply.message.isEmpty ? "The input level could not be read." : reply.message)
            }
        }
    }
}

/// Click on/off beside the transport, for playback and recording alike.
struct MetronomeButton: View {
    @ObservedObject var store: SessionStore
    var body: some View {
        Button {
            store.toggleMetronome()
        } label: {
            Image(systemName: "metronome").font(.system(size: 14))
                .foregroundStyle(store.metronome ? Theme.background : Theme.secondary)
                .frame(width: 30, height: 32)
                .background(
                    store.metronome ? Theme.accent : Theme.paper, in: RoundedRectangle(cornerRadius: 9)
                )
                .overlay(RoundedRectangle(cornerRadius: 9).stroke(store.metronome ? Theme.accent : Theme.line))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help("Metronome (K): click during playback and recording · \(store.metronome ? "On" : "Off")")
        .accessibilityLabel("Metronome")
        .accessibilityValue(store.metronome ? "On" : "Off")
        .accessibilityAddTraits(store.metronome ? .isSelected : [])
    }
}

/// Hardware input level for the inputs this session records from, so the
/// microphone gain is set here rather than in System Settings.
struct InputVolumeButton: View {
    @ObservedObject var store: SessionStore
    @State private var open = false
    var body: some View {
        Button {
            open.toggle()
        } label: {
            Image(systemName: "mic.and.signal.meter").font(.system(size: 13)).foregroundStyle(Theme.secondary)
                .frame(width: 26, height: 30).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help("Input volume")
        .accessibilityLabel("Input volume")
        .popover(isPresented: $open, arrowEdge: .bottom) {
            VStack(alignment: .leading, spacing: 12) {
                Text("Input volume").font(.system(size: 13, weight: .semibold))
                InputVolumeList(store: store)
            }
            .padding(16).frame(width: 300)
            .background(Theme.paper).foregroundStyle(Theme.ink).tint(Theme.accent).preferredColorScheme(.dark)
        }
    }
}

struct InputVolumeList: View {
    @ObservedObject var store: SessionStore
    var body: some View {
        InputVolumeRows(store: store, model: store.inputVolume)
    }
}

private struct InputVolumeRows: View {
    @ObservedObject var store: SessionStore
    @ObservedObject var model: InputVolumeModel
    var body: some View {
        let devices = store.inputVolumeDevices
        VStack(alignment: .leading, spacing: 10) {
            if devices.isEmpty {
                Text("Choose an input in Audio Connections.").font(.system(size: 11)).foregroundStyle(Theme.secondary)
            }
            ForEach(devices) { device in InputVolumeRow(store: store, level: model.levels[device.uid], device: device) }
        }
        .onAppear { devices.forEach { store.loadInputVolume(uid: $0.uid) } }
    }
}

private struct InputVolumeRow: View {
    @ObservedObject var store: SessionStore
    let level: InputVolumeLevel?
    let device: AudioDevice
    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(device.name).font(.system(size: 11)).lineLimit(1).truncationMode(.middle)
                Spacer()
                Text(readout).font(.system(size: 11).monospacedDigit()).foregroundStyle(Theme.secondary)
            }
            switch level {
            case .adjustable(let volume):
                RoomSlider(
                    value: Binding(get: { volume }, set: { store.setInputVolume($0, uid: device.uid) }), in: 0...1,
                    label: "\(device.name) input volume"
                )
                .disabled(store.remoteGUI?.connected != true)
            case .fixed:
                Text("This device has no adjustable input level. Use its own gain control.")
                    .font(.system(size: 10)).foregroundStyle(Theme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            case .unavailable(let message):
                Text(message).font(.system(size: 10)).foregroundStyle(Theme.warning)
                    .fixedSize(horizontal: false, vertical: true)
            case nil:
                ProgressView().controlSize(.small)
            }
        }
    }
    private var readout: String {
        guard case .adjustable(let volume) = level else { return "" }
        return "\(Int((volume * 100).rounded())) %"
    }
}
