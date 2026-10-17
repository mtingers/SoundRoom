import XCTest

@testable import SoundroomCore

final class AlignedGraphUpdateTests: XCTestCase {
    private func fixture() -> (Session, [UUID: AudioSamples]) {
        var session = Session()
        let id = UUID()
        let count = 32_000
        session.tracks[0].clips = [Clip(assetID: id, name: "Signal", start: 0, length: count, edgeFadeFrames: 0)]
        let signal = (0..<count).map { Float(sin(Double($0) * 0.031)) * 0.2 }
        return (session, [id: AudioSamples(channels: [signal])])
    }
    func testInsertionAndRemovalStayAlignedAndSettleToCanonicalOutput() throws {
        for inserting in [true, false] {
            var (old, assets) = fixture()
            var new = old
            var parameters = RealtimeEffectKind.warmth.defaultParameters
            parameters["mix"] = 0
            let warmth = RealtimeEffect(kind: .warmth, parameters: parameters)
            if inserting { new.tracks[0].realtimeEffects = [warmth] } else { old.tracks[0].realtimeEffects = [warmth] }
            let mixer = RealtimeMixer(session: old, assets: assets)
            let prepared = try XCTUnwrap(
                RealtimeMixer.prepareAlignedUpdate(
                    session: new, assets: assets, previousSession: old, handoffFrame: 10_000))
            _ = render(mixer, count: 10_000)
            XCTAssertTrue(mixer.applyPreparedUpdate(prepared, smoothManualChanges: true))
            prepared.releaseRetiredResources()
            let output = render(mixer, count: 2000)
            let reference = render(RealtimeMixer(session: new, assets: assets, startFrame: 10_000), count: 2000)
            for (a, b) in zip(output, reference) { XCTAssertEqual(a, b, accuracy: 0.0000001) }
            mixer.releaseCompletedGraphResources()
        }
    }
    func testEqualTotalLatencyReorderWarmsIndependentProcessorInputs() throws {
        var (old, assets) = fixture()
        var pitchParameters = RealtimeEffectKind.pitchCorrection.defaultParameters
        pitchParameters["mix"] = 0
        let pitch = RealtimeEffect(kind: .pitchCorrection, parameters: pitchParameters)
        let gain = RealtimeEffect(
            kind: .gain, amount: 0.5,
            automation: [.init(frame: 0, value: 0.5, curve: .hold), .init(frame: 10_000, value: 0.25)])
        old.tracks[0].realtimeEffects = [pitch, gain]
        var new = old
        new.tracks[0].realtimeEffects = [gain, pitch]
        XCTAssertTrue(RealtimeMixer.requiresAlignedUpdate(from: old, to: new))
        let prepared = try XCTUnwrap(
            RealtimeMixer.prepareAlignedUpdate(session: new, assets: assets, previousSession: old, handoffFrame: 12_000)
        )
        let mixer = RealtimeMixer(session: old, assets: assets)
        _ = render(mixer, count: 12_000)
        XCTAssertTrue(mixer.applyPreparedUpdate(prepared, smoothManualChanges: true))
        prepared.releaseRetiredResources()
        let output = render(mixer, count: 6000)
        let reference = render(RealtimeMixer(session: new, assets: assets, startFrame: 12_000), count: 6000)
        XCTAssertEqual(Array(output.dropFirst(480)), Array(reference.dropFirst(480)))
    }
    func testWrongHandoffFrameIsRejectedWithoutMutation() throws {
        let (old, assets) = fixture()
        var new = old
        new.tracks[0].realtimeEffects = [
            RealtimeEffect(kind: .warmth, parameters: RealtimeEffectKind.warmth.defaultParameters)
        ]
        let prepared = try XCTUnwrap(
            RealtimeMixer.prepareAlignedUpdate(session: new, assets: assets, previousSession: old, handoffFrame: 1000))
        let mixer = RealtimeMixer(session: old, assets: assets)
        _ = render(mixer, count: 1200)
        XCTAssertFalse(mixer.applyPreparedUpdate(prepared, smoothManualChanges: true))
        XCTAssertEqual(
            render(mixer, count: 500), render(RealtimeMixer(session: old, assets: assets, startFrame: 1200), count: 500)
        )
    }
    func testPreparationCancellationStopsBeforePublication() {
        let (old, assets) = fixture()
        var new = old
        new.tracks[0].realtimeEffects = [RealtimeEffect(kind: .pitchCorrection)]
        var checks = 0
        let prepared = RealtimeMixer.prepareAlignedUpdate(
            session: new, assets: assets, previousSession: old, handoffFrame: 20_000,
            isCancelled: {
                checks += 1
                return checks > 1
            })
        XCTAssertNil(prepared)
        XCTAssertLessThan(checks, 10)
    }
    func testCrossfadeIsIndependentOfBlockPartition() throws {
        let (old, assets) = fixture()
        var new = old
        new.tracks[0].realtimeEffects = [
            RealtimeEffect(kind: .warmth, parameters: RealtimeEffectKind.warmth.defaultParameters)
        ]
        func run(_ blocks: [Int]) throws -> [Float] {
            let mixer = RealtimeMixer(session: old, assets: assets)
            _ = render(mixer, count: 2000)
            let prepared = try XCTUnwrap(
                RealtimeMixer.prepareAlignedUpdate(
                    session: new, assets: assets, previousSession: old, handoffFrame: 2000))
            mixer.applyPreparedUpdate(prepared, smoothManualChanges: true)
            prepared.releaseRetiredResources()
            return blocks.flatMap { render(mixer, count: $0) }
        }
        XCTAssertEqual(try run([2000]), try run([1, 17, 127, 335, 521, 999]))
    }
    func testLatencyGraphRemovalRetainsDelayThroughOldDownstreamProcessor() throws {
        var session = Session()
        let id = UUID()
        session.tracks[0].clips = [Clip(assetID: id, name: "Impulse", start: 0, length: 1000, edgeFadeFrames: 0)]
        var warmth = RealtimeEffectKind.warmth.defaultParameters
        warmth["mix"] = 0
        session.tracks[0].realtimeEffects = [
            RealtimeEffect(kind: .echo, amount: 1), RealtimeEffect(kind: .warmth, parameters: warmth),
        ]
        var impulse = [Float](repeating: 0, count: 1000)
        impulse[0] = 0.5
        let assets = [id: AudioSamples(channels: [impulse])]
        let mixer = RealtimeMixer(session: session, assets: assets)
        var incoming = session
        incoming.tracks[0].realtimeEffects = []
        let prepared = try XCTUnwrap(
            RealtimeMixer.prepareAlignedUpdate(
                session: incoming, assets: assets, previousSession: session, handoffFrame: 1000))
        _ = render(mixer, count: 1000)
        XCTAssertTrue(mixer.applyPreparedUpdate(prepared, smoothManualChanges: true))
        prepared.releaseRetiredResources()
        XCTAssertGreaterThan(mixer.end, 12000)
        let output = render(mixer, count: 11001)
        XCTAssertEqual(output[11000], 0.5 * 0.65, accuracy: 0.000001)
    }
    func testOverlappingRapidLatencyChangesKeepDryGainAndFiniteOutput() throws {
        var (session, assets) = fixture()
        let mixer = RealtimeMixer(session: session, assets: assets)
        _ = render(mixer, count: 1000)
        var warmth = RealtimeEffectKind.warmth.defaultParameters
        warmth["mix"] = 0
        for change in 0..<30 {
            var incoming = session
            incoming.tracks[0].realtimeEffects =
                change.isMultiple(of: 2) ? [RealtimeEffect(kind: .warmth, parameters: warmth)] : []
            let prepared = try XCTUnwrap(
                RealtimeMixer.prepareAlignedUpdate(
                    session: incoming, assets: assets, previousSession: session, handoffFrame: mixer.position))
            XCTAssertTrue(mixer.applyPreparedUpdate(prepared, smoothManualChanges: true))
            prepared.releaseRetiredResources()
            let start = mixer.position
            let output = render(mixer, count: 20)
            let reference = render(RealtimeMixer(session: incoming, assets: assets, startFrame: start), count: 20)
            for (a, b) in zip(output, reference) {
                XCTAssertTrue(a.isFinite)
                XCTAssertEqual(a, b, accuracy: 0.0000002)
            }
            mixer.releaseCompletedGraphResources()
            session = incoming
        }
    }
    func testParallelPitchTailCallbackBenchmark() throws {
        var (session, assets) = fixture()
        let original = session.tracks[0]
        session.tracks = (0..<8).map { _ in
            var track = original
            track.id = UUID()
            track.gain = 0.08
            track.realtimeEffects = [RealtimeEffect(kind: .echo, amount: 0.4), RealtimeEffect(kind: .pitchCorrection)]
            return track
        }
        let mixer = RealtimeMixer(session: session, assets: assets)
        _ = render(mixer, count: 1000)
        let left = UnsafeMutablePointer<Float>.allocate(capacity: 128)
        let right = UnsafeMutablePointer<Float>.allocate(capacity: 128)
        defer {
            left.deallocate()
            right.deallocate()
        }
        var durations: [Double] = []
        durations.reserveCapacity(200)
        func block(_ count: Int = 128) {
            let start = ProcessInfo.processInfo.systemUptime
            mixer.render(intoLeft: left, intoRight: right, frameCount: count)
            durations.append(ProcessInfo.processInfo.systemUptime - start)
            for i in 0..<count { XCTAssertTrue(left[i].isFinite && right[i].isFinite) }
        }
        for _ in 0..<8 {
            var incoming = session
            for i in incoming.tracks.indices {
                incoming.tracks[i].realtimeEffects = [
                    RealtimeEffect(kind: .echo, amount: 0.4), RealtimeEffect(kind: .pitchCorrection),
                ]
            }
            let prepared = try XCTUnwrap(
                RealtimeMixer.prepareAlignedUpdate(
                    session: incoming, assets: assets, previousSession: session, handoffFrame: mixer.position + 480))
            mixer.beginAlignedRetirementFade(prepared)
            for _ in 0..<3 { block() }
            block(96)
            XCTAssertFalse(mixer.hasAlignedRetirements(prepared))
            XCTAssertTrue(mixer.applyPreparedUpdate(prepared, smoothManualChanges: true))
            prepared.releaseRetiredResources()
            for _ in 0..<4 { block() }
            mixer.releaseCompletedGraphResources()
            session = incoming
        }
        for _ in 0..<100 { block() }
        durations.sort()
        print(
            "Aligned graph burst stress: 8 tracks × Pitch/Echo, eight successive superseding edits, maximum two DSP graphs per track, \(durations.count) callbacks including every pre-handoff tail fade and crossfade; p95 \(durations[Int(Double(durations.count) * 0.95)] * 1000) ms, max \(durations.last! * 1000) ms; 128-frame budget \(128 / sessionRate * 1000) ms"
        )
        if ProcessInfo.processInfo.environment["SOUNDROOM_STRICT_REALTIME"] == "1" {
            XCTAssertLessThan(durations.last!, 128 / sessionRate)
        }
        mixer.releaseCompletedGraphResources()
    }
    private func render(_ mixer: RealtimeMixer, count: Int) -> [Float] {
        var left = [Float](repeating: 0, count: count)
        var right = left
        left.withUnsafeMutableBufferPointer { l in
            right.withUnsafeMutableBufferPointer { r in
                mixer.render(intoLeft: l.baseAddress!, intoRight: r.baseAddress!, frameCount: count)
            }
        }
        return left
    }
}
