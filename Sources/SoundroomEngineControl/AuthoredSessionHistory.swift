import Foundation
import SoundroomCore

/// Control-thread history shared by hardware hosts. No GUI, device, network or
/// storage work occurs here; callers install the accepted result in their engine.
@MainActor public final class AuthoredSessionHistory {
    public struct Status: Equatable, Sendable {
        public let canUndo: Bool
        public let canRedo: Bool
        public let undoName: String?
        public let redoName: String?
    }
    private struct Change {
        var before: Session
        var after: Session
        var name: String
        var gesture: String?
        var time: Double
        var affected: Set<UUID> {
            let old = AuthoredSessionHistory.entities(before), new = AuthoredSessionHistory.entities(after)
            return Set(old.keys).union(new.keys).filter { old[$0] != new[$0] }
        }
    }
    private var undo: [UUID: [Change]] = [:]
    private var redo: [UUID: [Change]] = [:]
    private let now: () -> Double
    public init(now: @escaping () -> Double = { ProcessInfo.processInfo.systemUptime }) { self.now = now }
    public nonisolated static func entities(_ document: Session) -> [UUID: Track] {
        var result = Dictionary(uniqueKeysWithValues: document.tracks.map { ($0.id, $0) })
        result[Session.masterTrackID] = document.masterTrack
        return result
    }
    public func status(author: UUID) -> Status {
        .init(canUndo: !(undo[author] ?? []).isEmpty, canRedo: !(redo[author] ?? []).isEmpty,
              undoName: undo[author]?.last?.name, redoName: redo[author]?.last?.name)
    }
    public func forget(author: UUID) { undo[author] = nil; redo[author] = nil }
    public func clear() { undo = [:]; redo = [:] }
    /// Include both directions so garbage collection cannot destroy Undo sources.
    public var referencedAssetIDs: Set<UUID> {
        var result: Set<UUID> = []
        for stack in Array(undo.values) + Array(redo.values) {
            for entry in stack {
                result.formUnion(entry.before.referencedAssetIDs)
                result.formUnion(entry.after.referencedAssetIDs)
            }
        }
        return result
    }
    public func continuesGesture(before: Session, after: Session, author: UUID, gesture: String?) -> Bool {
        guard let gesture, let last = undo[author]?.last, last.gesture == gesture, last.after == before,
              now() - last.time < 1 else { return false }
        let change = Change(before: before, after: after, name: "", gesture: gesture, time: now())
        return last.affected == change.affected
    }
    public func remember(before: Session, after: Session, author: UUID, name: String, gesture: String?) {
        guard before != after else { return }
        let change = Change(before: before, after: after, name: name, gesture: gesture, time: now())
        var stack = undo[author] ?? []
        if continuesGesture(before: before, after: after, author: author, gesture: gesture) {
            stack[stack.count - 1].after = after; stack[stack.count - 1].time = change.time
        } else { stack.append(change) }
        if stack.count > 40 { stack.removeFirst(stack.count - 40) }
        undo[author] = stack; redo[author] = []
    }
    /// Reject conflicts before consuming history; retain unrelated clients' work.
    public func apply(to current: Session, author: UUID, reverse: Bool, isRecording: Bool) throws -> Session {
        var source = reverse ? (undo[author] ?? []) : (redo[author] ?? [])
        guard let entry = source.last else { throw HistoryError(reverse ? "There is nothing to undo." : "There is nothing to redo.") }
        let expected = reverse ? entry.after : entry.before, target = reverse ? entry.before : entry.after
        let currentEntities = Self.entities(current), expectedEntities = Self.entities(expected), targetEntities = Self.entities(target)
        guard entry.affected.allSatisfy({ currentEntities[$0] == expectedEntities[$0] }),
              expected.name == target.name || current.name == expected.name else {
            throw HistoryError("Another client changed this edit's tracks. Undo would overwrite their work.")
        }
        var result = current
        if expected.timing != target.timing {
            guard !isRecording else { throw HistoryError("Finish recording before changing project tempo or meter.") }
            guard result.timing == expected.timing else { throw HistoryError("Another client changed the tempo or meter.") }
            result.timing = target.timing
        }
        let reorder = expected.tracks.map(\.id) != target.tracks.map(\.id)
        if reorder && result.tracks.map(\.id) != expected.tracks.map(\.id) { throw HistoryError("Another client changed the track order.") }
        if expected.masteringAnalysis != target.masteringAnalysis {
            guard result.masteringAnalysis == expected.masteringAnalysis else { throw HistoryError("Another client changed the mastering analysis.") }
            result.masteringAnalysis = target.masteringAnalysis
        }
        for id in entry.affected {
            if id == Session.masterTrackID { result.masterTrack = target.masterTrack; continue }
            if let track = targetEntities[id] {
                if let index = result.tracks.firstIndex(where: { $0.id == id }) { result.tracks[index] = track }
                else { let index = min(target.tracks.firstIndex(where: { $0.id == id }) ?? result.tracks.count, result.tracks.count); result.tracks.insert(track, at: index) }
            } else { result.tracks.removeAll { $0.id == id } }
        }
        if reorder { let entities = Self.entities(result); result.tracks = target.tracks.map(\.id).compactMap { entities[$0] } }
        if expected.name != target.name { result.name = target.name }
        result.version = result.requiredProjectVersion
        source.removeLast()
        if reverse { undo[author] = source; redo[author, default: []].append(entry) }
        else { redo[author] = source; undo[author, default: []].append(entry) }
        return result
    }
}

private struct HistoryError: LocalizedError {
    let errorDescription: String?
    init(_ message: String) { errorDescription = message }
}
