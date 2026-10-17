import Foundation

/// Parameters use physical units in projects; automation uses a normalized 0...1 axis.
public struct EffectParameter: Identifiable, Sendable {
    public var id: String
    public var name: String
    public var minimum: Float
    public var maximum: Float
    public var defaultValue: Float
    public var unit: String
    public var logarithmic: Bool
    public var choices: [String]?
    public var integer: Bool
    public init(
        _ id: String, _ name: String, _ minimum: Float, _ maximum: Float, _ defaultValue: Float, _ unit: String = "",
        logarithmic: Bool = false, choices: [String]? = nil, integer: Bool = false
    ) {
        self.id = id
        self.name = name
        self.minimum = minimum
        self.maximum = maximum
        self.defaultValue = defaultValue
        self.unit = unit
        self.logarithmic = logarithmic
        self.choices = choices
        self.integer = integer
    }
    public func clamped(_ value: Float) -> Float {
        let safe = value.isFinite ? max(minimum, min(maximum, value)) : defaultValue
        return choices == nil && !integer ? safe : safe.rounded()
    }
    public func normalized(_ value: Float) -> Float {
        let value = clamped(value)
        return logarithmic ? log(value / minimum) / log(maximum / minimum) : (value - minimum) / (maximum - minimum)
    }
    public func physical(_ normalized: Float) -> Float {
        let n = normalized.isFinite ? max(0, min(1, normalized)) : self.normalized(defaultValue)
        return clamped(logarithmic ? minimum * pow(maximum / minimum, n) : minimum + (maximum - minimum) * n)
    }
    public func display(_ value: Float) -> String {
        let v = clamped(value)
        if let choices, choices.indices.contains(Int(v - minimum)) { return choices[Int(v - minimum)] }
        if integer { return String(format: "%.0f %@", v, unit).trimmingCharacters(in: .whitespaces) }
        if unit == "%" { return String(format: "%.0f%%", v * 100) }
        if unit == "Hz", v >= 1000 { return String(format: "%.2f kHz", v / 1000) }
        if unit == "dB" { return String(format: "%+.1f dB", v) }
        return String(format: v >= 100 ? "%.0f %@" : "%.1f %@", v, unit).trimmingCharacters(in: .whitespaces)
    }
}

public struct EffectPreset: Identifiable, Sendable {
    public var name: String
    public var values: [String: Float]
    public var id: String { name }
}

