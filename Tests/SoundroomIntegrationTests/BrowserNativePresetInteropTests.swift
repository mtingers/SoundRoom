import Foundation
import SoundroomCore
import XCTest

@testable import Soundroom

/// The browser's interchange fixtures pass through the actual Mac file codecs,
/// rather than a second handwritten decoder with the same assumptions.
final class BrowserNativePresetInteropTests: XCTestCase {
    private var fixtures: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().appendingPathComponent("web/tests/fixtures")
    }

    @MainActor func testBrowserPresetFixturesRetainLegacyAndPhysicalSignalPathsInNativeLibrary() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let library = EffectPresetLibrary(url: root.appendingPathComponent("library.json"))
        XCTAssertEqual(
            try library.importPresets(from: fixtures.appendingPathComponent("native-effect-presets.json")), 2)
        let legacy = try XCTUnwrap(library.presets.first { $0.legacyAmount != nil })
        XCTAssertEqual(legacy.legacyAmount, 0.75)
        let physical = try XCTUnwrap(library.presets.first { $0.legacyAmount == nil })
        XCTAssertEqual(physical.parameters["gain"], 6)
        let store = SessionStore(headless: true)
        let target = RealtimeEffect(kind: .gain, parameters: ["gain": -6])
        store.session.tracks[0].realtimeEffects = [target]
        store.applyRealtimePreset(legacy, to: target.id)
        XCTAssertEqual(store.session.tracks[0].realtimeEffects[0].amount, 0.75)
        XCTAssertTrue(store.session.tracks[0].realtimeEffects[0].parameters.isEmpty)
        store.applyRealtimePreset(physical, to: target.id)
        XCTAssertEqual(store.session.tracks[0].realtimeEffects[0].parameters["gain"], 6)
        for preset in library.presets {
            let export = root.appendingPathComponent("\(preset.id).soundpreset")
            try library.exportPreset(preset.id, to: export)
            let reopened = EffectPresetLibrary(url: export)
            XCTAssertNil(reopened.loadError)
            XCTAssertEqual(reopened.presets, [preset])
        }
    }

    @MainActor func testBrowserAdvancedChainFixtureRetainsCurveDomainsAndNativeRecallSemantics() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let library = EffectChainLibrary(url: root.appendingPathComponent("library.json"))
        XCTAssertEqual(try library.importChains(from: fixtures.appendingPathComponent("native-effect-chains.json")), 1)
        let chain = try XCTUnwrap(library.chains.first)
        XCTAssertTrue(chain.includesAutomation)
        let effect = try XCTUnwrap(chain.effects.first)
        XCTAssertEqual(effect.amount, 0.75)
        XCTAssertTrue(effect.parameters.isEmpty)
        XCTAssertEqual(effect.automation[0].curve, .smooth)
        XCTAssertEqual(effect.automation[0].curveStart, 0.2)
        XCTAssertEqual(effect.automation[0].curveEnd, 0.8)
        XCTAssertEqual(effect.bakedRanges.map(\.range), [0..<48000])
        let recalled = EffectChainLibrary.prepared(chain.effects, includeAutomation: true)[0]
        XCTAssertEqual(recalled.automation, effect.automation)
        XCTAssertTrue(recalled.bakedRanges.isEmpty)
        XCTAssertNotEqual(recalled.id, effect.id)
        let export = root.appendingPathComponent("chain.soundchain")
        try library.exportChain(chain.id, to: export)
        let reopened = EffectChainLibrary(url: export)
        XCTAssertNil(reopened.loadError)
        XCTAssertEqual(reopened.chains, [chain])
    }
}
