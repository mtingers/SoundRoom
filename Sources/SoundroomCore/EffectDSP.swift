import Foundation

/// Transposed direct-form II filters; coefficients update without clearing history.
struct Biquad {
    var b0: Float = 1, b1: Float = 0, b2: Float = 0, a1: Float = 0, a2: Float = 0
    var z1: Float = 0, z2: Float = 0
    mutating func filter(_ input: Float) -> Float {
        let output = b0 * input + z1
        z1 = b1 * input - a1 * output + z2; z2 = b2 * input - a2 * output
        if !output.isFinite || !z1.isFinite || !z2.isFinite { z1 = 0; z2 = 0; return 0 }
        return output
    }
    mutating func configure(frequency: Float, q: Float, gain: Float = 0, mode: RealtimeEffectKind) {
        let w = 2 * Float.pi * min(Float(sessionRate) * 0.45, frequency) / Float(sessionRate)
        let c = cos(w), alpha = sin(w) / (2 * q)
        var denominator: Float = 1 + alpha
        if mode == .lowPass { b0 = (1 - c) / 2; b1 = 1 - c; b2 = b0; a1 = -2 * c; a2 = 1 - alpha }
        else if mode == .highPass { b0 = (1 + c) / 2; b1 = -(1 + c); b2 = b0; a1 = -2 * c; a2 = 1 - alpha }
        else {
            let amplitude = pow(Float(10), gain / 40)
            denominator = 1 + alpha / amplitude
            b0 = 1 + alpha * amplitude; b1 = -2 * c; b2 = 1 - alpha * amplitude
            a1 = -2 * c; a2 = 1 - alpha / amplitude
        }
        b0 /= denominator; b1 /= denominator; b2 /= denominator; a1 /= denominator; a2 /= denominator
    }
}

/// Parallel decorrelated combs with damped feedback, followed by two diffusion allpasses.
struct RoomNetwork {
    var lines: [[Float]]
    var positions = [0, 0, 0, 0]
    var damping = [Float](repeating: 0, count: 4)
    var allpasses: [[Float]]
    var allpassPositions = [0, 0]
    private var lastDecay: Float = -1, lastTone: Float = -1
    private var coefficient: Float = 0
    var feedback = [Float](repeating: 0, count: 4)
    init(offset: Int) {
        lines = [1423, 2137, 3163, 4789].map { Array(repeating: 0, count: $0 + offset) }
        allpasses = [347, 113].map { Array(repeating: 0, count: $0 + offset) }
    }
    mutating func process(_ input: Float, decay: Float, tone: Float) -> Float {
        if tone != lastTone { coefficient = 1 - exp(-2 * Float.pi * tone / Float(sessionRate)); lastTone = tone }
        if decay != lastDecay {
            for i in 0..<4 { feedback[i] = pow(Float(10), -3 * Float(lines[i].count) / (Float(sessionRate) * decay)) }
            lastDecay = decay
        }
        var result: Float = 0
        for i in 0..<4 {
            let delayed = lines[i][positions[i]]
            damping[i] += coefficient * (delayed - damping[i])
            lines[i][positions[i]] = input + damping[i] * feedback[i]
            positions[i] = (positions[i] + 1) % lines[i].count
            result += delayed * 0.25
        }
        for i in 0..<2 {
            let delayed = allpasses[i][allpassPositions[i]]
            let output = delayed - result * 0.5
            allpasses[i][allpassPositions[i]] = result + output * 0.5
            allpassPositions[i] = (allpassPositions[i] + 1) % allpasses[i].count
            result = output
        }
        return result
    }
}

