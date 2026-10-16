import XCTest
@testable import SoundroomCore

final class MIDIFileTests: XCTestCase {
    func testStandardMIDIInterchangePreservesChordsRestsLengthsAndTempo() throws {
        let original = SoundDesignPattern(notes: [
            .init(pitch: 60, velocity: 0.5, startBeat: 1.25, durationBeats: 0.5),
            .init(pitch: 67, velocity: 0.9, startBeat: 1.25, durationBeats: 1.5),
            .init(pitch: 60, velocity: 0.3, startBeat: 3.0, durationBeats: 0.25)
        ], bpm: 137, beats: 8)
        let data = MIDIFile.encode(original)
        XCTAssertEqual(String(data: data.prefix(4), encoding: .ascii), "MThd")
        let decoded = try MIDIFile.decode(data)
        XCTAssertEqual(decoded.bpm, 137, accuracy: 0.001); XCTAssertEqual(decoded.beats, 8)
        XCTAssertEqual(decoded.notes.count, 3)
        for (a, b) in zip(original.notes, decoded.notes) {
            XCTAssertEqual(a.pitch, b.pitch); XCTAssertEqual(a.startBeat, b.startBeat, accuracy: 1 / 480)
            XCTAssertEqual(a.durationBeats, b.durationBeats, accuracy: 1 / 480)
            XCTAssertEqual(a.velocity, b.velocity, accuracy: 1 / 127)
        }
    }
    func testRunningStatusAndZeroVelocityNoteOff() throws {
        let track: [UInt8] = [0, 0x90, 60, 100, 0, 64, 90, 0x83, 0x60, 60, 0, 0, 64, 0, 0, 0xFF, 0x2F, 0]
        let decoded = try MIDIFile.decode(file(track))
        XCTAssertEqual(decoded.notes.map(\.pitch), [60, 64])
        XCTAssertEqual(decoded.notes.map(\.durationBeats), [1, 1])
    }
    func testTruncatedOrUnterminatedNoteFileIsRejected() {
        let valid = MIDIFile.encode(SoundDesignPattern())
        for length in [0, 4, 14, valid.count - 2] { XCTAssertThrowsError(try MIDIFile.decode(valid.prefix(length))) }
        let unterminated: [UInt8] = [0, 0x90, 60, 100, 0, 0xFF, 0x2F, 0]
        XCTAssertThrowsError(try MIDIFile.decode(file(unterminated)))
    }
    func testVariableTempoIsExplicitlyRejected() {
        let track: [UInt8] = [0, 0xFF, 0x51, 3, 7, 0xA1, 0x20, 0x83, 0x60, 0xFF, 0x51, 3, 6, 0x1A, 0x80, 0, 0xFF, 0x2F, 0]
        XCTAssertThrowsError(try MIDIFile.decode(file(track))) { XCTAssertEqual($0 as? MIDIFileError, .variableTempo) }
    }
    func testMalformedVariableIntegerDoesNotReadPastTrackAndSMPTERejected() {
        XCTAssertThrowsError(try MIDIFile.decode(file([0x80, 0x80, 0x80, 0x80, 0x80])))
        var data = file([0, 0xFF, 0x2F, 0]); data[12] = 0xE8; data[13] = 40
        XCTAssertThrowsError(try MIDIFile.decode(data)) { XCTAssertEqual($0 as? MIDIFileError, .unsupportedTimeDivision) }
    }
    private func file(_ track: [UInt8]) -> Data {
        Data(Array("MThd".utf8) + [0, 0, 0, 6, 0, 0, 0, 1, 1, 0xE0] + Array("MTrk".utf8) +
             [UInt8((track.count >> 24) & 255), UInt8((track.count >> 16) & 255), UInt8((track.count >> 8) & 255), UInt8(track.count & 255)] + track)
    }
}
