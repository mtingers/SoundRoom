import XCTest

@testable import SoundroomCore

final class SoundDesignerTests: XCTestCase {
    func testTempoTimingRestsAndReleaseTail() {
        let pattern = SoundDesignPattern(
            notes: [.init(pitch: 69, startBeat: 1, durationBeats: 0.5)],
            bpm: 120, beats: 2, waveform: .sine, attack: 0.01, release: 0.1)
        let audio = pattern.render()
        XCTAssertEqual(audio.channels.count, 1)
        XCTAssertEqual(audio.count, 52_800)
        XCTAssertTrue(audio.channels[0][0..<24_000].allSatisfy { $0 == 0 })
        XCTAssertGreaterThan(audio.channels[0][25_000..<36_000].map(abs).max() ?? 0, 0.1)
        XCTAssertTrue(audio.channels[0][40_800..<audio.count].allSatisfy { $0 == 0 })
    }

    func testSinePitchMatchesNoteAndOctave() {
        for (pitch, expected) in [(69, 440.0), (81, 880.0)] {
            let pattern = SoundDesignPattern(
                notes: [.init(pitch: pitch, startBeat: 0, durationBeats: 2)],
                bpm: 120, beats: 2, waveform: .sine, tone: 1, attack: 0, release: 0)
            let samples = pattern.render().channels[0]
            let crossings = (4_801..<43_200).filter { samples[$0 - 1] <= 0 && samples[$0] > 0 }
            let measured = Double(crossings.count) / 0.8
            XCTAssertEqual(measured, expected, accuracy: 2)
        }
    }

    func testVelocityAndEnvelopeRemainAudibleWithoutNormalization() {
        func rms(velocity: Double) -> Double {
            let pattern = SoundDesignPattern(
                notes: [.init(pitch: 60, velocity: velocity, startBeat: 0, durationBeats: 1)],
                bpm: 120, beats: 1, waveform: .sine, attack: 0.01, release: 0.1)
            let samples = pattern.render().channels[0]
            XCTAssertEqual(samples.first, 0)
            XCTAssertLessThan(abs(samples[samples.count - 1]), 0.001)
            return sqrt(samples[2_000..<20_000].reduce(0) { $0 + Double($1 * $1) } / 18_000)
        }
        XCTAssertEqual(rms(velocity: 0.2) / rms(velocity: 0.1), 2, accuracy: 0.02)
    }

    func testWaveformsPolyphonyAndMalformedNotesStayFinite() {
        for wave in SoundDesignWaveform.allCases {
            var notes = (0..<20).map { SoundDesignNote(pitch: 60 + $0, velocity: 1, startBeat: 0, durationBeats: 0.25) }
            notes.append(SoundDesignNote(pitch: 69, velocity: .nan, startBeat: .infinity))
            notes.append(SoundDesignNote(pitch: 69, startBeat: 0, durationBeats: .nan))
            let pattern = SoundDesignPattern(
                notes: notes, bpm: 300, beats: 0.25, waveform: wave, tone: .nan, attack: .nan, release: 0)
            let samples = pattern.render().channels[0]
            XCTAssertTrue(samples.allSatisfy { $0.isFinite && abs($0) < 1 })
            XCTAssertGreaterThan(samples.map(abs).max() ?? 0, 0.1)
            let firstEight = SoundDesignPattern(
                notes: Array(notes.prefix(8)), bpm: 300, beats: 0.25,
                waveform: wave, tone: .nan, attack: .nan, release: 0
            ).render()
            XCTAssertEqual(samples, firstEight.channels[0])
        }
    }

    func testPatternRoundTripAndDeterministicRendering() throws {
        let pattern = SoundDesignPattern()
        let decoded = try JSONDecoder().decode(SoundDesignPattern.self, from: JSONEncoder().encode(pattern))
        XCTAssertEqual(decoded, pattern)
        XCTAssertEqual(decoded.render(), pattern.render())
        let defaults = try JSONDecoder().decode(SoundDesignPattern.self, from: Data("{}".utf8))
        XCTAssertEqual(defaults.bpm, 120)
        XCTAssertEqual(defaults.notes.count, 4)
    }

    func testEmptyPatternProducesSilence() {
        let pattern = SoundDesignPattern(notes: [], bpm: 120, beats: 1, release: 0)
        let rendered = pattern.render()
        XCTAssertEqual(rendered.count, 24_000)
        XCTAssertTrue(rendered.channels[0].allSatisfy { $0 == 0 })
    }
}
