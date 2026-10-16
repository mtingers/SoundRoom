import Foundation

public enum SoundDesignWaveform: String, CaseIterable, Codable, Sendable {
    case sine, triangle, saw, pulse
    public var title: String { rawValue.capitalized }
}

public struct SoundDesignNote: Identifiable, Codable, Equatable, Sendable {
    public var id: UUID
    public var pitch: Int
    public var velocity: Double
    public var startBeat: Double
    public var durationBeats: Double
    public var articulation: SoundDesignArticulation?
    public init(pitch: Int, velocity: Double = 0.8, startBeat: Double, durationBeats: Double = 0.5) {
        id = UUID(); self.pitch = pitch; self.velocity = velocity
        self.startBeat = startBeat; self.durationBeats = durationBeats
    }
    public var name: String {
        let names = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"]
        let value = max(0, min(127, pitch))
        return "\(names[value % 12])\(value / 12 - 1)"
    }
}

/// An editable, project-owned step pattern. Rendering uses band-limited oscillators;
/// no installed instruments or external plug-ins are involved.
public struct SoundDesignPattern: Codable, Equatable, Sendable {
    public var notes: [SoundDesignNote]
    public var instrument: MusicalInstrument = .synthesizer
    public var character: Double = 0.5
    public var resonance: Double = 0.5
    public var bpm: Double
    public var beats: Double
    public var waveform: SoundDesignWaveform
    public var tone: Double
    public var attack: Double
    public var release: Double
    public var decay: Double
    public var sustain: Double
    public var secondWaveform: SoundDesignWaveform
    public var oscillatorMix: Double
    public var fineTune: Double = 0
    public var detune: Double
    public var pulseWidth: Double
    public var subLevel: Double
    public var noiseLevel: Double
    public var drive: Double
    public var controllers: [SoundDesignController] = []
    public var layers: [SoundDesignLayer] = []
    public var organic = OrganicSound()
    public var baseGain: Double = 1
    public var basePan: Double = 0
    /// Instrument designer: generate the instrument from its measured model.
    public var instrumentShape: SoundDesignInstrumentShape?
    public init(notes: [SoundDesignNote]? = nil, bpm: Double = 120, beats: Double = 4,
                waveform: SoundDesignWaveform = .triangle, tone: Double = 0.65,
                attack: Double = 0.01, release: Double = 0.15, decay: Double = 0.1, sustain: Double = 1,
                secondWaveform: SoundDesignWaveform = .saw, oscillatorMix: Double = 0,
                detune: Double = 0, pulseWidth: Double = 0.5, subLevel: Double = 0,
                noiseLevel: Double = 0, drive: Double = 1) {
        self.notes = notes ?? [60, 64, 67, 64].enumerated().map {
            SoundDesignNote(pitch: $0.element, startBeat: Double($0.offset))
        }
        func safe(_ value: Double, _ range: ClosedRange<Double>, _ fallback: Double) -> Double {
            value.isFinite ? max(range.lowerBound, min(range.upperBound, value)) : fallback
        }
        self.bpm = safe(bpm, 30...300, 120); self.beats = safe(beats, 0.25...64, 4); self.waveform = waveform
        self.tone = safe(tone, 0...1, 0.65); self.attack = safe(attack, 0...2, 0.01); self.release = safe(release, 0...2, 0.15)
        self.decay = safe(decay, 0...2, 0.1); self.sustain = safe(sustain, 0...1, 1)
        self.secondWaveform = secondWaveform; self.oscillatorMix = safe(oscillatorMix, 0...1, 0)
        self.detune = safe(detune, -50...50, 0); self.pulseWidth = safe(pulseWidth, 0.05...0.95, 0.5)
        self.subLevel = safe(subLevel, 0...1, 0); self.noiseLevel = safe(noiseLevel, 0...1, 0)
        self.drive = safe(drive, 1...8, 1)
    }
    private enum CodingKeys: String, CodingKey { case fineTune, instrument, character, resonance, notes, bpm, beats, waveform, tone, attack, release, decay, sustain, secondWaveform, oscillatorMix, detune, pulseWidth, subLevel, noiseLevel, drive, controllers, layers, baseGain, basePan, organic, instrumentShape }
    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        self.init(notes: try values.decodeIfPresent([SoundDesignNote].self, forKey: .notes),
                  bpm: try values.decodeIfPresent(Double.self, forKey: .bpm) ?? 120,
                  beats: try values.decodeIfPresent(Double.self, forKey: .beats) ?? 4,
                  waveform: try values.decodeIfPresent(SoundDesignWaveform.self, forKey: .waveform) ?? .triangle,
                  tone: try values.decodeIfPresent(Double.self, forKey: .tone) ?? 0.65,
                  attack: try values.decodeIfPresent(Double.self, forKey: .attack) ?? 0.01,
                  release: try values.decodeIfPresent(Double.self, forKey: .release) ?? 0.15,
                  decay: try values.decodeIfPresent(Double.self, forKey: .decay) ?? 0.1,
                  sustain: try values.decodeIfPresent(Double.self, forKey: .sustain) ?? 1,
                  secondWaveform: try values.decodeIfPresent(SoundDesignWaveform.self, forKey: .secondWaveform) ?? .saw,
                  oscillatorMix: try values.decodeIfPresent(Double.self, forKey: .oscillatorMix) ?? 0,
                  detune: try values.decodeIfPresent(Double.self, forKey: .detune) ?? 0,
                  pulseWidth: try values.decodeIfPresent(Double.self, forKey: .pulseWidth) ?? 0.5,
                  subLevel: try values.decodeIfPresent(Double.self, forKey: .subLevel) ?? 0,
                  noiseLevel: try values.decodeIfPresent(Double.self, forKey: .noiseLevel) ?? 0,
                  drive: try values.decodeIfPresent(Double.self, forKey: .drive) ?? 1)
        fineTune = min(50, max(-50, try values.decodeIfPresent(Double.self, forKey: .fineTune) ?? 0))
        instrument = try values.decodeIfPresent(MusicalInstrument.self, forKey: .instrument) ?? .synthesizer
        character = try values.decodeIfPresent(Double.self, forKey: .character) ?? 0.5
        resonance = try values.decodeIfPresent(Double.self, forKey: .resonance) ?? 0.5
        controllers = Array((try values.decodeIfPresent([SoundDesignController].self, forKey: .controllers) ?? []).prefix(4096))
        layers = Array((try values.decodeIfPresent([SoundDesignLayer].self, forKey: .layers) ?? []).prefix(6))
        organic = try values.decodeIfPresent(OrganicSound.self, forKey: .organic) ?? OrganicSound()
        baseGain = try values.decodeIfPresent(Double.self, forKey: .baseGain) ?? 1
        basePan = try values.decodeIfPresent(Double.self, forKey: .basePan) ?? 0
        instrumentShape = try values.decodeIfPresent(SoundDesignInstrumentShape.self, forKey: .instrumentShape)
    }

    /// The release tail is retained after the last pattern beat. Eight overlapping
    /// voices and bounded pattern lengths keep malformed documents from overloading DSP.
    public func render(sampleRate: Double = sessionRate) -> AudioSamples { renderInternal(sampleRate: sampleRate, applyCeiling: true) }
    func renderInternal(sampleRate: Double, applyCeiling: Bool) -> AudioSamples {
        if Task.isCancelled { return AudioSamples(channels: [[]]) }
        if !layers.isEmpty || baseGain != 1 || basePan != 0 { return renderLayers(sampleRate: sampleRate) }
        if instrument != .synthesizer { return renderMusicalInstrument(sampleRate: sampleRate, applyCeiling: applyCeiling) }
        func bounded(_ value: Double, _ range: ClosedRange<Double>, _ fallback: Double) -> Double {
            value.isFinite ? max(range.lowerBound, min(range.upperBound, value)) : fallback
        }
        let rate = bounded(sampleRate, 8_000...192_000, sessionRate)
        let tempo = bounded(bpm, 30...300, 120), length = bounded(beats, 0.25...64, 4)
        let attackTime = bounded(attack, 0...2, 0.01), releaseTime = bounded(release, 0...2, 0.15)
        let secondsPerBeat = 60 / tempo
        let frameCount = Int(((length * secondsPerBeat + releaseTime) * rate).rounded())
        var output = Array(repeating: Float(0), count: frameCount)
        var voiceEnds: [Int] = []
                let controlTemplate = SoundDesignControlCursor(pattern: self, rate: rate)
        let ordered = performedNotes().prefix(256).enumerated().filter {
            $0.element.startBeat.isFinite && $0.element.durationBeats.isFinite
        }.sorted { a, b in
            a.element.startBeat == b.element.startBeat ? a.offset < b.offset : a.element.startBeat < b.element.startBeat
        }
        for (_, note) in ordered {
            if Task.isCancelled { return AudioSamples(channels: [[]]) }
            guard note.startBeat >= 0, note.startBeat < length, note.durationBeats > 0 else { continue }
            let velocity = bounded(note.velocity, 0...1, 0)
            guard velocity > 0 else { continue }
            let start = Int((note.startBeat * secondsPerBeat * rate).rounded())
            voiceEnds.removeAll { $0 <= start }
            guard voiceEnds.count < 8 else { continue }
            let gateFrames = max(1, Int((min(note.durationBeats, length - note.startBeat) * secondsPerBeat * rate).rounded()))
            let releaseFrames = Int((releaseTime * rate).rounded())
            let end = min(frameCount, start + gateFrames + releaseFrames)
            voiceEnds.append(end)
            let color = organic.coloring(self, pitch: note.pitch, beat: note.startBeat)
            let cutoff = min(rate * 0.45, 250 * pow(64, bounded(color.tone, 0...1, 0.65)))
            let filter = 1 - exp(-2 * Double.pi * cutoff / rate)
            var motion = OrganicMotion(settings: organic, pitch: note.pitch, beat: note.startBeat, rate: rate)
            let frequency = 440 * pow(2, Double(max(0, min(127, note.pitch)) - 69) / 12) * (fineTune == 0 ? 1 : pow(2, bounded(fineTune, -100...100, 0) / 1200))
            let mix = bounded(oscillatorMix, 0...1, 0)
            let detunedFrequency = frequency * pow(2, bounded(detune, -50...50, 0) / 1200)
            let table = wavetable(frequency: frequency * pow(2, 2.25 / 12), rate: rate, waveform: waveform)
            let secondTable = mix > 0 ? wavetable(frequency: detunedFrequency * pow(2, 2.25 / 12), rate: rate, waveform: secondWaveform) : []
            let step = frequency / rate, secondStep = detunedFrequency / rate
            let sustainLevel = bounded(sustain, 0...1, 1), decayFrames = bounded(decay, 0...2, 0.1) * rate
            let sub = bounded(subLevel, 0...1, 0), noise = bounded(noiseLevel, 0...1, 0)
            let driveGain = bounded(drive, 1...8, 1)
            var phase = 0.0, secondPhase = 0.0, subPhase = 0.0, filtered = 0.0
            // Seed from musical position rather than UUID so save/reopen is sample-exact.
            var random = UInt64(start) &+ UInt64(max(0, min(127, note.pitch))) &+ 0x9E3779B97F4A7C15
            let attackFrames = bounded(color.attack, 0...2, attackTime) * rate
            func envelope(_ local: Int) -> Double {
                let time = Double(local)
                if attackFrames > 0 && time < attackFrames { return time / attackFrames }
                if decayFrames > 0 && time < attackFrames + decayFrames {
                    return 1 - (1 - sustainLevel) * (time - attackFrames) / decayFrames
                }
                return sustainLevel
            }
            let releaseStart = envelope(gateFrames)
            var controls = controlTemplate
            controls.advance(to: start)
            var currentBend = controls.bend, currentModulation = controls.modulation, currentExpression = controls.expression
            let smoothing = 1 - exp(-1 / (0.005 * rate))
            for frame in start..<end {
                if frame & 4095 == 0 && Task.isCancelled { return AudioSamples(channels: [[]]) }
                func lookup(_ table: [Double], _ phase: Double) -> Double {
                    let position = phase * Double(table.count), index = Int(position)
                    let fraction = position - Double(index)
                    return table[index] * (1 - fraction) + table[(index + 1) % table.count] * fraction
                }
                var sample = lookup(table, phase) * (1 - mix)
                if mix > 0 { sample += lookup(secondTable, secondPhase) * mix }
                if sub > 0 { sample += sin(2 * Double.pi * subPhase) * sub * 0.5 }
                if noise > 0 {
                    random ^= random << 13; random ^= random >> 7; random ^= random << 17
                    sample += (Double(random & 0xFFFF) / 32767.5 - 1) * noise * 0.35
                }
                filtered += filter * (sample - filtered)
                let local = frame - start
                let gain = local < gateFrames ? envelope(local) : releaseFrames > 0 ? releaseStart * max(0, 1 - Double(local - gateFrames) / Double(releaseFrames)) : 0
                let driven = driveGain == 1 ? filtered : tanh(filtered * driveGain) / tanh(driveGain)
                controls.advance(to: frame)
                currentBend += (controls.bend - currentBend) * smoothing
                currentModulation += (controls.modulation - currentModulation) * smoothing
                currentExpression += (controls.expression - currentExpression) * smoothing
                let bendRatio = currentBend == 0 && currentModulation == 0 ? 1 : pow(2, (currentBend + sin(2 * .pi * Double(frame - start) / rate * 5) * currentModulation * 0.25) / 12)
                output[frame] += Float(driven * gain * velocity * 0.35 * currentExpression * motion.gain)
                phase += step * bendRatio * motion.ratio; phase -= floor(phase)
                secondPhase += secondStep * bendRatio * motion.ratio; secondPhase -= floor(secondPhase)
                subPhase += step * 0.5 * bendRatio * motion.ratio; motion.advance(); subPhase -= floor(subPhase)
            }
        }
        // A fixed soft ceiling preserves velocity differences without peak normalization.
        if applyCeiling { for index in output.indices { output[index] = Float(tanh(Double(output[index]))) } }
        return AudioSamples(channels: [output])
    }

    func wavetable(frequency: Double, rate: Double, waveform: SoundDesignWaveform) -> [Double] {
        let size = 2048
        let harmonics = min(64, max(0, Int((rate * 0.45 / frequency).rounded(.down))))
        guard harmonics > 0 else { return Array(repeating: 0, count: size) }
        let key = "\(waveform.rawValue):\(harmonics):\(waveform == .pulse ? pulseWidth.bitPattern : 0)"
        if let cached = DesignerWavetableCache.get(key) { return cached }
        var table = Array(repeating: 0.0, count: size)
        for harmonic in 1...harmonics {
            if Task.isCancelled { return Array(repeating: 0, count: size) }
            let coefficient: Double
            var cosine = 0.0
            switch waveform {
            case .sine: coefficient = harmonic == 1 ? 1 : 0
            case .triangle:
                coefficient = harmonic % 2 == 1 ? (harmonic % 4 == 1 ? 1 : -1) * 8 / (Double.pi * Double.pi * Double(harmonic * harmonic)) : 0
            case .saw: coefficient = -2 / (Double.pi * Double(harmonic))
            case .pulse: // Fourier pulse with adjustable duty cycle and zero DC.
                let width = pulseWidth.isFinite ? max(0.05, min(0.95, pulseWidth)) : 0.5
                coefficient = 2 * (1 - cos(2 * Double.pi * Double(harmonic) * width)) / (Double.pi * Double(harmonic))
                cosine = 2 * sin(2 * Double.pi * Double(harmonic) * width) / (Double.pi * Double(harmonic))
            }
            guard coefficient != 0 || cosine != 0 else { continue }
            for index in table.indices {
                let angle = 2 * Double.pi * Double(harmonic * index) / Double(size)
                table[index] += coefficient * sin(angle) + cosine * cos(angle)
            }
        }
        if !Task.isCancelled { DesignerWavetableCache.put(key, table) }
        return table
    }
}

