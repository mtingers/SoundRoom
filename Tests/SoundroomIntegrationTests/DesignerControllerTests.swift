import AVFoundation
import SoundroomCore
import XCTest

@testable import Soundroom

final class DesignerControllerTests: XCTestCase {
    func testRunningStatusControllersPitchBendAndPanicDoNotStealNotes() {
        var parser = DesignerMIDIParser()
        XCTAssertEqual(
            parser.receiveEvents([0xB0, 64, 127, 1, 64]),
            [.controller(.sustain, 1), .controller(.modulation, 64.0 / 127)])
        XCTAssertTrue(parser.receiveEvents([0xE0, 0]).isEmpty)
        XCTAssertEqual(
            parser.receiveEvents([0xF8, 64, 0, 0]), [.controller(.pitchBend, 0.5), .controller(.pitchBend, 0)])
        XCTAssertEqual(
            parser.receiveEvents([0xB0, 11, 80, 123, 0, 0x90, 60, 100]),
            [.controller(.expression, 80.0 / 127), .allNotesOff, .note(pitch: 60, velocity: 100, on: true)])
        XCTAssertEqual(parser.receive([0x90, 61, 100])[0].pitch, 61)
    }
}
