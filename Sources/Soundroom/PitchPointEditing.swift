import Foundation
import SoundroomCore

/// Selection and geometry are editor state; the persisted targets stay in the effect.
struct PitchPointEditing: Equatable {
    var selected: Set<UUID> = []
    var primary: UUID?
    var range: Range<Int>?
    var insertionFrame: Int?
    var insertionNote: Float?

    mutating func placeInsertion(frame: Int, note: Float) {
        guard note.isFinite else { return }
        selected = []; primary = nil; range = nil
        insertionFrame = frame; insertionNote = max(0, min(95, note))
    }
    func insertionTarget(in regions: [PitchEditRegion], defaultFrame: Int) -> (frame: Int, note: Float) {
        let point = selectedPoint(in: regions)
        let frame = insertionFrame ?? point?.frame ?? defaultFrame
        let note = insertionNote ?? regions.first { $0.range.contains(frame) }?.note(at: frame) ?? point?.note ?? 69
        return (frame, note)
    }

    mutating func select(_ id: UUID, extending: Bool) {
        insertionFrame = nil; insertionNote = nil
        if extending {
            if selected.contains(id) { selected.remove(id) } else { selected.insert(id) }
        } else if !selected.contains(id) { selected = [id] }
        primary = selected.contains(id) ? id : selected.first
        range = nil
    }
    mutating func select(_ range: Range<Int>, regions: [PitchEditRegion]) {
        insertionFrame = nil; insertionNote = nil
        self.range = range
        selected = Set(regions.flatMap(\.points).filter { range.contains($0.frame) }.map(\.id))
        primary = regions.flatMap(\.points).first { selected.contains($0.id) }?.id
    }
    mutating func reconcile(_ regions: [PitchEditRegion]) {
        selected.formIntersection(Set(regions.flatMap(\.points).map(\.id)))
        if let primary, !selected.contains(primary) { self.primary = selected.first }
    }
    func selectedPoint(in regions: [PitchEditRegion]) -> PitchEditPoint? {
        regions.flatMap(\.points).first { $0.id == primary } ?? regions.flatMap(\.points).first { selected.contains($0.id) }
    }

    /// Move from the gesture's original snapshot, so repeated UI updates never compound.
    /// Anchors cannot cross another anchor or leave their own voiced region.
    func moving(_ regions: [PitchEditRegion], frames requestedFrames: Int, notes requestedNotes: Float,
                snap: Bool = false, noteLimits: ClosedRange<Float> = 0...95) -> [PitchEditRegion] {
        let chosen = regions.flatMap(\.points).filter { selected.contains($0.id) }
        guard !chosen.isEmpty, requestedNotes.isFinite else { return regions }
        var minimumFrame = Int.min / 2, maximumFrame = Int.max / 2
        for region in regions {
            let sorted = region.points.sorted { $0.frame < $1.frame }
            for (index, point) in sorted.enumerated() where selected.contains(point.id) {
                var lower = region.startFrame, upper = region.endFrame
                if let neighbor = sorted[..<index].last(where: { !selected.contains($0.id) }) { lower = neighbor.frame + 1 }
                if let neighbor = sorted[(index + 1)...].first(where: { !selected.contains($0.id) }) { upper = neighbor.frame - 1 }
                minimumFrame = max(minimumFrame, lower - point.frame)
                maximumFrame = min(maximumFrame, upper - point.frame)
            }
        }
        guard minimumFrame <= maximumFrame else { return regions }
        let frames = max(minimumFrame, min(maximumFrame, requestedFrames))
        let minimumNote = noteLimits.lowerBound - (chosen.map(\.note).min() ?? 0)
        let maximumNote = noteLimits.upperBound - (chosen.map(\.note).max() ?? 95)
        var notes = requestedNotes
        if snap, let anchor = selectedPoint(in: regions) { notes = (anchor.note + notes).rounded() - anchor.note }
        notes = max(minimumNote, min(maximumNote, notes))
        var result = regions
        for r in result.indices {
            for p in result[r].points.indices where selected.contains(result[r].points[p].id) {
                result[r].points[p].frame += frames
                result[r].points[p].note += notes
            }
            result[r].points.sort { $0.frame < $1.frame }
        }
        return result
    }

