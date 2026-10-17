import XCTest

@testable import SoundroomCore

final class LegacyEffectModeTests: XCTestCase {
    private var source: AudioSamples {
        let left = (0..<14_000).map { Float(sin(2 * Double.pi * 440 * Double($0) / sessionRate)) * 0.3 }
        return AudioSamples(channels: [left, left.map { $0 * 0.5 }])
    }

    func testBareModernInsertsIgnoreAmountAndUsePhysicalDefaultsIncludingSameSeedDither() {
        let audio = source
        for kind in [RealtimeEffectKind.highPass, .room, .compressor, .equalizer, .chorus, .dither, .pitchCorrection] {
            let bare = RealtimeEffect(kind: kind, amount: 0.15)
            XCTAssertFalse(bare.usesLegacyProcessor, kind.name)
            var otherAmount = bare
            otherAmount.amount = 0.85
            var explicit = bare
            explicit.parameters = kind.defaultParameters
            // Copies retain the ID: dither has exactly the same deterministic noise seed.
            let actual = bare.process(audio)
            assertExactAudio(actual, otherAmount.process(audio), "\(kind.name): amount is not an active modern control")
            assertExactAudio(actual, explicit.process(audio), "\(kind.name): empty values mean actual modern defaults")
        }
    }

    func testTrueLegacyInsertsHaveAnAudibleAmountControl() {
        let audio = source
        for kind in [RealtimeEffectKind.gain, .lowPass, .echo, .warmth] {
            let low = RealtimeEffect(kind: kind, amount: 0.15)
            var high = low
            high.amount = 0.85
            XCTAssertTrue(low.usesLegacyProcessor, kind.name)
            let first = low.process(audio)
            let second = high.process(audio)
            var maximumDifference: Float = 0
            for frame in 0..<audio.count {
                maximumDifference = max(
                    maximumDifference,
                    abs(first.sample(channel: 0, frame: frame) - second.sample(channel: 0, frame: frame)))
            }
            XCTAssertGreaterThan(
                maximumDifference, 0.01, "\(kind.name) must not display an inert Legacy Amount control")
        }
    }

    func testActiveParameterAutomationSwitchesLegacySupportedGainToTheModernPath() {
        let audio = source
        var automated = RealtimeEffect(kind: .gain, amount: 0.75)
        automated.parameterAutomation["gain"] = [.init(frame: 0, value: 0.5)]
        XCTAssertFalse(automated.usesLegacyProcessor)
        // The automation selects 0 dB, overriding the old amount's +12 dB.
        assertExactAudio(automated.process(audio), audio, "Physical automation must drive the modern gain stage")
        var explicit = automated
        explicit.parameters = automated.parametersForEditing
        assertExactAudio(
            automated.process(audio), explicit.process(audio), "Rendering and control-mode classification agree")
        automated.parameterAutomation = [:]
        XCTAssertTrue(automated.usesLegacyProcessor)
        let legacy = automated.process(audio)
        XCTAssertEqual(
            legacy.sample(channel: 0, frame: 100), audio.sample(channel: 0, frame: 100) * pow(Float(10), 12 / 20),
            accuracy: 0.000001)
    }

    func testAutomationLaneCountTracksEffectiveCurvesAfterMigrationAndOverrides() {
        var gain = RealtimeEffect(
            kind: .gain, amount: 0.75,
            automation: [.init(frame: 0, value: 0.25), .init(frame: 1000, value: 0.75)])
        XCTAssertEqual(gain.automatedParameterIDs, ["gain"])
        gain.enableParameterControls()
        XCTAssertFalse(gain.automation.isEmpty, "Source legacy curve remains available for compatibility")
        XCTAssertFalse(gain.parameterAutomation["gain"]?.isEmpty ?? true)
        XCTAssertEqual(gain.automatedParameterIDs, ["gain"], "Migrating a curve does not create two audible lanes")
        gain.parameterAutomation["unknown"] = [.init(frame: 0, value: 0.5)]
        XCTAssertEqual(gain.automatedParameterIDs, ["gain"])

        var echo = RealtimeEffect(kind: .echo, automation: [.init(frame: 0, value: 0.9)])
        echo.parameterAutomation["damping"] = [.init(frame: 0, value: 0.5)]
        echo.parameterAutomation["time"] = []
        echo.parameterAutomation["unknown"] = [.init(frame: 0, value: 0.8)]
        XCTAssertEqual(
            echo.automatedParameterIDs, ["mix", "damping"],
            "Modern tone automation still permits the legacy mix fallback")
        echo.parameterAutomation["mix"] = [.init(frame: 0, value: 0.25)]
        XCTAssertEqual(
            echo.automatedParameterIDs, ["mix", "damping"],
            "A physical mix curve replaces rather than doubles its legacy fallback")
        echo.parameters = echo.kind.defaultParameters
        echo.parameterAutomation["mix"] = nil
        XCTAssertEqual(
            echo.automatedParameterIDs, ["damping"], "Explicit modern values leave retained legacy curves inactive")

        var overridden = RealtimeEffect(kind: .gain, amount: 0.75, automation: [.init(frame: 0, value: 1)])
        overridden.parameterAutomation["gain"] = [.init(frame: 0, value: 0.5)]
        XCTAssertEqual(overridden.automatedParameterIDs, ["gain"])
        assertExactAudio(
            overridden.process(source), source,
            "The counted physical gain curve overrides the retained +24 dB legacy curve")
    }

    private func assertExactAudio(
        _ actual: AudioSamples, _ expected: AudioSamples, _ message: String,
        file: StaticString = #filePath, line: UInt = #line
    ) {
        XCTAssertEqual(actual.count, expected.count, message, file: file, line: line)
        XCTAssertEqual(actual.channelCount, expected.channelCount, message, file: file, line: line)
        guard actual.count == expected.count, actual.channelCount == expected.channelCount else { return }
        var firstMismatch: String?
        for channel in 0..<actual.channelCount {
            for frame in 0..<actual.count {
                let a = actual.sample(channel: channel, frame: frame)
                let e = expected.sample(channel: channel, frame: frame)
                if a != e {
                    firstMismatch = "channel \(channel), frame \(frame), actual \(a), expected \(e)"
                    break
                }
            }
            if firstMismatch != nil { break }
        }
        XCTAssertNil(firstMismatch, "\(message); \(firstMismatch ?? "identical")", file: file, line: line)
    }
}
