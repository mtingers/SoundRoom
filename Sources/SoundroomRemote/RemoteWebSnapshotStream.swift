import Foundation

/// A pinned document retains one serialized copy, but produces only the next
/// bounded frame when the socket can send. Pages never flood the outbound queue.
struct RemoteWebSnapshotStream {
    private let pages: RemoteSnapshotPages
    private let lease: RemoteSnapshotLease?
    private let overlays: [Data]
    private var index = -1
    init(snapshot: RemoteSnapshot) throws {
        pages = try .init(snapshot: snapshot)
        lease = nil
        overlays = []
    }
    init(lease: RemoteSnapshotLease) {
        self.lease = lease
        pages = lease.pages
        overlays = lease.overlays
    }
    var retainedBytes: Int { pages.bytes.count + overlays.reduce(0) { $0 + $1.count } }
    mutating func next() throws -> Data? {
        if index >= 0 && index < overlays.count {
            defer { index += 1 }
            return overlays[index]
        }
        let pageIndex = index - overlays.count
        let page: RemoteSnapshotPage
        if index == -1 {
            page = pages.begin()
        } else if pageIndex < pages.pageCount {
            page = try pages.page(at: pageIndex)
        } else if pageIndex == pages.pageCount {
            page = pages.end()
        } else {
            return nil
        }
        index += 1
        return try JSONEncoder().encode(page)
    }
}
