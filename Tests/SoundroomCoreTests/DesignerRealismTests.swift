import XCTest
@testable import SoundroomCore

final class DesignerRealismTests: XCTestCase {
    func testSixLayersPersistTuningZonesAndEnvelopesAndRetainDelayTail() throws {
        var pattern = SoundDesignPattern(notes: [.init(pitch: 60, startBeat: 0, durationBeats: 0.25)], beats: 0.25)
        pattern.baseGain = 0; pattern.fineTune = 7
        for i in 0..<6 {
            var layer = SoundDesignLayer(preset: .kalimba)
            layer.gain = 0.1; layer.delay = Double(i) * 0.08; layer.fineTune = Double(i) - 3
            layer.lowestPitch = 60; layer.highestPitch = 72; layer.decayOverride = 0.17; layer.sustainOverride = 0.7
            pattern.layers.append(layer)
        }
        let restored = try JSONDecoder().decode(SoundDesignPattern.self, from: JSONEncoder().encode(pattern))
        XCTAssertEqual(restored, pattern)
        XCTAssertEqual(restored.layers.count, 6)
        let sixth = restored.layers[5].instrument(for: restored)
        XCTAssertEqual(sixth.fineTune, 9); XCTAssertEqual(sixth.decay, 0.17); XCTAssertEqual(sixth.sustain, 0.7)
        XCTAssertEqual(pattern.render().count, Int(((0.125 + 0.55 + 0.4) * sessionRate).rounded()))
        pattern.notes[0].pitch = 59
        XCTAssertTrue(pattern.render().channels.flatMap { $0 }.allSatisfy { $0 == 0 })
    }
    func testLiveTapReleasedBeforeDelayedOnsetStillSoundsAndFinishes() {
        let p = SoundDesignPreset.acousticGuitar.applying(to: .init())
        var voice = SoundDesignVoice(pitch: 60, velocity: 0.8, pattern: p, sampleRate: 8000, delay: 0.05)
        voice.noteOff()
        XCTAssertFalse(voice.isFinished)
        for _ in 0..<400 { XCTAssertEqual(voice.renderNext(), 0) }
        var samples: [Float] = []
        for _ in 0..<4000 { samples.append(voice.renderNext()) }
        XCTAssertGreaterThan(samples.map(abs).max()!, 0.02)
        XCTAssertTrue(voice.isFinished)
    }
    func testGroovesHaveValidStepsAndTempo() {
        for preset in DrumPatternPreset.allCases where preset.category != .basics {
            var p = DrumPattern(); p.apply(preset)
            XCTAssertFalse(p.steps.isEmpty, preset.rawValue)
            XCTAssertTrue(p.bpm >= 30 && p.bpm <= 300, preset.rawValue)
        }
    }
}
