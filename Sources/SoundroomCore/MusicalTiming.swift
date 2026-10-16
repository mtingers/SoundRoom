import Foundation

public enum TimelineTimebase: String, Codable, CaseIterable, Sendable { case time = "Time", beats = "Beats" }

/// Constant-tempo musical coordinates. BPM always counts quarter notes; the meter
/// determines bar lengths and displayed beat units. Audio stays in sample frames.
public struct MusicalTiming: Codable, Equatable, Sendable {
    public var tempo: Double
    public var beatsPerBar: Int
    public var beatUnit: Int
    public static let standard = MusicalTiming()
    public static let beatUnits = [2, 4, 8, 16]
    public init(tempo: Double = 120, beatsPerBar: Int = 4, beatUnit: Int = 4) {
        self.tempo = tempo; self.beatsPerBar = beatsPerBar; self.beatUnit = beatUnit
    }
    public var isValid: Bool { tempo.isFinite && (20...300).contains(tempo) && (1...16).contains(beatsPerBar) && Self.beatUnits.contains(beatUnit) }
    public var valid: MusicalTiming { isValid ? self : .standard }
    public var quarterNotesPerBeat: Double { 4 / Double(valid.beatUnit) }
    public var quarterNotesPerBar: Double { Double(valid.beatsPerBar) * quarterNotesPerBeat }
    public var meter: String { "\(valid.beatsPerBar)/\(valid.beatUnit)" }
    public func quarterNotes(at frame: Int, sampleRate: Double = 48_000) -> Double {
        Double(max(0, frame)) / (sampleRate.isFinite && sampleRate > 0 ? sampleRate : 48_000) * valid.tempo / 60
    }
    public func frame(atQuarterNote beat: Double, sampleRate: Double = 48_000) -> Int {
        guard beat.isFinite else { return 0 }
        let rate = sampleRate.isFinite && sampleRate > 0 ? sampleRate : 48_000
        return Int(min(Double(Int.max / 4), max(0, beat) * 60 / valid.tempo * rate).rounded())
    }
    public func snapped(_ frame: Int, quarterNoteDivision: Double, sampleRate: Double = 48_000) -> Int {
        guard quarterNoteDivision.isFinite && quarterNoteDivision > 0 else { return max(0, frame) }
        let beat = (quarterNotes(at: frame, sampleRate: sampleRate) / quarterNoteDivision).rounded() * quarterNoteDivision
        // Compute each absolute boundary independently. Rounding an interval and
        // repeatedly adding it drifts at tempos such as 123 BPM.
        return self.frame(atQuarterNote: beat, sampleRate: sampleRate)
    }
    public func position(_ frame: Int, sampleRate: Double = 48_000, ticks: Bool = true) -> String {
        let q = quarterNotes(at: frame, sampleRate: sampleRate)
        let totalTicks = Int(min(Double(Int.max / 4), (q * 960).rounded()))
        let ticksPerBeat = Int(quarterNotesPerBeat * 960)
        let ticksPerBar = valid.beatsPerBar * ticksPerBeat
        let bar = totalTicks / ticksPerBar
        let beat = (totalTicks % ticksPerBar) / ticksPerBeat
        let tick = totalTicks % ticksPerBeat
        return ticks ? String(format: "%d.%d.%03d", bar + 1, beat + 1, tick) : "\(bar + 1).\(beat + 1)"
    }
    /// Parse bar.beat[.tick]; bar and beat are one-based, ticks are 960 per quarter note.
    public func frame(atPosition text: String, sampleRate: Double = 48_000) -> Int? {
        let parts = text.split(separator: ".", omittingEmptySubsequences: false)
        guard (2...3).contains(parts.count), let bar = Int(parts[0]), let beat = Int(parts[1]), bar >= 1, bar <= 1000000, beat >= 1, beat <= valid.beatsPerBar else { return nil }
        let tick = parts.count == 3 ? Int(parts[2]) : 0
        guard let tick, tick >= 0, tick < Int(quarterNotesPerBeat * 960) else { return nil }
        return frame(atQuarterNote: Double(bar - 1) * quarterNotesPerBar + Double(beat - 1) * quarterNotesPerBeat + Double(tick) / 960, sampleRate: sampleRate)
    }
    /// Viewport-only musical ticks, at most a few hundred even on very long projects.
    public func gridFrames(start: Int, end: Int, quarterNoteDivision: Double, pixelsPerSecond: Double, minimumPixels: Double = 8, sampleRate: Double = 48_000) -> [Int] {
        guard end > start, quarterNoteDivision.isFinite, quarterNoteDivision > 0, pixelsPerSecond.isFinite,
              quarterNoteDivision * 60 / valid.tempo * pixelsPerSecond >= minimumPixels else { return [] }
        let first = floor(quarterNotes(at: start, sampleRate: sampleRate) / quarterNoteDivision)
        let last = ceil(quarterNotes(at: end, sampleRate: sampleRate) / quarterNoteDivision)
        guard first.isFinite, last.isFinite else { return [] }
        let count = Int(min(512, max(0, last - first + 1)))
        return (0..<count).map { frame(atQuarterNote: (first + Double($0)) * quarterNoteDivision, sampleRate: sampleRate) }
    }
    /// Viewport-only musical ticks, at most a few hundred even on very long projects.
    public func rulerTicks(start: Int, end: Int, pixelsPerSecond: Double, minimumPixels: Double = 64, sampleRate: Double = 48_000) -> [(frame: Int, label: String, bar: Bool)] {
        guard end > start, pixelsPerSecond.isFinite, pixelsPerSecond > 0 else { return [] }
        let beatPixels = quarterNotesPerBeat * 60 / valid.tempo * pixelsPerSecond
        var step = quarterNotesPerBeat
        if beatPixels < minimumPixels {
            step = quarterNotesPerBar
            while step * 60 / valid.tempo * pixelsPerSecond < minimumPixels { step *= 2 }
        }
        let first = floor(quarterNotes(at: start, sampleRate: sampleRate) / step)
        let last = ceil(quarterNotes(at: end, sampleRate: sampleRate) / step)
        guard first.isFinite && last.isFinite else { return [] }
        let count = min(512, max(0, Int(min(512, last - first + 1))))
        return (0..<count).map { i in
            let q = (first + Double(i)) * step
            let f = frame(atQuarterNote: q, sampleRate: sampleRate)
            let isBar = abs(q / quarterNotesPerBar - (q / quarterNotesPerBar).rounded()) < 1e-7
            return (f, position(f, sampleRate: sampleRate, ticks: false), isBar)
        }
    }
}
