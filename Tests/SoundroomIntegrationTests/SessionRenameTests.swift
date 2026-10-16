import XCTest
import SoundroomCore
import SoundroomRemote
@testable import Soundroom

final class SessionRenameTests: XCTestCase {
    @MainActor private func settle() async { for _ in 0..<80 { await Task.yield() } }
    @MainActor private func remoteFixture(role: RemoteRole = .editor, sender: @escaping RemoteGUIController.CommandSender) throws -> (SessionStore, RemoteGUIController, UUID) {
        let store = SessionStore(headless: true), serverID = UUID()
        let remote = RemoteGUIController(store: store, credential: .native(try RemoteCredential.generate()), host: "fixture", commandSender: sender)
        store.remoteGUI = remote
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: role))
        remote.receive(.snapshot(.init(serverID: serverID, serverName: "Fixture", session: Session(), revision: 3)))
        return (store, remote, serverID)
    }

    @MainActor func testRemoteRenameCarriesOpeningRevisionAndUsesAuthoritativeTitle() async throws {
        var sent: RemoteEditorIntent?, expected: Int?
        let (store, remote, _) = try remoteFixture { command, tracks, revision, _ in
            guard case .editor(let intent) = command else { throw RemoteError.invalidFrame }
            sent = intent; expected = revision; XCTAssertTrue(tracks.isEmpty)
            return .init(requestID: UUID(), revision: 4)
        }
        let originalTracks = store.session.tracks
        store.requestSessionRename(); let draft = try XCTUnwrap(store.sessionRenameDraft)
        draft.name = "  Named session  "; draft.submit(); await settle()
        XCTAssertEqual(sent?.operation, "session.rename")
        XCTAssertEqual(sent?.strings["name"], "Named session"); XCTAssertEqual(expected, 3)
        XCTAssertEqual(store.session.name, "Untitled session", "No optimistic session replacement")
        remote.receive(.delta(.init(revision: 4, name: "Named session")))
        XCTAssertEqual(store.session.name, "Named session")
        XCTAssertEqual(store.session.tracks, originalTracks)
        XCTAssertNil(store.sessionRenameDraft)
        remote.disconnect()
    }

    @MainActor func testRejectedRenameRetainsTextAndCurrentSession() async throws {
        let (store, remote, _) = try remoteFixture { _, _, _, _ in
            .init(requestID: UUID(), status: .conflict, revision: 4, message: "The session changed.")
        }
        let original = store.session
        store.requestSessionRename(); let draft = try XCTUnwrap(store.sessionRenameDraft)
        draft.name = "Retain this draft"; draft.submit(); await settle()
        XCTAssertEqual(store.session, original)
        XCTAssertEqual(store.sessionRenameDraft?.id, draft.id)
        XCTAssertEqual(draft.name, "Retain this draft"); XCTAssertFalse(draft.pending)
        XCTAssertEqual(draft.message, "The session changed.")
        remote.disconnect()
    }

    @MainActor func testChangedSessionAndReconnectCannotRedirectAnOpenRenameDraft() async throws {
        var sends = 0
        let (store, remote, serverID) = try remoteFixture { _, _, _, _ in sends += 1; return .init(requestID: UUID(), revision: 4) }
        store.requestSessionRename(); let draft = try XCTUnwrap(store.sessionRenameDraft)
        draft.name = "Old document name"
        remote.receive(.delta(.init(revision: 4, name: "Another session")))
        draft.submit(); await settle()
        XCTAssertEqual(sends, 0); XCTAssertEqual(store.session.name, "Another session")
        XCTAssertEqual(draft.name, "Old document name")
        store.requestSessionRename(); let reconnected = try XCTUnwrap(store.sessionRenameDraft)
        reconnected.name = "Before reconnect"
        remote.receiveState(.disconnected)
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .editor))
        remote.receive(.snapshot(.init(serverID: serverID, serverName: "Fixture", session: Session(), revision: 4)))
        reconnected.submit(); await settle(); XCTAssertEqual(sends, 0)
        remote.disconnect()
    }

    @MainActor func testViewerRecordingAndInvalidNamesCannotRename() throws {
        let (store, remote, _) = try remoteFixture(role: .viewer) { _, _, _, _ in XCTFail("Viewer must not send"); throw RemoteError.invalidFrame }
        XCTAssertFalse(store.canRenameSession); store.requestSessionRename(); XCTAssertNil(store.sessionRenameDraft)
        remote.disconnect()
        let (local, editor, _) = try remoteFixture { _, _, _, _ in XCTFail("Invalid names must not send"); throw RemoteError.invalidFrame }
        defer { editor.disconnect() }
        local.busy = true; XCTAssertFalse(local.canRenameSession); local.busy = false
        local.isRecording = true; XCTAssertFalse(local.canRenameSession); local.isRecording = false
        local.requestSessionRename(); let draft = try XCTUnwrap(local.sessionRenameDraft)
        draft.name = "../invalid"; draft.submit()
        XCTAssertEqual(local.session.name, "Untitled session"); XCTAssertFalse(draft.message.isEmpty)
    }
}
