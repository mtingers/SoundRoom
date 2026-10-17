import SoundroomCore
import XCTest

@testable import Soundroom

@MainActor final class SoundPuttyPresetTests: XCTestCase {
    func testSculptedShapesSurvivePersonalPresetReloadAndInvalidateDefaultMatch() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let url = directory.appendingPathComponent("presets.json")
        let library = EffectPresetLibrary(url: url)
        var values = RealtimeEffectKind.soundPutty.defaultParameters
        values["puttyPitch299"] = 0.7
        values["puttyEnergy001"] = -0.4
        values["puttyDamping150"] = -0.8
        values["puttyPitch999"] = 1
        let saved = try library.save(name: "Sculpted metal", kind: .soundPutty, parameters: values)
        XCTAssertNil(saved.parameters["puttyPitch999"])
        let restored = EffectPresetLibrary(url: url).presets(for: .soundPutty)
        XCTAssertEqual(restored.first?.parameters["puttyPitch299"], 0.7)
        XCTAssertEqual(restored.first?.parameters["puttyDamping150"], -0.8)
        XCTAssertFalse(
            EffectControlPresentation.matches(
                kind: .soundPutty, values: values, presetValues: RealtimeEffectKind.soundPutty.defaultParameters))
        XCTAssertTrue(EffectControlPresentation.matches(kind: .soundPutty, values: values, presetValues: values))
    }
}
