import XCTest
import SoundroomCore
import SoundroomRemote
@testable import SoundroomEngineControl

final class ServerCreativeEditAdmissionTests: XCTestCase {
    private func request(_ operation: String, context: RemoteEditorContext, strings: [String: String], versions: [UUID: Int] = [:], revision: Int? = nil) -> RemoteRequest {
        .init(clientID: UUID(), expectedRevision: revision, expectedTrackRevisions: versions,
              command: .editor(.init(operation: operation, context: context, strings: strings)))
    }
    func testExistingCreativeDestinationRequiresItsActualVersion() throws {
        let session = Session(), a = session.tracks[0].id, b = UUID()
        for operation in ["designer.commit", "drums.commit"] {
            let context = RemoteEditorContext(trackID: a)
            XCTAssertThrowsError(try ServerCreativeEditAdmission.validate(request(operation, context: context, strings: ["trackID": a.uuidString], versions: [b: 3]), in: session))
            XCTAssertNoThrow(try ServerCreativeEditAdmission.validate(request(operation, context: context, strings: ["trackID": a.uuidString], versions: [a: 3]), in: session))
        }
    }
    func testContextCannotFenceOneTrackWhileReplacingAnother() throws {
        var session = Session(); session.tracks.append(Track(name: "Other", color: 1))
        let a = session.tracks[0].id, b = session.tracks[1].id
        for operation in ["designer.commit", "drums.commit"] {
            XCTAssertThrowsError(try ServerCreativeEditAdmission.validate(request(operation, context: .init(trackID: a), strings: ["trackID": b.uuidString], versions: [a: 3, b: 3]), in: session))
        }
    }
    func testMissingTrackClipAndMalformedIdentityNeverFallBackToInsertion() throws {
        let session = Session(), a = session.tracks[0].id, missing = UUID()
        for operation in ["designer.commit", "drums.commit"] {
            XCTAssertThrowsError(try ServerCreativeEditAdmission.validate(request(operation, context: .init(), strings: ["trackID": missing.uuidString], versions: [missing: 3], revision: 3), in: session))
            XCTAssertThrowsError(try ServerCreativeEditAdmission.validate(request(operation, context: .init(trackID: a), strings: ["trackID": a.uuidString, "clipID": missing.uuidString], versions: [a: 3]), in: session))
            XCTAssertThrowsError(try ServerCreativeEditAdmission.validate(request(operation, context: .init(), strings: ["trackID": "wrong"], revision: 3), in: session))
        }
    }
    func testNewTrackRequiresSessionFenceAndNoReplacementIdentity() throws {
        let session = Session()
        for operation in ["designer.commit", "drums.commit"] {
            XCTAssertThrowsError(try ServerCreativeEditAdmission.validate(request(operation, context: .init(), strings: [:]), in: session))
            XCTAssertThrowsError(try ServerCreativeEditAdmission.validate(request(operation, context: .init(), strings: ["clipID": UUID().uuidString], revision: 3), in: session))
            XCTAssertNoThrow(try ServerCreativeEditAdmission.validate(request(operation, context: .init(), strings: [:], revision: 3), in: session))
        }
    }
    func testReplacementClipMustMatchContextAndBelongToDestination() throws {
        var session = Session(), clip = Clip(assetID: UUID(), name: "Source", start: 0, length: 100)
        session.tracks[0].clips = [clip]
        let id = session.tracks[0].id
        for operation in ["designer.commit", "drums.commit"] {
            let strings = ["trackID": id.uuidString, "clipID": clip.id.uuidString]
            XCTAssertNoThrow(try ServerCreativeEditAdmission.validate(request(operation, context: .init(trackID: id, clipID: clip.id), strings: strings, versions: [id: 3]), in: session))
            clip.id = UUID()
            XCTAssertThrowsError(try ServerCreativeEditAdmission.validate(request(operation, context: .init(trackID: id, clipID: clip.id), strings: strings, versions: [id: 3]), in: session))
            clip = session.tracks[0].clips[0]
        }
    }
}
