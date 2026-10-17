import Foundation
import SoundroomCore

@main struct GenerateSoundPuttyFixtures {
    static func main() throws {
        let root = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let asset = UUID(uuidString: "11111111-1111-4111-8111-111111111111")!
        let signal = AudioSamples(
            channels: (0..<2).map { channel in
                (0..<24000).map { i in
                    let t = Double(i) / 48000
                    return Float(
                        (sin(2 * .pi * (channel == 0 ? 173 : 281) * t) + sin(2 * .pi * 489 * t) * 0.3) * 0.2
                            + (i % 6000 == 17 ? 0.3 : 0))
                }
            })
        for (name, edits) in [
            ("pluck", [String: Float]()), ("strike", ["excite": 1, "wildMode": 3, "coupling": 0.25]),
            ("blow", ["excite": 2, "wildMode": 2, "width": 48]), ("phase", ["wildMode": 1, "intensity": 1]),
            ("sculpt", ["puttyPitch299": 0.9, "puttyEnergy145": 0.7, "puttyDamping030": -1, "variationSeed": 9245]),
            ("tail", ["decay": 0.1, "mix": 0.7]),
        ] {
            let folder = root.appendingPathComponent(name + ".soundroom", isDirectory: true)
            try FileManager.default.createDirectory(
                at: folder.appendingPathComponent("Audio"), withIntermediateDirectories: true)
            var effect = RealtimeEffect(
                kind: .soundPutty,
                parameters: RealtimeEffectKind.soundPutty.defaultParameters.merging(edits) { _, new in new })
            effect.id = UUID(uuidString: "55555555-5555-4555-8555-555555555555")!
            if name == "sculpt" {
                effect.parameterAutomation = [
                    "mix": [.init(frame: 0, value: 0.3), .init(frame: 21000, value: 1)],
                    "wild": [.init(frame: 0, value: 0.2), .init(frame: 21000, value: 0.8)],
                ]
            }
            if name == "tail" { for lane in 0..<300 { effect.parameters[SoundPuttyShape.key(.damping, lane)] = 0.7 } }
            var session = Session()
            session.version = 9
            var track = Track(name: "Sound Putty", color: 2)
            var clip = Clip(assetID: asset, name: "Parity source", start: 0, length: signal.count)
            clip.edgeProtectionDisabled = true
            track.clips = [clip]
            track.realtimeEffects = [effect]
            session.tracks = [track]
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
            try encoder.encode(session).write(to: folder.appendingPathComponent("session.json"))
            try WaveFile.encode(signal, float: true).write(
                to: folder.appendingPathComponent("Audio/" + asset.uuidString + ".wav"))
            let includeTails = name == "tail"
            let mixer = RealtimeMixer(session: session, assets: [asset: signal], includeEffectTails: includeTails)
            let frames = signal.count + (includeTails ? RealtimeMixer.tailFrames(session: session) : 0)
            var left = [Float](repeating: 0, count: frames)
            var right = left
            let start = Date()
            left.withUnsafeMutableBufferPointer { l in
                right.withUnsafeMutableBufferPointer { r in
                    var offset = 0
                    while offset < frames {
                        let count = min(257, frames - offset)
                        mixer.render(
                            intoLeft: l.baseAddress! + offset, intoRight: r.baseAddress! + offset, frameCount: count)
                        offset += count
                    }
                }
            }
            try WaveFile.encode(AudioSamples(channels: [left, right]), float: true).write(
                to: root.appendingPathComponent(name + "-swift.wav"))
            print("\(name): 0.5 seconds stereo rendered in \(Date().timeIntervalSince(start)) seconds")
        }
    }
}
