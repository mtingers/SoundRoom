import Foundation
import SoundroomCore

/// UI-owned sampling of audio feedback. A stopped callback cannot leave a voiced
/// note displayed indefinitely, and a seek/loop cannot join unrelated trace ends.
struct PitchFeedbackHistory {
    struct Point: Equatable {
        let time: TimeInterval
        let pitch: PitchMeterSnapshot
    }
    private(set) var points: [Point] = []
    private(set) var reading: PitchMeterSnapshot?
    private(set) var scale = PitchTraceScale()
    var notes: [Float] { points.flatMap { Self.notes(for: $0.pitch) } }
    var hasNotesOutsideScale: Bool { notes.contains { !scale.contains($0) } }

    private static func notes(for pitch: PitchMeterSnapshot) -> [Float] {
        guard pitch.isVoiced else { return [] }
        return [pitch.inputFrequency, pitch.targetFrequency, pitch.estimatedWetFrequency]
            .compactMap { pitch.midiNote(for: $0) }
    }
    mutating func fitScale() { scale.fit(notes: notes) }
    mutating func zoomScale(_ factor: Float) { scale.zoom(factor) }
    mutating func moveScale(_ semitones: Float) { scale.move(semitones: semitones) }
    private var lastObserved: PitchMeterSnapshot?
    private var lastRevision: UInt64?
    private var lastFreshTime: TimeInterval?
    static let duration: TimeInterval = 8

    mutating func clear() {
        points.removeAll(keepingCapacity: true)
        reading = nil
        lastFreshTime = nil
        lastObserved = nil
        lastRevision = nil
    }

    mutating func update(_ snapshot: PitchMeterSnapshot?, at time: TimeInterval, running: Bool, revision: UInt64? = nil)
    {
        guard running, let snapshot else {
            clear()
            return
        }
        guard snapshot != lastObserved || (revision != nil && revision != lastRevision) else {
            if let lastFreshTime, time - lastFreshTime >= 0.35 {
                points.removeAll(keepingCapacity: true)
                reading = nil
            }
            return
        }
        if let last = points.last {
            let frames = snapshot.sourceFrame - last.pitch.sourceFrame
            let elapsed = max(0.08, time - last.time)
            if frames < 0 || Double(frames) > sessionRate * max(0.5, elapsed * 3) {
                points.removeAll(keepingCapacity: true)
            }
        }
        scale.initialize(notes: Self.notes(for: snapshot))
        reading = snapshot
        lastObserved = snapshot
        lastFreshTime = time
        lastRevision = revision
        points.append(Point(time: time, pitch: snapshot))
        points.removeAll { time - $0.time > Self.duration }
        if points.count > 128 { points.removeFirst(points.count - 128) }
    }
}
