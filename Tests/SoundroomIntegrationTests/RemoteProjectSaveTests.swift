import XCTest
import CryptoKit
import SoundroomCore
import SoundroomRemote
@testable import Soundroom

@MainActor final class RemoteProjectSaveTests: XCTestCase {
    private func directory() throws -> URL {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("ProjectSave-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: url) }; return url
    }
    private func fixture(_ session: Session, name: String = "Original filename.soundroom", sender: @escaping RemoteGUIController.CommandSender) throws -> (SessionStore, RemoteGUIController, UUID) {
        let store = SessionStore(headless: true), id = UUID()
        let remote = RemoteGUIController(store: store, credential: .native(try RemoteCredential.generate()), host: "fixture", isLocalServer: true, commandSender: sender)
        store.remoteGUI = remote
        remote.receiveState(.connected(serverID: id, name: "Fixture", role: .operator))
        remote.receive(.snapshot(.init(serverID: id, serverName: "Fixture", session: session, revision: 3, projectName: name)))
        return (store, remote, id)
    }
    private func waitForSave(_ remote: RemoteGUIController) async throws {
        for _ in 0..<2_000 {
            if !remote.isSavingProject { return }
            try await Task.sleep(nanoseconds: 5_000_000)
        }
        XCTFail("Save did not complete")
    }
    func testOrdinarySaveUsesOpenedFilenameAndPreservesRenamedSessionOnRepeatedSaves() async throws {
        var session = Session(); session.name = "An edited session title"
        var saved: [String] = []
        let (store, remote, _) = try fixture(session) { command, _, expected, _ in
            guard case .saveProject(let name, let replace, let title) = command else { throw RemoteError.invalidFrame }
            saved.append(name); XCTAssertEqual(replace, true); XCTAssertNil(title)
            XCTAssertNil(expected, "Save captures after earlier queued edits, rather than fencing an old UI revision")
            return .init(requestID: UUID(), revision: 3)
        }
        defer { remote.disconnect() }
        for _ in 0..<2 { store.requestSave(); try await waitForSave(remote) }
        XCTAssertEqual(saved, ["Original filename.soundroom", "Original filename.soundroom"])
        XCTAssertEqual(store.session.name, session.name)
        XCTAssertEqual(remote.projectSaveStatus, .saved)
        XCTAssertTrue(remote.projectSaveHelp.contains("Original filename.soundroom"))
    }
    func testFailedSaveKeepsDestinationAndUnsavedBadge() async throws {
        let (store, remote, _) = try fixture(Session()) { command, _, _, _ in
            if case .saveProject = command { return .init(requestID: UUID(), status: .rejected, revision: 4, message: "Disk full") }
            return .init(requestID: UUID(), revision: 4)
        }
        defer { remote.disconnect() }
        remote.confirmProjectFileSaved(store.session, at: URL(fileURLWithPath: "/tmp/fixture.soundroom"))
        var track = store.session.tracks[0]; track.gain = 0.25
        remote.receive(.delta(.init(revision: 4, tracks: [track], trackRevisions: [track.id: 4])))
        store.requestSave(); try await waitForSave(remote)
        XCTAssertEqual(remote.projectSaveStatus, .modified)
        XCTAssertEqual(remote.serverProjectName, "Original filename.soundroom")
        XCTAssertTrue(store.activity.entries.contains { $0.kind == .failure && $0.message == "Disk full" })
    }
    func testSaveReceiptBeforeItsMetadataBecomesSavedOnlyWhenTheMatchingDocumentArrives() async throws {
        var session = Session(); session.name = "Old title"
        let (store, remote, _) = try fixture(session) { _, _, _, _ in .init(requestID: UUID(), revision: 4) }
        defer { remote.disconnect() }
        remote.saveServerProject(name: "New title", replaceExisting: false)
        try await waitForSave(remote)
        XCTAssertEqual(remote.projectSaveStatus, .modified)
        remote.receive(.delta(.init(revision: 4, name: "New title")))
        XCTAssertEqual(remote.projectSaveStatus, .saved)
        XCTAssertEqual(store.session.name, "New title")
    }
    func testSaveWritesDownloadedProjectBackToOpenedFileAndSaveAsKeepsOriginal() async throws {
        let directory = try directory(), original = directory.appendingPathComponent("Original.soundroom")
        var document = Session(); let asset = UUID()
        document.name = "Preserved title"
        document.tracks[0].clips = [Clip(assetID: asset, name: "Source", start: 0, length: 480)]
        document.tracks[0].realtimeEffects = [RealtimeEffect(kind: .soundPutty, parameters: ["puttyPitch299": 0.25, "puttyEnergy145": 0.5, "puttyDamping000": -0.25, "variationSeed": 777])]
        let samples = AudioSamples(channels: [(0..<480).map { sin(Float($0) * 0.1) * 0.2 }])
        try ProjectPersistence.saveSynchronously(session: document, assets: [asset: samples], to: original)
        document.tracks[0].gain = 0.25
        let savedCopy = directory.appendingPathComponent("Package.soundroom"), archive = directory.appendingPathComponent("Package.soundroom.zip")
        try ProjectPersistence.saveSynchronously(session: document, assets: [asset: samples], to: savedCopy)
        try await ProjectUploadArchive.create(savedCopy, to: archive, isLocal: true)
        let bytes = try Data(contentsOf: archive), artifact = RemoteArtifact(id: UUID(), name: "Session.soundroom.zip", bytes: bytes.count,
            sha256: SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined())
        var released = 0, packages = 0
        let (store, remote, _) = try fixture(document) { command, _, _, _ in
            if case .editor(let intent) = command, intent.operation == "project.package" {
                if intent.strings["release"] != nil { released += 1; return .init(requestID: UUID(), revision: 3) }
                packages += 1
                return .init(requestID: UUID(), revision: 3, payload: try JSONEncoder().encode(artifact))
            }
            guard case .downloadArtifact(let id, let offset, let count) = command else { throw RemoteError.invalidFrame }
            XCTAssertEqual(id, artifact.id)
            let end = min(bytes.count, offset + count)
            return .init(requestID: UUID(), revision: 3, payload: try JSONEncoder().encode(RemoteArtifactChunk(offset: offset, data: bytes.subdata(in: offset..<end), complete: end == bytes.count)))
        }
        defer { remote.disconnect() }
        remote.adoptProjectDestination(serverName: "Original filename.soundroom", file: original)
        store.requestSave(); try await waitForSave(remote)
        XCTAssertEqual(try ProjectPersistence.loadSynchronously(from: original).session.tracks[0].gain, 0.25)
        XCTAssertEqual(remote.projectFileURL, original)
        XCTAssertEqual(remote.projectSaveStatus, .saved)
        XCTAssertTrue(remote.projectSaveHelp.contains(original.path))
        let copy = directory.appendingPathComponent("Save As.soundroom")
        remote.saveProjectFile(to: copy, replaceExisting: false); try await waitForSave(remote)
        XCTAssertEqual(remote.projectFileURL, copy)
        XCTAssertEqual(try ProjectPersistence.loadSynchronously(from: copy).session, document)
        XCTAssertEqual(try ProjectPersistence.loadSynchronously(from: original).session.tracks[0].gain, 0.25)
        for _ in 0..<100 { await Task.yield() }
        XCTAssertEqual(packages, 2); XCTAssertEqual(released, 2)
    }
    func testDownloadedPackageFailureLeavesOldProjectUntouchedAndRejectsSymlink() async throws {
        let directory = try directory(), original = directory.appendingPathComponent("Original.soundroom")
        let session = Session()
        try ProjectPersistence.saveSynchronously(session: session, assets: [:], to: original)
        let before = try Data(contentsOf: original.appendingPathComponent("session.json"))
        let broken = directory.appendingPathComponent("broken.zip"); try Data("invalid".utf8).write(to: broken)
        do { _ = try await ProjectPackageDownload.commit(broken, to: original, replaceExisting: true); XCTFail("Invalid download accepted") } catch {}
        XCTAssertEqual(try Data(contentsOf: original.appendingPathComponent("session.json")), before)
        let link = directory.appendingPathComponent("Alias.soundroom")
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: original)
        do { _ = try await ProjectPackageDownload.commit(broken, to: link, replaceExisting: true); XCTFail("Symlink accepted") } catch {}
        XCTAssertEqual(try Data(contentsOf: original.appendingPathComponent("session.json")), before)
    }
    func testOpeningAnotherServerProjectClearsThePreviousDiskDestination() throws {
        let (store, remote, id) = try fixture(Session()) { _, _, _, _ in .init(requestID: UUID(), revision: 3) }
        defer { remote.disconnect() }
        remote.adoptProjectDestination(serverName: "Original filename.soundroom", file: URL(fileURLWithPath: "/tmp/first.soundroom"))
        remote.receive(.snapshot(.init(serverID: id, serverName: "Fixture", session: Session(), revision: 4, projectName: "Second.soundroom")))
        XCTAssertNil(remote.projectFileURL); XCTAssertNil(store.projectURL)
        XCTAssertEqual(remote.serverProjectName, "Second.soundroom")
    }
    func testNewSessionClearsAFileSavedWithoutANamedServerProject() async throws {
        let (store, remote, id) = try fixture(Session()) { _, _, _, _ in .init(requestID: UUID(), revision: 4) }
        defer { remote.disconnect() }
        remote.adoptProjectDestination(serverName: nil, file: URL(fileURLWithPath: "/tmp/first.soundroom"))
        remote.receive(.snapshot(.init(serverID: id, serverName: "Fixture", session: Session(), revision: 3)))
        remote.route(.init(operation: "project.new"))
        for _ in 0..<100 { await Task.yield() }
        XCTAssertNil(remote.projectFileURL); XCTAssertNil(store.projectURL)
        XCTAssertNil(remote.serverProjectName)
    }
    func testAnotherClientsNewSessionClearsTheFileEvenWhenBothServerNamesAreNil() throws {
        let (store, remote, id) = try fixture(Session()) { _, _, _, _ in .init(requestID: UUID(), revision: 3) }
        defer { remote.disconnect() }
        remote.receive(.snapshot(.init(serverID: id, serverName: "Fixture", session: Session(), revision: 3, projectID: UUID())))
        remote.adoptProjectDestination(serverName: nil, file: URL(fileURLWithPath: "/tmp/first.soundroom"))
        remote.receive(.snapshot(.init(serverID: id, serverName: "Fixture", session: Session(), revision: 4, projectID: UUID())))
        XCTAssertNil(remote.projectFileURL); XCTAssertNil(store.projectURL)
    }
}
