import Foundation

/// Original voiced gain stages and algorithmic speaker filtering. All storage
/// is prepared on workers, including the two 4× oversampled nonlinear stages.
final class GuitarRigDSP {
    static let latencyFrames = 32
    private var pedal = WarmthDSP(), amp = WarmthDSP()
    private var room = StereoRoomDSP()
    private var chorus = ChorusState()
    private var left = Array(repeating: EQSection(), count: 10), right = Array(repeating: EQSection(), count: 10)
    private var targets = Array(repeating: EQSection(), count: 10)
    private var filterFrames = 0, driveFrames = 0
    private var driveTargets = [Float](repeating: 0, count: 4)
    private var delayL = [Float](repeating: 0, count: 57602), delayR = [Float](repeating: 0, count: 57602)
    private var delayIndex = 0
    private var dryL = [Float](repeating: 0, count: latencyFrames), dryR = [Float](repeating: 0, count: latencyFrames)
    private var dryIndex = 0
    private var values = [Float](repeating: 0, count: 30)
    private var dirty = true, initialized = false
    private var tick = 0
    private var previous = [Float](repeating: .nan, count: 30)
    private var masterGain: Float = 1
    private var inputGain: Float = 1, outputGain: Float = 1
    private var pedalValues = [Float](repeating: 0, count: 8), ampValues = [Float](repeating: 0, count: 8)
    private var roomValues = [Float](repeating: 0, count: 10), chorusValues = [Float](repeating: 0, count: 6)
    private var gateThreshold: Float = 0, envelope: Float = 0, gateGain: Float = 1
    private var hold = 0
    private var pedalBlend: Float = 0, ampBlend: Float = 0, cabBlend: Float = 0
    private var modWeights: [Float] = [1, 0, 0]
    private var phase: Double = 0
    private var delayFrames: Float = 1, delayTone: Float = 0, delayFilteredL: Float = 0, delayFilteredR: Float = 0
    var lastDryLeft: Float = 0, lastDryRight: Float = 0
    var storageBytes: Int { (delayL.count + delayR.count + 256) * MemoryLayout<Float>.stride + pedal.storageBytes + amp.storageBytes + room.storageBytes + chorus.seekStorageBytes + 4096 }

