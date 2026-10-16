import AppKit
import SoundroomCore

extension SessionStore {
    func setSelectionEdge(start: Bool, seconds: Double) {
        guard let track = selectedTrack, let range = selectedRange, seconds.isFinite else { return }
        let frame = Int(min(Double(Int.max / 2), max(0, seconds * sessionRate)).rounded())
        guard !busy, !isRecording else { return }
        endContinuousEdit()
        selectedClip = nil; selectedClips = []
        let adjusted = start ? min(frame, range.upperBound - 1)..<range.upperBound : range.lowerBound..<max(range.lowerBound + 1, frame)
        selection = EditSelection(trackID: track, range: adjusted); playhead = adjusted.lowerBound

    }
    /// Pixels per second. The floor fits about half an hour in 900 points,
    /// so Fit Session works for whole songs (it was 5, which could not fit
    /// anything longer than about three minutes).
    static let zoomRange: ClosedRange<Double> = 0.5...1600
    static func clampedZoom(_ value: Double) -> Double { min(zoomRange.upperBound, max(zoomRange.lowerBound, value)) }
    func zoomToSelection() {
        guard let range = selectedRange, !range.isEmpty else { return }
        let margin = range.lowerBound > 0 ? min(40, visibleTimelineWidth * 0.1) : 0
        zoom = Self.clampedZoom(max(1, visibleTimelineWidth - margin) / (Double(range.count) / sessionRate + 0.2))
        requestTimelineReveal(range.lowerBound)
    }
    func fitSession() {
        zoom = Self.clampedZoom(min(400, visibleTimelineWidth / max(1, Double(session.end) / sessionRate + 1)))
        requestTimelineReveal(0)
    }
    func requestTimelineReveal(_ frame: Int, onlyIfHidden: Bool = false) {
        revealOnlyIfHidden = onlyIfHidden
        revealFrame = max(0, frame)
        revealRequestSerial &+= 1
    }
    func revealPlayhead() { requestTimelineReveal(playhead) }
    /// Tab / ⇧Tab: move the cursor to the next / previous clip start or end on
    /// the selected track (all tracks when none is selected).
    func jumpToClipEdge(forward: Bool) {
        guard !isRecording else { return }
        let tracks = currentTrack.map { [$0] } ?? session.tracks
        let edges = Set(tracks.flatMap(\.clips).flatMap { [$0.start, $0.end] })
        let from = isPlaying ? playbackPosition.frame : playhead
        guard let target = forward ? edges.filter({ $0 > from }).min() : edges.filter({ $0 < from }).max() else { return }
        selection = nil
        seek(target)
        requestTimelineReveal(target, onlyIfHidden: true)
    }
    func requestTimelineZoom(_ factor: Double) {
        guard factor.isFinite, factor > 0 else { return }
        timelineZoomFactor = factor
        timelineZoomSerial &+= 1
    }
    func trimClip(_ id: UUID, trackID: UUID, start: Int? = nil, end: Int? = nil) {
        guard let track = session.tracks.first(where: { $0.id == trackID }), let original = track.clips.first(where: { $0.id == id }), let source = assets[original.assetID] else { return }
        var changed = original
        if let start {
            let earliest = max(0, original.start - original.offset)
            changed.start = max(earliest, min(original.end - 1, start))
            let delta = changed.start - original.start
            changed.offset += delta; changed.length -= delta
        }
        if let end { changed.length = max(1, min(source.count - changed.offset, end - changed.start)) }
        guard changed != original else { return }
        changed.resetFadeEnvelope()
        edit("Trim clip") {
            guard let t = session.tracks.firstIndex(where: { $0.id == trackID }), let c = session.tracks[t].clips.firstIndex(where: { $0.id == id }) else { return }
            session.tracks[t].clips[c] = changed; selectedTrack = trackID; selectedClip = id; selectedClips = [id]; selection = nil
        }
    }
    func setClipFade(_ id: UUID, trackID: UUID, fadeIn: Bool, frames: Int) {
        updateTrack(trackID, actionName: fadeIn ? "Fade in" : "Fade out", coalescingKey: "\(id).fade.\(fadeIn)") { track in
            guard let c = track.clips.firstIndex(where: { $0.id == id }) else { return }
            track.clips[c].resetFadeEnvelope()
            if fadeIn { track.clips[c].fadeInFrames = min(track.clips[c].length, max(0, frames)) }
            else { track.clips[c].fadeOutFrames = min(track.clips[c].length, max(0, frames)) }
        }
    }
    func clipMoveProposal(primary id: UUID, to frame: Int) -> (ids: Set<UUID>, shift: Int, guide: Int?)? {
        let selected = selectedClips.contains(id) ? selectedClips : [id]
        let clips = session.tracks.flatMap(\.clips).filter { selected.contains($0.id) }
        guard let primary = clips.first(where: { $0.id == id }), !clips.isEmpty else { return nil }
        var destination = max(0, frame)
        var guide: Int?
        if snap {
            // Either side of the group may meet an anchor, including selection and grid edges.
            let edges = clips.flatMap { [$0.start, $0.end] }
            let rawShift = destination - primary.start
            let candidates = edges.map { edge -> (shift: Int, guide: Int?) in
                let snapped = snappedFrame(edge + rawShift, excluding: selected)
                return (snapped.frame - edge, snapped.guide)
            }.filter { $0.guide != nil }
            if let best = candidates.min(by: { abs($0.shift - rawShift) < abs($1.shift - rawShift) }) {
                destination = primary.start + best.shift; guide = best.guide
            }
        }
        let earliest = clips.map(\.start).min() ?? 0
        let shift = max(-earliest, destination - primary.start)
        if primary.start + shift != destination { guide = nil }
        return (selected, shift, guide)
    }
    func moveSelectedClips(primary id: UUID, to frame: Int, targetTrack: UUID? = nil, duplicate: Bool = false) {
        if let targetTrack, session.tracks.first(where: { $0.id == targetTrack })?.acceptsClips == false { status = "Place clips on an audio or instrument track. Buses and returns receive routed audio."; return }
        guard let proposal = clipMoveProposal(primary: id, to: frame) else { return }
        let selected = proposal.ids, shift = proposal.shift
        let clips = session.tracks.flatMap(\.clips).filter { selected.contains($0.id) }
        guard shift != 0 || duplicate || targetTrack != nil else { return }
        guard clips.allSatisfy({ $0.start <= Int.max - max(0, shift) - $0.length }) else { return }
        edit(duplicate ? "Duplicate clips" : "Move clips") {
            var newIDs: Set<UUID> = []
            let movingByTrack = session.tracks.map { $0.clips.filter { selected.contains($0.id) } }
            let originalEffects = session.tracks.map(\.realtimeEffects)
            let originalMix = session.tracks.map(\.mixAutomation)
            if !duplicate { for t in session.tracks.indices { session.tracks[t].clips.removeAll { selected.contains($0.id) } } }
            for t in session.tracks.indices {
                let moving = movingByTrack[t]
                guard !moving.isEmpty else { continue }
                let destinationIndex = targetTrack.flatMap { target in session.tracks.firstIndex { $0.id == target } } ?? t
                for var clip in moving {
                    clip.start += shift
                    if duplicate { clip.id = UUID() }
                    newIDs.insert(clip.id); session.tracks[destinationIndex].clips.append(clip)
                }
            }
            if moveAutomationWithClips {
                // Remove source points before inserting destination points so a group
                // crossing several tracks cannot shift incoming automation twice.
                if !duplicate {
                    for t in session.tracks.indices where !movingByTrack[t].isEmpty {
                        let moving = movingByTrack[t]
                        let contains: (AutomationPoint) -> Bool = { point in moving.contains { ($0.start..<$0.end).contains(point.frame) } }
                        for key in originalMix[t].keys { session.tracks[t].mixAutomation[key]?.removeAll(where: contains) }
                        for e in originalEffects[t].indices {
                            session.tracks[t].realtimeEffects[e].automation.removeAll(where: contains)
                            for key in originalEffects[t][e].parameterAutomation.keys { session.tracks[t].realtimeEffects[e].parameterAutomation[key]?.removeAll(where: contains) }
                            for scope in PitchEditRegions.sanitizedRanges(moving.map { PitchEditRange($0.start..<$0.end) }) {
                                session.tracks[t].realtimeEffects[e].pitchEdits = PitchEditRegions.removing(session.tracks[t].realtimeEffects[e].pitchEdits, in: scope.range)
                                session.tracks[t].realtimeEffects[e].bakedRanges = PitchEditRegions.removing(session.tracks[t].realtimeEffects[e].bakedRanges, in: scope.range)
                            }
                        }
                    }
                }
                let merged: ([AutomationPoint], [AutomationPoint]) -> [AutomationPoint] = { existing, incoming in
                    let incomingFrames = Set(incoming.map(\.frame))
                    return (existing.filter { !incomingFrames.contains($0.frame) } + incoming).sorted { $0.frame < $1.frame }
                }
                for t in session.tracks.indices where !movingByTrack[t].isEmpty {
                    let moving = movingByTrack[t]
                    let shifted: ([AutomationPoint]) -> [AutomationPoint] = { points in
                        points.filter { point in moving.contains { ($0.start..<$0.end).contains(point.frame) } }
                            .map { $0.relocated(frame: max(0, $0.frame + shift)) }
                    }
                    let destinationIndex = targetTrack.flatMap { target in session.tracks.firstIndex { $0.id == target } } ?? t
                    for (key, points) in originalMix[t] {
                        let incoming = shifted(points)
                        guard !incoming.isEmpty else { continue }
                        session.tracks[destinationIndex].mixAutomation[key] = merged(session.tracks[destinationIndex].mixAutomation[key] ?? [], incoming)
                    }
                    for original in originalEffects[t] {
                        let movedLegacy = shifted(original.automation)
                        let movedParameters = original.parameterAutomation.mapValues(shifted)
                        let scopes = PitchEditRegions.sanitizedRanges(moving.map { PitchEditRange($0.start..<$0.end) })
                        let selectedEdits = scopes.flatMap { PitchEditRegions.slicing(original.pitchEdits, to: $0.range) }
                        let movedEdits = PitchEditRegions.relocated(selectedEdits, by: shift, renewIDs: duplicate || destinationIndex != t)
                        let selectedBaked = scopes.flatMap { scope in
                            original.bakedRanges.compactMap { range -> PitchEditRange? in
                                let start = max(scope.startFrame, range.startFrame), end = min(scope.endFrame, range.endFrame)
                                return start < end ? .init(startFrame: start, endFrame: end) : nil
                            }
                        }
                        let movedBaked = PitchEditRegions.relocatedRanges(selectedBaked, by: shift)
                        guard !movedLegacy.isEmpty || movedParameters.values.contains(where: { !$0.isEmpty }) || !movedEdits.isEmpty || !movedBaked.isEmpty else { continue }
                        // Reuse an equivalent insert. Otherwise carry its settings and selected curves.
                        let destinationEffect = destinationIndex == t
                            ? session.tracks[t].realtimeEffects.firstIndex { $0.id == original.id }
                            : session.tracks[destinationIndex].realtimeEffects.firstIndex {
                                $0.kind == original.kind && $0.parameters == original.parameters && $0.amount == original.amount && $0.enabled == original.enabled && $0.pitchEditsOnly == original.pitchEditsOnly
                            }
                        if let destinationEffect {
                            let existing = session.tracks[destinationIndex].realtimeEffects[destinationEffect]
                            session.tracks[destinationIndex].realtimeEffects[destinationEffect].automation = merged(existing.automation, movedLegacy)
                            for (key, incoming) in movedParameters {
                                session.tracks[destinationIndex].realtimeEffects[destinationEffect].parameterAutomation[key] = merged(existing.parameterAutomation[key] ?? [], incoming)
                            }
                            var edits = existing.pitchEdits
                            for incoming in movedEdits { edits = PitchEditRegions.replacing(edits, with: incoming) }
                            session.tracks[destinationIndex].realtimeEffects[destinationEffect].pitchEdits = edits
                            session.tracks[destinationIndex].realtimeEffects[destinationEffect].bakedRanges = PitchEditRegions.sanitizedRanges(existing.bakedRanges + movedBaked)
                        } else {
                            var copied = original; copied.id = UUID(); copied.automation = movedLegacy
                            copied.parameterAutomation = movedParameters.filter { !$0.value.isEmpty }
                            copied.pitchEdits = PitchEditRegions.relocated(movedEdits, by: 0, renewIDs: true)
                            copied.bakedRanges = movedBaked
                            session.tracks[destinationIndex].realtimeEffects.append(copied)
                        }
                    }
                }
            }
            selectedClips = newIDs; selectedClip = duplicate ? newIDs.first : id
            if let targetTrack { selectedTrack = targetTrack }; selection = nil
        }
    }
    func trackForVerticalMove(from id: UUID, delta: Double) -> UUID? {
        guard abs(delta) > 40, let index = session.tracks.firstIndex(where: { $0.id == id }) else { return nil }
        let heights = session.tracks.map { trackSize.height + (showsAutomation($0) ? 84 : 0) + inlineDrumHeight(for: $0) + inlineSoundHeight(for: $0) }
        let targetY = heights.prefix(index).reduce(0, +) + trackSize.height / 2 + delta
        var top = 0.0
        for (i, height) in heights.enumerated() {
            if targetY >= top && targetY < top + height { return i == index || !session.tracks[i].acceptsClips ? nil : session.tracks[i].id }
            top += height
        }
        return nil
    }
    func deleteSelectedClips(actionName: String = "Delete clips") {
        guard selectedClips.count > 1 else { deleteSelection(actionName: actionName == "Delete clips" ? "Delete audio" : actionName); return }
        edit(actionName) {
            for t in session.tracks.indices { session.tracks[t].clips.removeAll { selectedClips.contains($0.id) } }
            selectedClips = []; selectedClip = nil; selection = nil
        }
    }
    var projectAudioBytes: Int {
        assets.values.reduce(0) { total, audio in
            let samples = audio.count.multipliedReportingOverflow(by: audio.channelCount)
            let bytes = samples.partialValue.multipliedReportingOverflow(by: MemoryLayout<Float>.stride)
            let sum = total.addingReportingOverflow(bytes.partialValue)
            return samples.overflow || bytes.overflow || sum.overflow ? Int.max : sum.partialValue
        }
    }
    var unusedAssetCount: Int {
        let used = session.referencedAssetIDs
        return assets.keys.filter { !used.contains($0) }.count
    }
    func reviewUnusedMedia() {
        guard unusedAssetCount > 0, !busy, !isRecording else { return }
        let alert = NSAlert(); alert.messageText = "Remove \(unusedAssetCount) unused audio files?"
        alert.informativeText = "Original takes that no clip uses will be removed from this session. Undo can restore them until you close the session."
        alert.addButton(withTitle: "Remove unused media"); alert.addButton(withTitle: "Cancel")
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        _ = remoteRoute("media.cleanup")
    }
}
