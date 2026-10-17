import Foundation

/// One non-realtime actor prepares ordinary web messages. Peers retain separate
/// cursors and live state while equal revision documents share serialized bytes.
/// Idle documents are evicted before admission; active leases are never evicted.
actor RemoteSnapshotEncodingPool {
    static let shared = RemoteSnapshotEncodingPool()
    struct Statistics: Sendable {
        var retainedBytes: Int
        var activeLeases: Int
        var entries: Int
        var documentEncodes: Int
        var messageEncodes: Int
    }
    enum Output: Sendable {
        case frame(Data)
        case snapshot(RemoteSnapshotLease)
    }
    struct Result: Sendable {
        var state: RemoteWebDocumentState
        var output: Output
    }
    private struct Entry {
        let id: UUID
        let snapshot: RemoteSnapshot
        let pages: RemoteSnapshotPages
        var active = 0
        var used: UInt64 = 0
        var oversizedDelta: Bool
    }
    private var entries: [Entry] = []
    private var retainedBytes = 0
    private var useSequence: UInt64 = 0
    private var documentEncodes = 0
    private var messageEncodes = 0
    private let maximumSerializedBytes: Int
    private let documentReservationBytes: Int
    private let maximumEntries: Int

    init(
        maximumSerializedBytes: Int = 128 * 1024 * 1024,
        documentReservationBytes: Int = RemoteSnapshotPages.maximumSnapshotBytes,
        maximumEntries: Int = 16
    ) {
        precondition(
            maximumSerializedBytes >= documentReservationBytes && documentReservationBytes > 0 && maximumEntries > 0)
        self.maximumSerializedBytes = maximumSerializedBytes
        self.documentReservationBytes = documentReservationBytes
        self.maximumEntries = maximumEntries
    }

    func encode(_ message: RemoteWireMessage, state original: RemoteWebDocumentState) throws -> Result {
        try Task.checkCancellation()
        var state = original
        try state.apply(message)
        if case .event(.snapshot) = message, let snapshot = state.snapshot {
            return .init(state: state, output: .snapshot(try prepare(snapshot, oversizedDelta: false)))
        }
        // Once one peer proves this change needs pages, other peers reuse that
        // document instead of repeatedly encoding the same oversized track.
        if case .event(.delta(let delta)) = message, !delta.tracks.isEmpty,
            let snapshot = state.snapshot, let index = matching(metadata(snapshot)), entries[index].oversizedDelta
        {
            return .init(state: state, output: .snapshot(try lease(at: index, live: snapshot)))
        }
        messageEncodes += 1
        let output = try state.encodeUpdated(message)
        try Task.checkCancellation()
        switch output {
        case .frame(let bytes): return .init(state: state, output: .frame(bytes))
        case .snapshot(let snapshot):
            return .init(state: state, output: .snapshot(try prepare(snapshot, oversizedDelta: true)))
        }
    }

    func statistics() -> Statistics {
        .init(
            retainedBytes: retainedBytes, activeLeases: entries.reduce(0) { $0 + $1.active }, entries: entries.count,
            documentEncodes: documentEncodes, messageEncodes: messageEncodes)
    }

    private func metadata(_ snapshot: RemoteSnapshot) -> RemoteSnapshot {
        var document = snapshot
        document.transport = .init(hostTime: 0)
        document.status = ""
        document.peers = []
        return document
    }
    private func matching(_ snapshot: RemoteSnapshot) -> Int? {
        entries.firstIndex {
            $0.snapshot.serverID == snapshot.serverID && $0.snapshot.revision == snapshot.revision
                && $0.snapshot == snapshot
        }
    }
    private func prepare(_ snapshot: RemoteSnapshot, oversizedDelta: Bool) throws -> RemoteSnapshotLease {
        let document = metadata(snapshot)
        if let index = matching(document) {
            entries[index].oversizedDelta = entries[index].oversizedDelta || oversizedDelta
            return try lease(at: index, live: snapshot)
        }
        // Reserve the maximum document before serializing, so retained bytes and
        // this worker's new serialized result together cannot exceed the bound.
        while retainedBytes > maximumSerializedBytes - documentReservationBytes || entries.count >= maximumEntries {
            guard
                let index = entries.indices.filter({ entries[$0].active == 0 }).min(by: {
                    entries[$0].used < entries[$1].used
                })
            else {
                throw RemoteError.queueFull
            }
            retainedBytes -= entries[index].pages.bytes.count
            entries.remove(at: index)
        }
        try Task.checkCancellation()
        let pages = try RemoteSnapshotPages(snapshot: document)
        guard pages.bytes.count <= documentReservationBytes else { throw RemoteError.oversizedFrame }
        try Task.checkCancellation()
        documentEncodes += 1
        retainedBytes += pages.bytes.count
        entries.append(.init(id: UUID(), snapshot: document, pages: pages, oversizedDelta: oversizedDelta))
        return try lease(at: entries.count - 1, live: snapshot)
    }
    private func lease(at index: Int, live: RemoteSnapshot) throws -> RemoteSnapshotLease {
        let overlays = try [RemoteEvent.transport(live.transport), .status(live.status), .presence(live.peers)].map {
            try RemoteV3Codec.encodeMessage(.event($0))
        }
        try Task.checkCancellation()
        useSequence &+= 1
        entries[index].used = useSequence
        entries[index].active += 1
        let id = entries[index].id
        return RemoteSnapshotLease(pages: .init(sharing: entries[index].pages), overlays: overlays) { [weak self] in
            Task { await self?.release(id) }
        }
    }
    private func release(_ id: UUID) {
        guard let index = entries.firstIndex(where: { $0.id == id }) else { return }
        precondition(entries[index].active > 0)
        entries[index].active -= 1
    }
}

/// One immutable document lease. Copying a stream retains this token, and the
/// pool releases its active-byte protection only when the final copy is gone.
final class RemoteSnapshotLease: @unchecked Sendable {
    let pages: RemoteSnapshotPages
    let overlays: [Data]
    private let release: @Sendable () -> Void
    init(pages: RemoteSnapshotPages, overlays: [Data], release: @escaping @Sendable () -> Void) {
        self.pages = pages
        self.overlays = overlays
        self.release = release
    }
    deinit { release() }
}
