import XCTest

@testable import SoundroomRemote

final class RemoteV3DecodeInboxTests: XCTestCase {
    func testReceiptDeliveryReservationBoundsActorTasksAndReleasesExactlyOnce() throws {
        var budget = RemoteV3ControlDeliveryBudget(maximumBytes: 8, maximumFrames: 2)
        let first = try budget.reserve(4)
        let second = try budget.reserve(4)
        XCTAssertThrowsError(try budget.reserve(1))
        XCTAssertEqual(budget.bytes, 8)
        budget.release(first)
        budget.release(first)
        XCTAssertEqual(budget.bytes, 4)
        XCTAssertEqual(budget.count, 1)
        let next = try budget.reserve(4)
        budget.release(second)
        budget.release(next)
        XCTAssertEqual(budget.bytes, 0)
        XCTAssertEqual(budget.count, 0)
        _ = try budget.reserve(8)
        budget.clear()
        XCTAssertEqual(budget.bytes, 0)
        XCTAssertEqual(budget.count, 0)
    }
    func testBoundedFIFOAdmissionRejectsWithoutLosingQueuedFrames() throws {
        var inbox = RemoteV3DecodeInbox(maximumBytes: 8, maximumFrames: 2)
        try inbox.append(Data([1, 2, 3]))
        try inbox.append(Data([4, 5, 6, 7, 8]))
        XCTAssertThrowsError(try inbox.append(Data([9])))
        XCTAssertEqual(inbox.bytes, 8)
        XCTAssertEqual(inbox.count, 2)
        XCTAssertEqual(inbox.next(), Data([1, 2, 3]))
        XCTAssertEqual(inbox.bytes, 5)
        try inbox.append(Data([9, 10, 11]))
        XCTAssertEqual(inbox.next(), Data([4, 5, 6, 7, 8]))
        XCTAssertEqual(inbox.next(), Data([9, 10, 11]))
        XCTAssertNil(inbox.next())
        XCTAssertEqual(inbox.bytes, 0)
        XCTAssertEqual(inbox.count, 0)
    }

    func testRepeatedPagingCompactsConsumedSlotsAndClearReleasesBacklog() throws {
        var inbox = RemoteV3DecodeInbox(maximumBytes: 1024, maximumFrames: 256)
        for value in 0..<256 { try inbox.append(Data([UInt8(value)])) }
        for value in 0..<200 { XCTAssertEqual(inbox.next(), Data([UInt8(value)])) }
        XCTAssertEqual(inbox.bytes, 56)
        XCTAssertEqual(inbox.count, 56)
        inbox.clear()
        XCTAssertEqual(inbox.count, 0)
        XCTAssertEqual(inbox.bytes, 0)
        try inbox.append(Data([42]))
        XCTAssertEqual(inbox.next(), Data([42]))
    }

    func testImmediateHeartbeatRequiresCanonicalSmallEnvelope() throws {
        let id = UUID()
        for message in [RemoteWireMessage.heartbeat(id), .heartbeatReply(id)] {
            let data = try RemoteV3Codec.encodeMessage(message)
            XCTAssertNotNil(try RemoteV3DecodeInbox.immediateControl(data))
            var fields = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
            fields["version"] = 2
            XCTAssertThrowsError(
                try RemoteV3DecodeInbox.immediateControl(JSONSerialization.data(withJSONObject: fields)))
            fields["version"] = 3
            fields["command"] = ["type": "stop"]
            XCTAssertThrowsError(
                try RemoteV3DecodeInbox.immediateControl(JSONSerialization.data(withJSONObject: fields)))
        }
        let request = RemoteRequest(clientID: UUID(), command: .stop)
        XCTAssertNil(try RemoteV3DecodeInbox.immediateControl(RemoteV3Codec.encodeMessage(.request(request))))
        XCTAssertNil(try RemoteV3DecodeInbox.immediateControl(Data(repeating: 32, count: 4097)))
        let reply = RemoteReply(requestID: request.id, revision: 7)
        XCTAssertNotNil(try RemoteV3DecodeInbox.immediateControl(RemoteV3Codec.encodeMessage(.reply(reply))))
        let payloadReply = RemoteReply(requestID: request.id, revision: 7, payload: Data([1, 2, 3]))
        XCTAssertNil(try RemoteV3DecodeInbox.immediateControl(RemoteV3Codec.encodeMessage(.reply(payloadReply))))
    }
}
