import Foundation

/// Four-times oversampled saturation. FIR histories and dry alignment are allocated
/// before playback; processing performs no allocation. Both paths have 16-frame latency.
final class WarmthDSP {
    static let latencyFrames = 16
    private static let taps: [Float] = {
        let count = 65, center = 32, cutoff = 0.105
        let values = (0..<count).map { i -> Double in
            let distance = Double(i - center)
            let sinc = distance == 0 ? 2 * cutoff : sin(2 * .pi * cutoff * distance) / (.pi * distance)
            let phase = 2 * Double.pi * Double(i) / Double(count - 1)
            return sinc * (0.42 - 0.5 * cos(phase) + 0.08 * cos(2 * phase))
        }
        let sum = values.reduce(0, +)
        return values.map { Float($0 / sum) }
    }()
    private var inputL = [Float](repeating: 0, count: 17), inputR = [Float](repeating: 0, count: 17)
    private var highL = [Float](repeating: 0, count: 65), highR = [Float](repeating: 0, count: 65)
    private var dryL = [Float](repeating: 0, count: latencyFrames), dryR = [Float](repeating: 0, count: latencyFrames)
    private var mixes = [Float](repeating: 1, count: latencyFrames), outputs = [Float](repeating: 0, count: latencyFrames)
    private var matches = [Bool](repeating: false, count: latencyFrames)
    private var initializedParameters = false
    private var inputIndex = 0, highIndex = 0, dryIndex = 0
    private var toneL: Float = 0, toneR: Float = 0
    private var presenceL: Float = 0, presenceR: Float = 0
    private var dcL: Float = 0, dcR: Float = 0
    private var characterWeights = SIMD8<Float>(repeating: 0), characterTarget = 0, characterFramesRemaining = 0
    private var characterInitialized = false
    private var inputPower: Float = 0, outputPower: Float = 0, matchGain: Float = 1
    private let powerCoefficient: Float = 1 - exp(-1 / (0.5 * Float(sessionRate)))
    private let gainCoefficient: Float = 1 - exp(-1 / (0.02 * Float(sessionRate)))
    private var previous = [Float](repeating: .nan, count: 5)
    private var cachedDrive: Float = 1, cachedTone: Float = 0, cachedEmphasis: Float = 0, cachedOutput: Float = 1
    private var norms = [Float](repeating: 1, count: 5)
    private var offset: Float = 0, offsetTanh: Float = 0
    var lastDryLeft: Float = 0, lastDryRight: Float = 0
    var levelMatchGain: Float { matchGain }
    var storageBytes: Int { (17 * 2 + 65 * 2 + Self.latencyFrames * 4) * MemoryLayout<Float>.stride + Self.latencyFrames * MemoryLayout<Bool>.stride + 256 }

    init() { _ = Self.taps } // Force immutable coefficient preparation off the callback.

    func deepCopy() -> WarmthDSP {
        let copy = WarmthDSP()
        copy.inputL = inputL.map { $0 }; copy.inputR = inputR.map { $0 }
        copy.highL = highL.map { $0 }; copy.highR = highR.map { $0 }
        copy.dryL = dryL.map { $0 }; copy.dryR = dryR.map { $0 }
        copy.mixes = mixes.map { $0 }; copy.outputs = outputs.map { $0 }; copy.matches = matches.map { $0 }
        copy.previous = previous.map { $0 }; copy.norms = norms.map { $0 }; copy.cachedDrive = cachedDrive; copy.cachedTone = cachedTone
        copy.cachedEmphasis = cachedEmphasis; copy.cachedOutput = cachedOutput; copy.offset = offset; copy.offsetTanh = offsetTanh
        copy.initializedParameters = initializedParameters
        copy.inputIndex = inputIndex; copy.highIndex = highIndex; copy.dryIndex = dryIndex
        copy.toneL = toneL; copy.toneR = toneR; copy.inputPower = inputPower; copy.outputPower = outputPower
        copy.presenceL = presenceL; copy.presenceR = presenceR; copy.dcL = dcL; copy.dcR = dcR
        copy.characterWeights = characterWeights; copy.characterTarget = characterTarget; copy.characterFramesRemaining = characterFramesRemaining; copy.characterInitialized = characterInitialized
        copy.matchGain = matchGain; copy.lastDryLeft = lastDryLeft; copy.lastDryRight = lastDryRight
        return copy
    }