extension EffectProcessor {
    func process(left: UnsafeMutablePointer<Float>, right: UnsafeMutablePointer<Float>, count: Int, position: Int) {
        if usesLegacy {
            processLegacy(left: left, right: right, count: count, position: position); return
        }
        if soundPuttyShapePending, let shape = soundPuttyShape { state.putty!.setShape(shape); soundPuttyShapePending = false }
        var measurement = EffectMeterAccumulator()
        for j in descriptors.indices {
            let descriptor = descriptors[j]
            values[j] = descriptor.physical(parameterCursors[j].value(at: position))
        }
        if let index = legacyParameterIndex { values[index] = legacyParameterValue(at: position, index: index) }
        for i in 0..<count {
            if automated {
                for j in descriptors.indices {
                    let descriptor = descriptors[j]
                    if parameterCursors[j].hasPoints {
                        values[j] = descriptor.physical(parameterCursors[j].value(at: position + i))
                    }
                }
                if let index = legacyParameterIndex, legacyCursor.hasPoints {
                    values[index] = legacyParameterValue(at: position + i, index: index)
                }
            }
            if hasParameterRamps {
                var remaining = false
                for j in parameterRamps.indices where parameterRamps[j] != nil {
                    values[j] = descriptors[j].physical(parameterRamps[j]!.next())
                    if parameterRamps[j]!.completed { parameterRamps[j] = nil } else { remaining = true }
                }
                hasParameterRamps = remaining
            }
            if cachedValues != values {
                switch effect.kind {
                case .lowPass, .highPass:
                    state.biquadsL[0].configure(frequency: values[0], q: values[1], mode: effect.kind)
                    state.biquadsR[0].configure(frequency: values[0], q: values[1], mode: effect.kind)
                case .limiter: state.limiter!.configure(values)
                case .resonance: state.resonance!.configure(values)
                case .guitarRig: state.guitarRig!.configure(values)
                case .soundPutty: state.putty!.configure(values)
                case .vocalEnhancer: state.vocal!.configure(values)
                case .sourceIsolator: state.isolation!.configure(values)
                case .equalizer:
                    state.equalizer!.configure(values)
                case .room:
                    state.stereoRoom!.configure(values)
                default: break
                }
                for j in values.indices { cachedValues[j] = values[j] }
            }
            let dryL = left[i].isFinite ? left[i] : 0, dryR = right[i].isFinite ? right[i] : 0
            var wetL = dryL, wetR = dryR
            var alignedDryL = dryL, alignedDryR = dryR
            var reductionDB: Float = 0
            switch effect.kind {
            case .guitarRig:
                (wetL, wetR) = state.guitarRig!.process(left: dryL, right: dryR)
                alignedDryL = state.guitarRig!.lastDryLeft; alignedDryR = state.guitarRig!.lastDryRight
            case .gate:
                (wetL, wetR) = state.gate!.process(dryL, dryR, values)
                reductionDB = state.gate!.reductionDB * bypassBlend
            case .utility:
                (wetL, wetR) = state.utility!.process(dryL, dryR, values)
            case .gain:
                let gain = pow(Float(10), values[0] / 20); wetL *= gain; wetR *= gain
            case .lowPass, .highPass:
                wetL = state.biquadsL[0].filter(dryL); wetR = state.biquadsR[0].filter(dryR)
            case .echo, .room:
                let time = effect.kind == .echo ? values[0] : values[1]
                let delay = max(Float(effect.kind == .echo ? 1 : 0), time / 1000 * Float(sessionRate))
                let count = state.advancedDelayL.count
                let cursor = Float(state.advancedDelayIndex) - delay
                let floorCursor = Int(floor(cursor)), fraction = cursor - Float(floorCursor)
                let first = (floorCursor % count + count) % count, next = (first + 1) % count
                var delayedL = state.advancedDelayL[first] * (1 - fraction) + state.advancedDelayL[next] * fraction
                var delayedR = state.advancedDelayR[first] * (1 - fraction) + state.advancedDelayR[next] * fraction
                if effect.kind == .echo {
                    let coefficient = 1 - exp(-2 * Float.pi * values[3] / Float(sessionRate))
                    state.toneL += coefficient * (delayedL - state.toneL); state.toneR += coefficient * (delayedR - state.toneR)
                    state.advancedDelayL[state.advancedDelayIndex] = dryL + state.toneL * values[1]
                    state.advancedDelayR[state.advancedDelayIndex] = dryR + state.toneR * values[1]
                    wetL = dryL * (1 - values[2]) + delayedL * values[2]
                    wetR = dryR * (1 - values[2]) + delayedR * values[2]
                } else {
                    if delay < 1 {
                        // The current write slot is old audio, not the current
                        // input. Interpolate sub-sample pre-delay against dry.
                        let previous = (state.advancedDelayIndex + count - 1) % count
                        delayedL = dryL * (1 - delay) + state.advancedDelayL[previous] * delay
                        delayedR = dryR * (1 - delay) + state.advancedDelayR[previous] * delay
                    }
                    state.advancedDelayL[state.advancedDelayIndex] = dryL; state.advancedDelayR[state.advancedDelayIndex] = dryR
                    let reverbL = state.roomL!.process(delayedL, decay: values[0], tone: values[2])
                    let reverbR = state.roomR!.process(delayedR, decay: values[0], tone: values[2])
                    let room = state.stereoRoom!.process(left: delayedL, right: delayedR, classicLeft: reverbL, classicRight: reverbR)
                    wetL = (dryL * (1 - values[3]) + room.0 * values[3]) * state.stereoRoom!.outputGain
                    wetR = (dryR * (1 - values[3]) + room.1 * values[3]) * state.stereoRoom!.outputGain
                }
                state.advancedDelayIndex = (state.advancedDelayIndex + 1) % count
            case .warmth:
                (wetL, wetR) = state.warmth!.process(left: dryL, right: dryR, driveDB: values[0],
                    tone: values[1], mix: values[2], outputDB: values[3], matchLevel: values[4] >= 0.5, character: Int(values[5]), bias: values[6], emphasisDB: values[7])
                alignedDryL = state.warmth!.lastDryLeft; alignedDryR = state.warmth!.lastDryRight
            case .limiter:
                (wetL, wetR) = state.limiter!.process(left: dryL, right: dryR)
                alignedDryL = state.limiter!.lastDryLeft; alignedDryR = state.limiter!.lastDryRight
                reductionDB = state.limiter!.reductionDB * bypassBlend
                measurement.snapshot.truePeak = max(measurement.snapshot.truePeak, state.limiter!.outputTruePeak)
                measurement.snapshot.momentaryLUFS = state.limiter!.loudness.momentary
                measurement.snapshot.shortTermLUFS = state.limiter!.loudness.shortTerm
            case .resonance:
                (wetL, wetR) = state.resonance!.process(left: dryL, right: dryR)
                alignedDryL = state.resonance!.lastDryLeft; alignedDryR = state.resonance!.lastDryRight
                reductionDB = state.resonance!.reductionDB * bypassBlend
                measurement.snapshot.resonance = state.resonance!.display
            case .soundPutty:
                (wetL, wetR) = state.putty!.process(left: dryL, right: dryR)
            case .vocalEnhancer:
                (wetL, wetR) = state.vocal!.process(left: dryL, right: dryR)
                alignedDryL = state.vocal!.lastDryLeft; alignedDryR = state.vocal!.lastDryRight
                reductionDB = state.vocal!.reductionDB * bypassBlend
            case .sourceIsolator:
                (wetL, wetR) = state.isolation!.process(left: dryL, right: dryR)
                alignedDryL = state.isolation!.lastDryLeft; alignedDryR = state.isolation!.lastDryRight
                reductionDB = state.isolation!.reductionDB * bypassBlend
                measurement.snapshot.resonance = state.isolation!.display
            case .compressor:
                let level = externalDetector?[externalDetectorOffset + i] ?? max(abs(dryL), abs(dryR))
                let milliseconds = level > state.envelope ? values[2] : values[3]
                state.envelope += (1 - exp(-1 / (milliseconds / 1000 * Float(sessionRate)))) * (level - state.envelope)
                let inputDB = 20 * log10(max(0.000001, state.envelope)), over = inputDB - values[0]
                let slope = 1 / values[1] - 1
                let reduction: Float
                if values[4] > 0 && abs(over) < values[4] / 2 { reduction = slope * pow(over + values[4] / 2, 2) / (2 * values[4]) }
                else { reduction = over > 0 ? slope * over : 0 }
                reductionDB = max(0, -reduction) * bypassBlend
                let gain = pow(Float(10), (reduction + values[5]) / 20)
                wetL = dryL * (1 - values[6]) + dryL * gain * values[6]
                wetR = dryR * (1 - values[6]) + dryR * gain * values[6]
            case .equalizer:
                (wetL, wetR) = state.equalizer!.process(left: dryL, right: dryR)
            case .chorus:
                (wetL, wetR) = state.chorus!.process(left: dryL, right: dryR, rate: values[0], depth: values[1],
                    delay: values[2], width: values[3], feedback: values[4], mix: values[5])
            case .pitchCorrection:
                var controls = PitchCorrectionParameters(key: Int(values[0]), scale: Int(values[1]), mask: Int(values[2]),
                    reference: values[3], retune: values[4], strength: values[5], humanize: values[6],
                    tolerance: values[7], vibrato: values[8], formant: values[9], mix: values[10], manualTarget: values[11])
                if let edit = pitchEditCursor?.sample(at: position + i) {
                    controls.manualTarget = edit.note
                    if effect.pitchEditsOnly { controls.mix *= edit.scopeBlend }
                }
                else if effect.pitchEditsOnly { controls.mix = 0 }
                (wetL, wetR) = state.pitch!.process(left: dryL, right: dryR, parameters: controls)
                alignedDryL = state.pitch!.lastDryLeft; alignedDryR = state.pitch!.lastDryRight
            case .dither:
                wetL = DitherDSP.process(dryL, frame: position + i, channel: 0, seed: noiseSeed,
                    bits: values[0], amount: values[1], quantize: values[2] >= 0.5)
                wetR = DitherDSP.process(dryR, frame: position + i, channel: 1, seed: noiseSeed,
                    bits: values[0], amount: values[1], quantize: values[2] >= 0.5)
            }
            advanceBypass()
            let baked = isBaked(outputFrame: position + i - effect.latencyFrames)
            left[i] = baked ? alignedDryL : blendBypass(dry: alignedDryL, wet: wetL)
            right[i] = baked ? alignedDryR : blendBypass(dry: alignedDryR, wet: wetR)
            if baked { reductionDB = 0 }
            if effect.kind == .pitchCorrection, let pitch = state.pitch {
                var feedback = pitch.meterSnapshot
                // Stage processing positions already account for earlier insert
                // latency. Keep the feedback timestamp in source timeline frames.
                feedback.sourceFrame = position + i - effect.latencyFrames
                if let loop = pitchMeterLoopRange {
                    feedback.sourceFrame = loop.lowerBound + ((feedback.sourceFrame - loop.lowerBound) % loop.count + loop.count) % loop.count
                }
                feedback.wetBlend *= baked ? 0 : bypassBlend
                measurement.snapshot.pitch = pitchMeterDelay?.process(feedback) ?? feedback
            }
            measurement.add(inputL: dryL, inputR: dryR, outputL: left[i], outputR: right[i], reduction: reductionDB)
        }
        meter.publish(measurement.finish())
    }
}
