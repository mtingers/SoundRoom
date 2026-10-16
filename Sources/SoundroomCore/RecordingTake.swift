import Foundation

/// Source clips retained independently of the track's playable comp. Only `clips`
/// on Track enter the mixer; takes never sum together or change exported audio.
public struct RecordingTake: Codable, Equatable, Identifiable, Sendable {
    public var id: UUID
    public var name: String
    public var clips: [Clip]
    public init(id: UUID = UUID(), name: String, clips: [Clip]) {
        self.id = id; self.name = name; self.clips = clips
    }
    public var range: Range<Int>? {
        guard let first = clips.map(\.start).min(), let last = clips.map(\.end).max(), first < last else { return nil }
        return first..<last
    }
}

public struct RecordingPlan: Equatable, Sendable {
    public let targetStart: Int
    public let targetEnd: Int?
    public let playbackStart: Int
    public let countInFrames: Int
    public var inputLeadFrames: Int { countInFrames + targetStart - playbackStart }
    public var punchFrameCount: Int? { targetEnd.map { max(0, $0 - targetStart) } }
    public init(playhead: Int, punch: Range<Int>? = nil, preRollSeconds: Double = 0, countInBars: Int = 0, tempo: Double = 120, beatsPerBar: Int = 4, beatUnit: Int = 4) {
        targetStart = max(0, punch?.lowerBound ?? playhead)
        targetEnd = punch.flatMap { $0.isEmpty ? nil : $0.upperBound }
        let tempo = tempo.isFinite ? min(300, max(20, tempo)) : 120
        let preRoll = preRollSeconds.isFinite ? min(30, max(0, preRollSeconds)) : 0
        playbackStart = max(0, targetStart - Int(preRoll * sessionRate))
        countInFrames = Int((Double(min(4, max(0, countInBars))) * MusicalTiming(tempo: tempo, beatsPerBar: beatsPerBar, beatUnit: beatUnit).valid.quarterNotesPerBar * 60 / tempo * sessionRate).rounded())
    }
    public func elapsedCaptureFrames(transportElapsedFrames: Int) -> Int {
        let elapsed = max(0, transportElapsedFrames - inputLeadFrames)
        return targetEnd.map { min(elapsed, max(0, $0 - targetStart)) } ?? elapsed
    }
    public func timelineFrame(renderedFrame: Int) -> Int { max(playbackStart, renderedFrame - countInFrames) }
    /// Output rendering can lead the input device by a buffer or more. Never
    /// stop capture until actual input covers the whole exclusive punch range.
    public func shouldCompletePunch(renderedFrame: Int, capturedTimelineFrames: Int) -> Bool {
        guard let targetEnd, let punchFrameCount else { return false }
        return timelineFrame(renderedFrame: renderedFrame) >= targetEnd && capturedTimelineFrames >= punchFrameCount
    }
    public func punchCompletionTimedOut(transportElapsedFrames: Int, graceFrames: Int = Int(sessionRate * 2)) -> Bool {
        guard let punchFrameCount else { return false }
        let elapsed = max(0, transportElapsedFrames - inputLeadFrames)
        return elapsed >= punchFrameCount && elapsed - punchFrameCount >= max(0, graceFrames)
    }
}

/// A quiet synthesized cue, independent of project assets and exports. Sample
/// math keeps tempo and envelope independent of device callback partitioning.
public struct RecordingMetronome: Sendable {
    public var tempo: Double
    public var beatsPerBar: Int
    public var beatUnit: Int
    public var enabled: Bool
    public var countInStart: Int
    public var countInEnd: Int
    public init(tempo: Double = 120, enabled: Bool = false, countInStart: Int = 0, countInEnd: Int = 0, beatsPerBar: Int = 4, beatUnit: Int = 4) {
        self.tempo = tempo.isFinite ? min(300, max(20, tempo)) : 120
        let timing = MusicalTiming(tempo: self.tempo, beatsPerBar: beatsPerBar, beatUnit: beatUnit).valid
        self.beatsPerBar = timing.beatsPerBar; self.beatUnit = timing.beatUnit
        self.enabled = enabled; self.countInStart = countInStart; self.countInEnd = countInEnd
    }
    public func sample(at frame: Int) -> Float {
        let inCount = frame >= countInStart && frame < countInEnd
        guard enabled || inCount, frame >= 0 else { return 0 }
        let beatFrames = 60 / tempo * 4 / Double(beatUnit) * sessionRate
        // Playback clips are shifted by the count-in duration. Afterwards the
        // cue follows the project's beat/bar phase, including a mid-bar start.
        let origin = inCount ? countInStart : max(0, countInEnd - countInStart)
        let local = Double(frame - origin)
        guard local >= 0 else { return 0 }
        let beat = Int((local / beatFrames).rounded(.down))
        let within = local - Double(beat) * beatFrames
        let duration = sessionRate * 0.025
        guard within < duration else { return 0 }
        let phase = within / sessionRate
        let attack = min(1, within / (sessionRate * 0.001))
        let release = pow(max(0, 1 - within / duration), 3)
        let frequency = beat % beatsPerBar == 0 ? 1600.0 : 1100.0
        return Float(sin(2 * .pi * frequency * phase) * attack * release * 0.15)
    }
}
