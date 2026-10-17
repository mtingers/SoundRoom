import CryptoKit
import Foundation
import SoundroomRemote

/// Owns the server's named storage; callers never pass client filesystem paths.
@MainActor public final class ServerFiles {
    public nonisolated static let maximumUploadBytes = 2 * 1024 * 1024 * 1024
    public nonisolated static let maximumChunkBytes = 256 * 1024
    public nonisolated static let maximumGlobalUploadBytes = 8 * 1024 * 1024 * 1024
    public let dataDirectory: URL
    private let projectsDirectory: URL
    private let uploadsDirectory: URL
    private let exportsDirectory: URL
    private let artifactsDirectory: URL
    private var artifactOrder: [UUID] = []
    private struct Upload {
        var clientID: UUID
        var url: URL
        var name: String
        var bytes: Int
        var received: Int
        var hash: String
        var handle: FileHandle
        var finishing = false
        var writing = false
        var cancelled = false
        var expansionReservation = 0
    }
    private var uploads: [UUID: Upload] = [:]
    private var artifacts: [UUID: URL] = [:]
    public init(dataDirectory: URL) throws {
        self.dataDirectory = dataDirectory.standardizedFileURL
        projectsDirectory = self.dataDirectory.appendingPathComponent("Projects", isDirectory: true)
        uploadsDirectory = self.dataDirectory.appendingPathComponent("Uploads", isDirectory: true)
        exportsDirectory = self.dataDirectory.appendingPathComponent("Exports", isDirectory: true)
        artifactsDirectory = exportsDirectory.appendingPathComponent("Artifacts", isDirectory: true)
        for directory in [
            self.dataDirectory, projectsDirectory, uploadsDirectory, exportsDirectory, artifactsDirectory,
        ] {
            try FileManager.default.createDirectory(
                at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            let values = try directory.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
            guard values.isDirectory == true, values.isSymbolicLink != true else {
                throw FileError("Server storage must be a real directory.")
            }
        }
    }
    public func projects() throws -> [String] {
        try FileManager.default.contentsOfDirectory(
            at: projectsDirectory, includingPropertiesForKeys: [.isSymbolicLinkKey, .isDirectoryKey],
            options: [.skipsHiddenFiles]
        ).filter { url in
            guard url.pathExtension == "soundroom",
                let values = try? url.resourceValues(forKeys: [.isSymbolicLinkKey, .isDirectoryKey])
            else { return false }
            return values.isSymbolicLink != true && values.isDirectory == true
        }.map(\.lastPathComponent).sorted()
    }
    public func projectURL(id: String) throws -> URL {
        let url = try saveProjectURL(name: id)
        guard FileManager.default.fileExists(atPath: url.appendingPathComponent("session.json").path) else {
            throw FileError("That server project does not exist.")
        }
        return url
    }
    public func saveProjectURL(name: String) throws -> URL {
        try managedURL(name: name, extension: "soundroom", in: projectsDirectory)
    }
    public func exportURL(name: String) throws -> URL {
        try managedURL(name: name, extension: "wav", in: exportsDirectory)
    }
    private func managedURL(name: String, extension suffix: String, in directory: URL) throws -> URL {
        let name = try Self.validName(name)
        let file = name.hasSuffix("." + suffix) ? name : name + "." + suffix
        let url = directory.appendingPathComponent(file)
        if FileManager.default.fileExists(atPath: url.path) {
            guard try url.resourceValues(forKeys: [.isSymbolicLinkKey]).isSymbolicLink != true else {
                throw FileError("Symbolic links are not accepted in server storage.")
            }
        }
        return url
    }
    public nonisolated static func validName(_ name: String) throws -> String {
        let value = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !value.isEmpty, value.utf8.count <= 128, value != ".", value != "..", !value.hasPrefix("."),
            !value.contains("/"), !value.contains("\\"), !value.contains(":"),
            !value.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) })
        else { throw FileError("Use a simple name, without directories or control characters (1–128 bytes).") }
        return value
    }
    public func beginUpload(clientID: UUID, id: UUID, name: String, bytes: Int, sha256: String) throws {
        let name = try Self.validName(name)
        let hash = sha256.lowercased()
        guard bytes > 0, bytes <= Self.maximumUploadBytes, hash.count == 64,
            hash.allSatisfy({ $0.isHexDigit && $0.isASCII }), uploads[id] == nil
        else { throw FileError("Invalid upload size, hash, or duplicate upload ID.") }
        let owned = uploads.values.filter { $0.clientID == clientID }
        guard owned.count < 2, owned.reduce(bytes, { $0 + $1.bytes }) <= Self.maximumUploadBytes,
            uploads.count < 16,
            uploads.values.reduce(bytes, { $0 + $1.bytes + $1.expansionReservation }) <= Self.maximumGlobalUploadBytes
        else { throw FileError("The upload quota is full; finish or cancel an existing upload.") }
        let url = uploadsDirectory.appendingPathComponent(id.uuidString + "-" + name)
        guard (try? FileManager.default.attributesOfItem(atPath: url.path)) == nil,
            FileManager.default.createFile(atPath: url.path, contents: nil, attributes: [.posixPermissions: 0o600])
        else { throw FileError("Could not create server upload.") }
        let handle = try FileHandle(forWritingTo: url)
        uploads[id] = Upload(
            clientID: clientID, url: url, name: name, bytes: bytes, received: 0, hash: hash, handle: handle)
    }
    public func appendUpload(clientID: UUID, id: UUID, offset: Int, data: Data) async throws {
        guard var upload = uploads[id], upload.clientID == clientID, !upload.finishing, !upload.writing,
            offset == upload.received,
            !data.isEmpty, data.count <= Self.maximumChunkBytes, data.count <= upload.bytes - upload.received
        else { throw FileError("Invalid upload owner, offset or chunk size.") }
        upload.writing = true
        uploads[id] = upload
        let handle = upload.handle
        do {
            try await Task.detached(priority: .utility) { try handle.write(contentsOf: data) }.value
            guard var current = uploads[id], !current.cancelled else { throw FileError("Upload cancelled.") }
            current.received += data.count
            current.writing = false
            uploads[id] = current
        } catch {
            try? upload.handle.close()
            try? FileManager.default.removeItem(at: upload.url)
            uploads.removeValue(forKey: id)
            throw error
        }
    }
    public func finishUpload(clientID: UUID, id: UUID) async throws -> URL {
        guard var upload = uploads[id], upload.clientID == clientID, !upload.finishing, !upload.writing,
            upload.received == upload.bytes
        else { throw FileError("The upload is incomplete or belongs to another client.") }
        guard uploads.values.filter({ $0.finishing }).count < 2 else {
            throw FileError("Two uploads are being verified; wait and retry this upload ID.")
        }
        if upload.name.lowercased().hasSuffix(".soundroom.zip") {
            guard
                uploads.values.reduce(Self.maximumUploadBytes, { $0 + $1.bytes + $1.expansionReservation })
                    <= Self.maximumGlobalUploadBytes
            else { throw FileError("The ZIP expansion quota is full; wait for another import to finish.") }
            upload.expansionReservation = Self.maximumUploadBytes
        }
        try upload.handle.close()
        upload.finishing = true
        uploads[id] = upload
        do {
            let url = upload.url
            let hash = upload.hash
            let name = upload.name
            let directory = uploadsDirectory
            let verified = try await Task.detached(priority: .utility) {
                guard try Self.hashFile(url) == hash else {
                    throw FileError("The uploaded file's SHA-256 does not match.")
                }
                if name.lowercased().hasSuffix(".soundroom.zip") {
                    return try Self.extractProjectArchive(url, directory: directory)
                }
                guard
                    ["wav", "aiff", "aif", "caf", "m4a", "mp3", "flac", "mp4"].contains(url.pathExtension.lowercased())
                else { throw FileError("Upload audio or a .soundroom.zip project package.") }
                return url
            }.value
            var result = verified
            if verified.pathExtension == "soundroom" {
                let originalName = verified.deletingPathExtension().lastPathComponent
                var target = try saveProjectURL(name: originalName)
                if FileManager.default.fileExists(atPath: target.path) {
                    target = try saveProjectURL(name: originalName + " " + String(UUID().uuidString.prefix(8)))
                }
                try FileManager.default.moveItem(at: verified, to: target)
                try? FileManager.default.removeItem(at: verified.deletingLastPathComponent())
                result = target
            }
            uploads.removeValue(forKey: id)
            if verified != upload.url { try? FileManager.default.removeItem(at: upload.url) }
            return result
        } catch {
            uploads.removeValue(forKey: id)
            try? FileManager.default.removeItem(at: upload.url)
            throw error
        }
    }
    public func cancelUploads(clientID: UUID) {
        for (id, var upload) in uploads where upload.clientID == clientID && !upload.finishing {
            if upload.writing {
                upload.cancelled = true
                uploads[id] = upload
                continue
            }
            try? upload.handle.close()
            try? FileManager.default.removeItem(at: upload.url)
            uploads.removeValue(forKey: id)
        }
    }
    public func artifactID(for url: URL) throws -> UUID {
        let normalized = url.standardizedFileURL
        guard normalized.deletingLastPathComponent() == exportsDirectory,
            try normalized.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey]).isRegularFile == true,
            try normalized.resourceValues(forKeys: [.isSymbolicLinkKey]).isSymbolicLink != true
        else { throw FileError("Artifacts must be regular files in server exports.") }
        let id = UUID()
        let immutable = artifactsDirectory.appendingPathComponent(id.uuidString + "." + normalized.pathExtension)
        // Exports commit through rename, so a hard link preserves this completed
        // inode while a later export replaces the human-readable name.
        try FileManager.default.linkItem(at: normalized, to: immutable)
        artifacts[id] = immutable
        artifactOrder.append(id)
        if artifactOrder.count > 256 {
            let expired = artifactOrder.removeFirst()
            if let old = artifacts.removeValue(forKey: expired) { try? FileManager.default.removeItem(at: old) }
        }
        return id
    }
    public func artifact(for url: URL) async throws -> RemoteArtifact {
        let id = try artifactID(for: url)
        guard let immutable = artifacts[id] else { throw FileError("The artifact expired.") }
        let values = try immutable.resourceValues(forKeys: [.fileSizeKey])
        let hash = try await Task.detached(priority: .utility) { try Self.hashFile(immutable) }.value
        return RemoteArtifact(id: id, name: url.lastPathComponent, bytes: values.fileSize ?? 0, sha256: hash)
    }
    public func releaseArtifact(_ id: UUID) {
        if let file = artifacts.removeValue(forKey: id) { try? FileManager.default.removeItem(at: file) }
        artifactOrder.removeAll { $0 == id }
    }
    public func readArtifactChunkAsync(id: UUID, offset: Int, bytes: Int) async throws -> RemoteArtifactChunk {
        guard let url = artifacts[id], offset >= 0, bytes > 0, bytes <= Self.maximumChunkBytes else {
            throw FileError("Unknown artifact or invalid chunk range.")
        }
        return try await Task.detached(priority: .utility) {
            let total = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
            let data = try Self.readArtifactFile(url, offset: offset, bytes: bytes)
            return RemoteArtifactChunk(offset: offset, data: data, complete: offset + data.count >= total)
        }.value
    }
    public func readArtifactAsync(id: UUID, offset: Int, bytes: Int) async throws -> Data {
        guard let url = artifacts[id], offset >= 0, bytes > 0, bytes <= Self.maximumChunkBytes else {
            throw FileError("Unknown artifact or invalid chunk range.")
        }
        return try await Task.detached(priority: .utility) {
            try Self.readArtifactFile(url, offset: offset, bytes: bytes)
        }.value
    }
    public func readArtifactAsync(id: String, offset: Int, bytes: Int) async throws -> Data {
        guard let id = UUID(uuidString: id) else { throw FileError("Unknown artifact ID.") }
        return try await readArtifactAsync(id: id, offset: offset, bytes: bytes)
    }
    public func readArtifact(id: UUID, offset: Int, bytes: Int) throws -> Data {
        guard let url = artifacts[id], offset >= 0, bytes > 0, bytes <= Self.maximumChunkBytes else {
            throw FileError("Unknown artifact or invalid chunk range.")
        }
        return try Self.readArtifactFile(url, offset: offset, bytes: bytes)
    }
    nonisolated private static func readArtifactFile(_ url: URL, offset: Int, bytes: Int) throws -> Data {
        let values = try url.resourceValues(forKeys: [.fileSizeKey, .isRegularFileKey, .isSymbolicLinkKey])
        guard values.isRegularFile == true, values.isSymbolicLink != true, offset <= values.fileSize ?? -1 else {
            throw FileError("Invalid artifact range.")
        }
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        try handle.seek(toOffset: UInt64(offset))
        return try handle.read(upToCount: bytes) ?? Data()
    }
    public func readArtifact(id: String, offset: Int, bytes: Int) throws -> Data {
        guard let id = UUID(uuidString: id) else { throw FileError("Unknown artifact ID.") }
        return try readArtifact(id: id, offset: offset, bytes: bytes)
    }
    public nonisolated static func hashFile(_ url: URL) throws -> String {
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        var hash = SHA256()
        while let bytes = try handle.read(upToCount: 256 * 1024), !bytes.isEmpty {
            try Task.checkCancellation()
            hash.update(data: bytes)
        }
        return hash.finalize().map { String(format: "%02x", $0) }.joined()
    }
    struct FileError: Error, LocalizedError, CustomStringConvertible, Sendable {
        let description: String
        init(_ description: String) { self.description = description }
        var errorDescription: String? { description }
    }
}

