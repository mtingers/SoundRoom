import Foundation
import SoundroomCore

struct SampleManifest: Codable {
    let path: String
    let kit: DrumKit
    let voice: DrumVoice
    let velocity: Int
    let roundRobin: Int
    let frames: Int
    let peak: Float
    let rms: Double
}
struct PackManifest: Codable {
    let version: Int
    let sampleRate: Int
    let bitDepth: Int
    let channels: Int
    let kits: [DrumKit]
    let velocities: [Int]
    let roundRobins: Int
    let provenance: String
    let license: String
    let samples: [SampleManifest]
}
let arguments = CommandLine.arguments
let path = arguments.dropFirst().first ?? "build/DrumPacks"
let velocities = [16, 32, 48, 64, 80, 96, 112, 127]
let roundRobins = 8
let destination = URL(fileURLWithPath: path, isDirectory: true)
do {
    try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
    var manifest: [SampleManifest] = []
    for kit in DrumKit.allCases {
        let folder = destination.appendingPathComponent(kit.rawValue, isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        for voice in DrumVoice.allCases {
            for velocity in velocities {
                for rr in 0..<roundRobins {
                    let samples = DrumSynthesis.render(
                        kit: kit, voice: voice, velocity: Double(velocity) / 127, roundRobin: rr)
                    let channel = samples.channels[0]
                    let peak = channel.map { abs($0) }.max() ?? 0
                    guard channel.allSatisfy({ $0.isFinite }), peak < 1, channel.first == 0, channel.last == 0 else {
                        throw NSError(
                            domain: "DrumPack", code: 1,
                            userInfo: [NSLocalizedDescriptionKey: "Invalid output: \(kit) \(voice) \(velocity) \(rr)"])
                    }
                    let filename = "\(voice.rawValue)_v\(velocity)_rr\(rr + 1).wav"
                    try WaveFile.encode(samples).write(to: folder.appendingPathComponent(filename), options: .atomic)
                    let rms = sqrt(channel.reduce(0.0) { $0 + Double($1) * Double($1) } / Double(channel.count))
                    manifest.append(
                        SampleManifest(
                            path: "\(kit.rawValue)/\(filename)", kit: kit, voice: voice, velocity: velocity,
                            roundRobin: rr + 1, frames: samples.count, peak: peak, rms: rms))
                }
            }
        }
        var pattern = DrumPattern()
        pattern.apply(kit.isAcoustic ? .backbeat : .house)
        pattern.kit = kit
        pattern.variationCycles = 4
        pattern.velocityHumanize = kit.isAcoustic ? 0.035 : 0
        pattern.humanize = kit.isAcoustic ? 0.003 : 0
        try WaveFile.encode(pattern.render(repeats: 4)).write(
            to: destination.appendingPathComponent("\(kit.rawValue)-demo-110bpm.wav"), options: .atomic)
        print("Generated \(kit.title)")
    }
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
    try encoder.encode(
        PackManifest(
            version: 2, sampleRate: 48000, bitDepth: 24, channels: 1, kits: DrumKit.allCases, velocities: velocities,
            roundRobins: roundRobins,
            provenance:
                "Original deterministic procedural membrane/modal cymbal and electronic synthesis. No recorded or external source samples.",
            license: "CC0-1.0", samples: manifest)
    ).write(to: destination.appendingPathComponent("manifest.json"), options: .atomic)
    let readme = """
        # Soundroom Raw Drums

        \(manifest.count) original dry mono samples: seven kits, nine voices, eight velocities (16, 32, 48, 64, 80, 96, 112, 127), eight round robins. 48 kHz / 24-bit PCM WAV. Four-bar stereo demo loops at 110 BPM are included.

        Acoustic studio is a physically inspired membrane, snare-wire and inharmonic cymbal model, not an acoustic recording. Electronic analog uses pitch envelopes, resonant tones, noise and metal oscillators. Maple jazz, birch rock, vintage dry, brushes and orchestral add distinct modeled excitation, shell/wire balance and damping profiles. All are original procedural sounds. No reverb, compression, limiter or peak normalization is baked in. Consistent gains preserve velocity dynamics; harder strikes also alter tone and mode balance. Round robins vary strike position, tuning and excitation. Start/end samples are silent, with short edge envelopes. Tune, layer and mix to taste.

        Files: `<voice>_v<MIDI velocity>_rr<1–8>.wav`. Choose velocity layers using the midpoint between neighboring layer values, or crossfade them. Built-in playback synthesizes velocity continuously; these exported layers sample that range. The pack contains default center strikes; rimshot, side-stick, brush and bell controls are available inside Soundroom. Cycle round robins per voice. Closed hats should choke open hats with a 2–5 ms release. The included manifest lists every file's peak/RMS/frame count.

        Nine voices: kick, snare, closed/open hat, low/high tom, clap, ride and crash. Acoustic clap is a modeled hand clap. Subjective acoustic realism and production suitability depend on context; these should not be represented as recordings of a real kit.

        Regenerate: `swift run -c release DrumPackGenerator build/DrumPacks`.

        Samples and demo audio: CC0 1.0 Universal, original Soundroom synthesis, no attribution required. You may use, modify and distribute them commercially. Source code keeps the repository's license. See LICENSE.txt.
        """
    try Data(readme.utf8).write(to: destination.appendingPathComponent("README.md"), options: .atomic)
    try Data(
        "Soundroom Raw Drums: original generated sample and demo audio dedicated to the public domain under CC0 1.0 Universal. No attribution required. https://creativecommons.org/publicdomain/zero/1.0/\n"
            .utf8
    ).write(to: destination.appendingPathComponent("LICENSE.txt"), options: .atomic)
    print(
        "Generated \(manifest.count) original samples and \(DrumKit.allCases.count) demo loops at \(destination.path)")
} catch {
    FileHandle.standardError.write(Data("Drum pack generation failed: \(error.localizedDescription)\n".utf8))
    exit(1)
}
