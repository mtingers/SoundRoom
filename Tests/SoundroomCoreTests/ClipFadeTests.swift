import XCTest

@testable import SoundroomCore

final class ClipFadeTests: XCTestCase {
    private func fixture() -> (Session, UUID, [UUID: AudioSamples]) {
        var session = Session()
        let asset = UUID()
        var clip = Clip(assetID: asset, name: "Faded", start: 10, length: 100, edgeFadeFrames: 0)
        clip.fadeInFrames = 70
        clip.fadeOutFrames = 50
        session.tracks[0].clips = [clip]
        return (session, session.tracks[0].id, [asset: AudioSamples(channels: [Array(repeating: 1, count: 100)])])
    }
    func testSplitAndRepeatedSplitPreserveRenderedFadeEnvelope() throws {
        var (session, track, assets) = fixture()
        let original = Mixer.render(session: session, assets: assets, range: 0..<120)
        session.split(trackID: track, clipID: session.tracks[0].clips[0].id, at: 45)
        session.split(trackID: track, clipID: session.tracks[0].clips[1].id, at: 80)
        XCTAssertEqual(Mixer.render(session: session, assets: assets, range: 0..<120), original)
        let reopened = try JSONDecoder().decode(Session.self, from: JSONEncoder().encode(session))
        XCTAssertEqual(Mixer.render(session: reopened, assets: assets, range: 0..<120), original)
    }
    func testDeletePreservesNeighboringFadeSamples() {
        var (session, track, assets) = fixture()
        var expected = Mixer.render(session: session, assets: assets, range: 0..<120)
        for channel in expected.channels.indices { for frame in 30..<65 { expected.channels[channel][frame] = 0 } }
        session.delete(trackID: track, range: 30..<65)
        XCTAssertEqual(Mixer.render(session: session, assets: assets, range: 0..<120), expected)
    }
    func testCopiedPartialFadeRetainsSoundWhenPastedAndResliced() {
        var (session, track, assets) = fixture()
        let expected = Mixer.render(session: session, assets: assets, range: 25..<95)
        session.tracks[0].clips = session.fragments(trackID: track, range: 25..<95)
        XCTAssertEqual(Mixer.render(session: session, assets: assets, range: 0..<70), expected)
        let resliced = Mixer.render(session: session, assets: assets, range: 10..<55)
        session.tracks[0].clips = session.fragments(trackID: track, range: 10..<55)
        XCTAssertEqual(Mixer.render(session: session, assets: assets, range: 0..<45), resliced)
    }
    func testExplicitFadeEditResetsFragmentEnvelopeLocally() {
        var (session, track, _) = fixture()
        var fragment = session.fragments(trackID: track, range: 25..<95)[0]
        XCTAssertGreaterThan(fragment.fadeGain(at: 0), 0)
        fragment.resetFadeEnvelope()
        fragment.fadeInFrames = 10
        fragment.fadeOutFrames = 10
        XCTAssertEqual(fragment.fadeGain(at: 0), 0)
        XCTAssertEqual(fragment.fadeGain(at: fragment.length - 1), 0)
        XCTAssertEqual(fragment.fadeGain(at: 35), 1)
        session.tracks[0].clips = [fragment]
    }
}
