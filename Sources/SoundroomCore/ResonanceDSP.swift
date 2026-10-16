import Foundation
import Accelerate

/// Fixed-size callback telemetry. UI reads the values without sharing DSP arrays.
public struct SpectralReductionSnapshot: Equatable, Codable, Sendable {
    public var b0: Float = 0, b1: Float = 0, b2: Float = 0, b3: Float = 0
    public var b4: Float = 0, b5: Float = 0, b6: Float = 0, b7: Float = 0
    public var b8: Float = 0, b9: Float = 0, b10: Float = 0, b11: Float = 0
    public init() {}
    public var bands: [Float] { [b0,b1,b2,b3,b4,b5,b6,b7,b8,b9,b10,b11] }
    mutating func set(_ i: Int, _ v: Float) {
        switch i { case 0: b0 = v; case 1: b1 = v; case 2: b2 = v; case 3: b3 = v; case 4: b4 = v; case 5: b5 = v; case 6: b6 = v; case 7: b7 = v; case 8: b8 = v; case 9: b9 = v; case 10: b10 = v; default: b11 = v }
    }
}

/// 2048-point complex FFT, square-root Hann, four-way overlap-add. The linked
/// detector compares each bin with its local spectral neighborhood; gain follows
/// attack/release envelopes, with flux-based transient protection. It does not
/// classify unwanted resonances or attempt to reconstruct a commercial algorithm.
final class ResonanceDSP {
    static let latencyFrames = 2048
    private static let hop = 512, size = 2048, ringSize = 4096
    private let forward: vDSP_DFT_Setup, inverse: vDSP_DFT_Setup
    private var window: [Float]
    private var inputL = [Float](repeating: 0, count: size), inputR = [Float](repeating: 0, count: size)
    private var outputL = [Float](repeating: 0, count: ringSize), outputR = [Float](repeating: 0, count: ringSize)
    private var real = [Float](repeating: 0, count: size), imaginary = [Float](repeating: 0, count: size)
    private var spectrumL = [Float](repeating: 0, count: size), imagL = [Float](repeating: 0, count: size)
    private var spectrumR = [Float](repeating: 0, count: size), imagR = [Float](repeating: 0, count: size)
    private var levels = [Float](repeating: -120, count: size / 2 + 1)
    private var prefix = [Float](repeating: 0, count: size / 2 + 2)
    private var reductions = [Float](repeating: 0, count: size / 2 + 1)
    private var mixHistory = [Float](repeating: 1, count: size), deltaHistory = [Bool](repeating: false, count: size)
    private var inputIndex = 0, outputIndex = 0, hopClock = 0
    private var initializedControls = false
    private var depth: Float = 6, threshold: Float = 10.5, sharpness: Float = 0.6
    private var low: Float = 500, high: Float = 16000, attack: Float = 0.6, release: Float = 0.07
    private var preserve: Float = 0.7, mix: Float = 1, delta = false, previousEnergy: Float = 0
    private(set) var lastDryLeft: Float = 0, lastDryRight: Float = 0, reductionDB: Float = 0
    private(set) var display = SpectralReductionSnapshot()
    var storageBytes: Int { (Self.size * 10 + Self.ringSize * 2 + levels.count * 3) * 4 + Self.size + 256 }
    init() {
        forward = vDSP_DFT_zop_CreateSetup(nil, vDSP_Length(Self.size), .FORWARD)!
        inverse = vDSP_DFT_zop_CreateSetup(nil, vDSP_Length(Self.size), .INVERSE)!
        window = (0..<Self.size).map { sqrt(0.5 - 0.5 * cos(2 * Float.pi * Float($0) / Float(Self.size))) }
    }
    deinit { vDSP_DFT_DestroySetup(forward); vDSP_DFT_DestroySetup(inverse) }
    func configure(_ v: [Float]) {
        depth = v[0]; threshold = 18 - 15 * v[1]; sharpness = v[2]; low = v[3]; high = max(low, v[4])
        attack = 1 - exp(-Float(Self.hop) / (v[5] * 0.001 * Float(sessionRate)))
        release = 1 - exp(-Float(Self.hop) / (v[6] * 0.001 * Float(sessionRate)))
        preserve = v[7]; mix = v[8]; delta = v[9] >= 0.5
        if !initializedControls {
            for i in mixHistory.indices { mixHistory[i] = mix; deltaHistory[i] = delta }
            initializedControls = true
        }
    }
    func process(left l: Float, right r: Float) -> (Float, Float) {
        lastDryLeft = inputL[inputIndex]; lastDryRight = inputR[inputIndex]
        let delayedMix = mixHistory[inputIndex], delayedDelta = deltaHistory[inputIndex]
        inputL[inputIndex] = l; inputR[inputIndex] = r; mixHistory[inputIndex] = mix; deltaHistory[inputIndex] = delta
        inputIndex = (inputIndex + 1) % Self.size
        let wetL = outputL[outputIndex], wetR = outputR[outputIndex]
        outputL[outputIndex] = 0; outputR[outputIndex] = 0
        outputIndex = (outputIndex + 1) % Self.ringSize
        hopClock += 1
        if hopClock == Self.hop { analyze(); hopClock = 0 }
        if delayedDelta { return ((lastDryLeft - wetL) * delayedMix, (lastDryRight - wetR) * delayedMix) }
        return (lastDryLeft + (wetL - lastDryLeft) * delayedMix, lastDryRight + (wetR - lastDryRight) * delayedMix)
    }
    private func transformInput(_ source: [Float], into realOutput: inout [Float], imaginaryOutput: inout [Float]) {
        for i in 0..<Self.size { real[i] = source[(inputIndex + i) % Self.size] * window[i]; imaginary[i] = 0 }
        real.withUnsafeBufferPointer { a in imaginary.withUnsafeBufferPointer { b in realOutput.withUnsafeMutableBufferPointer { c in imaginaryOutput.withUnsafeMutableBufferPointer { d in
            vDSP_DFT_Execute(forward, a.baseAddress!, b.baseAddress!, c.baseAddress!, d.baseAddress!)
        } } } }
    }
    private func synthesize(_ a: inout [Float], _ b: inout [Float], into output: inout [Float]) {
        for k in 0...Self.size / 2 {
            let gain = pow(Float(10), -reductions[k] / 20)
            a[k] *= gain; b[k] *= gain
            if k > 0 && k < Self.size / 2 { a[Self.size - k] *= gain; b[Self.size - k] *= gain }
        }
        a.withUnsafeBufferPointer { x in b.withUnsafeBufferPointer { y in real.withUnsafeMutableBufferPointer { z in imaginary.withUnsafeMutableBufferPointer { w in
            vDSP_DFT_Execute(inverse, x.baseAddress!, y.baseAddress!, z.baseAddress!, w.baseAddress!)
        } } } }
        for i in 0..<Self.size { output[(outputIndex + i) % Self.ringSize] += real[i] * window[i] / Float(Self.size * 2) }
    }
    private func analyze() {
        transformInput(inputL, into: &spectrumL, imaginaryOutput: &imagL)
        transformInput(inputR, into: &spectrumR, imaginaryOutput: &imagR)
        var energy: Float = 0
        prefix[0] = 0
        for k in levels.indices {
            let power = spectrumL[k] * spectrumL[k] + imagL[k] * imagL[k] + spectrumR[k] * spectrumR[k] + imagR[k] * imagR[k]
            levels[k] = 10 * log10(max(1e-12, power / Float(Self.size * Self.size)))
            prefix[k + 1] = prefix[k] + levels[k]; energy += power
        }
        let flux = previousEnergy > 1e-8 ? max(0, min(1, (energy / previousEnergy - 1) / 3)) : 1
        previousEnergy += 0.25 * (energy - previousEnergy)
        reductionDB = 0; display = SpectralReductionSnapshot()
        for k in levels.indices {
            let hz = Float(k) * Float(sessionRate) / Float(Self.size)
            let radius = max(4, Int(Float(k) * (0.04 + 0.12 * sharpness)))
            let lo = max(0, k - radius), hi = min(levels.count - 1, k + radius)
            let excludedLo = max(lo, k - 1), excludedHi = min(hi, k + 1)
            let neighbors = hi - lo + 1 - (excludedHi - excludedLo + 1)
            let baseline = neighbors > 0 ? (prefix[hi + 1] - prefix[lo] - prefix[excludedHi + 1] + prefix[excludedLo]) / Float(neighbors) : levels[k]
            let prominence = max(0, levels[k] - baseline - threshold)
            let edge = min(1, max(0, (hz - low) / max(80, low * 0.15)), max(0, (high - hz) / max(200, high * 0.15)))
            let target = levels[k] > -85 ? depth * (1 - exp(-prominence / 9)) * edge * (1 - preserve * flux) : 0
            reductions[k] += (target > reductions[k] ? attack : release) * (target - reductions[k])
            reductionDB = max(reductionDB, reductions[k])
            let band = min(11, max(0, Int(log2(max(80, hz) / 80) / log2(20000.0 / 80) * 12)))
            // Each displayed band shows its greatest attenuation, preserving
            // narrow resonances that would disappear in an averaged display.
            switch band {
            case 0: display.b0 = max(display.b0, reductions[k]); case 1: display.b1 = max(display.b1, reductions[k]); case 2: display.b2 = max(display.b2, reductions[k]); case 3: display.b3 = max(display.b3, reductions[k]); case 4: display.b4 = max(display.b4, reductions[k]); case 5: display.b5 = max(display.b5, reductions[k]); case 6: display.b6 = max(display.b6, reductions[k]); case 7: display.b7 = max(display.b7, reductions[k]); case 8: display.b8 = max(display.b8, reductions[k]); case 9: display.b9 = max(display.b9, reductions[k]); case 10: display.b10 = max(display.b10, reductions[k]); default: display.b11 = max(display.b11, reductions[k])
            }
        }
        synthesize(&spectrumL, &imagL, into: &outputL); synthesize(&spectrumR, &imagR, into: &outputR)
    }
    func deepCopy() -> ResonanceDSP {
        let c = ResonanceDSP()
        c.inputL = inputL.map { $0 }; c.inputR = inputR.map { $0 }; c.outputL = outputL.map { $0 }; c.outputR = outputR.map { $0 }
        c.real = real.map { $0 }; c.imaginary = imaginary.map { $0 }; c.spectrumL = spectrumL.map { $0 }; c.spectrumR = spectrumR.map { $0 }; c.imagL = imagL.map { $0 }; c.imagR = imagR.map { $0 }
        c.levels = levels.map { $0 }; c.prefix = prefix.map { $0 }; c.reductions = reductions.map { $0 }; c.mixHistory = mixHistory.map { $0 }; c.deltaHistory = deltaHistory.map { $0 }
        c.inputIndex = inputIndex; c.outputIndex = outputIndex; c.hopClock = hopClock; c.initializedControls = initializedControls
        c.depth = depth; c.threshold = threshold; c.sharpness = sharpness; c.low = low; c.high = high; c.attack = attack; c.release = release; c.preserve = preserve; c.mix = mix; c.delta = delta; c.previousEnergy = previousEnergy
        c.lastDryLeft = lastDryLeft; c.lastDryRight = lastDryRight; c.reductionDB = reductionDB; c.display = display; return c
    }
}
