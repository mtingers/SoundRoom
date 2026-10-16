import Foundation

public struct DrumStep: Codable, Equatable, Sendable {
    public var articulation: DrumArticulation?
    public var voice: DrumVoice
    public var index: Int
    public var velocity: Double
    public var pan: Double
    public var probability: Double
    public init(voice: DrumVoice, index: Int, velocity: Double = 0.8, pan: Double = 0, probability: Double = 1) {
        self.voice = voice; self.index = index; self.velocity = velocity; self.pan = pan; self.probability = probability
    }
}
/// Kit-designer shaping of one piece of the modeled kit, applied on top of
/// the measured recordings by the Rust host. Defaults leave it unchanged.
public struct DrumShape: Codable, Equatable, Sendable {
    /// Head tension, semitones (−12…12): moves resonances, keeps timing.
    public var tension: Double = 0
    /// Decay multiplier (0.25…3).
    public var sustain: Double = 1
    /// Resonant body level, dB (−24…12).
    public var body: Double = 0
    /// Stick or beater contact click, dB (−24…12).
    public var snap: Double = 0
    /// Snare wires, cymbal wash and head rasp, dB (−60 = off … 12).
    public var wires: Double = 0
    /// Tilt, −1 dark … 1 bright (±4 dB per octave about 1 kHz).
    public var tone: Double = 0
    /// Pitch-drop multiplier after the strike (0…3).
    public var pitchDrop: Double = 1
    /// Room level, dB (−36…9).
    public var room: Double = 0
    /// Room decay and pre-delay multiplier (0.4…2.5).
    public var roomSize: Double = 1
    /// Round-robin variation multiplier (0…2.5).
    public var variation: Double = 1
    public init() {}
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        func value(_ key: CodingKeys, _ fallback: Double) throws -> Double { try c.decodeIfPresent(Double.self, forKey: key) ?? fallback }
        tension = try value(.tension, 0); sustain = try value(.sustain, 1); body = try value(.body, 0); snap = try value(.snap, 0)
        wires = try value(.wires, 0); tone = try value(.tone, 0); pitchDrop = try value(.pitchDrop, 1); room = try value(.room, 0)
        roomSize = try value(.roomSize, 1); variation = try value(.variation, 1)
    }
}
/// A saved kit-designer kit: a name and a shape per piece (keyed by
/// `DrumVoice.rawValue`). The server exports it once into samples.
public struct DrumKitDesign: Codable, Equatable, Sendable, Identifiable {
    public var id: UUID
    public var name: String
    public var shapes: [String: DrumShape]
    public init(id: UUID = UUID(), name: String, shapes: [String: DrumShape] = [:]) { self.id = id; self.name = name; self.shapes = shapes }
}
public struct DrumLane: Codable, Equatable, Sendable {
    public var kit: DrumKit
    public var damping: Double?
    /// Kit-designer shaping (modeled kit only).
    public var shape: DrumShape?
    /// Saved kit-designer kit this piece plays (`DrumKitDesign.id`).
    public var customKit: UUID?
    public var gain: Double = 1
    public var pan: Double = 0
    public var tuning: Double = 0
    public var muted = false
    public init(kit: DrumKit) { self.kit = kit }
}
public enum DrumPatternPreset: String, CaseIterable, Identifiable {
    case empty = "Empty", backbeat = "Backbeat", house = "Four on the floor", breakbeat = "Breakbeat", halftime = "Half-time"
    case funk = "Funk · ghost notes", shuffle = "Blues shuffle", jazzRide = "Jazz ride · swing", bossaNova = "Bossa nova"
    case rock = "Rock · driving 8ths", ballad = "Pop ballad", oneDrop = "Reggae · one drop", boomBap = "Boom bap"
    case trap = "Trap · rolling hats", techno = "Techno · driving", deepHouse = "Deep house", drumAndBass = "Drum & bass"
    case ukGarage = "UK garage · 2-step", lofiHipHop = "Lo-fi hip-hop", synthwave = "Synthwave"
    public var id: Self { self }
    public enum Category: String, CaseIterable, Identifiable { case basics = "Basics", acoustic = "Acoustic grooves", electronic = "Electronic grooves"; public var id: Self { self } }
    public var category: Category {
        switch self {
        case .empty, .backbeat, .house, .breakbeat, .halftime: return .basics
        case .funk, .shuffle, .jazzRide, .bossaNova, .rock, .ballad, .oneDrop, .boomBap: return .acoustic
        default: return .electronic
        }
    }
}
public struct DrumPattern: Codable, Equatable, Sendable {
    public var velocityHumanize: Double?
    public var variationCycles: Int?
    public var name = "Drum loop"
    public var kit = DrumKit.acoustic
    public var bpm: Double = 110
    public var bars = 1
    public var stepsPerBeat = 4
    public var swing: Double = 0
    public var humanize: Double = 0
    public var masterGain: Double = 0.8
    public var steps: [DrumStep] = []
    public var lanes: [String: DrumLane] = [:]
    public init() {}
    public var usesExtendedModels: Bool {
        (kit != .acoustic && kit != .electronic) || variationCycles != nil || velocityHumanize != nil ||
        steps.contains { $0.articulation != nil } || lanes.values.contains { $0.damping != nil || $0.shape != nil || $0.customKit != nil || ($0.kit != .acoustic && $0.kit != .electronic) }
    }
    public var stepCount: Int { max(1, min(8, bars)) * 4 * (stepsPerBeat == 2 ? 2 : stepsPerBeat == 8 ? 8 : 4) }
    public var duration: Double { Double(max(1, min(8, bars)) * 4) * 60 / safeTempo }
    public var renderedLoopFrames: Int { max(1, Int((duration * sessionRate).rounded())) * max(1, min(8, variationCycles ?? 1)) }
    /// Extending a groove repeats its existing bars; the new hits are independent.
    public mutating func resizeBars(_ count: Int) {
        let oldCount = stepCount
        let source = steps.filter { $0.index >= 0 && $0.index < oldCount }
        bars = max(1, min(8, count))
        steps = source.filter { $0.index < stepCount }
        if stepCount > oldCount {
            for base in stride(from: oldCount, to: stepCount, by: oldCount) {
                steps += source.compactMap { hit in
                    var hit = hit; hit.index += base
                    return hit.index < stepCount ? hit : nil
                }
            }
        }
    }
    public var safeTempo: Double { bpm.isFinite ? min(300, max(30, bpm)) : 110 }
    public func timeForStep(_ index: Int) -> Double {
        let divisions = stepsPerBeat == 2 ? 2 : stepsPerBeat == 8 ? 8 : 4
        let stepDuration = 60 / safeTempo / Double(divisions)
        return (Double(index) + (index % 2 == 1 ? safe(swing, range: 0...0.7) : 0)) * stepDuration
    }
    public mutating func apply(_ preset: DrumPatternPreset) {
        steps = []; bars = 1; stepsPerBeat = 4
        func add(_ voice: DrumVoice, _ indices: [Int], velocity: Double = 0.85) {
            for index in indices { steps.append(DrumStep(voice: voice, index: index, velocity: velocity)) }
        }
        switch preset {
        case .empty: break
        case .backbeat: add(.kick, [0, 8, 10]); add(.snare, [4, 12]); add(.closedHat, Array(stride(from: 0, to: 16, by: 2)), velocity: 0.62)
        case .house: add(.kick, [0, 4, 8, 12]); add(.clap, [4, 12]); add(.openHat, [2, 6, 10, 14], velocity: 0.56)
        case .breakbeat: add(.kick, [0, 6, 10]); add(.snare, [4, 12]); add(.snare, [7, 15], velocity: 0.3); add(.closedHat, Array(0..<16), velocity: 0.48)
        case .halftime: add(.kick, [0, 7, 10]); add(.snare, [8]); add(.closedHat, [0, 2, 4, 6, 10, 12, 14], velocity: 0.58); add(.openHat, [15], velocity: 0.45)
        default: applyGroove(preset)
        }
        name = preset == .empty ? "Drum loop" : preset.rawValue
    }
    /// Style grooves set kit, tempo and feel along with the parts: dynamics,
    /// ghost notes, articulations and pass-to-pass variation make them breathe.
    private mutating func applyGroove(_ preset: DrumPatternPreset) {
        lanes = [:]; velocityHumanize = 0.04; variationCycles = 2; humanize = 0.003; swing = 0
        func hit(_ voice: DrumVoice, _ indices: [Int], _ velocity: Double, _ articulation: DrumArticulation? = nil, probability: Double = 1) {
            for index in indices {
                var step = DrumStep(voice: voice, index: index, velocity: velocity, probability: probability); step.articulation = articulation
                steps.append(step)
            }
        }
        /// Accented eighths with lighter offbeats across `count` steps.
        func eighths(_ voice: DrumVoice, _ count: Int, strong: Double, weak: Double, every: Int = 2) {
            for index in stride(from: 0, to: count, by: every) { hit(voice, [index], index % (every * 2) == 0 ? strong : weak) }
        }
        switch preset {
        case .funk:
            kit = .funkTight; bpm = 98; swing = 0.08; velocityHumanize = 0.05
            hit(.kick, [0], 0.95); hit(.kick, [7, 10], 0.78); hit(.kick, [14], 0.55, probability: 0.6)
            hit(.snare, [4, 12], 0.95)
            hit(.snare, [6, 9, 15], 0.22); hit(.snare, [11], 0.28, probability: 0.7)
            for i in 0..<16 where i != 14 { hit(.closedHat, [i], i % 4 == 0 ? 0.78 : i % 2 == 0 ? 0.62 : 0.36) }
            hit(.openHat, [14], 0.55)
        case .shuffle:
            kit = .vintageWarm; bpm = 92; stepsPerBeat = 2; swing = 0.333
            for i in 0..<8 { hit(.ride, [i], i % 2 == 0 ? 0.74 : 0.5) }
            hit(.kick, [0, 4], 0.85); hit(.kick, [3, 7], 0.45, probability: 0.7)
            hit(.snare, [2, 6], 0.9); hit(.snare, [5], 0.24, probability: 0.5)
            hit(.closedHat, [2, 6], 0.45)
        case .jazzRide:
            kit = .jazzMaple; bpm = 140; stepsPerBeat = 2; swing = 0.333; variationCycles = 4; velocityHumanize = 0.06; humanize = 0.005
            hit(.ride, [0, 4], 0.8); hit(.ride, [2, 6], 0.82); hit(.ride, [3, 7], 0.52)
            hit(.closedHat, [2, 6], 0.55)
            hit(.kick, [0, 2, 4, 6], 0.22)
            hit(.snare, [5], 0.34, probability: 0.55); hit(.snare, [3], 0.26, probability: 0.4); hit(.snare, [7], 0.3, probability: 0.35)
        case .bossaNova:
            kit = .jazzMaple; bpm = 128; bars = 2; velocityHumanize = 0.05
            hit(.snare, [0, 3, 6, 10, 13, 18, 22, 26, 29], 0.62, .sideStick)
            for bar in [0, 16] {
                hit(.kick, [bar, bar + 8], 0.72); hit(.kick, [bar + 3, bar + 11], 0.48); hit(.kick, [bar + 4, bar + 12], 0.66)
            }
            for i in stride(from: 0, to: 32, by: 2) { hit(.closedHat, [i], i % 4 == 0 ? 0.42 : 0.3) }
        case .rock:
            kit = .arenaRock; bpm = 120; bars = 2
            for bar in [0, 16] {
                hit(.kick, [bar, bar + 8], 0.95); hit(.kick, [bar + 10], 0.8)
                hit(.snare, [bar + 4], 0.95, .rimshot)
            }
            hit(.snare, [12], 0.95, .rimshot)
            hit(.kick, [7], 0.55, probability: 0.5)
            for i in stride(from: 0, to: 28, by: 2) where i != 0 { hit(.closedHat, [i], i % 4 == 0 ? 0.78 : 0.6) }
            hit(.crash, [0], 0.88)
            // A short fill turns the second bar around.
            hit(.highTom, [26, 27], 0.78); hit(.lowTom, [28, 29], 0.82); hit(.snare, [30], 0.85); hit(.snare, [31], 0.6)
        case .ballad:
            kit = .vintageWarm; bpm = 72; velocityHumanize = 0.06; humanize = 0.004
            hit(.kick, [0], 0.8); hit(.kick, [7], 0.42); hit(.kick, [10], 0.7)
            hit(.snare, [4, 12], 0.78)
            for i in stride(from: 0, to: 16, by: 2) { hit(.closedHat, [i], i % 4 == 0 ? 0.48 : 0.34) }
            hit(.closedHat, [15], 0.22, probability: 0.5)
        case .oneDrop:
            kit = .vintageDry; bpm = 76; swing = 0.12
            hit(.kick, [8], 0.9); hit(.snare, [8], 0.85, .sideStick)
            hit(.snare, [15], 0.3, .sideStick, probability: 0.5)
            for i in stride(from: 0, to: 16, by: 2) where i != 14 { hit(.closedHat, [i], i % 4 == 2 ? 0.55 : 0.4) }
            hit(.openHat, [14], 0.45); hit(.closedHat, [3, 11], 0.24, probability: 0.6)
        case .boomBap:
            kit = .vintageDry; bpm = 90; swing = 0.18; humanize = 0.006; velocityHumanize = 0.06
            hit(.kick, [0], 0.95); hit(.kick, [7], 0.65); hit(.kick, [10], 0.85); hit(.kick, [13], 0.45, probability: 0.4)
            hit(.snare, [4, 12], 1)
            for i in stride(from: 0, to: 16, by: 2) where i != 6 { hit(.closedHat, [i], i % 4 == 0 ? 0.55 : 0.42) }
            hit(.openHat, [6], 0.4); hit(.closedHat, [15], 0.28)
        case .trap:
            kit = .trapModern; bpm = 140; stepsPerBeat = 8
            hit(.kick, [0], 0.95); hit(.kick, [11, 22], 0.85); hit(.kick, [27], 0.6, probability: 0.5)
            hit(.clap, [16], 0.95); hit(.snare, [16], 0.55)
            for i in stride(from: 0, to: 32, by: 4) where i < 12 || (i > 15 && i < 28) { hit(.closedHat, [i], i % 8 == 0 ? 0.62 : 0.5) }
            // Rolls: sixteenth and thirty-second bursts with rising velocity.
            hit(.closedHat, [12, 14], 0.42); hit(.closedHat, [13, 15], 0.32)
            hit(.closedHat, [28, 29, 30, 31], 0.36); hit(.closedHat, [30, 31], 0.48, probability: 0.6)
        case .techno:
            kit = .technoMinimal; bpm = 130; velocityHumanize = 0.03
            hit(.kick, [0, 4, 8, 12], 0.95)
            hit(.openHat, [2, 6, 10, 14], 0.52)
            for i in 0..<16 where i % 4 != 2 { hit(.closedHat, [i], i % 2 == 0 ? 0.34 : 0.26) }
            hit(.clap, [4, 12], 0.62)
            hit(.lowTom, [3, 11], 0.3, probability: 0.5); hit(.ride, [8], 0.3, probability: 0.5)
        case .deepHouse:
            kit = .machine909; bpm = 122; swing = 0.1
            hit(.kick, [0, 4, 8, 12], 0.92); hit(.clap, [4, 12], 0.72)
            hit(.openHat, [2, 6, 10, 14], 0.55)
            for i in 0..<16 where i % 4 != 2 { hit(.closedHat, [i], i % 2 == 0 ? 0.38 : 0.28) }
            hit(.highTom, [3, 14], 0.32); hit(.lowTom, [11], 0.35, probability: 0.7)
        case .drumAndBass:
            kit = .machine909; bpm = 172
            hit(.kick, [0], 0.95); hit(.kick, [10], 0.88)
            hit(.snare, [4, 12], 1); hit(.snare, [7], 0.3); hit(.snare, [15], 0.26, probability: 0.6)
            for i in stride(from: 0, to: 16, by: 2) { hit(.closedHat, [i], i % 4 == 2 ? 0.62 : 0.45) }
            hit(.ride, [0, 8], 0.35)
        case .ukGarage:
            kit = .machine909; bpm = 132; swing = 0.22
            hit(.kick, [0], 0.92); hit(.kick, [9], 0.6); hit(.kick, [10], 0.8)
            hit(.clap, [4, 12], 0.85); hit(.snare, [7], 0.4, .sideStick)
            hit(.openHat, [2, 6, 10, 14], 0.5)
            hit(.closedHat, [3, 7, 11, 15], 0.3, probability: 0.7)
        case .lofiHipHop:
            kit = .lofiDusty; bpm = 82; swing = 0.25; humanize = 0.008; velocityHumanize = 0.08; variationCycles = 4
            hit(.kick, [0], 0.9); hit(.kick, [7], 0.6); hit(.kick, [10], 0.8); hit(.kick, [13], 0.45, probability: 0.5)
            hit(.snare, [4, 12], 0.85)
            for i in stride(from: 0, to: 16, by: 2) where i != 6 { hit(.closedHat, [i], i % 4 == 0 ? 0.42 : 0.3) }
            hit(.openHat, [6], 0.32, probability: 0.7)
        case .synthwave:
            kit = .machine808; bpm = 100; bars = 2
            for bar in [0, 16] {
                hit(.kick, [bar, bar + 8], 0.92); hit(.kick, [bar + 6], 0.55)
                hit(.snare, [bar + 4, bar + 12], 0.9); hit(.clap, [bar + 4, bar + 12], 0.55)
            }
            eighths(.closedHat, 32, strong: 0.55, weak: 0.42)
            hit(.crash, [0], 0.7)
        case .empty, .backbeat, .house, .breakbeat, .halftime: break
        }
    }
    /// Each render is deterministic. Export is a cyclic buffer: long cymbal tails
    /// wrap to the beginning and hat choke envelopes stop open hats naturally.
    /// Earlier repeats warm the steady-state loop; no peak normalization is applied.
    public func render(repeats: Int = 1, includeTail: Bool = false) -> AudioSamples {
        let frames = max(1, Int((duration * sessionRate).rounded()))
        let variations = max(1, min(8, variationCycles ?? 1))
        let repeatCount = max(variations, max(1, min(16, repeats)))
        let length = frames * repeatCount + (includeTail ? Int(3.2 * sessionRate) : 0)
        var output = Array(repeating: [Float](repeating: 0, count: length), count: 2)
        var cache: [String: [Float]] = [:]
        let valid = steps.prefix(2304).filter { $0.index >= 0 && $0.index < stepCount }.sorted { $0.index < $1.index }
        let strikeCounts = Dictionary(grouping: valid, by: \.voice).mapValues(\.count)
        let gain = safe(masterGain, range: 0...1)
        let warmupCycles = Int(ceil(6.4 / duration)) + 1
        for cycle in -warmupCycles..<repeatCount {
            if Task.isCancelled { return AudioSamples(channels: [[], []]) }
            var voiceStrikes: [DrumVoice: Int] = [:]
            for step in valid {
                let lane = lanes[step.voice.rawValue] ?? DrumLane(kit: kit)
                guard !lane.muted else { continue }
                let strike = voiceStrikes[step.voice, default: 0]; voiceStrikes[step.voice] = strike + 1
                let pass = (cycle % variations + variations) % variations
                let hash = UInt64(pass * 7919 + step.index * 977 + (DrumVoice.allCases.firstIndex(of: step.voice) ?? 0) * 131 + strike * 31 + 17)
                let chance = Double((hash &* 1664525 &+ 1013904223) % 10000) / 10000
                guard chance < safe(step.probability, range: 0...1) else { continue }
                let jitter = (Double((hash &* 1103515245 &+ 12345) % 10000) / 5000 - 1) * safe(humanize, range: 0...0.025)
                let onset = Int(((max(0, min(duration - 1 / sessionRate, timeForStep(step.index) + jitter))) * sessionRate).rounded()) + cycle * frames
                let velocityOffset = (Double((hash &* 214013 &+ 2531011) % 10000) / 5000 - 1) * safe(velocityHumanize ?? 0, range: 0...0.2)
                let velocity = safe(step.velocity + velocityOffset, range: 0...1)
                // An odd stride visits every robin, even with eight hi-hat hits
                // per bar. A stride of eight would repeat the same strike each pass.
                let robinStride = max(1, strikeCounts[step.voice] ?? 1) | 1
                let rr = (strike + pass * robinStride) % 8
                let key = "\(lane.kit.rawValue)-\(step.voice.rawValue)-\(velocity)-\(rr)-\(lane.tuning)-\(step.articulation?.rawValue ?? "center")-\(lane.damping ?? 0.5)"
                let sample: [Float]
                if let existing = cache[key] { sample = existing }
                else {
                    let raw = DrumSynthesis.render(kit: lane.kit, voice: step.voice, velocity: velocity, roundRobin: rr, articulation: step.articulation ?? .center, damping: lane.damping ?? 0.5).channels[0]
                    let ratio = pow(2, safe(lane.tuning, range: -12...12) / 12)
                    if ratio == 1 { sample = raw }
                    else {
                        sample = (0..<Int(Double(raw.count) / ratio)).map { index in
                            let position = Double(index) * ratio, a = min(raw.count - 1, Int(position)), b = min(raw.count - 1, a + 1)
                            let fraction = Float(position - Double(a))
                            return raw[a] * (1 - fraction) + raw[b] * fraction
                        }
                    }
                    cache[key] = sample
                }
                let pan = safe(step.pan + lane.pan, range: -1...1)
                let left = Float(cos((pan + 1) * .pi / 4) * gain * safe(lane.gain, range: 0...1.5)), right = Float(sin((pan + 1) * .pi / 4) * gain * safe(lane.gain, range: 0...1.5))
                var choke = sample.count
                if step.voice == .openHat, let closing = valid.first(where: { $0.voice == .closedHat && $0.index > step.index }) {
                    choke = min(choke, max(0, Int((timeForStep(closing.index) - timeForStep(step.index)) * sessionRate)))
                } else if step.voice == .openHat, let closing = valid.first(where: { $0.voice == .closedHat }) {
                    choke = min(choke, max(0, Int((duration + timeForStep(closing.index) - timeForStep(step.index)) * sessionRate)))
                }
                for i in 0..<min(sample.count, choke + 120) {
                    if i & 8191 == 0 && Task.isCancelled { return AudioSamples(channels: [[], []]) }
                    let frame = onset + i
                    if frame >= 0 && frame < length {
                        let chokeGain = i < choke ? Float(1) : Float(max(0, 1 - Double(i - choke) / 120))
                        output[0][frame] += sample[i] * left * chokeGain
                        output[1][frame] += sample[i] * right * chokeGain
                    }
                }
            }
        }
        return AudioSamples(channels: output)
    }
    public func audition(_ voice: DrumVoice, velocity: Double = 0.8) -> AudioSamples {
        let lane = lanes[voice.rawValue] ?? DrumLane(kit: kit)
        let raw = DrumSynthesis.render(kit: lane.kit, voice: voice, velocity: velocity, damping: lane.damping ?? 0.5).channels[0]
        let ratio = pow(2, safe(lane.tuning, range: -12...12) / 12)
        let left = Float(cos((safe(lane.pan, range: -1...1) + 1) * .pi / 4) * safe(lane.gain, range: 0...1.5) * safe(masterGain, range: 0...1))
        let right = Float(sin((safe(lane.pan, range: -1...1) + 1) * .pi / 4) * safe(lane.gain, range: 0...1.5) * safe(masterGain, range: 0...1))
        let tuned = (0..<Int(Double(raw.count) / ratio)).map { index -> Float in
            let position = Double(index) * ratio, a = min(raw.count - 1, Int(position)), b = min(raw.count - 1, a + 1), fraction = Float(position - Double(a))
            return raw[a] * (1 - fraction) + raw[b] * fraction
        }
        return AudioSamples(channels: [tuned.map { $0 * left }, tuned.map { $0 * right }])
    }
    public var peak: Float { render().channels.flatMap { $0 }.map { abs($0) }.max() ?? 0 }
}
private func safe(_ value: Double, range: ClosedRange<Double>) -> Double {
    value.isFinite ? min(range.upperBound, max(range.lowerBound, value)) : range.lowerBound
}
