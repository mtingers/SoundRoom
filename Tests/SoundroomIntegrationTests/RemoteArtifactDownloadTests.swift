import CryptoKit
import SoundroomCore
import SoundroomRemote
import XCTest

@testable import Soundroom

@MainActor final class RemoteArtifactDownloadTests: XCTestCase {
    private func directory() throws -> URL {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("Download-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: url) }
        return url
    }
    func testOutOfOrderDownloadsRemainBoundedAndRetainExactBytes() async throws {
        let directory = try directory()
        let size = 8 * RemoteProtocol.maximumUploadChunkBytes
        let data = Data((0..<size).map { UInt8(truncatingIfNeeded: $0 &* 31) })
        let artifact = RemoteArtifact(
            id: UUID(), name: "fixture.bin", bytes: size,
            sha256: SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined())
        var times: [Double] = []
        for window in [1, 3] {
            let store = SessionStore(headless: true)
            var active = 0
            var peak = 0
            var requests = 0
            var firstFinished = false
            let remote = RemoteGUIController(
                store: store, credential: .native(try RemoteCredential.generate()), host: "fixture",
                commandSender: { command, _, _, _ in
                    guard case .downloadArtifact(let id, let offset, let count) = command else {
                        throw RemoteError.invalidFrame
                    }
                    XCTAssertEqual(id, artifact.id)
                    active += 1
                    peak = max(peak, active)
                    requests += 1
                    defer { active -= 1 }
                    if !firstFinished {
                        XCTAssertLessThan(
                            offset, window * RemoteProtocol.maximumUploadChunkBytes,
                            "Completed later chunks must remain inside the bounded window")
                    }
                    try await Task.sleep(nanoseconds: offset == 0 ? 40_000_000 : 8_000_000)
                    if offset == 0 { firstFinished = true }
                    let end = min(size, offset + count)
                    return .init(
                        requestID: UUID(), revision: 0,
                        payload: try JSONEncoder().encode(
                            RemoteArtifactChunk(
                                offset: offset,
                                data: data.subdata(in: offset..<end), complete: end == size)))
                })
            let output = directory.appendingPathComponent("window-\(window).bin")
            let start = ProcessInfo.processInfo.systemUptime
            try await remote.download(artifact, to: output, parallelism: window)
            times.append(ProcessInfo.processInfo.systemUptime - start)
            XCTAssertEqual(try Data(contentsOf: output), data)
            XCTAssertEqual(requests, 8)
            XCTAssertEqual(peak, window)
            XCTAssertEqual(active, 0)
            remote.disconnect()
        }
        print(
            "Two MiB download, injected first-chunk latency 40 ms / other chunks 8 ms: serial=\(times[0])s bounded3=\(times[1])s"
        )
    }
    func testInvalidChunksOrHashNeverReplaceThePreviousFile() async throws {
        let directory = try directory()
        let destination = directory.appendingPathComponent("keep.bin")
        let previous = Data("previous valid export".utf8)
        let bytes = Data(repeating: 9, count: 1024)
        for failure in ["short", "offset", "complete", "hash"] {
            try previous.write(to: destination)
            let store = SessionStore(headless: true)
            let remote = RemoteGUIController(
                store: store, credential: .native(try RemoteCredential.generate()), host: "fixture",
                commandSender: { _, _, _, _ in
                    let chunk = RemoteArtifactChunk(
                        offset: failure == "offset" ? 1 : 0,
                        data: failure == "short" ? bytes.dropLast() : bytes, complete: failure != "complete")
                    return .init(requestID: UUID(), revision: 0, payload: try JSONEncoder().encode(chunk))
                })
            let digest = SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined()
            let artifact = RemoteArtifact(
                id: UUID(), name: "fixture.bin", bytes: bytes.count,
                sha256: failure == "hash" ? String(repeating: "0", count: 64) : digest)
            do {
                try await remote.download(artifact, to: destination)
                XCTFail("Invalid \(failure) accepted")
            } catch {}
            XCTAssertEqual(try Data(contentsOf: destination), previous)
            XCTAssertFalse(
                try FileManager.default.contentsOfDirectory(atPath: directory.path).contains {
                    $0.hasPrefix(".soundroom-download-")
                })
            remote.disconnect()
        }
    }
}
