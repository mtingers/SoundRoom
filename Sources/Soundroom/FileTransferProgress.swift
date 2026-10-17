import Combine
import Foundation

/// Chunk telemetry belongs to one small status view, not the session document.
@MainActor final class FileTransferProgress: ObservableObject {
    enum Phase: String, Hashable {
        case preparing = "Preparing"
        case checking = "Checking"
        case uploading = "Uploading"
        case opening = "Opening"
    }
    struct Snapshot: Equatable {
        let name: String
        let phase: Phase
        let bytes: Int
        let total: Int
        var percent: Int { total > 0 ? Int(min(1, Double(bytes) / Double(total)) * 100) : 0 }
        var label: String { "\(phase.rawValue) \(name)" + (phase == .uploading ? " · \(percent)%" : "…") }
    }
    @Published private(set) var snapshot: Snapshot?
    private var owner: UUID?
    private var publishedAt = -Double.infinity
    var isActive: Bool { owner != nil }
    func begin(name: String) -> UUID {
        let id = UUID()
        owner = id
        publishedAt = -Double.infinity
        update(id, name: name, phase: .preparing)
        return id
    }
    func update(
        _ id: UUID, name: String, phase: Phase, bytes: Int = 0, total: Int = 0,
        now: TimeInterval = ProcessInfo.processInfo.systemUptime
    ) {
        guard owner == id else { return }
        let next = Snapshot(name: name, phase: phase, bytes: max(0, bytes), total: max(0, total))
        guard next != snapshot else { return }
        if let current = snapshot, current.name == name, current.phase == phase {
            guard next.percent != current.percent else { return }
            guard now - publishedAt >= 0.1 || total > 0 && bytes >= total else { return }
        }
        publishedAt = now
        snapshot = next
    }
    func finish(_ id: UUID) {
        guard owner == id else { return }
        owner = nil
        snapshot = nil
    }
}
