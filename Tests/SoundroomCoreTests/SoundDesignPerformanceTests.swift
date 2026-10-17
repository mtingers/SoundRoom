import XCTest

@testable import SoundroomCore

final class SoundDesignPerformanceTests: XCTestCase {
    private func heldPattern() -> SoundDesignPattern {
        SoundDesignPattern(
            notes: [.init(pitch: 69, startBeat: 0, durationBeats: 4)], bpm: 120, beats: 4, waveform: .sine, tone: 1,
            attack: 0.005, release: 0.05)
    }
    func testRecordedSustainHoldsUntilPedalReleaseInRenderedAudio() {
        var pattern = heldPattern()
        pattern.notes[0].durationBeats = 0.25
        let dry = pattern.render()
        pattern.controllers = [.init(kind: .sustain, beat: 0, value: 1), .init(kind: .sustain, beat: 1.5, value: 0)]
        XCTAssertEqual(pattern.performedNotes()[0].durationBeats, 1.5)
        let held = pattern.render()
        XCTAssertTrue(dry.channels[0][12000..<24000].allSatisfy { $0 == 0 })
        XCTAssertGreaterThan(held.channels[0][12000..<24000].map(abs).max()!, 0.1)
        XCTAssertTrue(held.channels[0][40000...].allSatisfy { $0 == 0 })
        pattern.controllers.removeLast()
        XCTAssertEqual(pattern.performedNotes()[0].durationBeats, 4)
    }
    func testPitchBendHasMeasuredFrequencyExpressionIsSmoothedAndModulationChangesAudio() {
        var pattern = heldPattern()
        let dry = pattern.render()
        pattern.controllers = [.init(kind: .pitchBend, beat: 0, value: 1)]
        let bent = pattern.render().channels[0]
        let crossings = (24001..<72000).filter { bent[$0 - 1] <= 0 && bent[$0] > 0 }.count
        XCTAssertEqual(Double(crossings), 440 * pow(2, 2.0 / 12), accuracy: 2)
        pattern.controllers = [.init(kind: .expression, beat: 1, value: 0)]
        let faded = pattern.render().channels[0]
        XCTAssertLessThan(faded[26000..<30000].map(abs).max()!, 0.0001)
        XCTAssertLessThan(abs(faded[24000] - faded[23999]), 0.025)
        pattern.controllers = [.init(kind: .modulation, beat: 0, value: 1)]
        XCTAssertNotEqual(pattern.render(), dry)
    }
    func testArticulationAndDeterministicHumanizeStayWithinPatternBounds() {
        var pattern = heldPattern()
        pattern.notes[0].durationBeats = 1
        pattern.notes[0].velocity = 0.5
        pattern.notes[0].articulation = .staccato
        XCTAssertEqual(pattern.performedNotes()[0].durationBeats, 0.45)
        pattern.notes[0].articulation = .accent
        XCTAssertEqual(pattern.performedNotes()[0].velocity, 0.625)
        let notes = [
            SoundDesignNote(pitch: 60, startBeat: 0, durationBeats: 0.25),
            SoundDesignNote(pitch: 67, startBeat: 3.75, durationBeats: 0.25),
        ]
        let humanized = SoundDesignEditing.humanized(notes, patternBeats: 4)
        XCTAssertEqual(humanized, SoundDesignEditing.humanized(notes, patternBeats: 4))
        XCTAssertTrue(
            humanized.allSatisfy {
                $0.startBeat >= 0 && $0.startBeat + $0.durationBeats <= 4 && $0.velocity >= 0.05 && $0.velocity <= 1
            })
        XCTAssertEqual(SoundDesignEditing.humanized(notes, ids: [notes[0].id], patternBeats: 4)[1], notes[1])
    }
    func testLayeredRenderHasStereoRoutingVelocityZonesAndProjectRoundtrip() throws {
        var pattern = heldPattern()
        pattern.baseGain = 0
        var layer = SoundDesignLayer(preset: .glassPluck)
        layer.pan = -1
        layer.transpose = 12
        layer.minimumVelocity = 0.6
        pattern.layers = [layer]
        let layered = pattern.render()
        XCTAssertEqual(layered.channels.count, 2)
        XCTAssertTrue(layered.channels[1].allSatisfy { abs($0) < 0.000001 })
        XCTAssertGreaterThan(layered.channels[0].map(abs).max()!, 0.05)
        pattern.notes[0].velocity = 0.5
        XCTAssertTrue(pattern.render().channels.flatMap { $0 }.allSatisfy { $0 == 0 })
        XCTAssertEqual(try JSONDecoder().decode(SoundDesignPattern.self, from: JSONEncoder().encode(pattern)), pattern)
        let legacy = Data("{\"notes\":[],\"bpm\":120,\"beats\":4}".utf8)
        let decoded = try JSONDecoder().decode(SoundDesignPattern.self, from: legacy)
        XCTAssertTrue(decoded.controllers.isEmpty)
        XCTAssertTrue(decoded.layers.isEmpty)
    }
    func testLiveSustainAndTransposedLayerKeyReleaseAreBoundedAndStereo() {
        let instrument = SoundDesignInstrument(maximumVoices: 32)
        let pattern = heldPattern()
        instrument.addVoice(
            SoundDesignVoice(pitch: 81, velocity: 0.7, pattern: pattern, keyPitch: 69, gain: 0.5, pan: -1, stereo: true)
        )
        instrument.controller(.sustain, value: 1)
        instrument.noteOff(pitch: 69)
        var left = [Float](repeating: 0, count: 12000)
        var right = left
        left.withUnsafeMutableBufferPointer { l in
            right.withUnsafeMutableBufferPointer { r in
                instrument.renderStereo(left: l.baseAddress!, right: r.baseAddress!, count: 12000)
            }
        }
        XCTAssertGreaterThan(left.suffix(1000).map(abs).max()!, 0.05)
        XCTAssertTrue(right.allSatisfy { abs($0) < 0.000001 })
        XCTAssertEqual(instrument.activeVoiceCount, 1)
        instrument.controller(.sustain, value: 0)
        left.withUnsafeMutableBufferPointer { instrument.render(into: $0.baseAddress!, count: 12000) }
        XCTAssertTrue(left.suffix(1000).allSatisfy { $0 == 0 })
        XCTAssertEqual(instrument.activeVoiceCount, 0)
    }
    func testLiveControllerSignalIsIndependentOfBufferPartitions() {
        let pattern = heldPattern()
        let whole = SoundDesignInstrument()
        let split = SoundDesignInstrument()
        for instrument in [whole, split] {
            instrument.addVoice(.init(pitch: 69, velocity: 0.8, pattern: pattern))
            instrument.controller(.pitchBend, value: 0.7)
            instrument.controller(.modulation, value: 0.4)
        }
        func render(_ instrument: SoundDesignInstrument, count: Int) -> [Float] {
            var samples = [Float](repeating: 0, count: count)
            samples.withUnsafeMutableBufferPointer { instrument.render(into: $0.baseAddress!, count: count) }
            return samples
        }
        XCTAssertEqual(
            render(whole, count: 4096),
            render(split, count: 137) + render(split, count: 1024) + render(split, count: 2935))
    }
    func testLayerReleaseTailIsRetainedAndReportedForPreviewTiming() {
        var pattern = heldPattern()
        pattern.beats = 1
        pattern.notes[0].durationBeats = 1
        pattern.layers = [SoundDesignLayer(preset: .widePad)]
        XCTAssertEqual(pattern.performanceRelease, 1.2)
        XCTAssertEqual(pattern.render().count, Int((0.5 + 1.2) * sessionRate))
    }
    func testLayeredOfflineAndPreparedLiveStereoMatchAtTheSameNote() {
        var pattern = heldPattern()
        pattern.notes[0].durationBeats = 0.1
        pattern.baseGain = 0.4
        pattern.basePan = 0.2
        var layer = SoundDesignLayer(preset: .glassPluck)
        layer.gain = 0.3
        layer.pan = -0.7
        pattern.layers = [layer]
        let offline = pattern.render()
        let instrument = SoundDesignInstrument(maximumVoices: 32)
        instrument.addVoice(
            .init(
                pitch: 69, velocity: 0.8, pattern: pattern, gain: pattern.baseGain, pan: pattern.basePan, stereo: true))
        instrument.addVoice(
            .init(
                pitch: 69, velocity: 0.8, pattern: layer.instrument(for: pattern), keyPitch: 69, gain: layer.gain,
                pan: layer.pan, stereo: true))
        var left = [Float](repeating: 0, count: 2400)
        var right = left
        left.withUnsafeMutableBufferPointer { l in
            right.withUnsafeMutableBufferPointer { r in
                instrument.renderStereo(left: l.baseAddress!, right: r.baseAddress!, count: 2400)
            }
        }
        XCTAssertLessThan(zip(left, offline.channels[0]).map { abs($0 - $1) }.max()!, 0.000001)
        XCTAssertLessThan(zip(right, offline.channels[1]).map { abs($0 - $1) }.max()!, 0.000001)
    }
    func testMIDIControllersRoundtripAndArticulationIsCompatible() throws {
        var pattern = heldPattern()
        pattern.controllers = [
            .init(kind: .sustain, beat: 0, value: 1), .init(kind: .pitchBend, beat: 0.5, value: 0.5),
            .init(kind: .modulation, beat: 1, value: 0.7), .init(kind: .expression, beat: 2, value: 0.3),
            .init(kind: .sustain, beat: 3, value: 0),
        ]
        var articulated = pattern
        articulated.notes[0].durationBeats = 1
        articulated.notes[0].articulation = .staccato
        XCTAssertEqual(try MIDIFile.decode(MIDIFile.encode(articulated)).notes[0].durationBeats, 0.45, accuracy: 0.003)
        let decoded = try MIDIFile.decode(MIDIFile.encode(pattern))
        XCTAssertEqual(decoded.notes.map(\.startBeat), pattern.notes.map(\.startBeat))
        XCTAssertEqual(decoded.notes.map(\.durationBeats), pattern.notes.map(\.durationBeats))
        XCTAssertEqual(decoded.controllers.count, pattern.controllers.count)
        for (actual, expected) in zip(decoded.controllers, pattern.controllers) {
            XCTAssertEqual(actual.kind, expected.kind)
            XCTAssertEqual(actual.beat, expected.beat, accuracy: 0.003)
            XCTAssertEqual(actual.value, expected.value, accuracy: 1 / 127)
        }
    }
}