    /// Original nonlinear curves, not emulations of named hardware units.
    static func shape(_ x: Float, drive: Float, normalization: Float, character: Int, bias: Float) -> Float {
        switch character {
        case 1: return atan(x * 1.4) / atan(drive * 1.4) // progressively rounded tape-like curve
        case 2:
            let offset = bias * 0.8
            return (tanh(x + offset) - tanh(offset)) / max(0.1, (tanh(drive + offset) - tanh(-drive + offset)) * 0.5)
        case 3: return x / sqrt(1 + x * x) * sqrt(1 + drive * drive) / drive
        case 4:
            let positive = 1 + bias, negative = 1 - bias * 0.45
            let k = x >= 0 ? positive : negative
            return tanh(x * k * 1.8) / max(0.1, tanh(drive * 1.8))
        default: return tanh(x) / normalization
        }
    }
    private func shaped(_ x: Float, character: Int, bias: Float) -> Float {
        switch character {
        case 1: return atan(x * 1.4) / norms[1]
        case 2: return (tanh(x + offset) - offsetTanh) / norms[2]
        case 3: return x / sqrt(1 + x * x) * norms[3] / cachedDrive
        case 4:
            let k: Float = x >= 0 ? 1 + bias : 1 - bias * 0.45
            return tanh(x * k * 1.8) / norms[4]
        default: return tanh(x) / norms[0]
        }
    }
    func process(left: Float, right: Float, driveDB: Float, tone: Float, mix: Float,
                 outputDB: Float, matchLevel: Bool, character: Int = 0, bias: Float = 0.25, emphasisDB: Float = 0) -> (Float, Float) {
        if !initializedParameters {
            for i in mixes.indices { mixes[i] = mix; outputs[i] = outputDB; matches[i] = matchLevel }
            initializedParameters = true
        }
        let left = left.isFinite ? left : 0, right = right.isFinite ? right : 0
        lastDryLeft = dryL[dryIndex]; lastDryRight = dryR[dryIndex]
        let delayedMix = mixes[dryIndex], delayedOutput = outputs[dryIndex], delayedMatch = matches[dryIndex]
        mixes[dryIndex] = mix; outputs[dryIndex] = outputDB; matches[dryIndex] = matchLevel
        dryL[dryIndex] = left; dryR[dryIndex] = right; dryIndex = (dryIndex + 1) % Self.latencyFrames
        let selectedCharacter = min(4, max(0, character))
        if !characterInitialized { characterTarget = selectedCharacter; characterWeights[selectedCharacter] = 1; characterInitialized = true }
        if selectedCharacter != characterTarget { characterTarget = selectedCharacter; characterFramesRemaining = 240 }
        if characterFramesRemaining > 0 {
            for model in 0..<5 {
                let target: Float = model == characterTarget ? 1 : 0
                characterWeights[model] += (target - characterWeights[model]) / Float(characterFramesRemaining)
            }
            characterFramesRemaining -= 1
        }
        let presenceCoefficient: Float = 1 - exp(-2 * Float.pi * 1800 / Float(sessionRate))
        presenceL += presenceCoefficient * (left - presenceL); presenceR += presenceCoefficient * (right - presenceR)
        let newCharacterWeight = 1 - characterWeights[0]
        if previous[0] != driveDB || previous[4] != bias {
            cachedDrive = pow(Float(10), driveDB / 20); offset = bias * 0.8; offsetTanh = tanh(offset)
            norms[0] = tanh(cachedDrive); norms[1] = atan(cachedDrive * 1.4)
            norms[2] = max(0.1, (tanh(cachedDrive + offset) - tanh(-cachedDrive + offset)) * 0.5)
            norms[3] = sqrt(1 + cachedDrive * cachedDrive); norms[4] = max(0.1, tanh(cachedDrive * 1.8))
            previous[0] = driveDB; previous[4] = bias
        }
        if previous[1] != tone { cachedTone = 1 - exp(-2 * Float.pi * tone / Float(sessionRate)); previous[1] = tone }
        if previous[2] != emphasisDB { cachedEmphasis = pow(Float(10), emphasisDB / 20) - 1; previous[2] = emphasisDB }
        if previous[3] != delayedOutput { cachedOutput = pow(Float(10), delayedOutput / 20); previous[3] = delayedOutput }
        let emphasis = cachedEmphasis * newCharacterWeight
        inputL[inputIndex] = left + (left - presenceL) * emphasis
        inputR[inputIndex] = right + (right - presenceR) * emphasis
        let drive = cachedDrive
        var downL: Float = 0, downR: Float = 0
        for phase in 0..<4 {
            var upL: Float = 0, upR: Float = 0
            var tap = phase, history = inputIndex
            while tap < Self.taps.count {
                let coefficient = Self.taps[tap] * 4
                upL += coefficient * inputL[history]; upR += coefficient * inputR[history]
                tap += 4; history = (history + 16) % 17
            }
            var shapedL: Float = 0, shapedR: Float = 0
            for model in 0..<5 where characterWeights[model] > 0 {
                shapedL += shaped(upL * drive, character: model, bias: bias) * characterWeights[model]
                shapedR += shaped(upR * drive, character: model, bias: bias) * characterWeights[model]
            }
            highL[highIndex] = shapedL; highR[highIndex] = shapedR
            // Taking phase zero preserves the exact 16-base-frame linear-phase delay.
            if phase == 0 {
                var history = highIndex
                for coefficient in Self.taps {
                    downL += coefficient * highL[history]; downR += coefficient * highR[history]
                    history = (history + 64) % 65
                }
            }
            highIndex = (highIndex + 1) % 65
        }
        inputIndex = (inputIndex + 1) % 17
        // Remove asymmetry-induced DC on the new character paths. Classic is
        // deliberately untouched, retaining previous expanded Warmth renders.
        dcL += 0.00065 * (downL - dcL); dcR += 0.00065 * (downR - dcR)
        downL -= dcL * newCharacterWeight; downR -= dcR * newCharacterWeight
        let toneCoefficient = cachedTone
        toneL += toneCoefficient * (downL - toneL); toneR += toneCoefficient * (downR - toneR)
        inputPower += powerCoefficient * ((lastDryLeft * lastDryLeft + lastDryRight * lastDryRight) * 0.5 - inputPower)
        outputPower += powerCoefficient * ((toneL * toneL + toneR * toneR) * 0.5 - outputPower)
        let target = delayedMatch && outputPower > 1e-12 ? min(4, max(0.05, sqrt(inputPower / outputPower))) : 1
        matchGain += gainCoefficient * (target - matchGain)
        let output = cachedOutput
        return ((lastDryLeft * (1 - delayedMix) + toneL * matchGain * delayedMix) * output,
                (lastDryRight * (1 - delayedMix) + toneR * matchGain * delayedMix) * output)
    }
}
