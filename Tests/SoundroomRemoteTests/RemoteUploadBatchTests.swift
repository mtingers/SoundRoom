import XCTest
@testable import SoundroomRemote

@MainActor final class RemoteUploadBatchTests: XCTestCase {
    func testBurstEnqueuesExactContiguousBytesBeforeAnyAcknowledgmentAndRetainsReplyOrder() async throws {
        let id = UUID(), chunks = [Data([1, 2]), Data([3, 4, 5]), Data([6])]
        var offsets: [Int] = [], bytes = Data(), callbacks: [RemoteUploadBatch.Completion] = []
        let task = Task {
            try await RemoteUploadBatch.run(id: id, offset: 7, chunks: chunks) { command, completion in
                guard case .uploadChunk(let upload, let offset, let data) = command else { return XCTFail() }
                XCTAssertEqual(upload, id); offsets.append(offset); bytes.append(data); callbacks.append(completion)
            }
        }
        await Task.yield()
        XCTAssertEqual(offsets, [7, 9, 12]); XCTAssertEqual(bytes, Data([1, 2, 3, 4, 5, 6]))
        XCTAssertEqual(callbacks.count, 3)
        for index in [2, 0, 1] { callbacks[index](.success(.init(requestID: UUID(), revision: index))) }
        let replies = try await task.value
        XCTAssertEqual(replies.map(\.revision), [0, 1, 2])
    }

    func testInvalidBurstsSendNothingAndEnqueueFailureOrDisconnectCompletesOnce() async throws {
        for (offset, chunks) in [(0, []), (-1, [Data([1])]), (0, [Data()]),
                                 (0, Array(repeating: Data([1]), count: 4)),
                                 (Int.max, [Data([1])]), (0, [Data(repeating: 0, count: 262145)])] {
            var sent = 0
            do { _ = try await RemoteUploadBatch.run(id: UUID(), offset: offset, chunks: chunks) { _, _ in sent += 1 }; XCTFail() }
            catch { XCTAssertEqual(sent, 0) }
        }
        for synchronous in [false, true] {
            var count = 0, callbacks: [RemoteUploadBatch.Completion] = []
            let task = Task {
                try await RemoteUploadBatch.run(id: UUID(), offset: 0, chunks: [Data([1]), Data([2]), Data([3])]) { _, completion in
                    count += 1
                    if synchronous && count == 2 { throw RemoteError.queueFull }
                    callbacks.append(completion)
                }
            }
            await Task.yield()
            if !synchronous { callbacks[1](.failure(RemoteError.connectionLost)) }
            do { _ = try await task.value; XCTFail() } catch {}
            XCTAssertEqual(count, synchronous ? 2 : 3)
            // Late acknowledgments/timeouts after terminal failure cannot
            // double-resume the batch continuation or start more transmission.
            for callback in callbacks { callback(.failure(RemoteError.connectionLost)) }
        }
    }

    func testInjectedLatencyUsesThreeBoundedRequestsInsteadOfEightRoundTrips() async throws {
        var measurements: [[String: Any]] = []
        for window in [1, 3] {
            var outstanding = 0, peak = 0, sent = 0, completed = 0
            let started = ProcessInfo.processInfo.systemUptime
            for first in stride(from: 0, to: 8, by: window) {
                let chunks = Array(repeating: Data([1]), count: min(window, 8 - first))
                _ = try await RemoteUploadBatch.run(id: UUID(), offset: first, chunks: chunks) { _, completion in
                    sent += 1; outstanding += 1; peak = max(peak, outstanding)
                    Task {
                        try? await Task.sleep(nanoseconds: 40_000_000)
                        outstanding -= 1; completed += 1
                        completion(.success(.init(requestID: UUID(), revision: 0)))
                    }
                }
                XCTAssertEqual(outstanding, 0)
            }
            XCTAssertEqual(sent, 8); XCTAssertEqual(completed, 8); XCTAssertEqual(peak, window)
            measurements.append(["window": window, "seconds": ProcessInfo.processInfo.systemUptime - started,
                                 "chunks": sent, "peakRequests": peak, "injectedAckMilliseconds": 40])
        }
        let data = try JSONSerialization.data(withJSONObject: measurements, options: [.prettyPrinted, .sortedKeys])
        if let path = ProcessInfo.processInfo.environment["SOUNDROOM_UPLOAD_PERF_REPORT"] { try data.write(to: URL(fileURLWithPath: path)) }
        print("UPLOAD_BURST_PERFORMANCE " + String(decoding: data, as: UTF8.self))
    }
}
