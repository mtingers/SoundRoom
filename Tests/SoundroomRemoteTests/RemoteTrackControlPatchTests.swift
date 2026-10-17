import SoundroomCore
import XCTest

@testable import SoundroomRemote

final class RemoteTrackControlPatchTests: XCTestCase {
    func testDenseAutomationPatchIsSmallAndPreservesAllNonScalarState() throws {
        var session = Session()
        let id = session.tracks[0].id
        session.tracks[0].mixAutomation["gain"] = (0..<100_000).map { .init(frame: $0, value: 0.5) }
        var effect = RealtimeEffect(kind: .echo)
        effect.parameterAutomation["feedback"] = session.tracks[0].mixAutomation["gain"]
        session.tracks[0].realtimeEffects = [effect]
        let before = session.tracks[0]
        var snapshot = RemoteSnapshot(serverID: UUID(), serverName: "Host", session: session, revision: 4)
        let patch = RemoteTrackControlPatch(trackID: id, gain: 0, pan: -1, muted: false, solo: false, armed: true)
        let delta = RemoteSessionDelta(revision: 5, controlPatches: [patch], trackRevisions: [id: 5])
        let bytes = try RemoteV3Codec.encodeMessage(.event(.delta(delta)))
        XCTAssertLessThan(bytes.count, 1024, "Scalar patch must not serialize dense automation")
        guard case .event(.delta(let decoded)) = try RemoteV3Codec.decodeMessage(bytes) else { return XCTFail() }
        XCTAssertEqual(decoded, delta)
        snapshot.apply(decoded)
        var expected = before
        patch.apply(to: &expected)
        XCTAssertEqual(snapshot.session.tracks[0], expected)
        XCTAssertEqual(snapshot.session.tracks[0].gain, 0)
        XCTAssertEqual(snapshot.session.tracks[0].mixAutomation, before.mixAutomation)
        XCTAssertEqual(snapshot.session.tracks[0].realtimeEffects, before.realtimeEffects)
        XCTAssertEqual(snapshot.trackRevisions[id], 5)
    }
    func testMasterFalseValuesPartialFieldsAndStaleRevisions() throws {
        var session = Session()
        session.masterTrack.gain = 4
        session.masterTrack.muted = true
        session.masterTrack.solo = true
        session.masterTrack.armed = true
        let originalGain = session.masterTrack.gain
        var snapshot = RemoteSnapshot(serverID: UUID(), serverName: "Host", session: session, revision: 4)
        let patch = RemoteTrackControlPatch(
            trackID: Session.masterTrackID, pan: 0, muted: false, solo: false, armed: false)
        snapshot.apply(.init(revision: 5, controlPatches: [patch]))
        XCTAssertEqual(snapshot.session.masterTrack.gain, originalGain)
        XCTAssertEqual(snapshot.session.masterTrack.pan, 0)
        XCTAssertFalse(snapshot.session.masterTrack.muted)
        XCTAssertFalse(snapshot.session.masterTrack.solo)
        XCTAssertFalse(snapshot.session.masterTrack.armed)
        let installed = snapshot
        snapshot.apply(.init(revision: 3, controlPatches: [.init(trackID: Session.masterTrackID, gain: 4)]))
        XCTAssertEqual(snapshot, installed)
        var track = session.tracks[0]
        let before = track
        patch.apply(to: &track)
        XCTAssertEqual(track, before)
    }
    func testLegacyScalarGainRangeSurvivesEveryPublicTrackRepresentation() throws {
        var track = Session().tracks[0]
        track.gain = 4
        let encoder = JSONEncoder()
        try RemoteV3ModelValidator.validateJSON(encoder.encode(track), type: "Track")
        try RemoteV3ModelValidator.validateJSON(
            encoder.encode(RemoteTrackControls(track: track)), type: "RemoteTrackControls")
        try RemoteV3ModelValidator.validateJSON(
            encoder.encode(RemoteTrackControlPatch(trackID: track.id, gain: 4)), type: "RemoteTrackControlPatch")
        for gain: Float in [-0.01, 4.0001] {
            track.gain = gain
            XCTAssertThrowsError(try RemoteV3ModelValidator.validateJSON(encoder.encode(track), type: "Track"))
            XCTAssertThrowsError(
                try RemoteV3ModelValidator.validateJSON(
                    encoder.encode(RemoteTrackControls(track: track)), type: "RemoteTrackControls"))
        }
    }
    func testUnnegotiatedGrammarOmitsNewFieldAndNegotiationRequiresBothCapabilities() throws {
        let delta = RemoteSessionDelta(revision: 1)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(delta)) as? [String: Any])
        XCTAssertNil(object["controlPatches"], "Old strict clients must never receive an unnegotiated field")
        guard
            case .event(.delta(let decoded)) = try RemoteV3Codec.decodeMessage(
                RemoteV3Codec.encodeMessage(.event(.delta(delta))))
        else { return XCTFail() }
        XCTAssertNil(decoded.controlPatches)
        var caps = RemoteCapabilities()
        XCTAssertNil(caps.clientFeatureDeclaration())
        caps.features = ["compact-control-patches"]
        let declaration = try XCTUnwrap(caps.clientFeatureDeclaration())
        guard case .editor(let intent) = declaration else { return XCTFail() }
        XCTAssertEqual(intent.operation, "client.features")
        XCTAssertEqual(
            try JSONDecoder().decode([String].self, from: XCTUnwrap(intent.payload)), ["compact-control-patches"])
        caps.editorOperations.removeAll { $0 == "client.features" }
        XCTAssertNil(caps.clientFeatureDeclaration())
        caps.editorOperations.append("client.features")
        caps.commands.removeAll { $0 == "editor" }
        XCTAssertNil(caps.clientFeatureDeclaration())
    }
    func testUnknownPatchFieldsAndWrongScalarTypesAreRejected() throws {
        let id = UUID().uuidString
        for invalid in [
            "{\"trackID\":\"\(id)\",\"muted\":1}", "{\"trackID\":\"\(id)\",\"gain\":true}",
            "{\"trackID\":\"\(id)\",\"realtimeEffects\":[]}",
        ] {
            XCTAssertThrowsError(
                try RemoteV3ModelValidator.validateJSON(Data(invalid.utf8), type: "RemoteTrackControlPatch"))
        }
        for patch in [
            RemoteTrackControlPatch(trackID: UUID()), .init(trackID: UUID(), gain: 4.00001),
            .init(trackID: UUID(), pan: -1.01),
        ] {
            XCTAssertFalse(patch.hasValidValues())
            XCTAssertThrowsError(
                try RemoteV3ModelValidator.validateJSON(JSONEncoder().encode(patch), type: "RemoteTrackControlPatch"))
        }
        let invalidDeclaration = RemoteRequest(
            clientID: UUID(), command: .editor(.init(operation: "client.features", payload: Data("[1]".utf8))))
        XCTAssertThrowsError(try RemoteV3Codec.decodeMessage(RemoteV3Codec.encodeMessage(.request(invalidDeclaration))))
    }
    func testAllSparseTargetsAreValidatedBeforeMutation() throws {
        let original = RemoteSnapshot(serverID: UUID(), serverName: "Host", session: Session(), revision: 4)
        let id = original.session.tracks[0].id
        for patches in [
            [RemoteTrackControlPatch(trackID: id, gain: 0), .init(trackID: UUID(), pan: 0)],
            [.init(trackID: id, gain: 0), .init(trackID: id, pan: 0)],
        ] {
            var snapshot = original
            XCTAssertFalse(snapshot.apply(.init(revision: 5, controlPatches: patches)))
            XCTAssertEqual(snapshot, original)
        }
        var snapshot = original
        XCTAssertFalse(
            snapshot.apply(.init(revision: 5, controlPatches: [.init(trackID: id, gain: 0)], removedTrackIDs: [id])))
        XCTAssertEqual(snapshot, original)
    }
}