public enum SoundDesignPreset: String, CaseIterable, Identifiable, Codable, Sendable {
    case warmKeys = "Warm keys", glassPluck = "Glass pluck", roundBass = "Round bass", widePad = "Wide pad", brightLead = "Bright lead", softDrum = "Soft drum"
    case studioPiano = "Studio piano", tinePiano = "Tine electric piano", nylonStrings = "Nylon plucked strings", marimba = "Concert marimba", glassBells = "Glass bells", tonewheelOrgan = "Tonewheel organ", stringEnsemble = "String ensemble"
    case electricGuitar = "Electric guitar", pickedBass = "Picked bass", vocalEnsemble = "Vocal ensemble"
    case feltUpright = "Felt upright piano", acousticGuitar = "Steel acoustic guitar", uprightBass = "Upright bass", vibraphone = "Vibraphone", woodenFlute = "Wooden flute", soloCello = "Solo cello", concertHarp = "Concert harp", kalimba = "Kalimba"
    public var id: String { rawValue }
    public func applying(to pattern: SoundDesignPattern) -> SoundDesignPattern {
        var result = SoundDesignPattern(notes: pattern.notes, bpm: pattern.bpm, beats: pattern.beats)
        result.controllers = pattern.controllers
        switch self {
        case .studioPiano: result = MusicalInstrument.studioPiano.applying(to: result)
        case .tinePiano: result = MusicalInstrument.tinePiano.applying(to: result)
        case .nylonStrings: result = MusicalInstrument.nylonStrings.applying(to: result)
        case .marimba: result = MusicalInstrument.marimba.applying(to: result)
        case .glassBells: result = MusicalInstrument.glassBells.applying(to: result)
        case .tonewheelOrgan: result = MusicalInstrument.tonewheelOrgan.applying(to: result)
        case .stringEnsemble: result = MusicalInstrument.stringEnsemble.applying(to: result)
        case .electricGuitar: result = MusicalInstrument.electricGuitar.applying(to: result)
        case .pickedBass: result = MusicalInstrument.pickedBass.applying(to: result)
        case .vocalEnsemble: result = MusicalInstrument.vocalEnsemble.applying(to: result)
        case .feltUpright: result = MusicalInstrument.uprightPiano.applying(to: result)
        case .acousticGuitar: result = MusicalInstrument.acousticGuitar.applying(to: result)
        case .uprightBass: result = MusicalInstrument.uprightBass.applying(to: result)
        case .vibraphone: result = MusicalInstrument.vibraphone.applying(to: result)
        case .woodenFlute: result = MusicalInstrument.woodenFlute.applying(to: result)
        case .soloCello: result = MusicalInstrument.soloCello.applying(to: result)
        case .concertHarp: result = MusicalInstrument.concertHarp.applying(to: result)
        case .kalimba: result = MusicalInstrument.kalimba.applying(to: result)
        case .warmKeys: result.waveform = .triangle; result.tone = 0.6; result.decay = 0.3; result.sustain = 0.55; result.release = 0.25
        case .glassPluck: result.waveform = .sine; result.secondWaveform = .triangle; result.oscillatorMix = 0.3; result.detune = 12; result.tone = 0.9; result.attack = 0.002; result.decay = 0.35; result.sustain = 0.05; result.release = 0.15
        case .roundBass: result.waveform = .pulse; result.pulseWidth = 0.35; result.tone = 0.35; result.subLevel = 0.8; result.decay = 0.2; result.sustain = 0.65; result.release = 0.08; result.drive = 1.4
        case .widePad: result.waveform = .saw; result.secondWaveform = .saw; result.oscillatorMix = 0.5; result.detune = 18; result.tone = 0.5; result.attack = 0.5; result.decay = 0.5; result.sustain = 0.8; result.release = 1.2
        case .brightLead: result.waveform = .saw; result.secondWaveform = .pulse; result.oscillatorMix = 0.35; result.detune = -7; result.tone = 0.9; result.attack = 0.005; result.decay = 0.1; result.sustain = 0.8; result.release = 0.12; result.drive = 2
        case .softDrum: result.waveform = .sine; result.noiseLevel = 0.8; result.subLevel = 0.5; result.tone = 0.5; result.attack = 0; result.decay = 0.12; result.sustain = 0; result.release = 0.05
        }
        result.fineTune = pattern.fineTune; result.organic = pattern.organic; result.controllers = pattern.controllers; result.layers = pattern.layers; result.baseGain = pattern.baseGain; result.basePan = pattern.basePan
        return result
    }
}

