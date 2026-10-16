import XCTest
@testable import Soundroom

final class DesignerVirtualKeyboardTests: XCTestCase {
    func testDisplayedKeysMatchEveryChromaticNote() {
        for (offset, key) in DesignerTypingKeyboard.keys.enumerated() {
            XCTAssertEqual(DesignerTypingKeyboard.pitch(for: key.uppercased(), octave: 5), 60 + offset)
        }
        XCTAssertEqual(DesignerTypingKeyboard.name(60), "C4")
        XCTAssertEqual(DesignerTypingKeyboard.name(61), "C♯4")
        XCTAssertEqual(DesignerTypingKeyboard.name(72), "C5")
    }
    func testExtremeOctavesCannotPlayInvalidMIDINotes() {
        XCTAssertEqual(DesignerTypingKeyboard.pitch(for: "a", octave: 0), 0)
        XCTAssertEqual(DesignerTypingKeyboard.pitch(for: "g", octave: 10), 127)
        XCTAssertNil(DesignerTypingKeyboard.pitch(for: "y", octave: 10))
        XCTAssertNil(DesignerTypingKeyboard.pitch(for: "k", octave: 10))
        XCTAssertNil(DesignerTypingKeyboard.pitch(for: "a", octave: -1))
        XCTAssertNil(DesignerTypingKeyboard.pitch(for: "a", octave: 11))
        XCTAssertNil(DesignerTypingKeyboard.pitch(for: "r", octave: 5))
    }
    func testComputerAndPointerUnisonReleasesOnlyAfterBothInputs() {
        var keys = DesignerHeldKeys()
        XCTAssertTrue(keys.press(.computer(0), pitch: 60))
        XCTAssertFalse(keys.press(.pointer(60), pitch: 60))
        XCTAssertNil(keys.release(.computer(0)))
        XCTAssertEqual(keys.pitches, [60])
        XCTAssertEqual(keys.release(.pointer(60)), 60)
        XCTAssertTrue(keys.pitches.isEmpty)
    }
    func testKeyRepeatAndOctaveChangeKeepOriginalNoteUntilRelease() {
        var keys = DesignerHeldKeys()
        XCTAssertTrue(keys.press(.computer(0), pitch: 60))
        XCTAssertFalse(keys.press(.computer(0), pitch: 72))
        XCTAssertEqual(keys.pitch(for: .computer(0)), 60)
        XCTAssertEqual(keys.release(.computer(0)), 60)
        XCTAssertNil(keys.release(.computer(0)))
    }
    func testClosingKeyboardReleasesLocalChordButKeepsMIDIHeld() {
        var keys = DesignerHeldKeys()
        XCTAssertTrue(keys.press(.computer(0), pitch: 60))
        XCTAssertTrue(keys.press(.pointer(64), pitch: 64))
        XCTAssertFalse(keys.press(.midi(60), pitch: 60))
        XCTAssertTrue(keys.press(.midi(67), pitch: 67))
        XCTAssertEqual(keys.releaseLocal(), [64])
        XCTAssertEqual(keys.pitches, [60, 67])
        XCTAssertEqual(keys.release(.midi(60)), 60)
        XCTAssertEqual(keys.release(.midi(67)), 67)
    }
    func testStopClearsAllHeldIndicatorsAndRejectsInvalidNotes() {
        var keys = DesignerHeldKeys()
        XCTAssertFalse(keys.press(.pointer(128), pitch: 128))
        XCTAssertTrue(keys.press(.midi(0), pitch: 0))
        keys.clear()
        XCTAssertTrue(keys.pitches.isEmpty)
        XCTAssertNil(keys.release(.midi(0)))
    }
}
