import SoundroomCore
import XCTest

@testable import SoundroomEngineControl

final class ProjectStorageTests: XCTestCase {
    /// Tests exercise the actual staging/recovery implementation with a codec
    /// that has no dependency on AVFoundation, AppKit or an audio device.
    private final class Codec: ProjectMediaIO, @unchecked Sendable {
        private let lock = NSLock()
        private var writes = 0
        private var reads = 0
        private var writesOnMain = 0
        private var fail = false
        func setFailure(_ value: Bool) {
            lock.lock()
            fail = value
            lock.unlock()
        }
        func counts() -> (writes: Int, reads: Int, writesOnMain: Int) {
            lock.lock()
            defer { lock.unlock() }
            return (writes, reads, writesOnMain)
        }
        func readAudio(from url: URL) throws -> AudioSamples {
            lock.lock()
            reads += 1
            lock.unlock()
            return AudioSamples(channels: try JSONDecoder().decode([[Float]].self, from: Data(contentsOf: url)))
        }
        func writeFloatAudio(_ samples: AudioSamples, to url: URL) throws {
            lock.lock()
            writes += 1
            if Thread.isMainThread { writesOnMain += 1 }
            let shouldFail = fail
            lock.unlock()
            if shouldFail {
                try Data("partial source".utf8).write(to: url)
                throw ProjectStorageError.message("Injected storage write failure")
            }
            try JSONEncoder().encode(samples.channels).write(to: url)
        }
    }
    private func directory() throws -> URL {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(
            "soundroom-engine-storage-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: url) }
        return url
    }
    private func fixture() -> LoadedProject {
        let id = UUID()
        let samples = AudioSamples(channels: [[0.1, 0.2, -0.3], [0.4, -0.5, 0.6]])
        var session = Session()
        session.tracks[0].clips = [Clip(assetID: id, name: "Take", start: 7, length: 3)]
        return .init(session: session, assets: [id: samples])
    }
    func testWorkerCodecAndIncrementalMediaReuseWithoutEditorRuntime() async throws {
        let target = try directory().appendingPathComponent("Session.soundroom")
        let codec = Codec()
        var project = fixture()
        let first = try await ProjectPersistence.save(
            session: project.session, assets: project.assets, to: target, media: codec)
        XCTAssertEqual(first.encodedAssets, 1)
        XCTAssertEqual(codec.counts().writesOnMain, 0)
        project.session.tracks[0].gain = 0.25
        let second = try await ProjectPersistence.save(
            session: project.session, assets: project.assets, to: target, media: codec)
        XCTAssertEqual(second.reusedAssets, 1)
        XCTAssertEqual(second.encodedAssets, 0)
        XCTAssertEqual(codec.counts().writes, 1)
        let loaded = try await ProjectPersistence.load(from: target, media: codec)
        XCTAssertEqual(loaded.session, project.session)
        XCTAssertEqual(loaded.assets, project.assets)
        XCTAssertEqual(codec.counts().reads, 1)
    }
    func testPartialCodecWriteCannotReplaceSavedProjectOrLeakStaging() throws {
        let root = try directory()
        let target = root.appendingPathComponent("Session.soundroom")
        let codec = Codec()
        var project = fixture()
        try ProjectPersistence.saveSynchronously(
            session: project.session, assets: project.assets, to: target, media: codec)
        let metadata = try Data(contentsOf: target.appendingPathComponent("session.json"))
        let id = UUID()
        project.assets[id] = AudioSamples(channels: [[0.7]])
        project.session.name = "Failed replacement"
        codec.setFailure(true)
        XCTAssertThrowsError(
            try ProjectPersistence.saveSynchronously(
                session: project.session, assets: project.assets, to: target, media: codec))
        XCTAssertEqual(try Data(contentsOf: target.appendingPathComponent("session.json")), metadata)
        XCTAssertEqual(
            try FileManager.default.contentsOfDirectory(at: root, includingPropertiesForKeys: nil).map(
                \.lastPathComponent), ["Session.soundroom"])
        XCTAssertEqual(
            try ProjectPersistence.loadSynchronously(from: target, media: codec).session.name, "Untitled session")
    }
    func testRecoveryPreservesManualSaveAndOnlyClearsItsOwnRevision() async throws {
        let root = try directory()
        let target = root.appendingPathComponent("Manual.soundroom")
        let codec = Codec()
        var project = fixture()
        try ProjectPersistence.saveSynchronously(
            session: project.session, assets: project.assets, to: target, media: codec)
        project.session.name = "Unsaved edits"
        let revision = UUID()
        let recovery = RecoveryStore(directory: root.appendingPathComponent("Recovery"), media: codec)
        try await recovery.write(
            session: project.session, assets: project.assets, projectURL: target, revision: revision)
        let snapshot = try await recovery.recover()
        XCTAssertEqual(snapshot?.revision, revision)
        XCTAssertEqual(snapshot?.projectURL?.path, target.path)
        XCTAssertEqual(snapshot?.project.session.name, "Unsaved edits")
        XCTAssertEqual(
            try ProjectPersistence.loadSynchronously(from: target, media: codec).session.name, "Untitled session")
        try await recovery.clearIfRevision(UUID())
        let retained = try await recovery.recover()
        XCTAssertNotNil(retained)
        try await recovery.clearIfRevision(revision)
        let absent = try await recovery.recover()
        XCTAssertNil(absent)
    }
    func testFailedRecoveryReplacementRetainsPreviousSnapshotAndRevision() async throws {
        let root = try directory()
        let codec = Codec()
        var project = fixture()
        let firstRevision = UUID()
        let recovery = RecoveryStore(directory: root.appendingPathComponent("Recovery"), media: codec)
        try await recovery.write(
            session: project.session, assets: project.assets, projectURL: nil, revision: firstRevision)
        let id = UUID()
        project.assets[id] = AudioSamples(channels: [[0.9]])
        project.session.name = "Failed recovery"
        codec.setFailure(true)
        do {
            try await recovery.write(
                session: project.session, assets: project.assets, projectURL: nil, revision: UUID())
            XCTFail("Expected injected codec failure")
        } catch { XCTAssertTrue(error.localizedDescription.contains("Injected storage write failure")) }
        let retained = try await recovery.recover()
        XCTAssertEqual(retained?.revision, firstRevision)
        XCTAssertEqual(retained?.project.session.name, "Untitled session")
    }
}
