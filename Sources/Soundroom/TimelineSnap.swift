import Combine
import Foundation
import SoundroomCore
// Only this small overlay observes client-local ruler settings.
import SwiftUI

enum TimelineGrid: String, CaseIterable, Identifiable {
    case off = "Edges"
    case time = "Time grid"
    case beat = "Beat grid"
    var id: Self { self }
    /// What moved clips and selections snap to.
    var title: String { self == .off ? "Clip edges" : rawValue }
}
@MainActor final class TimelineEditingState: ObservableObject {
    var clipSettingsVisible = false
    var contextTrack: UUID?
    var contextClip: UUID?
    var contextDate = Date.distantPast
    @Published var grid = TimelineGrid.off
    @Published var timeDivision: Double = 0.1
    @Published var timebase = TimelineTimebase.time
    @Published var beatDivision: Double = 0.25
    func gridFrames(timing: MusicalTiming) -> Int? {
        switch grid {
        case .off: return nil
        case .time: return max(1, Int(max(0.001, min(60, timeDivision.isFinite ? timeDivision : 0.1)) * sessionRate))
        case .beat: return timing.frame(atQuarterNote: beatDivision == 0 ? timing.quarterNotesPerBar : beatDivision)
        }
    }
}

extension SessionStore {
    func setMusicalTiming(tempo: Double? = nil, beatsPerBar: Int? = nil, beatUnit: Int? = nil) {
        let value = MusicalTiming(
            tempo: tempo ?? session.timing.tempo, beatsPerBar: beatsPerBar ?? session.timing.beatsPerBar,
            beatUnit: beatUnit ?? session.timing.beatUnit)
        guard value.isValid, !isRecording else { return }
        _ = remoteRoute(
            "session.timing",
            integers: (beatsPerBar.map { ["beatsPerBar": $0] } ?? [:]).merging(beatUnit.map { ["beatUnit": $0] } ?? [:])
            { _, new in new }, numbers: tempo.map { ["tempo": $0] } ?? [:])
    }
    func timelinePosition(_ frame: Int) -> String {
        timelineEditing.timebase == .beats ? session.timing.position(frame) : timeString(frame)
    }

    /// Beat grids quantize to absolute musical boundaries; time/edge grids
    /// attract only within eight pixels of an anchor.
    func snappedFrame(_ proposed: Int, excluding: Set<UUID> = [], extraAnchors: [Int] = [], useSelection: Bool = true)
        -> (frame: Int, guide: Int?)
    {
        let frame = max(0, proposed)
        guard snap else { return (frame, nil) }
        if timelineEditing.grid == .beat {
            let division =
                timelineEditing.beatDivision == 0 ? session.timing.quarterNotesPerBar : timelineEditing.beatDivision
            let quantized = session.timing.snapped(frame, quarterNoteDivision: division)
            return (quantized, quantized)
        }
        var anchors =
            [0, playhead] + extraAnchors
            + session.tracks.flatMap(\.clips).filter { !excluding.contains($0.id) }.flatMap { [$0.start, $0.end] }
        if useSelection, let range = selection?.range { anchors += [range.lowerBound, range.upperBound] }
        if let spacing = timelineEditing.gridFrames(timing: session.timing) {
            let remainder = frame % spacing
            anchors.append(frame - remainder)
            if frame - remainder <= Int.max - spacing { anchors.append(frame - remainder + spacing) }
        }
        let threshold = max(1, Int(sessionRate * 8 / max(5, zoom)))
        guard let nearest = anchors.min(by: { abs($0 - frame) < abs($1 - frame) }), abs(nearest - frame) <= threshold
        else { return (frame, nil) }
        return (nearest, nearest)
    }
    func setClipProtection(_ id: UUID, trackID: UUID, enabled: Bool, milliseconds: Double? = nil) {
        guard milliseconds == nil || milliseconds!.isFinite else { return }
        updateTrack(trackID, actionName: "Clip edge protection", coalescingKey: "\(id).protection") { track in
            guard let c = track.clips.firstIndex(where: { $0.id == id }) else { return }
            track.clips[c].renderEnvelope = nil
            track.clips[c].edgeProtectionDisabled = !enabled
            if let milliseconds {
                let frames = Int(max(0, min(100, milliseconds)) / 1000 * sessionRate)
                track.clips[c].edgeFadeFrames = frames
                if track.clips[c].cutFadeInFrames != nil { track.clips[c].cutFadeInFrames = frames }
                if track.clips[c].cutFadeOutFrames != nil { track.clips[c].cutFadeOutFrames = frames }
            }
        }
    }
    /// The right crossfade handle is also the outgoing clip end; it changes overlap without
    /// moving the incoming audio or changing source offsets. One drag is one project undo.
    func setCrossfadeLength(outgoing id: UUID, incoming incomingID: UUID, trackID: UUID, frames: Int) {
        guard let track = session.tracks.first(where: { $0.id == trackID }),
            let outgoing = track.clips.first(where: { $0.id == id }),
            let incoming = track.clips.first(where: { $0.id == incomingID }),
            let audio = assets[outgoing.assetID]
        else { return }
        let maximum = min(incoming.end, outgoing.start + audio.count - outgoing.offset)
        let end = max(incoming.start + 1, min(maximum, incoming.start + max(1, frames)))
        guard end > outgoing.start, end != outgoing.end else { return }
        edit("Resize crossfade") {
            guard let t = session.tracks.firstIndex(where: { $0.id == trackID }),
                let c = session.tracks[t].clips.firstIndex(where: { $0.id == id })
            else { return }
            session.tracks[t].clips[c].length = end - outgoing.start
            session.tracks[t].clips[c].soundDesign = nil
            session.tracks[t].clips[c].resetFadeEnvelope()
        }
    }
}

struct MusicalLaneGrid: View {
    @ObservedObject var store: SessionStore
    @ObservedObject var settings: TimelineEditingState
    var visiblePixels: Range<Double>?
    var body: some View {
        Canvas { context, size in
            guard settings.timebase == .beats else { return }
            let lower = max(0, visiblePixels?.lowerBound ?? 0)
            let upper = min(size.width, visiblePixels?.upperBound ?? size.width)
            let start = Int(lower / store.zoom * sessionRate)
            let end = Int(upper / store.zoom * sessionRate)
            if settings.grid == .beat && settings.beatDivision > 0 {
                var subdivisions = Path()
                for frame in store.session.timing.gridFrames(
                    start: start, end: end, quarterNoteDivision: settings.beatDivision, pixelsPerSecond: store.zoom)
                {
                    let x = Double(frame) / sessionRate * store.zoom
                    subdivisions.move(to: CGPoint(x: x, y: 0))
                    subdivisions.addLine(to: CGPoint(x: x, y: size.height))
                }
                context.stroke(subdivisions, with: .color(Theme.line.opacity(0.2)), lineWidth: 0.5)
            }
            for tick in store.session.timing.rulerTicks(
                start: start, end: end, pixelsPerSecond: store.zoom, minimumPixels: 12)
            {
                let x = Double(tick.frame) / sessionRate * store.zoom
                var line = Path()
                line.move(to: CGPoint(x: x, y: 0))
                line.addLine(to: CGPoint(x: x, y: size.height))
                context.stroke(
                    line, with: .color(tick.bar ? Theme.accent.opacity(0.25) : Theme.line.opacity(0.25)),
                    lineWidth: tick.bar ? 1 : 0.5)
            }
        }
    }
}
