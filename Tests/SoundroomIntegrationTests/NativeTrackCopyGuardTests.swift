import SoundroomCore
import XCTest

@testable import Soundroom

/// Every store is hardware-disabled; reference rendering is worker-only DSP.
final class NativeTrackCopyGuardTests: XCTestCase {
    @MainActor func testPlainAndAutomatedCopiesRelocateBakedGuardsAndPreserveAudibleOutput() {
        for includeAutomation in [false, true] {
            let store = SessionStore(headless: true)
            let asset = UUID()
            let audio = AudioSamples(channels: [Array(repeating: 0.2, count: 512)])
            store.assets[asset] = audio
            var track = Track(name: "Rendered audio", color: 2)
            track.clips = [Clip(assetID: asset, name: "Baked", start: 200, length: 128, edgeFadeFrames: 0)]
            track.mixAutomation["volume"] = [.init(frame: 0, value: 2 / 3)]
            track.realtimeEffects = [
                RealtimeEffect(
                    kind: .gain, parameters: ["gain": 6],
                    parameterAutomation: ["gain": [.init(frame: 0, value: 0.625)]],
                    bakedRanges: [.init(startFrame: 180, endFrame: 350)])
            ]
            store.session.tracks = [track]
            store.selectedTrack = track.id
            store.copyTrack(includeAutomation: includeAutomation)
            store.playhead = 400
            store.pasteTrack()
            let pasted = store.session.tracks[1]
            XCTAssertEqual(pasted.realtimeEffects[0].bakedRanges.map(\.range), [380..<550])
            XCTAssertNotEqual(pasted.realtimeEffects[0].id, track.realtimeEffects[0].id)
            XCTAssertEqual(store.session.tracks[0], track)
            XCTAssertEqual(pasted.mixAutomation.isEmpty, !includeAutomation)
            XCTAssertEqual(pasted.realtimeEffects[0].parameterAutomation.isEmpty, !includeAutomation)
            let expected = render(track, asset: asset, audio: audio, start: 200)
            let actual = render(pasted, asset: asset, audio: audio, start: 400)
            for (a, b) in zip(actual, expected) { XCTAssertEqual(a, b, accuracy: 0.000001) }
            var unprotected = pasted
            unprotected.realtimeEffects[0].bakedRanges = []
            let processedTwice = render(unprotected, asset: asset, audio: audio, start: 400)
            XCTAssertGreaterThan(zip(actual, processedTwice).map { abs($0 - $1) }.max() ?? 0, 0.01)
        }
    }
    @MainActor func testPlainCopyClearsManualOnlyModeWhileRetainingRenderedRegions() {
        let store = SessionStore(headless: true)
        var track = store.session.tracks[0]
        track.realtimeEffects = [
            RealtimeEffect(
                kind: .pitchCorrection,
                pitchEdits: [.init(startFrame: 0, endFrame: 100, points: [.init(frame: 0, note: 60)])],
                pitchEditsOnly: true,
                bakedRanges: [.init(startFrame: 0, endFrame: 100)])
        ]
        store.session.tracks[0] = track
        store.selectedTrack = track.id
        store.copyTrack()
        store.pasteTrack()
        let effect = store.session.tracks[1].realtimeEffects[0]
        XCTAssertTrue(effect.pitchEdits.isEmpty)
        XCTAssertFalse(effect.pitchEditsOnly)
        XCTAssertEqual(effect.bakedRanges, track.realtimeEffects[0].bakedRanges)
    }
    private func render(_ track: Track, asset: UUID, audio: AudioSamples, start: Int) -> [Float] {
        var session = Session()
        session.tracks = [track]
        let mixer = RealtimeMixer(session: session, assets: [asset: audio], startFrame: start)
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
