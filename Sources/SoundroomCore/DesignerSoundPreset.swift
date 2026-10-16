import Foundation

/// A sound preset contains no song data. Loading it never replaces notes or tempo.
public struct DesignerSoundPreset: Codable, Equatable, Sendable {
    public let version: Int
    public let sound: SoundDesignPattern
    public init(pattern: SoundDesignPattern) {
        version = 1
        var p = pattern; p.notes = []; p.controllers = []; p.bpm = 120; p.beats = 4
        sound = p
    }
    public func applying(to sequence: SoundDesignPattern) -> SoundDesignPattern {
        var p = sound; p.notes = sequence.notes; p.controllers = sequence.controllers
        p.bpm = sequence.bpm; p.beats = sequence.beats
        return p
    }
}