    func deleting(_ regions: [PitchEditRegion]) -> [PitchEditRegion] {
        var result = regions
        for index in result.indices { result[index].points.removeAll { selected.contains($0.id) } }
        return result.filter { !$0.points.isEmpty }
    }
    func settingCurve(_ curve: AutomationCurve, in regions: [PitchEditRegion]) -> [PitchEditRegion] {
        var result = regions
        for r in result.indices {
            for p in result[r].points.indices where selected.contains(result[r].points[p].id) {
                result[r].points[p].curve = curve
                result[r].points[p].curveStart = 0; result[r].points[p].curveEnd = 1
            }
        }
        return result
    }
    /// A local pitch operation, independent of the effect's Retune/Humanize controls.
    /// Region endpoints and times remain unchanged; silence gaps are never bridged.
    func smoothing(_ regions: [PitchEditRegion], amount: Float) -> [PitchEditRegion] {
        guard amount.isFinite else { return regions }
        let blend = max(0, min(1, amount))
        var result = regions
        for r in result.indices {
            let original = result[r].points.sorted { $0.frame < $1.frame }
            guard original.count >= 3 else { continue }
            result[r].points = original
            for p in 1..<(original.count - 1) where selected.contains(original[p].id) {
                let middle = (original[p - 1].note + original[p].note * 2 + original[p + 1].note) / 4
                result[r].points[p].note = original[p].note + (middle - original[p].note) * blend
            }
        }
        return result
    }
    func adding(to regions: [PitchEditRegion], frame: Int, note: Float) -> (regions: [PitchEditRegion], point: UUID?) {
        guard note.isFinite, let r = regions.firstIndex(where: { ($0.startFrame..<$0.endFrame).contains(frame) }),
              !regions[r].points.contains(where: { $0.frame == frame }) else { return (regions, nil) }
        var result = regions
        let point = PitchEditPoint(frame: frame, note: max(0, min(95, note)))
        result[r].points.append(point); result[r].points.sort { $0.frame < $1.frame }
        return (result, point.id)
    }
}

enum PitchCurvePreset: String, CaseIterable, Identifiable {
    case natural = "Natural", tight = "Tight", robotic = "Robotic", performance = "Keep performance"
    var id: Self { self }
    var strength: Float { switch self { case .natural: return 0.5; case .tight, .robotic: return 1; case .performance: return 0 } }
    var curve: AutomationCurve { switch self { case .natural: return .smooth; case .robotic: return .hold; default: return .linear } }
}

