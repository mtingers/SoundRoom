import CoreMIDI
import XCTest

@testable import Soundroom

final class DesignerMIDIParserTests: XCTestCase {
    @MainActor
    func testMIDISourceSelectionRequiresAndUsesServerRouting() {
        let input = DesignerMIDIInput()
        XCTAssertTrue(input.sources.isEmpty)
        input.connect(7)
        XCTAssertEqual(input.connected, 0)
        XCTAssertNotNil(input.error)
        var selections: [UInt32] = []
        input.configureRemote(sources: [.init(id: 7, name: "Server keyboard")]) { selections.append($0) }
        input.connect(7)
        XCTAssertEqual(selections, [7])
        XCTAssertEqual(input.connected, 7)
        XCTAssertNil(input.error)
        input.close()
        XCTAssertEqual(selections, [7, 0])
        XCTAssertEqual(input.connected, 0)
    }
    func testRunningStatusSurvivesSplitPacketsAndRealtimeClock() {
        var parser = DesignerMIDIParser()
        XCTAssertTrue(parser.receive([0x90, 60]).isEmpty)
        let first = parser.receive([0xF8, 100, 64, 80])
        XCTAssertEqual(first.count, 2)
        XCTAssertEqual(first[0].pitch, 60)
        XCTAssertEqual(first[0].velocity, 100)
        XCTAssertTrue(first[0].on)
        XCTAssertEqual(first[1].pitch, 64)
        let off = parser.receive([60, 0])
        XCTAssertFalse(off[0].on)
    }
    func testSysExAndOtherMessagesDoNotProduceSpuriousNotes() {
        var parser = DesignerMIDIParser()
        XCTAssertTrue(parser.receive([0xF0, 60, 100, 0xF8, 0xF7, 60, 100, 0xB0, 1, 127, 0xC0, 12]).isEmpty)
        let notes = parser.receive([0x80, 60, 64, 0x90, 64, 90])
        XCTAssertEqual(notes.count, 2)
        XCTAssertFalse(notes[0].on)
        XCTAssertTrue(notes[1].on)
    }
}
