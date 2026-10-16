import Foundation
import CoreAudio

struct AudioDevice: Identifiable, Hashable {
    let id: AudioDeviceID
    let name: String
    var uid: String = ""
    let inputChannels: Int
    let outputChannels: Int
}

enum SoundroomError: LocalizedError {
    case message(String)
    var errorDescription: String? { if case let .message(s) = self { return s }; return nil }
}

