import Foundation
import SoundroomCore
import SoundroomRemote
@testable import Soundroom

/// Supplies server authority for metadata-only editor tests without a socket or device.
@MainActor func attachClientFixture(_ store: SessionStore) throws {
    let document = store.session, serverID = UUID()
    let remote = RemoteGUIController(store: store, credential: .native(try RemoteCredential.generate()), host: "fixture", commandSender: { _, _, _, _ in
        .init(requestID: UUID(), status: .rejected, revision: 0, message: "Metadata fixture has no audio host")
    })
    store.remoteGUI = remote
    remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .operator))
    remote.receive(.snapshot(.init(serverID: serverID, serverName: "Fixture", session: document, trackRevisions: Dictionary(uniqueKeysWithValues: document.tracks.map { ($0.id, 0) }))))
}
