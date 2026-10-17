import Accelerate
import Foundation

/// Spectral cleanup and harmonic/percussive or mid/side component masks.
/// These controls do not recognize instruments; learned stem separation is a
/// separate offline operation. All callback storage is allocated at creation.
final class SourceIsolatorDSP {
    static let latencyFrames = 2048
    private static let size = 2048, hop = 512, ringSize = 4096, bins = 1025, historyFrames = 9
    private let forward: vDSP_DFT_Setup, inverse: vDSP_DFT_Setup
    private var window: [Float]
    private var inputL = [Float](repeating: 0, count: size), inputR = [Float](repeating: 0, count: size)
    private var outputL = [Float](repeating: 0, count: ringSize), outputR = [Float](repeating: 0, count: ringSize)
    private var real = [Float](repeating: 0, count: size), imaginary = [Float](repeating: 0, count: size)
    private var spectrumL = [Float](repeating: 0, count: size), imagL = [Float](repeating: 0, count: size)
    private var spectrumR = [Float](repeating: 0, count: size), imagR = [Float](repeating: 0, count: size)
    private var levels = [Float](repeating: 0, count: bins)
    private var history = [Float](repeating: 0, count: bins * historyFrames)
    private var masks = [Float](repeating: 1, count: bins), gains = [Float](repeating: 1, count: bins)
    private var mixHistory = [Float](repeating: 1, count: size), strengthHistory = [Float](repeating: 0.75, count: size)
    private var listenHistory = [Bool](repeating: false, count: size)
    private var temporal = [Float](repeating: 0, count: historyFrames), frequency = [Float](repeating: 0, count: 17)
    private var inputIndex = 0, outputIndex = 0, hopClock = 0, historyIndex = 0
    private var initializedControls = false
    private var mode = 0
    private var strength: Float = 0.75, floorPower = pow(Float(10), -55 / 10) * Float(size) * 0.5
    private var low: Float = 20, high: Float = 20000, mix: Float = 1
    private var listen = false
    private(set) var lastDryLeft: Float = 0, lastDryRight: Float = 0, reductionDB: Float = 0
    private(set) var display = SpectralReductionSnapshot()
    var storageBytes: Int {
        (Self.size * 11 + Self.ringSize * 2 + Self.bins * (Self.historyFrames + 3) + Self.historyFrames + 17) * 4
            + Self.size + 512
    }

    init() {
        forward = vDSP_DFT_zop_CreateSetup(nil, vDSP_Length(Self.size), .FORWARD)!
        inverse = vDSP_DFT_zop_CreateSetup(nil, vDSP_Length(Self.size), .INVERSE)!
        window = (0..<Self.size).map { sqrt(0.5 - 0.5 * cos(2 * Float.pi * Float($0) / Float(Self.size))) }
    }
    deinit {
        vDSP_DFT_DestroySetup(forward)
        vDSP_DFT_DestroySetup(inverse)
    }

    /// [mode, strength, broadband noise-floor dBFS, low Hz, high Hz, mix, listen].
    /// Modes: cleanup, sustained/harmonic, transient/percussive, center, sides.
    func configure(_ v: [Float]) {
        func value(_ i: Int, _ fallback: Float, _ low: Float, _ high: Float) -> Float {
            let x = i < v.count ? v[i] : fallback
            return x.isFinite ? min(high, max(low, x)) : fallback
        }
        mode = Int(value(0, 0, 0, 4).rounded())
        strength = value(1, 0.75, 0, 1)
        // sqrt-Hann window energy is N/2; detector averages channel power.
        floorPower = pow(Float(10), value(2, -55, -90, -20) / 10) * Float(Self.size) * 0.5
        low = value(3, 20, 20, 12000)
        high = max(low, value(4, 20000, 200, 20000))
        mix = value(5, 1, 0, 1)
        listen = value(6, 0, 0, 1) >= 0.5
        if !initializedControls {
            for i in 0..<Self.size {
                mixHistory[i] = mix
                strengthHistory[i] = strength
                listenHistory[i] = listen
            }
            initializedControls = true
        }
    }

