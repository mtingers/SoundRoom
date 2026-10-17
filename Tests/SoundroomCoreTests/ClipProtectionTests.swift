import XCTest

@testable import SoundroomCore

final class ClipProtectionTests: XCTestCase {
    func testNewClipEdgesAreSilentAndLegacyDecoderIsUnchanged() throws {
        let clip = Clip(assetID: UUID(), name: "DC", start: 0, length: 1000)
        var track = Track(name: "Audio", color: 0)
        track.clips = [clip]
        let envelope = ClipEnvelope.resolve(track: track)[clip.id]!
        XCTAssertEqual(envelope.gain(at: 0), 0)
        XCTAssertEqual(envelope.gain(at: 240), 1)
        XCTAssertEqual(envelope.gain(at: 999), 0)
        let encoded = try JSONEncoder().encode(clip)
        var object = try JSONSerialization.jsonObject(with: encoded) as! [String: Any]
        object.removeValue(forKey: "edgeFadeFrames")
        let legacy = try JSONDecoder().decode(Clip.self, from: JSONSerialization.data(withJSONObject: object))
        track.clips = [legacy]
        XCTAssertEqual(ClipEnvelope.resolve(track: track)[legacy.id]!.gain(at: 0), 1)
    }
    func testSplitRepeatedRejoinIsSampleExactButSeparatedCutIsProtected() {
        var session = Session()
        let asset = UUID()
        let clip = Clip(assetID: asset, name: "Continuous", start: 0, length: 3000)
        session.tracks[0].clips = [clip]
        let audio = AudioSamples(channels: [
            (0..<3000).map { Float(sin(Double($0) * 0.11)) }, (0..<3000).map { Float(cos(Double($0) * 0.07)) },
        ])
        let original = Mixer.render(session: session, assets: [asset: audio], range: 0..<3500)
        session.split(trackID: session.tracks[0].id, clipID: clip.id, at: 100)
        session.split(trackID: session.tracks[0].id, clipID: session.tracks[0].clips[1].id, at: 1800)
        XCTAssertEqual(Mixer.render(session: session, assets: [asset: audio], range: 0..<3500), original)
        session.tracks[0].clips[1].start += 200
        let envelopes = ClipEnvelope.resolve(track: session.tracks[0])
        XCTAssertEqual(envelopes[session.tracks[0].clips[0].id]!.gain(at: 99), 0)
        XCTAssertEqual(envelopes[session.tracks[0].clips[1].id]!.gain(at: 0), 0)
    }
    func testStereoCrossfadeHasConstantSumAndOfflineCallbackAgreement() {
        let first = UUID()
        let second = UUID()
        var session = Session()
        let a = Clip(assetID: first, name: "Left", start: 0, length: 2000)
        let b = Clip(assetID: second, name: "Right", start: 1000, length: 2000)
        session.tracks[0].clips = [a, b]
        let assets = [
            first: AudioSamples(channels: [Array(repeating: 0.5, count: 2000), Array(repeating: -0.25, count: 2000)]),
            second: AudioSamples(channels: [Array(repeating: 0.5, count: 2000), Array(repeating: -0.25, count: 2000)]),
        ]
        let offline = Mixer.render(session: session, assets: assets, range: 0..<3000)
        for frame in 1000..<2000 {
            XCTAssertEqual(offline.channels[0][frame], 0.5, accuracy: 0.000001)
            XCTAssertEqual(offline.channels[1][frame], -0.25, accuracy: 0.000001)
        }
        for size in [1, 17, 512] {
            let mixer = RealtimeMixer(session: session, assets: assets)
            var left = Array(repeating: Float(0), count: 3000)
            var right = left
            left.withUnsafeMutableBufferPointer { l in
                right.withUnsafeMutableBufferPointer { r in
                    var offset = 0
                    while offset < 3000 {
                        let count = min(size, 3000 - offset)
                        mixer.render(
                            intoLeft: l.baseAddress!.advanced(by: offset),
                            intoRight: r.baseAddress!.advanced(by: offset), frameCount: count)
                        offset += count
                    }
                }
            }
            XCTAssertEqual(AudioSamples(channels: [left, right]), offline)
        }
    }
    func testExistingLongFadesBypassGapsAndTinyClips() {
        var track = Track(name: "Audio", color: 0)
        for length in [1, 2, 3, 20, 1000] {
            var clip = Clip(assetID: UUID(), name: "Tiny", start: 50, length: length)
            clip.fadeInFrames = 800
            clip.fadeOutFrames = 800
            track.clips = [clip]
            let envelope = ClipEnvelope.resolve(track: track)[clip.id]!
            for frame in 0..<length {
                XCTAssertTrue(envelope.gain(at: frame).isFinite)
                XCTAssertLessThanOrEqual(envelope.gain(at: frame), clip.fadeGain(at: frame))
            }
            clip.edgeProtectionDisabled = true
            clip.fadeInFrames = nil
            clip.fadeOutFrames = nil
            track.clips = [clip]
            XCTAssertEqual(ClipEnvelope.resolve(track: track)[clip.id]!.gain(at: 0), 1)
        }
    }
    func testAdjacentGeneratedDrumLoopsRemainSeamlessButIsolatedEdgesAreProtected() {
        var pattern = DrumPattern()
        pattern.apply(.backbeat)
        pattern.steps.append(DrumStep(voice: .crash, index: 15, velocity: 0.8))
        let raw = pattern.render()
        XCTAssertGreaterThan(abs(raw.channels[0][0]), 0)
        XCTAssertGreaterThan(abs(raw.channels[0][raw.count - 1]), 0)
        let originalSource = raw
        let asset = UUID()
        let count = raw.count
        var first = Clip(assetID: asset, name: "Drum", start: 0, length: count)
        first.drumPattern = pattern
        var next = Clip(assetID: asset, name: "Drum", start: count, length: count)
        next.drumPattern = pattern
        var session = Session()
        session.tracks[0].clips = [first, next]
        let joined = Mixer.render(session: session, assets: [asset: raw], range: 0..<(count * 2))
        for channel in 0..<2 {
            XCTAssertEqual(joined.channels[channel][count - 1], raw.channels[channel][count - 1])
            XCTAssertEqual(joined.channels[channel][count], raw.channels[channel][0])
            XCTAssertEqual(joined.channels[channel][0], 0)
            XCTAssertEqual(joined.channels[channel][count * 2 - 1], 0)
        }
        XCTAssertEqual(raw, originalSource)
        first.fadeOutFrames = 100
        session.tracks[0].clips = [first, next]
        let manual = ClipEnvelope.resolve(track: session.tracks[0])
        XCTAssertEqual(manual[first.id]!.gain(at: count - 1), 0)
        next.start += 100
        session.tracks[0].clips = [first, next]
        let gap = ClipEnvelope.resolve(track: session.tracks[0])
        XCTAssertEqual(gap[next.id]!.gain(at: 0), 0)
        next.start = count
        next.drumPattern?.bpm += 1
        session.tracks[0].clips = [first, next]
        XCTAssertEqual(ClipEnvelope.resolve(track: session.tracks[0])[next.id]!.gain(at: 0), 0)
    }
    func testDeletedRangeProtectsNewEdgesAndRoundTrips() throws {
        var session = Session()
        let asset = UUID()
        let clip = Clip(assetID: asset, name: "Cut", start: 0, length: 3000)
        session.tracks[0].clips = [clip]
        session.delete(trackID: session.tracks[0].id, range: 1000..<2000)
        let decoded = try JSONDecoder().decode(Session.self, from: JSONEncoder().encode(session))
        XCTAssertEqual(decoded, session)
        let rendered = Mixer.render(
            session: decoded, assets: [asset: AudioSamples(channels: [Array(repeating: 1, count: 3000)])],
            range: 0..<3000)
        XCTAssertEqual(rendered.channels[0][999], 0)
        XCTAssertEqual(rendered.channels[0][2000], 0)
        XCTAssertEqual(rendered.channels[0][1000..<2000].reduce(0, +), 0)
    }
}
