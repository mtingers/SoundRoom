import CryptoKit
import Foundation
import SoundroomRemote

/// Runtime retry receipts for an authenticated hardware host. Large command
/// serialization belongs on workers; the control actor retains only digests,
/// bounded replies and tasks that are actually executing.
@MainActor public final class RequestLedger {
    public struct Limits: Sendable {
        public var clients = 64
        public var receiptsPerClient = 256
        public var inFlightPerClient = 16
        public var inFlightTotal = 128
        public var interruptReserve = 16
        public var interruptsPerClient = 8
        public var replyBytesPerClient = 4 * 1024 * 1024
        public var replyBytesTotal = 32 * 1024 * 1024
        public init() {}
    }
    private struct Key: Hashable {
        let client: UUID
        let request: UUID
    }
    private enum Result {
        case running(Task<RemoteReply, Never>)
        case completed(RemoteReply, bytes: Int, order: UInt64)
    }
    private struct Entry {
        let digest: Data
        var result: Result
        var isRunning: Bool {
            if case .running = result { return true }
            return false
        }
    }
    private var entries: [Key: Entry] = [:]
    private var completed: [UUID: [UUID]] = [:]
    private var clients: Set<UUID> = []
    private var executingByClient: [UUID: Int] = [:]
    private var bytesByClient: [UUID: Int] = [:]
    private var nextOrder: UInt64 = 0
    private let limits: Limits
    public private(set) var retainedReplyBytes = 0
    public private(set) var inFlightCount = 0
    public var clientCount: Int { clients.count }
    public var receiptCount: Int { completed.values.reduce(0) { $0 + $1.count } }

    public init(limits: Limits = .init()) {
        precondition(
            limits.clients > 0 && limits.receiptsPerClient > 0 && limits.inFlightPerClient > 0
                && limits.inFlightTotal > 0)
        precondition(
            limits.replyBytesPerClient >= 0 && limits.replyBytesTotal >= 0 && limits.interruptReserve >= 0
                && limits.interruptsPerClient >= 0)
        precondition(limits.inFlightTotal <= Int.max - limits.interruptReserve)
        precondition(limits.inFlightPerClient <= Int.max - min(limits.interruptReserve, limits.interruptsPerClient))
        self.limits = limits
    }

    /// Called after peer identity/permissions have been checked. Duplicate
    /// in-flight requests await the same operation; changed content is rejected.
    public func perform(
        clientID: UUID, requestID: UUID, digest: Data, revision: Int,
        protectedClients: Set<UUID>, interrupt: Bool = false, onEvict: (UUID) -> Void = { _ in },
        operation: @escaping @MainActor () async -> RemoteReply
    ) async -> RemoteReply {
        let key = Key(client: clientID, request: requestID)
        if let entry = entries[key] {
            guard entry.digest == digest else {
                return .init(
                    requestID: requestID, status: .rejected, revision: revision,
                    message: "This request ID was already used for different content. Use a new request ID.")
            }
            switch entry.result {
            case .running(let task): return await task.value
            case .completed(let reply, _, _): return reply
            }
        }
        let clientInFlight = executingByClient[clientID] ?? 0
        let admission = limits.inFlightTotal + (interrupt ? limits.interruptReserve : 0)
        // A saturated client must still be able to release notes or stop its
        // transport. Both its own reserve and the shared reserve stay bounded.
        let clientAdmission =
            limits.inFlightPerClient + (interrupt ? min(limits.interruptsPerClient, limits.interruptReserve) : 0)
        guard clientInFlight < clientAdmission && inFlightCount < admission else {
            return .init(
                requestID: requestID, status: .busy, revision: revision,
                message: "Too many requests are executing. Retry later.")
        }
        if !clients.contains(clientID) && clients.count >= limits.clients {
            guard
                let expired = clients.sorted(by: { $0.uuidString < $1.uuidString }).first(where: { candidate in
                    !protectedClients.contains(candidate)
                        && !entries.contains(where: { $0.key.client == candidate && $0.value.isRunning })
                })
            else {
                return .init(
                    requestID: requestID, status: .busy, revision: revision,
                    message: "Retry history is full. Retry after an inactive client disconnects.")
            }
            forget(clientID: expired)
            onEvict(expired)
        }
        clients.insert(clientID)
        inFlightCount += 1
        executingByClient[clientID, default: 0] += 1
        let task = Task { @MainActor in
            let reply = await operation()
            self.finish(key, digest: digest, reply: reply)
            return reply
        }
        // The task starts after this actor yields; reservation precedes execution.
        entries[key] = Entry(digest: digest, result: .running(task))
        return await task.value
    }