    func process(left l: Float, right r: Float) -> (Float, Float) {
        lastDryLeft = inputL[inputIndex]
        lastDryRight = inputR[inputIndex]
        let delayedMix = mixHistory[inputIndex]
        let delayedStrength = strengthHistory[inputIndex]
        let delayedListen = listenHistory[inputIndex]
        // Keep squared FFT energies finite even for corrupt finite Float input.
        inputL[inputIndex] = l.isFinite ? max(-16, min(16, l)) : 0
        inputR[inputIndex] = r.isFinite ? max(-16, min(16, r)) : 0
        mixHistory[inputIndex] = mix
        strengthHistory[inputIndex] = strength
        listenHistory[inputIndex] = listen
        inputIndex = (inputIndex + 1) % Self.size
        let wetL = outputL[outputIndex]
        let wetR = outputR[outputIndex]
        outputL[outputIndex] = 0
        outputR[outputIndex] = 0
        outputIndex = (outputIndex + 1) % Self.ringSize
        hopClock += 1
        if hopClock == Self.hop {
            analyze()
            hopClock = 0
        }
        if delayedMix == 0 || delayedStrength == 0 {
            return delayedListen ? (0, 0) : (lastDryLeft, lastDryRight)
        }
        if delayedListen { return ((lastDryLeft - wetL) * delayedMix, (lastDryRight - wetR) * delayedMix) }
        return (lastDryLeft + (wetL - lastDryLeft) * delayedMix, lastDryRight + (wetR - lastDryRight) * delayedMix)
    }

    private func transformInput(_ source: [Float], into realOutput: inout [Float], imaginaryOutput: inout [Float]) {
        for i in 0..<Self.size {
            real[i] = source[(inputIndex + i) % Self.size] * window[i]
            imaginary[i] = 0
        }
        real.withUnsafeBufferPointer { a in
            imaginary.withUnsafeBufferPointer { b in
                realOutput.withUnsafeMutableBufferPointer { c in
                    imaginaryOutput.withUnsafeMutableBufferPointer { d in
                        vDSP_DFT_Execute(forward, a.baseAddress!, b.baseAddress!, c.baseAddress!, d.baseAddress!)
                    }
                }
            }
        }
    }
    private func synthesize(_ a: [Float], _ b: [Float], into output: inout [Float]) {
        a.withUnsafeBufferPointer { x in
            b.withUnsafeBufferPointer { y in
                real.withUnsafeMutableBufferPointer { z in
                    imaginary.withUnsafeMutableBufferPointer { w in
                        vDSP_DFT_Execute(inverse, x.baseAddress!, y.baseAddress!, z.baseAddress!, w.baseAddress!)
                    }
                }
            }
        }
        for i in 0..<Self.size {
            output[(outputIndex + i) % Self.ringSize] += real[i] * window[i] / Float(Self.size * 2)
        }
    }
    private static func median(_ values: inout [Float]) -> Float {
        for i in 1..<values.count {
            let value = values[i]
            var j = i
            while j > 0 && values[j - 1] > value {
                values[j] = values[j - 1]
                j -= 1
            }
            values[j] = value
        }
        return values[values.count / 2]
    }
    private func analyze() {
        transformInput(inputL, into: &spectrumL, imaginaryOutput: &imagL)
        transformInput(inputR, into: &spectrumR, imaginaryOutput: &imagR)
        for k in 0..<Self.bins {
            let power =
                0.5
                * (spectrumL[k] * spectrumL[k] + imagL[k] * imagL[k] + spectrumR[k] * spectrumR[k] + imagR[k] * imagR[k])
            levels[k] = sqrt(max(0, power))
            history[historyIndex * Self.bins + k] = levels[k]
        }
        reductionDB = 0
        display = SpectralReductionSnapshot()
        for k in 0..<Self.bins {
            let hz = Float(k) * Float(sessionRate) / Float(Self.size)
            let shoulder = max(low * 0.15, Float(sessionRate) / Float(Self.size) * 2)
            let upperShoulder = max(high * 0.1, Float(sessionRate) / Float(Self.size) * 2)
            let focus = min(1, max(0, (hz - low) / shoulder), max(0, (high - hz) / upperShoulder))
            let target: Float
            if mode == 0 {
                let power = levels[k] * levels[k]
                let cleaned = sqrt(min(1, max(0, 1 - floorPower / max(1e-20, power))))
                target = 1 - focus * (1 - cleaned)
            } else if mode == 1 || mode == 2 {
                for i in temporal.indices { temporal[i] = history[i * Self.bins + k] }
                let harmonic = Self.median(&temporal)
                for i in frequency.indices { frequency[i] = levels[min(Self.bins - 1, max(0, k + i - 8))] }
                let percussive = Self.median(&frequency)
                let h2 = harmonic * harmonic
                let p2 = percussive * percussive
                let harmonicMask = h2 + p2 > 1e-20 ? h2 / (h2 + p2) : 0.5
                target = focus * (mode == 1 ? harmonicMask : 1 - harmonicMask)
            } else {
                target = focus
            }
            if mode == 0 {
                masks[k] += (target > masks[k] ? 0.35 : 0.8) * (target - masks[k])
            } else {
                masks[k] = target
            }
            gains[k] = 1 - strength * (1 - masks[k])
            let measuredGain: Float
            if mode >= 3 && levels[k] > 1e-8 {
                let sideSign: Float = mode == 3 ? 1 : -1
                let weight = strength * masks[k]
                let dry = 1 - strength
                let re = 0.5 * (spectrumL[k] + sideSign * spectrumR[k])
                let im = 0.5 * (imagL[k] + sideSign * imagR[k])
                let reL = dry * spectrumL[k] + weight * re
                let reR = dry * spectrumR[k] + sideSign * weight * re
                let imL = dry * imagL[k] + weight * im
                let imR = dry * imagR[k] + sideSign * weight * im
                let power = 0.5 * (reL * reL + reR * reR + imL * imL + imR * imR)
                measuredGain = min(1, max(0, sqrt(max(0, power)) / levels[k]))
            } else {
                measuredGain = gains[k]
            }
            let reduction: Float = levels[k] > 0.001 ? -20 * log10(max(1e-6, measuredGain)) : 0
            reductionDB = max(reductionDB, reduction)
            let band = min(11, max(0, Int(log2(max(80, hz) / 80) / log2(20000.0 / 80) * 12)))
            display.set(band, max(displayValue(band), reduction))
        }
        historyIndex = (historyIndex + 1) % Self.historyFrames
        for k in 0..<Self.bins {
            applyBin(k, mask: k)
            if k > 0 && k < Self.size / 2 { applyBin(Self.size - k, mask: k) }
        }
        synthesize(spectrumL, imagL, into: &outputL)
        synthesize(spectrumR, imagR, into: &outputR)
    }
    private func displayValue(_ band: Int) -> Float {
        // Avoid allocating the snapshot's convenience `bands` array on callback.
        switch band {
        case 0: return display.b0
        case 1: return display.b1
        case 2: return display.b2
        case 3: return display.b3
        case 4: return display.b4
        case 5: return display.b5
        case 6: return display.b6
        case 7: return display.b7
        case 8: return display.b8
        case 9: return display.b9
        case 10: return display.b10
        default: return display.b11
        }
    }
    private func applyBin(_ bin: Int, mask k: Int) {
        if mode < 3 {
            spectrumL[bin] *= gains[k]
            imagL[bin] *= gains[k]
            spectrumR[bin] *= gains[k]
            imagR[bin] *= gains[k]
        } else {
            let weight = strength * masks[k]
            let dry = 1 - strength
            let sideSign: Float = mode == 3 ? 1 : -1
            let re = 0.5 * (spectrumL[bin] + sideSign * spectrumR[bin])
            let im = 0.5 * (imagL[bin] + sideSign * imagR[bin])
            spectrumL[bin] = dry * spectrumL[bin] + weight * re
            spectrumR[bin] = dry * spectrumR[bin] + sideSign * weight * re
            imagL[bin] = dry * imagL[bin] + weight * im
            imagR[bin] = dry * imagR[bin] + sideSign * weight * im
        }
    }

