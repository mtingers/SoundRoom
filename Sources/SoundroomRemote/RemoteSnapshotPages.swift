import CryptoKit
import Foundation

/// Immutable revision-pinned serialization, prepared off the realtime thread.
/// Every page is independently bounded; snapshots above the total bound fail
/// explicitly, never partially replace a client's current session.
public struct RemoteSnapshotPages: Sendable {
    public static let maximumSnapshotBytes = 64 * 1024 * 1024
    public static let pageBytes = 512 * 1024
    public let transferID: UUID
    public let revision: Int
    public let bytes: Data
    public let sha256: String
    public init(snapshot: RemoteSnapshot, transferID: UUID = UUID()) throws {
        self.transferID = transferID
        revision = snapshot.revision
        bytes = try RemoteV3Codec.encodeSnapshotDocument(snapshot)
        guard bytes.count <= Self.maximumSnapshotBytes else { throw RemoteError.oversizedFrame }
        sha256 = SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined()
    }
    /// Independent transfer identity and cursor sharing immutable serialized metadata.
    init(sharing document: RemoteSnapshotPages) {
        transferID = UUID()
        revision = document.revision
        bytes = document.bytes
        sha256 = document.sha256
    }
    public var pageCount: Int { (bytes.count + Self.pageBytes - 1) / Self.pageBytes }
    public func begin() -> RemoteSnapshotPage {
        .init(kind: .snapshotBegin, transferID: transferID, revision: revision, totalBytes: bytes.count, sha256: sha256)
    }
    public func page(at index: Int) throws -> RemoteSnapshotPage {
        guard (0..<pageCount).contains(index) else { throw RemoteError.invalidFrame }
        let offset = index * Self.pageBytes
        return .init(
            kind: .snapshotPage, transferID: transferID, revision: revision, offset: offset,
            data: bytes.subdata(in: offset..<min(bytes.count, offset + Self.pageBytes)))
    }
    public func end() -> RemoteSnapshotPage {
        .init(kind: .snapshotEnd, transferID: transferID, revision: revision, totalBytes: bytes.count, sha256: sha256)
    }
}
public struct RemoteSnapshotPage: Codable, Equatable, Sendable {
    public enum Kind: String, Codable, Sendable { case snapshotBegin, snapshotPage, snapshotEnd }
    public var version = 3
    public var kind: Kind
    public var transferID: UUID
    public var revision: Int
    public var totalBytes: Int?
    public var sha256: String?
    public var offset: Int?
    public var data: Data?
    public init(
        kind: Kind, transferID: UUID, revision: Int, totalBytes: Int? = nil, sha256: String? = nil, offset: Int? = nil,
        data: Data? = nil
    ) {
        self.kind = kind
        self.transferID = transferID
        self.revision = revision
        self.totalBytes = totalBytes
        self.sha256 = sha256
        self.offset = offset
        self.data = data
    }
}
/// Sequential, bounded assembly. Accepted deltas newer than the pinned revision
/// must be retained separately and applied after the verified snapshot installs.
public struct RemoteSnapshotAssembler {
    private var begin: RemoteSnapshotPage?
    private var buffer = Data()
    public init() {}
    public var nextOffset: Int { buffer.count }
    public mutating func cancel() {
        begin = nil
        buffer = Data()
    }
    public mutating func accept(_ page: RemoteSnapshotPage) throws -> RemoteSnapshot? {
        guard page.version == 3, page.revision >= 0, Double(page.revision) <= RemoteV3Codec.maximumSafeInteger else {
            throw RemoteError.invalidFrame
        }
        switch page.kind {
        case .snapshotBegin:
            guard let count = page.totalBytes, count > 0, count <= RemoteSnapshotPages.maximumSnapshotBytes,
                let digest = page.sha256, digest.count == 64, digest.allSatisfy(\.isHexDigit), page.data == nil,
                page.offset == nil
            else { throw RemoteError.invalidFrame }
            begin = page
            buffer = Data()
            buffer.reserveCapacity(min(count, RemoteSnapshotPages.pageBytes))
            return nil
        case .snapshotPage:
            guard let begin, page.transferID == begin.transferID, page.revision == begin.revision,
                let offset = page.offset, let data = page.data, !data.isEmpty,
                data.count <= RemoteSnapshotPages.pageBytes,
                offset == buffer.count, buffer.count <= begin.totalBytes! - data.count, page.totalBytes == nil,
                page.sha256 == nil
            else { throw RemoteError.invalidFrame }
            buffer.append(data)
            return nil
        case .snapshotEnd:
            guard let begin, page.transferID == begin.transferID, page.revision == begin.revision,
                page.totalBytes == begin.totalBytes, page.sha256 == begin.sha256, buffer.count == begin.totalBytes,
                page.offset == nil, page.data == nil
            else { throw RemoteError.invalidFrame }
            guard SHA256.hash(data: buffer).map({ String(format: "%02x", $0) }).joined() == begin.sha256 else {
                cancel()
                throw RemoteError.invalidFrame
            }
            let snapshot = try RemoteV3Codec.decodeSnapshotDocument(buffer)
            guard snapshot.revision == begin.revision else {
                cancel()
                throw RemoteError.invalidFrame
            }
            cancel()
            return snapshot
        }
    }
}
