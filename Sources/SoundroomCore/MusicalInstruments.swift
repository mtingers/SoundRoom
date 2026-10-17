import Foundation

public enum MusicalInstrument: String, Codable, CaseIterable, Identifiable, Sendable {
    case synthesizer, studioPiano, tinePiano, nylonStrings, marimba, glassBells, tonewheelOrgan, stringEnsemble,
        electricGuitar, pickedBass, vocalEnsemble
    case uprightPiano, acousticGuitar, uprightBass, vibraphone, woodenFlute, soloCello, concertHarp, kalimba
    public var id: Self { self }
    /// Instruments the Rust host plays from recordings and can generate
    /// from measurements (instrument designer).
    public var isRecorded: Bool {
        [
            .studioPiano, .uprightPiano, .electricGuitar, .acousticGuitar, .pickedBass, .uprightBass, .soloCello,
            .stringEnsemble, .woodenFlute, .concertHarp, .vibraphone, .marimba, .kalimba,
        ].contains(self)
    }
    public var title: String {
        switch self {
        case .uprightPiano: return "Felt upright piano"
        case .acousticGuitar: return "Steel acoustic guitar"
        case .uprightBass: return "Upright bass"
        case .vibraphone: return "Vibraphone"
        case .woodenFlute: return "Wooden flute"
        case .soloCello: return "Solo cello"
        case .concertHarp: return "Concert harp"
        case .kalimba: return "Kalimba"
        case .synthesizer: return "Analog synthesizer"
        case .studioPiano: return "Studio piano"
        case .tinePiano: return "Tine electric piano"
        case .nylonStrings: return "Nylon plucked strings"
        case .marimba: return "Concert marimba"
        case .glassBells: return "Glass bells"
        case .tonewheelOrgan: return "Tonewheel organ"
        case .stringEnsemble: return "String ensemble"
        case .electricGuitar: return "Electric guitar"
        case .pickedBass: return "Picked bass"
        case .vocalEnsemble: return "Vocal ensemble"
        }
    }
    public var characterLabel: String {
        switch self {
        case .studioPiano: return "Hammer hardness"
        case .tinePiano: return "Tine bite"
        case .nylonStrings: return "Pick position"
        case .marimba: return "Mallet hardness"
        case .glassBells: return "Bell color"
        case .tonewheelOrgan: return "Drawbar color"
        case .stringEnsemble: return "Ensemble spread"
        case .electricGuitar: return "Pickup position"
        case .pickedBass: return "Pick hardness"
        case .vocalEnsemble: return "Vowel color"
        case .uprightPiano: return "Hammer hardness"
        case .acousticGuitar: return "Pick position"
        case .uprightBass: return "Pluck position"
        case .vibraphone: return "Motor depth"
        case .woodenFlute: return "Overblow"
        case .soloCello: return "Bow brightness"
        case .concertHarp: return "Pluck position"
        case .kalimba: return "Tine bite"
        case .synthesizer: return "Character"
        }
    }
    public func applying(to pattern: SoundDesignPattern) -> SoundDesignPattern {
        var p = pattern
        p.instrument = self
        p.character = 0.5
        p.resonance = 0.5
        p.tone = 0.86
        p.attack = 0.003
        p.decay = 0.12
        p.sustain = 1
        p.release = 0.25
        p.drive = 1
        switch self {
        case .studioPiano:
            p.release = 0.24
            p.character = 0.45
        case .tinePiano:
            p.release = 0.4
            p.character = 0.6
        case .nylonStrings: p.release = 0.18
        case .marimba: p.release = 0.12
        case .glassBells:
            p.release = 0.9
            p.resonance = 0.75
        case .tonewheelOrgan:
            p.attack = 0.006
            p.release = 0.04
        case .stringEnsemble:
            p.attack = 0.18
            p.release = 0.65
            p.tone = 0.65
        case .electricGuitar:
            p.release = 0.2
            p.character = 0.6
            p.tone = 0.72
        case .pickedBass:
            p.release = 0.13
            p.tone = 0.58
            p.character = 0.4
        case .vocalEnsemble:
            p.attack = 0.12
            p.release = 0.45
            p.tone = 0.82
        case .uprightPiano:
            p.attack = 0.002
            p.release = 0.3
            p.character = 0.45
            p.tone = 0.8
        case .acousticGuitar:
            p.release = 0.3
            p.character = 0.4
            p.tone = 0.82
        case .uprightBass:
            p.release = 0.16
            p.character = 0.45
            p.tone = 0.62
        case .vibraphone:
            p.release = 0.9
            p.resonance = 0.55
        case .woodenFlute:
            p.attack = 0.06
            p.release = 0.14
            p.character = 0.4
            p.resonance = 0.45
            p.tone = 0.78
        case .soloCello:
            p.attack = 0.09
            p.release = 0.3
            p.character = 0.45
            p.tone = 0.72
        case .concertHarp:
            p.release = 0.7
            p.tone = 0.8
        case .kalimba:
            p.release = 0.55
            p.tone = 0.85
        case .synthesizer: break
        }
        return p
    }
}

