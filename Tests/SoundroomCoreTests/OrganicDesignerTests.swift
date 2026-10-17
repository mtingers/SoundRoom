import XCTest

@testable import SoundroomCore

final class OrganicDesignerTests: XCTestCase {
    private func sequence() -> SoundDesignPattern {
        var p = SoundDesignPattern(
            notes: [.init(pitch: 60, velocity: 0.72, startBeat: 0, durationBeats: 0.5)], bpm: 92, beats: 1)
        p.controllers = [.init(kind: .expression, beat: 0, value: 0.8)]
        return p
    }
    func testOrganicBypassAndLegacyLayerDecodingPreserveOldSound() throws {
        let p = sequence()
        let encoder = JSONEncoder()
        var object = try JSONSerialization.jsonObject(with: encoder.encode(p)) as! [String: Any]
        object.removeValue(forKey: "organic")
        let old = try JSONDecoder().decode(
            SoundDesignPattern.self, from: JSONSerialization.data(withJSONObject: object))
        XCTAssertEqual(old, p)
        XCTAssertEqual(old.render(), p.render())
        let layer = try JSONDecoder().decode(
            SoundDesignLayer.self,
            from: Data(
                "{\"preset\":\"Wide pad\",\"gain\":0.4,\"pan\":-0.3,\"transpose\":12,\"minimumVelocity\":0,\"maximumVelocity\":1,\"enabled\":true}"
                    .utf8))
        XCTAssertNil(layer.toneOverride)
        XCTAssertNil(layer.velocityFade)
        XCTAssertFalse(layer.hasCustomSound)
        XCTAssertEqual(layer.velocityWeight(0.5), 1)
    }
    func testPatchAndSoundPresetPreserveSequenceAndControllers() throws {
        var source = sequence()
        source.organic = OrganicSound(drift: 0.2, timbre: 0.3, breath: 0.1)
        for patch in OrganicPatch.allCases {
            let p = patch.applying(to: source)
            XCTAssertEqual(p.notes, source.notes)
            XCTAssertEqual(p.controllers, source.controllers)
            XCTAssertEqual(p.bpm, source.bpm)
            XCTAssertEqual(p.beats, source.beats)
            XCTAssertEqual(p.layers.count, [.midnightUpright, .forestFlute, .lateNightVibes].contains(patch) ? 1 : 2)
            XCTAssertTrue(p.organic.isActive)
            let saved = DesignerSoundPreset(pattern: p)
            XCTAssertTrue(saved.sound.notes.isEmpty)
            XCTAssertTrue(saved.sound.controllers.isEmpty)
            let loaded = try JSONDecoder().decode(DesignerSoundPreset.self, from: JSONEncoder().encode(saved))
            XCTAssertEqual(loaded.applying(to: source), p)
            XCTAssertEqual(
                try JSONDecoder().decode(SoundDesignPattern.self, from: JSONEncoder().encode(p)).render(
                    sampleRate: 8000), p.render(sampleRate: 8000))
            XCTAssertTrue(p.render(sampleRate: 8000).channels.flatMap { $0 }.allSatisfy { $0.isFinite && abs($0) < 1 })
        }
        for preset in SoundDesignPreset.allCases {
            XCTAssertEqual(preset.applying(to: source).controllers, source.controllers)
            XCTAssertEqual(preset.applying(to: source).organic, source.organic)
        }
    }
    func testOrganicRenderIsRepeatableAndSeedChangesSoundWithoutEditingNotes() {
        var p = OrganicPatch.feltGlass.applying(to: sequence())
        let originalNotes = p.notes
        let first = p.render(sampleRate: 8000)
        XCTAssertEqual(p.render(sampleRate: 8000), first)
        p.organic.seed &+= 1
        XCTAssertNotEqual(p.render(sampleRate: 8000), first)
        XCTAssertEqual(p.notes, originalNotes)
    }
    func testOrganicMotionBoundsContinuityAndDeterministicNoteVariation() {
        let settings = OrganicSound(drift: 1, timbre: 1, breath: 1)
        var motion = OrganicMotion(settings: settings, pitch: 60, beat: 4, rate: 8000)
        var last = motion.ratio
        for _ in 0..<80000 {
            let cents = 1200 * log2(motion.ratio)
            XCTAssertLessThanOrEqual(abs(cents), 6.000001)
            XCTAssertGreaterThanOrEqual(motion.gain, 0.94)
            XCTAssertLessThanOrEqual(motion.gain, 1.06)
            XCTAssertLessThan(abs(motion.ratio - last), 0.00006)
            last = motion.ratio
            motion.advance()
        }
        XCTAssertEqual(settings.variation(pitch: 60, beat: 4), settings.variation(pitch: 60, beat: 4))
        XCTAssertNotEqual(settings.variation(pitch: 60, beat: 4), settings.variation(pitch: 60, beat: 5))
    }
    func testLayerCrossfadeAndSoundOverridesApplyToAuditionAndOffline() {
        var layer = SoundDesignLayer(preset: .studioPiano)
        layer.minimumVelocity = 0.2
        layer.maximumVelocity = 0.8
        layer.velocityFade = 0.2
        XCTAssertEqual(layer.velocityWeight(0.1), 0)
        XCTAssertEqual(layer.velocityWeight(0.2), 0)
        XCTAssertEqual(layer.velocityWeight(0.4), 1, accuracy: 0.000001)
        XCTAssertEqual(layer.velocityWeight(0.8), 0, accuracy: 0.000001)
        XCTAssertGreaterThan(layer.velocityWeight(0.35), layer.velocityWeight(0.25))
        layer.toneOverride = 0.3
        layer.attackOverride = 0.015
        layer.releaseOverride = 0.6
        layer.characterOverride = 0.2
        var source = sequence()
        source.organic = OrganicSound(drift: 0.4, timbre: 0.6, breath: 0.3)
        source.notes[0].velocity = 0.3
        source.baseGain = 0
        source.layers = [layer]
        let voicePattern = layer.instrument(for: source)
        XCTAssertEqual(voicePattern.tone, 0.3)
        XCTAssertEqual(voicePattern.release, 0.6)
        XCTAssertEqual(source.performanceRelease, 0.6)
        var voice = SoundDesignVoice(
            pitch: 60, velocity: 0.3 * layer.velocityWeight(0.3), pattern: voicePattern, sampleRate: 8000,
            gain: layer.gain, stereo: true)
        voice.controller(.expression, value: 0.8, immediate: true)
        let offline = source.render(sampleRate: 8000)
        for frame in 0..<1000 {
            let live = Float(tanh(Double(voice.renderNext() * voice.leftGain)))
            XCTAssertEqual(live, offline.channels[0][frame], accuracy: 0.000001)
        }
    }
    func testOrganicSynthOfflineAndPreparedVoiceAgreeAndPartitionsStayStable() {
        var p = sequence()
        p.organic = OrganicSound(drift: 0.6, timbre: 0.5, breath: 0.4)
        p.controllers = []
        p.noiseLevel = 0.1
        let offline = p.render(sampleRate: 8000)
        var voice = SoundDesignVoice(pitch: 60, velocity: 0.72, pattern: p, sampleRate: 8000)
        for frame in 0..<1000 {
            XCTAssertEqual(Float(tanh(Double(voice.renderNext()))), offline.channels[0][frame], accuracy: 0.000001)
        }
        func render(_ counts: [Int]) -> [Float] {
            let instrument = SoundDesignInstrument()
            instrument.addVoice(.init(pitch: 60, velocity: 0.72, pattern: p, sampleRate: 8000))
            return counts.flatMap { count in
                var b = [Float](repeating: 0, count: count)
                b.withUnsafeMutableBufferPointer { instrument.render(into: $0.baseAddress!, count: count) }
                return b
            }
        }
        XCTAssertEqual(render([2048]), render([127, 512, 1409]))
    }
    func testOutOfRangeOrganicAndLayerValuesDecodeSafely() throws {
        let organic = try JSONDecoder().decode(
            OrganicSound.self, from: Data("{\"drift\":100000,\"timbre\":-4,\"breath\":1e200}".utf8))
        XCTAssertEqual(organic.drift, 1)
        XCTAssertEqual(organic.timbre, 0)
        XCTAssertEqual(organic.breath, 1)
        let layer = try JSONDecoder().decode(
            SoundDesignLayer.self,
            from: Data(
                "{\"gain\":1e200,\"pan\":-1e200,\"transpose\":300,\"toneOverride\":8,\"attackOverride\":-2,\"velocityFade\":9}"
                    .utf8))
        XCTAssertEqual(layer.gain, 1)
        XCTAssertEqual(layer.pan, -1)
        XCTAssertEqual(layer.transpose, 24)
        XCTAssertEqual(layer.toneOverride, 1)
        XCTAssertEqual(layer.attackOverride, 0)
        XCTAssertEqual(layer.velocityFade, 0.5)
    }
    func testViewportBoundsZoomAnchorAndNotesCrossingLeftEdge() {
        let view = DesignerViewport(start: 40, span: 8).fitted(to: 64)
        let zoom = view.zoomed(by: 0.5, around: 44, beats: 64)
        XCTAssertEqual(zoom.start, 42)
        XCTAssertEqual(zoom.span, 4)
        XCTAssertEqual(DesignerViewport(start: 100, span: 8).fitted(to: 64).start, 56)
        XCTAssertEqual(
            DesignerViewport(start: .nan, span: .infinity).fitted(to: 64), DesignerViewport(start: 0, span: 8))
        XCTAssertTrue(view.includes(.init(pitch: 60, startBeat: 39, durationBeats: 2)))
        XCTAssertFalse(view.includes(.init(pitch: 60, startBeat: 48, durationBeats: 2)))
        XCTAssertEqual(view.beat(at: 0.5), 44)
        XCTAssertEqual(DesignerViewport(span: 64).fitted(to: 1).span, 1)
    }
}
