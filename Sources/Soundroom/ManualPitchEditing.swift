import Foundation
import SoundroomCore

struct ManualPitchEditing: Equatable {
    private(set) var frames = 0
    private(set) var region = 0..<0
    private(set) var target = 69
    private(set) var targetLimits = 57...81
    private(set) var hasVoicedRegion = false

    mutating func load(frames: Int, contour: [PitchContourPoint], reference: Float) {
        self.frames = max(0, frames)
        if self.frames == 0 { region = 0..<0; hasVoicedRegion = false; return }
        select(0..<self.frames, contour: contour, reference: reference)
    }
    mutating func select(_ proposed: Range<Int>, contour: [PitchContourPoint], reference: Float) {
        guard frames > 0 else { return }
        let minimum = min(frames, 4096)
        let start = max(0, min(frames - minimum, proposed.lowerBound))
        let end = max(start + minimum, min(frames, proposed.upperBound))
        region = start..<end
        let reference = reference.isFinite && (420...460).contains(reference) ? reference : 440
        let notes = contour.filter { region.contains($0.frame) && $0.isVoiced }
            .map { 69 + 12 * log2($0.frequency / reference) }.filter(\.isFinite).sorted()
        hasVoicedRegion = !notes.isEmpty
        guard !notes.isEmpty else { return }
        let median = Int(notes[notes.count / 2].rounded())
        targetLimits = max(32, median - 12)...min(83, median + 12)
        target = max(targetLimits.lowerBound, min(targetLimits.upperBound, median))
    }
    mutating func setTarget(_ value: Int) { target = max(targetLimits.lowerBound, min(targetLimits.upperBound, value)) }
    mutating func stepTarget(_ delta: Int) { setTarget(target + delta) }
    func parameters(from values: [String: Float]) -> [String: Float] {
        var result = values.isEmpty ? RealtimeEffectKind.pitchCorrection.defaultParameters : values
        result["manualTarget"] = Float(target)
        return result
    }
    func timelineRange(selection: Range<Int>) -> Range<Int>? {
        guard frames == selection.count, region.lowerBound >= 0, region.upperBound <= frames, !region.isEmpty else { return nil }
        return (selection.lowerBound + region.lowerBound)..<(selection.lowerBound + region.upperBound)
    }
}

struct PitchChartViewport: Equatable {
    private(set) var fullFrames = 0
    private(set) var frameRange = 0..<1
    private(set) var pitchLower: Float = 65
    private(set) var pitchUpper: Float = 73
    private var originalLower: Float = 65
    private var originalUpper: Float = 73

    mutating func fit(frames: Int, contour: [PitchContourPoint], reference: Float, extraNote: Float? = nil) {
        fullFrames = max(1, frames)
        let reference = reference.isFinite && (420...460).contains(reference) ? reference : 440
        var notes = contour.filter(\.isVoiced).map { 69 + 12 * log2($0.frequency / reference) }.filter(\.isFinite)
        if let extraNote, extraNote.isFinite { notes.append(extraNote) }
        let middle = ((notes.min() ?? 69) + (notes.max() ?? 69)) / 2
        let span = max(8, (notes.max() ?? 69) - (notes.min() ?? 69) + 4)
        originalLower = floor(middle - span / 2); originalUpper = ceil(middle + span / 2)
        reset()
    }
    mutating func reset() {
        frameRange = 0..<max(1, fullFrames)
        pitchLower = originalLower; pitchUpper = originalUpper
    }
    mutating func zoomTime(_ factor: Double, around anchor: Int? = nil) {
        guard factor.isFinite, factor > 0 else { return }
        let count = Int(max(Double(min(fullFrames, 4800)), min(Double(fullFrames), Double(frameRange.count) / factor)))
        let middle = anchor ?? (frameRange.lowerBound + frameRange.count / 2)
        let start = max(0, min(fullFrames - count, middle - count / 2))
        frameRange = start..<(start + count)
    }
    mutating func panTime(_ direction: Int) {
        let delta = frameRange.count / 2 * direction
        let start = max(0, min(fullFrames - frameRange.count, frameRange.lowerBound + delta))
        frameRange = start..<(start + frameRange.count)
    }
    mutating func zoomPitch(_ factor: Float, around anchor: Float? = nil) {
        guard factor.isFinite, factor > 0 else { return }
        let middle = anchor.flatMap { $0.isFinite ? $0 : nil } ?? (pitchLower + pitchUpper) / 2
        let span = max(2, min(72, (pitchUpper - pitchLower) / factor))
        pitchLower = middle - span / 2; pitchUpper = middle + span / 2
    }
    mutating func panPitch(_ semitones: Float) {
        let delta = max(-24 - pitchLower, min(127 - pitchUpper, semitones))
        pitchLower += delta; pitchUpper += delta
    }
    func frame(at fraction: Double) -> Int {
        guard fraction.isFinite else { return frameRange.lowerBound }
        return frameRange.lowerBound + Int(max(0, min(1, fraction)) * Double(frameRange.count))
    }
    func note(at fraction: Double) -> Int {
        guard fraction.isFinite else { return Int(pitchLower.rounded()) }
        return Int((pitchUpper - Float(max(0, min(1, fraction))) * (pitchUpper - pitchLower)).rounded())
    }
}

extension SessionStore {
    /// Local manual parameters never change the automatic whole-selection effect.
    /// The same history/Undo path as Apply replaces only this timeline region.
    func applyManualPitchTune(trackID: UUID, selectionRange: Range<Int>, region: Range<Int>, target: Int, parameters: [String: Float]) {
        _ = remoteRoute("pitch.manualApply", integers: ["selectionStart": selectionRange.lowerBound, "selectionEnd": selectionRange.upperBound,
            "start": region.lowerBound, "end": region.upperBound, "target": target], strings: ["trackID": trackID.uuidString], payload: try? JSONEncoder().encode(parameters))
    }
}
