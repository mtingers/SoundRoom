import Combine
import SoundroomCore
import SoundroomRemote
import XCTest

@testable import Soundroom

final class FileTransferPerformanceTests: XCTestCase {
    @MainActor func testChunkProgressIsThrottledAndNeverInvalidatesTheDocument() {
        let store = SessionStore(headless: true)
        let progress = store.fileTransfer
        var documentUpdates = 0
        var progressUpdates = 0
        let document = store.objectWillChange.sink { documentUpdates += 1 }
        let telemetry = progress.objectWillChange.sink { progressUpdates += 1 }
        defer {
            document.cancel()
            telemetry.cancel()
        }
        let id = progress.begin(name: "Session.soundroom")
        progress.update(id, name: "Session.soundroom", phase: .uploading, total: 1_000, now: 0)
        for chunk in 1...1_000 {
            progress.update(
                id, name: "Session.soundroom", phase: .uploading, bytes: chunk, total: 1_000, now: Double(chunk) / 1_000
            )
        }
        XCTAssertEqual(documentUpdates, 0)
        XCTAssertLessThanOrEqual(progressUpdates, 13, "No more than 10 periodic samples plus phase/final updates")
        XCTAssertEqual(progress.snapshot?.percent, 100)
        progress.update(id, name: "Session.soundroom", phase: .opening, now: 1.001)
        XCTAssertEqual(progress.snapshot?.phase, .opening, "Opening must replace the completed upload immediately")
        progress.finish(id)
        XCTAssertNil(progress.snapshot)
        let replacement = progress.begin(name: "New.wav")
        progress.finish(id)
        progress.update(id, name: "Old.wav", phase: .opening)
        XCTAssertEqual(progress.snapshot?.name, "New.wav", "A stale task must not clear or overwrite a new transfer")
        progress.finish(replacement)
    }

    @MainActor func testUploadsPreserveBytesAndOffsetsWithoutPerChunkDocumentPublications() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("transfer-\(UUID()).wav")
        let bytes = Data((0..<(RemoteProtocol.maximumUploadChunkBytes * 4 + 37)).map { UInt8(truncatingIfNeeded: $0) })
        try bytes.write(to: url)
        defer { try? FileManager.default.removeItem(at: url) }
        let store = SessionStore(headless: true)
        var sent = Data()
        var offsets: [Int] = []
        var phases: [FileTransferProgress.Phase] = []
        var editorUpdates = 0
        let remote = RemoteGUIController(
            store: store, credential: .native(try RemoteCredential.generate()), host: "fixture",
            commandSender: { command, _, _, _ in
                switch command {
                case .beginUpload(_, _, let count, _): XCTAssertEqual(count, bytes.count)
                case .uploadChunk(_, let offset, let data):
                    offsets.append(offset)
                    sent.append(data)
                case .finishUpload: XCTAssertEqual(store.fileTransfer.snapshot?.phase, .opening)
                default: XCTFail("Unexpected command")
                }
                return .init(requestID: UUID(), revision: 0)
            })
        store.remoteGUI = remote
        remote.receiveState(.connected(serverID: UUID(), name: "Fixture", role: .operator))
        let editor = store.objectWillChange.sink { editorUpdates += 1 }
        let telemetry = store.fileTransfer.$snapshot.compactMap { $0?.phase }.sink { phases.append($0) }
        defer {
            editor.cancel()
            telemetry.cancel()
            remote.disconnect()
        }
        let work = try XCTUnwrap(remote.uploadFiles([url]))
        await work.value
        XCTAssertEqual(sent, bytes)
        XCTAssertEqual(offsets, [0, 262_144, 524_288, 786_432, 1_048_576])
        XCTAssertEqual(Set(phases), [.preparing, .checking, .uploading, .opening])
        XCTAssertLessThanOrEqual(
            editorUpdates, 2, "Only import start and completion should publish the document status")
        XCTAssertFalse(store.fileTransfer.isActive)
    }

    @MainActor func testDisconnectDuringUploadPreventsFinishingAgainstAnotherConnection() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("transfer-disconnect-\(UUID()).wav")
        try Data(repeating: 1, count: 300_000).write(to: url)
        defer { try? FileManager.default.removeItem(at: url) }
        let store = SessionStore(headless: true)
        var target: RemoteGUIController?
        var finishes = 0
        let remote = RemoteGUIController(
            store: store, credential: .native(try RemoteCredential.generate()), host: "fixture",
            commandSender: { command, _, _, _ in
                if case .uploadChunk = command { target?.receiveState(.disconnected) }
                if case .finishUpload = command { finishes += 1 }
                return .init(requestID: UUID(), revision: 0)
            })
        target = remote
        store.remoteGUI = remote
        remote.receiveState(.connected(serverID: UUID(), name: "Fixture", role: .operator))
        let work = try XCTUnwrap(remote.uploadFiles([url]))
        await work.value
        XCTAssertEqual(finishes, 0)
        XCTAssertNil(store.fileTransfer.snapshot)
        remote.disconnect()
    }
}
