import XCTest
import SoundroomCore
@testable import Soundroom

final class EffectControlPresentationTests: XCTestCase {
    func testSemanticGroupsCoverEachDescriptorOnceAndOnlyHideManualPitchTarget() {
        for kind in RealtimeEffectKind.allCases {
            let groups = EffectControlPresentation.groups(for: kind)
            let ids = groups.flatMap(\.parameterIDs)
            XCTAssertEqual(Set(ids), Set(kind.parameters.map(\.id)), kind.name)
            XCTAssertEqual(ids.count, Set(ids).count, kind.name)
            XCTAssertEqual(groups.count, Set(groups.map(\.id)).count)
            let visible = EffectControlPresentation.visibleParameterIDs(for: kind)
            XCTAssertEqual(Set(visible), Set(ids.filter { kind != .pitchCorrection || $0 != "manualTarget" }))
        }
    }
    func testPresetComparisonPreservesSmallFloatRoundTripButDetectsActualChanges() {
        let base = RealtimeEffectKind.echo.defaultParameters
        var roundTrip = base
        roundTrip["time"] = base["time"]!.nextUp
        roundTrip["mix"] = base["mix"]!.nextDown
        XCTAssertTrue(EffectControlPresentation.matches(kind: .echo, values: roundTrip, presetValues: base))
        XCTAssertTrue(EffectControlPresentation.matches(kind: .echo, values: [:], presetValues: base))
        roundTrip["mix"] = base["mix"]! + 0.001
        XCTAssertFalse(EffectControlPresentation.matches(kind: .echo, values: roundTrip, presetValues: base))
        XCTAssertFalse(EffectControlPresentation.matches(kind: .dither, values: ["bits": 16], presetValues: ["bits": 24]))
        XCTAssertFalse(EffectControlPresentation.matches(kind: .pitchCorrection, values: ["manualTarget": 60.25], presetValues: ["manualTarget": 60]))
    }
    func testPhysicalSummaryAndPrecisionDoNotLoseSmallOrSignedValues() {
        let summary = EffectControlPresentation.summary(kind: .echo, values: ["mix": 0.95, "time": 1400])
        XCTAssertTrue(summary.contains("95%")); XCTAssertTrue(summary.contains("1400"))
        let frequency = RealtimeEffectKind.lowPass.parameters.first { $0.id == "frequency" }!
        XCTAssertEqual(EffectControlPresentation.fractionDigits(for: frequency), 1)
        let attack = RealtimeEffectKind.compressor.parameters.first { $0.id == "attack" }!
        XCTAssertEqual(EffectControlPresentation.fractionDigits(for: attack), 2)
        XCTAssertEqual(attack.clamped(0.25), 0.25)
        XCTAssertTrue(EffectControlPresentation.summary(kind: .compressor, values: ["threshold": -12.5]).contains("-12.5"))
        XCTAssertEqual(RealtimeEffectKind.dither.parameters.first { $0.id == "bits" }!.minimum, 8)
    }
    func testCompressorStaticTransferIncludesKneeMakeupAndParallelMix() {
        let hard = EffectSettingMath.Compressor(values: ["threshold": -20, "ratio": 4, "knee": 0, "mix": 1])
        XCTAssertEqual(hard.outputDB(inputDB: 0), -15, accuracy: 1e-10)
        XCTAssertEqual(hard.outputDB(inputDB: -30), -30, accuracy: 1e-10)
        let soft = EffectSettingMath.Compressor(values: ["threshold": -20, "ratio": 4, "knee": 8, "mix": 1])
        XCTAssertEqual(soft.outputDB(inputDB: -24), -24, accuracy: 1e-10)
        XCTAssertEqual(soft.outputDB(inputDB: -16), -19, accuracy: 1e-10)
        XCTAssertEqual(soft.outputDB(inputDB: -20), -20.75, accuracy: 1e-10)
        let parallel = EffectSettingMath.Compressor(values: ["threshold": -20, "ratio": 4, "knee": 0, "makeup": 6, "mix": 0.5])
        XCTAssertEqual(parallel.outputDB(inputDB: 0), 20 * log10(0.5 + 0.5 * pow(10, -9.0 / 20)), accuracy: 1e-10)
        XCTAssertEqual(EffectSettingMath.Compressor(values: ["mix": 0, "makeup": 24]).outputDB(inputDB: -7), -7, accuracy: 1e-10)
    }
    func testStaticCompressorModelMatchesIndependentSteadyDSP() {
        // Constant input lets the actual detector settle; the UI model never models transients.
        let settings: [[String: Float]] = [
            ["threshold": -20, "ratio": 4, "knee": 0, "attack": 0.1, "release": 10, "mix": 1],
            ["threshold": -20, "ratio": 3, "knee": 8, "attack": 0.1, "release": 10, "makeup": 3, "mix": 0.4]
        ]
        for setting in settings {
            for inputDB in [-24.0, -20, -16, -6] {
                let amplitude = Float(pow(10, inputDB / 20))
                let input = AudioSamples(channels: [Array(repeating: amplitude, count: 12_000)])
                let output = RealtimeEffect(kind: .compressor, parameters: setting).process(input).channels[0]
                let measured = 20 * log10(Double(output.last!))
                XCTAssertEqual(measured, EffectSettingMath.Compressor(values: setting).outputDB(inputDB: inputDB), accuracy: 0.004)
            }
        }
    }
    func testEchoModelExplicitlyDescribesRepeatsBeforeFeedbackTone() {
        let repeats = EffectSettingMath.echoRepeats(values: ["time": 200, "feedback": 0.5, "mix": 0.4])
        XCTAssertEqual(repeats[0].milliseconds, 200); XCTAssertEqual(repeats[6].milliseconds, 1400)
        XCTAssertEqual(repeats[0].amplitude, 0.4, accuracy: 1e-7)
        XCTAssertEqual(repeats[3].amplitude, 0.05, accuracy: 1e-7)
        let single = EffectSettingMath.echoRepeats(values: ["feedback": 0, "mix": 1])
        XCTAssertEqual(single[0].amplitude, 1); XCTAssertEqual(single[1].amplitude, 0)
        XCTAssertTrue(EffectSettingMath.echoRepeats(values: ["mix": 0]).allSatisfy { $0.amplitude == 0 })
    }
    func testChorusSweepUsesActualBasePlusDepthAndStereoPhase() {
        let setting: [String: Float] = ["delay": 12, "depth": 4, "width": 1]
        XCTAssertEqual(EffectSettingMath.chorusDelay(phase: -.pi / 2, right: false, values: setting), 12, accuracy: 1e-10)
        XCTAssertEqual(EffectSettingMath.chorusDelay(phase: .pi / 2, right: false, values: setting), 16, accuracy: 1e-10)
        XCTAssertEqual(EffectSettingMath.chorusDelay(phase: .pi / 2, right: true, values: setting), 12, accuracy: 1e-10)
        let mono: [String: Float] = ["width": 0]
        XCTAssertEqual(EffectSettingMath.chorusDelay(phase: 0.7, right: true, values: mono), EffectSettingMath.chorusDelay(phase: 0.7, right: false, values: mono))
    }
    func testDitherQuantizationStepUsesSignedFullScaleResolution() {
        XCTAssertEqual(EffectSettingMath.quantizationStep(bits: 16), 1 / 32768.0)
        XCTAssertEqual(EffectSettingMath.quantizationStep(bits: 24), 1 / 8388608.0)
        XCTAssertEqual(EffectSettingMath.quantizationStep(bits: 8), 1 / 128.0)
        XCTAssertEqual(EffectSettingMath.quantizationStep(bits: 3), 1 / 128.0)
    }
}