enum PitchPointTargets {
    /// Seeds only measured voiced runs; explicit unvoiced decisions split regions.
    /// RDP removes redundant measured knots within a declared cent tolerance.
    static func seed(contour: [PitchContourPoint], scope: Range<Int>, reference: Float,
                     preset: PitchCurvePreset = .natural, toleranceCents: Float = 5,
                     allowedPitchClasses: Int = 4095) -> [PitchEditRegion] {
        guard !scope.isEmpty else { return [] }
        let reference = reference.isFinite && reference > 0 ? reference : 440
        let sorted = contour.filter { scope.contains($0.frame) }.sorted { $0.frame < $1.frame }
        let deltas = zip(sorted, sorted.dropFirst()).map { $1.frame - $0.frame }.filter { $0 > 0 }.sorted()
        let hop = deltas.isEmpty ? 256 : deltas[deltas.count / 2]
        var runs: [[PitchContourPoint]] = [], run: [PitchContourPoint] = []
        for point in sorted {
            if !point.isVoiced || (run.last.map { point.frame - $0.frame > hop * 2 } ?? false) {
                if !run.isEmpty { runs.append(run); run = [] }
            }
            if point.isVoiced { run.append(point) }
        }
        if !run.isEmpty { runs.append(run) }
        return runs.compactMap { run in
            guard let first = run.first, let last = run.last else { return nil }
            let start = max(scope.lowerBound, first.frame - hop / 2)
            let end = min(scope.upperBound, last.frame + max(1, hop / 2))
            guard start < end else { return nil }
            let measured = run.compactMap { point -> PitchEditPoint? in
                let note = 69 + 12 * log2(point.frequency / reference)
                guard note.isFinite else { return nil }
                return PitchEditPoint(frame: point.frame, note: note)
            }
            let targets: [PitchEditPoint]
            let stable = preset == .performance ? measured.map(\.note) : stabilizedNotes(measured, mask: allowedPitchClasses)
            let prepared = measured.enumerated().map { index, point in
                var result = point
                result.note += (stable[index] - point.note) * preset.strength
                return result
            }
            if preset == .robotic {
                let snapped = prepared.map { point -> PitchEditPoint in
                    var result = point; result.curve = allowedPitchClasses & 4095 == 0 ? .linear : .hold; return result
                }
                targets = snapped.enumerated().compactMap { index, point in
                    index == 0 || index == snapped.count - 1 || snapped[index - 1].note != point.note ? point : nil
                }
            } else if preset == .tight {
                targets = simplify(prepared, toleranceCents: toleranceCents)
            } else {
                targets = simplify(prepared, toleranceCents: toleranceCents).map { point in
                    var result = point
                    result.curve = allowedPitchClasses & 4095 == 0 ? .linear : preset.curve
                    return result
                }
            }
            return targets.isEmpty ? nil : PitchEditRegion(startFrame: start, endFrame: end, points: targets)
        }
    }
    /// Seeded presets reject vibrato crossings; hand-authored points never pass here.
    /// A confirmed change is backdated to its onset, without shifting musical timing.
    /// The 110 ms confirmation deliberately favors stable notes over very short ornaments.
    private static func stabilizedNotes(_ points: [PitchEditPoint], mask: Int) -> [Float] {
        guard let first = points.first, mask & 4095 != 0 else { return points.map(\.note) }
        let initialWindow = Int(sessionRate * 0.2), confirmation = Int(sessionRate * 0.11)
        let initial = points.prefix { $0.frame - first.frame < initialWindow }.map(\.note).sorted()
        var established = nearestAllowed(initial[initial.count / 2], mask: mask)
        var result = [Float](repeating: established, count: points.count)
        var pending: Float?, pendingIndex = 0
        for (index, point) in points.enumerated() {
            result[index] = established
            let desired = nearestAllowed(point.note, mask: mask)
            let preference = abs(point.note - established) - abs(point.note - desired)
            if desired == established || preference < 0.18 {
                pending = nil
                continue
            }
            if pending != desired { pending = desired; pendingIndex = index }
            if point.frame - points[pendingIndex].frame >= confirmation {
                established = desired
                for accepted in pendingIndex...index { result[accepted] = desired }
                pending = nil
            }
        }
        return result
    }
    static func applying(_ preset: PitchCurvePreset, to regions: [PitchEditRegion], selected: Set<UUID>? = nil,
                         allowedPitchClasses: Int = 4095) -> [PitchEditRegion] {
        regions.map { region in
            var result = region
            result.points = applying(preset, to: region.points, selected: selected, allowedPitchClasses: allowedPitchClasses)
            return result
        }
    }
    private static func applying(_ preset: PitchCurvePreset, to points: [PitchEditPoint], selected: Set<UUID>? = nil,
                                 allowedPitchClasses: Int = 4095) -> [PitchEditPoint] {
        points.map { point in
            guard selected == nil || selected!.contains(point.id) else { return point }
            var result = point
            result.note += (nearestAllowed(point.note, mask: allowedPitchClasses) - point.note) * preset.strength
            result.curve = allowedPitchClasses & 4095 == 0 ? .linear : preset.curve
            result.curveStart = 0; result.curveEnd = 1
            return result
        }
    }
    static func nearestAllowed(_ note: Float, mask: Int) -> Float {
        guard note.isFinite, mask & 4095 != 0 else { return note }
        var nearest = note, distance = Float.infinity
        for target in 0...95 where mask & (1 << (target % 12)) != 0 {
            let delta = abs(Float(target) - note)
            if delta < distance { nearest = Float(target); distance = delta }
        }
        return nearest
    }
    static func allowedPitchClasses(parameters: [String: Float]) -> Int {
        let scaleValue = parameters["scale"] ?? 0, keyValue = parameters["key"] ?? 0
        let scale = scaleValue.isFinite ? Int(max(0, min(3, scaleValue))) : 0
        if scale == 0 { return 4095 }
        if scale == 3 {
            let mask = parameters["mask"] ?? 4095
            return mask.isFinite ? Int(max(0, min(4095, mask))) : 4095
        }
        let key = keyValue.isFinite ? Int(max(0, min(11, keyValue))) : 0
        let base = scale == 1 ? 0b101010110101 : 0b010110101101
        return ((base << key) | (base >> (12 - key))) & 4095
    }
    static func simplify(_ points: [PitchEditPoint], toleranceCents: Float) -> [PitchEditPoint] {
        guard points.count > 2, toleranceCents.isFinite, toleranceCents > 0 else { return points }
        let normalized = points.map { AutomationPoint(frame: $0.frame, value: $0.note / 95,
            curve: $0.curve, curveStart: $0.curveStart, curveEnd: $0.curveEnd) }
        let retained = Set(AutomationSimplifier.simplify(normalized, tolerance: toleranceCents / 9500).map(\.frame))
        return points.filter { retained.contains($0.frame) }
    }
}

