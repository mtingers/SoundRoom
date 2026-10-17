import XCTest

@testable import SoundroomCore

final class DesignerExpansionTests: XCTestCase {
    func testCancelledRenderingReturnsNoAudioInsteadOfFinishingLargePattern() async {
        let task = Task {
            while !Task.isCancelled { await Task.yield() }
            return SoundDesignPattern(beats: 64).render()
        }
        task.cancel()
        let audio = await task.value
        XCTAssertEqual(audio.count, 0)
    }

    func testOldPatternUsesSilentAdditionalOscillatorsAndFullSustain() throws {
        let data = Data("{\"notes\":[],\"bpm\":123,\"waveform\":\"sine\",\"beats\":2}".utf8)
        let old = try JSONDecoder().decode(SoundDesignPattern.self, from: data)
        XCTAssertEqual(old.oscillatorMix, 0)
        XCTAssertEqual(old.subLevel, 0)
        XCTAssertEqual(old.noiseLevel, 0)
        XCTAssertEqual(old.sustain, 1)
        XCTAssertEqual(old.drive, 1)
        XCTAssertEqual(try JSONDecoder().decode(SoundDesignPattern.self, from: JSONEncoder().encode(old)), old)
    }
    func testADSRDecayReachesSustainAndReleaseBeginsAtCurrentLevel() {
        func render(sustain: Double) -> [Float] {
            SoundDesignPattern(
                notes: [.init(pitch: 69, velocity: 0.3, startBeat: 0, durationBeats: 1)],
                bpm: 120, beats: 1, waveform: .sine, tone: 1, attack: 0.01, release: 0.1,
                decay: 0.04, sustain: sustain
            ).render().channels[0]
        }
        func rms(_ samples: [Float], _ range: Range<Int>) -> Double {
            sqrt(samples[range].reduce(0) { $0 + Double($1 * $1) } / Double(range.count))
        }
        let loud = render(sustain: 1)
        let quiet = render(sustain: 0.2)
        XCTAssertEqual(rms(quiet, 9_600..<19_200) / rms(loud, 9_600..<19_200), 0.2, accuracy: 0.01)
        XCTAssertEqual(rms(quiet, 24_000..<25_920) / rms(loud, 24_000..<25_920), 0.2, accuracy: 0.01)
        XCTAssertEqual(quiet.count, 28_800)
        XCTAssertLessThan(abs(quiet.last!), 0.001)
    }
    func testAdditionalOscillatorAndNoiseAreDeterministicAndBounded() {
        var pattern = SoundDesignPattern(
            notes: [.init(pitch: 60, startBeat: 0, durationBeats: 0.5)],
            bpm: 120, beats: 0.5, waveform: .pulse, release: 0.01,
            oscillatorMix: 0.7, detune: 18, pulseWidth: 0.15,
            subLevel: 0.6, noiseLevel: 0.4, drive: 3)
        let mixed = pattern.render()
        XCTAssertEqual(mixed, pattern.render())
        XCTAssertTrue(mixed.channels[0].allSatisfy { $0.isFinite && abs($0) < 1 })
        pattern.oscillatorMix = 0
        pattern.subLevel = 0
        pattern.noiseLevel = 0
        XCTAssertNotEqual(mixed, pattern.render())
        let clean = pattern.render()
        pattern.detune = -50
        pattern.secondWaveform = .sine
        XCTAssertEqual(clean, pattern.render(), "Unused oscillator controls do not change oscillator 1")
    }
    func testPresetPreservesCompositionAndHasDifferentEnvelopeAndTone() {
        let original = SoundDesignPattern(bpm: 93, beats: 8)
        let pluck = SoundDesignPreset.glassPluck.applying(to: original)
        let pad = SoundDesignPreset.widePad.applying(to: original)
        XCTAssertEqual(pluck.notes, original.notes)
        XCTAssertEqual(pad.bpm, 93)
        XCTAssertEqual(pad.beats, 8)
        XCTAssertGreaterThan(pad.attack, pluck.attack)
        XCTAssertGreaterThan(pad.release, pluck.release)
        XCTAssertNotEqual(pluck.render(), pad.render())
    }
    func testMoveMultipleNotesClampsAsGroupWithoutChangingIntervals() {
        let a = SoundDesignNote(pitch: 2, startBeat: 0.5, durationBeats: 0.5)
        let b = SoundDesignNote(pitch: 9, startBeat: 2, durationBeats: 1)
        let ids: Set<UUID> = [a.id, b.id]
        let earlier = SoundDesignEditing.moved([a, b], ids: ids, by: -10, semitones: -12, patternBeats: 4, grid: 0.25)
        XCTAssertEqual(earlier[0].startBeat, 0)
        XCTAssertEqual(earlier[1].startBeat, 1.5)
        XCTAssertEqual(earlier[0].pitch, 0)
        XCTAssertEqual(earlier[1].pitch, 7)
        let later = SoundDesignEditing.moved([a, b], ids: ids, by: 10, semitones: 0, patternBeats: 4, grid: 0.25)
        XCTAssertEqual(later[1].startBeat + later[1].durationBeats, 4)
        XCTAssertEqual(later[1].startBeat - later[0].startBeat, b.startBeat - a.startBeat)
    }
    func testResizeAndQuantizeKeepNotesInsidePattern() {
        let note = SoundDesignNote(pitch: 60, startBeat: 3.75, durationBeats: 0.125)
        let resized = SoundDesignEditing.resized(note, to: 20, patternBeats: 4, grid: 0.25)
        XCTAssertEqual(resized.durationBeats, 0.25)
        XCTAssertEqual(resized.id, note.id)
        let quantized = SoundDesignEditing.quantized(
            [.init(pitch: 60, startBeat: 3.99, durationBeats: 0.03)], grid: 0.25, patternBeats: 4)
        XCTAssertEqual(quantized[0].startBeat, 3.75)
        XCTAssertEqual(quantized[0].durationBeats, 0.25)
        let short = SoundDesignEditing.quantized(
            [.init(pitch: 60, startBeat: 0, durationBeats: 0.1)], grid: 1, patternBeats: 0.25)
        XCTAssertEqual(short[0].startBeat, 0)
        XCTAssertEqual(short[0].durationBeats, 0.25)
    }
}
