import XCTest

@testable import SoundroomCore

final class MasterBusTests: XCTestCase {
    private func fixture(count: Int = 12_000) -> (Session, [UUID: AudioSamples]) {
        let id = UUID()
        var session = Session()
        session.tracks[0].clips = [
            .init(assetID: id, name: "Master source", start: 0, length: count, edgeFadeFrames: 0)
        ]
        var l = (0..<count).map { Float(sin(Double($0) * 0.031) * 0.18 + cos(Double($0) * 0.007) * 0.07) }
        l[0] = 0.31
        l[count - 1] = -0.28
        return (session, [id: AudioSamples(channels: [l, l.map { $0 * -0.6 }])])
    }
    private func transparentWarmth() -> RealtimeEffect {
        var values = RealtimeEffectKind.warmth.defaultParameters
        values["mix"] = 0
        return .init(kind: .warmth, parameters: values)
    }
    private func render(_ mixer: RealtimeMixer, count: Int) -> AudioSamples {
        var l = [Float](repeating: 0, count: count)
        var r = l
        l.withUnsafeMutableBufferPointer { lp in
            r.withUnsafeMutableBufferPointer { rp in
                mixer.render(intoLeft: lp.baseAddress!, intoRight: rp.baseAddress!, frameCount: count)
            }
        }
        return AudioSamples(channels: [l, r])
    }
    private func assertSamples(
        _ a: AudioSamples, _ b: AudioSamples, accuracy: Float = 0.000002, file: StaticString = #filePath,
        line: UInt = #line
    ) {
        XCTAssertEqual(a.count, b.count, file: file, line: line)
        XCTAssertEqual(a.channelCount, b.channelCount, file: file, line: line)
        var largest: Float = 0
        for channel in 0..<min(a.channelCount, b.channelCount) {
            for frame in 0..<min(a.count, b.count) {
                largest = max(
                    largest, abs(a.sample(channel: channel, frame: frame) - b.sample(channel: channel, frame: frame)))
            }
        }
        XCTAssertLessThanOrEqual(largest, accuracy, file: file, line: line)
    }
    func testEmptyMasterAddsNoLatencyAndPreservesImmediateTrackUpdates() {
        var (session, assets) = fixture()
        XCTAssertEqual(MasterBusProcessor.lookAheadFrames(for: session.masterTrack), 0)
        let mixer = RealtimeMixer(session: session, assets: assets)
        assertSamples(
            render(mixer, count: 333), Mixer.render(session: session, assets: assets, range: 0..<333), accuracy: 0)
        session.tracks[0].gain = 0.25
        mixer.update(session: session, assets: assets)
        assertSamples(
            render(mixer, count: 256), Mixer.render(session: session, assets: assets, range: 333..<589), accuracy: 0)
        let seek = RealtimeMixer(session: session, assets: assets, startFrame: 7000)
        assertSamples(
            render(seek, count: 513), Mixer.render(session: session, assets: assets, range: 7000..<7513), accuracy: 0)
    }
    func testMasterLimiterProtectsFinalSumAfterTrackAndMasterGain() {
        var (session, assets) = fixture(count: 8000)
        let id = assets.keys.first!
        assets[id] = AudioSamples(channels: [
            [Float](repeating: 0.8, count: 8000), [Float](repeating: 0.4, count: 8000),
        ])
        var second = session.tracks[0]
        second.id = UUID()
        second.gain = 1.3
        session.tracks.append(second)
        session.tracks[0].gain = 1.2
        session.masterTrack.gain = 1.4
        var values = RealtimeEffectKind.limiter.defaultParameters
        values["ceiling"] = -2
        values["truePeak"] = 0
        let effect = RealtimeEffect(kind: .limiter, parameters: values)
        session.masterTrack.realtimeEffects = [effect]
        let mixer = RealtimeMixer(session: session, assets: assets)
        let limited = render(mixer, count: 8000)
        XCTAssertLessThanOrEqual(limited.channels[0].map(abs).max()!, pow(10, Float(-2) / 20) + 0.00001)
        XCTAssertGreaterThan(limited.channels[0][7000], 0.6)
        XCTAssertEqual(limited.channels[1][7000] / limited.channels[0][7000], 0.5, accuracy: 0.00001)
        XCTAssertGreaterThan(mixer.effectMeter(id: effect.id)!.snapshot().gainReductionDB, 8)
    }
    func testMasterLatencyPreservesEndpointsTrackAlignmentAndParameterAutomation() {
        var (reference, assets) = fixture()
        var second = reference.tracks[0]
        second.id = UUID()
        second.gain = 0.35
        second.pan = 0.4
        second.realtimeEffects = [transparentWarmth()]
        reference.tracks.append(second)
        let descriptor = RealtimeEffectKind.gain.parameters[0]
        let gain = RealtimeEffect(
            kind: .gain, parameters: ["gain": 0],
            parameterAutomation: [
                "gain": [
                    .init(frame: 0, value: descriptor.normalized(-12)),
                    .init(frame: 11999, value: descriptor.normalized(2)),
                ]
            ])
        reference.masterTrack.realtimeEffects = [gain]
        let expected = Mixer.render(session: reference, assets: assets, range: 0..<12000)
        var delayed = reference
        delayed.masterTrack.realtimeEffects = [transparentWarmth(), gain, transparentWarmth()]
        XCTAssertEqual(MasterBusProcessor.lookAheadFrames(for: delayed.masterTrack), 4096)
        assertSamples(Mixer.render(session: delayed, assets: assets, range: 0..<12000), expected, accuracy: 0)
        assertSamples(
            Mixer.render(session: delayed, assets: assets, range: 8371..<12000), expected.slice(8371..<12000),
            accuracy: 0)
    }
    func testMasterVolumeAutomationUsesAudibleTimelineAndWrapsLoop() {
        var (reference, assets) = fixture(count: 8000)
        reference.masterTrack.mixAutomation["volume"] = [
            .init(frame: 1000, value: 0.2), .init(frame: 2000, value: 0.8), .init(frame: 2999, value: 0.4),
        ]
        var delayed = reference
        delayed.masterTrack.realtimeEffects = [transparentWarmth()]
        let loop = 1000..<3000
        let a = RealtimeMixer(
            session: reference, assets: assets, startFrame: loop.lowerBound, renderEndFrame: loop.upperBound,
            loopRange: loop)
        let b = RealtimeMixer(
            session: delayed, assets: assets, startFrame: loop.lowerBound, renderEndFrame: loop.upperBound,
            loopRange: loop)
        for _ in 0..<3 {
            assertSamples(render(b, count: 2000), render(a, count: 2000), accuracy: 0)
            a.rewindForLoop(to: loop.lowerBound)
            b.rewindForLoop(to: loop.lowerBound)
        }
    }
    func testMasterSeekCheckpointRestoresIndependentLimiterAndPaddingHistory() {
        var (session, assets) = fixture(count: 16000)
        session.masterTrack.realtimeEffects = [.init(kind: .limiter, parameters: ["input": 18, "ceiling": -1])]
        let cache = DSPSeekCache(intervalFrames: 2000, maximumBytes: 8 * 1024 * 1024)
        _ = RealtimeMixer(session: session, assets: assets, startFrame: 13000, seekCache: cache)
        let cached = RealtimeMixer(session: session, assets: assets, startFrame: 11031, seekCache: cache)
        let plain = RealtimeMixer(session: session, assets: assets, startFrame: 11031)
        XCTAssertEqual(cached.restoredCheckpointFrame, 10000)
        for count in [1, 77, 513] {
            assertSamples(render(cached, count: count), render(plain, count: count), accuracy: 0)
        }
        let again = RealtimeMixer(session: session, assets: assets, startFrame: 11031, seekCache: cache)
        assertSamples(
            render(again, count: 512), Mixer.render(session: session, assets: assets, range: 11031..<11543), accuracy: 0
        )
        XCTAssertLessThanOrEqual(cache.residentBytes, cache.maximumBytes)
        XCTAssertGreaterThan(
            cache.residentBytes, cache.checkpointCount * 160_000,
            "Master callback work buffers must count toward the cache budget")
    }
    func testMasterLatencyRackIsIndependentOfRenderPartitioningAndInteractiveSeekPrimesCue() {
        var (session, assets) = fixture(count: 14000)
        session.masterTrack.realtimeEffects = [transparentWarmth(), .init(kind: .limiter, enabled: false)]
        let expected = Mixer.render(session: session, assets: assets, range: 0..<14000)
        let mixer = RealtimeMixer(session: session, assets: assets)
        var l: [Float] = []
        var r: [Float] = []
        for size in [1, 127, 500, 8192, 5180] {
            let audio = render(mixer, count: size)
            l += audio.channels[0]
            r += audio.channels[1]
        }
        assertSamples(AudioSamples(channels: [l, r]), expected, accuracy: 0)
        let interactive = RealtimeMixer(session: session, assets: assets, startFrame: 12000, seekPolicy: .interactive)
        assertSamples(render(interactive, count: 512), expected.slice(12000..<12512), accuracy: 0)
        XCTAssertEqual(interactive.seekWarmupFrames, RealtimeSeekPolicy.interactiveHistoryFrames)
    }
    func testAddingAndRemovingLatencyMasterCrossfadesOnAudibleTimeline() throws {
        var (session, assets) = fixture(count: 16000)
        session.tracks[0].realtimeEffects = [transparentWarmth()]
        let mixer = RealtimeMixer(session: session, assets: assets)
        _ = render(mixer, count: 613)
        for active in [true, false, true, false] {
            let previous = session
            session.masterTrack.realtimeEffects = active ? [transparentWarmth()] : []
            XCTAssertTrue(RealtimeMixer.requiresAlignedUpdate(from: previous, to: session))
            let prepared = try XCTUnwrap(
                RealtimeMixer.prepareAlignedUpdate(
                    session: session, assets: assets,
                    previousSession: previous, handoffFrame: mixer.position))
            XCTAssertTrue(mixer.applyPreparedUpdate(prepared, smoothManualChanges: true))
            prepared.releaseRetiredResources()
            let start = mixer.position
            assertSamples(
                render(mixer, count: 701),
                Mixer.render(session: previous, assets: assets, range: start..<(start + 701)), accuracy: 0.0000001)
            mixer.releaseCompletedGraphResources()
        }
    }
    func testMasterMuteSilencesExistingEffectTailAndRampsLiveOutput() {
        var (session, assets) = fixture(count: 12000)
        session.masterTrack.realtimeEffects = [.init(kind: .room)]
        let mixer = RealtimeMixer(session: session, assets: assets)
        _ = render(mixer, count: 3000)
        session.masterTrack.muted = true
        let prepared = RealtimeMixer.prepareUpdate(session: session, assets: assets, realtimeStartFrame: mixer.position)
        XCTAssertTrue(mixer.applyPreparedUpdate(prepared, smoothManualChanges: true))
        prepared.releaseRetiredResources()
        let muted = render(mixer, count: 1000)
        XCTAssertGreaterThan(muted.channels[0].prefix(100).map(abs).max()!, 0.001)
        XCTAssertTrue(
            muted.channels.allSatisfy { $0.suffix(520).allSatisfy { $0 == 0 } },
            "Mute must silence wet tails as well as fresh input")
    }

