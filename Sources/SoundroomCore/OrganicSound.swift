import Foundation

/// Small, repeatable differences between strikes and slow motion within held notes.
/// Zero is an exact bypass. Seed and settings belong to the patch, never the audio clock.
public struct OrganicSound: Codable, Equatable, Sendable {
    public var drift: Double = 0
    public var timbre: Double = 0
    public var breath: Double = 0
    public var seed: UInt64 = 17
    public init(drift: Double = 0, timbre: Double = 0, breath: Double = 0, seed: UInt64 = 17) {
        self.drift = safe(drift); self.timbre = safe(timbre); self.breath = safe(breath); self.seed = seed
    }
    private enum CodingKeys: String, CodingKey { case drift, timbre, breath, seed }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        self.init(drift: try c.decodeIfPresent(Double.self, forKey: .drift) ?? 0,
                  timbre: try c.decodeIfPresent(Double.self, forKey: .timbre) ?? 0,
                  breath: try c.decodeIfPresent(Double.self, forKey: .breath) ?? 0,
                  seed: try c.decodeIfPresent(UInt64.self, forKey: .seed) ?? 17)
    }
    public var isActive: Bool { drift > 0 || timbre > 0 || breath > 0 }
    func variation(pitch: Int, beat: Double) -> Double {
        var state = seed &+ UInt64(max(0, min(127, pitch))) &* 0x9E3779B97F4A7C15
        state ^= (beat.isFinite ? max(0, beat) : 0).bitPattern
        state = (state ^ (state >> 30)) &* 0xBF58476D1CE4E5B9
        state = (state ^ (state >> 27)) &* 0x94D049BB133111EB
        state ^= state >> 31
        return Double(state >> 11) / Double(UInt64(1) << 53) * 2 - 1
    }
    func coloring(_ pattern: SoundDesignPattern, pitch: Int, beat: Double) -> SoundDesignPattern {
        guard isActive else { return pattern }
        var result = pattern
        let variation = variation(pitch: pitch, beat: beat) * safe(timbre)
        result.tone = max(0, min(1, pattern.tone + variation * 0.07))
        result.character = max(0, min(1, pattern.character + variation * 0.09))
        result.attack = max(0, min(2, pattern.attack * (1 + variation * 0.15)))
        return result
    }
}

/// Two incommensurate slow oscillations, evaluated at control rate. Continuous
/// oscillator phase prevents discontinuities; no noise, allocation or random draws in DSP.
struct OrganicMotion {
    private let drift: Double, breath: Double, phase: Double, rate: Double
    private var age = 0
    private(set) var ratio = 1.0, gain = 1.0
    init(settings: OrganicSound, pitch: Int, beat: Double, rate: Double) {
        drift = safe(settings.drift); breath = safe(settings.breath)
        phase = settings.variation(pitch: pitch, beat: beat) * .pi
        self.rate = rate
        update()
    }
    private mutating func update() {
        let seconds = Double(age) / rate
        if drift > 0 {
            let cents = drift * (4 * sin(seconds * 2 * .pi * 0.31 + phase) + 2 * sin(seconds * 2 * .pi * 0.173 - phase))
            ratio = pow(2, cents / 1200)
        }
        if breath > 0 { gain = 1 + breath * 0.06 * sin(seconds * 2 * .pi * 0.43 + phase) }
    }
    mutating func advance() {
        age += 1
        if age & 63 == 0 && (drift > 0 || breath > 0) { update() }
    }
}
private func safe(_ value: Double) -> Double { value.isFinite ? max(0, min(1, value)) : 0 }