private struct InstrumentMode {
    var sine: Double = 0, cosine: Double = 1
    var rotationSine: Double, rotationCosine: Double
    let radians: Double
    var amplitude: Double
    let damping: Double
    mutating func next() -> Double {
        let result = sine * amplitude
        let next = sine * rotationCosine + cosine * rotationSine
        cosine = cosine * rotationCosine - sine * rotationSine
        sine = next
        amplitude *= damping
        return result
    }
}

/// Modal strings/bars, velocity-dependent FM tines, harmonic tonewheels and
/// detuned string oscillators. Prepared off the callback; no per-frame allocation.
struct MusicalVoice {
    private var modes: [InstrumentMode] = []
    private let kind: MusicalInstrument, character: Double, resonance: Double, velocity: Double, rate: Double,
        frequency: Double
    private var random: UInt64 = 17
    private var burstGain = 0.0, burstDecay = 1.0, burstFilter = 0.0, burstLP = 0.0, burstHigh = false
    private var breathGain = 0.0, breathLow = 0.0, breathHigh = 0.0, breathLP1 = 0.0, breathLP2 = 0.0
    private var vibratoRate = 0.0, vibratoCents = 0.0, vibratoDelay = 0.0, vibratoRatio = 1.0, tremoloDepth = 0.0
    private var age: Double = 0, carrier = 0.0, modulator = 0.0, lastRatio = 1.0
    init(pattern: SoundDesignPattern, frequency: Double, velocity: Double, rate: Double) {
        kind = pattern.instrument
        self.frequency = frequency
        self.velocity = velocity
        self.rate = rate
        character = pattern.character.isFinite ? min(1, max(0, pattern.character)) : 0.5
        resonance = pattern.resonance.isFinite ? min(1, max(0, pattern.resonance)) : 0.5
        func add(_ ratio: Double, _ gain: Double, _ decay: Double) {
            let hz = frequency * ratio
            guard hz < rate * 0.43 else { return }
            let radians = 2 * Double.pi * hz / rate
            modes.append(
                InstrumentMode(
                    rotationSine: sin(radians), rotationCosine: cos(radians), radians: radians,
                    amplitude: gain, damping: decay > 0 ? exp(-1 / (decay * rate)) : 1))
        }
        random = pattern.organic.seed ^ frequency.bitPattern ^ 0x9E3779B97F4A7C15
        func burst(_ gain: Double, _ tau: Double, _ cutoff: Double, _ high: Bool) {
            burstGain = gain
            burstDecay = exp(-1 / (tau * rate))
            burstFilter = 1 - exp(-2 * .pi * min(cutoff, rate * 0.43) / rate)
            burstHigh = high
        }
        func breath(_ gain: Double, _ low: Double, _ high: Double) {
            breathGain = gain
            breathLow = 1 - exp(-2 * .pi * min(low, rate * 0.43) / rate)
            breathHigh = 1 - exp(-2 * .pi * min(high, rate * 0.43) / rate)
        }
        let c = character
        let r = resonance
        let f = frequency
        let v = velocity
        let PI = Double.pi
        switch kind {
        case .studioPiano:
            let stiffness = 0.00004 + pow(frequency / 1000, 2) * 0.00035
            for n in 1...24 {
                let h = Double(n)
                let ratio = h * sqrt((1 + stiffness * h * h) / (1 + stiffness))
                let brightness = 0.65 + character * 1.6 + velocity * 1.3
                let hammer = exp(-h / (2.5 + brightness * 3))
                add(
                    ratio, 1.25 * hammer / pow(h, 0.65),
                    (0.7 + resonance * 4.5) / pow(h, 0.7) * pow(440 / frequency, 0.22))
            }
        case .nylonStrings:
            let position = 0.12 + character * 0.34
            for n in 1...24 {
                let h = Double(n)
                add(
                    h * sqrt(1 + 0.000015 * h * h), sin(.pi * h * position) / pow(h, 1.12),
                    (0.65 + resonance * 2.7) / pow(h, 0.7))
            }
        case .marimba:
            for (i, ratio) in [1.0, 4, 9.2, 16, 25.3].enumerated() {
                add(
                    ratio, [1.2, 0.35, 0.17, 0.08, 0.035][i] * pow(0.2 + 0.8 * velocity * character, Double(i) * 0.45),
                    (0.22 + resonance * 1.5) / pow(ratio, 0.55))
            }
        case .glassBells:
            for (i, ratio) in [1.0, 2.01, 2.76, 4.07, 5.43, 6.81, 8.93, 11.2].enumerated() {
                add(
                    ratio * (1 + character * Double(i) * 0.0008), 0.9 / pow(Double(i + 1), 1.1),
                    (0.45 + resonance * 3.5) / pow(ratio, 0.35))
            }
        case .tonewheelOrgan:
            for (i, ratio) in [0.5, 1, 1.5, 2, 3, 4, 5, 6, 8].enumerated() {
                let drawbar = [0.3, 1.0, 0.25, 0.4, 0.23, 0.18, 0.12, 0.08, 0.05][i]
                add(ratio, drawbar * (i < 2 ? 1 : 0.2 + character * 1.5) * 0.55, 0)
            }
            add(3, 0.22 * velocity, 0.045 + resonance * 0.15)
        case .stringEnsemble:
            for detune in [-1.0, 0, 1.0] {
                for n in 1...12 {
                    let h = Double(n)
                    add(h * pow(2, detune * (3 + character * 16) / 1200), 0.3 / pow(h, 1.8 - resonance * 0.8), 0)
                }
            }
        case .electricGuitar:
            // Pickup comb and strike harmonics feed the shared tone/drive stages.
            for n in 1...32 {
                let h = Double(n)
                let pickup = sin(.pi * h * (0.06 + character * 0.22))
                add(
                    h * sqrt(1 + 0.000008 * h * h), 1.6 * pickup * exp(-h / (5 + velocity * 12)) / pow(h, 0.9),
                    (0.45 + resonance * 3.2) / pow(h, 0.6))
            }
        case .pickedBass:
            for n in 1...24 {
                let h = Double(n)
                add(
                    h * sqrt(1 + 0.000004 * h * h), 1.15 * exp(-h / (2.5 + character * velocity * 14)) / pow(h, 1.25),
                    (0.3 + resonance * 2.6) / pow(h, 0.65))
            }
        case .vocalEnsemble:
            // Continuous vowel formants on two detuned voiced excitations.
            // This is wordless modeled singing, not a recording or speech model.
            let f1 = character < 0.5 ? 350 + character * 900 : 800 - (character - 0.5) * 980
            let f2 = character < 0.5 ? 750 + character * 800 : 1150 + (character - 0.5) * 2200
            let width = 100 + resonance * 140
            for detune in [-1.0, 1.0] {
                for n in 1...64 {
                    let h = Double(n)
                    let hz = frequency * h
                    func formant(_ center: Double, _ bandwidth: Double) -> Double {
                        exp(-0.5 * pow((hz - center) / bandwidth, 2))
                    }
                    let throat =
                        0.12 + 1.2 * formant(f1, width) + 0.85 * formant(f2, width * 1.5) + 0.35
                        * formant(2700, width * 2)
                    add(h * pow(2, detune * (2 + resonance * 5) / 1200), throat * 0.38 / pow(h, 0.65), 0)
                }
            }
        case .uprightPiano:
            let stiffness = 0.00005 + pow(f / 1000.0, 2.0) * 0.0004
            let strings = f < 110.0 ? 2 : 3
            let bright = 0.35 + 1.3 * c + 1.1 * v
            for n in 1...20 {
                let h = Double(n)
                for d in 0..<strings {
                    let detune = (Double(d) - Double(strings - 1) / 2.0) * (0.6 + 1.6 * r)
                    add(
                        h * sqrt((1.0 + stiffness * h * h) / (1.0 + stiffness)) * pow(2.0, detune / 1200.0),
                        1.15 * exp(-h / (2.0 + 2.6 * bright)) / pow(h, 0.7) / Double(strings),
                        (0.55 + 3.6 * r) / pow(h, 0.75) * pow(440.0 / f, 0.25) * (1.0 + 0.15 * Double(d)))
                }
            }
            burst(0.05 + 0.10 * v, 0.012, 650.0, false)
        case .acousticGuitar:
            let p = 0.08 + 0.22 * c
            for n in 1...32 {
                let h = Double(n)
                add(
                    h * sqrt(1.0 + 0.00002 * h * h), 1.4 * sin(PI * h * p) / pow(h, 0.95) * exp(-h / (8.0 + 16.0 * v)),
                    (0.9 + 3.0 * r) / pow(h, 0.65) * pow(330.0 / f, 0.15))
            }
            add(102.0 / f, 0.16 * v, 0.16)
            add(205.0 / f, 0.10 * v, 0.11)
            add(390.0 / f, 0.07 * v, 0.07)
            add(580.0 / f, 0.04 * v, 0.05)
            burst(0.05 + 0.07 * v, 0.004, 2500.0, true)
        case .uprightBass:
            let p = 0.18 + 0.12 * c
            for n in 1...18 {
                let h = Double(n)
                add(
                    h, 1.25 * sin(PI * h * p) / pow(h, 1.05) * exp(-h / (2.5 + 4.0 * v + 5.0 * c)),
                    (0.5 + 2.2 * r) / pow(h, 0.7))
            }
            add(72.0 / f, 0.2 * v, 0.2)
            add(145.0 / f, 0.09 * v, 0.12)
            burst(0.07 + 0.08 * v, 0.018, 300.0, false)
        case .vibraphone:
            add(1.0, 1.0, 1.6 + 4.0 * r)
            add(3.932, 0.18 + 0.3 * v, (1.6 + 4.0 * r) / pow(3.932, 0.6))
            add(9.538, 0.06 + 0.08 * v, (1.6 + 4.0 * r) / pow(9.538, 0.6))
            add(16.8, 0.02, (1.6 + 4.0 * r) / pow(16.8, 0.6))
            tremoloDepth = 0.08 + 0.42 * c
            burst(0.01 + 0.03 * v, 0.003, 3500.0, false)
        case .woodenFlute:
            for n in 1...12 {
                let h = Double(n)
                let gain = n == 1 ? 1.0 : (0.35 + 0.45 * c) / pow(h, 1.7)
                add(h, gain * (n % 2 == 0 ? 0.8 : 1.0), 0.0)
            }
            breath(0.05 + 0.16 * r, f * 1.2, min(f * 7.0, 9000.0))
            vibratoRate = 5.1
            vibratoCents = 11.0
            vibratoDelay = 0.3
            burst(0.10 + 0.06 * v, 0.025, 2600.0, false)
        case .soloCello:
            for n in 1...40 {
                let h = Double(n)
                let hz = f * h
                let weight =
                    0.18 + exp(-0.5 * pow((hz - 280.0) / 90.0, 2.0)) + 0.8 * exp(-0.5 * pow((hz - 560.0) / 170.0, 2.0))
                    + 0.55 * exp(-0.5 * pow((hz - 1150.0) / 320.0, 2.0)) + 0.3
                    * exp(-0.5 * pow((hz - 2500.0) / 600.0, 2.0))
                add(h, 0.8 * weight / pow(h, 1.05 - 0.45 * c), 0.0)
            }
            breath(0.012 + 0.03 * r, 1400.0, 5200.0)
            vibratoRate = 5.4
            vibratoCents = 14.0
            vibratoDelay = 0.28
            burst(0.05 + 0.05 * v, 0.030, 1800.0, false)
        case .concertHarp:
            let p = 0.32 + 0.12 * c
            for n in 1...20 {
                let h = Double(n)
                add(
                    h, 1.2 * sin(PI * h * p) / pow(h, 1.15) * exp(-h / (6.0 + 9.0 * v)),
                    (1.3 + 4.2 * r) / pow(h, 0.55) * pow(220.0 / f, 0.2))
            }
            burst(0.03 + 0.03 * v, 0.006, 1400.0, false)
        case .kalimba:
            add(1.0, 1.0, 0.8 + 2.6 * r)
            add(5.93, 0.3 * (0.4 + 0.6 * v) * (0.5 + c), (0.8 + 2.6 * r) / pow(5.93, 0.7))
            add(15.3, 0.07 * v, (0.8 + 2.6 * r) / pow(15.3, 0.7))
            add(310.0 / f, 0.06 * v, 0.05)
            burst(0.01 + 0.04 * v, 0.0025, 4200.0, true)
        case .tinePiano, .synthesizer: break
        }
    }
    mutating func next(ratio: Double) -> Double {
        age += 1
        if kind == .tinePiano {
            // Bound the FM excursion and taper brightness in the upper register.
            let t = age / rate
            let index = min(4, max(0, rate * 0.4 / frequency - 1) / 2) * character * velocity * exp(-t / 0.35)
            carrier += 2 * .pi * frequency * ratio / rate
            modulator += 2 * .pi * frequency * 2 * ratio / rate
            carrier.formTruncatingRemainder(dividingBy: 2 * .pi)
            modulator.formTruncatingRemainder(dividingBy: 2 * .pi)
            return
                (sin(carrier + index * sin(modulator))
                + (3 * frequency * ratio < rate * 0.43 ? 0.12 * sin(3 * carrier) * exp(-t / 0.12) : 0))
                * exp(-t / (0.7 + resonance * 4.5))
        }
        if vibratoCents > 0 && Int(age) & 31 == 0 {
            let t = age / rate
            vibratoRatio = pow(
                2, sin(2 * .pi * vibratoRate * t) * vibratoCents * min(1, max(0, (t - vibratoDelay) / 0.3)) / 1200)
        }
        let ratio = ratio * vibratoRatio
        if ratio != lastRatio {
            for i in modes.indices {
                modes[i].rotationSine = sin(modes[i].radians * ratio)
                modes[i].rotationCosine = cos(modes[i].radians * ratio)
            }
            lastRatio = ratio
        }
        var result = 0.0
        for i in modes.indices { result += modes[i].next() }
        if burstGain > 0 || breathGain > 0 {
            random ^= random << 13
            random ^= random >> 7
            random ^= random << 17
            let noise = Double(random & 0xFFFF) / 32767.5 - 1
            burstLP += burstFilter * (noise - burstLP)
            result += (burstHigh ? noise - burstLP : burstLP) * burstGain
            burstGain *= burstDecay
            breathLP1 += breathHigh * (noise - breathLP1)
            breathLP2 += breathLow * (noise - breathLP2)
            result += (breathLP1 - breathLP2) * breathGain
        }
        if tremoloDepth > 0 { result *= 1 - tremoloDepth * (0.5 - 0.5 * cos(2 * .pi * 5.5 * age / rate)) }
        return result
    }
}

