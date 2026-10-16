import XCTest
@testable import Soundroom

final class ProjectUploadArchiveTests: XCTestCase {
    private func fixture() throws -> URL {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("archive-\(UUID())").appendingPathComponent("A session.soundroom")
        try FileManager.default.createDirectory(at: url.appendingPathComponent("Media"), withIntermediateDirectories: true)
        return url
    }
    private func noise(_ count: Int) -> Data {
        var seed: UInt64 = 12_345
        return Data((0..<count).map { _ in seed = seed &* 6_364_136_223_846_793_005 &+ 1; return UInt8(truncatingIfNeeded: seed >> 32) })
    }
    func testAdaptiveCompressionPreservesCompressibleAudioAndStoresNoise() throws {
        let project = try fixture(); defer { try? FileManager.default.removeItem(at: project.deletingLastPathComponent()) }
        let audio = project.appendingPathComponent("Media/audio.wav")
        try noise(256 * 1_024).write(to: audio)
        XCTAssertEqual(try ProjectUploadArchive.compressionLevel(for: project, isLocal: true), 0)
        XCTAssertEqual(try ProjectUploadArchive.compressionLevel(for: project, isLocal: false), 0)
        try Data(repeating: 0, count: 256 * 1_024).write(to: audio)
        XCTAssertEqual(try ProjectUploadArchive.compressionLevel(for: project, isLocal: true), 1)
        // Moderately compressible PCM keeps a cheap local compression pass.
        var mixed = noise(256 * 1_024)
        for start in stride(from: 0, to: mixed.count, by: 1_024) { mixed.replaceSubrange(start..<(start + 128), with: repeatElement(UInt8(0), count: 128)) }
        try mixed.write(to: audio)
        XCTAssertEqual(try ProjectUploadArchive.compressionLevel(for: project, isLocal: true), 1)
        XCTAssertEqual(try ProjectUploadArchive.compressionLevel(for: project, isLocal: false), 6)
    }
    func testStoredArchiveRoundTripsEveryOriginalByteAndKeepsPackageRoot() async throws {
        let project = try fixture(), root = project.deletingLastPathComponent()
        defer { try? FileManager.default.removeItem(at: root) }
        let bytes = noise(256 * 1_024)
        try bytes.write(to: project.appendingPathComponent("Media/audio.wav"))
        let metadata = Data("{\"name\":\"A session\"}\n".utf8); try metadata.write(to: project.appendingPathComponent("Session.json"))
        let zip = root.appendingPathComponent("session.zip"), output = root.appendingPathComponent("unpacked")
        let level = try await ProjectUploadArchive.create(project, to: zip, isLocal: true)
        XCTAssertEqual(level, 0)
        let process = Process(); process.executableURL = URL(fileURLWithPath: "/usr/bin/ditto")
        process.arguments = ["-x", "-k", zip.path, output.path]; try process.run(); process.waitUntilExit()
        XCTAssertEqual(process.terminationStatus, 0)
        XCTAssertEqual(try Data(contentsOf: output.appendingPathComponent(project.lastPathComponent).appendingPathComponent("Media/audio.wav")), bytes)
        XCTAssertEqual(try Data(contentsOf: output.appendingPathComponent(project.lastPathComponent).appendingPathComponent("Session.json")), metadata)
    }
    func testFastLocalDeflateRoundTripsCompressibleAudio() async throws {
        let project = try fixture(), root = project.deletingLastPathComponent()
        defer { try? FileManager.default.removeItem(at: root) }
        let bytes = Data((0..<(256 * 1024)).map { UInt8(truncatingIfNeeded: $0) })
        try bytes.write(to: project.appendingPathComponent("Media/audio.wav"))
        let zip = root.appendingPathComponent("session.zip"), output = root.appendingPathComponent("unpacked")
        let level = try await ProjectUploadArchive.create(project, to: zip, isLocal: true)
        XCTAssertEqual(level, 1)
        let process = Process(); process.executableURL = URL(fileURLWithPath: "/usr/bin/ditto")
        process.arguments = ["-x", "-k", zip.path, output.path]; try process.run(); process.waitUntilExit()
        XCTAssertEqual(process.terminationStatus, 0)
        XCTAssertEqual(try Data(contentsOf: output.appendingPathComponent(project.lastPathComponent).appendingPathComponent("Media/audio.wav")), bytes)
    }

    func testSyntheticArchivePreparationMeasurements() async throws {
        let project = try fixture(), root = project.deletingLastPathComponent()
        defer { try? FileManager.default.removeItem(at: root) }
        let count = 32 * 1_024 * 1_024
        var measurements: [[String: Any]] = []
        for material in ["incompressible", "periodic"] {
            let bytes = material == "incompressible" ? noise(count) : Data((0..<count).map { UInt8(truncatingIfNeeded: $0) })
            try bytes.write(to: project.appendingPathComponent("Media/audio.wav"))
            for trial in 0..<2 {
                for adaptive in trial == 0 ? [false, true] : [true, false] {
                    let zip = root.appendingPathComponent("measurement.zip")
                    let start = ProcessInfo.processInfo.systemUptime
                    let level: Int
                    if adaptive { level = try await ProjectUploadArchive.create(project, to: zip, isLocal: true) }
                    else {
                        level = 6
                        let process = Process(); process.executableURL = URL(fileURLWithPath: "/usr/bin/ditto")
                        process.arguments = ["-c", "-k", "--norsrc", "--keepParent", project.path, zip.path]
                        try process.run(); process.waitUntilExit(); XCTAssertEqual(process.terminationStatus, 0)
                    }
                    let elapsed = ProcessInfo.processInfo.systemUptime - start
                    let size = try zip.resourceValues(forKeys: [.fileSizeKey]).fileSize!
                    if adaptive { XCTAssertEqual(level, material == "incompressible" ? 0 : 1) }
                    if material == "periodic" { XCTAssertLessThan(size, count / 100) }
                    measurements.append(["material": material, "adaptive": adaptive, "trial": trial, "seconds": elapsed, "archiveBytes": size, "sourceBytes": count, "compressionLevel": level])
                    try FileManager.default.removeItem(at: zip)
                }
            }
        }
        let report = try JSONSerialization.data(withJSONObject: ["scope": "Synthetic 32 MiB files, warm filesystem; elapsed includes adaptive sampling and process launch; trial order alternates", "measurements": measurements], options: [.prettyPrinted, .sortedKeys])
        if let path = ProcessInfo.processInfo.environment["SOUNDROOM_PERF_REPORT_PATH"] { try report.write(to: URL(fileURLWithPath: path)) }
        print("ARCHIVE_PERFORMANCE " + String(decoding: report, as: UTF8.self))
    }
}
