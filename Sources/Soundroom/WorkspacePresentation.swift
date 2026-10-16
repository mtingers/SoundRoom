import Foundation
import Combine
import SoundroomCore

enum TrackSize: String, CaseIterable, Identifiable {
    case compact = "Compact", comfortable = "Comfortable", large = "Large"
    var id: Self { self }
    var height: Double { switch self { case .compact: return 116; case .comfortable: return 148; case .large: return 196 } }
}

/// One geometry model for waveforms, trim/fade handles and recording overlays.
struct AudioLaneGeometry {
    let height: Double
    var bottom: Double { max(108, height - 8) }
    var center: Double { (36 + bottom) / 2 }
    var amplitude: Double { max(1, (bottom - 36) / 2 - 4) }
}

struct TimelineOverviewGeometry {
    let totalSeconds: Double
    let viewport: TimelineViewport
    let zoom: Double
    let width: Double
    var contentWidth: Double { max(viewport.width, totalSeconds * zoom) }
    var maximumOffset: Double { max(0, contentWidth - viewport.width) }
    var thumbWidth: Double { min(width, max(12, width * viewport.width / max(1, contentWidth))) }
    var thumbX: Double { maximumOffset > 0 ? min(maximumOffset, max(0, viewport.offset)) / maximumOffset * max(0, width - thumbWidth) : 0 }
    func offset(at x: Double, grabOffset: Double) -> Double {
        guard x.isFinite, grabOffset.isFinite, width > thumbWidth else { return 0 }
        return min(1, max(0, (x - grabOffset) / (width - thumbWidth))) * maximumOffset
    }
}

struct EditingScope: Equatable {
    let trackName: String
    let detail: String
    let range: Range<Int>?
    var timeLabel: String? { range.map { "\(timeString($0.lowerBound)) – \(timeString($0.upperBound))" } }
}

enum ProjectSaveStatus: String {
    case unverified = "On audio server", saved = "Saved", modified = "Unsaved changes"
}

@MainActor final class WorkspaceActivity: ObservableObject {
    enum Kind: Equatable { case information, success, failure }
    struct Entry: Identifiable {
        let id = UUID()
        let date = Date()
        let message: String
        let kind: Kind
        let recoveryTitle: String?
        let recovery: (() -> Void)?
    }
    @Published private(set) var entries: [Entry] = []
    @Published private(set) var unreadFailures = 0
    func record(_ message: String, kind: Kind = .information, recoveryTitle: String? = nil, recovery: (() -> Void)? = nil) {
        guard !message.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        var replacingFailure = false
        if let last = entries.first, last.message == message {
            if (last.kind == kind && recovery == nil) || kind == .information { return }
            replacingFailure = last.kind == .failure
            entries.removeFirst()
        }
        entries.insert(Entry(message: String(message.prefix(4096)), kind: kind, recoveryTitle: recoveryTitle, recovery: recovery), at: 0)
        while entries.count > 100 {
            if let index = entries.lastIndex(where: { $0.kind != .failure }) { entries.remove(at: index) }
            else { entries.removeLast() }
        }
        if kind == .failure, !replacingFailure { unreadFailures = min(100, unreadFailures + 1) }
    }
    func markRead() { unreadFailures = 0 }
    func clear() { entries.removeAll(); unreadFailures = 0 }
}

extension SessionStore {
    var editingScope: EditingScope {
        guard let track = currentTrack else { return .init(trackName: "No track selected", detail: "Select a track or audio clip", range: nil) }
        if track.id == Session.masterTrackID { return .init(trackName: "Master", detail: "Entire mix · live processing", range: nil) }
        if let selection, selection.trackID == track.id, !selection.range.isEmpty {
            return .init(trackName: track.name, detail: "Selected audio", range: selection.range)
        }
        if let clip = currentClip { return .init(trackName: track.name, detail: clip.name, range: clip.start..<clip.end) }
        return .init(trackName: track.name, detail: "Whole track · live processing", range: nil)
    }
    func requestTimelineScroll(_ offset: Double) {
        guard offset.isFinite else { return }
        followPlayhead = false
        timelineScrollOffset = max(0, offset)
        timelineScrollSerial &+= 1
    }
    func recordFailure(_ message: String) {
        status = message
        activity.record(message, kind: .failure)
    }
}
