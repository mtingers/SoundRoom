import Foundation

/// Offline references for the legacy scalar gain range and normalized automation.
/// This tool never initializes an audio device or a client/server process.
@main struct GenerateGainFixtures {
    static func main() throws {
        guard CommandLine.arguments.count == 2 else {
            throw NSError(
                domain: "GainFixtures", code: 1,
                userInfo: [NSLocalizedDescriptionKey: "Supply a new fixture output directory"])
        }
        let root = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
        let manager = FileManager.default
        guard !manager.fileExists(atPath: root.path) else {
            throw NSError(
                domain: "GainFixtures", code: 2,
                userInfo: [NSLocalizedDescriptionKey: "Output directory already exists"])
        }
        try manager.createDirectory(at: root, withIntermediateDirectories: true)
        let assetID = UUID(uuidString: "11111111-1111-4111-8111-111111111111")!
        let audio = AudioSamples(channels: [
            (0..<8192).map { Float(sin(Double($0) * 0.071)) * 0.035 },
            (0..<8192).map { Float(cos(Double($0) * 0.053)) * 0.029 },
        ])
        let cases: [(String, Float, Float, Bool, Bool, AutomationValueScale)] = [
            ("scalar-track", 3.25, 1, false, false, .linear),
            ("scalar-master", 1, 4, false, false, .linear),
            ("scalar-both", 2.75, 1.8, false, false, .linear),
            ("delayed-track-linear", 4, 1, true, false, .linear),
            ("delayed-track-decibels", 2, 1, true, false, .decibels),
            ("delayed-master-linear", 1, 3, false, true, .linear),
            ("delayed-master-decibels", 1, 2.5, false, true, .decibels),
            ("delayed-both", 4, 4, true, true, .decibels),
        ]
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        for (name, trackGain, masterGain, trackAutomated, masterAutomated, scale) in cases {
            var session = Session()
            session.version = name == "scalar-track" ? 1 : 9
            session.name = name
            var track = Track(name: "Gain reference", color: 2)
            track.id = UUID(uuidString: "22222222-2222-4222-8222-222222222222")!
            track.gain = trackGain
            track.pan = 0.35
            var clip = Clip(assetID: assetID, name: "Source", start: 0, length: audio.count, edgeFadeFrames: 240)
            clip.id = UUID(uuidString: "33333333-3333-4333-8333-333333333333")!
            track.clips = [clip]
            if trackAutomated {
                track.volumeAutomationScale = scale
                track.mixAutomation["volume"] = [
                    .init(frame: 2048, value: 0.42, curve: .smooth),
                    .init(frame: 6144, value: 0.84),
                ]
            }
            session.tracks = [track]
            session.masterTrack.gain = masterGain
            if masterAutomated {
                session.masterTrack.volumeAutomationScale = scale
                session.masterTrack.mixAutomation["volume"] = [
                    .init(frame: 1024, value: 0.73, curve: .easeOut),
                    .init(frame: 7168, value: 0.28),
                ]
            }
            let folder = root.appendingPathComponent(name + ".soundroom", isDirectory: true)
            try manager.createDirectory(at: folder.appendingPathComponent("Audio"), withIntermediateDirectories: true)
            try encoder.encode(session).write(to: folder.appendingPathComponent("session.json"))
            try WaveFile.encode(audio, float: true).write(
                to: folder.appendingPathComponent("Audio/" + assetID.uuidString + ".wav"))
            let mixer = RealtimeMixer(session: session, assets: [assetID: audio])
            var left = [Float](repeating: 0, count: audio.count)
            var right = left
            var offset = 0
            while offset < audio.count {
                let count = min(257, audio.count - offset)
                left.withUnsafeMutableBufferPointer { l in
                    right.withUnsafeMutableBufferPointer { r in
                        mixer.render(
                            intoLeft: l.baseAddress! + offset, intoRight: r.baseAddress! + offset, frameCount: count)
                    }
                }
                offset += count
            }
            try WaveFile.encode(AudioSamples(channels: [left, right]), float: true)
                .write(to: root.appendingPathComponent(name + "-swift.wav"))
        }
    }
}
