import Foundation
import SoundroomCore

struct EffectParameterGroup: Identifiable, Equatable {
    var id: String
    var title: String
    var parameterIDs: [String]
    init(_ id: String, _ title: String, _ parameterIDs: [String]) {
        self.id = id; self.title = title; self.parameterIDs = parameterIDs
    }
}

/// Presentation only: parameter values and the saved signal path remain in Core.
enum EffectControlPresentation {
    static func groups(for kind: RealtimeEffectKind) -> [EffectParameterGroup] {
        switch kind {
        case .guitarRig: return [.init("input", "Input & noise gate", ["input", "gate"]),
            .init("drive", "Drive pedal", ["pedal", "pedalDrive", "pedalTone", "pedalLevel"]),
            .init("amp", "Amplifier", ["amp", "ampDrive", "bass", "mid", "treble", "presence", "sag", "master"]),
            .init("cab", "Cabinet & microphone", ["cabinet", "mic", "lowCut", "highCut"]),
            .init("mod", "Modulation", ["modulation", "modRate", "modDepth", "modMix"]),
            .init("delay", "Delay", ["delayTime", "delayFeedback", "delayTone", "delayMix"]),
            .init("room", "Room ambience", ["reverbDecay", "reverbMix"]),
            .init("output", "Output & parallel mix", ["output", "mix"])]
        case .gate: return [.init("detection", "Detection & expansion", ["threshold", "ratio", "range", "hysteresis"]), .init("timing", "Timing", ["attack", "hold", "release"])]
        case .utility: return [.init("channels", "Channels", ["polarityL", "polarityR", "swap", "mono", "width"]), .init("alignment", "Alignment · 48 samples = 1 ms", ["delayL", "delayR"]), .init("level", "Level", ["gain"])]
        case .gain: return [.init("level", "Level", ["gain"])]
        case .soundPutty: return [.init("field", "Frequency field", ["start", "end", "curve"]),
                                 .init("wildcard", "Wildcard", ["wildMode", "wild", "width", "intensity", "variationSeed"]),
                                 .init("excitation", "Excitation & motion", ["excite", "exciteAt", "spread", "coupling", "decay"]),
                                 .init("output", "Drive & balance", ["drive", "mix", "output"])]
        case .vocalEnhancer: return [.init("restore", "Restore", ["cleanup", "deEss"]), .init("tone", "Voice character", ["body", "clarity", "air", "warmth"]), .init("level", "Level & balance", ["dynamics", "output", "mix"])]
        case .sourceIsolator: return [.init("focus", "Component focus", ["mode", "strength"]), .init("clean", "Noise & region", ["noiseFloor", "low", "high"]), .init("listen", "Listen & balance", ["listen", "mix"])]
        case .lowPass, .highPass: return [.init("filter", "Filter", ["frequency", "resonance"])]
        case .echo: return [.init("repeats", "Repeats", ["time", "feedback"]), .init("tone", "Tone & balance", ["damping", "mix"])]
        case .warmth: return [.init("character", "Character", ["character", "drive", "bias", "emphasis", "tone"]), .init("output", "Output", ["match", "mix", "output"])]
        case .limiter: return [.init("limit", "Limiting", ["input", "ceiling"]), .init("recovery", "Recovery", ["style", "release", "truePeak"])]
        case .resonance: return [.init("detect", "Detection", ["depth", "sensitivity", "sharpness"]), .init("region", "Frequency region", ["low", "high"]), .init("timing", "Timing", ["attack", "release", "transients"]), .init("listen", "Listen & balance", ["delta", "mix"])]
        case .compressor: return [.init("compression", "Compression", ["threshold", "ratio", "knee"]),
                                 .init("timing", "Timing", ["attack", "release"]), .init("output", "Output", ["makeup", "mix"])]
        case .room: return [.init("space", "Space", ["character", "decay", "predelay"]),
                           .init("texture", "Dense stereo · reflections", ["diffusion", "early", "width"]),
                           .init("tone", "Tone", ["lowCut", "damping"]), .init("output", "Output", ["mix", "output"])]
        case .equalizer: return [.init("low", "Low band", ["lowShape", "lowFrequency", "lowGain", "lowQ"]),
                                .init("mid", "Mid bell", ["midFrequency", "midGain", "midQ"]),
                                .init("high", "High band", ["highShape", "highFrequency", "highGain", "highQ"]),
                                                                .init("band4", "Band 4 · dynamic bell", ["band4Frequency", "band4Gain", "band4Q", "band4Threshold", "band4Range"]),
                                .init("band5", "Band 5 · dynamic bell", ["band5Frequency", "band5Gain", "band5Q", "band5Threshold", "band5Range"]),
                                .init("band6", "Band 6 · dynamic bell", ["band6Frequency", "band6Gain", "band6Q", "band6Threshold", "band6Range"]),
                                .init("dynamic", "Dynamic timing", ["dynamicAttack", "dynamicRelease"]),
                                .init("cuts", "Cut filters · 12 dB/oct", ["highPassEnabled", "highPassFrequency", "lowPassEnabled", "lowPassFrequency"]),
                                .init("output", "Output", ["output"])]
        case .chorus: return [.init("movement", "Movement", ["rate", "depth", "delay"]),
                             .init("space", "Stereo & repeats", ["width", "feedback"]), .init("output", "Balance", ["mix"])]
        case .dither: return [.init("resolution", "Resolution", ["bits", "quantize"]), .init("noise", "Dither", ["amount"])]
        case .pitchCorrection: return [.init("targets", "Targets", ["scale", "key", "mask", "manualTarget"]),
                                      .init("timing", "Correction", ["retune", "strength", "humanize", "vibrato"]),
                                      .init("voice", "Voice", ["formant", "mix", "reference", "tolerance"])]
        }
    }
    static func visibleParameterIDs(for kind: RealtimeEffectKind) -> [String] {
        groups(for: kind).flatMap(\.parameterIDs).filter { kind != .pitchCorrection || $0 != "manualTarget" }
    }
    static func matches(kind: RealtimeEffectKind, values: [String: Float], presetValues: [String: Float]) -> Bool {
        if kind == .soundPutty {
            let actual = SoundPuttyShape(parameters: values), expected = SoundPuttyShape(parameters: presetValues)
            for axis in 0..<3 { for lane in 0..<300 { if abs(actual.rows[axis][lane] - expected.rows[axis][lane]) > 0.000001 { return false } } }
        }
        return kind.parameters.allSatisfy { parameter in
            let actual = parameter.clamped(values[parameter.id] ?? parameter.defaultValue)
            let expected = parameter.clamped(presetValues[parameter.id] ?? parameter.defaultValue)
            if parameter.integer || parameter.choices != nil { return actual == expected }
            let tolerance = max(0.000001, max(actual.ulp, expected.ulp) * 8)
            return abs(actual - expected) <= tolerance
        }
    }
    static func summary(kind: RealtimeEffectKind, values: [String: Float]) -> String {
        let keys: [String]
        switch kind {
        case .compressor: keys = ["threshold", "ratio", "attack", "release"]
        case .equalizer: keys = ["lowGain", "midFrequency", "midGain", "highGain"]
        case .pitchCorrection: keys = ["scale", "retune", "strength", "formant"]
        case .warmth: keys = ["character", "drive", "tone", "match"]
        case .chorus: keys = ["rate", "depth", "width", "mix"]
        default: keys = Array(visibleParameterIDs(for: kind).prefix(4))
        }
        return keys.compactMap { key in
            guard let parameter = kind.parameters.first(where: { $0.id == key }) else { return nil }
            return "\(parameter.name) \(parameter.display(values[key] ?? parameter.defaultValue))"
        }.joined(separator: " · ")
    }
    static func fractionDigits(for parameter: EffectParameter) -> Int {
        if parameter.integer || parameter.choices != nil { return 0 }
        if parameter.unit == "Hz", parameter.maximum >= 1000 { return 1 }
        return 2
    }
    static func hint(kind: RealtimeEffectKind, parameter key: String) -> String {
        switch (kind, key) {
        case (.guitarRig, "gate"): return "Reduce noise between phrases. −90 dB bypasses the gate; raise the threshold only enough to silence your input noise."
        case (.guitarRig, "amp"): return "Choose an original clean, chime, crunch or high-gain voice. Direct / bypass keeps the pedalboard and skips the amp tone controls."
        case (.guitarRig, "cabinet"): return "Choose speaker and enclosure coloration. Direct / bypass skips all cabinet and microphone filtering, useful before a real guitar cabinet."
        case (.guitarRig, "mic"): return "0% points toward the bright speaker center; 100% moves toward the darker edge."
        case (.guitarRig, "sag"): return "Adds a softer power-stage response to stronger picking. Zero keeps the response tight."
        case (.guitarRig, "delayTime"): return "Delay in milliseconds. At 120 BPM, 375 ms is a dotted eighth note. Time changes glide and can briefly bend the echoes' pitch."
        case (.guitarRig, "reverbMix"), (.guitarRig, "delayMix"): return "Zero turns this section off while preserving its other settings."
        case (.guitarRig, "mix"): return "Blend the original audio with the complete rig. Dry audio is aligned to the two oversampled gain stages."
        case (.utility, "mono"): return "Average the channels for a mono compatibility check. Switching it off restores the saved stereo width."
        case (.utility, "width"): return "0% is mono, 100% preserves the original stereo image, and 200% doubles the side signal. Mono overrides this control."
        case (.utility, "delayL"), (.utility, "delayR"): return "Intentional output-channel alignment, 48 samples per millisecond. The mixer does not compensate this delay away."
        case (.gate, "ratio"): return "1:1 leaves the level unchanged. Higher ratios attenuate audio farther below the threshold, up to Maximum reduction."
        case (.gate, "hold"): return "Keep the gate open between brief dips to preserve syllables and drum decays."
        case (.gate, "hysteresis"): return "The closing threshold is this many decibels below the opening threshold, reducing chatter."

        case (.soundPutty, "excite"): return "Track audio drives the field. Pluck emphasizes changes, Strike drives directly, and Blow smooths the excitation. Silence never starts a new sound."
        case (.soundPutty, "coupling"): return "Transfers motion between neighboring resonators. Zero leaves the lanes independent, except for wildcard Scatter."
        case (.soundPutty, "decay"): return "Base amplitude decay time. Sculpted damping changes each lane; exported tails are bounded to 30 seconds."
        case (.soundPutty, "output"): return "Gain on the resonant wet signal; dry audio keeps its original level."
        case (.soundPutty, "variationSeed"): return "A repeatable detuning pattern. New variation chooses another seed without changing your sculpt curves."
        case (.vocalEnhancer, "cleanup"): return "Reduces rumble and quiet noise without raising breaths and room tone."
        case (.vocalEnhancer, "deEss"): return "Reduces strong sibilant energy selectively while retaining the body of the voice."
        case (.vocalEnhancer, "dynamics"): return "Levels active vocals and softly compresses peaks; quiet tails are protected from gain boost."
        case (.vocalEnhancer, "warmth"): return "Adds oversampled harmonic color. Use less for transparent restoration."
        case (.sourceIsolator, "mode"): return "Focuses signal components. Harmonic, percussive and stereo focus cannot identify individual instruments in a dense mix. Use Separate source for model-based stems."
        case (.sourceIsolator, "noiseFloor"): return "Set near the background noise level. A higher floor removes more noise but can also soften quiet notes."
        case (.sourceIsolator, "listen"): return "Audition retained audio or the latency-aligned removed signal before committing."
        case (.lowPass, "frequency"): return "Frequencies above the cutoff roll off."
        case (.highPass, "frequency"): return "Frequencies below the cutoff roll off."
        case (.lowPass, "resonance"), (.highPass, "resonance"): return "Higher Q adds a narrower resonance near the cutoff."
        case (.equalizer, "midQ"), (.equalizer, "lowQ"), (.equalizer, "highQ"): return "Higher Q narrows a bell or adds resonance at the shelf transition."
        case (.equalizer, "lowShape"), (.equalizer, "highShape"): return "Bell shapes a region around the frequency. Shelf changes all frequencies beyond it."
        case (.equalizer, "highPassFrequency"): return "Removes rumble below the cutoff at 12 dB per octave when High-pass is On."
        case (.equalizer, "lowPassFrequency"): return "Softens frequencies above the cutoff at 12 dB per octave when Low-pass is On."
        case (.echo, "damping"): return "Low-pass filtering in the feedback loop; the first wet repeat is undamped."
        case (.echo, "feedback"): return "How much of each repeat feeds the next."
        case (.compressor, "threshold"): return "Compression starts when the stereo-linked peak envelope exceeds this level."
        case (.compressor, "knee"): return "Smooths the transition into compression around the threshold."
        case (.compressor, "attack"): return "How quickly the peak envelope rises."
        case (.compressor, "release"): return "How quickly the peak envelope falls."
        case (.compressor, "makeup"): return "Gain on the compressed branch, before the wet mix."
        case (.room, "decay"): return "Nominal low-frequency feedback decay to −60 dB. Damping shortens the high-frequency tail."
        case (.room, "character"): return "Dense stereo adds coupled reflections and diffusion. Classic preserves the original sound of saved sessions. Changes crossfade over 5 ms."
        case (.room, "lowCut"): return "Removes bass from the Dense stereo wet input at 6 dB per octave. Zero disables the cut; dry audio is unchanged."
        case (.room, "diffusion"): return "Spreads attacks through six all-pass input stages in Dense stereo. Higher values produce a denser onset."
        case (.room, "early"): return "Balances the first spatial reflections against the diffuse late tail in Dense stereo. 100% gives early reflections only."
        case (.room, "width"): return "Width of the Dense stereo wet signal. Zero sums it to mono; 100% preserves the decorrelated stereo tail."
        case (.room, "output"): return "Gain after the dry and reverb branches are mixed."
        case (.room, "damping"): return "Low-pass filtering inside the reverb feedback loops."
        case (.chorus, "depth"): return "Delay sweeps from the base delay up to base delay plus depth."
        case (.chorus, "width"): return "Offsets the phase of the right delay head relative to the left."
        case (.limiter, "ceiling"): return "Output ceiling. True-peak protection uses a 4× reconstructed detector with 0.3 dB safety margin; put this insert last on Master."
        case (.limiter, "style"): return "Adaptive recovery releases shallow reductions faster and deep reductions more slowly."
        case (.resonance, "depth"): return "Caps attenuation of individual spectral bins; stereo channels share the same reduction."
        case (.resonance, "sensitivity"): return "Higher sensitivity catches peaks closer to their local spectral neighborhood. It cannot tell musical harmonics from unwanted resonances."
        case (.resonance, "delta"): return "Audition the latency-aligned signal removed by the processor. Return to Processed before applying or exporting."
        case (.resonance, "transients"): return "Relaxes suppression during sudden increases in spectral energy, preserving attacks."
        case (.warmth, "character"): return "Five original oversampled curves. Classic preserves previous sessions; Tape, Tube, Console and Diode offer distinct harmonic character."
        case (.warmth, "bias"): return "Adds asymmetric harmonic color in Tube and Diode. A DC filter keeps the new characters centered."
        case (.warmth, "emphasis"): return "High-frequency emphasis before saturation; zero is neutral. Classic ignores this control."
        case (.warmth, "match"): return "Slowly matches the wet branch's measured level to the dry branch."
        case (.warmth, "output"): return "Gain after the dry and saturated branches are mixed."
        case (.dither, "amount"): return "Scales TPDF noise relative to one quantization step."
        case (.dither, "quantize"): return "Dither only adds noise; Dither + quantize also rounds to the selected bit depth."
        default: return ""
        }
    }
}