extension ServerFiles {
    /// Validate central and local ZIP headers before ditto can write anything.
    /// ZIP64, encryption, links, devices, path escapes and oversized expansion are rejected.
    public nonisolated static func validateArchive(_ data: Data) throws -> String {
        func u16(_ offset: Int) throws -> Int {
            guard offset >= 0, offset + 2 <= data.count else { throw FileError("Truncated ZIP header.") }
            return Int(data[offset]) | Int(data[offset + 1]) << 8
        }
        func u32(_ offset: Int) throws -> Int { try u16(offset) | u16(offset + 2) << 16 }
        func validateExtra(_ start: Int, _ length: Int) throws {
            var position = start
            let allowed: Set<Int> = [0x5455, 0x5855, 0x7855, 0x7875, 0x000A]
            while position < start + length {
                guard position + 4 <= start + length else { throw FileError("Truncated ZIP extra field.") }
                let tag = try u16(position)
                let size = try u16(position + 2)
                guard allowed.contains(tag), position + 4 + size <= start + length else {
                    throw FileError("ZIP link, alternate-path or unsupported extra field.")
                }
                position += 4 + size
            }
        }
        guard data.count >= 22 else { throw FileError("Invalid project archive.") }
        var end: Int?
        for offset in stride(from: data.count - 22, through: max(0, data.count - 65_557), by: -1) {
            if try u32(offset) == 0x06054b50, try offset + 22 + u16(offset + 20) == data.count {
                end = offset
                break
            }
        }
        guard let end, try u16(end + 4) == 0, try u16(end + 6) == 0 else {
            throw FileError("Multipart or ZIP64 archives are unsupported.")
        }
        let entries = try u16(end + 10)
        let start = try u32(end + 16)
        let size = try u32(end + 12)
        guard entries > 0, entries <= 16_384, try u16(end + 8) == entries, start + size <= end else {
            throw FileError("Invalid ZIP directory.")
        }
        var offset = start
        var expanded = 0
        var package: String?
        var seen: Set<String> = []
        for _ in 0..<entries {
            guard try u32(offset) == 0x02014b50 else { throw FileError("Invalid ZIP entry.") }
            let flags = try u16(offset + 8)
            let packed = try u32(offset + 20)
            let unpacked = try u32(offset + 24)
            let nameLength = try u16(offset + 28)
            let extra = try u16(offset + 30)
            let comment = try u16(offset + 32)
            let local = try u32(offset + 42)
            let attributes = try u32(offset + 38)
            let mode = (attributes >> 16) & 0xF000
            guard flags & 1 == 0, [0, 0x4000, 0x8000].contains(mode), nameLength > 0, nameLength <= 1024,
                [0, 8].contains(try u16(offset + 10)),
                offset + 46 + nameLength + extra + comment <= start + size,
                unpacked <= maximumUploadBytes - expanded, packed != 0xFFFFFFFF, unpacked != 0xFFFFFFFF
            else { throw FileError("Unsafe or oversized ZIP entry.") }
            try validateExtra(offset + 46 + nameLength, extra)
            let bytes = data.subdata(in: offset + 46..<offset + 46 + nameLength)
            guard let name = String(data: bytes, encoding: .utf8), !name.hasPrefix("/"), !name.contains("\\"),
                !name.contains(":"),
                !name.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) })
            else { throw FileError("Unsafe ZIP path.") }
            let components = name.split(separator: "/", omittingEmptySubsequences: false)
            guard !components.isEmpty, !components.contains(".."), !components.contains("."),
                !components.dropLast().contains(""), let root = components.first, root.hasSuffix(".soundroom"),
                package == nil || package == String(root), seen.insert(name).inserted
            else { throw FileError("The ZIP must contain exactly one .soundroom package with safe paths.") }
            _ = try validName(String(root))
            package = String(root)
            expanded += unpacked
            guard try u32(local) == 0x04034b50, try u16(local + 6) == flags, try u16(local + 26) == nameLength else {
                throw FileError("ZIP local header does not match its directory.")
            }
            let localExtra = try u16(local + 28)
            let content = local + 30 + nameLength + localExtra
            try validateExtra(local + 30 + nameLength, localExtra)
            guard content + packed <= start, data.subdata(in: local + 30..<local + 30 + nameLength) == bytes else {
                throw FileError("ZIP local path or content range does not match.")
            }
            offset += 46 + nameLength + extra + comment
        }
        guard offset == start + size, let package, seen.contains(package + "/session.json") else {
            throw FileError("The archive is missing session.json.")
        }
        return package
    }
    public nonisolated static func extractProjectArchive(_ url: URL, directory: URL) throws -> URL {
        let data = try Data(contentsOf: url, options: .mappedIfSafe)
        let package = try validateArchive(data)
        let destination = directory.appendingPathComponent("unpacked-" + UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(
            at: destination, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        do {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/usr/bin/ditto")
            process.arguments = ["-x", "-k", "--norsrc", url.path, destination.path]
            process.standardOutput = FileHandle.nullDevice
            process.standardError = FileHandle.nullDevice
            try process.run()
            process.waitUntilExit()
            guard process.terminationStatus == 0 else { throw FileError("Could not extract the project archive.") }
            let keys: [URLResourceKey] = [.isSymbolicLinkKey, .isRegularFileKey, .isDirectoryKey, .fileSizeKey]
            guard let enumerator = FileManager.default.enumerator(at: destination, includingPropertiesForKeys: keys)
            else { throw FileError("Could not inspect extracted project.") }
            var total = 0
            var count = 0
            for case let entry as URL in enumerator {
                count += 1
                let values = try entry.resourceValues(forKeys: Set(keys))
                guard count <= 16_384, values.isSymbolicLink != true,
                    values.isDirectory == true || values.isRegularFile == true,
                    entry.resolvingSymlinksInPath().path.hasPrefix(destination.path + "/")
                else { throw FileError("The extracted project contains an unsafe file.") }
                total += values.fileSize ?? 0
                guard total <= maximumUploadBytes else {
                    throw FileError("The extracted project exceeds its disk quota.")
                }
            }
            return destination.appendingPathComponent(package, isDirectory: true)
        } catch {
            try? FileManager.default.removeItem(at: destination)
            throw error
        }
    }
}
