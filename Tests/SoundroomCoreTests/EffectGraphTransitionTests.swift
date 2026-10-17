import XCTest

@testable import SoundroomCore

final class EffectGraphTransitionTests: XCTestCase {
    func testLiveRemovalExtendsTransportAndPreservesEchoAfterOldGraphRetirement() {
        var session = Session()
        let id = UUID()
        session.tracks[0].clips = [Clip(assetID: id, name: "Impulse", start: 0, length: 1000, edgeFadeFrames: 0)]
        session.tracks[0].realtimeEffects = [RealtimeEffect(kind: .echo, amount: 1)]
        var input = [Float](repeating: 0, count: 1000)
        input[0] = 0.5
        let assets = [id: AudioSamples(channels: [input])]
        let mixer = RealtimeMixer(session: session, assets: assets)
        _ = render(mixer, count: 500)
        session.tracks[0].realtimeEffects = []
        let prepared = RealtimeMixer.prepareUpdate(session: session, assets: assets)
        XCTAssertTrue(mixer.applyPreparedUpdate(prepared, smoothManualChanges: true))
        prepared.releaseRetiredResources()
        XCTAssertGreaterThan(mixer.end, 12_000)
        let output = render(mixer, count: 11_501)
        XCTAssertEqual(output[11_500], 0.5 * 0.65, accuracy: 0.000001)
    }
    func testLiveInsertionDoesNotJumpConstantSignal() {
        var session = Session()
        let id = UUID()
        session.tracks[0].clips = [Clip(assetID: id, name: "Constant", start: 0, length: 2000, edgeFadeFrames: 0)]
        let assets = [id: AudioSamples(channels: [[Float](repeating: 0.3, count: 2000)])]
        let mixer = RealtimeMixer(session: session, assets: assets)
        _ = render(mixer, count: 500)
        session.tracks[0].realtimeEffects = [RealtimeEffect(kind: .gain, amount: 0)]
        let prepared = RealtimeMixer.prepareUpdate(session: session, assets: assets)
        mixer.applyPreparedUpdate(prepared, smoothManualChanges: true)
        prepared.releaseRetiredResources()
        let output = render(mixer, count: 600)
        XCTAssertEqual(output[0], 0.3, accuracy: 0.001)
        XCTAssertEqual(output[500], 0.3 * pow(10, -24.0 / 20), accuracy: 0.000001)
    }
    private func render(_ mixer: RealtimeMixer, count: Int) -> [Float] {
        var left = [Float](repeating: 0, count: count)
        var right = left
        left.withUnsafeMutableBufferPointer { l in
            right.withUnsafeMutableBufferPointer { r in
                mixer.render(intoLeft: l.baseAddress!, intoRight: r.baseAddress!, frameCount: count)
            }
        }
        return left
    }
}