/// Static parameter models, never a measurement of an audio signal or automation.
enum EffectSettingMath {
    static func value(_ key: String, kind: RealtimeEffectKind, values: [String: Float]) -> Double {
        guard let parameter = kind.parameters.first(where: { $0.id == key }) else { return 0 }
        return Double(parameter.clamped(values[key] ?? parameter.defaultValue))
    }
    static func gainAmplitude(decibels: Double) -> Double { pow(10, decibels / 20) }
    struct Compressor {
        let threshold: Double, ratio: Double, knee: Double, makeup: Double, mix: Double
        init(values: [String: Float]) {
            threshold = EffectSettingMath.value("threshold", kind: .compressor, values: values)
            ratio = EffectSettingMath.value("ratio", kind: .compressor, values: values)
            knee = EffectSettingMath.value("knee", kind: .compressor, values: values)
            makeup = EffectSettingMath.value("makeup", kind: .compressor, values: values)
            mix = EffectSettingMath.value("mix", kind: .compressor, values: values)
        }
        func outputDB(inputDB: Double) -> Double {
            let over = inputDB - threshold, slope = 1 / ratio - 1
            let reduction: Double
            if knee > 0 && abs(over) < knee / 2 { reduction = slope * pow(over + knee / 2, 2) / (2 * knee) }
            else { reduction = over > 0 ? slope * over : 0 }
            // Both branches have the same instantaneous phase in the compressor.
            let gain = (1 - mix) + mix * pow(10, (reduction + makeup) / 20)
            return inputDB + 20 * log10(max(1e-15, gain))
        }
    }
    static func echoRepeats(values: [String: Float], count: Int = 7) -> [(milliseconds: Double, amplitude: Double)] {
        let time = value("time", kind: .echo, values: values), feedback = value("feedback", kind: .echo, values: values)
        let mix = value("mix", kind: .echo, values: values)
        return (1...max(1, min(12, count))).map { (time * Double($0), mix * pow(feedback, Double($0 - 1))) }
    }
    static func chorusDelay(phase: Double, right: Bool, values: [String: Float]) -> Double {
        let base = value("delay", kind: .chorus, values: values), depth = value("depth", kind: .chorus, values: values)
        let width = value("width", kind: .chorus, values: values)
        return base + depth * (sin(phase + (right ? Double.pi * width : 0)) + 1) * 0.5
    }
    static func quantizationStep(bits: Double) -> Double { pow(2, 1 - max(8, min(24, bits.rounded()))) }
}
