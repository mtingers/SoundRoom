import Foundation

/// Eight mutually coupled delay lines with an orthogonal feedback matrix. All
/// buffers are allocated off the callback; parameter updates never resize them.
final class StereoRoomDSP {
    private var lines = [1499, 1601, 1867, 2053, 2251, 2399, 2791, 3163].map { [Float](repeating: 0, count: $0) }
    private var positions = [Int](repeating: 0, count: 8)
    private var filtered = [Float](repeating: 0, count: 8)
    private var feedback = [Float](repeating: 0, count: 8)
    private var scratch = [Float](repeating: 0, count: 8)
    private var diffusionLines = [211, 397, 683, 227, 419, 719].map { [Float](repeating: 0, count: $0) }
    private var diffusionPositions = [Int](repeating: 0, count: 6)
    private var earlyL = [Float](repeating: 0, count: 3200)
    private var earlyR = [Float](repeating: 0, count: 3200)
    private var earlyPosition = 0
    private var lowL: Float = 0, lowR: Float = 0
    private var tone: Float = 0, lowCut: Float = 0, diffusion: Float = 0
    private var width: Float = 1, earlyBalance: Float = 0
    private(set) var outputGain: Float = 1
    private var algorithmBlend: Float = 0, algorithmTarget: Float = 0
    private var configured = false
    private var previousDecay: Float = -1, previousTone: Float = -1, previousCut: Float = -1
    private static let leftTaps = [173, 431, 809, 1237, 1861, 2741]
    private static let rightTaps = [239, 557, 947, 1459, 2081, 3011]
    private static let tapGains: [Float] = [0.38, -0.31, 0.25, 0.20, -0.15, 0.11]

    func configure(_ values: [Float]) {
        if values[0] != previousDecay {
            for i in feedback.indices {
                feedback[i] = pow(10, -3 * Float(lines[i].count) / (Float(sessionRate) * values[0]))
            }
            previousDecay = values[0]
        }
        if values[2] != previousTone {
            tone = 1 - exp(-2 * .pi * values[2] / Float(sessionRate))
            previousTone = values[2]
        }
        if values[5] != previousCut {
            lowCut = values[5] == 0 ? 0 : 1 - exp(-2 * .pi * values[5] / Float(sessionRate))
            previousCut = values[5]
        }
        diffusion = 0.35 + values[6] * 0.4
        width = values[7]
        earlyBalance = values[8]
        outputGain = pow(10, values[9] / 20)
        algorithmTarget = values[4]
        if !configured {
            algorithmBlend = algorithmTarget
            configured = true
        }
    }

