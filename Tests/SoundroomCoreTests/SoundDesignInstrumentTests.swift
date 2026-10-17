import XCTest

@testable import SoundroomCore

final class SoundDesignInstrumentTests: XCTestCase {
    func testShortNoteWithLongAttackMatchesLiveAuditionAndOfflineSound() {
        let gate = 6_000
        let pattern = SoundDesignPattern(
            notes: [.init(pitch: 69, velocity: 0.7, startBeat: 0, durationBeats: 0.25)],
            bpm: 120, beats: 0.25, waveform: .sine, tone: 1, attack: 0.5, release: 0.1,
            decay: 0.2, sustain: 0.4)
        let offline = pattern.render().channels[0]
        let live = SoundDesignInstrument()
        live.addVoice(.init(pitch: 69, velocity: 0.7, pattern: pattern))
        let attack = render(live, count: gate)
        live.noteOff(pitch: 69)
        let release = render(live, count: offline.count - gate)
        let audition = attack + release
        XCTAssertEqual(audition.count, offline.count)
        XCTAssertLessThan(zip(audition, offline).map { abs($0 - $1) }.max()!, 0.000001)
        XCTAssertLessThan(
            offline.map(abs).max()!, 0.07, "A short note does not compress a half-second attack into its gate")
    }
    func testHeldNoteSustainsUntilNoteOffThenReleases() {
        let pattern = SoundDesignPattern(
            notes: [], waveform: .sine, tone: 1, attack: 0.01, release: 0.05, decay: 0.05, sustain: 0.4)
        let instrument = SoundDesignInstrument()
        instrument.addVoice(SoundDesignVoice(pitch: 69, velocity: 0.8, pattern: pattern))
        let held = render(instrument, count: 48_000)
        XCTAssertGreaterThan(held[40_000..<48_000].map(abs).max()!, 0.1)
        XCTAssertEqual(instrument.activeVoiceCount, 1)
        instrument.noteOff(pitch: 69)
        let released = render(instrument, count: 4_800)
        XCTAssertTrue(released[2_401...].allSatisfy { $0 == 0 })
        XCTAssertEqual(instrument.activeVoiceCount, 0)
    }
    func testReleaseDuringAttackStartsAtCurrentLevel() {
        let pattern = SoundDesignPattern(notes: [], waveform: .sine, tone: 1, attack: 1, release: 0.1)
        let instrument = SoundDesignInstrument()
        instrument.addVoice(.init(pitch: 69, velocity: 1, pattern: pattern))
        let attack = render(instrument, count: 2_400)
        instrument.allNotesOff()
        let release = render(instrument, count: 4_800)
        XCTAssertLessThan(release.map(abs).max()!, 0.02)
        XCTAssertLessThan(abs(release.first! - attack.last!), 0.002)
    }
    func testLiveDSPProducesSameSustainForAnyBufferPartition() {
        let pattern = SoundDesignPreset.brightLead.applying(to: SoundDesignPattern(notes: []))
        let whole = SoundDesignInstrument()
        let split = SoundDesignInstrument()
        for pitch in [60, 64, 67] {
            whole.addVoice(.init(pitch: pitch, velocity: 0.6, pattern: pattern))
            split.addVoice(.init(pitch: pitch, velocity: 0.6, pattern: pattern))
        }
        let expected = render(whole, count: 4096)
        let partitioned = render(split, count: 7) + render(split, count: 1024) + render(split, count: 3065)
        XCTAssertEqual(expected, partitioned)
        XCTAssertTrue(expected.allSatisfy { $0.isFinite && abs($0) < 1 })
    }
    func testPolyphonyIsBoundedAndResetSilencesAllVoices() {
        let instrument = SoundDesignInstrument()
        let pattern = SoundDesignPattern(notes: [])
        for pitch in 48..<72 { instrument.addVoice(.init(pitch: pitch, velocity: 1, pattern: pattern)) }
        XCTAssertEqual(instrument.activeVoiceCount, 8)
        XCTAssertTrue(render(instrument, count: 128).allSatisfy { $0.isFinite && abs($0) < 1 })
        instrument.reset()
        XCTAssertTrue(render(instrument, count: 128).allSatisfy { $0 == 0 })
    }
    private func render(_ instrument: SoundDesignInstrument, count: Int) -> [Float] {
        var output = Array(repeating: Float(0), count: count)
        output.withUnsafeMutableBufferPointer { instrument.render(into: $0.baseAddress!, count: count) }
        return output
    }
}
