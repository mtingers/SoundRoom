import Foundation

public enum SoundDesignArticulation: String, Codable, CaseIterable, Identifiable, Sendable {
    case normal, staccato, tenuto, accent
    public var id: Self { self }
    public var title: String { rawValue.capitalized }
}
public enum SoundDesignControllerKind: String, Codable, CaseIterable, Identifiable, Sendable {
    case sustain, modulation, expression, pitchBend
    public var id: Self { self }
    public var title: String {
        switch self {
        case .sustain: return "Sustain"
        case .modulation: return "Modulation"
        case .expression: return "Expression"
        case .pitchBend: return "Pitch bend"
        }
    }
    public var defaultValue: Double { self == .expression ? 1 : self == .pitchBend ? 0.5 : 0 }
}
public struct SoundDesignController: Codable, Equatable, Sendable {
    public var kind: SoundDesignControllerKind
    public var beat: Double
    public var value: Double
    public init(kind: SoundDesignControllerKind, beat: Double, value: Double) {
        self.kind = kind
        self.beat = beat
        self.value = value
    }
}
public struct SoundDesignLayer: Codable, Equatable, Identifiable, Sendable {
    public var id = UUID()
    public var preset: SoundDesignPreset = .glassPluck
    public var gain: Double = 0.5
    public var pan: Double = 0
    public var transpose = 0
    public var minimumVelocity: Double = 0
    public var maximumVelocity: Double = 1
    public var enabled = true
    public var toneOverride: Double?
    public var attackOverride: Double?
    public var releaseOverride: Double?
    public var characterOverride: Double?
    public var velocityFade: Double?
    public var fineTune: Double?
    public var delay: Double?
    public var lowestPitch: Int?
    public var highestPitch: Int?
    public var decayOverride: Double?
    public var sustainOverride: Double?
    public var organicSeed: UInt64?
    public init(preset: SoundDesignPreset = .glassPluck) { self.preset = preset }
    private enum CodingKeys: String, CodingKey {
        case id, preset, gain, pan, transpose, minimumVelocity, maximumVelocity, enabled
        case fineTune, delay, lowestPitch, highestPitch, decayOverride, sustainOverride
        case toneOverride, attackOverride, releaseOverride, characterOverride, velocityFade, organicSeed
    }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decodeIfPresent(UUID.self, forKey: .id) ?? UUID()
        preset = try c.decodeIfPresent(SoundDesignPreset.self, forKey: .preset) ?? .glassPluck
        gain = bounded(try c.decodeIfPresent(Double.self, forKey: .gain) ?? 0.5, 0...1)
        pan = bounded(try c.decodeIfPresent(Double.self, forKey: .pan) ?? 0, -1...1)
        transpose = max(-24, min(24, try c.decodeIfPresent(Int.self, forKey: .transpose) ?? 0))
        minimumVelocity = bounded(try c.decodeIfPresent(Double.self, forKey: .minimumVelocity) ?? 0, 0...1)
        maximumVelocity = max(
            minimumVelocity, bounded(try c.decodeIfPresent(Double.self, forKey: .maximumVelocity) ?? 1, 0...1))
        enabled = try c.decodeIfPresent(Bool.self, forKey: .enabled) ?? true
        toneOverride = try c.decodeIfPresent(Double.self, forKey: .toneOverride).map { bounded($0, 0...1) }
        attackOverride = try c.decodeIfPresent(Double.self, forKey: .attackOverride).map { bounded($0, 0...2) }
        releaseOverride = try c.decodeIfPresent(Double.self, forKey: .releaseOverride).map { bounded($0, 0...2) }
        characterOverride = try c.decodeIfPresent(Double.self, forKey: .characterOverride).map { bounded($0, 0...1) }
        velocityFade = try c.decodeIfPresent(Double.self, forKey: .velocityFade).map { bounded($0, 0...0.5) }
        fineTune = try c.decodeIfPresent(Double.self, forKey: .fineTune).map { bounded($0, -50...50) }
        delay = try c.decodeIfPresent(Double.self, forKey: .delay).map { bounded($0, 0...0.5) }
        lowestPitch = try c.decodeIfPresent(Int.self, forKey: .lowestPitch).map { max(0, min(127, $0)) }
        highestPitch = try c.decodeIfPresent(Int.self, forKey: .highestPitch).map {
            max(lowestPitch ?? 0, min(127, $0))
        }
        decayOverride = try c.decodeIfPresent(Double.self, forKey: .decayOverride).map { bounded($0, 0...2) }
        sustainOverride = try c.decodeIfPresent(Double.self, forKey: .sustainOverride).map { bounded($0, 0...1) }
        organicSeed = try c.decodeIfPresent(UInt64.self, forKey: .organicSeed)
    }
    public func instrument(for pattern: SoundDesignPattern) -> SoundDesignPattern {
        var result = preset.applying(
            to: SoundDesignPattern(notes: pattern.notes, bpm: pattern.bpm, beats: pattern.beats))
        result.fineTune = bounded(pattern.fineTune, -50...50) + bounded(fineTune ?? 0, -50...50)
        if let value = decayOverride { result.decay = bounded(value, 0...2) }
        if let value = sustainOverride { result.sustain = bounded(value, 0...1) }
        result.controllers = pattern.controllers
        result.organic = pattern.organic
        result.organic.seed =
            organicSeed
            ?? (pattern.organic.seed &+ UInt64(97 + (SoundDesignPreset.allCases.firstIndex(of: preset) ?? 0) * 131))
        if let value = toneOverride { result.tone = bounded(value, 0...1) }
        if let value = attackOverride { result.attack = bounded(value, 0...2) }
        if let value = releaseOverride { result.release = bounded(value, 0...2) }
        if let value = characterOverride { result.character = bounded(value, 0...1) }
        result.notes = pattern.notes.filter {
            accepts(pitch: $0.pitch) && $0.velocity >= bounded(minimumVelocity, 0...1)
                && $0.velocity <= bounded(maximumVelocity, 0...1)
        }.map {
            var note = $0
            note.velocity *= velocityWeight(note.velocity)
            note.pitch = max(0, min(127, note.pitch + max(-24, min(24, transpose))))
            return note
        }
        return result
    }
    public func accepts(pitch: Int) -> Bool {
        let low = max(0, min(127, lowestPitch ?? 0))
        let high = max(low, min(127, highestPitch ?? 127))
        return pitch >= low && pitch <= high
    }
    public var onsetDelay: Double { bounded(delay ?? 0, 0...0.5) }
    public func velocityWeight(_ velocity: Double) -> Double {
        let low = bounded(minimumVelocity, 0...1)
        let high = bounded(maximumVelocity, 0...1)
        guard velocity.isFinite, velocity >= low, velocity <= high else { return 0 }
        let fade = min((high - low) / 2, bounded(velocityFade ?? 0, 0...0.5))
        guard fade > 0 else { return 1 }
        let inWeight = low > 0 ? sin(min(1, (velocity - low) / fade) * .pi / 2) : 1
        let outWeight = high < 1 ? sin(min(1, (high - velocity) / fade) * .pi / 2) : 1
        return inWeight * outWeight
    }
    public var hasCustomSound: Bool {
        fineTune != nil || delay != nil || lowestPitch != nil || highestPitch != nil || decayOverride != nil
            || sustainOverride != nil || toneOverride != nil || attackOverride != nil || releaseOverride != nil
            || characterOverride != nil || velocityFade != nil || organicSeed != nil
    }
}
extension SoundDesignPattern {
    public var performanceRelease: Double {
        let base = release.isFinite ? max(0, min(2, release)) : 0.15
        return max(
            base,
            layers.prefix(6).filter { $0.enabled }.map { $0.instrument(for: self).release + $0.onsetDelay }.max() ?? 0)
    }
    public func performedNotes() -> [SoundDesignNote] {
        let pedal = controllers.prefix(4096).filter {
            $0.kind == .sustain && $0.beat.isFinite && $0.value.isFinite && $0.beat >= 0 && $0.beat <= beats
        }.sorted { $0.beat < $1.beat }
        return notes.map { original in
            var note = original
            switch note.articulation ?? .normal {
            case .normal: break
            case .staccato: note.durationBeats *= 0.45
            case .tenuto: note.durationBeats *= 0.95
            case .accent: note.velocity = min(1, note.velocity * 1.25)
            }
            let end = note.startBeat + note.durationBeats
            if let last = pedal.last(where: { $0.beat <= end }), last.value >= 0.5 {
                let pedalRelease = pedal.first(where: { $0.beat > end && $0.value < 0.5 })?.beat ?? beats
                note.durationBeats = min(beats, pedalRelease) - note.startBeat
            }
            return note
        }
    }
    func renderLayers(sampleRate: Double) -> AudioSamples {
        var base = self
        base.layers = []
        base.baseGain = 1
        base.basePan = 0
        let rate = sampleRate.isFinite ? max(8000, min(192000, sampleRate)) : sessionRate
        let tempo = bpm.isFinite ? max(30, min(300, bpm)) : 120
        let length = beats.isFinite ? max(0.25, min(64, beats)) : 4
        let count = Int(((length * 60 / tempo + performanceRelease) * rate).rounded())
        var output = Array(repeating: [Float](repeating: 0, count: count), count: 2)
        func add(_ audio: AudioSamples, gain: Double, pan: Double, delay: Double = 0) {
            let angle = (bounded(pan, -1...1) + 1) * .pi / 4
            let level = bounded(gain, 0...1)
            let left = Float(cos(angle) * level)
            let right = Float(sin(angle) * level)
            guard let input = audio.channels.first else { return }
            let offset = Int((delay * rate).rounded())
            for frame in 0..<min(max(0, count - offset), audio.count) {
                if frame & 8191 == 0 && Task.isCancelled { return }
                output[0][frame + offset] += input[frame] * left
                output[1][frame + offset] += input[frame] * right
            }
        }
        // One temporary layer buffer at a time, instead of four complete bounces.
        if baseGain > 0 {
            add(base.renderInternal(sampleRate: rate, applyCeiling: false), gain: baseGain, pan: basePan)
        }
        for layer in layers.prefix(6) where layer.enabled && layer.gain > 0 {
            if Task.isCancelled { return AudioSamples(channels: [[], []]) }
            add(
                layer.instrument(for: self).renderInternal(sampleRate: rate, applyCeiling: false), gain: layer.gain,
                pan: layer.pan, delay: layer.onsetDelay)
        }
        for channel in 0..<2 {
            for frame in output[channel].indices {
                if frame & 8191 == 0 && Task.isCancelled { return AudioSamples(channels: [[], []]) }
                output[channel][frame] = Float(tanh(Double(output[channel][frame])))
            }
        }
        return AudioSamples(channels: output)
    }
}
struct SoundDesignControlCursor {
    let events: [(frame: Int, kind: SoundDesignControllerKind, value: Double)]
    var index = 0
    var bend: Double = 0, modulation: Double = 0, expression: Double = 1
    init(pattern: SoundDesignPattern, rate: Double) {
        let bpm = pattern.bpm.isFinite ? max(30, min(300, pattern.bpm)) : 120
        events = pattern.controllers.prefix(4096).filter {
            $0.beat.isFinite && $0.beat >= 0 && $0.beat <= pattern.beats && $0.value.isFinite
        }.sorted { $0.beat < $1.beat }.map {
            (Int(($0.beat * 60 / bpm * rate).rounded()), $0.kind, bounded($0.value, 0...1))
        }
    }
    mutating func advance(to frame: Int) {
        while index < events.count && events[index].frame <= frame {
            let event = events[index]
            switch event.kind {
            case .pitchBend: bend = (event.value - 0.5) * 4
            case .modulation: modulation = event.value
            case .expression: expression = event.value
            case .sustain: break
            }
            index += 1
        }
    }
}
extension SoundDesignEditing {
    public static func humanized(
        _ notes: [SoundDesignNote], ids: Set<UUID>? = nil, timing: Double = 0.03, velocity: Double = 0.08,
        patternBeats: Double, seed: UInt64 = 17
    ) -> [SoundDesignNote] {
        guard patternBeats.isFinite, patternBeats > 0 else { return notes }
        var state = seed
        func random() -> Double {
            state = state &* 6364136223846793005 &+ 1
            return Double(state >> 11) / Double(UInt64(1) << 53) * 2 - 1
        }
        return notes.map { original in
            guard ids == nil || ids!.contains(original.id), original.startBeat.isFinite, original.durationBeats.isFinite
            else { return original }
            var note = original
            let shift = random() * bounded(timing, 0...0.125)
            note.startBeat = max(0, min(max(0, patternBeats - note.durationBeats), note.startBeat + shift))
            note.velocity = bounded(note.velocity + random() * bounded(velocity, 0...0.3), 0.05...1)
            return note
        }
    }
}
private func bounded(_ value: Double, _ range: ClosedRange<Double>) -> Double {
    value.isFinite ? max(range.lowerBound, min(range.upperBound, value)) : range.lowerBound
}
