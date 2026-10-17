import Foundation

/// A prepared, project-native oscillator voice. Its tables are built on the control
/// thread; renderNext performs only bounded DSP without allocating. Key release uses
/// the current ADSR level, so a short tap cannot jump to full volume on note-off.
public struct SoundDesignVoice {
    /// Remaining frames of a 5 ms fade after the bank steals this voice.
    var stealFade: Int?
    public let pitch: Int
    public let keyPitch: Int
    public let leftGain: Float, rightGain: Float, levelGain: Float
    private let table: [Double], secondTable: [Double]
    private let step: Double, secondStep: Double, mix: Double, sub: Double, noise: Double
    private let attackFrames: Double, decayFrames: Double, sustain: Double, releaseFrames: Double
    private let filter: Double, velocity: Double, drive: Double
    private var phase = 0.0, secondPhase = 0.0, subPhase = 0.0, filtered = 0.0
    private var age = 0.0, releaseAge = 0.0, releaseLevel = 0.0, gated = true
    private var musicalVoice: MusicalVoice?
    private var organicMotion: OrganicMotion
    private var random: UInt64
    private let rate: Double, controlSmoothing: Double
    private var targetBend = 0.0, currentBend = 0.0, targetModulation = 0.0, currentModulation = 0.0,
        targetExpression = 1.0, currentExpression = 1.0
    private var preDelay = 0, pendingNoteOff = false, minimumGate = 0
    public var isFinished: Bool { preDelay == 0 && !gated && releaseAge >= releaseFrames }