extension SoundDesignPattern {
    func renderMusicalInstrument(sampleRate: Double, applyCeiling: Bool) -> AudioSamples {
        let rate = sampleRate.isFinite ? max(8000, min(192000, sampleRate)) : sessionRate
        let tempo = bpm.isFinite ? max(30, min(300, bpm)) : 120
        let length = beats.isFinite ? max(0.25, min(64, beats)) : 4
        let tail = release.isFinite ? max(0, min(2, release)) : 0.25
        let frames = Int(((length * 60 / tempo + tail) * rate).rounded())
        var output = [Float](repeating: 0, count: frames)
        var ends: [Int] = []
        let controlTemplate = SoundDesignControlCursor(pattern: self, rate: rate)
        for note in performedNotes().prefix(256).sorted(by: { $0.startBeat < $1.startBeat }) {
            guard note.startBeat.isFinite, note.durationBeats.isFinite, note.startBeat >= 0, note.startBeat < length,
                note.durationBeats > 0
            else { continue }
            if Task.isCancelled { return AudioSamples(channels: [[]]) }
            let start = Int((note.startBeat * 60 / tempo * rate).rounded())
            ends.removeAll { $0 <= start }
            guard ends.count < 8 else { continue }
            let gate = max(1, Int((min(note.durationBeats, length - note.startBeat) * 60 / tempo * rate).rounded()))
            let end = min(frames, start + gate + Int((tail * rate).rounded()))
            ends.append(end)
            var voice = SoundDesignVoice(
                pitch: note.pitch, velocity: note.velocity, pattern: self, sampleRate: rate, beat: note.startBeat)
            var controls = controlTemplate
            controls.advance(to: start)
            voice.controller(.pitchBend, value: controls.bend / 4 + 0.5, immediate: true)
            voice.controller(.modulation, value: controls.modulation, immediate: true)
            voice.controller(.expression, value: controls.expression, immediate: true)
            for frame in start..<end {
                if frame & 4095 == 0 && Task.isCancelled { return AudioSamples(channels: [[]]) }
                if frame == start + gate { voice.noteOff() }
                controls.advance(to: frame)
                voice.controller(.pitchBend, value: controls.bend / 4 + 0.5)
                voice.controller(.modulation, value: controls.modulation)
                voice.controller(.expression, value: controls.expression)
                output[frame] += voice.renderNext()
            }
        }
        if applyCeiling { for i in output.indices { output[i] = Float(tanh(Double(output[i]))) } }
        return AudioSamples(channels: [output])
    }
    public func renderLoop() -> AudioSamples {
        let audio = render()
        let tempo = bpm.isFinite ? max(30, min(300, bpm)) : 120
        let length = beats.isFinite ? max(0.25, min(64, beats)) : 4
        let frames = max(1, Int((length * 60 / tempo * sessionRate).rounded()))
        if Task.isCancelled { return AudioSamples(channels: [[], []]) }
        let channels = audio.channels.map { source -> [Float] in
            var loop = Array(source.prefix(frames))
            if loop.count < frames { loop.append(contentsOf: repeatElement(0, count: frames - loop.count)) }
            // Copy the body in bulk; only the short release tail needs folding.
            if source.count > frames {
                for f in frames..<source.count {
                    if f & 8191 == 0 && Task.isCancelled { return [] }
                    loop[f % frames] += source[f]
                }
            }
            return loop
        }
        if Task.isCancelled { return AudioSamples(channels: [[], []]) }
        return AudioSamples(channels: channels)
    }
}