    func deepCopy() -> GuitarRigDSP {
        let copy = GuitarRigDSP()
        copy.pedal = pedal.deepCopy(); copy.amp = amp.deepCopy(); copy.room = room.deepCopy(); copy.chorus = chorus.seekCopy()
        copy.left = left.map { $0 }; copy.right = right.map { $0 }; copy.targets = targets.map { $0 }
        copy.filterFrames = filterFrames; copy.driveFrames = driveFrames; copy.driveTargets = driveTargets.map { $0 }
        copy.delayL = delayL.map { $0 }; copy.delayR = delayR.map { $0 }; copy.delayIndex = delayIndex
        copy.dryL = dryL.map { $0 }; copy.dryR = dryR.map { $0 }; copy.dryIndex = dryIndex
        copy.values = values.map { $0 }; copy.dirty = dirty; copy.initialized = initialized; copy.tick = tick
        copy.inputGain = inputGain; copy.outputGain = outputGain; copy.previous = previous.map { $0 }; copy.masterGain = masterGain
        copy.pedalValues = pedalValues.map { $0 }; copy.ampValues = ampValues.map { $0 }
        copy.roomValues = roomValues.map { $0 }; copy.chorusValues = chorusValues.map { $0 }
        copy.gateThreshold = gateThreshold; copy.envelope = envelope; copy.gateGain = gateGain; copy.hold = hold
        copy.pedalBlend = pedalBlend; copy.ampBlend = ampBlend; copy.cabBlend = cabBlend
        copy.modWeights = modWeights.map { $0 }; copy.phase = phase
        copy.delayFrames = delayFrames; copy.delayTone = delayTone; copy.delayFilteredL = delayFilteredL; copy.delayFilteredR = delayFilteredR
        copy.lastDryLeft = lastDryLeft; copy.lastDryRight = lastDryRight
        return copy
    }
    func configure(_ values: [Float]) {
        for i in self.values.indices { self.values[i] = values[i] }
        dirty = true
    }
    private func db(_ value: Float) -> Float { pow(10, value / 20) }
    private func glide(_ current: Float, _ target: Float) -> Float { current + max(-1 / 480, min(1 / 480, target - current)) }
    private func setFilter(_ index: Int, _ hz: Float, _ q: Float, _ gain: Float, _ shape: EQSection.Shape) {
        let old = targets[index]
        targets[index].configure(frequency: hz, q: q, gain: gain, shape: shape)
        let target = targets[index]
        if old.b0 != target.b0 || old.b1 != target.b1 || old.b2 != target.b2 || old.a1 != target.a1 || old.a2 != target.a2 { filterFrames = 480 }
    }
    private func controls() {
        let oldPedalDrive = pedalValues[0], oldPedalBias = pedalValues[6], oldAmpDrive = ampValues[0], oldAmpBias = ampValues[6]
        let v = values, pedalKind = Int(values[2]), ampKind = Int(values[6]), cabinet = Int(values[14])
        if previous[0] != v[0] { inputGain = db(v[0]) }
        if previous[28] != v[28] { outputGain = db(v[28]) }
        if previous[13] != v[13] { masterGain = db(v[13]) }
        if previous[1] != v[1] { gateThreshold = db(v[1]) }
        pedalValues[0] = v[3] + (pedalKind == 3 ? 6 : 0); pedalValues[1] = v[4]; pedalValues[2] = 1; pedalValues[3] = v[5]
        pedalValues[4] = 0; pedalValues[5] = pedalKind == 1 ? 2 : 4; pedalValues[6] = pedalKind == 3 ? 0.85 : 0.2; pedalValues[7] = pedalKind == 1 ? 4 : 0
        let gain: Float, character: Float, bias: Float, bass: Float, mid: Float, treble: Float
        switch ampKind {
        case 1: (gain, character, bias, bass, mid, treble) = (v[7] * 0.35, 2, 0.12, 0, -2, 1)
        case 2: (gain, character, bias, bass, mid, treble) = (3 + v[7] * 0.65, 2, 0.4, -2, 2, 2)
        case 3: (gain, character, bias, bass, mid, treble) = (6 + v[7] * 0.8, 4, 0.3, 0, 3, -1)
        case 4: (gain, character, bias, bass, mid, treble) = (12 + v[7] * 0.75, 4, 0.15, -1, -1, 0)
        default: (gain, character, bias, bass, mid, treble) = (0, 0, 0, 0, 0, 0)
        }
        ampValues[0] = gain; ampValues[1] = 18000; ampValues[2] = 1; ampValues[3] = 0
        ampValues[4] = 0; ampValues[5] = character; ampValues[6] = bias; ampValues[7] = v[11]
        driveTargets[0] = pedalValues[0]; driveTargets[1] = pedalValues[6]; driveTargets[2] = ampValues[0]; driveTargets[3] = ampValues[6]
        if initialized {
            pedalValues[0] = oldPedalDrive; pedalValues[6] = oldPedalBias; ampValues[0] = oldAmpDrive; ampValues[6] = oldAmpBias
            if driveTargets[0] != oldPedalDrive || driveTargets[1] != oldPedalBias || driveTargets[2] != oldAmpDrive || driveTargets[3] != oldAmpBias { driveFrames = 480 }
        }
        let pedalCutoff: Float = pedalKind == 1 ? 480 : pedalKind == 2 ? 110 : pedalKind == 3 ? 70 : 20
        if previous[2] != v[2] { setFilter(0, pedalCutoff, 0.707, 0, .highPass) }
        if previous[6] != v[6] || previous[8] != v[8] || previous[9] != v[9] || previous[10] != v[10] {
        setFilter(1, 140, 0.707, v[8] + bass, .lowShelf)
        setFilter(2, ampKind == 2 ? 1500 : 800, 0.75, v[9] + mid, .bell)
        setFilter(3, 3000, 0.707, v[10] + treble, .highShelf)
        }
        if previous[14] != v[14] || previous[15] != v[15] || previous[16] != v[16] || previous[17] != v[17] {
        setFilter(4, v[16], 0.707, 0, .highPass)
        let resonance: Float, boost: Float, scoop: Float, highCut: Float
        switch cabinet {
        case 1: (resonance, boost, scoop, highCut) = (110, 2, -1, 6500)
        case 2: (resonance, boost, scoop, highCut) = (95, 1, 1, 7200)
        case 3: (resonance, boost, scoop, highCut) = (100, 4, -3, 5600)
        case 4: (resonance, boost, scoop, highCut) = (85, 5, -4, 6200)
        default: (resonance, boost, scoop, highCut) = (100, 0, 0, 16000)
        }
        setFilter(5, resonance, 0.85, boost, .bell); setFilter(6, 650, 0.8, scoop, .bell)
        setFilter(7, 2400, 1.1, (1 - v[15]) * 4 - v[15] * 5, .bell)
        let cutoff = min(v[17], highCut * (1 - v[15] * 0.35))
        setFilter(8, cutoff, 0.5411961, 0, .lowPass); setFilter(9, cutoff, 1.306563, 0, .lowPass)
        }
        chorusValues[0] = v[19]; chorusValues[1] = v[20] * 8; chorusValues[2] = 12; chorusValues[3] = 0.85; chorusValues[4] = 0; chorusValues[5] = v[21]
        roomValues[0] = v[26]; roomValues[1] = 0; roomValues[2] = 5000; roomValues[3] = 1; roomValues[4] = 1
        roomValues[5] = 120; roomValues[6] = 0.8; roomValues[7] = 0.85; roomValues[8] = 0.15; roomValues[9] = 0
        if previous[26] != v[26] { room.configure(roomValues) }
        if previous[22] != v[22] { delayTone = 1 - exp(-2 * Float.pi * v[22] / 48000) }
        for i in previous.indices { previous[i] = v[i] }
        if !initialized {
            for i in targets.indices { left[i].copyCoefficients(from: targets[i]); right[i].copyCoefficients(from: targets[i]) }
            filterFrames = 0
            pedalBlend = pedalKind > 0 ? 1 : 0; ampBlend = ampKind > 0 ? 1 : 0; cabBlend = cabinet > 0 ? 1 : 0
            for i in modWeights.indices { modWeights[i] = i == Int(v[18]) ? 1 : 0 }
            delayFrames = v[23] * 48; initialized = true
        }
        dirty = false
    }
    private func filter(_ x: (Float, Float), _ from: Int, _ to: Int) -> (Float, Float) {
        var l = x.0, r = x.1
        for i in from..<to { l = Float(left[i].process(Double(l))); r = Float(right[i].process(Double(r))) }
        return (l, r)
    }
    private func saturate(_ state: WarmthDSP, _ x: (Float, Float), _ v: [Float]) -> (Float, Float) {
        state.process(left: x.0, right: x.1, driveDB: v[0], tone: v[1], mix: v[2], outputDB: v[3], matchLevel: false, character: Int(v[5]), bias: v[6], emphasisDB: v[7])
    }
    func process(left inputL: Float, right inputR: Float) -> (Float, Float) {
        if dirty && tick == 0 { controls() }; tick = (tick + 1) % 32
        if filterFrames > 0 {
            let step = 1 / Double(filterFrames)
            for i in targets.indices {
                left[i].b0 += (targets[i].b0 - left[i].b0) * step; left[i].b1 += (targets[i].b1 - left[i].b1) * step; left[i].b2 += (targets[i].b2 - left[i].b2) * step
                left[i].a1 += (targets[i].a1 - left[i].a1) * step; left[i].a2 += (targets[i].a2 - left[i].a2) * step
                right[i].copyCoefficients(from: left[i])
            }
            filterFrames -= 1
        }
        if driveFrames > 0 {
            let step = 1 / Float(driveFrames)
            pedalValues[0] += (driveTargets[0] - pedalValues[0]) * step; pedalValues[6] += (driveTargets[1] - pedalValues[6]) * step
            ampValues[0] += (driveTargets[2] - ampValues[0]) * step; ampValues[6] += (driveTargets[3] - ampValues[6]) * step
            driveFrames -= 1
        }
        let inputL = inputL.isFinite ? inputL : 0, inputR = inputR.isFinite ? inputR : 0
        let v = values
        lastDryLeft = dryL[dryIndex]; lastDryRight = dryR[dryIndex]
        dryL[dryIndex] = inputL; dryR[dryIndex] = inputR; dryIndex = (dryIndex + 1) % Self.latencyFrames
        var x = (max(-64, min(64, inputL)) * inputGain, max(-64, min(64, inputR)) * inputGain)
        let peak = max(abs(x.0), abs(x.1))
        envelope += (peak > envelope ? 0.02 : 0.00014) * (peak - envelope)
        if envelope >= gateThreshold { hold = 1200 } else { hold = max(0, hold - 1) }
        let target: Float = v[1] <= -89.9 || hold > 0 ? 1 : pow(min(1, envelope / gateThreshold), 2)
        gateGain += (target > gateGain ? 0.02 : 0.001) * (target - gateGain)
        x.0 *= gateGain; x.1 *= gateGain
        pedalBlend = glide(pedalBlend, v[2] >= 0.5 ? 1 : 0); ampBlend = glide(ampBlend, v[6] >= 0.5 ? 1 : 0); cabBlend = glide(cabBlend, v[14] >= 0.5 ? 1 : 0)
        let filtered = filter(x, 0, 1)
        x.0 += (filtered.0 - x.0) * pedalBlend; x.1 += (filtered.1 - x.1) * pedalBlend
        pedalValues[2] = pedalBlend; pedalValues[3] = v[5] * pedalBlend
        x = saturate(pedal, x, pedalValues)
        let sag = 1 / (1 + envelope * v[12] * 3 * ampBlend)
        x.0 *= sag; x.1 *= sag
        ampValues[2] = ampBlend; x = saturate(amp, x, ampValues)
        let toned = filter(x, 1, 4), master = 1 + (masterGain - 1) * ampBlend
        x.0 = (x.0 + (toned.0 - x.0) * ampBlend) * master; x.1 = (x.1 + (toned.1 - x.1) * ampBlend) * master
        let cabinet = filter(x, 4, 10)
        x.0 += (cabinet.0 - x.0) * cabBlend; x.1 += (cabinet.1 - x.1) * cabBlend
        chorusValues[5] = v[18] == 1 || modWeights[1] > 0 ? v[21] : 0
        let doubled = chorus.process(left: x.0, right: x.1, rate: chorusValues[0], depth: chorusValues[1], delay: chorusValues[2], width: chorusValues[3], feedback: 0, mix: chorusValues[5])
        let tremolo: Float = v[18] == 2 || modWeights[2] > 0 ? 1 - v[20] * Float((sin(phase) + 1) * 0.5) * v[21] : 1
        phase = (phase + 2 * Double.pi * Double(v[19]) / 48000).truncatingRemainder(dividingBy: 2 * .pi)
        for i in modWeights.indices { modWeights[i] = glide(modWeights[i], i == Int(v[18]) ? 1 : 0) }
        x.0 = x.0 * modWeights[0] + doubled.0 * modWeights[1] + x.0 * tremolo * modWeights[2]
        x.1 = x.1 * modWeights[0] + doubled.1 * modWeights[1] + x.1 * tremolo * modWeights[2]
        delayFrames += (v[23] * 48 - delayFrames) * 0.001
        let position = Float(delayIndex) - delayFrames, lower = Int(floor(position)), fraction = position - Float(lower)
        let first = (lower % delayL.count + delayL.count) % delayL.count, next = (first + 1) % delayL.count
        let wetL = delayL[first] * (1 - fraction) + delayL[next] * fraction, wetR = delayR[first] * (1 - fraction) + delayR[next] * fraction
        delayFilteredL += delayTone * (wetL - delayFilteredL); delayFilteredR += delayTone * (wetR - delayFilteredR)
        delayL[delayIndex] = x.0 + delayFilteredL * v[24]; delayR[delayIndex] = x.1 + delayFilteredR * v[24]
        x.0 += delayFilteredL * v[25]; x.1 += delayFilteredR * v[25]; delayIndex = (delayIndex + 1) % delayL.count
        let space = room.process(left: x.0, right: x.1, classicLeft: 0, classicRight: 0)
        x.0 = (x.0 + (space.0 - x.0) * v[27]) * outputGain; x.1 = (x.1 + (space.1 - x.1) * v[27]) * outputGain
        return (lastDryLeft * (1 - v[29]) + x.0 * v[29], lastDryRight * (1 - v[29]) + x.1 * v[29])
    }
}
