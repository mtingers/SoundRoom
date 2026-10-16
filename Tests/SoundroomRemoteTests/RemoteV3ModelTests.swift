import XCTest
import Foundation
import SoundroomCore
@testable import SoundroomRemote

final class RemoteV3ModelTests: XCTestCase {
    func testSnapshotVersionsOneThroughNineRoundTripWithNestedModelGrammar() throws {
        var session = Session()
        var pattern = OrganicPatch.cathedral.applying(to: SoundDesignPattern())
        pattern.notes = [SoundDesignNote(pitch: 60, startBeat: 0)]
        var clip = Clip(assetID: UUID(), name: "Voice", start: 0, length: 48000)
        clip.soundDesign = pattern
        var track = Track(name: "Voice", color: 0); track.clips = [clip]
        clip.renderEnvelope = ClipRenderEnvelope(try XCTUnwrap(ClipEnvelope.resolve(track: track)[clip.id]))
        clip.renderEnvelope?.crossfadeIn = 0..<128
        session.tracks[0].clips = [clip]
        for version in 1...9 {
            session.version = version
            let data = try RemoteV3Codec.encodeSnapshotDocument(.init(serverID: UUID(), serverName: "Host", session: session))
            XCTAssertNoThrow(try RemoteV3ModelValidator.validateJSON(data, type: "RemoteSnapshot"))
            XCTAssertEqual(try RemoteV3Codec.decodeSnapshotDocument(data).session, session)
        }
    }
    func testNestedGrammarRejectsUnknownFieldsEnumsWrongNumbersAndSeeds() throws {
        var object = try modelObject(SoundDesignPattern()); object["hiddenSelector"] = "anything"
        XCTAssertThrowsError(try validate(object, type: "SoundDesignPattern"))
        object = try modelObject(SoundDesignPattern()); object["waveform"] = "thirdPartyPlugin"
        XCTAssertThrowsError(try validate(object, type: "SoundDesignPattern"))
        object = try modelObject(SoundDesignPattern()); object["organic"] = ["seed": "18446744073709551616"]
        XCTAssertThrowsError(try validate(object, type: "SoundDesignPattern"))
        object = try modelObject(SoundDesignPattern()); object["organic"] = ["seed": "017"]
        XCTAssertThrowsError(try validate(object, type: "SoundDesignPattern"))
        object = try modelObject(Track(name: "Voice", color: 0)); object["pan"] = true
        XCTAssertThrowsError(try validate(object, type: "Track"))
        object = try modelObject(Track(name: "Voice", color: 0)); object["inputChannel"] = 1.5
        XCTAssertThrowsError(try validate(object, type: "Track"))
        XCTAssertThrowsError(try validate(["tempo": 301, "beatUnit": 4, "beatsPerBar": 4], type: "MusicalTiming"))
        XCTAssertThrowsError(try validate(["version": 12], type: "Session"))
    }
    func testMalformedDesignerPayloadIsRejectedBeforeHostExecution() throws {
        let intent = RemoteEditorIntent(operation: "designer.commit", payload: Data(#"{"notes":[{"id":"bad","pitch":true,"velocity":0.5,"startBeat":0,"durationBeats":1}]}"#.utf8))
        let request = RemoteRequest(clientID: UUID(), command: .editor(intent))
        XCTAssertThrowsError(try RemoteV3Codec.decodeMessage(RemoteV3Codec.encodeMessage(.request(request))))
    }
    func testNonemptyEffectMetersUseSafeNumericRevisionAndExactModelFields() throws {
        let id = UUID(); var meter = EffectMeterSnapshot(); meter.revision = 120
        let event = RemoteEvent.effectMeters([id: meter])
        guard case .event(let decoded) = try RemoteV3Codec.decodeMessage(RemoteV3Codec.encodeMessage(.event(event))) else { return XCTFail() }
        XCTAssertEqual(decoded, event)
    }
    func testPresenceAcceptsAllRolesIncludingNativeOperatorWithoutChangingAuthority() throws {
        let peers = [RemoteRole.viewer, .editor, .operator].map { RemotePeer(id: UUID(), name: "Peer", role: $0) }
        let event = RemoteEvent.presence(peers)
        guard case .event(let decoded) = try RemoteV3Codec.decodeMessage(RemoteV3Codec.encodeMessage(.event(event))) else { return XCTFail() }
        XCTAssertEqual(decoded, event)
    }
    func testNestedEnumFailureIdentifiesItsModelPathWithoutEchoingPayload() throws {
        var peer = try modelObject(RemotePeer(id: UUID(), name: "Peer", role: .operator))
        peer["role"] = "unapproved-secret-value"
        XCTAssertThrowsError(try RemoteV3ModelValidator.validateJSON(JSONSerialization.data(withJSONObject: [peer]), type: "[RemotePeer]")) { error in
            guard let failure = error as? RemoteV3ModelValidationError else { return XCTFail("Missing model diagnostic: \(error)") }
            XCTAssertEqual(failure.path, "[RemotePeer][0].role")
            XCTAssertEqual(failure.expectedType, "RemoteRole")
            XCTAssertTrue(failure.reason.contains("operator"))
            XCTAssertFalse(failure.localizedDescription.contains("unapproved-secret-value"))
        }
    }
    func testMasterProposalPayloadRequiresKnownEffectModels() throws {
        let context = RemoteEditorContext()
        let valid = RemoteEditorIntent(operation: "master.applyProposal", context: context,
            payload: try JSONEncoder().encode([RealtimeEffect(kind: .limiter)]))
        let request = RemoteRequest(clientID: UUID(), command: .editor(valid))
        XCTAssertNoThrow(try RemoteV3Codec.decodeMessage(RemoteV3Codec.encodeMessage(.request(request))))
        let invalid = RemoteEditorIntent(operation: "master.applyProposal", payload: Data(#"[{"kind":"externalPlugin"}]"#.utf8))
        let rejected = RemoteRequest(clientID: UUID(), command: .editor(invalid))
        XCTAssertThrowsError(try RemoteV3Codec.decodeMessage(RemoteV3Codec.encodeMessage(.request(rejected))))
    }
    private func modelObject<T: Encodable>(_ model: T) throws -> [String: Any] {
        try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(model)) as? [String: Any])
    }
    private func validate(_ object: [String: Any], type: String) throws { try RemoteV3ModelValidator.validateJSON(JSONSerialization.data(withJSONObject: object), type: type) }
}
