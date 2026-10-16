import Foundation
import SoundroomCore
let root = URL(fileURLWithPath: "build/Demos/Organic palette", isDirectory: true)
try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
let chords = [[53,60,65,68], [49,56,60,65], [51,58,63,67], [48,55,60,63]]
var notes: [SoundDesignNote] = []
for (bar, chord) in chords.enumerated() {
    for (i, pitch) in chord.enumerated() {
        notes.append(.init(pitch: pitch, velocity: 0.57 + Double(i) * 0.055, startBeat: Double(bar * 4) + Double(i) * 0.04, durationBeats: 2.8))
    }
    for (i,pitch) in [chord[2] + 12, chord[3] + 12, chord[2] + 12].enumerated() {
        notes.append(.init(pitch: pitch, velocity: 0.63 + Double(i % 2) * 0.11, startBeat: Double(bar * 4) + 2.5 + Double(i) * 0.5, durationBeats: 0.35))
    }
}
let source = SoundDesignPattern(notes: notes, bpm: 96, beats: 16)
let encoder = JSONEncoder(); encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
var session = Session(); session.name = "Organic palette"; session.tracks = []
let project = root.appendingPathComponent("Organic palette.soundroom", isDirectory: true)
let media = project.appendingPathComponent("Audio", isDirectory: true)
try FileManager.default.createDirectory(at: media, withIntermediateDirectories: true)
struct MediaInfo: Codable { let frames: Int; let channels: Int }
var metadata: [UUID: MediaInfo] = [:]
for (i,patch) in OrganicPatch.allCases.enumerated() {
    let p = patch.applying(to: source), audio = p.render(), id = UUID()
    let wave = WaveFile.encode(audio, float: true)
    try wave.write(to: root.appendingPathComponent(patch.rawValue + ".wav"), options: .atomic)
    try encoder.encode(DesignerSoundPreset(pattern: p)).write(to: root.appendingPathComponent(patch.rawValue + ".soundroom-preset.json"))
    try wave.write(to: media.appendingPathComponent(id.uuidString + ".wav"))
    metadata[id] = .init(frames: audio.count, channels: audio.channelCount)
    var track = Track(name: patch.rawValue, color: i)
    var clip = Clip(assetID: id, name: patch.rawValue, start: i * Int(12 * sessionRate), length: audio.count)
    clip.soundDesign = p; track.clips = [clip]; session.tracks.append(track)
    let peak = audio.channels.flatMap { $0 }.map(abs).max() ?? 0
    print("\(patch.rawValue): \(audio.count) frames, peak \(20 * log10(Double(peak))) dBFS")
}
session.version = session.requiredProjectVersion
try encoder.encode(session).write(to: project.appendingPathComponent("session.json"))
try encoder.encode(metadata).write(to: project.appendingPathComponent("media.json"))
