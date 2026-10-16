import XCTest
import SoundroomCore
@testable import Soundroom

final class DesignerMIDIImportTests: XCTestCase {
    func testNativeImportRetainsDecodedPerformanceAndCurrentInstrument() throws {
        var instrument = SoundDesignPattern(notes: [.init(pitch: 72, startBeat: 0)])
        instrument.tone = 0.31; instrument.detune = 13
        instrument.layers = [SoundDesignLayer()]
        instrument.controllers = [.init(kind: .expression, beat: 0, value: 0.2)]
        var source = SoundDesignPattern(notes: [.init(pitch: 60, startBeat: 0, durationBeats: 1)], bpm: 90, beats: 4)
        source.controllers = [.init(kind: .sustain, beat: 0, value: 1), .init(kind: .modulation, beat: 0.5, value: 0.7), .init(kind: .pitchBend, beat: 1, value: 0.8), .init(kind: .expression, beat: 2, value: 0.4), .init(kind: .sustain, beat: 3, value: 0)]
        let decoded = try MIDIFile.decode(MIDIFile.encode(source))
        let imported = DesignerMIDIImport.replacingSequence(in: instrument, with: decoded)
        XCTAssertEqual(imported.notes, decoded.notes)
        XCTAssertEqual(imported.controllers, decoded.controllers)
        XCTAssertEqual(imported.controllers.count, 5)
        XCTAssertEqual(imported.bpm, decoded.bpm); XCTAssertEqual(imported.beats, decoded.beats)
        XCTAssertEqual(imported.tone, instrument.tone); XCTAssertEqual(imported.detune, instrument.detune)
        XCTAssertEqual(imported.layers, instrument.layers)
    }
    func testControllerlessImportClearsPriorSequenceControllers() throws {
        var instrument = SoundDesignPattern(notes: [.init(pitch: 60, startBeat: 0)])
        instrument.controllers = [.init(kind: .sustain, beat: 0, value: 1)]
        let decoded = try MIDIFile.decode(MIDIFile.encode(SoundDesignPattern(notes: [.init(pitch: 64, startBeat: 0)])))
        XCTAssertTrue(DesignerMIDIImport.replacingSequence(in: instrument, with: decoded).controllers.isEmpty)
    }
}
