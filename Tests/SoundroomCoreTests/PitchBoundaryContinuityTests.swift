import XCTest

@testable import SoundroomCore

/// These reproducible signals isolate synthesis and edit-boundary failures.
/// They do not qualify the naturalness of a microphone vocal recording.
final class PitchBoundaryContinuityTests: XCTestCase {
    private func voice(frequency: Double, count: Int, phaseOffset: Double = 0) -> [Float] {
        (0..<count).map { frame in
            let phase = 2 * Double.pi * frequency * Double(frame) / sessionRate + phaseOffset
            return Float(0.25 * sin(phase) + 0.08 * sin(2 * phase + 0.4) + 0.03 * sin(3 * phase + 0.8))
        }
    }
    private func maximumStep(_ samples: [Float], in range: Range<Int>) -> (frame: Int, step: Float) {
        var result = (frame: range.lowerBound, step: Float(0))
        for frame in max(1, range.lowerBound)..<min(samples.count, range.upperBound) {
            let step = abs(samples[frame] - samples[frame - 1])
            if step > result.step { result = (frame, step) }
        }
        return result
    }
    private func insert(regions: [PitchEditRegion], hardTune: Bool = false) -> RealtimeEffect {
        var p = RealtimeEffectKind.pitchCorrection.defaultParameters
        if hardTune {
            p["retune"] = 0
            p["humanize"] = 0
            p["vibrato"] = 0
            p["tolerance"] = 0
        }
        return RealtimeEffect(kind: .pitchCorrection, parameters: p, pitchEdits: regions, pitchEditsOnly: true)
    }
    func testHeldDownwardOctaveDoesNotReinsertDryCarrierAtGrainTroughs() {
        for (frequency, note) in [(Double(440), Float(57)), (Double(526), Float(65))] {
            let input = voice(frequency: frequency, count: 96_000, phaseOffset: 1.7)
            let state = PitchCorrectionState()
            var p = PitchCorrectionParameters()
            p.retune = 0
            p.humanize = 0
            p.vibrato = 0
            p.tolerance = 0
            p.manualTarget = note
            let output = input.map { state.process(left: $0, right: -0.7 * $0, parameters: p).0 }
            let largest = maximumStep(output, in: 24_000..<90_000)
            // The source's largest step is about 0.035; a lower, envelope-modulated
            // carrier can tolerate twice that slope, not an isolated jump back to
            // the 0.25-amplitude dry carrier at a Hann-window trough.
            print(
                "Held downward note \(note) from \(frequency) Hz largest sample step \(largest.step) at \(largest.frame)"
            )
            XCTAssertLessThan(largest.step, 0.08, "An isolated grain-trough spike is audible as a click")
            let expected = 440 * pow(2, Double(note - 69) / 12)
            var real: Double = 0
            var imaginary: Double = 0
            for frame in 48_000..<72_000 {
                let phase = Double(frame) * 2 * Double.pi * expected / sessionRate
                real += Double(output[frame]) * cos(phase)
                imaginary += Double(output[frame]) * sin(phase)
            }
            XCTAssertGreaterThan(
                2 * hypot(real, imaginary) / 24_000, 0.03,
                "Click removal must retain the selected \(expected) Hz fundamental")
        }
    }
    func testDefaultRetuneLargeCurvesAcrossVoicedGapRemainQuietAndContinuous() {
        let firstEnd = 80_000
        let secondStart = firstEnd + 9472
        let count = 136_000
        var source = [Float](repeating: 0, count: count)
        let first = voice(frequency: 450, count: firstEnd)
        let second = voice(frequency: 526, count: count - secondStart, phaseOffset: 1.7)
        // Genuine smooth phrase ends/starts, not a source waveform discontinuity.
        for frame in first.indices {
            let edge = min(frame, first.count - 1 - frame)
            let fade = min(1, Float(edge) / 240)
            source[frame] = first[frame] * fade
        }
        for frame in second.indices {
            let edge = min(frame, second.count - 1 - frame)
            let fade = min(1, Float(edge) / 240)
            source[secondStart + frame] = second[frame] * fade
        }
        let regions = [
            PitchEditRegion(
                range: 5000..<firstEnd,
                points: [
                    .init(frame: 5000, note: 69.25, curve: .smooth), .init(frame: 30_000, note: 67, curve: .smooth),
                    .init(frame: 35_000, note: 77, curve: .smooth), .init(frame: firstEnd, note: 70.15),
                ]),
            PitchEditRegion(
                range: secondStart..<(count - 5000),
                points: [
                    .init(frame: secondStart, note: 73, curve: .smooth),
                    .init(frame: secondStart + 12_000, note: 72.1, curve: .smooth),
                    .init(frame: secondStart + 22_000, note: 65, curve: .smooth),
                    .init(frame: secondStart + 23_000, note: 72.1, curve: .smooth),
                    .init(frame: count - 5000, note: 79),
                ]),
        ]
        let input = AudioSamples(channels: [source])
        let output = insert(regions: regions).process(input).channels[0]
        XCTAssertTrue(output.allSatisfy(\.isFinite))
        for frame in firstEnd..<secondStart {
            XCTAssertEqual(
                output[frame], 0, accuracy: 0.000001, "Voiced edit tails must not fill a dry silent gap at \(frame)")
        }
        for range in [
            (firstEnd - 480)..<(firstEnd + 480), (secondStart - 480)..<(secondStart + 960),
            (secondStart + 20_000)..<(secondStart + 25_000),
        ] {
            let largest = maximumStep(output, in: range)
            print("Default pitch boundary largest sample step \(largest.step) at \(largest.frame)")
            XCTAssertLessThan(
                largest.step, 0.08, "A smooth source and retuned curve must not introduce a boundary impulse")
        }
        let residual = zip(output, source).map { $0 - $1 }
        for boundary in [5000, firstEnd, secondStart, count - 5000] {
            let largest = maximumStep(residual, in: max(1, boundary - 2)..<min(count, boundary + 3))
            XCTAssertLessThan(
                largest.step, 0.001, "Scope edge derivatives must vanish independently of the source slope")
        }
    }
    func testProtectedDifferentClipJoinKeepsItsZeroEndpointAfterPitch() {
        let length = 16_000
        let firstID = UUID()
        let secondID = UUID()
        let assets = [
            firstID: AudioSamples(channels: [voice(frequency: 450, count: length)]),
            secondID: AudioSamples(channels: [voice(frequency: 250, count: length, phaseOffset: 1.7)]),
        ]
        var session = Session()
        session.tracks[0].clips = [
            Clip(assetID: firstID, name: "First", start: 0, length: length, edgeFadeFrames: 240),
            Clip(assetID: secondID, name: "Second", start: length, length: length, edgeFadeFrames: 240),
        ]
        var p = RealtimeEffectKind.pitchCorrection.defaultParameters
        p["retune"] = 0
        p["humanize"] = 0
        p["vibrato"] = 0
        p["tolerance"] = 0
        let dry = Mixer.render(session: session, assets: assets, range: 0..<(2 * length)).channels[0]
        XCTAssertEqual(dry[length - 1], 0)
        XCTAssertEqual(dry[length], 0)
        session.tracks[0].realtimeEffects = [RealtimeEffect(kind: .pitchCorrection, parameters: p)]
        let output = Mixer.render(session: session, assets: assets, range: 0..<(2 * length)).channels[0]
        print("Protected pitch clip join endpoints \(output[length - 1]), \(output[length])")
        XCTAssertEqual(
            output[length - 1], 0, accuracy: 0.002, "Pitch grains must preserve the protected release endpoint")
        XCTAssertEqual(output[length], 0, accuracy: 0.002, "Pitch grains must preserve the protected attack endpoint")
        let largest = maximumStep(output, in: (length - 480)..<(length + 480))
        XCTAssertLessThan(
            largest.step, 0.08, "A protected unlike-source join must not acquire a one-sample pitch impulse")
    }
    func testSourceContiguousSplitRemainsIdenticalWithPitch() {
        let count = 40_000
        let split = 18_713
        let id = UUID()
        let assets = [id: AudioSamples(channels: [voice(frequency: 450, count: count)])]
        var session = Session()
        session.tracks[0].clips = [Clip(assetID: id, name: "Whole", start: 0, length: count, edgeFadeFrames: 240)]
        session.tracks[0].realtimeEffects = [RealtimeEffect(kind: .pitchCorrection)]
        let reference = Mixer.render(session: session, assets: assets, range: 0..<count)
        session.tracks[0].clips = [
            Clip(assetID: id, name: "Left", start: 0, length: split, edgeFadeFrames: 240),
            Clip(assetID: id, name: "Right", start: split, offset: split, length: count - split, edgeFadeFrames: 240),
        ]
        let splitOutput = Mixer.render(session: session, assets: assets, range: 0..<count)
        XCTAssertEqual(
            splitOutput, reference,
            "A source-contiguous split must not introduce another edge gate or reset pitch state")
    }
}
