import XCTest

@testable import SoundroomCore

final class EditableAudioLoopTests: XCTestCase {
    private func read(_ loop: EditableAudioLoop, _ count: Int) -> [Float] {
        var l = [Float](repeating: 0, count: count)
        var r = l
        l.withUnsafeMutableBufferPointer { left in
            r.withUnsafeMutableBufferPointer { right in
                loop.render(left: left.baseAddress!, right: right.baseAddress!, count: count)
            }
        }
        return l
    }
    func testLiveReplacementKeepsBeatAndBlendsWithoutAJump() {
        let loop = EditableAudioLoop()
        loop.replace(.init(audio: AudioSamples(channels: [[Float](repeating: 0.3, count: 4800)]), beats: 4))
        let before = read(loop, 1800)
        XCTAssertEqual(loop.beat, 1.5, accuracy: 0.000001)
        loop.replace(.init(audio: AudioSamples(channels: [[Float](repeating: -0.3, count: 9600)]), beats: 4))
        XCTAssertEqual(loop.beat, 1.5, accuracy: 0.000001, "Tempo edits keep the musical cursor")
        let after = read(loop, 700)
        XCTAssertEqual(after.first!, before.last!, accuracy: 0.000001)
        XCTAssertLessThan(zip(after.dropFirst(), after).map { abs($0 - $1) }.max()!, 0.002)
        XCTAssertEqual(after.last!, -0.3, accuracy: 0.000001)
    }
    func testStopRampsToSilenceAndPartitioningDoesNotChangeAudio() {
        let whole = EditableAudioLoop()
        let split = EditableAudioLoop()
        let audio = AudioSamples(channels: [(0..<8000).map { Float(sin(Double($0) * 0.02) * 0.2) }])
        for loop in [whole, split] { loop.replace(.init(audio: audio, beats: 4)) }
        XCTAssertEqual(read(whole, 3000), read(split, 7) + read(split, 128) + read(split, 2865))
        whole.stop()
        let end = read(whole, 1000)
        XCTAssertTrue(end.suffix(760).allSatisfy { $0 == 0 })
        let stopped = whole.beat
        XCTAssertTrue(read(whole, 512).allSatisfy { $0 == 0 })
        XCTAssertEqual(whole.beat, stopped)
    }
    func testSilentMalformedSourceAndLengthChangeRemainSafe() {
        let loop = EditableAudioLoop()
        loop.replace(.init(audio: AudioSamples(channels: [[], []]), beats: .nan))
        XCTAssertTrue(read(loop, 1000).allSatisfy { $0 == 0 })
        loop.replace(.init(audio: AudioSamples(channels: [[.nan, .infinity, 0.2]]), beats: 0.25))
        XCTAssertTrue(read(loop, 1000).allSatisfy(\.isFinite))
    }
    func testFoldedTailHeadroomPreservesDynamicsAndStereoBalance() {
        let prepared = EditableAudioLoop.Prepared(
            audio: AudioSamples(channels: [[2, 1, -2], [1, 0.5, -1]]), beats: 4)
        XCTAssertEqual(prepared.channels[0][0], 0.98, accuracy: 0.000001)
        XCTAssertEqual(prepared.channels[0][1], 0.49, accuracy: 0.000001)
        XCTAssertEqual(prepared.channels[1][0], 0.49, accuracy: 0.000001)
    }
    func testDrumVariationLoopBeyond64BeatsKeepsItsMusicalPhaseOnTempoEdit() {
        let loop = EditableAudioLoop()
        let prepared = EditableAudioLoop.Prepared(
            audio: AudioSamples(channels: [[Float](repeating: 0.2, count: 25600)]), beats: 256)
        loop.replace(prepared)
        _ = read(loop, 9000)
        XCTAssertEqual(loop.beat, 90, accuracy: 0.000001)
        loop.replace(.init(audio: AudioSamples(channels: [[Float](repeating: 0.2, count: 51200)]), beats: 256))
        XCTAssertEqual(loop.beat, 90, accuracy: 0.000001)
        _ = read(loop, 200)
        XCTAssertEqual(loop.beat, 91, accuracy: 0.000001)
    }
}
