import SoundroomCore
import XCTest

@testable import Soundroom

final class NativeChainGuardTests: XCTestCase {
    @MainActor func testChainRecallProcessesNewAudioWithoutSourceBakedGuards() {
        let asset = UUID()
        let audio = AudioSamples(channels: [Array(repeating: Float(0.2), count: 128)])
        let original = RealtimeEffect(
            kind: .gain, parameters: ["gain": 6],
            bakedRanges: [.init(startFrame: 0, endFrame: 128)])
        var expected = original
        expected.bakedRanges = []
        let reference = render(expected, asset: asset, audio: audio)
        let bypassed = render(original, asset: asset, audio: audio)
        XCTAssertGreaterThan(zip(reference, bypassed).map { abs($0 - $1) }.max() ?? 0, 0.01)
        for includeAutomation in [false, true] {
            let recalled = EffectChainLibrary.prepared([original], includeAutomation: includeAutomation)[0]
            XCTAssertTrue(recalled.bakedRanges.isEmpty)
            XCTAssertNotEqual(recalled.id, original.id)
            for (a, b) in zip(render(recalled, asset: asset, audio: audio), reference) {
                XCTAssertEqual(a, b, accuracy: 0.000001)
            }
        }
    }

    @MainActor func testSettingsOnlyRecallClearsManualOnlyGateAndCurveState() {
        let original = RealtimeEffect(
            kind: .pitchCorrection,
            parameterAutomation: ["mix": [.init(frame: 0, value: 0.5)]],
            pitchEdits: [.init(startFrame: 0, endFrame: 100, points: [.init(frame: 0, note: 60)])],
            pitchEditsOnly: true, bakedRanges: [.init(startFrame: 0, endFrame: 100)])
        let settings = EffectChainLibrary.prepared([original], includeAutomation: false)[0]
        XCTAssertTrue(settings.pitchEdits.isEmpty)
        XCTAssertFalse(settings.pitchEditsOnly)
        XCTAssertTrue(settings.parameterAutomation.isEmpty)
        XCTAssertTrue(settings.bakedRanges.isEmpty)
        let withCurves = EffectChainLibrary.prepared([original], includeAutomation: true)[0]
        XCTAssertTrue(withCurves.pitchEditsOnly)
        XCTAssertEqual(withCurves.pitchEdits.count, 1)
        XCTAssertNotEqual(withCurves.pitchEdits[0].id, original.pitchEdits[0].id)
        XCTAssertTrue(withCurves.bakedRanges.isEmpty)
        // Importing a version-2 file preserves its data before a new recall.
        let decoded = EffectChainLibrary.prepared([original], includeAutomation: true, regenerateIDs: false)[0]
        XCTAssertEqual(decoded.bakedRanges, original.bakedRanges)
    }

    private func render(_ effect: RealtimeEffect, asset: UUID, audio: AudioSamples) -> [Float] {
        var session = Session()
        session.tracks[0].clips = [Clip(assetID: asset, name: "New audio", start: 0, length: 128, edgeFadeFrames: 0)]
        session.tracks[0].realtimeEffects = [effect]
        let mixer = RealtimeMixer(session: session, assets: [asset: audio], startFrame: 0)
        var left = [Float](repeating: 0, count: 128)
        var right = left
        left.withUnsafeMutableBufferPointer { l in
            right.withUnsafeMutableBufferPointer { r in
                mixer.render(intoLeft: l.baseAddress!, intoRight: r.baseAddress!, frameCount: 128)
            }
        }
        return left + right
    }
}
