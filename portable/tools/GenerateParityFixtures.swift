import Foundation

@main struct GenerateParityFixtures {
    static func main() throws {
        let root = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
        let manager = FileManager.default
        let assetID = UUID(uuidString: "11111111-1111-4111-8111-111111111111")!
        let samples = AudioSamples(channels: [
            (0..<14000).map { i in Float(sin(Double(i) * 0.067)) * 0.2 + (i == 0 ? 0.4 : 0) },
            (0..<14000).map { i in Float(cos(Double(i) * 0.093)) * 0.17 },
        ])
        let kinds: [(String, RealtimeEffect?)] = [
            ("envelopes", nil),
            ("legacy-gain", RealtimeEffect(kind: .gain, amount: 0.63)),
            ("legacy-lowpass", RealtimeEffect(kind: .lowPass, amount: 0.82)),
            ("legacy-echo", RealtimeEffect(kind: .echo, amount: 0.57)),
            ("legacy-warmth", RealtimeEffect(kind: .warmth, amount: 0.34)),
            ("modern-gain", RealtimeEffect(kind: .gain, parameters: ["gain": 2.7])),
            ("modern-lowpass", RealtimeEffect(kind: .lowPass, parameters: ["frequency": 1650, "resonance": 1.7])),
            ("modern-highpass", RealtimeEffect(kind: .highPass, parameters: ["frequency": 820, "resonance": 0.88])),
            (
                "modern-echo",
                RealtimeEffect(kind: .echo, parameters: ["time": 11.2, "feedback": 0.44, "mix": 0.37, "damping": 5800])
            ),
            (
                "compressor",
                RealtimeEffect(
                    kind: .compressor,
                    parameters: [
                        "threshold": -26, "ratio": 5.2, "attack": 3.7, "release": 77, "knee": 4.2, "makeup": 3,
                        "mix": 0.86,
                    ])
            ),
            (
                "chorus",
                RealtimeEffect(
                    kind: .chorus,
                    parameters: ["rate": 6.2, "depth": 7, "delay": 11, "width": 0.83, "feedback": 0.32, "mix": 0.61])
            ),
            ("dither", RealtimeEffect(kind: .dither, parameters: ["bits": 16, "amount": 0.9, "quantize": 1])),
            (
                "equalizer",
                RealtimeEffect(
                    kind: .equalizer,
                    parameters: [
                        "lowGain": 4.1, "midFrequency": 980, "midGain": -6.2, "midQ": 1.9, "highGain": 3.2,
                        "output": -2, "lowFrequency": 220, "lowQ": 0.94, "lowShape": 1, "highFrequency": 6800,
                        "highQ": 0.72, "highShape": 1, "highPassFrequency": 90, "highPassEnabled": 1,
                        "lowPassFrequency": 10000, "lowPassEnabled": 1,
                    ])
            ),
            (
                "room-classic",
                RealtimeEffect(
                    kind: .room,
                    parameters: [
                        "decay": 0.28, "predelay": 0.004, "damping": 5800, "mix": 0.67, "character": 0, "output": -1.2,
                    ])
            ),
            (
                "room-dense",
                RealtimeEffect(
                    kind: .room,
                    parameters: [
                        "decay": 0.23, "predelay": 3.2, "damping": 4100, "mix": 0.58, "character": 1, "lowCut": 190,
                        "diffusion": 0.83, "width": 0.72, "early": 0.32, "output": -0.7,
                    ])
            ),
            (
                "room-transition",
                RealtimeEffect(
                    kind: .room,
                    parameters: ["decay": 0.31, "predelay": 5.1, "damping": 6100, "mix": 0.43, "character": 0])
            ),
            (
                "warmth-tube",
                RealtimeEffect(
                    kind: .warmth,
                    parameters: [
                        "drive": 15.3, "tone": 7300, "mix": 0.77, "output": 0.8, "match": 1, "character": 2,
                        "bias": 0.41, "emphasis": 4.2,
                    ])
            ),
            (
                "warmth-transition",
                RealtimeEffect(
                    kind: .warmth,
                    parameters: [
                        "drive": 12.1, "tone": 8200, "mix": 0.9, "output": -1.2, "character": 1, "bias": 0.63,
                        "emphasis": 2.3,
                    ])
            ),
            (
                "limiter-track",
                RealtimeEffect(
                    kind: .limiter, parameters: ["input": 18, "ceiling": -3, "release": 85, "style": 1, "truePeak": 1])
            ),
            (
                "limiter-master",
                RealtimeEffect(
                    kind: .limiter, parameters: ["input": 18, "ceiling": -3, "release": 85, "style": 1, "truePeak": 1])
            ),
            (
                "limiter-bypass",
                RealtimeEffect(kind: .limiter, enabled: false, parameters: ["input": 18, "ceiling": -3])
            ),
            (
                "resonance-track",
                RealtimeEffect(
                    kind: .resonance,
                    parameters: [
                        "depth": 14, "sensitivity": 0.95, "sharpness": 0.78, "low": 300, "high": 6000, "attack": 3.2,
                        "release": 50, "transients": 0.24, "mix": 0.83,
                    ])
            ),
            (
                "resonance-master",
                RealtimeEffect(
                    kind: .resonance,
                    parameters: [
                        "depth": 14, "sensitivity": 0.95, "sharpness": 0.78, "low": 300, "high": 6000, "attack": 3.2,
                        "release": 50, "transients": 0.24, "mix": 0.83,
                    ])
            ),
            (
                "resonance-delta",
                RealtimeEffect(
                    kind: .resonance,
                    parameters: [
                        "depth": 14, "sensitivity": 0.95, "low": 300, "high": 6000, "attack": 3.2, "release": 50,
                        "transients": 0.24, "mix": 0.83, "delta": 1,
                    ])
            ),
            (
                "resonance-bypass",
                RealtimeEffect(kind: .resonance, enabled: false, parameters: ["depth": 14, "sensitivity": 0.95])
            ),
            (
                "latency-chain",
                RealtimeEffect(
                    kind: .resonance,
                    parameters: ["depth": 9, "sensitivity": 0.85, "low": 300, "high": 9000, "mix": 0.72])
            ),
            (
                "tail-room",
                RealtimeEffect(
                    kind: .room,
                    parameters: ["decay": 0.11, "predelay": 2.2, "damping": 6200, "mix": 0.61, "character": 1])
            ),
            (
                "tail-chorus",
                RealtimeEffect(
                    kind: .chorus, parameters: ["rate": 1.2, "depth": 4, "delay": 7, "feedback": 0.23, "mix": 0.6])
            ),
            ("tail-echo", RealtimeEffect(kind: .echo, amount: 0.4)),
            (
                "limiter-sample",
                RealtimeEffect(
                    kind: .limiter,
                    parameters: ["input": 21, "ceiling": -2.2, "release": 57, "style": 0, "truePeak": 0])
            ),
            ("dither-only", RealtimeEffect(kind: .dither, parameters: ["bits": 19, "amount": 0.81, "quantize": 0])),
            (
                "warmth-diode",
                RealtimeEffect(
                    kind: .warmth,
                    parameters: [
                        "drive": 11.8, "tone": 5300, "mix": 0.83, "character": 4, "bias": 0.55, "emphasis": 2.7,
                    ])
            ),
            (
                "warmth-classic",
                RealtimeEffect(kind: .warmth, parameters: ["drive": 13.6, "tone": 9300, "mix": 0.81, "character": 0])
            ),
            (
                "mixed-latencies",
                RealtimeEffect(kind: .resonance, parameters: ["depth": 11.3, "sensitivity": 0.76, "mix": 0.81])
            ),
            ("pitch-automatic", RealtimeEffect(kind: .pitchCorrection)),
            (
                "pitch-major",
                RealtimeEffect(
                    kind: .pitchCorrection,
                    parameters: [
                        "key": 3, "scale": 1, "reference": 442, "retune": 11, "humanize": 0.12, "vibrato": 0.73,
                        "tolerance": 0,
                    ])
            ),
            (
                "pitch-custom",
                RealtimeEffect(
                    kind: .pitchCorrection,
                    parameters: ["scale": 3, "mask": 1161, "retune": 0, "humanize": 0, "vibrato": 0])
            ),
            ("pitch-mask-zero", RealtimeEffect(kind: .pitchCorrection, parameters: ["scale": 3, "mask": 0])),
            (
                "pitch-up",
                RealtimeEffect(
                    kind: .pitchCorrection,
                    parameters: ["manualTarget": 62.3, "retune": 0, "humanize": 0, "vibrato": 0, "tolerance": 0])
            ),
            (
                "pitch-down",
                RealtimeEffect(
                    kind: .pitchCorrection,
                    parameters: ["manualTarget": 45, "retune": 0, "humanize": 0, "vibrato": 0, "tolerance": 0])
            ),
            (
                "pitch-formant",
                RealtimeEffect(
                    kind: .pitchCorrection,
                    parameters: ["manualTarget": 60.4, "formant": -7.3, "strength": 0.78, "retune": 7, "mix": 0.83])
            ),
            (
                "pitch-edits",
                RealtimeEffect(kind: .pitchCorrection, parameters: ["retune": 3, "tolerance": 0, "humanize": 0])
            ),
            (
                "pitch-automation",
                RealtimeEffect(kind: .pitchCorrection, parameters: ["retune": 9, "humanize": 0.1, "tolerance": 0])
            ),
            ("pitch-bypass", RealtimeEffect(kind: .pitchCorrection, enabled: false)),
            ("pitch-master", RealtimeEffect(kind: .pitchCorrection, parameters: ["scale": 2, "key": 4, "retune": 23])),
            (
                "pitch-chain",
                RealtimeEffect(kind: .pitchCorrection, parameters: ["manualTarget": 60.4, "retune": 1, "humanize": 0])
            ),

        ]
        for (name, effect) in kinds {
            let samples =
                name.hasPrefix("pitch-")
                ? AudioSamples(channels: [
                    (0..<24000).map { i in
                        let time = Double(i) / sessionRate
                        let frequency = i < 13000 ? 224.75 : 277.81
                        let phase = 2 * Double.pi * frequency * time + 0.025 * sin(2 * Double.pi * 5 * time)
                        let gate: Double = (8000..<9500).contains(i) ? 0 : 0.2
                        return Float(gate * (sin(phase) + 0.32 * sin(phase * 2 + 0.1) + 0.13 * sin(phase * 3 + 0.3)))
                    },
                    (0..<24000).map { i in
                        let time = Double(i) / sessionRate
                        let frequency = i < 13000 ? 224.75 : 277.81
                        let phase = 2 * Double.pi * frequency * time + 0.025 * sin(2 * Double.pi * 5 * time)
                        let gate: Double = (8000..<9500).contains(i) ? 0 : -0.24
                        return Float(gate * (sin(phase) + 0.32 * sin(phase * 2 + 0.1) + 0.13 * sin(phase * 3 + 0.3)))
                    },
                ]) : samples
            let folder = root.appendingPathComponent(name + ".soundroom", isDirectory: true)
            try manager.createDirectory(
                at: folder.appendingPathComponent("Audio", isDirectory: true), withIntermediateDirectories: true)
            var session = Session()
            session.version = 9
            session.name = name
            session.timing = MusicalTiming(tempo: 123, beatsPerBar: 7, beatUnit: 8)
            var track = Track(name: "Parity", color: 2)
            track.id = UUID(uuidString: "22222222-2222-4222-8222-222222222222")!
            track.gain = 0.82
            track.pan = 0.21
            var clip = Clip(assetID: assetID, name: "Source", start: 0, length: samples.count, edgeFadeFrames: 240)
            clip.id = UUID(uuidString: "33333333-3333-4333-8333-333333333333")!
            track.clips = [clip]
            if name == "envelopes" {
                var left = clip
                left.length = 7500
                left.fadeInFrames = 630
                var right = clip
                right.id = UUID(uuidString: "44444444-4444-4444-8444-444444444444")!
                right.start = 7000
                right.offset = 7000
                right.length = 7000
                right.fadeOutFrames = 960
                track.clips = [left, right]
            }
            if var effect {
                effect.id = UUID(uuidString: "55555555-5555-4555-8555-555555555555")!
                if name == "legacy-gain" {
                    effect.automation = [
                        AutomationPoint(frame: 0, value: 0.42, curve: .smooth),
                        AutomationPoint(frame: 7000, value: 0.76),
                    ]
                }
                if name == "modern-lowpass" {
                    effect.parameterAutomation = [
                        "frequency": [
                            AutomationPoint(frame: 0, value: 0.3, curve: .easeIn, curveStart: 0.2, curveEnd: 0.9),
                            AutomationPoint(frame: 7000, value: 0.74),
                        ]
                    ]
                }
                if name == "modern-echo" { effect.bakedRanges = [PitchEditRange(startFrame: 4000, endFrame: 4200)] }
                if name == "equalizer" {
                    effect.parameterAutomation = [
                        "lowShape": [.init(frame: 0, value: 0, curve: .hold), .init(frame: 7000, value: 1)],
                        "highPassEnabled": [.init(frame: 0, value: 0, curve: .hold), .init(frame: 6000, value: 1)],
                        "midGain": [.init(frame: 0, value: 0.2, curve: .smooth), .init(frame: 10000, value: 0.7)],
                    ]
                }
                if name == "room-transition" {
                    effect.parameterAutomation = [
                        "character": [.init(frame: 0, value: 0, curve: .hold), .init(frame: 7000, value: 1)]
                    ]
                }
                if name == "warmth-transition" {
                    effect.parameterAutomation = [
                        "character": [.init(frame: 0, value: 0.25, curve: .hold), .init(frame: 7000, value: 0.75)],
                        "drive": [.init(frame: 0, value: 0.3, curve: .smooth), .init(frame: 13000, value: 0.8)],
                    ]
                }
                if name.hasPrefix("limiter-") && name != "limiter-bypass" {
                    effect.parameterAutomation = [
                        "input": [.init(frame: 0, value: 0.6, curve: .smooth), .init(frame: 13000, value: 0.94)]
                    ]
                }
                if name == "limiter-master" || name == "resonance-track" {
                    effect.bakedRanges = [.init(startFrame: 6000, endFrame: 6300)]
                }
                if name == "resonance-track" {
                    effect.parameterAutomation = [
                        "mix": [.init(frame: 0, value: 0.22, curve: .smooth), .init(frame: 13000, value: 0.91)],
                        "depth": [.init(frame: 0, value: 0.57, curve: .easeIn), .init(frame: 13000, value: 0.81)],
                    ]
                }
                if name == "pitch-edits" {
                    effect.pitchEditsOnly = true
                    effect.pitchEdits = [
                        .init(
                            id: UUID(uuidString: "ABABABAB-ABAB-4BAB-8BAB-ABABABABABAB")!, startFrame: 2600,
                            endFrame: 19000,
                            points: [
                                .init(frame: 2600, note: 60, curve: .smooth, curveStart: 0.2, curveEnd: 0.8),
                                .init(frame: 19000, note: 59.4),
                            ]),
                        .init(
                            id: UUID(uuidString: "ACACACAC-ACAC-4CAC-8CAC-ACACACACACAC")!, startFrame: 7000,
                            endFrame: 8000,
                            points: [.init(frame: 7000, note: 55.4, curve: .hold), .init(frame: 8000, note: 61)]),
                    ]
                }
                if name == "pitch-automation" {
                    effect.parameterAutomation = [
                        "manualTarget": [
                            .init(frame: 0, value: 58.4 / 96), .init(frame: 17000, value: 65.3 / 96, curve: .smooth),
                        ], "mix": [.init(frame: 0, value: 0.3, curve: .smooth), .init(frame: 20000, value: 0.9)],
                        "formant": [.init(frame: 0, value: 0.38, curve: .easeOut), .init(frame: 18000, value: 0.6)],
                    ]
                    effect.bakedRanges = [.init(startFrame: 10000, endFrame: 10700)]
                }
                track.realtimeEffects = [effect]
                if name == "latency-chain" || name == "pitch-chain" {
                    var warmth = RealtimeEffect(kind: .warmth, parameters: ["drive": 8.2, "character": 3, "mix": 0.68])
                    warmth.id = UUID(uuidString: "77777777-7777-4777-8777-777777777777")!
                    var limiter = RealtimeEffect(
                        kind: .limiter, parameters: ["input": 4, "ceiling": -1.7, "release": 96])
                    limiter.id = UUID(uuidString: "88888888-8888-4888-8888-888888888888")!
                    track.realtimeEffects += [warmth, limiter]
                }
            }
            track.volumeAutomationScale = .decibels
            track.mixAutomation = [
                "volume": [
                    AutomationPoint(frame: 0, value: 0.57, curve: .smooth), AutomationPoint(frame: 13000, value: 0.31),
                ],
                "pan": [
                    AutomationPoint(frame: 1000, value: 0.4, curve: .easeOut),
                    AutomationPoint(frame: 9000, value: 0.71),
                ],
            ]
            session.tracks = [track]
            if name == "mixed-latencies" {
                var dry = track
                dry.id = UUID(uuidString: "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA")!
                dry.name = "Zero-delay source"
                dry.realtimeEffects = [RealtimeEffect(kind: .gain, parameters: ["gain": -6])]
                dry.realtimeEffects[0].id = UUID(uuidString: "BBBBBBBB-BBBB-4BBB-8BBB-BBBBBBBBBBBB")!
                dry.gain = 0.43
                dry.pan = -0.62
                session.tracks += [dry]
            }
            session.masterTrack.gain = 0.91
            session.masterTrack.realtimeEffects = [RealtimeEffect(kind: .gain, parameters: ["gain": -0.8])]
            session.masterTrack.realtimeEffects[0].id = UUID(uuidString: "66666666-6666-4666-8666-666666666666")!
            if name.hasSuffix("-master") {
                session.masterTrack.realtimeEffects += session.tracks[0].realtimeEffects
                session.tracks[0].realtimeEffects = []
            }
            if name == "latency-chain" || name == "pitch-chain" {
                var limiter = RealtimeEffect(kind: .limiter, parameters: ["input": 5, "ceiling": -2])
                limiter.id = UUID(uuidString: "99999999-9999-4999-8999-999999999999")!
                session.masterTrack.realtimeEffects += [limiter]
            }
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
            try encoder.encode(session).write(to: folder.appendingPathComponent("session.json"))
            try WaveFile.encode(samples, float: true).write(
                to: folder.appendingPathComponent("Audio/" + assetID.uuidString + ".wav"))
            if name == "pitch-automatic" {
                let contour = try PitchContourAnalysis.analyze(samples, maximumPoints: 96)
                let values = contour.map {
                    ["frame": Double($0.frame), "frequency": Double($0.frequency), "confidence": Double($0.confidence)]
                }
                try JSONSerialization.data(withJSONObject: values, options: [.sortedKeys, .prettyPrinted]).write(
                    to: root.appendingPathComponent("pitch-input-contour-swift.json"))
            }
            let includeTails = name.hasPrefix("tail-")
            let mixer = RealtimeMixer(session: session, assets: [assetID: samples], includeEffectTails: includeTails)
            let end = session.end + (includeTails ? RealtimeMixer.tailFrames(session: session) : 0)
            var left = [Float](repeating: 0, count: end)
            var right = left
            var offset = 0
            while offset < end {
                let count = min(257, end - offset)
                left.withUnsafeMutableBufferPointer { l in
                    right.withUnsafeMutableBufferPointer { r in
                        mixer.render(
                            intoLeft: l.baseAddress! + offset, intoRight: r.baseAddress! + offset, frameCount: count)
                    }
                }
                offset += count
            }
            if name == "pitch-up" || name == "pitch-down" {
                let contour = try PitchContourAnalysis.analyze(
                    AudioSamples(channels: [left, right]), maximumPoints: 96)
                let values = contour.map {
                    ["frame": Double($0.frame), "frequency": Double($0.frequency), "confidence": Double($0.confidence)]
                }
                try JSONSerialization.data(withJSONObject: values, options: [.sortedKeys, .prettyPrinted]).write(
                    to: root.appendingPathComponent(name + "-contour-swift.json"))
            }
            try WaveFile.encode(AudioSamples(channels: [left, right]), float: true).write(
                to: root.appendingPathComponent(name + "-swift.wav"))
        }
    }
}