    public init(
        pitch: Int, velocity: Double, pattern: SoundDesignPattern, sampleRate: Double = sessionRate,
        keyPitch: Int? = nil, gain: Double = 1, pan: Double = 0, stereo: Bool = false, beat: Double = 0,
        delay: Double = 0
    ) {
        self.pitch = max(0, min(127, pitch))
        self.keyPitch = keyPitch ?? self.pitch
        let level = gain.isFinite ? max(0, min(1, gain)) : 0
        let safePan = pan.isFinite ? max(-1, min(1, pan)) : 0
        levelGain = Float(level)
        leftGain = Float(level * (stereo ? cos((safePan + 1) * .pi / 4) : 1))
        rightGain = Float(level * (stereo ? sin((safePan + 1) * .pi / 4) : 1))
        self.velocity = velocity.isFinite ? max(0, min(1, velocity)) : 0
        rate = sampleRate.isFinite ? max(8_000, min(192_000, sampleRate)) : sessionRate
        preDelay = Int(((delay.isFinite ? min(0.5, max(0, delay)) : 0) * rate).rounded())
        controlSmoothing = 1 - exp(-1 / (0.005 * rate))
        // The public pattern remains editable; sanitize mutable values before DSP.
        let colored = pattern.organic.coloring(pattern, pitch: self.pitch, beat: beat)
        organicMotion = OrganicMotion(settings: pattern.organic, pitch: self.pitch, beat: beat, rate: rate)
        let safe = SoundDesignPattern(
            notes: [], waveform: pattern.waveform, tone: colored.tone, attack: colored.attack,
            release: pattern.release, decay: pattern.decay, sustain: pattern.sustain,
            secondWaveform: pattern.secondWaveform, oscillatorMix: pattern.oscillatorMix,
            detune: pattern.detune, pulseWidth: pattern.pulseWidth, subLevel: pattern.subLevel,
            noiseLevel: pattern.noiseLevel, drive: pattern.drive)
        let tune = pattern.fineTune.isFinite ? min(100, max(-100, pattern.fineTune)) : 0
        let frequency = 440 * pow(2, Double(self.pitch - 69) / 12) * (tune == 0 ? 1 : pow(2, tune / 1200))
        let secondFrequency = frequency * pow(2, safe.detune / 1200)
        table =
            pattern.instrument == .synthesizer
            ? safe.wavetable(frequency: frequency * pow(2, 2.25 / 12), rate: rate, waveform: safe.waveform) : []
        secondTable =
            pattern.instrument == .synthesizer && safe.oscillatorMix > 0
            ? safe.wavetable(frequency: secondFrequency * pow(2, 2.25 / 12), rate: rate, waveform: safe.secondWaveform)
            : []
        musicalVoice =
            pattern.instrument == .synthesizer
            ? nil : MusicalVoice(pattern: colored, frequency: frequency, velocity: self.velocity, rate: rate)
        step = frequency / rate
        secondStep = secondFrequency / rate
        mix = safe.oscillatorMix
        sub = safe.subLevel
        noise = safe.noiseLevel
        drive = safe.drive
        attackFrames = safe.attack * rate
        decayFrames = safe.decay * rate
        sustain = safe.sustain
        releaseFrames = safe.release * rate
        let cutoff = min(rate * 0.45, 250 * pow(64, safe.tone))
        filter = 1 - exp(-2 * Double.pi * cutoff / rate)
        random = UInt64(self.pitch) &+ 0x9E3779B97F4A7C15
    }
    public mutating func controller(_ kind: SoundDesignControllerKind, value: Double, immediate: Bool = false) {
        let value = value.isFinite ? max(0, min(1, value)) : kind.defaultValue
        switch kind {
        case .pitchBend: targetBend = (value - 0.5) * 4
        case .modulation: targetModulation = value
        case .expression: targetExpression = value
        case .sustain: break
        }
        if immediate {
            currentBend = targetBend
            currentModulation = targetModulation
            currentExpression = targetExpression
        }
    }
    public mutating func noteOff() {
        if preDelay > 0 {
            pendingNoteOff = true
            return
        }
        guard gated else { return }
        releaseLevel = envelope()
        gated = false
        releaseAge = 0
    }
    private func envelope() -> Double {
        if attackFrames > 0 && age < attackFrames { return age / attackFrames }
        if decayFrames > 0 && age < attackFrames + decayFrames {
            return 1 - (1 - sustain) * (age - attackFrames) / decayFrames
        }
        return sustain
    }
    public mutating func renderNext() -> Float {
        if preDelay > 0 {
            preDelay -= 1
            if preDelay == 0 && pendingNoteOff { minimumGate = max(1, Int(rate * 0.01)) }
            return 0
        }
        if minimumGate > 0 {
            minimumGate -= 1
            if minimumGate == 0 { noteOff() }
        }
        if isFinished { return 0 }
        func lookup(_ table: [Double], _ phase: Double) -> Double {
            let position = phase * Double(table.count)
            let index = Int(position)
            let fraction = position - Double(index)
            return table[index] * (1 - fraction) + table[(index + 1) % table.count] * fraction
        }
        let smoothing = controlSmoothing
        currentBend += (targetBend - currentBend) * smoothing
        currentModulation += (targetModulation - currentModulation) * smoothing
        currentExpression += (targetExpression - currentExpression) * smoothing
        let bendRatio =
            currentBend == 0 && currentModulation == 0
            ? 1 : pow(2, (currentBend + sin(2 * .pi * age / rate * 5) * currentModulation * 0.25) / 12)
        var sample: Double
        if musicalVoice != nil {
            sample = musicalVoice!.next(ratio: bendRatio * organicMotion.ratio)
        } else {
            sample = lookup(table, phase) * (1 - mix)
            if mix > 0 { sample += lookup(secondTable, secondPhase) * mix }
            if sub > 0 { sample += sin(2 * Double.pi * subPhase) * sub * 0.5 }
            if noise > 0 {
                random ^= random << 13
                random ^= random >> 7
                random ^= random << 17
                sample += (Double(random & 0xFFFF) / 32767.5 - 1) * noise * 0.35
            }
        }
        filtered += filter * (sample - filtered)
        let gain = gated ? envelope() : releaseFrames > 0 ? releaseLevel * max(0, 1 - releaseAge / releaseFrames) : 0
        let driven = drive == 1 ? filtered : tanh(filtered * drive) / tanh(drive)
        phase += step * bendRatio * organicMotion.ratio
        phase -= floor(phase)
        secondPhase += secondStep * bendRatio * organicMotion.ratio
        secondPhase -= floor(secondPhase)
        subPhase += step * 0.5 * bendRatio * organicMotion.ratio
        subPhase -= floor(subPhase)
        age += 1
        if !gated { releaseAge += 1 }
        let result = Float(driven * gain * velocity * 0.35 * currentExpression * organicMotion.gain)
        organicMotion.advance()
        return result
    }
}

