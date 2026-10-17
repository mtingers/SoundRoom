import SoundroomCore
import XCTest

@testable import Soundroom

final class EffectPresetTests: XCTestCase {
    private func temporaryURL() -> URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("Soundroom-presets-\(UUID())", isDirectory: true)
            .appendingPathComponent("presets.json")
    }

    func testCustomPresetsSurviveReloadRenameDeleteAndRestoreAcrossEffects() throws {
        try MainActor.assumeIsolated {
            let url = temporaryURL()
            defer { try? FileManager.default.removeItem(at: url.deletingLastPathComponent()) }
            let library = EffectPresetLibrary(url: url)
            let echo = try library.save(name: "  My room  ", kind: .echo, parameters: ["time": 540, "feedback": 0.7])
            let room = try library.save(name: "My room", kind: .room, parameters: ["decay": 3])
            let reloaded = EffectPresetLibrary(url: url)
            XCTAssertNil(reloaded.loadError)
            XCTAssertEqual(reloaded.presets(for: .echo), [echo])
            XCTAssertEqual(reloaded.presets(for: .room), [room])
            try reloaded.rename(echo.id, to: "Evening repeats")
            XCTAssertEqual(reloaded.presets(for: .echo).first?.parameters, echo.parameters)
            let deleted = try reloaded.delete(echo.id)
            XCTAssertTrue(EffectPresetLibrary(url: url).presets(for: .echo).isEmpty)
            try reloaded.restore(deleted)
            XCTAssertEqual(EffectPresetLibrary(url: url).presets(for: .echo).first?.name, "Evening repeats")
        }
    }
    func testDuplicateAndInvalidNamesDoNotOverwriteSavedSettings() throws {
        try MainActor.assumeIsolated {
            let url = temporaryURL()
            defer { try? FileManager.default.removeItem(at: url.deletingLastPathComponent()) }
            let library = EffectPresetLibrary(url: url)
            let first = try library.save(name: "Café", kind: .echo, parameters: ["time": 300])
            let second = try library.save(name: "Second", kind: .echo, parameters: [:])
            let data = try Data(contentsOf: url)
            for name in [" cafe ", "", "first\nsecond", String(repeating: "x", count: 81)] {
                XCTAssertThrowsError(try library.save(name: name, kind: .echo, parameters: ["time": 900]))
            }
            XCTAssertThrowsError(try library.rename(second.id, to: "CAFÉ"))
            XCTAssertEqual(try Data(contentsOf: url), data)
            XCTAssertEqual(library.presets.first, first)
        }
    }
    func testSavedValuesAreFiniteClampedAndComplete() throws {
        try MainActor.assumeIsolated {
            let url = temporaryURL()
            defer { try? FileManager.default.removeItem(at: url.deletingLastPathComponent()) }
            let library = EffectPresetLibrary(url: url)
            let preset = try library.save(
                name: "Safe", kind: .echo, parameters: ["time": 50_000, "feedback": -1, "mix": .nan, "unknown": 123])
            XCTAssertEqual(preset.parameters["time"], 2000)
            XCTAssertEqual(preset.parameters["feedback"], 0)
            XCTAssertEqual(preset.parameters["mix"], RealtimeEffectKind.echo.defaultParameters["mix"])
            XCTAssertEqual(preset.parameters.count, RealtimeEffectKind.echo.parameters.count)
            XCTAssertNil(preset.parameters["unknown"])
            XCTAssertEqual(EffectPresetLibrary(url: url).presets.first, preset)
        }
    }
    func testCorruptOrNewerLibrariesRemainUntouchedAndCanBeRetried() throws {
        try MainActor.assumeIsolated {
            let url = temporaryURL()
            defer { try? FileManager.default.removeItem(at: url.deletingLastPathComponent()) }
            try FileManager.default.createDirectory(
                at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            for text in ["broken", "{\"version\":2,\"presets\":[]}"] {
                let data = Data(text.utf8)
                try data.write(to: url)
                let library = EffectPresetLibrary(url: url)
                XCTAssertNotNil(library.loadError)
                XCTAssertThrowsError(try library.save(name: "New", kind: .echo, parameters: [:]))
                XCTAssertEqual(try Data(contentsOf: url), data)
                try Data("{\"version\":1,\"presets\":[]}".utf8).write(to: url)
                library.reload()
                XCTAssertNil(library.loadError)
                XCTAssertNoThrow(try library.save(name: "New", kind: .echo, parameters: [:]))
            }
        }
    }
    func testDiskFailureDoesNotPublishUnsavedPresetChanges() throws {
        try MainActor.assumeIsolated {
            let url = temporaryURL()
            defer { try? FileManager.default.removeItem(at: url.deletingLastPathComponent()) }
            let library = EffectPresetLibrary(url: url)
            let original = try library.save(name: "Keep", kind: .echo, parameters: [:])
            try FileManager.default.removeItem(at: url)
            try FileManager.default.createDirectory(at: url, withIntermediateDirectories: false)
            XCTAssertThrowsError(try library.rename(original.id, to: "Changed"))
            XCTAssertThrowsError(try library.delete(original.id))
            XCTAssertThrowsError(try library.save(name: "Unwritten", kind: .echo, parameters: [:]))
            XCTAssertEqual(library.presets, [original])
        }
    }
}