/// An absolute timeline viewport. Editing targets does not change either axis.
struct PitchPointViewport: Equatable {
    private(set) var scope = 0..<1
    private(set) var frames = 0..<1
    private(set) var lower: Float = 57
    private(set) var upper: Float = 81
    mutating func fit(scope: Range<Int>, notes: [Float]) {
        guard !scope.isEmpty else { return }
        self.scope = scope; frames = scope
        let valid = notes.filter { $0.isFinite && (0...95).contains($0) }
        let lo = valid.min() ?? 65, hi = valid.max() ?? 73
        let middle = (lo + hi) / 2, span = max(8, hi - lo + 4)
        lower = floor(middle - span / 2); upper = ceil(middle + span / 2)
    }
    mutating func fitSelected(_ points: [PitchEditPoint]) {
        guard let first = points.map(\.frame).min(), let last = points.map(\.frame).max() else { return }
        let margin = max(2400, (last - first) / 5)
        let start = max(scope.lowerBound, first - margin), end = min(scope.upperBound, last + margin + 1)
        frames = start..<max(start + 1, end)
        let notes = points.map(\.note)
        let lo = notes.min() ?? 65, hi = notes.max() ?? 73
        let middle = (lo + hi) / 2, span = max(3, hi - lo + 2)
        lower = middle - span / 2; upper = middle + span / 2
    }
    mutating func zoomTime(_ factor: Double, around frame: Int? = nil) {
        guard factor.isFinite, factor > 0 else { return }
        let count = Int(max(Double(min(480, scope.count)), min(Double(scope.count), Double(frames.count) / factor)))
        let middle = frame ?? (frames.lowerBound + frames.count / 2)
        let start = max(scope.lowerBound, min(scope.upperBound - count, middle - count / 2))
        frames = start..<(start + count)
    }
    mutating func panTime(_ direction: Int) {
        let step = frames.count / 2 * max(-1, min(1, direction))
        let start = max(scope.lowerBound, min(scope.upperBound - frames.count, frames.lowerBound + step))
        frames = start..<(start + frames.count)
    }
    mutating func zoomPitch(_ factor: Float) {
        guard factor.isFinite, factor > 0 else { return }
        let middle = (lower + upper) / 2, span = max(0.5, min(96, (upper - lower) / factor))
        lower = middle - span / 2; upper = middle + span / 2
    }
    mutating func panPitch(_ notes: Float) {
        guard notes.isFinite else { return }
        let delta = max(-12 - lower, min(107 - upper, notes))
        lower += delta; upper += delta
    }
    func frame(_ fraction: Double) -> Int {
        guard fraction.isFinite else { return frames.lowerBound }
        return min(frames.upperBound - 1, frames.lowerBound + Int(max(0, min(1, fraction)) * Double(frames.count)))
    }
    func note(_ fraction: Double) -> Float {
        guard fraction.isFinite else { return lower }
        return upper - Float(max(0, min(1, fraction))) * (upper - lower)
    }
}
