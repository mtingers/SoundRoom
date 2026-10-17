import CryptoKit
import XCTest

@testable import Soundroom

@MainActor final class ServerFilesTests: XCTestCase {
    func directory() throws -> URL {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(
            "ServerFilesTests-" + UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: url) }
        return url
    }
    func hash(_ data: Data) -> String { SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined() }
    func testManagedNamesRejectPathsAndSymlinks() throws {
        let directory = try directory()
        let files = try ServerFiles(dataDirectory: directory)
        for name in ["../evil", "/tmp/evil", "nested/project", "a\\b", ".hidden", "bad\u{1b}name"] {
            XCTAssertThrowsError(try files.saveProjectURL(name: name))
        }
        let target = try files.saveProjectURL(name: "Session")
        XCTAssertEqual(target.lastPathComponent, "Session.soundroom")
        try FileManager.default.createSymbolicLink(at: target, withDestinationURL: directory)
        XCTAssertThrowsError(try files.saveProjectURL(name: "Session"))
    }
    func testUploadOwnerOffsetsHashAndAudioFilename() async throws {
        let files = try ServerFiles(dataDirectory: directory())
        let owner = UUID()
        let upload = UUID()
        let data = Data("audio fixture".utf8)
        try files.beginUpload(clientID: owner, id: upload, name: "take.wav", bytes: data.count, sha256: hash(data))
        do {
            try await files.appendUpload(clientID: UUID(), id: upload, offset: 0, data: data)
            XCTFail("Other client wrote upload")
        } catch {}
        do {
            try await files.appendUpload(clientID: owner, id: upload, offset: 1, data: data)
            XCTFail("Wrong offset wrote upload")
        } catch {}
        try await files.appendUpload(clientID: owner, id: upload, offset: 0, data: data)
        let completed = try await files.finishUpload(clientID: owner, id: upload)
        XCTAssertEqual(try Data(contentsOf: completed), data)
        XCTAssertEqual(completed.pathExtension, "wav")
    }
    func testMismatchDeletesTemporaryUploadAndReleasesID() async throws {
        let files = try ServerFiles(dataDirectory: directory())
        let owner = UUID()
        let upload = UUID()
        let data = Data([1, 2, 3])
        try files.beginUpload(
            clientID: owner, id: upload, name: "take.wav", bytes: data.count, sha256: String(repeating: "0", count: 64))
        try await files.appendUpload(clientID: owner, id: upload, offset: 0, data: data)
        do {
            _ = try await files.finishUpload(clientID: owner, id: upload)
            XCTFail("Wrong hash accepted")
        } catch {}
        try files.beginUpload(clientID: owner, id: upload, name: "take.wav", bytes: data.count, sha256: hash(data))
        files.cancelUploads(clientID: owner)
    }
    func testQuotasAndOversizedChunksAreRejectedBeforeWriting() async throws {
        let files = try ServerFiles(dataDirectory: directory())
        let owner = UUID()
        let upload = UUID()
        let sha = String(repeating: "a", count: 64)
        XCTAssertThrowsError(
            try files.beginUpload(
                clientID: owner, id: upload, name: "take.wav", bytes: ServerFiles.maximumUploadBytes + 1, sha256: sha))
        try files.beginUpload(
            clientID: owner, id: upload, name: "take.wav", bytes: ServerFiles.maximumUploadBytes, sha256: sha)
        XCTAssertThrowsError(
            try files.beginUpload(clientID: owner, id: UUID(), name: "other.wav", bytes: 1, sha256: sha))
        do {
            try await files.appendUpload(
                clientID: owner, id: upload, offset: 0,
                data: Data(repeating: 0, count: ServerFiles.maximumChunkBytes + 1))
            XCTFail("Oversized chunk accepted")
        } catch {}
        files.cancelUploads(clientID: owner)
    }
    func testArtifactRangesAreBoundedAndUnknownPathsCannotBeRead() throws {
        let files = try ServerFiles(dataDirectory: directory())
        let url = try files.exportURL(name: "Mix")
        let bytes = Data([0, 1, 2, 3, 4])
        try bytes.write(to: url)
        let id = try files.artifactID(for: url)
        XCTAssertEqual(try files.readArtifact(id: id, offset: 2, bytes: 2), Data([2, 3]))
        XCTAssertThrowsError(try files.readArtifact(id: UUID(), offset: 0, bytes: 1))
        XCTAssertThrowsError(try files.readArtifact(id: id, offset: -1, bytes: 1))
        XCTAssertThrowsError(try files.readArtifact(id: id, offset: 0, bytes: ServerFiles.maximumChunkBytes + 1))
        XCTAssertThrowsError(try files.artifactID(for: url.deletingLastPathComponent().deletingLastPathComponent()))
    }
    func testArtifactsRemainImmutableWhenNamedExportIsReplaced() async throws {
        let files = try ServerFiles(dataDirectory: directory())
        let target = try files.exportURL(name: "Repeat")
        try Data([1, 2, 3]).write(to: target, options: .atomic)
        let old = try files.artifactID(for: target)
        try Data([9, 8, 7]).write(to: target, options: .atomic)
        let oldBytes = try await files.readArtifactAsync(id: old, offset: 0, bytes: 3)
        XCTAssertEqual(oldBytes, Data([1, 2, 3]))
        let new = try files.artifactID(for: target)
        XCTAssertEqual(try files.readArtifact(id: new, offset: 0, bytes: 3), Data([9, 8, 7]))
    }
    func testDittoProjectUploadMovesIntoNamedProjectStorage() async throws {
        let workspace = try directory()
        let source = workspace.appendingPathComponent("Test.soundroom", isDirectory: true)
        try FileManager.default.createDirectory(at: source, withIntermediateDirectories: false)
        try Data("{}".utf8).write(to: source.appendingPathComponent("session.json"))
        let archive = workspace.appendingPathComponent("Test.soundroom.zip")
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/ditto")
        process.arguments = ["--norsrc", "-c", "-k", "--keepParent", source.path, archive.path]
        try process.run()
        process.waitUntilExit()
        XCTAssertEqual(process.terminationStatus, 0)
        let data = try Data(contentsOf: archive)
        let owner = UUID()
        let id = UUID()
        let files = try ServerFiles(dataDirectory: workspace.appendingPathComponent("Server", isDirectory: true))
        try files.beginUpload(
            clientID: owner, id: id, name: archive.lastPathComponent, bytes: data.count, sha256: hash(data))
        try await files.appendUpload(clientID: owner, id: id, offset: 0, data: data)
        let result = try await files.finishUpload(clientID: owner, id: id)
        XCTAssertEqual(result.deletingLastPathComponent().lastPathComponent, "Projects")
        XCTAssertEqual(try files.projects(), ["Test.soundroom"])
        XCTAssertEqual(try Data(contentsOf: result.appendingPathComponent("session.json")), Data("{}".utf8))
    }
    /// Minimal stored ZIP used to exercise traversal/link checks before extraction.
    nonisolated func zip(name: String, unixMode: Int = 0x81A4) -> Data {
        let name = Data(name.utf8)
        let payload = Data("{}".utf8)
        func bytes(_ value: Int, _ count: Int) -> Data {
            Data((0..<count).map { UInt8(truncatingIfNeeded: value >> ($0 * 8)) })
        }
        var local = Data()
        local += bytes(0x04034b50, 4)
        local += bytes(20, 2)
        local += bytes(0, 2)
        local += bytes(0, 2)
        local += bytes(0, 4)
        local += bytes(0, 4)
        local += bytes(payload.count, 4)
        local += bytes(payload.count, 4)
        local += bytes(name.count, 2)
        local += bytes(0, 2)
        local += name
        local += payload
        var central = Data()
        central += bytes(0x02014b50, 4)
        central += bytes(0x314, 2)
        central += bytes(20, 2)
        central += bytes(0, 2)
        central += bytes(0, 2)
        central += bytes(0, 4)
        central += bytes(0, 4)
        central += bytes(payload.count, 4)
        central += bytes(payload.count, 4)
        central += bytes(name.count, 2)
        central += bytes(0, 2)
        central += bytes(0, 2)
        central += bytes(0, 2)
        central += bytes(0, 2)
        central += bytes(unixMode << 16, 4)
        central += bytes(0, 4)
        central += name
        var end = Data()
        end += bytes(0x06054b50, 4)
        end += bytes(0, 4)
        end += bytes(1, 2)
        end += bytes(1, 2)
        end += bytes(central.count, 4)
        end += bytes(local.count, 4)
        end += bytes(0, 2)
        return local + central + end
    }
    func testZipValidationRejectsTraversalLinksAndMissingProjectManifest() throws {
        XCTAssertEqual(try ServerFiles.validateArchive(zip(name: "Test.soundroom/session.json")), "Test.soundroom")
        for name in [
            "../Test.soundroom/session.json", "/Test.soundroom/session.json", "Test.soundroom/../../session.json",
            "Test.soundroom\\session.json", "Test.soundroom/other.json",
        ] {
            XCTAssertThrowsError(try ServerFiles.validateArchive(zip(name: name)), name)
        }
        XCTAssertThrowsError(
            try ServerFiles.validateArchive(zip(name: "Test.soundroom/session.json", unixMode: 0xA1FF)))
    }
}
