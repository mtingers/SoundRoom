import Foundation
import SoundroomCore

enum AutomationTarget: String, CaseIterable, Identifiable {
    case effect = "Effect", volume = "Volume", pan = "Pan"
    var id: String { rawValue }
    var mixParameter: TrackAutomationParameter? { self == .volume ? .volume : self == .pan ? .pan : nil }
}

extension SessionStore {
    var automationContextID: String {
        let track = selectedTrack?.uuidString ?? ""
        guard automationTarget == .effect else { return "\(track).\(automationTarget.rawValue)" }
        return "\(track).\(currentRealtimeEffect?.id.uuidString ?? "").\(currentRealtimeEffect?.selectedAutomationParameter ?? "legacy")"
    }
    var automationPoints: [AutomationPoint] {
        if let parameter = automationTarget.mixParameter { return currentTrack?.mixAutomation[parameter.rawValue] ?? [] }
        return currentRealtimeEffect?.activeAutomation ?? []
    }
    var automationBaseValue: Float {
        if let parameter = automationTarget.mixParameter { return currentTrack?.baseValue(for: parameter) ?? parameter.normalized(parameter == .volume ? 1 : 0) }
        return currentRealtimeEffect?.activeAutomationAmount ?? 0.5
    }
    var automationValueAtPlayhead: Float {
        if let parameter = automationTarget.mixParameter { return currentTrack?.value(at: playhead, parameter: parameter) ?? automationBaseValue }
        guard let effect = currentRealtimeEffect else { return automationBaseValue }
        return effect.selectedAutomationParameter.map { effect.value(at: playhead, parameter: $0) } ?? effect.value(at: playhead)
    }
    var automationTitle: String {
        if let parameter = automationTarget.mixParameter { return parameter.name }
        return currentRealtimeEffect.map { effect in
            let parameter = effect.selectedAutomationParameter.flatMap { key in effect.kind.parameters.first { $0.id == key }?.name } ?? "Legacy amount"
            return "\(effect.kind.name) · \(parameter)"
        } ?? "Effect"
    }
    var canEditAutomation: Bool { currentTrack != nil && (automationTarget != .effect || currentRealtimeEffect != nil) && !busy && !isRecording }
    func automationDisplay(_ value: Float) -> String {
        automationTarget.mixParameter?.display(value) ?? currentRealtimeEffect?.automationDisplay(value) ?? ""
    }
    func showMixAutomation(_ parameter: TrackAutomationParameter, trackID: UUID) {
        choose(track: trackID); effectMode = trackID == Session.masterTrackID ? .master : .realtime; automationLaneVisible = true
        automationTarget = parameter == .volume ? .volume : .pan
    }
    func setTrackMixValue(_ trackID: UUID, parameter: TrackAutomationParameter, value: Float) {
        let value = parameter.physical(parameter.normalized(value))
        let key = "\(trackID).\(parameter == .volume ? "gain" : "pan")"
        let writing = automationWrites && isPlaying && !isPreviewing
        let previousFrame = isContinuingEdit(key) ? mixWriteFrames[key] : nil
        if writing, let track = trackID == Session.masterTrackID ? session.masterTrack : session.tracks.first(where: { $0.id == trackID }) {
            prepareAutomationTouch(key: key, trackID: trackID, parameter: parameter.rawValue, points: track.mixAutomation[parameter.rawValue] ?? [], baseline: track.baseValue(for: parameter), scale: parameter == .volume ? (track.volumeAutomationScale ?? .linear) : .linear)
        }
        updateTrack(trackID, actionName: parameter.name, coalescingKey: key) { track in
            let before = track
            if parameter == .volume { track.gain = value } else { track.pan = value }
            if writing {
                if let touch = writeAutomationTouch(key: key, value: parameter.normalized(value)) {
                    track.mixAutomation[parameter.rawValue] = touch
                } else {
                    var curve = track.mixAutomation[parameter.rawValue] ?? []
                    if !curve.contains(where: { $0.frame == 0 }) { curve.append(.init(frame: 0, value: before.value(at: 0, parameter: parameter))) }
                    if let previousFrame, previousFrame <= playhead {
                        // Join successive fader samples with ramps, replacing the portion being written.
                        curve.removeAll { $0.frame > previousFrame && $0.frame < playhead }
                    } else if playhead > 0 {
                        curve = AutomationCurveEditing.insertingBoundary(curve, at: playhead - 1, baseline: before.baseValue(for: parameter), scale: parameter == .volume ? (before.volumeAutomationScale ?? .linear) : .linear)
                    }
                    curve.removeAll { $0.frame == playhead }
                    curve.append(.init(frame: playhead, value: parameter.normalized(value)))
                    track.mixAutomation[parameter.rawValue] = curve.sorted { $0.frame < $1.frame }
                }
            }
        }
        if writing { mixWriteFrames[key] = playhead } else { mixWriteFrames[key] = nil }
    }
    func mixFaderValue(_ track: Track, parameter: TrackAutomationParameter) -> Float {
        if !(track.mixAutomation[parameter.rawValue] ?? []).isEmpty { return parameter.physical(track.value(at: playhead, parameter: parameter)) }
        return parameter == .volume ? track.gain : track.pan
    }
    func putCurrentAutomationPoint(frame: Int, value: Float, replacing original: Int? = nil, coalescing: Bool = false) {
        guard canEditAutomation else { return }
        let frame = max(0, frame), value = max(0, min(1, value.isFinite ? value : automationBaseValue))
        if let parameter = automationTarget.mixParameter, let trackID = selectedTrack {
            updateTrack(trackID, actionName: "\(parameter.name) automation", coalescingKey: coalescing ? automationEditKey(frame: original ?? frame) : nil) { track in
                var curve = track.mixAutomation[parameter.rawValue] ?? []
                if curve.isEmpty && frame > 0 { curve.append(.init(frame: 0, value: track.baseValue(for: parameter))) }
                let updated = curve.first { $0.frame == (original ?? frame) }?.relocated(frame: frame, value: value) ?? AutomationPoint(frame: frame, value: value)
                curve.removeAll { $0.frame == frame || $0.frame == original }
                curve.append(updated); track.mixAutomation[parameter.rawValue] = curve.sorted { $0.frame < $1.frame }
            }
        } else if let effect = currentRealtimeEffect {
            changeRealtimeEffect(effect.id, name: "Automation point", coalescingKey: coalescing ? automationEditKey(frame: original ?? frame) : nil) { effect in
                var curve = effect.activeAutomation
                if curve.isEmpty && frame > 0 { curve.append(.init(frame: 0, value: effect.activeAutomationAmount)) }
                let updated = curve.first { $0.frame == (original ?? frame) }?.relocated(frame: frame, value: value) ?? AutomationPoint(frame: frame, value: value)
                curve.removeAll { $0.frame == frame || $0.frame == original }
                curve.append(updated); effect.activeAutomation = curve.sorted { $0.frame < $1.frame }
            }
        }
        if let original, automationEditing.selectedFrames.contains(original) {
            automationEditing.selectedFrames.remove(original); automationEditing.selectedFrames.insert(frame)
        }

    }
    func automationEditKey(frame: Int) -> String {
        if let parameter = automationTarget.mixParameter { return "\(selectedTrack?.uuidString ?? "").\(parameter.rawValue).\(frame)" }
        return "\(currentRealtimeEffect?.id.uuidString ?? "").\(frame)"
    }
    func deleteCurrentAutomationPoint(frame: Int) {
        guard canEditAutomation else { return }
        if let parameter = automationTarget.mixParameter, let id = selectedTrack {
            updateTrack(id, actionName: "Delete \(parameter.name.lowercased()) point") { $0.mixAutomation[parameter.rawValue]?.removeAll { $0.frame == frame } }
        } else if let effect = currentRealtimeEffect { deleteAutomationPoint(effectID: effect.id, frame: frame) }
        automationEditing.selectedFrames.remove(frame)
        focusEditor()
    }
    func clearCurrentAutomation() {
        guard canEditAutomation else { return }
        if let parameter = automationTarget.mixParameter, let id = selectedTrack {
            updateTrack(id, actionName: "Clear \(parameter.name.lowercased()) automation") { $0.mixAutomation[parameter.rawValue] = [] }
        } else if let effect = currentRealtimeEffect { changeRealtimeEffect(effect.id, name: "Clear automation") { $0.activeAutomation = [] } }
        automationEditing.selectedFrames = []; automationEditing.range = nil
        focusEditor()
    }
    func fadeVolumeSelection(out: Bool) {
        guard !busy, !isRecording, let track = currentTrack, let range = selectedRange, range.count > 1 else { return }
        let start = range.lowerBound, end = range.upperBound - 1, baseline = track.value(at: out ? start : end, parameter: .volume)
        updateTrack(track.id, actionName: out ? "Volume fade out" : "Volume fade in") { item in
            var curve = item.mixAutomation["volume"] ?? []
            if start > 0 {
                let needsBoundary = !curve.isEmpty || !out
                if curve.isEmpty { curve.append(.init(frame: 0, value: item.baseValue(for: .volume))) }
                if needsBoundary {
                    curve.removeAll { $0.frame == start - 1 }
                    curve.append(.init(frame: start - 1, value: track.value(at: start - 1, parameter: .volume)))
                }
            }
            curve.removeAll { (start...end).contains($0.frame) }
            curve.append(.init(frame: start, value: out ? baseline : 0)); curve.append(.init(frame: end, value: out ? 0 : baseline))
            item.mixAutomation["volume"] = curve.sorted { $0.frame < $1.frame }
        }
        automationTarget = .volume; focusEditor()
    }
}
