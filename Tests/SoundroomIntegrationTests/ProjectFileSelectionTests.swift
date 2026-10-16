import Foundation
import UniformTypeIdentifiers
import XCTest
import SoundroomCore
@testable import Soundroom

final class ProjectFileSelectionTests: XCTestCase {
    private func withDirectory(_ body: (URL) throws -> Void) throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("soundroom-open-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        try body(root)
    }
    func testOrdinaryProjectDirectoryCanBeSelectedAndLoadedWithoutPackageMetadata() throws {
        try withDirectory { root in
            let project = root.appendingPathComponent("Nutshell.soundroom", isDirectory: true)
            var session = Session(); session.name = "Nutshell fixture"
            try ProjectPersistence.saveSynchronously(session: session, assets: [:], to: project)
            // No Finder flags or content-type registration are needed.
            XCTAssertNoThrow(try ProjectFileSelection.validate(project))
            let loaded = try ProjectPersistence.loadSynchronously(from: project)
            XCTAssertEqual(loaded.session.name, "Nutshell fixture")
            XCTAssertEqual(loaded.session.tracks.count, session.tracks.count)
        }
    }
    func testPickerIncludesOrdinaryFoldersAlongsideTheDeclaredProjectType() {
        XCTAssertTrue(ProjectFileSelection.contentTypes.contains(.folder), "A package-only filter disables unregistered .soundroom directories")
        XCTAssertTrue(ProjectFileSelection.contentTypes.contains { $0.identifier == "com.soundroom.session" })
    }
    func testContainingDirectoryCannotBeMistakenForTheProject() throws {
        try withDirectory { root in
            try Data("{}".utf8).write(to: root.appendingPathComponent("session.json"))
            XCTAssertThrowsError(try ProjectFileSelection.validate(root)) { error in
                XCTAssertTrue(error.localizedDescription.contains(".soundroom project folder"))
            }
        }
    }
    func testFileWithProjectExtensionIsRejectedBeforeUpload() throws {
        try withDirectory { root in
            let file = root.appendingPathComponent("Nutshell.soundroom")
            try Data("{}".utf8).write(to: file)
            XCTAssertThrowsError(try ProjectFileSelection.validate(file))
        }
    }
    func testIncompleteProjectExplainsTheMissingMetadata() throws {
        try withDirectory { root in
            let project = root.appendingPathComponent("Incomplete.soundroom", isDirectory: true)
            try FileManager.default.createDirectory(at: project, withIntermediateDirectories: true)
            XCTAssertThrowsError(try ProjectFileSelection.validate(project)) { error in
                XCTAssertTrue(error.localizedDescription.contains("session.json"))
            }
            try FileManager.default.createDirectory(at: project.appendingPathComponent("session.json"), withIntermediateDirectories: true)
            XCTAssertThrowsError(try ProjectFileSelection.validate(project))
        }
    }
}