/// Bounded polyphonic live instrument. Its owner serializes voice commands and rendering.
public final class SoundDesignInstrument {
    private var voices: [SoundDesignVoice] = []
    private let maximumVoices: Int
    private var held: Set<Int> = [], pendingRelease: Set<Int> = []
    private var sustainDown = false
    private var controllers: [SoundDesignControllerKind: Double] = [:]
    /// A stolen voice fades over 5 ms instead of being cut (no click).
    static let stealFade = 240
    public var activeVoiceCount: Int { voices.filter { $0.stealFade == nil && !$0.isFinished }.count }
    public init(maximumVoices: Int = 8) {
        self.maximumVoices = max(1, min(32, maximumVoices))
        voices.reserveCapacity(self.maximumVoices * 2)
    }
    public func addVoice(_ prepared: SoundDesignVoice) {
        var voice = prepared
        for (kind, value) in controllers { voice.controller(kind, value: value, immediate: true) }
        held.insert(voice.keyPitch)
        pendingRelease.remove(voice.keyPitch)
        voices.removeAll { $0.isFinished || $0.stealFade == 0 }
        if voices.filter({ $0.stealFade == nil }).count >= maximumVoices,
            let oldest = voices.firstIndex(where: { $0.stealFade == nil })
        {
            voices[oldest].stealFade = Self.stealFade
        }
        if voices.count >= maximumVoices * 2 { voices.removeFirst() }
        voices.append(voice)
    }
    /// 1, the steal-fade gain, or nil once the stolen voice has faded out.
    private func stealGain(_ index: Int) -> Float? {
        guard let left = voices[index].stealFade else { return 1 }
        if left == 0 { return nil }
        voices[index].stealFade = left - 1
        return Float(left - 1) / Float(Self.stealFade)
    }
    public func controller(_ kind: SoundDesignControllerKind, value: Double) {
        if kind == .sustain {
            sustainDown = value >= 0.5
            if !sustainDown {
                for pitch in pendingRelease { release(pitch) }
                pendingRelease.removeAll(keepingCapacity: true)
            }
        } else {
            controllers[kind] = value
            for index in voices.indices { voices[index].controller(kind, value: value) }
        }
    }
    public func noteOff(pitch: Int) {
        held.remove(pitch)
        if sustainDown { pendingRelease.insert(pitch) } else { release(pitch) }
    }
    private func release(_ pitch: Int) {
        for index in voices.indices where voices[index].keyPitch == pitch { voices[index].noteOff() }
    }
    public func allNotesOff() {
        sustainDown = false
        held.removeAll(keepingCapacity: true)
        pendingRelease.removeAll(keepingCapacity: true)
        for index in voices.indices { voices[index].noteOff() }
    }
    public func reset() {
        voices.removeAll(keepingCapacity: true)
        held.removeAll(keepingCapacity: true)
        pendingRelease.removeAll(keepingCapacity: true)
        sustainDown = false
        controllers.removeAll(keepingCapacity: true)
    }
    public func render(into buffer: UnsafeMutablePointer<Float>, count: Int) {
        guard count > 0 else { return }
        for frame in 0..<count {
            var value: Float = 0
            for index in voices.indices {
                if let g = stealGain(index) { value += voices[index].renderNext() * voices[index].levelGain * g }
            }
            buffer[frame] = Float(tanh(Double(value)))
        }
        voices.removeAll { $0.isFinished || $0.stealFade == 0 }
    }
    public func renderStereo(left: UnsafeMutablePointer<Float>, right: UnsafeMutablePointer<Float>, count: Int) {
        guard count > 0 else { return }
        for frame in 0..<count {
            var l: Float = 0
            var r: Float = 0
            for index in voices.indices {
                guard let g = stealGain(index) else { continue }
                let sample = voices[index].renderNext() * g
                l += sample * voices[index].leftGain
                r += sample * voices[index].rightGain
            }
            left[frame] = Float(tanh(Double(l)))
            right[frame] = Float(tanh(Double(r)))
        }
        voices.removeAll { $0.isFinished || $0.stealFade == 0 }
    }
}
