import Foundation

/// Soundroom's own effect interface. All processors operate on a copy of the selected samples.
public protocol AudioEffect {
    var name: String { get }
    func process(_ audio: AudioSamples, amount: Float) -> AudioSamples
}

public enum BuiltInEffect: String, CaseIterable, Identifiable, AudioEffect, Sendable {
    case normalize, gain, fadeIn, fadeOut, reverse, echo, room, warmth, lowPass, highPass, compressor, equalizer, chorus, dither, pitchCorrection, limiter, resonance, vocalEnhancer, sourceIsolator, soundPutty, gate, utility, guitarRig
    public var id: String { rawValue }
    public var name: String {
        switch self {
        case .normalize: return "Normalize"
        case .limiter: return "Master limiter"
        case .resonance: return "Resonance control"
        case .gate: return "Gate / Expander"
        case .guitarRig: return "Guitar rig"
        case .utility: return "Utility / Alignment"
        case .soundPutty: return "Sound Putty · Deep 300"
        case .vocalEnhancer: return "Vocal enhancer"
        case .sourceIsolator: return "Source isolator"
        case .gain: return "Gain"
        case .fadeIn: return "Fade in"
        case .fadeOut: return "Fade out"
        case .reverse: return "Reverse"
        case .echo: return "Echo"
        case .room: return "Room reverb"
        case .warmth: return "Warmth"
        case .lowPass: return "Low-pass filter"
        case .highPass: return "High-pass filter"
        case .compressor: return "Compressor"
        case .equalizer: return "Channel EQ"
        case .chorus: return "Chorus"
        case .dither: return "Dither"
        case .pitchCorrection: return "Pitch correction"
        }
    }
    public var detail: String {
        switch self {
        case .normalize: return "Bring the loudest sample to −1 dB."
        case .limiter: return "Stereo-linked look-ahead limiting with true-peak protection."
        case .resonance: return "Reduce moving spectral peaks; audition the removed signal."
        case .vocalEnhancer: return "Clean, level and shape a vocal with selective de-essing and protected peaks."
        case .gate: return "Reduce noise between phrases with hold, hysteresis, and downward expansion."
        case .guitarRig: return "Build a pedal, amp and cabinet rig with modulation, delay and room ambience."
        case .utility: return "Align channels, invert polarity, check mono, and adjust stereo width."
        case .soundPutty: return "Sculpt audio through 300 coupled resonators with a movable wildcard."
        case .sourceIsolator: return "Clean noise, focus harmonic or percussive components, or select stereo center and sides."
        case .gain: return "Adjust the selection from −24 to +24 dB."
        case .fadeIn: return "A smooth rise from silence."
        case .fadeOut: return "A smooth fall to silence."
        case .reverse: return "Turn the selected audio around."
        case .echo: return "Shape timed repeats with feedback and tone."
        case .room: return "Add a little space around the sound."
        case .warmth: return "Soft saturation with compensated output."
        case .lowPass: return "Soften bright frequencies."
        case .highPass: return "Remove rumble and low frequencies."
        case .compressor: return "Gently bring loud and quiet closer together."
        case .equalizer: return "Eight-band EQ with shelves, cuts, and three dynamic bells."
        case .chorus: return "Add a gently moving stereo double."
        case .dither: return "Add TPDF dither when reducing PCM bit depth."
        case .pitchCorrection: return "Tune a single voice to a key or custom notes; preserve vibrato and shape formants."
        }
    }
    public var hasAmount: Bool { ![.normalize, .fadeIn, .fadeOut, .reverse].contains(self) }
    public var usesLegacyAmountProcessor: Bool {
        [.gain, .lowPass, .highPass, .echo, .room, .warmth, .compressor].contains(self)
    }
    public func process(_ audio: AudioSamples, amount: Float = 0.5) -> AudioSamples {
        guard audio.count > 0, !Task.isCancelled else { return audio }
        if [.equalizer, .chorus, .dither, .pitchCorrection, .limiter, .resonance, .vocalEnhancer, .sourceIsolator, .soundPutty, .gate, .utility, .guitarRig].contains(self) { return process(audio, parameters: defaultParameters) }
        let a = max(0, min(1, amount))
        var peak: Float = 0
        if self == .normalize {
            for start in stride(from: 0, to: audio.count, by: 8192) {
                guard !Task.isCancelled else { return audio }
                for channel in audio.readFrames(start..<min(audio.count, start + 8192)) {
                    for value in channel { peak = max(peak, abs(value)) }
                }
            }
        }
        let normalizedGain: Float = peak > 0 ? pow(10, -1 / 20) / peak : 1
        return AudioSamples(channels: audio.channels.map { input in
            switch self {
            case .reverse: return Array(input.reversed())
            case .normalize: return input.map { $0 * normalizedGain }
            case .gain: let gain = pow(Float(10), (a * 48 - 24) / 20); return input.map { $0 * gain }
            case .fadeIn, .fadeOut:
                return input.enumerated().map { i, value in
                    let progress = Float(i) / Float(max(1, input.count - 1))
                    return value * (self == .fadeIn ? progress : 1 - progress)
                }
            case .echo, .room:
                let delays = self == .echo ? [12_000] : [1423, 2137, 3163, 4789]
                var result = input
                for delay in delays {
                    var line = Array(repeating: Float(0), count: delay)
                    for i in input.indices {
                        if i % 8192 == 0, Task.isCancelled { return input }
                        let delayed = line[i % delay]
                        line[i % delay] = input[i] + delayed * (self == .echo ? 0.4 : 0.65)
                        result[i] += delayed * a * (self == .echo ? 0.65 : 0.16)
                    }
                }
                return result
            case .warmth:
                let drive: Float = 1 + a * 8
                return input.map { tanh($0 * drive) / tanh(drive) }
            case .lowPass, .highPass:
                let hz: Float = self == .lowPass ? 200 * pow(80, 1 - a) : 30 * pow(100, a)
                let coefficient = 1 - exp(-2 * Float.pi * hz / Float(sessionRate))
                var state: Float = 0
                return input.map { value in state += coefficient * (value - state); return self == .lowPass ? state : value - state }
            case .equalizer, .chorus, .dither, .pitchCorrection, .limiter, .resonance, .vocalEnhancer, .sourceIsolator, .soundPutty, .gate, .utility, .guitarRig:
                return input
            case .compressor:
                let threshold: Float = pow(10, (-6 - a * 24) / 20)
                let attack: Float = 1 - exp(-1 / (0.005 * Float(sessionRate)))
                let release: Float = 1 - exp(-1 / (0.1 * Float(sessionRate)))
                var envelope: Float = 0
                return input.map { value in
                    let level = abs(value)
                    envelope += (level > envelope ? attack : release) * (level - envelope)
                    let gain: Float = envelope > threshold ? pow(threshold / envelope, 0.75) : 1
                    return value * gain
                }
            }
        })
    }
}
