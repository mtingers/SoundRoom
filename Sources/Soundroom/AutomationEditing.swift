import Foundation
import Combine
import SoundroomCore

enum AutomationRecordMode: String, CaseIterable, Identifiable {
    case read = "Read", touch = "Touch", write = "Write"
    var id: Self { self }
}
struct AutomationClipboard {
    var points: [AutomationPoint]
    var duration: Int
    var domain: String
}
struct AutomationTouchCapture {
    var trackID: UUID
    var effectID: UUID?
    var parameter: String
    var points: [AutomationPoint]
    var baseline: Float
    var startFrame: Int
    var scale: AutomationValueScale
    var curve: AutomationTouchCurve
}
@MainActor final class AutomationEditingState: ObservableObject {
    @Published var hasFocus = false
    @Published var selectedFrames: Set<Int> = []
    @Published var range: Range<Int>?
    @Published var mode = AutomationRecordMode.read
    @Published var clipboard: AutomationClipboard?
    var touches: [String: AutomationTouchCapture] = [:]
    var finishingTouch = false
    var simplificationTolerance: Float = 0.002
    func resetSelection() { hasFocus = false; selectedFrames = []; range = nil }
}

extension SessionStore {
    var automationKeyboardFocused: Bool { automationEditing.hasFocus && canEditAutomation && (effectMode == .realtime || effectMode == .master) && automationLaneVisible }
    func nudgeSelectedAutomation(_ direction: Int, large: Bool = false, vertically: Bool = false) {
        if vertically { moveAutomationGroup(frames: automationEditing.selectedFrames, by: 0, valueDelta: Float(direction) * (large ? 0.1 : 0.01)) }
        else { moveAutomationGroup(frames: automationEditing.selectedFrames, by: direction * Int(sessionRate * (large ? 1 : 0.01)), valueDelta: 0) }
    }
    /// Client cursors/interpolated playback are view state, not recording clocks.
    /// Remote capture is enabled only through a callback-stamped host lease.
    var canRecordAutomation: Bool { false }
    var availableAutomationRecordModes: [AutomationRecordMode] {
        canRecordAutomation ? AutomationRecordMode.allCases : [.read]
    }
    var automationWrites: Bool { canRecordAutomation && (automationWrite || automationEditing.mode != .read) }
    var automationRecordingHelp: String {
        canRecordAutomation
            ? "Read follows curves. Touch records while a control is held, then returns to the old curve over 100 ms. Write records gestures without returning."
            : "This server doesn’t support live automation recording yet. You can edit points directly."
    }
    var currentAutomationScale: AutomationValueScale { automationTarget == .volume ? (currentTrack?.volumeAutomationScale ?? .linear) : .linear }
    var automationDomain: String {
        if let mix = automationTarget.mixParameter { return mix.rawValue }
        return "\(currentRealtimeEffect?.kind.rawValue ?? "").\(currentRealtimeEffect?.selectedAutomationParameter ?? "legacy")"
    }
    var automationSelectionRange: Range<Int>? {
        if let range = automationEditing.range { return range }
        if let first = automationEditing.selectedFrames.min(), let last = automationEditing.selectedFrames.max() { return first..<max(first + 1, last + 1) }
        return selectedRange
    }
    var canPasteAutomation: Bool { canEditAutomation && automationEditing.clipboard?.domain == automationDomain }
    func setAutomationMode(_ mode: AutomationRecordMode) {
        guard mode == .read || canRecordAutomation else {
            automationEditing.mode = .read; automationWrite = false
            status = "This server doesn’t support live automation recording yet."; return
        }
        finishAutomationTouches(); endContinuousEdit()
        automationEditing.mode = mode; automationWrite = mode == .write; focusEditor()
    }
    func updateCurrentAutomation(_ name: String, _ change: (inout [AutomationPoint]) -> Void) {
        guard canEditAutomation, let trackID = selectedTrack else { return }
        if let parameter = automationTarget.mixParameter {
            updateTrack(trackID, actionName: name) { track in
                var curve = track.mixAutomation[parameter.rawValue] ?? []; change(&curve)
                track.mixAutomation[parameter.rawValue] = curve.sorted { $0.frame < $1.frame }
            }
        } else if let effect = currentRealtimeEffect {
            changeRealtimeEffect(effect.id, name: name) { item in
                var curve = item.activeAutomation; change(&curve); item.activeAutomation = curve.sorted { $0.frame < $1.frame }
            }
        }
    }
    func selectAutomationPoint(_ frame: Int, extending: Bool) {
        automationEditing.hasFocus = true; automationLaneVisible = true
        if extending {
            if automationEditing.selectedFrames.contains(frame) { automationEditing.selectedFrames.remove(frame) }
            else { automationEditing.selectedFrames.insert(frame) }
        } else if !automationEditing.selectedFrames.contains(frame) { automationEditing.selectedFrames = [frame] }
        automationEditing.range = nil
    }
    func selectAutomationRange(_ range: Range<Int>) {
        automationEditing.hasFocus = true; automationLaneVisible = true
        automationEditing.range = range
        automationEditing.selectedFrames = Set(automationPoints.filter { range.contains($0.frame) }.map(\.frame))
    }
    func moveAutomationGroup(frames: Set<Int>, by deltaFrames: Int, valueDelta: Float) {
        let points = automationPoints.filter { frames.contains($0.frame) }
        guard !points.isEmpty else { return }
        let shift = max(-(points.map(\.frame).min() ?? 0), deltaFrames)
        guard points.allSatisfy({ $0.frame <= Int.max - max(0, shift) }) else { return }
        let delta = max(-(points.map(\.value).min() ?? 0), min(1 - (points.map(\.value).max() ?? 1), valueDelta.isFinite ? valueDelta : 0))
        guard shift != 0 || delta != 0 else { return }
        let moved = points.map { $0.relocated(frame: $0.frame + shift, value: $0.value + delta) }
        let destinations = Set(moved.map(\.frame))
        updateCurrentAutomation("Move automation points") { curve in
            curve.removeAll { frames.contains($0.frame) || destinations.contains($0.frame) }; curve += moved
        }
        automationEditing.selectedFrames = destinations
        if let range = automationEditing.range { automationEditing.range = max(0, range.lowerBound + shift)..<max(1, range.upperBound + shift) }
        focusEditor()
    }
    func deleteSelectedAutomationPoints() {
        let selected = automationEditing.selectedFrames
        guard !selected.isEmpty else { return }
        updateCurrentAutomation("Delete automation points") { $0.removeAll { selected.contains($0.frame) } }
        automationEditing.resetSelection(); automationEditing.hasFocus = true; automationLaneVisible = true; focusEditor()
    }
    func setAutomationCurve(_ curve: AutomationCurve, frames: Set<Int>? = nil) {
        let selected = frames ?? automationEditing.selectedFrames
        guard !selected.isEmpty else { return }
        updateCurrentAutomation("\(curve.name) automation") { points in
            for i in points.indices where selected.contains(points[i].frame) { points[i].curve = curve; points[i].curveStart = 0; points[i].curveEnd = 1 }
        }
        focusEditor()
    }
    func copyAutomationRange() {
        guard canEditAutomation, let range = automationSelectionRange, !range.isEmpty else { return }
        var prepared = AutomationCurveEditing.insertingBoundary(automationPoints, at: range.lowerBound, baseline: automationBaseValue, scale: currentAutomationScale)
        prepared = AutomationCurveEditing.insertingBoundary(prepared, at: range.upperBound - 1, baseline: automationBaseValue, scale: currentAutomationScale)
        let copied = prepared.filter { range.contains($0.frame) }.map { $0.relocated(frame: $0.frame - range.lowerBound) }
        automationEditing.clipboard = AutomationClipboard(points: copied.sorted { $0.frame < $1.frame }, duration: range.count, domain: automationDomain)
        status = "Automation copied"; focusEditor()
    }
    func pasteAutomation(at frame: Int? = nil) {
        guard canPasteAutomation, let clipboard = automationEditing.clipboard else { return }
        let start = max(0, frame ?? playhead)
        guard start <= Int.max - clipboard.duration else { return }
        let end = start + clipboard.duration
        let points = clipboard.points.map { $0.relocated(frame: start + $0.frame) }
        var prepared = automationPoints
        if start > 0 { prepared = AutomationCurveEditing.insertingBoundary(prepared, at: start - 1, baseline: automationBaseValue, scale: currentAutomationScale) }
        prepared = AutomationCurveEditing.insertingBoundary(prepared, at: end, baseline: automationBaseValue, scale: currentAutomationScale)
        updateCurrentAutomation("Paste automation") { curve in
            curve = prepared.filter { !(start..<end).contains($0.frame) } + points
        }
        automationEditing.selectedFrames = Set(points.map(\.frame)); automationEditing.range = start..<end; focusEditor()
    }
    func duplicateAutomationRange() {
        guard let range = automationSelectionRange else { return }
        copyAutomationRange(); pasteAutomation(at: range.upperBound)
    }
    func simplifyCurrentAutomation() {
        let scale = currentAutomationScale
        let tolerance: Float = scale == .decibels ? 0.2 : automationEditing.simplificationTolerance
        let reduced = AutomationSimplifier.simplify(automationPoints, tolerance: tolerance, scale: scale)
        guard reduced != automationPoints else { status = "No redundant automation points"; return }
        let removed = automationPoints.count - reduced.count
        updateCurrentAutomation("Simplify automation") { $0 = reduced }
        automationEditing.selectedFrames.formIntersection(Set(reduced.map(\.frame)))
        status = "Removed \(removed) points · maximum error \(scale == .decibels ? "0.2 dB" : "0.2%")"; focusEditor()
    }
    func prepareAutomationTouch(key: String, trackID: UUID, effectID: UUID? = nil, parameter: String, points: [AutomationPoint], baseline: Float, scale: AutomationValueScale = .linear) {
        guard canRecordAutomation, automationEditing.mode == .touch, isPlaying, !isPreviewing, automationEditing.touches[key] == nil else { return }
        let cursor = automationTransportCursor
        automationEditing.touches[key] = AutomationTouchCapture(trackID: trackID, effectID: effectID, parameter: parameter, points: points, baseline: baseline, startFrame: cursor.frame, scale: scale, curve: AutomationTouchCurve(points: points, baseline: baseline, scale: scale, start: cursor.frame, loop: automationPlaybackLoopRange, serial: cursor.serial))
    }
    func writeAutomationTouch(key: String, value: Float) -> [AutomationPoint]? {
        guard var capture = automationEditing.touches[key] else { return nil }
        let cursor = automationTransportCursor
        let curve = capture.curve.write(at: cursor.frame, value: value, serial: cursor.serial)
        automationEditing.touches[key] = capture
        return curve
    }
    /// A held fader may cross a loop without producing a new slider event.
    func observeAutomationTransport(at frame: Int, serial: Int? = nil) {
        for key in Array(automationEditing.touches.keys) {
            automationEditing.touches[key]?.curve.observe(frame, serial: serial)
        }
    }
    /// Touch writes only during the gesture, then returns smoothly to the previous Read
    /// curve over 100 ms. Future original knots resume after the return segment.
    func finishAutomationTouches() {
        guard !automationEditing.finishingTouch, !automationEditing.touches.isEmpty else { return }
        automationEditing.finishingTouch = true
        let touches = automationEditing.touches; automationEditing.touches = [:]
        for (key, capture) in touches {
            var capture = capture
            let cursor = automationTransportCursor
            let released = capture.curve.finish(at: cursor.frame, serial: cursor.serial)
            let apply: (inout [AutomationPoint]) -> Void = { $0 = released }
            // Reuse the gesture's history entry rather than creating a second undo.
            updateTrack(capture.trackID, actionName: "Touch automation", coalescingKey: key) { track in
                if let effectID = capture.effectID, let e = track.realtimeEffects.firstIndex(where: { $0.id == effectID }) {
                    if capture.parameter == "legacy" { apply(&track.realtimeEffects[e].automation) }
                    else { var curve = track.realtimeEffects[e].parameterAutomation[capture.parameter] ?? []; apply(&curve); track.realtimeEffects[e].parameterAutomation[capture.parameter] = curve }
                } else { var curve = track.mixAutomation[capture.parameter] ?? []; apply(&curve); track.mixAutomation[capture.parameter] = curve }
            }
        }
        automationEditing.finishingTouch = false
    }
}
