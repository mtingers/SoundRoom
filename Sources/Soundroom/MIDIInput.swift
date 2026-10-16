import Foundation
import CoreMIDI
import Combine
import SoundroomCore

struct DesignerMIDISource: Identifiable, Equatable {
    let id: MIDIEndpointRef
    let name: String
}

/// MIDI byte-stream parser keeps running status across packets and ignores realtime
/// bytes without corrupting an in-progress note. Used on the main queue, off audio DSP.
enum DesignerMIDIEvent: Equatable {
    case note(pitch: Int, velocity: Int, on: Bool)
    case controller(SoundDesignControllerKind, Double)
    case allNotesOff
}
struct DesignerMIDIParser {
    private var status: UInt8 = 0
    private var data: [UInt8] = []
    private var sysex = false
    mutating func receive(_ bytes: [UInt8]) -> [(pitch: Int, velocity: Int, on: Bool)] {
        receiveEvents(bytes).compactMap { event in
            if case .note(let pitch, let velocity, let on) = event { return (pitch, velocity, on) }; return nil
        }
    }
    mutating func receiveEvents(_ bytes: [UInt8]) -> [DesignerMIDIEvent] {
        var output: [DesignerMIDIEvent] = []
        for byte in bytes {
            if byte >= 0xF8 { continue }
            if byte >= 0x80 {
                data.removeAll(keepingCapacity: true)
                if byte == 0xF0 { sysex = true; status = 0 }
                else if byte == 0xF7 { sysex = false; status = 0 }
                else if byte < 0xF0 { sysex = false; status = byte }
                else { status = 0 }
                continue
            }
            guard !sysex, status >= 0x80 && status < 0xF0 else { continue }
            data.append(byte)
            let command = status & 0xF0, needed = command == 0xC0 || command == 0xD0 ? 1 : 2
            guard data.count == needed else { continue }
            if command == 0x90 || command == 0x80 {
                output.append(.note(pitch: Int(data[0]), velocity: Int(data[1]), on: command == 0x90 && data[1] > 0))
            } else if command == 0xB0 {
                let kind: SoundDesignControllerKind?
                switch data[0] { case 1: kind = .modulation; case 7, 11: kind = .expression; case 64: kind = .sustain; case 120, 123: output.append(.allNotesOff); kind = nil; default: kind = nil }
                if let kind { output.append(.controller(kind, Double(data[1]) / 127)) }
            } else if command == 0xE0 {
                let raw = Int(data[0]) | Int(data[1]) << 7
                let value = raw <= 8192 ? Double(raw) / 16384 : 0.5 + Double(raw - 8192) / 16382
                output.append(.controller(.pitchBend, value))
            }
            data.removeAll(keepingCapacity: true)
        }
        return output
    }
}

/// Server MIDI inventory and selection; this controller never opens local ports.
@MainActor
final class DesignerMIDIInput: ObservableObject {
    @Published private(set) var sources: [DesignerMIDISource] = []
    @Published private(set) var connected: MIDIEndpointRef = 0
    @Published private(set) var error: String?
    var onNote: ((Int, Int, Bool) -> Void)?
    var onController: ((SoundDesignControllerKind, Double) -> Void)?
    var onAllNotesOff: (() -> Void)?
    private var remoteConnect: ((UInt32) -> Void)?

    func configureRemote(sources: [DesignerMIDISource], onConnect: @escaping (UInt32) -> Void) {
        self.sources = sources; remoteConnect = onConnect
    }
    func updateRemoteSources(_ sources: [DesignerMIDISource]) { self.sources = sources }
    func connect(_ endpoint: MIDIEndpointRef) {
        guard let remoteConnect else { error = "Connect to the audio server before selecting a MIDI source."; return }
        connected = endpoint; error = nil; remoteConnect(endpoint)
    }
    func close() {
        remoteConnect?(0); connected = 0; remoteConnect = nil
        onNote = nil; onController = nil; onAllNotesOff = nil
    }
}
