import Foundation

public struct PitchContourPoint: Equatable, Sendable {
    public let frame: Int
    public let frequency: Float
    public let confidence: Float
    public var isVoiced: Bool { (50...1000).contains(frequency) && confidence > 0.75 }
    public init(frame: Int, frequency: Float, confidence: Float) {
        self.frame = frame
        self.frequency = frequency
        self.confidence = confidence
    }
}

/// Worker-only analysis of a rendered waveform. Uses the input detector, never
/// correction commands/target notes, so before/after charts measure each signal.
public enum PitchContourAnalysis {
    public static func analyze(_ audio: AudioSamples, maximumPoints: Int = 1024) throws -> [PitchContourPoint] {
        guard audio.count > 0 else { return [] }
        let detector = PitchCorrectionState()
        var controls = PitchCorrectionParameters()
        controls.mix = 0
        controls.strength = 0
        controls.formant = 0
        // Bound chart complexity without changing the detector's audio cadence.
        let pointLimit = max(1, min(65_536, maximumPoints))
        let hop = max(256, Int(ceil(Double(audio.count) / Double(pointLimit) / 256)) * 256)
        var result: [PitchContourPoint] = []
        result.reserveCapacity(min(pointLimit + 1, audio.count / hop + 1))
        for index in 0..<(audio.count + 1024) {
            if index % 8192 == 0 { try Task.checkCancellation() }
            let left = audio.sample(channel: 0, frame: index)
            let right = audio.sample(channel: min(1, max(0, audio.channelCount - 1)), frame: index)
            _ = detector.process(left: left, right: right, parameters: controls)
            let sourceFrame = index - 1024
            // Decisions arrive at frame 2044, 2300, ... and describe windows
            // centred 1024 frames earlier. Keep their actual timing in the plot.
            if sourceFrame >= 0, sourceFrame < audio.count, (index + 4) % hop == 0 {
                result.append(
                    PitchContourPoint(
                        frame: sourceFrame, frequency: detector.detectedFrequency, confidence: detector.confidence))
            }
        }
        return result
    }
}
