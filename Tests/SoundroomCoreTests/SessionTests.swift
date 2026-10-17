import XCTest

@testable import SoundroomCore

final class SessionTests: XCTestCase {
    func fixture() -> (Session, UUID, UUID, AudioSamples) {
        var session = Session()
        let asset = UUID()
        let audio = AudioSamples(channels: [[0.1, 0.2, 0.3, 0.4, 0.5, 0.6]])
        session.tracks[0].clips = [Clip(assetID: asset, name: "Take", start: 10, length: 6, edgeFadeFrames: 0)]
        return (session, session.tracks[0].id, asset, audio)
    }
    func testSplitPreservesExactSourceAndTiming() {
        var (session, track, asset, audio) = fixture()
        let before = Mixer.render(session: session, assets: [asset: audio], range: 0..<20)
        session.split(trackID: track, clipID: session.tracks[0].clips[0].id, at: 13)
        XCTAssertEqual(session.tracks[0].clips.map(\.start), [10, 13])
        XCTAssertEqual(session.tracks[0].clips.map(\.offset), [0, 3])
        XCTAssertEqual(session.tracks[0].clips.map(\.length), [3, 3])
        XCTAssertEqual(Mixer.render(session: session, assets: [asset: audio], range: 0..<20), before)
    }
    func testDeleteMiddleLeavesBothSidesAndDoesNotRipple() {
        var (session, track, asset, audio) = fixture()
        session.delete(trackID: track, range: 12..<14)
        XCTAssertEqual(session.tracks[0].clips.map(\.start), [10, 14])
        XCTAssertEqual(session.tracks[0].clips.map(\.offset), [0, 4])
        let out = Mixer.render(session: session, assets: [asset: audio], range: 10..<16)
        XCTAssertEqual(out.channels[0], [0.1, 0.2, 0, 0, 0.5, 0.6])
        XCTAssertEqual(session.end, 16)
    }
    func testCopyFragmentsAndPasteRetainGapsAndOffsets() {
        var (session, track, asset, _) = fixture()
        session.split(trackID: track, clipID: session.tracks[0].clips[0].id, at: 13)
        session.tracks[0].clips[1].start = 15
        let clips = session.fragments(trackID: track, range: 11..<17)
        XCTAssertEqual(clips.map(\.start), [0, 4])
        XCTAssertEqual(clips.map(\.offset), [1, 3])
        XCTAssertEqual(clips.map(\.length), [2, 2])
        XCTAssertTrue(clips.allSatisfy { $0.assetID == asset })
        XCTAssertNotEqual(clips[0].id, session.tracks[0].clips[0].id)
    }
    func testDeleteOutOfBoundsAndSplitEdgesAreNoOps() {
        var (session, track, _, _) = fixture()
        let before = session
        session.split(trackID: track, clipID: session.tracks[0].clips[0].id, at: 10)
        session.split(trackID: track, clipID: session.tracks[0].clips[0].id, at: 16)
        session.delete(trackID: track, range: 20..<30)
        XCTAssertEqual(session, before)
    }
    func testMuteSoloPanAndOverlappingClips() {
        var (session, _, asset, audio) = fixture()
        var other = Track(name: "Other", color: 1)
        other.clips = session.tracks[0].clips
        other.gain = 0.5
        other.pan = 1
        other.solo = true
        session.tracks.append(other)
        let mix = Mixer.render(session: session, assets: [asset: audio], range: 10..<16)
        XCTAssertEqual(mix.channels[0], [0, 0, 0, 0, 0, 0])
        XCTAssertEqual(mix.channels[1][0], 0.05, accuracy: 0.00001)
        session.tracks[1].muted = true
        XCTAssertEqual(
            Mixer.render(session: session, assets: [asset: audio], range: 10..<16).channels[1], [0, 0, 0, 0, 0, 0])
    }
    func testSelectionEffectCanReplaceRangeWithoutChangingNeighbors() {
        var (session, track, asset, audio) = fixture()
        let part = session.fragments(trackID: track, range: 12..<14)[0]
        let processed = BuiltInEffect.reverse.process(
            audio.slice(part.offset..<(part.offset + part.length)), amount: 0.5)
        session.delete(trackID: track, range: 12..<14)
        let newAsset = UUID()
        session.tracks[0].clips.append(
            Clip(assetID: newAsset, name: "Reversed", start: 12, length: processed.count, edgeFadeFrames: 0))
        XCTAssertEqual(
            Mixer.render(session: session, assets: [asset: audio, newAsset: processed], range: 10..<16).channels[0],
            [0.1, 0.2, 0.4, 0.3, 0.5, 0.6])
        XCTAssertEqual(audio.channels[0], [0.1, 0.2, 0.3, 0.4, 0.5, 0.6])
    }
    func testSessionMetadataRoundTrips() throws {
        var (session, _, _, _) = fixture()
        session.tracks[0].armed = true
        session.tracks[0].inputChannel = 2
        session.tracks[0].stereoInput = true
        XCTAssertEqual(try JSONDecoder().decode(Session.self, from: JSONEncoder().encode(session)), session)
    }
    func testResamplingDurationAndChannels() {
        let input = AudioSamples(channels: [Array(repeating: 0.25, count: 441), Array(repeating: -0.5, count: 441)])
        let output = input.resampled(from: 44100)
        XCTAssertEqual(output.count, 480)
        XCTAssertEqual(output.channels[0][300], 0.25)
        XCTAssertEqual(output.channels[1][300], -0.5)
    }
    func testNormalizeSilenceAndFades() {
        let audio = AudioSamples(channels: [[-0.5, 0.25, 0.5]])
        let normalized = BuiltInEffect.normalize.process(audio, amount: 0.5)
        XCTAssertEqual(normalized.channels[0][2], pow(10, -1 / 20), accuracy: 0.0001)
        XCTAssertEqual(BuiltInEffect.fadeIn.process(audio, amount: 0.5).channels[0], [0, 0.125, 0.5])
        XCTAssertEqual(BuiltInEffect.fadeOut.process(audio, amount: 0.5).channels[0], [-0.5, 0.125, 0])
        let silence = AudioSamples(channels: [[0, 0, 0]])
        XCTAssertEqual(BuiltInEffect.normalize.process(silence, amount: 0.5), silence)
    }
    func testAllEffectsStayFiniteAndKeepDuration() {
        let audio = AudioSamples(channels: [(0..<48000).map { Float(sin(Double($0) * 0.03)) * 0.6 }])
        for effect in BuiltInEffect.allCases {
            let processed = effect.process(audio, amount: 0.8)
            XCTAssertEqual(processed.count, audio.count, effect.name)
            XCTAssertTrue(processed.channels.flatMap { $0 }.allSatisfy(\.isFinite), effect.name)
        }
    }
    func testWaveHeaderClampsPCMAndPreservesFloatHeadroom() {
        let audio = AudioSamples(channels: [[-2, 0, 2], [0.5, 0, -0.5]])
        let pcm = WaveFile.encode(audio)
        XCTAssertEqual(String(data: pcm.prefix(4), encoding: .ascii), "RIFF")
        XCTAssertEqual(pcm[20], 1)
        XCTAssertEqual(pcm[22], 2)
        XCTAssertEqual(pcm[34], 24)
        XCTAssertEqual(pcm.count, 44 + 3 * 2 * 3)
        let float = WaveFile.encode(audio, float: true)
        XCTAssertEqual(float[20], 3)
        XCTAssertEqual(float[34], 32)
        let bits = (0..<4).reduce(UInt32(0)) { $0 | UInt32(float[44 + $1]) << ($1 * 8) }
        XCTAssertEqual(Float(bitPattern: bits), -2)
    }
    func testOddLengthWaveHasRIFFPadding() {
        let wave = WaveFile.encode(AudioSamples(channels: [[0]]))
        XCTAssertEqual(wave.count, 48)
        XCTAssertEqual(wave[40], 3)
        XCTAssertEqual(wave[4], 40)
    }
    func testFloatWaveRoundTripKeepsStereoHeadroomAndTiming() throws {
        let audio = AudioSamples(channels: [[1.2, -0.3, 0.7], [-2, 0.8, 0]])
        let decoded = try XCTUnwrap(WaveFile.decodeFloat(WaveFile.encode(audio, float: true)))
        XCTAssertEqual(decoded.samples, audio)
        XCTAssertEqual(decoded.rate, sessionRate)
    }
    func testFloatWaveRejectsTruncatedOrUnsupportedInput() {
        let audio = AudioSamples(channels: [[0.1, -0.2]])
        let wave = WaveFile.encode(audio, float: true)
        for count in [0, 4, 11, 30, wave.count - 1] { XCTAssertNil(WaveFile.decodeFloat(Data(wave.prefix(count)))) }
        XCTAssertNil(WaveFile.decodeFloat(WaveFile.encode(audio)))
    }
}