/// Curated combinations, rather than a randomizer. All preserve the played sequence.
public enum OrganicPatch: String, CaseIterable, Identifiable, Sendable {
    case feltGlass = "Felt & glass", cathedral = "Cathedral breath", copperGarden = "Copper garden", duskStrings = "Dusk strings"
    case firesideGuitar = "Fireside guitar", midnightUpright = "Midnight upright", chamberCello = "Chamber cello", forestFlute = "Forest flute", lateNightVibes = "Late-night vibes", kalimbaRain = "Kalimba rain"
    public var id: Self { self }
    public var detail: String {
        switch self {
        case .firesideGuitar: return "Steel strings · warm nylon body · soft harp air"
        case .midnightUpright: return "Felt piano · upright bass below E3"
        case .chamberCello: return "Bowed strings · delayed octave cello"
        case .forestFlute: return "Wooden breath · distant flute harmony"
        case .lateNightVibes: return "Motor shimmer · felt piano warmth"
        case .kalimbaRain: return "Wooden tines · delayed stereo echoes"
        case .feltGlass: return "Soft piano body · glass overtones · gentle tine shimmer"
        case .cathedral: return "Wordless voice · bowed strings · dark organ foundation"
        case .copperGarden: return "Nylon attack · wooden resonance · electric harmonics"
        case .duskStrings: return "Slow analog warmth · bowed body · breath above"
        }
    }
    public func applying(to source: SoundDesignPattern) -> SoundDesignPattern {
        let preset: SoundDesignPreset
        switch self { case .feltGlass: preset = .studioPiano; case .cathedral: preset = .vocalEnsemble; case .copperGarden: preset = .nylonStrings; case .duskStrings: preset = .widePad
        case .firesideGuitar: preset = .acousticGuitar
        case .midnightUpright: preset = .feltUpright
        case .chamberCello: preset = .stringEnsemble
        case .forestFlute: preset = .woodenFlute
        case .lateNightVibes: preset = .vibraphone
        case .kalimbaRain: preset = .kalimba
        }
        var p = preset.applying(to: source); p.controllers = source.controllers
        p.baseGain = 0.68; p.organic = OrganicSound(drift: 0.3, timbre: 0.4, breath: 0.25)
        func layer(_ preset: SoundDesignPreset, _ gain: Double, _ pan: Double, _ transpose: Int = 0) -> SoundDesignLayer {
            var l = SoundDesignLayer(preset: preset); l.gain = gain; l.pan = pan; l.transpose = transpose; return l
        }
        switch self {
        case .firesideGuitar:
            p.layers = [layer(.nylonStrings, 0.16, -0.3), layer(.concertHarp, 0.08, 0.35)]
            p.layers[1].delay = 0.012; p.layers[1].fineTune = 3
        case .midnightUpright:
            p.layers = [layer(.uprightBass, 0.38, -0.12)]
            p.layers[0].highestPitch = 51
        case .chamberCello:
            p.layers = [layer(.soloCello, 0.34, -0.25, -12), layer(.soloCello, 0.18, 0.3)]
            p.layers[0].delay = 0.018; p.layers[0].fineTune = -4
            p.layers[1].delay = 0.009; p.layers[1].fineTune = 3
        case .forestFlute:
            p.layers = [layer(.woodenFlute, 0.14, 0.45, 12)]
            p.layers[0].delay = 0.035; p.layers[0].fineTune = -5
        case .lateNightVibes:
            p.layers = [layer(.feltUpright, 0.2, -0.25)]
            p.layers[0].delay = 0.008
        case .kalimbaRain:
            p.layers = [layer(.kalimba, 0.26, -0.5), layer(.kalimba, 0.14, 0.5, 12)]
            p.layers[0].delay = 0.18; p.layers[0].fineTune = -3
            p.layers[1].delay = 0.36; p.layers[1].fineTune = 4
        case .feltGlass:
            p.tone = 0.65; p.character = 0.25
            p.layers = [layer(.glassBells, 0.14, 0.45, 12), layer(.tinePiano, 0.24, -0.4)]
            p.layers[0].minimumVelocity = 0.35; p.layers[0].velocityFade = 0.25
        case .cathedral:
            p.character = 0.18; p.tone = 0.62; p.attack = 0.2; p.release = 0.8
            p.layers = [layer(.stringEnsemble, 0.3, -0.35), layer(.tonewheelOrgan, 0.15, 0.35, -12)]
        case .copperGarden:
            p.tone = 0.72; p.character = 0.35
            p.layers = [layer(.marimba, 0.24, -0.3), layer(.electricGuitar, 0.18, 0.45, 12)]
            p.layers[1].minimumVelocity = 0.3; p.layers[1].velocityFade = 0.25
        case .duskStrings:
            p.layers = [layer(.stringEnsemble, 0.32, -0.5), layer(.vocalEnsemble, 0.22, 0.5, 12)]
            p.organic.drift = 0.55; p.organic.breath = 0.45
        }
        // Separate seeds give layers independent movement while retaining repeatability.
        for i in p.layers.indices { p.layers[i].organicSeed = UInt64(97 + i * 131) }
        return p
    }
}
