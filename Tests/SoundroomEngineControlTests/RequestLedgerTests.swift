import XCTest
import SoundroomRemote
@testable import SoundroomEngineControl

final class RequestLedgerTests: XCTestCase {
    func testFingerprintCanonicalizesRevisionOrderAndIncludesRolePayloadAndPreconditions() throws {
        let client = UUID(), a = UUID(), b = UUID(), id = UUID()
        var first = RemoteRequest(id: id, clientID: client, expectedRevision: 4, expectedTrackRevisions: [a: 2, b: 3], command: .addTrack(name: "Bass"))
        var second = first; second.expectedTrackRevisions = [b: 3, a: 2]
        let fingerprint = try RequestLedger.fingerprint(first, role: .editor)
        XCTAssertEqual(fingerprint, try RequestLedger.fingerprint(second, role: .editor))
        XCTAssertNotEqual(fingerprint, try RequestLedger.fingerprint(second, role: .viewer))
        second.expectedTrackRevisions[a] = 3
        XCTAssertNotEqual(fingerprint, try RequestLedger.fingerprint(second, role: .editor))
        first.command = .uploadChunk(id: id, offset: 0, data: Data([1, 2, 3]))
        second = first; second.command = .uploadChunk(id: id, offset: 0, data: Data([1, 2, 4]))
        XCTAssertNotEqual(try RequestLedger.fingerprint(first, role: .editor), try RequestLedger.fingerprint(second, role: .editor))
        first.command = .editor(.init(operation: "master.applyProposal", payload: Data([1, 2, 3])))
        second = first; second.command = .editor(.init(operation: "master.applyProposal", payload: Data([1, 2, 4])))
        XCTAssertNotEqual(try RequestLedger.fingerprint(first, role: .editor), try RequestLedger.fingerprint(second, role: .editor))
    }

    @MainActor func testInflightRetryRunsOnceAndChangedContentCannotAttach() async {
        let ledger = RequestLedger(), client = UUID(), id = UUID()
        let started = Gate(), release = Gate(), retryStarted = Gate()
        var runs = 0
        let original = RemoteReply(requestID: id, revision: 1, message: "Committed", payload: Data([4]))
        let first = Task { @MainActor in
            await ledger.perform(clientID: client, requestID: id, digest: Data([1]), revision: 0, protectedClients: []) {
                runs += 1; started.open(); await release.wait(); return original
            }
        }
        await started.wait()
        let retry = Task { @MainActor in
            retryStarted.open()
            return await ledger.perform(clientID: client, requestID: id, digest: Data([1]), revision: 0, protectedClients: []) {
                runs += 1; return original
            }
        }
        await retryStarted.wait()
        let changed = await ledger.perform(clientID: client, requestID: id, digest: Data([2]), revision: 0, protectedClients: []) {
            runs += 1; return original
        }
        XCTAssertEqual(changed.status, .rejected); XCTAssertEqual(ledger.inFlightCount, 1)
        release.open()
        let accepted = await first.value, repeated = await retry.value
        XCTAssertEqual(accepted, original); XCTAssertEqual(repeated, original)
        XCTAssertEqual(runs, 1); XCTAssertEqual(ledger.inFlightCount, 0)
    }

    @MainActor func testReplyBudgetsRetainOutcomeWithoutReexecutingLargeResults() async {
        var limits = RequestLedger.Limits(); limits.replyBytesPerClient = 8; limits.replyBytesTotal = 12
        let ledger = RequestLedger(limits: limits), first = UUID(), second = UUID(), a = UUID(), b = UUID(), c = UUID()
        var runs = 0
        for (client, id) in [(first, a), (first, b), (second, c)] {
            let reply = await ledger.perform(clientID: client, requestID: id, digest: Data([1]), revision: 0, protectedClients: []) {
                runs += 1; return .init(requestID: id, revision: runs, payload: Data(repeating: 7, count: 8))
            }
            XCTAssertTrue(reply.accepted); XCTAssertEqual(reply.payload?.count, 8)
            XCTAssertLessThanOrEqual(ledger.retainedReplyBytes, 12)
        }
        let replay = await ledger.perform(clientID: first, requestID: a, digest: Data([1]), revision: 3, protectedClients: []) {
            runs += 1; return .init(requestID: a, revision: runs)
        }
        XCTAssertTrue(replay.accepted); XCTAssertEqual(replay.revision, 1); XCTAssertNil(replay.payload)
        XCTAssertTrue(replay.message.contains("payload has expired")); XCTAssertEqual(runs, 3)
        XCTAssertEqual(ledger.receiptCount, 3); XCTAssertEqual(ledger.retainedReplyBytes, 8)
        ledger.forget(clientID: second)
        XCTAssertEqual(ledger.retainedReplyBytes, 0); XCTAssertEqual(ledger.receiptCount, 2)
    }

