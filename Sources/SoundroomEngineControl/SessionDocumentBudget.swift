import Foundation
import SoundroomRemote

/// The legacy adapter remains bounded while common clients use paced, verified
/// snapshots. Both encodings are measured on a worker before a session commits.
public enum SessionDocumentTransport: Sendable {
    case legacyNative
    case pagedWeb
    public var maximumDocumentBytes: Int {
        switch self { case .legacyNative: 6 * 1024 * 1024; case .pagedWeb: RemoteSnapshotPages.maximumSnapshotBytes }
    }
}

public enum SessionDocumentBudget {
    public static func validate(_ snapshot: RemoteSnapshot, transport: SessionDocumentTransport) async throws {
        try Task.checkCancellation()
        let worker = Task.detached(priority: .userInitiated) {
            try Task.checkCancellation()
            let count: Int
            switch transport {
            case .legacyNative: count = try JSONEncoder().encode(RemoteEvent.snapshot(snapshot)).count
            case .pagedWeb: count = try RemoteV3Codec.encodeSnapshotDocument(snapshot).count
            }
            try Task.checkCancellation()
            try validateEncodedByteCount(count, transport: transport)
        }
        try await withTaskCancellationHandler(operation: { try await worker.value }, onCancel: { worker.cancel() })
        try Task.checkCancellation()
    }

    static func validateEncodedByteCount(_ count: Int, transport: SessionDocumentTransport) throws {
        guard count <= transport.maximumDocumentBytes else {
            switch transport {
            case .legacyNative:
                throw RemoteError.rejected("This session exceeds the legacy remote document limit (6 MiB). Connect all clients through the common WSS endpoint to edit a larger session.")
            case .pagedWeb:
                throw RemoteError.rejected("This session exceeds the bounded paged document limit (64 MiB). Reduce retained metadata before committing it.")
            }
        }
    }
}
