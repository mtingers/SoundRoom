import Foundation
import XCTest

@testable import SoundroomRemote

final class RemotePendingRequestsTests: XCTestCase {
    func testSaturatedOrdinaryRequestsLeaveRoomForTransportAndNoteRelease() throws {
        var pending = RemotePendingRequests<Int>()
        let ordinaryLimit = RemoteProtocol.maximumPendingRequests - RemoteProtocol.reservedPendingInterruptRequests
        for index in 0..<ordinaryLimit {
            try pending.insert(index, for: UUID(), command: .ping(clientTime: 0))
        }
        assertFull { try pending.insert(-1, for: UUID(), command: .play(trackOnly: nil)) }
        let interrupts: [RemoteCommand] = [
            .stop, .editor(.init(operation: "transport.pause")),
            .editor(.init(operation: "designer.noteOff")), .editor(.init(operation: "designer.allNotesOff")),
            .editor(.init(operation: "jobs.cancel")), .editor(.init(operation: "audition.stop")),
            .editor(.init(operation: "drums.stop")), .editor(.init(operation: "transport.finishRecording")),
        ]
        XCTAssertEqual(interrupts.count, RemoteProtocol.reservedPendingInterruptRequests)
        for (index, command) in interrupts.enumerated() {
            try pending.insert(index, for: UUID(), command: command)
        }
        XCTAssertEqual(pending.count, RemoteProtocol.maximumPendingRequests)
        assertFull { try pending.insert(-1, for: UUID(), command: .stop) }
    }

    func testDuplicateCannotReplaceOriginalContinuationOrConsumeReserve() throws {
        var pending = RemotePendingRequests<String>()
        let id = UUID()
        try pending.insert("original", for: id, command: .ping(clientTime: 0))
        assertFull { try pending.insert("replacement", for: id, command: .stop) }
        XCTAssertEqual(pending.count, 1)
        XCTAssertEqual(pending.removeValue(forKey: id), "original")
        XCTAssertNil(pending.removeValue(forKey: id))
    }

    func testReceiptOrTimeoutReleasesCapacityAndInterruptsShareTheTotalBound() throws {
        var pending = RemotePendingRequests<Int>()
        let ids = (0..<RemoteProtocol.maximumPendingRequests).map { _ in UUID() }
        for (index, id) in ids.enumerated() { try pending.insert(index, for: id, command: .stop) }
        assertFull { try pending.insert(-1, for: UUID(), command: .ping(clientTime: 0)) }
        for id in ids.prefix(RemoteProtocol.reservedPendingInterruptRequests + 1) {
            XCTAssertNotNil(pending.removeValue(forKey: id))
        }
        try pending.insert(100, for: UUID(), command: .ping(clientTime: 0))
        XCTAssertEqual(
            pending.count, RemoteProtocol.maximumPendingRequests - RemoteProtocol.reservedPendingInterruptRequests)
        assertFull { try pending.insert(-1, for: UUID(), command: .ping(clientTime: 0)) }
        try pending.insert(101, for: UUID(), command: .stop)
    }

    func testDisconnectDrainsEveryContinuationExactlyOnceAndAllowsReconnect() throws {
        var pending = RemotePendingRequests<Int>()
        for index in 0..<RemoteProtocol.maximumPendingRequests {
            try pending.insert(index, for: UUID(), command: .stop)
        }
        XCTAssertEqual(Set(pending.removeAll()), Set(0..<RemoteProtocol.maximumPendingRequests))
        XCTAssertEqual(pending.count, 0)
        XCTAssertTrue(pending.removeAll().isEmpty)
        try pending.insert(1, for: UUID(), command: .ping(clientTime: 0))
        XCTAssertEqual(pending.count, 1)
    }

    private func assertFull(_ operation: () throws -> Void, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertThrowsError(try operation(), file: file, line: line) {
            XCTAssertEqual($0 as? RemoteError, .queueFull, file: file, line: line)
        }
    }
}