public extension RealtimeEffectKind {
    var parameters: [EffectParameter] {
        switch self {
        case .guitarRig: return GuitarRigDesign.parameters
        case .gate:
            return [
                .init("threshold", "Threshold", -90, 0, -40, "dB"),
                .init("ratio", "Expansion ratio", 1, 20, 4, ":1"),
                .init("range", "Maximum reduction", 0, 96, 60, "dB"),
                .init("attack", "Attack", 0.1, 100, 1, "ms", logarithmic: true),
                .init("hold", "Hold", 0, 1000, 50, "ms"),
                .init("release", "Release", 5, 3000, 150, "ms", logarithmic: true),
                .init("hysteresis", "Hysteresis", 0, 24, 4, "dB"),
            ]
        case .utility:
            return [
                .init("gain", "Gain", -24, 24, 0, "dB"),
                .init("polarityL", "Left polarity", 0, 1, 0, "", choices: ["Normal", "Invert"]),
                .init("polarityR", "Right polarity", 0, 1, 0, "", choices: ["Normal", "Invert"]),
                .init("swap", "Channels", 0, 1, 0, "", choices: ["Normal", "Swap L/R"]),
                .init("mono", "Mono", 0, 1, 0, "", choices: ["Off", "On"]),
                .init("width", "Stereo width", 0, 2, 1, "%"),
                .init("delayL", "Left delay", 0, 4800, 0, "samples", integer: true),
                .init("delayR", "Right delay", 0, 4800, 0, "samples", integer: true),
            ]
        case .gain: return [.init("gain", "Gain", -24, 24, 0, "dB")]
        case .soundPutty:
            return [
                .init("start", "Start frequency", 45, 800, 95, "Hz", logarithmic: true),
                .init("end", "End frequency", 45, 1600, 560, "Hz", logarithmic: true),
                .init("curve", "Interpolation", 0.2, 4, 1.2),
                .init("wild", "Wildcard lane", 0, 299, 160, integer: true),
                .init("width", "Wildcard width", 1, 90, 22, "lanes"),
                .init("intensity", "Wildcard intensity", 0, 1, 0.7, "%"),
                .init(
                    "wildMode", "Wildcard behavior", 0, 3, 0,
                    choices: ["Detune", "Invert phase", "Absorb energy", "Scatter"]),
                .init("excite", "Excitation", 0, 2, 0, choices: ["Pluck", "Strike", "Blow"]),
                .init("exciteAt", "Excitation lane", 0, 299, 145, integer: true),
                .init("spread", "Excitation spread", 1, 150, 85, "lanes"),
                .init("coupling", "Cross-lane coupling", 0, 0.3, 0.075),
                .init("decay", "Decay time", 0.1, 5, 1.8, "s", logarithmic: true),
                .init("drive", "Input drive", -12, 24, 0, "dB"),
                .init("mix", "Wet mix", 0, 1, 0.4, "%"),
                .init("output", "Wet output", -24, 12, 0, "dB"),
                .init("variationSeed", "Variation seed", 0, 16_777_215, 1234, integer: true),
            ]
        case .vocalEnhancer:
            return [
                .init("cleanup", "Cleanup", 0, 1, 0.45, "%"), .init("body", "Body", -6, 6, 0, "dB"),
                .init("clarity", "Clarity", 0, 1, 0.45, "%"), .init("deEss", "De-ess", 0, 1, 0.5, "%"),
                .init("dynamics", "Level & dynamics", 0, 1, 0.45, "%"), .init("air", "Air", 0, 1, 0.2, "%"),
                .init("warmth", "Warmth", 0, 1, 0.2, "%"), .init("output", "Output", -12, 12, 0, "dB"),
                .init("mix", "Wet mix", 0, 1, 1, "%"),
            ]
        case .sourceIsolator:
            return [
                .init(
                    "mode", "Focus", 0, 4, 0,
                    choices: ["Noise cleanup", "Harmonic", "Percussive", "Stereo center", "Stereo sides"]),
                .init("strength", "Strength", 0, 1, 0.75, "%"),
                .init("noiseFloor", "Noise floor", -90, -20, -55, "dBFS"),
                .init("low", "Lower boundary", 20, 12_000, 20, "Hz", logarithmic: true),
                .init("high", "Upper boundary", 200, 20_000, 20_000, "Hz", logarithmic: true),
                .init("mix", "Wet mix", 0, 1, 1, "%"),
                .init("listen", "Listen", 0, 1, 0, choices: ["Retained", "Removed signal"]),
            ]
        case .lowPass:
            return [
                .init("frequency", "Cutoff", 30, 20_000, 4000, "Hz", logarithmic: true),
                .init("resonance", "Resonance", 0.5, 8, 0.707, "Q", logarithmic: true),
            ]
        case .highPass:
            return [
                .init("frequency", "Cutoff", 20, 12_000, 100, "Hz", logarithmic: true),
                .init("resonance", "Resonance", 0.5, 8, 0.707, "Q", logarithmic: true),
            ]
        case .echo:
            return [
                .init("time", "Delay", 1, 2000, 250, "ms", logarithmic: true),
                .init("feedback", "Feedback", 0, 0.95, 0.4, "%"), .init("mix", "Wet mix", 0, 1, 0.25, "%"),
                .init("damping", "Tone", 200, 20_000, 12_000, "Hz", logarithmic: true),
            ]
        case .warmth:
            return [
                .init("drive", "Drive", 0, 30, 9, "dB"),
                .init("tone", "Tone", 200, 20_000, 12_000, "Hz", logarithmic: true),
                .init("mix", "Wet mix", 0, 1, 1, "%"), .init("output", "Output", -24, 12, 0, "dB"),
                .init("match", "Level match", 0, 1, 0, choices: ["Off", "On"]),
                .init("character", "Character", 0, 4, 0, choices: ["Classic", "Tape", "Tube", "Console", "Diode"]),
                .init("bias", "Asymmetry", 0, 1, 0.25, "%"), .init("emphasis", "Presence", -12, 12, 0, "dB"),
            ]
        case .limiter:
            return [
                .init("input", "Input gain", -12, 24, 0, "dB"), .init("ceiling", "Ceiling", -12, 0, -1, "dBTP"),
                .init("release", "Release", 20, 1000, 150, "ms", logarithmic: true),
                .init("style", "Recovery", 0, 1, 1, choices: ["Fixed", "Adaptive"]),
                .init("truePeak", "Peak protection", 0, 1, 1, choices: ["Sample peak", "True peak · 4×"]),
            ]
        case .resonance:
            return [
                .init("depth", "Maximum reduction", 0, 18, 6, "dB"),
                .init("sensitivity", "Sensitivity", 0, 1, 0.5, "%"), .init("sharpness", "Selectivity", 0, 1, 0.6, "%"),
                .init("low", "Lower boundary", 80, 8000, 500, "Hz", logarithmic: true),
                .init("high", "Upper boundary", 1000, 20000, 16000, "Hz", logarithmic: true),
                .init("attack", "Attack", 1, 100, 12, "ms", logarithmic: true),
                .init("release", "Release", 20, 1000, 160, "ms", logarithmic: true),
                .init("transients", "Preserve attacks", 0, 1, 0.7, "%"), .init("mix", "Wet mix", 0, 1, 1, "%"),
                .init("delta", "Listen", 0, 1, 0, choices: ["Processed", "Removed signal"]),
            ]
        case .compressor:
            return [
                .init("threshold", "Threshold", -60, 0, -18, "dB"), .init("ratio", "Ratio", 1, 20, 4, ":1"),
                .init("attack", "Attack", 0.1, 200, 10, "ms", logarithmic: true),
                .init("release", "Release", 10, 2000, 100, "ms", logarithmic: true),
                .init("knee", "Soft knee", 0, 24, 6, "dB"), .init("makeup", "Makeup", -12, 24, 0, "dB"),
                .init("mix", "Wet mix", 0, 1, 1, "%"),
            ]
        case .room:
            return [
                .init("decay", "Decay", 0.1, 10, 1.4, "s", logarithmic: true),
                .init("predelay", "Pre-delay", 0, 200, 12, "ms"),
                .init("damping", "Tone", 200, 20_000, 6000, "Hz", logarithmic: true),
                .init("mix", "Wet mix", 0, 1, 0.2, "%"),
                .init("character", "Character", 0, 1, 0, choices: ["Classic", "Dense stereo"]),
                .init("lowCut", "Low-cut", 0, 1000, 120, "Hz"), .init("diffusion", "Diffusion", 0, 1, 0.8, "%"),
                .init("width", "Stereo width", 0, 1, 1, "%"), .init("early", "Early reflections", 0, 1, 0.25, "%"),
                .init("output", "Output", -24, 12, 0, "dB"),
            ]
        case .equalizer:
            return [
                .init("lowGain", "Low gain", -18, 18, 0, "dB"),
                .init("midFrequency", "Mid frequency", 150, 10_000, 1000, "Hz", logarithmic: true),
                .init("midGain", "Mid gain", -18, 18, 0, "dB"),
                .init("midQ", "Mid width", 0.3, 8, 1, "Q", logarithmic: true),
                .init("highGain", "High gain", -18, 18, 0, "dB"), .init("output", "Output", -24, 12, 0, "dB"),
                .init("lowFrequency", "Low frequency", 20, 1000, 100, "Hz", logarithmic: true),
                .init("lowQ", "Low width", 0.3, 8, 0.707, "Q", logarithmic: true),
                .init("lowShape", "Low shape", 0, 1, 0, choices: ["Bell", "Shelf"]),
                .init("highFrequency", "High frequency", 1000, 20_000, 8000, "Hz", logarithmic: true),
                .init("highQ", "High width", 0.3, 8, 0.707, "Q", logarithmic: true),
                .init("highShape", "High shape", 0, 1, 0, choices: ["Bell", "Shelf"]),
                .init("highPassFrequency", "High-pass cutoff", 20, 1000, 60, "Hz", logarithmic: true),
                .init("highPassEnabled", "High-pass", 0, 1, 0, choices: ["Off", "On"]),
                .init("lowPassFrequency", "Low-pass cutoff", 1000, 20_000, 16_000, "Hz", logarithmic: true),
                .init("lowPassEnabled", "Low-pass", 0, 1, 0, choices: ["Off", "On"]),
                .init("band4Frequency", "Band 4 frequency", 20, 20000, 400, "Hz", logarithmic: true),
                .init("band4Gain", "Band 4 gain", -18, 18, 0, "dB"),
                .init("band4Q", "Band 4 width", 0.3, 12, 1, "Q", logarithmic: true),
                .init("band4Threshold", "Band 4 threshold", -60, 0, -24, "dB"),
                .init("band4Range", "Band 4 dynamic cut", -18, 0, 0, "dB"),
                .init("band5Frequency", "Band 5 frequency", 20, 20000, 2000, "Hz", logarithmic: true),
                .init("band5Gain", "Band 5 gain", -18, 18, 0, "dB"),
                .init("band5Q", "Band 5 width", 0.3, 12, 1, "Q", logarithmic: true),
                .init("band5Threshold", "Band 5 threshold", -60, 0, -24, "dB"),
                .init("band5Range", "Band 5 dynamic cut", -18, 0, 0, "dB"),
                .init("band6Frequency", "Band 6 frequency", 20, 20000, 6000, "Hz", logarithmic: true),
                .init("band6Gain", "Band 6 gain", -18, 18, 0, "dB"),
                .init("band6Q", "Band 6 width", 0.3, 12, 1, "Q", logarithmic: true),
                .init("band6Threshold", "Band 6 threshold", -60, 0, -24, "dB"),
                .init("band6Range", "Band 6 dynamic cut", -18, 0, 0, "dB"),
                .init("dynamicAttack", "Dynamic attack", 0.1, 200, 10, "ms", logarithmic: true),
                .init("dynamicRelease", "Dynamic release", 10, 2000, 150, "ms", logarithmic: true),
            ]
        case .chorus:
            return [
                .init("rate", "Rate", 0.05, 8, 0.8, "Hz", logarithmic: true), .init("depth", "Depth", 0, 20, 3, "ms"),
                .init("delay", "Base delay", 1, 40, 16, "ms"), .init("width", "Stereo width", 0, 1, 0.7, "%"),
                .init("feedback", "Feedback", 0, 0.85, 0.1, "%"), .init("mix", "Wet mix", 0, 1, 0.35, "%"),
            ]
        case .pitchCorrection:
            return [
                .init(
                    "key", "Key", 0, 11, 0, choices: ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"]),
                .init("scale", "Scale", 0, 3, 0, choices: ["Chromatic", "Major", "Natural minor", "Custom"]),
                .init("mask", "Target notes", 0, 4095, 4095, integer: true),
                .init("reference", "A reference", 420, 460, 440, "Hz"),
                .init("retune", "Retune time", 0, 500, 40, "ms"),
                .init("strength", "Correction", 0, 1, 1, "%"),
                .init("humanize", "Humanize", 0, 1, 0.35, "%"),
                .init("tolerance", "In-tune tolerance", 0, 100, 5, "cents"),
                .init("vibrato", "Preserve vibrato", 0, 1, 0.5, "%"),
                .init("formant", "Formant shift", -12, 12, 0, "st"),
                .init("mix", "Wet mix", 0, 1, 1, "%"),
                .init("manualTarget", "Manual target", -1, 95, -1, "MIDI"),
            ]
        case .dither:
            return [
                .init("bits", "Target depth", 8, 24, 24, "bits", choices: (8...24).map { "\($0)-bit" }),
                .init("amount", "Noise level", 0, 1, 1, "%"),
                .init("quantize", "Processing", 0, 1, 0, choices: ["Dither only", "Dither + quantize"]),
            ]
        }
    }
    var defaultParameters: [String: Float] {
        var result = Dictionary(uniqueKeysWithValues: parameters.map { ($0.id, $0.defaultValue) })
        // Omitted Character in an existing project means Classic. New inserts
        // and factory presets explicitly opt into the denser stereo algorithm.
        if self == .room { result["character"] = 1 }
        return result
    }
    var presets: [EffectPreset] {
        if self == .guitarRig { return GuitarRigDesign.presets }
        var choices: [(String, [String: Float])] = []
        switch self {
        case .guitarRig: break
        case .gain: choices = [("Quiet by 6 dB", ["gain": -6]), ("Boost by 3 dB", ["gain": 3])]
        case .soundPutty:
            choices = [
                ("Deep 300", [:]),
                (
                    "Glass threads",
                    ["start": 220, "end": 1500, "curve": 0.7, "coupling": 0.04, "decay": 2.6, "excite": 1, "mix": 0.45]
                ),
                (
                    "Soft metal",
                    [
                        "start": 70, "end": 420, "coupling": 0.2, "decay": 3, "wildMode": 3, "width": 36, "excite": 1,
                        "mix": 0.3, "drive": 3,
                    ]
                ),
                (
                    "Absorbing cloud",
                    [
                        "wildMode": 2, "intensity": 0.9, "start": 80, "end": 750, "decay": 4.5, "excite": 2,
                        "width": 55, "coupling": 0.1, "mix": 0.55,
                    ]
                ),
                (
                    "Phase fold",
                    [
                        "wildMode": 1, "intensity": 1, "exciteAt": 160, "wild": 160, "width": 45, "decay": 1.2,
                        "mix": 0.45,
                    ]
                ),
            ]
        case .vocalEnhancer:
            choices = [
                ("Natural vocal", [:]),
                (
                    "Intimate & warm",
                    ["body": 1.5, "clarity": 0.25, "deEss": 0.55, "dynamics": 0.35, "air": 0.1, "warmth": 0.35]
                ), ("Lead presence", ["body": -0.5, "clarity": 0.65, "deEss": 0.65, "dynamics": 0.65, "air": 0.4]),
                (
                    "Spoken clarity",
                    ["cleanup": 0.7, "clarity": 0.55, "deEss": 0.65, "dynamics": 0.6, "air": 0.1, "warmth": 0.1]
                ),
                (
                    "Gentle rescue",
                    ["cleanup": 0.8, "clarity": 0.3, "deEss": 0.8, "dynamics": 0.25, "air": 0.05, "warmth": 0.05]
                ),
            ]
        case .sourceIsolator:
            choices = [
                ("Light noise cleanup", ["strength": 0.55, "noiseFloor": -65]),
                ("Strong noise cleanup", ["strength": 0.95, "noiseFloor": -50]),
                ("Sustained notes", ["mode": 1, "strength": 0.9]),
                ("Percussive attacks", ["mode": 2, "strength": 0.9]),
                ("Center focus", ["mode": 3, "strength": 1, "low": 150, "high": 10_000]),
                ("Stereo ambience", ["mode": 4, "strength": 1]),
            ]
        case .lowPass:
            choices = [
                ("Soften", ["frequency": 6500]), ("Telephone top", ["frequency": 3200]),
                ("Resonant sweep", ["frequency": 1800, "resonance": 3]),
            ]
        case .highPass:
            choices = [
                ("Remove rumble", ["frequency": 80]), ("Voice cleanup", ["frequency": 140]),
                ("Telephone bottom", ["frequency": 500]),
            ]
        case .echo:
            choices = [
                ("Slapback", ["time": 95, "feedback": 0.12, "mix": 0.2]),
                ("Wide repeats", ["time": 420, "feedback": 0.55, "mix": 0.35, "damping": 5000]),
            ]
        case .limiter:
            choices = [
                ("Transparent master", [:]), ("Drum peaks", ["release": 65, "style": 0]),
                ("Loud & controlled", ["input": 5, "release": 220]),
                ("Conservative delivery", ["ceiling": -2, "release": 250]),
            ]
        case .resonance:
            choices = [
                ("Vocal smoothness", ["low": 900, "high": 10000, "depth": 5, "transients": 0.6]),
                ("Guitar edge", ["low": 1800, "high": 8500, "depth": 8, "sensitivity": 0.65]),
                ("Cymbal polish", ["low": 4000, "high": 18000, "depth": 4, "transients": 0.9]),
                ("Gentle mix", ["low": 300, "depth": 3, "sensitivity": 0.35, "release": 250]),
            ]
        case .warmth:
            choices = [
                ("Gentle tape", ["drive": 3, "mix": 0.6]),
                ("Heavy drive", ["drive": 24, "tone": 3500, "mix": 0.8, "output": -6]),
                ("Tape cohesion", ["character": 1, "drive": 6, "tone": 14000, "match": 1]),
                ("Tube vocal", ["character": 2, "drive": 8, "bias": 0.45, "mix": 0.7, "match": 1]),
                ("Console punch", ["character": 3, "drive": 5, "emphasis": 2, "match": 1]),
                ("Diode grit", ["character": 4, "drive": 16, "bias": 0.6, "tone": 8000, "mix": 0.45, "match": 1]),
            ]
        case .gate:
            choices = [
                ("Gentle vocal cleanup", ["ratio": 2, "range": 18, "hold": 80, "release": 220]),
                ("Tight drums", ["threshold": -30, "ratio": 20, "hold": 25, "release": 80]),
            ]
        case .utility:
            choices = [
                ("Mono check", ["mono": 1]), ("Invert both channels", ["polarityL": 1, "polarityR": 1]),
                ("Narrow stereo", ["width": 0.5]),
            ]
        case .compressor:
            choices = [
                ("Voice leveling", ["threshold": -24, "ratio": 3, "attack": 5, "release": 160, "makeup": 3]),
                ("Punch", ["threshold": -16, "ratio": 6, "attack": 30, "release": 80]),
                ("Parallel", ["threshold": -32, "ratio": 8, "makeup": 6, "mix": 0.5]),
            ]
        case .room:
            choices = [
                ("Small room", ["decay": 0.5, "predelay": 5, "early": 0.45, "mix": 0.18]),
                (
                    "Vocal chamber",
                    ["decay": 1.7, "predelay": 28, "lowCut": 180, "damping": 6500, "early": 0.3, "mix": 0.22]
                ),
                (
                    "Drum studio",
                    ["decay": 0.7, "predelay": 8, "lowCut": 220, "early": 0.55, "diffusion": 0.6, "mix": 0.18]
                ),
                ("Hall", ["decay": 3.5, "predelay": 35, "early": 0.18, "mix": 0.3]),
                (
                    "Glass cathedral",
                    [
                        "decay": 6.5, "predelay": 45, "damping": 4200, "lowCut": 240, "diffusion": 1, "early": 0.12,
                        "mix": 0.35,
                    ]
                ),
                ("Dark ambience", ["decay": 5, "damping": 1800, "lowCut": 280, "early": 0.12, "mix": 0.4]),
            ]
        case .equalizer:
            choices = [
                (
                    "Voice clarity",
                    [
                        "highPassEnabled": 1, "highPassFrequency": 80, "lowGain": -3, "midFrequency": 2500,
                        "midGain": 3, "highGain": 2,
                    ]
                ),
                (
                    "Warm",
                    ["lowShape": 1, "lowGain": 3, "midFrequency": 3500, "midGain": -2, "highShape": 1, "highGain": -3]
                ),
                (
                    "Vocal presence",
                    [
                        "highPassEnabled": 1, "highPassFrequency": 90, "lowFrequency": 280, "lowGain": -3, "lowQ": 1.2,
                        "midFrequency": 3200, "midGain": 2, "midQ": 0.8, "highShape": 1, "highFrequency": 10_000,
                        "highGain": 2,
                    ]
                ),
                (
                    "Bass definition",
                    [
                        "highPassEnabled": 1, "highPassFrequency": 30, "lowShape": 1, "lowFrequency": 80, "lowGain": 2,
                        "midFrequency": 350, "midGain": -4, "midQ": 1.2, "highShape": 1, "highFrequency": 2500,
                        "highGain": 2,
                    ]
                ),
                (
                    "Dark guitar",
                    [
                        "highPassEnabled": 1, "highPassFrequency": 100, "lowPassEnabled": 1, "lowPassFrequency": 6500,
                        "midFrequency": 2200, "midGain": 2, "midQ": 0.8,
                    ]
                ),
                (
                    "Air & weight",
                    [
                        "lowShape": 1, "lowFrequency": 90, "lowGain": 1.5, "highShape": 1, "highFrequency": 12_000,
                        "highGain": 2, "highPassEnabled": 1, "highPassFrequency": 25,
                    ]
                ),
            ]
        case .chorus:
            choices = [
                ("Subtle double", ["rate": 0.4, "depth": 1.5, "mix": 0.2, "feedback": 0]),
                ("Wide ensemble", ["rate": 1.2, "depth": 6, "width": 1, "mix": 0.5]),
                ("Slow shimmer", ["rate": 0.2, "depth": 10, "delay": 22, "mix": 0.4]),
            ]
        case .pitchCorrection:
            choices = [
                ("Natural voice", ["retune": 60, "humanize": 0.5, "vibrato": 0.8, "tolerance": 10]),
                ("Hard tune", ["retune": 0, "humanize": 0, "vibrato": 0, "tolerance": 0]),
                (
                    "Subtle correction",
                    ["retune": 120, "strength": 0.6, "humanize": 0.7, "vibrato": 1, "tolerance": 15]
                ),
                (
                    "Robot · A only",
                    ["scale": 3, "mask": 512, "retune": 0, "humanize": 0, "vibrato": 0, "tolerance": 0, "formant": -3]
                ),
            ]
        case .dither:
            choices = [
                ("24-bit TPDF", ["bits": 24, "quantize": 0]), ("16-bit TPDF", ["bits": 16, "quantize": 0]),
                ("16-bit reduction", ["bits": 16, "quantize": 1]),
            ]
        }
        return choices.map { name, overrides in
            EffectPreset(name: name, values: defaultParameters.merging(overrides) { _, new in new })
        }
    }
}

public extension BuiltInEffect {
    var realtimeKind: RealtimeEffectKind? {
        switch self {
        case .guitarRig: return .guitarRig
        case .gate: return .gate
        case .utility: return .utility
        case .limiter: return .limiter
        case .resonance: return .resonance
        case .soundPutty: return .soundPutty
        case .vocalEnhancer: return .vocalEnhancer
        case .sourceIsolator: return .sourceIsolator
        case .gain: return .gain
        case .echo: return .echo
        case .room: return .room
        case .warmth: return .warmth
        case .lowPass: return .lowPass
        case .highPass: return .highPass
        case .compressor: return .compressor
        case .equalizer: return .equalizer
        case .chorus: return .chorus
        case .dither: return .dither
        case .pitchCorrection: return .pitchCorrection
        default: return nil
        }
    }
    var parameters: [EffectParameter] { realtimeKind?.parameters ?? [] }
    var defaultParameters: [String: Float] { realtimeKind?.defaultParameters ?? [:] }
    var presets: [EffectPreset] { realtimeKind?.presets ?? [] }
    func process(
        _ audio: AudioSamples, parameters: [String: Float], includeTail: Bool = false,
        progress: (@Sendable (Double) -> Void)? = nil
    ) -> AudioSamples {
        guard let kind = realtimeKind else { return process(audio, amount: 0.5) }
        return RealtimeEffect(kind: kind, parameters: parameters.isEmpty ? kind.defaultParameters : parameters).process(
            audio, includeTail: includeTail, progress: progress)
    }
}

public extension RealtimeEffect {
    /// Meaningful starting values when explicitly upgrading an old one-knob insert.
    var parametersForEditing: [String: Float] {
        guard parameters.isEmpty else { return parameters }
        var result = kind.defaultParameters
        let amount = self.amount.isFinite ? max(0, min(1, self.amount)) : kind.defaultAmount
        switch kind {
        case .gain: result["gain"] = amount * 48 - 24
        case .lowPass: result["frequency"] = 200 * pow(80, 1 - amount)
        case .echo:
            result["time"] = 250
            result["feedback"] = 0.4
            result["mix"] = amount * 0.65
            result["damping"] = 20_000
        case .room: result["character"] = 0
        case .warmth:
            result["drive"] = 20 * log10(1 + amount * 8)
            result["tone"] = 20_000
        default: break
        }
        return result
    }
    mutating func enableParameterControls() {
        guard parameters.isEmpty else { return }
        let original = self
        parameters = parametersForEditing
        let key: String
        switch kind {
        case .gain: key = "gain"
        case .lowPass: key = "frequency"
        case .echo: key = "mix"
        case .warmth: key = "drive"
        default: return
        }
        guard parameterAutomation[key]?.isEmpty != false,
            let descriptor = kind.parameters.first(where: { $0.id == key })
        else { return }
        parameterAutomation[key] = original.automation.map { point in
            var sample = original
            sample.amount = point.value
            sample.automation = []
            return point.relocated(
                frame: point.frame,
                value: descriptor.normalized(sample.parametersForEditing[key] ?? descriptor.defaultValue))
        }
    }
}