/// Bounded musical editing used by both pointer and keyboard controls.
public enum SoundDesignEditing {
    public static func moved(_ notes: [SoundDesignNote], ids: Set<UUID>, by beats: Double, semitones: Int,
                             patternBeats: Double, grid: Double) -> [SoundDesignNote] {
        guard grid.isFinite, grid > 0, patternBeats.isFinite, patternBeats > 0 else { return notes }
        let selected = notes.filter { ids.contains($0.id) }
        guard selected.allSatisfy({ $0.startBeat.isFinite && $0.durationBeats.isFinite && $0.startBeat >= 0 && $0.durationBeats > 0 && $0.startBeat + $0.durationBeats <= patternBeats && (0...127).contains($0.pitch) }) else { return notes }
        guard let first = selected.map(\.startBeat).min(), let last = selected.map({ $0.startBeat + $0.durationBeats }).max(),
              let low = selected.map(\.pitch).min(), let high = selected.map(\.pitch).max() else { return notes }
        let desired = beats.isFinite ? (beats / grid).rounded() * grid : 0
        let shift = max(-first, min(patternBeats - last, desired))
        let pitchShift = max(-low, min(127 - high, semitones))
        return notes.map { source in
            guard ids.contains(source.id) else { return source }
            var note = source; note.startBeat += shift; note.pitch += pitchShift; return note
        }
    }
    public static func resized(_ note: SoundDesignNote, to beats: Double, patternBeats: Double, grid: Double) -> SoundDesignNote {
        guard beats.isFinite, grid.isFinite, grid > 0, patternBeats.isFinite, note.startBeat.isFinite, note.startBeat >= 0, note.startBeat < patternBeats else { return note }
        var result = note
        result.durationBeats = max(min(grid, patternBeats - note.startBeat), min(patternBeats - note.startBeat, (beats / grid).rounded() * grid))
        return result
    }
    public static func quantized(_ notes: [SoundDesignNote], grid: Double, patternBeats: Double) -> [SoundDesignNote] {
        guard grid.isFinite, grid > 0, patternBeats.isFinite, patternBeats > 0 else { return notes }
        let minimum = min(grid, patternBeats)
        return notes.map { source in
            var note = source
            note.startBeat = max(0, min(patternBeats - minimum, (note.startBeat / grid).rounded() * grid))
            note.durationBeats = max(minimum, min(patternBeats - note.startBeat, (note.durationBeats / grid).rounded() * grid))
            return note
        }
    }
}

/// Bounded preparation cache. Never accessed by a render callback.
private enum DesignerWavetableCache {
    static let lock = NSLock()
    static var tables: [String: [Double]] = [:]
    static func get(_ key: String) -> [Double]? { lock.lock(); defer { lock.unlock() }; return tables[key] }
    static func put(_ key: String, _ table: [Double]) {
        lock.lock(); defer { lock.unlock() }
        if tables.count >= 256 { tables.removeAll(keepingCapacity: true) }
        tables[key] = table
    }
}