/// Instrument-designer shaping of a recorded instrument's measured model.
/// When set on a pattern, the Rust host generates the instrument (instead of
/// playing its recording); defaults leave the measured sound unchanged.
public struct SoundDesignInstrumentShape: Codable, Equatable, Sendable {
    /// −1 dark … 1 bright (±4 dB per harmonic octave).
    public var brightness: Double = 0
    /// Decay and release multiplier (0.2…4).
    public var sustain: Double = 1
    /// Attack noise — hammer, pick, bow, breath — in dB (−36…12).
    public var noise: Double = 0
    /// 0 as recorded … 1 slow, soft swell.
    public var softness: Double = 0
    /// String stiffness (inharmonicity) multiplier (0…4).
    public var stiffness: Double = 1
    /// Extra vibrato depth in cents (0…60) and its rate in Hz (2…9).
    public var vibrato: Double = 0
    public var vibratoRate: Double = 5.5
    /// Low harmonics below 600 Hz, dB (−18…12).
    public var body: Double = 0
    /// Harmonic detune spread in cents (0…25).
    public var detune: Double = 0
    public init() {}
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        func v(_ k: CodingKeys, _ d: Double) throws -> Double { try c.decodeIfPresent(Double.self, forKey: k) ?? d }
        brightness = try v(.brightness, 0)
        sustain = try v(.sustain, 1)
        noise = try v(.noise, 0)
        softness = try v(.softness, 0)
        stiffness = try v(.stiffness, 1)
        vibrato = try v(.vibrato, 0)
        vibratoRate = try v(.vibratoRate, 5.5)
        body = try v(.body, 0)
        detune = try v(.detune, 0)
    }
}
