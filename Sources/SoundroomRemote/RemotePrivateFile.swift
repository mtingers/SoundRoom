import Foundation
import Darwin

/// Reads a bounded host-owned secret without following links or widening access.
public enum RemotePrivateFile {
    /// Atomically replaces a private regular file; the temporary is exclusive,
    /// host-owned and never readable by other users, even before the rename.
    public static func write(_ data: Data, to url: URL) throws {
        let parent = url.deletingLastPathComponent()
        try FileManager.default.createDirectory(at: parent, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        let temporary = parent.appendingPathComponent(".soundroom-private-\(UUID().uuidString)")
        let descriptor = open(temporary.path, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0o600)
        guard descriptor >= 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
        defer { close(descriptor); try? FileManager.default.removeItem(at: temporary) }
        try data.withUnsafeBytes { buffer in
            var offset = 0
            while offset < buffer.count {
                let count = Darwin.write(descriptor, buffer.baseAddress!.advanced(by: offset), buffer.count - offset)
                if count < 0, errno == EINTR { continue }
                guard count > 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
                offset += count
            }
        }
        guard fsync(descriptor) == 0, Darwin.rename(temporary.path, url.path) == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    }
    public static func read(_ url: URL, maximumBytes: Int = 16_384) throws -> Data {
        let descriptor = open(url.path, O_RDONLY | O_NOFOLLOW | O_CLOEXEC)
        guard descriptor >= 0 else { throw RemoteError.invalidCredential }
        defer { close(descriptor) }
        var info = stat()
        guard fstat(descriptor, &info) == 0, info.st_mode & S_IFMT == S_IFREG, info.st_uid == getuid(),
              info.st_mode & 0o077 == 0, info.st_size > 0, info.st_size <= maximumBytes else { throw RemoteError.invalidCredential }
        var data = Data(count: Int(info.st_size))
        try data.withUnsafeMutableBytes { buffer in
            var offset = 0
            while offset < buffer.count {
                let count = Darwin.read(descriptor, buffer.baseAddress!.advanced(by: offset), buffer.count - offset)
                if count < 0, errno == EINTR { continue }
                guard count > 0 else { throw RemoteError.invalidCredential }; offset += count
            }
        }
        return data
    }
}
