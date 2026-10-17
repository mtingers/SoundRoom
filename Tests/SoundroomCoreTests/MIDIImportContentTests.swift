import XCTest

@testable import SoundroomCore

final class MIDIImportContentTests: XCTestCase {
    // Independently authored format-1 SMF: conductor tempo, two named instrument
    // tracks, program changes, per-channel controllers and running-status notes.
    func testFormatOneKeepsNamedPartsAndTheirOwnControllers() throws {
        let conductor: [UInt8] = [0, 0xFF, 0x51, 3, 10, 0x2C, 0x2A, 0x87, 0x40, 0xFF, 0x2F, 0]
        let piano: [UInt8] =
            [0, 0xFF, 3, 5] + Array("Piano".utf8) + [
                0, 0xC0, 0, 0, 0xB0, 11, 64,
                0, 0x90, 60, 100, 0, 64, 90, 0x83, 0x60, 60, 0, 0, 64, 0, 0, 0xFF, 0x2F, 0,
            ]
        let bass: [UInt8] =
            [0, 0xFF, 3, 4] + Array("Bass".utf8) + [
                0, 0xC1, 32, 0, 0xB1, 11, 127,
                0x83, 0x60, 0x91, 36, 80, 0x83, 0x60, 0x81, 36, 0, 0, 0xFF, 0x2F, 0,
            ]
        let content = try MIDIFile.inspect(file([conductor, piano, bass]))
        XCTAssertEqual(content.bpm, 90, accuracy: 0.001)
        XCTAssertEqual(content.beats, 2)
        XCTAssertEqual(content.parts.count, 2)
        XCTAssertEqual(content.parts[0].name, "Piano · Channel 1")
        XCTAssertEqual(content.parts[1].name, "Bass · Channel 2")
        let selected = try content.pattern(partID: content.parts[0].id)
        XCTAssertEqual(selected.notes.map(\.pitch), [60, 64])
        XCTAssertEqual(selected.controllers.count, 1)
        XCTAssertEqual(selected.controllers[0].value, 64.0 / 127)
        XCTAssertEqual(try content.pattern().notes.map(\.pitch), [60, 64, 36])
    }
    func testLongFileCanBeReviewedAndExplicitlyExcerptedWithoutSilentTruncation() throws {
        let track = [UInt8](arrayLiteral: 0, 0x90, 60, 90) + variable(480 * 96) + [0x80, 60, 0, 0, 0xFF, 0x2F, 0]
        let content = try MIDIFile.inspect(file([track], format: 0))
        XCTAssertEqual(content.beats, 96)
        XCTAssertThrowsError(try content.pattern()) { XCTAssertEqual($0 as? MIDIFileError, .tooLarge) }
        let excerpt = try content.pattern(startBeat: 32, length: 16)
        XCTAssertEqual(excerpt.beats, 16)
        XCTAssertEqual(excerpt.notes.count, 1)
        XCTAssertEqual(excerpt.notes[0].startBeat, 0)
        XCTAssertEqual(excerpt.notes[0].durationBeats, 16)
    }
    func testExcerptCarriesControllerStateAndRebasesLaterPoints() throws {
        let track: [UInt8] = [
            0, 0xB0, 11, 64, 0, 0x90, 60, 100,
            0x83, 0x60, 0xB0, 1, 127, 0x83, 0x60, 0xB0, 11, 127,
            0x87, 0x40, 0x80, 60, 0, 0, 0xFF, 0x2F, 0,
        ]
        let excerpt = try MIDIFile.inspect(file([track], format: 0)).pattern(startBeat: 1.5, length: 1)
        XCTAssertEqual(excerpt.notes[0].durationBeats, 1)
        XCTAssertEqual(excerpt.controllers.count, 3)
        XCTAssertEqual(excerpt.controllers.filter { $0.beat == 0 }.count, 2)
        XCTAssertEqual(excerpt.controllers.last?.kind, .expression)
        XCTAssertEqual(excerpt.controllers.last?.beat, 0.5)
    }
    func testEmptyRangeAndControllerOnlyFileReportNoNotes() throws {
        XCTAssertThrowsError(try MIDIFile.inspect(file([[0, 0xB0, 1, 127, 0, 0xFF, 0x2F, 0]], format: 0))) {
            XCTAssertEqual($0 as? MIDIFileError, .noNotes)
        }
        let track: [UInt8] = [0, 0x90, 60, 90, 0x83, 0x60, 0x80, 60, 0, 0x83, 0x60, 0xFF, 0x2F, 0]
        let content = try MIDIFile.inspect(file([track], format: 0))
        XCTAssertThrowsError(try content.pattern(startBeat: 1, length: 1)) {
            XCTAssertEqual($0 as? MIDIFileError, .noNotes)
        }
        XCTAssertThrowsError(try content.pattern(startBeat: .nan, length: 1))
        XCTAssertThrowsError(try content.pattern(startBeat: 0, length: .infinity))
    }
    func testMissingEndOfTrackAndInvalidFormatZeroTrackCountAreRejected() {
        XCTAssertThrowsError(try MIDIFile.inspect(file([[0, 0x90, 60, 90, 0x83, 0x60, 0x80, 60, 0]], format: 0)))
        XCTAssertThrowsError(try MIDIFile.inspect(file([[0, 0xFF, 0x2F, 0], [0, 0xFF, 0x2F, 0]], format: 0)))
    }
    func testDenseFileRequiresShorterExplicitSelection() throws {
        var track: [UInt8] = []
        for _ in 0..<300 { track += [0, 0x90, 60, 90, 96, 0x80, 60, 0] }
        track += [0, 0xFF, 0x2F, 0]
        let content = try MIDIFile.inspect(file([track], format: 0))
        XCTAssertEqual(content.parts[0].notes.count, 300)
        XCTAssertThrowsError(try content.pattern()) { XCTAssertEqual($0 as? MIDIFileError, .tooLarge) }
        XCTAssertEqual(try content.pattern(length: 4).notes.count, 20)
    }
    private func file(_ tracks: [[UInt8]], format: UInt8 = 1) -> Data {
        var bytes =
            Array("MThd".utf8) + [0, 0, 0, 6, 0, format, UInt8(tracks.count >> 8), UInt8(tracks.count & 255), 1, 0xE0]
        for track in tracks {
            bytes +=
                Array("MTrk".utf8) + [
                    UInt8((track.count >> 24) & 255), UInt8((track.count >> 16) & 255), UInt8((track.count >> 8) & 255),
                    UInt8(track.count & 255),
                ] + track
        }
        return Data(bytes)
    }
    private func variable(_ value: Int) -> [UInt8] {
        var value = value
        var bytes = [UInt8(value & 127)]
        value >>= 7
        while value > 0 {
            bytes.insert(UInt8(value & 127) | 128, at: 0)
            value >>= 7
        }
        return bytes
    }
}
