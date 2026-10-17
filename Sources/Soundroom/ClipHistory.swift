import AppKit
import SoundroomCore
import SwiftUI

extension SessionStore {
    /// Call once after a project edit, or after the first update of a continuous gesture.
    func captureClipHistory(before: Session, operation: String) {
        session.captureClipHistory(before: before, operation: operation)
    }
    /// Selection processing replaces clips. Retain the source range as an independently
    /// recoverable state even when the processed selection crosses several original clips.
    func processedClip(
        assetID: UUID, name: String, range: Range<Int>, length: Int, source: Session, operation: String,
        originalAudio: AudioSamples
    ) -> Clip {
        var clip = Clip(assetID: assetID, name: name, start: range.lowerBound, length: length)
        guard let track = source.tracks.first(where: { $0.id == selectedTrack }) else { return clip }
        let overlapping = track.clips.filter { $0.start < range.upperBound && $0.end > range.lowerBound }
        clip.edgeFadeFrames = overlapping.map { $0.edgeFadeFrames ?? 0 }.max() ?? Clip.defaultEdgeFadeFrames
        clip.edgeProtectionDisabled = overlapping.allSatisfy { $0.edgeProtectionDisabled == true }
        if overlapping.count == 1, let original = overlapping.first,
            original.start <= range.lowerBound, original.end >= range.upperBound
        {
            var prior = original
            let lo = max(original.start, range.lowerBound)
            let hi = min(original.end, range.upperBound)
            prior.offset += lo - original.start
            prior.length = max(1, hi - lo)
            prior.preserveHistoryFadeReference(from: original, localOffset: lo - original.start)
            if lo != original.start || hi != original.end { prior.soundDesign = nil }
            clip.previousStates = prior.previousStates
            clip.remember(prior, operation: operation)
        } else if !overlapping.isEmpty {
            // Reuse the worker's dry render. History must never render minutes of PCM on the UI thread.
            let sourceID = UUID()
            assets[sourceID] = originalAudio
            var prior = Clip(
                assetID: sourceID, name: "Original selection", start: range.lowerBound, length: originalAudio.count,
                edgeFadeFrames: 0)
            // Preserve the aligned span, including silence and envelopes already
            // rendered by the worker. Raw offsets would shift it to the new start.
            prior.renderEnvelope = .unattenuated(length: prior.length)
            clip.remember(prior, operation: operation)
        }
        return clip
    }
    func restoreClipState(_ stateID: UUID, clipID: UUID, trackID: UUID, toNewTrack: Bool) {
        guard let original = session.tracks.first(where: { $0.id == trackID })?.clips.first(where: { $0.id == clipID }),
            let state = original.previousStates?.first(where: { $0.id == stateID }), let audio = assets[state.assetID],
            state.offset >= 0, state.length > 0, state.offset <= audio.count - state.length
        else { return }
        edit(toNewTrack ? "Create track from clip history" : "Restore clip state") {
            var restored = state.applying(to: original)
            if toNewTrack {
                restored.id = UUID()
                restored.previousStates = nil
                var track = Track(name: "\(state.name) · \(state.operation)", color: session.tracks.count % 5)
                track.clips = [restored]
                session.tracks.append(track)
                selectedTrack = track.id
                selectedClip = restored.id
                selectedClips = [restored.id]
            } else if let t = session.tracks.firstIndex(where: { $0.id == trackID }),
                let c = session.tracks[t].clips.firstIndex(where: { $0.id == clipID })
            {
                session.tracks[t].clips[c] = restored
                selectedTrack = trackID
                selectedClip = restored.id
                selectedClips = [restored.id]
            }
            selection = nil
        }
    }
}

struct ClipHistoryMenu: View {
    @ObservedObject var store: SessionStore
    let clip: Clip
    let trackID: UUID
    var body: some View {
        Menu("Clip History") {
            let states = Array((clip.previousStates ?? []).reversed())
            if states.isEmpty { Text("No previous states") }
            ForEach(Array(states.enumerated()), id: \.element.id) { index, state in
                Menu("\(index + 1). Before \(state.operation) · \(state.name)") {
                    Button("Restore This State") {
                        store.restoreClipState(state.id, clipID: clip.id, trackID: trackID, toNewTrack: false)
                    }
                    Button("Create New Track From State") {
                        store.restoreClipState(state.id, clipID: clip.id, trackID: trackID, toNewTrack: true)
                    }
                }
            }
        }.disabled(store.busy || store.isRecording)
    }
}