    @MainActor func testBoundedHistoryEvictsOnlyInactiveClientsAndKeepsExecutingOperations() async {
        var limits = RequestLedger.Limits(); limits.clients = 1; limits.receiptsPerClient = 2
        let ledger = RequestLedger(limits: limits), first = UUID(), second = UUID()
        var evicted: [UUID] = []
        for _ in 0..<3 {
            let id = UUID()
            _ = await ledger.perform(clientID: first, requestID: id, digest: Data([1]), revision: 0, protectedClients: []) { .init(requestID: id, revision: 0) }
        }
        XCTAssertEqual(ledger.receiptCount, 2)
        let id = UUID()
        let blocked = await ledger.perform(clientID: second, requestID: id, digest: Data([1]), revision: 0, protectedClients: [first]) { .init(requestID: id, revision: 0) }
        XCTAssertEqual(blocked.status, .busy); XCTAssertEqual(ledger.clientCount, 1)
        let admitted = await ledger.perform(clientID: second, requestID: id, digest: Data([1]), revision: 0, protectedClients: [], onEvict: { evicted.append($0) }) { .init(requestID: id, revision: 0) }
        XCTAssertTrue(admitted.accepted); XCTAssertEqual(evicted, [first])
        XCTAssertEqual(ledger.clientCount, 1); XCTAssertEqual(ledger.receiptCount, 1)
    }

    @MainActor func testStopReserveAndBusyRetriesRemainAvailable() async {
        var limits = RequestLedger.Limits(); limits.inFlightTotal = 1; limits.inFlightPerClient = 1; limits.interruptReserve = 1
        let ledger = RequestLedger(limits: limits), client = UUID(), normal = UUID(), stop = UUID(), retry = UUID()
        let started = Gate(), release = Gate()
        let running = Task { @MainActor in
            await ledger.perform(clientID: client, requestID: normal, digest: Data([1]), revision: 0, protectedClients: []) {
                started.open(); await release.wait(); return .init(requestID: normal, revision: 1)
            }
        }
        await started.wait()
        let busy = await ledger.perform(clientID: client, requestID: retry, digest: Data([2]), revision: 0, protectedClients: []) { .init(requestID: retry, revision: 0) }
        XCTAssertEqual(busy.status, .busy)
        let stopped = await ledger.perform(clientID: client, requestID: stop, digest: Data([3]), revision: 0, protectedClients: [], interrupt: true) {
            release.open(); return .init(requestID: stop, revision: 0)
        }
        XCTAssertTrue(stopped.accepted)
        _ = await running.value
        let retried = await ledger.perform(clientID: client, requestID: retry, digest: Data([2]), revision: 1, protectedClients: []) { .init(requestID: retry, revision: 1) }
        XCTAssertTrue(retried.accepted)
        let transient = UUID()
        let notAdmitted = await ledger.perform(clientID: client, requestID: transient, digest: Data([4]), revision: 1, protectedClients: []) { .init(requestID: transient, status: .busy, revision: 1) }
        XCTAssertEqual(notAdmitted.status, .busy)
        let nowAdmitted = await ledger.perform(clientID: client, requestID: transient, digest: Data([4]), revision: 1, protectedClients: []) { .init(requestID: transient, revision: 2) }
        XCTAssertTrue(nowAdmitted.accepted)
    }

    @MainActor func testClientInterruptReserveCannotBeConsumedByOrdinaryRequestsOrGrowUnbounded() async {
        var limits = RequestLedger.Limits()
        limits.inFlightPerClient = 1; limits.interruptsPerClient = 1
        limits.inFlightTotal = 4; limits.interruptReserve = 2
        let ledger = RequestLedger(limits: limits), client = UUID(), other = UUID()
        let ordinaryStarted = Gate(), interruptStarted = Gate(), release = Gate()
        let normalID = UUID(), interruptID = UUID()
        let ordinary = Task { @MainActor in
            await ledger.perform(clientID: client, requestID: normalID, digest: Data([1]), revision: 0, protectedClients: []) {
                ordinaryStarted.open(); await release.wait(); return .init(requestID: normalID, revision: 0)
            }
        }
        await ordinaryStarted.wait()
        let interrupt = Task { @MainActor in
            await ledger.perform(clientID: client, requestID: interruptID, digest: Data([2]), revision: 0, protectedClients: [], interrupt: true) {
                interruptStarted.open(); await release.wait(); return .init(requestID: interruptID, revision: 0)
            }
        }
        await interruptStarted.wait()
        for priority in [false, true] {
            let id = UUID()
            let rejected = await ledger.perform(clientID: client, requestID: id, digest: Data([3]), revision: 0, protectedClients: [], interrupt: priority) {
                XCTFail("Exceeded per-client admission"); return .init(requestID: id, revision: 0)
            }
            XCTAssertEqual(rejected.status, .busy)
        }
        let otherID = UUID()
        let admitted = await ledger.perform(clientID: other, requestID: otherID, digest: Data([4]), revision: 0, protectedClients: [], interrupt: true) {
            .init(requestID: otherID, revision: 0)
        }
        XCTAssertTrue(admitted.accepted); XCTAssertEqual(ledger.inFlightCount, 2)
        release.open(); _ = await ordinary.value; _ = await interrupt.value
        XCTAssertEqual(ledger.inFlightCount, 0)
        let nextID = UUID()
        let next = await ledger.perform(clientID: client, requestID: nextID, digest: Data([5]), revision: 0, protectedClients: []) {
            .init(requestID: nextID, revision: 0)
        }
        XCTAssertTrue(next.accepted)
    }
}

@MainActor private final class Gate {
    private var opened = false
    private var waiting: [CheckedContinuation<Void, Never>] = []
    func wait() async { if opened { return }; await withCheckedContinuation { waiting.append($0) } }
    func open() { opened = true; let pending = waiting; waiting = []; for continuation in pending { continuation.resume() } }
}
