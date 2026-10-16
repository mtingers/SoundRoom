import XCTest
import SoundroomCore
@testable import Soundroom

final class PortablePresetTests: XCTestCase {
    func testSinglePresetExportImportAndRecoverableReplacement() throws {
        try MainActor.assumeIsolated {
            let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            defer { try? FileManager.default.removeItem(at: directory) }
            let library = EffectPresetLibrary(url: directory.appendingPathComponent("library.json"))
            let saved = try library.save(name: "Dry repeats", kind: .echo, parameters: ["time": 200, "mix": 0.1])
            let file = directory.appendingPathComponent("export.soundpreset")
            try library.exportPreset(saved.id, to: file)
            try library.replace(saved.id, parameters: ["time": 900, "mix": 0.6], legacyAmount: nil)
            XCTAssertEqual(library.presets.first?.parameters["time"], 900)
            try library.undoChange(); XCTAssertEqual(library.presets, [saved])
            XCTAssertEqual(try library.importPresets(from: file), 1)
            XCTAssertEqual(library.presets[1].name, "Dry repeats (2)")
            XCTAssertNotEqual(library.presets[1].id, saved.id)
            XCTAssertEqual(library.presets[1].parameters, saved.parameters)
            try library.undoChange(); XCTAssertEqual(library.presets, [saved])
            XCTAssertEqual(EffectPresetLibrary(url: library.url).presets, [saved])
        }
    }
    func testChainOrderBypassLegacyAndAutomationRoundTrip() throws {
        try MainActor.assumeIsolated {
            let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            defer { try? FileManager.default.removeItem(at: directory) }
            let library = EffectChainLibrary(url: directory.appendingPathComponent("chains.json"))
            var echo = RealtimeEffect(kind: .echo, amount: 0.6, enabled: false)
            echo.automation = [.init(frame: 20, value: 0.4)]
            var gain = RealtimeEffect(kind: .gain, parameters: ["gain": 4])
            gain.parameterAutomation = ["gain": [.init(frame: 10, value: 0.7)]]
            let dry = try library.save(name: "Settings", effects: [echo, gain], includeAutomation: false)
            XCTAssertEqual(dry.effects.map(\.kind), [.echo, .gain])
            XCTAssertFalse(dry.effects[0].enabled); XCTAssertTrue(dry.effects[0].parameters.isEmpty)
            XCTAssertEqual(dry.effects[0].amount, 0.6)
            XCTAssertTrue(dry.effects.allSatisfy { $0.automation.isEmpty && $0.parameterAutomation.isEmpty })
            let wet = try library.save(name: "With curves", effects: [echo, gain], includeAutomation: true)
            XCTAssertEqual(wet.effects[0].automation, echo.automation)
            XCTAssertEqual(wet.effects[1].parameterAutomation, gain.parameterAutomation)
            let reloaded = EffectChainLibrary(url: library.url)
            XCTAssertNil(reloaded.loadError); XCTAssertEqual(reloaded.chains, library.chains)
            let file = directory.appendingPathComponent("export.soundchain")
            try library.exportChain(wet.id, to: file)
            XCTAssertEqual(try library.importChains(from: file), 1)
            XCTAssertEqual(library.chains.last?.name, "With curves (2)")
            XCTAssertNotEqual(library.chains.last?.id, wet.id)
            XCTAssertEqual(library.chains.last?.effects.map(\.kind), [.echo, .gain])
        }
    }
    func testChainReplacementAndDeleteCanBeRecoveredWithoutProjectUndo() throws {
        try MainActor.assumeIsolated {
            let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            defer { try? FileManager.default.removeItem(at: directory) }
            let library = EffectChainLibrary(url: directory.appendingPathComponent("chains.json"))
            let saved = try library.save(name: "Original", effects: [.init(kind: .gain)], includeAutomation: false)
            try library.save(name: "Updated", effects: [.init(kind: .room)], includeAutomation: false, replacing: saved.id)
            XCTAssertEqual(library.chains.first?.effects.first?.kind, .room)
            try library.undoChange(); XCTAssertEqual(library.chains, [saved])
            try library.delete(saved.id); XCTAssertTrue(library.chains.isEmpty)
            try library.undoChange(); XCTAssertEqual(library.chains, [saved])
        }
    }

    func testInvalidChainImportAndDiskFailurePreserveLibrary() throws {
        try MainActor.assumeIsolated {
            let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            defer { try? FileManager.default.removeItem(at: directory) }
            let library = EffectChainLibrary(url: directory.appendingPathComponent("chains.json"))
            let original = try library.save(name: "Keep", effects: [.init(kind: .chorus)], includeAutomation: false)
            let bytes = try Data(contentsOf: library.url)
            let invalid = directory.appendingPathComponent("invalid.soundchain")
            try Data("{\"version\":3,\"chains\":[]}".utf8).write(to: invalid)
            XCTAssertThrowsError(try library.importChains(from: invalid))
            XCTAssertEqual(try Data(contentsOf: library.url), bytes)
            try FileManager.default.removeItem(at: library.url)
            try FileManager.default.createDirectory(at: library.url, withIntermediateDirectories: false)
            XCTAssertThrowsError(try library.delete(original.id))
            XCTAssertEqual(library.chains, [original])
        }
    }
}