    func deepCopy() -> SourceIsolatorDSP {
        let c = SourceIsolatorDSP()
        c.inputL = inputL.map { $0 }
        c.inputR = inputR.map { $0 }
        c.outputL = outputL.map { $0 }
        c.outputR = outputR.map { $0 }
        c.real = real.map { $0 }
        c.imaginary = imaginary.map { $0 }
        c.spectrumL = spectrumL.map { $0 }
        c.spectrumR = spectrumR.map { $0 }
        c.imagL = imagL.map { $0 }
        c.imagR = imagR.map { $0 }
        c.levels = levels.map { $0 }
        c.history = history.map { $0 }
        c.masks = masks.map { $0 }
        c.gains = gains.map { $0 }
        c.mixHistory = mixHistory.map { $0 }
        c.strengthHistory = strengthHistory.map { $0 }
        c.listenHistory = listenHistory.map { $0 }
        c.temporal = temporal.map { $0 }
        c.frequency = frequency.map { $0 }
        c.inputIndex = inputIndex
        c.outputIndex = outputIndex
        c.hopClock = hopClock
        c.historyIndex = historyIndex
        c.initializedControls = initializedControls
        c.mode = mode
        c.strength = strength
        c.floorPower = floorPower
        c.low = low
        c.high = high
        c.mix = mix
        c.listen = listen
        c.lastDryLeft = lastDryLeft
        c.lastDryRight = lastDryRight
        c.reductionDB = reductionDB
        c.display = display
        return c
    }
}
