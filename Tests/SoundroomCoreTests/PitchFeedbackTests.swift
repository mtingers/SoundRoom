import XCTest
@testable import SoundroomCore

final class PitchFeedbackTests: XCTestCase {
    func testWetEstimateIncludesAppliedShiftAndReferenceButRejectsUnvoicedData() {
        var value = PitchMeterSnapshot()
        value.inputFrequency = 450; value.confidence = 0.98; value.sourceFrame = 100
        value.correctionCents = 1200 * log2(440 / 450)
        XCTAssertTrue(value.isVoiced)
        XCTAssertEqual(value.estimatedWetFrequency, 440, accuracy: 0.001)
        value.referenceFrequency = 442
        XCTAssertEqual(value.midiNote(for: 442)!, 69, accuracy: 0.00001)
        value.confidence = 0.2
        XCTAssertFalse(value.isVoiced); XCTAssertEqual(value.estimatedWetFrequency, 0)
        value.confidence = 1; value.inputFrequency = .nan
        XCTAssertFalse(value.isVoiced); XCTAssertNil(value.midiNote(for: .nan))
    }

    func testFeedbackDelayUsesExactDownstreamLatencyAndHasNoStaleInitialPitch() {
        let delay = PitchMeterDelay(frames: 16)
        for frame in 0..<60 {
            var value = PitchMeterSnapshot()
            value.sourceFrame = frame; value.inputFrequency = 450 + Float(frame)
            value.targetFrequency = 440; value.confidence = 1; value.correctionCents = Float(frame)
            let delayed = delay.process(value)
            if frame < 16 {
                XCTAssertFalse(delayed.isVoiced); XCTAssertEqual(delayed.sourceFrame, -1)
            } else {
                XCTAssertEqual(delayed.sourceFrame, frame - 16)
                XCTAssertEqual(delayed.inputFrequency, 450 + Float(frame - 16))
                XCTAssertEqual(delayed.correctionCents, Float(frame - 16))
            }
        }
        let clone = delay.deepCopy()
        for frame in 60..<92 {
            var a = PitchMeterSnapshot(); a.sourceFrame = frame; a.inputFrequency = 550; a.confidence = 1
            var b = a; b.inputFrequency = 650
            let first = delay.process(a), second = clone.process(b)
            if frame >= 76 { XCTAssertEqual(first.inputFrequency, 550); XCTAssertEqual(second.inputFrequency, 650) }
        }
    }

    func testMixerPitchFeedbackFollowsAudibleTimelineThroughDownstreamLatencyAndBypass() {
        let asset = UUID(), pitchID = UUID()
        var parameters = RealtimeEffectKind.pitchCorrection.defaultParameters
        parameters["retune"] = 0; parameters["humanize"] = 0; parameters["vibrato"] = 0; parameters["tolerance"] = 0
        var session = Session()
        session.tracks[0].clips = [Clip(assetID: asset, name: "Voice", start: 0, length: 48_000, edgeFadeFrames: 0)]
        var pitchEffect = RealtimeEffect(kind: .pitchCorrection, parameters: parameters)
        pitchEffect.id = pitchID
        session.tracks[0].realtimeEffects = [
            pitchEffect,
            RealtimeEffect(kind: .warmth, parameters: RealtimeEffectKind.warmth.defaultParameters)
        ]
        let samples = (0..<48_000).map { Float(sin(2 * .pi * 450 * Double($0) / sessionRate)) * 0.2 }
        let mixer = RealtimeMixer(session: session, assets: [asset: AudioSamples(channels: [samples])])
        var left = [Float](repeating: 0, count: 8192), right = left
        for _ in 0..<3 {
            left.withUnsafeMutableBufferPointer { l in right.withUnsafeMutableBufferPointer { r in
                mixer.render(intoLeft: l.baseAddress!, intoRight: r.baseAddress!, frameCount: l.count)
            } }
        }
        let value = try! XCTUnwrap(mixer.effectMeter(id: pitchID)?.snapshot().pitch)
        XCTAssertEqual(value.sourceFrame, mixer.position - 1)
        XCTAssertEqual(value.inputFrequency, 450, accuracy: 0.5)
        XCTAssertEqual(value.targetFrequency, 440, accuracy: 0.5)
        XCTAssertEqual(value.estimatedWetFrequency, 440, accuracy: 0.5)
        XCTAssertGreaterThan(value.wetBlend, 0.95)

        // The downstream feedback ring travels with surviving DSP history when
        // a manual parameter edit replaces the graph at a block boundary.
        session.tracks[0].realtimeEffects[0].parameters["retune"] = 20
        let prepared = RealtimeMixer.prepareUpdate(session: session, assets: [asset: AudioSamples(channels: [samples])])
        mixer.applyPreparedUpdate(prepared, smoothManualChanges: true)
        left.withUnsafeMutableBufferPointer { l in right.withUnsafeMutableBufferPointer { r in
            mixer.render(intoLeft: l.baseAddress!, intoRight: r.baseAddress!, frameCount: 8)
        } }
        let adopted = try! XCTUnwrap(mixer.effectMeter(id: pitchID)?.snapshot().pitch)
        XCTAssertTrue(adopted.isVoiced); XCTAssertEqual(adopted.sourceFrame, mixer.position - 1)
        prepared.releaseRetiredResources()

        session.tracks[0].realtimeEffects[0].enabled = false
        mixer.update(session: session, assets: [asset: AudioSamples(channels: [samples])])
        left.withUnsafeMutableBufferPointer { l in right.withUnsafeMutableBufferPointer { r in
            mixer.render(intoLeft: l.baseAddress!, intoRight: r.baseAddress!, frameCount: l.count)
        } }
        XCTAssertEqual(mixer.effectMeter(id: pitchID)?.snapshot().pitch?.wetBlend, 0)
    }

