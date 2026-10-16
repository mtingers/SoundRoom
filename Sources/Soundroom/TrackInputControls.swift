import SwiftUI
import SoundroomCore

struct TrackInputControls: View {
    @ObservedObject var store: SessionStore
    let track: Track
    private var device: AudioDevice? {
        if let uid = track.inputDeviceUID { return store.devices.first { $0.uid == uid && $0.inputChannels > 0 } }
        return store.devices.first { $0.id == store.inputDevice && $0.inputChannels > 0 }
    }
    private var label: String {
        let name = track.inputDeviceUID == nil ? "Session input" : (device?.name ?? "Missing: \(track.inputDeviceName ?? "input")")
        let channels = track.deviceInputChannels.compactMap { $0 < Int.max ? String($0 + 1) : nil }.joined(separator: "–")
        return "\(name) · \(channels.isEmpty ? "Unavailable channel" : channels)"
    }
    var body: some View {
        Menu {
            Menu("Session input\(store.devices.first { $0.id == store.inputDevice }.map { " · " + $0.name } ?? "")") {
                routes(device: store.devices.first { $0.id == store.inputDevice && $0.inputChannels > 0 }, followsSession: true)
            }
            Divider()
            ForEach(store.devices.filter { $0.inputChannels > 0 && !$0.uid.isEmpty }) { input in
                Menu(input.name) { routes(device: input, followsSession: false) }
            }
            if track.inputDeviceUID != nil && device == nil {
                Divider(); Text("\(track.inputDeviceName ?? "Saved input") is disconnected")
            }
        } label: { Text(label).font(.system(size: 10)).lineLimit(1).truncationMode(.middle) }
        .menuStyle(.borderlessButton).frame(width: 108)
        .disabled(store.isRecording || store.busy)
        .help("\(label). Choose a microphone and mono channel or stereo pair for this track.")
        .accessibilityLabel("\(track.name) recording input")
        .accessibilityValue(label)
    }
    @ViewBuilder private func routes(device: AudioDevice?, followsSession: Bool) -> some View {
        if let device {
            ForEach(0..<device.inputChannels, id: \.self) { channel in
                Button("Mono · Input \(channel + 1)") { route(device, followsSession: followsSession, channel: channel, stereo: false) }
                if channel + 1 < device.inputChannels {
                    Button("Stereo · Inputs \(channel + 1)–\(channel + 2)") { route(device, followsSession: followsSession, channel: channel, stereo: true) }
                }
            }
        } else { Text("Choose a session input in Audio Connections") }
    }
    private func route(_ device: AudioDevice, followsSession: Bool, channel: Int, stereo: Bool) {
        store.updateTrack(track.id, actionName: "Input routing") {
            $0.inputDeviceUID = followsSession ? nil : device.uid
            $0.inputDeviceName = followsSession ? nil : device.name
            $0.inputChannel = channel; $0.stereoInput = stereo
        }
        store.focusEditor()
    }
}
