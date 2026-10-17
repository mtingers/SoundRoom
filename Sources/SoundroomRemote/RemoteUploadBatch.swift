import Foundation

/// A small ordered burst removes per-chunk network round trips without changing
/// the upload protocol. Registration and sending are synchronous on the owner;
/// independently scheduled Tasks would reorder the server's strict offsets.
@MainActor enum RemoteUploadBatch {
    typealias Completion = (Result<RemoteReply, Error>) -> Void
    typealias Enqueue = (RemoteCommand, @escaping Completion) throws -> Void

    static func run(id: UUID, offset: Int, chunks: [Data], enqueue: Enqueue) async throws -> [RemoteReply] {
        guard (1...3).contains(chunks.count), offset >= 0,
            chunks.allSatisfy({ !$0.isEmpty && $0.count <= RemoteProtocol.maximumUploadChunkBytes })
        else { throw RemoteError.invalidFrame }
        var next = offset
        var commands: [RemoteCommand] = []
        for chunk in chunks {
            let end = next.addingReportingOverflow(chunk.count)
            guard !end.overflow, end.partialValue <= 2 * 1024 * 1024 * 1024 else { throw RemoteError.invalidFrame }
            commands.append(.uploadChunk(id: id, offset: next, data: chunk))
            next = end.partialValue
        }
        try Task.checkCancellation()
        return try await withCheckedThrowingContinuation { continuation in
            var replies = [RemoteReply?](repeating: nil, count: commands.count)
            var remaining = commands.count
            var finished = false
            for (index, command) in commands.enumerated() {
                guard !finished else { break }
                do {
                    try enqueue(command) { result in
                        guard !finished, replies[index] == nil else { return }
                        switch result {
                        case .failure(let error):
                            finished = true
                            continuation.resume(throwing: error)
                        case .success(let reply):
                            replies[index] = reply
                            remaining -= 1
                            if remaining == 0 {
                                finished = true
                                continuation.resume(returning: replies.compactMap { $0 })
                            }
                        }
                    }
                } catch {
                    if !finished {
                        finished = true
                        continuation.resume(throwing: error)
                    }
                    break
                }
            }
        }
    }
}
