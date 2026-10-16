import XCTest
import Foundation
@testable import SoundroomRemote

final class RemoteAutomationWriteContractTests: XCTestCase {
    func testSharedWriteFixturesRejectUnsafeDomainsAndClientCursor() throws {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let fixtures = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: root.appendingPathComponent("protocol/fixtures/automation-write-models.v3.json"))) as? [[String: Any]])
        for (index, fixture) in fixtures.enumerated() {
            let type = try XCTUnwrap(fixture["type"] as? String)
            let data = try JSONSerialization.data(withJSONObject: XCTUnwrap(fixture["value"]))
            if fixture["valid"] as? Bool == true { XCTAssertNoThrow(try RemoteV3ModelValidator.validateJSON(data, type: type), "Fixture \(index): \(type)") }
            else { XCTAssertThrowsError(try RemoteV3ModelValidator.validateJSON(data, type: type), "Fixture \(index): \(type)") }
        }
    }
    func testTypedRequestsAndEventRoundTripWithExactBeginFenceAndNoHostClaim() throws {
        let id = UUID()
        let target = RemoteAutomationWriteTarget(kind: .volume)
        let begin = RemoteAutomationWriteBeginRequest(gestureID: id, trackID: id, target: target, mode: .touch)
        let intent = RemoteEditorIntent(operation: "automation.writeBegin", payload: try JSONEncoder().encode(begin))
        let request = RemoteRequest(clientID: id, expectedTrackRevisions: [id: 2], command: .editor(intent))
        let encoded = try RemoteV3Codec.encodeMessage(.request(request))
        guard case .request(let decoded) = try RemoteV3Codec.decodeMessage(encoded) else { return XCTFail() }
        XCTAssertEqual(decoded.id, request.id)
        XCTAssertEqual(decoded.clientID, request.clientID)
        XCTAssertEqual(decoded.expectedTrackRevisions, request.expectedTrackRevisions)
        guard case .editor(let restoredIntent) = decoded.command else { return XCTFail() }
        XCTAssertEqual(restoredIntent.operation, intent.operation)
        XCTAssertEqual(try JSONDecoder().decode(RemoteAutomationWriteBeginRequest.self, from: XCTUnwrap(restoredIntent.payload)), begin)
        XCTAssertThrowsError(try RemoteV3Codec.encodeMessage(.request(.init(clientID: id, command: .editor(intent)))))
        let cursorIntent = RemoteEditorIntent(operation: "automation.writeBegin", context: .init(cursorFrame: 42), payload: try JSONEncoder().encode(begin))
        XCTAssertThrowsError(try RemoteV3Codec.encodeMessage(.request(.init(clientID: id, expectedTrackRevisions: [id: 2], command: .editor(cursorIntent)))))
        var wire = try XCTUnwrap(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
        wire["expectedRevision"] = 2
        XCTAssertThrowsError(try RemoteV3Codec.decodeMessage(JSONSerialization.data(withJSONObject: wire)))
        XCTAssertFalse(RemoteCapabilities().editorOperations.contains(where: { $0.hasPrefix("automation.write") }))
        XCTAssertFalse(RemoteCapabilities().features.contains("automation-write"))
        let update = RemoteAutomationWriteLeaseUpdate(gestureID: id, phase: .conflict, reason: "Review the applied prefix", draftID: id)
        guard case .event(.automationWrite(let restored)) = try RemoteV3Codec.decodeMessage(RemoteV3Codec.encodeMessage(.event(.automationWrite(update: update)))) else { return XCTFail() }
        XCTAssertEqual(restored, update)
    }
}
