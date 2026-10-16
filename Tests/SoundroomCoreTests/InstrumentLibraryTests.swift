import XCTest
@testable import SoundroomCore

final class InstrumentLibraryTests: XCTestCase {
    func testModeledInstrumentsMatchHeldVoiceAndPersistTheirTimbre() throws {
        for kind in MusicalInstrument.allCases where kind != .synthesizer {
            let note = SoundDesignNote(pitch: 60, velocity: 0.7, startBeat: 0, durationBeats: 0.25)
            var p = kind.applying(to: SoundDesignPattern(notes: [note], bpm: 120, beats: 0.25))
            p.attack = 0.01
            let decoded = try JSONDecoder().decode(SoundDesignPattern.self, from: JSONEncoder().encode(p))
            XCTAssertEqual(decoded, p)
            let offline = p.render().channels[0]
            let live = SoundDesignInstrument()
            live.addVoice(.init(pitch: 60, velocity: 0.7, pattern: p))
            func read(_ count: Int) -> [Float] {
                var out = [Float](repeating: 0, count: count)
                out.withUnsafeMutableBufferPointer { live.render(into: $0.baseAddress!, count: count) }
                return out
            }
            let gate = read(6000); live.noteOff(pitch: 60)
            let actual = gate + read(offline.count - 6000)
            XCTAssertLessThan(zip(actual, offline).map { abs($0 - $1) }.max()!, 0.000001, kind.title)
            XCTAssertTrue(actual.allSatisfy { $0.isFinite && abs($0) < 1 })
            XCTAssertGreaterThan(actual.map(abs).max()!, 0.02, kind.title)
            XCTAssertEqual(actual.last!, 0, accuracy: 0.002)
        }
    }
    func testCharacterResonanceAndVelocityAreAudibleNotOnlyLabels() {
        for kind in MusicalInstrument.allCases where kind != .synthesizer {
            var p = kind.applying(to: SoundDesignPattern(notes: [.init(pitch: 57, velocity: 0.8, startBeat: 0, durationBeats: 2)], beats: 2))
            let baseline = p.render().channels[0]
            p.character = 0.05
            let dark = p.render().channels[0]
            p.character = 0.95
            let bright = p.render().channels[0]
            XCTAssertGreaterThan(zip(dark, bright).reduce(0.0) { $0 + abs(Double($1.0 - $1.1)) } / Double(dark.count), 0.001, kind.title)
            p.character = 0.5; p.resonance = 0.05
            let damped = p.render().channels[0]
            p.resonance = 0.95
            let ringing = p.render().channels[0]
            XCTAssertNotEqual(damped, ringing, kind.title)
            p.notes[0].velocity = 0.25
            XCTAssertLessThan(p.render().channels[0].map(abs).max()!, baseline.map(abs).max()!, kind.title)
        }
    }
    func testNewProjectDataRequiresVersionSixAndOldPatternsRemainAnalog() throws {
        let old = Data(#"{"notes":[],"waveform":"triangle"}"#.utf8)
        XCTAssertEqual(try JSONDecoder().decode(SoundDesignPattern.self, from: old).instrument, .synthesizer)
        var session = Session(), clip = Clip(assetID: UUID(), name: "Piano", start: 0, length: 100)
        clip.soundDesign = MusicalInstrument.studioPiano.applying(to: .init())
        session.tracks[0].clips = [clip]
        XCTAssertEqual(session.requiredProjectVersion, 6)
        clip.soundDesign = nil; var drums = DrumPattern(); drums.kit = .jazzMaple; clip.drumPattern = drums
        session.tracks[0].clips = [clip]
        XCTAssertEqual(session.requiredProjectVersion, 6)
    }
    func testAllModelsHandleExtremeKeysAndMutableBadControls() {
        for kind in MusicalInstrument.allCases where kind != .synthesizer {
            var p = kind.applying(to: SoundDesignPattern(notes: []))
            p.character = .nan; p.resonance = .infinity
            for pitch in [0, 127] {
                var voice = SoundDesignVoice(pitch: pitch, velocity: 1, pattern: p)
                for _ in 0..<2000 { XCTAssertTrue(voice.renderNext().isFinite) }
            }
        }
    }
}