    func testFeedbackSurvivesCachedSeekWithMultipleLatencyInserts() {
        let asset = UUID()
        var session = Session()
        session.tracks[0].clips = [Clip(assetID: asset, name: "Voice", start: 0, length: 36_000, edgeFadeFrames: 0)]
        session.tracks[0].realtimeEffects = [RealtimeEffect(kind: .pitchCorrection), RealtimeEffect(kind: .pitchCorrection)]
        let id = session.tracks[0].realtimeEffects[0].id
        let samples = (0..<36_000).map { Float(sin(2 * .pi * 450 * Double($0) / sessionRate)) * 0.2 }
        let assets = [asset: AudioSamples(channels: [samples])], cache = DSPSeekCache(intervalFrames: 2048)
        _ = RealtimeMixer(session: session, assets: assets, startFrame: 24_000, seekCache: cache)
        let mixer = RealtimeMixer(session: session, assets: assets, startFrame: 16_384, seekCache: cache)
        XCTAssertNotNil(mixer.restoredCheckpointFrame)
        var left = [Float](repeating: 0, count: 128), right = left
        left.withUnsafeMutableBufferPointer { l in right.withUnsafeMutableBufferPointer { r in
            mixer.render(intoLeft: l.baseAddress!, intoRight: r.baseAddress!, frameCount: l.count)
        } }
        let value = try! XCTUnwrap(mixer.effectMeter(id: id)?.snapshot().pitch)
        XCTAssertTrue(value.isVoiced)
        XCTAssertEqual(value.sourceFrame, mixer.position - 1)
        XCTAssertEqual(value.inputFrequency, 450, accuracy: 0.5)
    }

    func testSelectionContoursMeasureRenderedAudioAndKeepStereoPhaseSafe() throws {
        let samples = (0..<16_000).map { Float(sin(2 * .pi * 450 * Double($0) / sessionRate)) * 0.2 }
        let beforeAudio = AudioSamples(channels: [samples, samples.map { -$0 }])
        var parameters = RealtimeEffectKind.pitchCorrection.defaultParameters
        parameters["retune"] = 0; parameters["humanize"] = 0; parameters["vibrato"] = 0; parameters["tolerance"] = 0
        let wet = BuiltInEffect.pitchCorrection.process(beforeAudio, parameters: parameters)
        let before = try PitchContourAnalysis.analyze(beforeAudio).filter(\.isVoiced)
        let after = try PitchContourAnalysis.analyze(wet).filter(\.isVoiced)
        XCTAssertGreaterThan(before.count, 20); XCTAssertGreaterThan(after.count, 20)
        XCTAssertTrue(before.allSatisfy { (0..<beforeAudio.count).contains($0.frame) })
        XCTAssertEqual(before.reduce(0) { $0 + $1.frequency } / Float(before.count), 450, accuracy: 0.7)
        XCTAssertEqual(after.reduce(0) { $0 + $1.frequency } / Float(after.count), 440, accuracy: 1)
        let silent = try PitchContourAnalysis.analyze(AudioSamples(channels: [[Float](repeating: 0, count: 8000)]))
        XCTAssertFalse(silent.contains(where: \.isVoiced))
    }
}