    func testTrackPitchFeedbackIncludesMasterDelayAndSurvivesCacheSeek() throws {
        var (session, assets) = fixture(count: 32000)
        let id = assets.keys.first!
        assets[id] = AudioSamples(channels: [
            (0..<32000).map { Float(sin(2 * .pi * 450 * Double($0) / sessionRate) * 0.2) }
        ])
        let pitch = RealtimeEffect(kind: .pitchCorrection)
        session.tracks[0].realtimeEffects = [pitch, transparentWarmth()]
        session.masterTrack.realtimeEffects = [transparentWarmth()]
        let mixer = RealtimeMixer(session: session, assets: assets)
        _ = render(mixer, count: 12000)
        let feedback = try XCTUnwrap(mixer.effectMeter(id: pitch.id)?.snapshot().pitch)
        XCTAssertEqual(feedback.sourceFrame, mixer.position - 1)
        XCTAssertEqual(feedback.inputFrequency, 450, accuracy: 0.5)
        let cache = DSPSeekCache(intervalFrames: 4096, maximumBytes: 12 * 1024 * 1024)
        _ = RealtimeMixer(session: session, assets: assets, startFrame: 24000, seekCache: cache)
        let cached = RealtimeMixer(session: session, assets: assets, startFrame: 18333, seekCache: cache)
        let plain = RealtimeMixer(session: session, assets: assets, startFrame: 18333)
        XCTAssertNotNil(cached.restoredCheckpointFrame)
        assertSamples(render(cached, count: 128), render(plain, count: 128), accuracy: 0)
        XCTAssertEqual(cached.effectMeter(id: pitch.id)?.snapshot().pitch?.sourceFrame, cached.position - 1)
    }
    func testSynchronousMasterLayoutUpdatePreparesHistoryAndUnsafeDirectHandoffIsRejected() {
        var (session, assets) = fixture()
        let mixer = RealtimeMixer(session: session, assets: assets)
        _ = render(mixer, count: 521)
        session.masterTrack.realtimeEffects = [transparentWarmth()]
        let cold = RealtimeMixer.prepareUpdate(session: session, assets: assets, realtimeStartFrame: mixer.position)
        XCTAssertFalse(mixer.applyPreparedUpdate(cold), "A latency rack needs a worker-prepared aligned handoff")
        mixer.update(session: session, assets: assets)
        assertSamples(
            render(mixer, count: 512), Mixer.render(session: session, assets: assets, range: 521..<1033),
            accuracy: 0.0000001)
    }
}
