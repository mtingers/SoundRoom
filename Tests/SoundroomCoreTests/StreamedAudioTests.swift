import XCTest

@testable import SoundroomCore

final class StreamedAudioTests: XCTestCase {
    private func fixture(_ audio: AudioSamples, body: (URL, AudioSamples) throws -> Void) throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            .appendingPathExtension("wav")
        defer { try? FileManager.default.removeItem(at: url) }
        try WaveFile.encode(audio, float: true).write(to: url)
        try body(url, AudioSamples(mappedWaveFile: url))
    }
    func testMappedFloatMetadataAndChannelViewsPreserveExactSamples() throws {
        let original = AudioSamples(channels: [[0.2, -0.4, 0.7, 0.1], [-0.1, 0.3, -0.8, 0.4]])
        try fixture(original) { _, mapped in
            XCTAssertTrue(mapped.isStreamed)
            XCTAssertEqual(mapped.count, 4)
            XCTAssertEqual(mapped.channelCount, 2)
            XCTAssertEqual(mapped.channels, original.channels)
            XCTAssertEqual(mapped.readFrames(0..<4), original.channels)
            XCTAssertEqual(mapped.readFrames(-1..<6, channels: [1]), [[0, -0.1, 0.3, -0.8, 0.4, 0, 0]])
            let view = try XCTUnwrap(mapped.view(range: 1..<4, channels: [1], padding: 2))
            XCTAssertTrue(view.isStreamed)
            XCTAssertEqual(view.channels, [[0, 0, 0.3, -0.8, 0.4]])
            XCTAssertEqual(view.sample(channel: 0, frame: 3), -0.8)
            XCTAssertEqual(view.readFrames(0..<view.count), view.channels)
            XCTAssertNil(mapped.view(range: 0..<1, channels: [9]))
            var edited = view
            edited.channels[0][2] = 1
            XCTAssertFalse(edited.isStreamed)
            XCTAssertEqual(view.sample(channel: 0, frame: 2), 0.3)
        }
    }
    func testMappedIntegerMediaAndInvalidHeaders() throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            .appendingPathExtension("wav")
        defer { try? FileManager.default.removeItem(at: url) }
        let source = AudioSamples(channels: [[-1, -0.5, 0, 0.5, 1]])
        try WaveFile.encode(source).write(to: url)
        let mapped = try AudioSamples(mappedWaveFile: url)
        for frame in 0..<5 {
            XCTAssertEqual(
                mapped.sample(channel: 0, frame: frame), source.sample(channel: 0, frame: frame), accuracy: 0.000001)
        }
        let bad = url.deletingPathExtension().appendingPathExtension("bad.wav")
        defer { try? FileManager.default.removeItem(at: bad) }
        try Data(repeating: 0, count: 64).write(to: bad)
        XCTAssertThrowsError(try AudioSamples(mappedWaveFile: bad))
    }
    func testReadAheadMatchesOfflineAcrossPartitionsLoopsAndSeek() throws {
        let frames = 32000
        let original = AudioSamples(channels: [
            (0..<frames).map { Float(sin(Double($0) * 0.02) * 0.4) },
            (0..<frames).map { Float(cos(Double($0) * 0.017) * 0.3) },
        ])
        try fixture(original) { _, mapped in
            let id = UUID()
            var session = Session()
            session.tracks[0].clips = [Clip(assetID: id, name: "Mapped", start: 0, length: frames)]
            session.tracks[0].realtimeEffects = [RealtimeEffect(kind: .echo)]
            let assets = [id: mapped]
            let realtime = RealtimeMixer(session: session, assets: assets, startFrame: 16000)
            realtime.prepareRealtimeSources(session: session, assets: assets)
            let offline = RealtimeMixer(session: session, assets: [id: original], startFrame: 16000)
            for size in [17, 1024, 7, 2048, 256] {
                let actual = render(realtime, count: size)
                let expected = render(offline, count: size)
                XCTAssertEqual(actual.0, expected.0)
                XCTAssertEqual(actual.1, expected.1)
            }
            realtime.rewindForLoop(to: 18000)
            offline.rewindForLoop(to: 18000)
            let actual = render(realtime, count: 500)
            let expected = render(offline, count: 500)
            XCTAssertEqual(actual.0, expected.0)
            XCTAssertEqual(actual.1, expected.1)
            XCTAssertEqual(realtime.sourceReadUnderruns, 0)
        }
    }
    func testReadAheadPinsBeginningOfLongLoopWhenPreparedNearItsEnd() throws {
        let frames = 240000
        let original = AudioSamples(channels: [(0..<frames).map { Float($0 % 1000) / 2000 }])
        try fixture(original) { _, mapped in
            let id = UUID()
            var session = Session()
            session.tracks[0].clips = [Clip(assetID: id, name: "Long loop", start: 0, length: frames)]
            let realtime = RealtimeMixer(session: session, assets: [id: mapped], startFrame: frames - 1000)
            realtime.prepareRealtimeSources(session: session, assets: [id: mapped], loopRange: 0..<frames)
            realtime.rewindForLoop(to: 0)
            let offline = RealtimeMixer(session: session, assets: [id: original])
            let actual = render(realtime, count: 2048)
            let expected = render(offline, count: 2048)
            XCTAssertEqual(actual.0, expected.0)
            XCTAssertEqual(actual.1, expected.1)
            XCTAssertEqual(realtime.sourceReadUnderruns, 0)
        }
    }
    private func render(_ mixer: RealtimeMixer, count: Int) -> ([Float], [Float]) {
        var left = Array(repeating: Float(0), count: count)
        var right = left
        left.withUnsafeMutableBufferPointer { l in
            right.withUnsafeMutableBufferPointer { r in
                mixer.render(intoLeft: l.baseAddress!, intoRight: r.baseAddress!, frameCount: count)
            }
        }
        return (left, right)
    }
}
