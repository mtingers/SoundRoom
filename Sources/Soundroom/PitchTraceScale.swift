import Foundation

/// Display-only viewport. Meter updates never reframe an established scale.
struct PitchTraceScale: Equatable {
    private(set) var lower: Float = 57
    private(set) var upper: Float = 81
    private(set) var initialized = false
    var span: Float { upper - lower }

    mutating func initialize(notes: [Float]) {
        guard !initialized else { return }
        fit(notes: notes)
    }
    mutating func fit(notes: [Float]) {
        let finite = notes.filter(\.isFinite)
        guard let low = finite.min(), let high = finite.max() else { return }
        let center = ((low + high) / 2).rounded()
        let half = max(12, ceil(max(center - low, high - center) + 2))
        lower = center - half
        upper = center + half
        initialized = true
    }
    mutating func zoom(_ factor: Float) {
        guard factor.isFinite, factor > 0 else { return }
        let center = (lower + upper) / 2
        let half = max(2, min(48, span * factor / 2))
        lower = center - half
        upper = center + half
        initialized = true
    }
    mutating func move(semitones: Float) {
        guard semitones.isFinite else { return }
        lower += semitones
        upper += semitones
        initialized = true
    }
    func contains(_ note: Float) -> Bool { note >= lower && note <= upper }
}