    public func forget(clientID: UUID) {
        // An executing operation owns its receipt until it completes.
        guard !entries.contains(where: { $0.key.client == clientID && $0.value.isRunning }) else { return }
        for key in entries.keys.filter({ $0.client == clientID }) { remove(key) }
        completed[clientID] = nil
        clients.remove(clientID)
        executingByClient[clientID] = nil
        bytesByClient[clientID] = nil
    }

    private func finish(_ key: Key, digest: Data, reply: RemoteReply) {
        inFlightCount -= 1
        executingByClient[key.client, default: 0] -= 1
        // Busy means the command was not admitted. A client may retry the same
        // request once the competing job ends; committed outcomes stay deduped.
        if reply.status == .busy {
            entries[key] = nil
            return
        }
        var retained = reply
        retained.message = String(retained.message.prefix(2048))
        let bytes = retained.payload?.count ?? 0
        nextOrder += 1
        entries[key] = Entry(digest: digest, result: .completed(retained, bytes: bytes, order: nextOrder))
        retainedReplyBytes += bytes
        bytesByClient[key.client, default: 0] += bytes
        completed[key.client, default: []].append(key.request)
        while completed[key.client]!.count > limits.receiptsPerClient {
            let old = completed[key.client]!.removeFirst()
            remove(Key(client: key.client, request: old))
        }
        while (bytesByClient[key.client] ?? 0) > limits.replyBytesPerClient {
            guard let oldest = oldestPayload(client: key.client) else { break }
            expirePayload(oldest)
        }
        while retainedReplyBytes > limits.replyBytesTotal {
            guard let oldest = oldestPayload(client: nil) else { break }
            expirePayload(oldest)
        }
    }
    private func oldestPayload(client: UUID?) -> Key? {
        var oldest: (Key, UInt64)?
        for (key, entry) in entries {
            guard client == nil || key.client == client, case .completed(_, let bytes, let order) = entry.result,
                bytes > 0
            else { continue }
            if oldest == nil || order < oldest!.1 { oldest = (key, order) }
        }
        return oldest?.0
    }
    private func expirePayload(_ key: Key) {
        guard var entry = entries[key], case .completed(let original, let bytes, let order) = entry.result else {
            return
        }
        // Keep a digest and explicit outcome tombstone. Never silently execute an
        // already-committed mutation because a large reply left the memory budget.
        let receipt = RemoteReply(
            requestID: original.requestID, status: original.status, revision: original.revision,
            message:
                "This request already completed (\(original.status.rawValue)) at revision \(original.revision), but its result payload has expired. Refresh state; repeat a read with a new request ID."
        )
        entry.result = .completed(receipt, bytes: 0, order: order)
        entries[key] = entry
        retainedReplyBytes -= bytes
        bytesByClient[key.client, default: 0] -= bytes
    }
    private func remove(_ key: Key) {
        if let old = entries.removeValue(forKey: key), case .completed(_, let bytes, _) = old.result {
            retainedReplyBytes -= bytes
            bytesByClient[key.client, default: 0] -= bytes
        }
    }

    /// All UUID-keyed revision maps are sorted explicitly; sorted JSON object keys
    /// alone cannot canonicalize Swift's alternating-array dictionary encoding.
    public nonisolated static func fingerprint(_ request: RemoteRequest, role: RemoteRole) throws -> Data {
        struct Version: Encodable {
            let id: String
            let revision: Int
        }
        struct Content: Encodable {
            let clientID: UUID
            let role: RemoteRole
            let expectedRevision: Int?
            let tracks: [Version]
            let command: RemoteCommand
        }
        var command = request.command
        var bytes: Data?
        switch command {
        case .uploadChunk(let id, let offset, let data):
            command = .uploadChunk(id: id, offset: offset, data: Data())
            bytes = data
        case .editor(var intent):
            bytes = intent.payload
            if bytes != nil { intent.payload = Data() }
            command = .editor(intent)
        default: break
        }
        let tracks = request.expectedTrackRevisions.map { Version(id: $0.key.uuidString, revision: $0.value) }.sorted {
            $0.id < $1.id
        }
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        let data = try encoder.encode(
            Content(
                clientID: request.clientID, role: role, expectedRevision: request.expectedRevision, tracks: tracks,
                command: command))
        var hash = SHA256()
        hash.update(data: data)
        if let bytes { hash.update(data: bytes) }
        return Data(hash.finalize())
    }
}
