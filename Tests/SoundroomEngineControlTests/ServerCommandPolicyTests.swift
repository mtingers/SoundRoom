import XCTest
import SoundroomCore
@testable import SoundroomRemote
import SoundroomEngineControl

final class ServerCommandPolicyTests: XCTestCase {
    func testReadonlyAndInterruptPoliciesPreserveIndependentViewsAndTransportReserve() {
        for operation in RemoteCapabilities().editorOperations {
            let command = RemoteCommand.editor(.init(operation: operation))
            XCTAssertEqual(command.canInterruptServerJob, command.isInterrupt, operation)
            if command.isServerReadOnly {
                XCTAssertFalse(command.changesServerCursor, operation)
                XCTAssertFalse(command.recordsServerHistory, operation)
            }
        }
        XCTAssertTrue(RemoteCommand.snapshot.isServerReadOnly)
        XCTAssertTrue(RemoteCommand.stop.canInterruptServerJob)
        XCTAssertTrue(RemoteCommand.stop.changesServerCursor)
        XCTAssertFalse(RemoteCommand.stop.recordsServerHistory)
        XCTAssertFalse(RemoteCommand.editor(.init(operation: "unapproved.operation")).isServerReadOnly)
        XCTAssertFalse(RemoteCommand.editor(.init(operation: "unapproved.operation")).canInterruptServerJob)
    }
    func testAuthoredEditMetadataAndWorkerPolicyRetainHistoryAndPreflight() {
        let track = Track(name: "Source", color: 2), gesture = "drag"
        let command = RemoteCommand.replaceTracks(tracks: [track], name: "Move", gestureID: gesture)
        XCTAssertTrue(command.recordsServerHistory)
        XCTAssertFalse(command.changesServerCursor)
        XCTAssertEqual(command.serverTrackEdit, .init(tracks: [track], name: "Move", gestureID: gesture))
        XCTAssertEqual(command.serverTitle, "Move"); XCTAssertEqual(command.serverGesture, gesture)
        for operation in ["effects.apply", "pitch.apply", "pitch.targets", "master.applyProposal", "clips.combine"] {
            let edit = RemoteCommand.editor(.init(operation: operation))
            XCTAssertTrue(edit.recordsServerHistory, operation)
            XCTAssertTrue(edit.requiresPostRenderPreflight, operation)
            XCTAssertTrue(edit.isHeavyServerOperation, operation)
            XCTAssertFalse(edit.isServerReadOnly, operation)
        }
        let declaration = RemoteCommand.editor(.init(operation: "client.features"))
        XCTAssertTrue(declaration.isServerReadOnly)
        XCTAssertFalse(declaration.isHeavyServerOperation)
        XCTAssertFalse(declaration.requiresPostRenderPreflight)
    }
}