    func process(left: Float, right: Float, classicLeft: Float, classicRight: Float) -> (Float, Float) {
        // Warm both engines so changing Character during playback can crossfade
        // continuous tails rather than replacing a live network with silence.
        lowL += lowCut * (left - lowL)
        lowR += lowCut * (right - lowR)
        let inputL = lowCut == 0 ? left : left - lowL
        let inputR = lowCut == 0 ? right : right - lowR
        earlyL[earlyPosition] = inputL
        earlyR[earlyPosition] = inputR
        var reflectionL: Float = 0
        var reflectionR: Float = 0
        for i in 0..<6 {
            let l = (earlyPosition - Self.leftTaps[i] + earlyL.count) % earlyL.count
            let r = (earlyPosition - Self.rightTaps[i] + earlyL.count) % earlyL.count
            reflectionL += (earlyL[l] * 0.8 + earlyR[r] * 0.2) * Self.tapGains[i]
            reflectionR += (earlyR[r] * 0.8 + earlyL[l] * 0.2) * Self.tapGains[i]
        }
        earlyPosition = (earlyPosition + 1) % earlyL.count
        var diffuseL = inputL
        var diffuseR = inputR
        for i in 0..<6 {
            let input = i < 3 ? diffuseL : diffuseR
            let delayed = diffusionLines[i][diffusionPositions[i]]
            let result = delayed - diffusion * input
            diffusionLines[i][diffusionPositions[i]] = input + diffusion * result
            diffusionPositions[i] = (diffusionPositions[i] + 1) % diffusionLines[i].count
            if i < 3 { diffuseL = result } else { diffuseR = result }
        }
        for i in 0..<8 {
            let delayed = lines[i][positions[i]]
            filtered[i] += tone * (delayed - filtered[i])
            scratch[i] = filtered[i] * feedback[i]
        }
        // Different orthogonal projections preserve stereo input while giving
        // even a hard-panned source a spatially decorrelated stereo tail.
        let lateL =
            (scratch[0] + scratch[1] - scratch[2] - scratch[3] + scratch[4] + scratch[5] - scratch[6] - scratch[7])
            * 0.35355339
        let lateR =
            (scratch[0] - scratch[1] + scratch[2] - scratch[3] + scratch[4] - scratch[5] + scratch[6] - scratch[7])
            * 0.35355339
        var stride = 1
        while stride < 8 {
            var base = 0
            while base < 8 {
                for j in 0..<stride {
                    let a = scratch[base + j]
                    let b = scratch[base + j + stride]
                    scratch[base + j] = a + b
                    scratch[base + j + stride] = a - b
                }
                base += stride * 2
            }
            stride *= 2
        }
        for i in 0..<8 {
            let signL: Float = (i & 2) == 0 ? 1 : -1
            let signR: Float = (i & 1) == 0 ? 1 : -1
            lines[i][positions[i]] = scratch[i] * 0.35355339 + (diffuseL * signL + diffuseR * signR) * 0.25
            positions[i] = (positions[i] + 1) % lines[i].count
        }
        let wetL = reflectionL * earlyBalance + lateL * (1 - earlyBalance)
        let wetR = reflectionR * earlyBalance + lateR * (1 - earlyBalance)
        let mid = (wetL + wetR) * 0.5
        let side = (wetL - wetR) * 0.5 * width
        if algorithmBlend < algorithmTarget {
            algorithmBlend = min(algorithmTarget, algorithmBlend + 1 / 240)
        } else if algorithmBlend > algorithmTarget {
            algorithmBlend = max(algorithmTarget, algorithmBlend - 1 / 240)
        }
        return (
            (classicLeft * (1 - algorithmBlend) + (mid + side) * algorithmBlend),
            (classicRight * (1 - algorithmBlend) + (mid - side) * algorithmBlend)
        )
    }

    var storageBytes: Int {
        (lines.reduce(0) { $0 + $1.count } + diffusionLines.reduce(0) { $0 + $1.count } + earlyL.count + earlyR.count)
            * 4 + 1024
    }
    func deepCopy() -> StereoRoomDSP {
        func floats(_ source: [Float]) -> [Float] { source.withUnsafeBufferPointer { Array($0) } }
        func ints(_ source: [Int]) -> [Int] { source.withUnsafeBufferPointer { Array($0) } }
        let copy = StereoRoomDSP()
        copy.lines = lines.map(floats)
        copy.positions = ints(positions)
        copy.filtered = floats(filtered)
        copy.feedback = floats(feedback)
        copy.scratch = floats(scratch)
        copy.diffusionLines = diffusionLines.map(floats)
        copy.diffusionPositions = ints(diffusionPositions)
        copy.earlyL = floats(earlyL)
        copy.earlyR = floats(earlyR)
        copy.earlyPosition = earlyPosition
        copy.lowL = lowL
        copy.lowR = lowR
        copy.tone = tone
        copy.lowCut = lowCut
        copy.diffusion = diffusion
        copy.width = width
        copy.earlyBalance = earlyBalance
        copy.outputGain = outputGain
        copy.algorithmBlend = algorithmBlend
        copy.algorithmTarget = algorithmTarget
        copy.configured = configured
        copy.previousDecay = previousDecay
        copy.previousTone = previousTone
        copy.previousCut = previousCut
        return copy
    }
}
