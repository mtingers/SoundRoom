import Foundation
import CryptoKit
import SoundroomRemote

/// CLI media transfer is file-only; hardware remains exclusively on the server.
enum TUIFileTransfer {
    static let limit = 2 * 1024 * 1024 * 1024
    struct Prepared: Sendable { var url: URL; var bytes: Int; var hash: String; var temporary: Bool }
    static func hash(_ url: URL) throws -> String {
        let handle = try FileHandle(forReadingFrom: url); defer { try? handle.close() }; var digest = SHA256()
        while let chunk = try handle.read(upToCount: 256 * 1024), !chunk.isEmpty { try Task.checkCancellation(); digest.update(data: chunk) }
        return digest.finalize().map { String(format: "%02x", $0) }.joined()
    }
    static func prepare(_ source: URL) throws -> Prepared {
        let values = try source.resourceValues(forKeys: [.isDirectoryKey, .isRegularFileKey, .fileSizeKey])
        var file = source, temporary = false
        if values.isDirectory == true, source.pathExtension == "soundroom" {
            let directory = FileManager.default.temporaryDirectory.appendingPathComponent("soundroom-cli-" + UUID().uuidString, isDirectory: true)
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
            file = directory.appendingPathComponent(source.lastPathComponent + ".zip"); temporary = true
            let process = Process(); process.executableURL = URL(fileURLWithPath: "/usr/bin/ditto")
            process.arguments = ["--norsrc", "-c", "-k", "--keepParent", source.path, file.path]
            process.standardOutput = FileHandle.nullDevice; process.standardError = FileHandle.nullDevice
            do { try process.run(); process.waitUntilExit(); guard process.terminationStatus == 0 else { throw TransferError("Could not package the project.") } }
            catch { try? FileManager.default.removeItem(at: directory); throw error }
        } else if values.isRegularFile != true { throw TransferError("Choose an audio file or .soundroom project package.") }
        do {
            let bytes = try file.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
            guard bytes > 0, bytes <= limit else { throw TransferError("Imports must contain 1 byte…2 GiB.") }
            return Prepared(url: file, bytes: bytes, hash: try hash(file), temporary: temporary)
        } catch { if temporary { try? FileManager.default.removeItem(at: file.deletingLastPathComponent()) }; throw error }
    }
    @MainActor static func upload(_ file: URL, connection: TUIConnection, trackID: UUID?, frame: Int) async throws -> String {
        let prepared = try await Task.detached(priority: .utility) { try prepare(file) }.value
        defer { if prepared.temporary { try? FileManager.default.removeItem(at: prepared.url.deletingLastPathComponent()) } }
        let id = UUID()
        let beginning = try await connection.client.request(.beginUpload(id: id, name: prepared.url.lastPathComponent, bytes: prepared.bytes, sha256: prepared.hash))
        guard beginning.accepted else { throw TransferError(beginning.message) }
        let reader = try FileHandle(forReadingFrom: prepared.url); defer { try? reader.close() }
        var offset = 0
        while offset < prepared.bytes {
            let data = try await Task.detached(priority: .utility) { try reader.read(upToCount: RemoteProtocol.maximumUploadChunkBytes) ?? Data() }.value
            guard !data.isEmpty, data.count <= prepared.bytes - offset else { throw TransferError("The local file changed during upload.") }
            let reply = try await connection.client.request(.uploadChunk(id: id, offset: offset, data: data))
            guard reply.accepted else { throw TransferError(reply.message) }; offset += data.count
        }
        let reply = try await connection.client.request(.finishUpload(id: id, trackID: trackID, frame: frame), timeout: 600)
        guard reply.accepted else { throw TransferError(reply.message) }
        return reply.message.isEmpty ? "Imported into the server." : TUIRenderer.safe(reply.message)
    }
    @MainActor static func download(_ id: UUID, to target: URL, connection: TUIConnection) async throws {
        guard (try? FileManager.default.attributesOfItem(atPath: target.path)) == nil else { throw TransferError("The output already exists; choose a new filename.") }
        let staging = target.deletingLastPathComponent().appendingPathComponent(".soundroom-download-" + UUID().uuidString)
        guard FileManager.default.createFile(atPath: staging.path, contents: nil, attributes: [.posixPermissions: 0o600]) else { throw TransferError("Could not create the output file.") }
        defer { try? FileManager.default.removeItem(at: staging) }
        let writer = try FileHandle(forWritingTo: staging); defer { try? writer.close() }
        var offset = 0
        while true {
            let reply = try await connection.client.request(.downloadArtifact(id: id, offset: offset, bytes: RemoteProtocol.maximumUploadChunkBytes))
            guard reply.accepted, let payload = reply.payload else { throw TransferError(reply.message.isEmpty ? "The artifact download failed." : reply.message) }
            let chunk = try JSONDecoder().decode(RemoteArtifactChunk.self, from: payload)
            guard chunk.offset == offset, chunk.data.count <= RemoteProtocol.maximumUploadChunkBytes,
                  chunk.data.count <= limit - offset, !chunk.data.isEmpty || chunk.complete else { throw TransferError("Invalid artifact chunk or artifact larger than 2 GiB.") }
            let data = chunk.data
            try await Task.detached(priority: .utility) { try writer.write(contentsOf: data) }.value
            offset += chunk.data.count
            if chunk.complete { break }
        }
        try writer.close()
        // moveItem refuses to overwrite if a file appeared during the transfer.
        try FileManager.default.moveItem(at: staging, to: target)
    }
    struct TransferError: Error, CustomStringConvertible { let description: String; init(_ text: String) { description = text } }
}
