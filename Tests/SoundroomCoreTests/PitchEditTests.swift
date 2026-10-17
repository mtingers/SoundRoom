import XCTest

@testable import SoundroomCore

final class PitchEditTests: XCTestCase {
    private func voice(count: Int = 96_000) -> AudioSamples {
        AudioSamples(channels: [
            (0..<count).map { frame in
                let phase = 2 * Float.pi * 440 * Float(frame) / 48_000
                return 0.32 * sin(phase) + 0.10 * sin(2 * phase) + 0.04 * sin(3 * phase)
            }
        ])
    }
    private func effect(regions: [PitchEditRegion], scoped: Bool = true) -> RealtimeEffect {
        var parameters = RealtimeEffectKind.pitchCorrection.defaultParameters
        parameters["retune"] = 0
        parameters["humanize"] = 0
        parameters["tolerance"] = 0
        parameters["vibrato"] = 0
        return RealtimeEffect(
            kind: .pitchCorrection, parameters: parameters, pitchEdits: regions, pitchEditsOnly: scoped)
    }
    private func region(_ range: Range<Int>, _ note: Float, curve: AutomationCurve = .linear) -> PitchEditRegion {
        .init(
            range: range,
            points: [
                .init(frame: range.lowerBound, note: note, curve: curve), .init(frame: range.upperBound, note: note),
            ])
    }
    private func render(_ effect: RealtimeEffect, input: AudioSamples, blockSize: Int, startFrame: Int = 0) -> [Float] {
        let processor = EffectProcessor(effect: effect)
        var result: [Float] = []
        var offset = 0
        while offset < input.count + effect.latencyFrames {
            let count = min(blockSize, input.count + effect.latencyFrames - offset)
            var left = (offset..<(offset + count)).map { input.sample(channel: 0, frame: $0) }
            var right = left
            left.withUnsafeMutableBufferPointer { l in
                right.withUnsafeMutableBufferPointer { r in
                    processor.process(
                        left: l.baseAddress!, right: r.baseAddress!, count: count, position: startFrame + offset)
                }
            }
            result.append(contentsOf: left)
            offset += count
        }
        return Array(result[effect.latencyFrames..<(effect.latencyFrames + input.count)])
    }
    func testFractionalTargetMeasuredIndependentlyAndScopeDryIsExact() throws {
        let input = voice()
        let start = 5_000
        let insert = effect(regions: [region(29_000..<77_000, 69.25)])
        let output = insert.process(input, startFrame: start)
        XCTAssertEqual(output.count, input.count)
        for frame in 0..<input.count where !(24_000..<72_000).contains(frame) {
            XCTAssertEqual(output.sample(channel: 0, frame: frame), input.sample(channel: 0, frame: frame))
        }
        let measured = try PitchContourAnalysis.analyze(output).filter {
            (40_000..<64_000).contains($0.frame) && $0.isVoiced
        }
        .map(\.frequency).sorted()
        XCTAssertFalse(measured.isEmpty)
        if !measured.isEmpty { XCTAssertEqual(measured[measured.count / 2], 440 * pow(2, 0.25 / 12), accuracy: 1) }
        XCTAssertTrue(output.channels[0].allSatisfy { $0.isFinite && abs($0) < 1 })
        var zeroStrength = insert
        zeroStrength.parameters["strength"] = 0
        XCTAssertEqual(zeroStrength.process(input, startFrame: start), input)
        var empty = insert
        empty.pitchEdits = []
        XCTAssertEqual(empty.process(input, startFrame: start), input)
    }
    func testRegionReplacementAndSlicingPreserveAllNonlinearDomains() throws {
        for curve in AutomationCurve.allCases {
            let original = PitchEditRegion(
                range: 0..<1000,
                points: [
                    .init(frame: 0, note: 60.25, curve: curve), .init(frame: 1000, note: 72.75),
                ])
            let replaced = PitchEditRegions.replacing([original], with: region(300..<600, 69.125))
            XCTAssertEqual(replaced.map(\.startFrame), [0, 300, 600])
            XCTAssertEqual(replaced.map(\.endFrame), [300, 600, 1000])
            XCTAssertEqual(Set(replaced.map(\.id)).count, 3)
            for frame in 0..<1000 {
                let active = try XCTUnwrap(replaced.first { $0.range.contains(frame) })
                if (300..<600).contains(frame) {
                    XCTAssertEqual(try XCTUnwrap(active.note(at: frame)), 69.125, accuracy: 0.00001)
                } else {
                    XCTAssertEqual(
                        try XCTUnwrap(active.note(at: frame)), try XCTUnwrap(original.note(at: frame)),
                        accuracy: 0.00002)
                }
            }
            let slice = try XCTUnwrap(original.sliced(to: 222..<777))
            for frame in 222..<777 {
                XCTAssertEqual(
                    try XCTUnwrap(slice.note(at: frame)), try XCTUnwrap(original.note(at: frame)), accuracy: 0.00002)
            }
        }
    }
    func testFullScopeSlicePreservesSparseAnalyzedKnotsAndTheirIdentities() throws {
        let source = PitchEditRegion(
            range: 100..<1000,
            points: [
                .init(frame: 228, note: 69.375, curve: .smooth), .init(frame: 740, note: 70.125),
            ])
        XCTAssertEqual(try XCTUnwrap(source.sliced(to: 100..<1000)), source)
        XCTAssertEqual(try XCTUnwrap(source.sliced(to: 0..<2000)), source)
        XCTAssertEqual(PitchEditRegions.slicing([source], to: 0..<2000), [source])
        let copiedID = UUID()
        let copied = try XCTUnwrap(source.sliced(to: 0..<2000, id: copiedID))
        XCTAssertEqual(copied.id, copiedID)
        XCTAssertEqual(copied.points, source.points)
        let partial = try XCTUnwrap(source.sliced(to: 100..<500))
        XCTAssertEqual(partial.points.first?.frame, 100)
        XCTAssertEqual(partial.points.last?.frame, 500)
        for frame in 100..<500 {
            XCTAssertEqual(
                try XCTUnwrap(partial.note(at: frame)), try XCTUnwrap(source.note(at: frame)), accuracy: 0.00002)
        }
    }
    func testPersistenceDefaultsSanitizationAndFractionalPoints() throws {
        let pointID = UUID()
        let source = effect(regions: [
            .init(
                startFrame: -100, endFrame: 1000,
                points: [
                    .init(id: pointID, frame: 0, note: 69.375, curve: .smooth), .init(frame: 1000, note: 70.125),
                ])
        ])
        var original = source
        original.bakedRanges = [.init(startFrame: 12, endFrame: 33)]
        XCTAssertEqual(try JSONDecoder().decode(RealtimeEffect.self, from: JSONEncoder().encode(original)), original)
        let oldJSON = Data("{\"kind\":\"pitchCorrection\",\"parameters\":{\"manualTarget\":69}}".utf8)
        let legacy = try JSONDecoder().decode(RealtimeEffect.self, from: oldJSON)
        XCTAssertEqual(legacy.pitchEdits, [])
        XCTAssertFalse(legacy.pitchEditsOnly)
        XCTAssertEqual(legacy.bakedRanges, [])
        XCTAssertEqual(legacy.parameters["manualTarget"], 69)
        let malformed = PitchEditRegion(
            startFrame: Int.min, endFrame: Int.max,
            points: [
                .init(frame: Int.min, note: .nan, curveStart: .nan, curveEnd: .infinity),
                .init(frame: Int.max, note: .infinity), .init(frame: 100, note: 200), .init(frame: 100, note: -20),
            ])
        XCTAssertEqual(malformed.startFrame, 0)
        XCTAssertLessThan(malformed.endFrame, Int.max / 2)
        XCTAssertEqual(malformed.points.filter { $0.frame == 100 }.map(\.note), [0])
        XCTAssertTrue(malformed.points.allSatisfy { $0.note.isFinite && (0...95).contains($0.note) })
        XCTAssertNil(PitchCorrectionState.clampedManualTarget(Float.nan))
        XCTAssertNil(PitchCorrectionState.clampedManualTarget(Float.infinity))
        XCTAssertEqual(PitchCorrectionState.clampedManualTarget(Float(69.375)), 69.375)
    }
    func testRelocationRoundTripBoundsNonlinearSlicingAndCopiedIDs() throws {
        let source = PitchEditRegion(
            range: 100..<1000,
            points: [
                .init(frame: 100, note: 60.125, curve: .easeIn), .init(frame: 1000, note: 72.875),
            ])
        let shifted = PitchEditRegions.relocated([source], by: 500)
        XCTAssertEqual(PitchEditRegions.relocated(shifted, by: -500), [source])
        let copied = try XCTUnwrap(PitchEditRegions.relocated([source], by: 500, renewIDs: true).first)
        XCTAssertNotEqual(copied.id, source.id)
        XCTAssertTrue(Set(copied.points.map(\.id)).isDisjoint(with: source.points.map(\.id)))
        let clipped = try XCTUnwrap(PitchEditRegions.relocated([source], by: -400).first)
        XCTAssertEqual(clipped.range, 0..<600)
        for frame in 0..<600 {
            XCTAssertEqual(
                try XCTUnwrap(clipped.note(at: frame)), try XCTUnwrap(source.note(at: frame + 400)), accuracy: 0.00002)
        }
        let limit = Int.max / 4
        let nearEnd = PitchEditRegion(
            range: (limit - 1000)..<(limit - 100),
            points: [
                .init(frame: limit - 1000, note: 60, curve: .smooth), .init(frame: limit - 100, note: 72),
            ])
        let upperClip = try XCTUnwrap(PitchEditRegions.relocated([nearEnd], by: 400).first)
        XCTAssertEqual(upperClip.endFrame, limit)
        for frame in (limit - 600)..<limit {
            XCTAssertEqual(
                try XCTUnwrap(upperClip.note(at: frame)), try XCTUnwrap(nearEnd.note(at: frame - 400)),
                accuracy: 0.00002)
        }
        XCTAssertEqual(PitchEditRegions.relocated([source], by: Int.min), [])
        XCTAssertEqual(PitchEditRegions.relocated([source], by: Int.max), [])
        let ranges = [PitchEditRange(startFrame: 100, endFrame: 1000)]
        XCTAssertEqual(PitchEditRegions.relocated(ranges, by: -400), [.init(startFrame: 0, endFrame: 600)])
        XCTAssertEqual(PitchEditRegions.relocatedRanges(ranges, by: Int.max), [])
        XCTAssertEqual(PitchEditRegions.relocatedRanges(ranges, by: Int.min), [])
    }
    func testBlockBoundariesAndTimelineOffsetsProduceIdenticalAudio() {
        let input = voice(count: 20_000)
        let region = PitchEditRegion(
            range: 5_123..<19_777,
            points: [
                .init(frame: 5_123, note: 68.8, curve: .smooth), .init(frame: 12_234, note: 70.125, curve: .hold),
                .init(frame: 19_777, note: 69.5),
            ])
        let insert = effect(regions: [region])
        let reference = render(insert, input: input, blockSize: 8192, startFrame: 111)
        XCTAssertEqual(render(insert, input: input, blockSize: 128, startFrame: 111), reference)
        XCTAssertEqual(render(insert, input: input, blockSize: 997, startFrame: 111), reference)
        XCTAssertEqual(insert.process(input, startFrame: 111).channels[0], reference)
    }
    func testMixerAndOfflineScopedInsertMatchAndWarmRangeRetainsContext() {
        let input = voice(count: 32_000)
        let asset = UUID()
        let insert = effect(regions: [
            PitchEditRegion(
                range: 5001..<25_000,
                points: [
                    .init(frame: 5001, note: 68.75, curve: .smooth),
                    .init(frame: 12_345, note: 70.125, curve: .easeOut),
                    .init(frame: 25_000, note: 69.5),
                ])
        ])
        var session = Session()
        session.tracks[0].clips = [
            Clip(assetID: asset, name: "Voice", start: 0, length: input.count, edgeFadeFrames: 0)
        ]
        session.tracks[0].realtimeEffects = [insert]
        let full = Mixer.render(session: session, assets: [asset: input], range: 0..<input.count)
        XCTAssertEqual(full.channels[0], insert.process(input).channels[0])
        XCTAssertEqual(full.channels[1], full.channels[0])
        for range in [3811..<12_345, 12_888..<28_777] {
            XCTAssertEqual(Mixer.render(session: session, assets: [asset: input], range: range), full.slice(range))
        }
    }
    func testOptInDenseScopedPitchCoreCallbackQualification() throws {
        #if DEBUG
            throw XCTSkip("Dense pitch callback qualification requires an optimized build")
        #else
            guard ProcessInfo.processInfo.environment["SOUNDROOM_PITCH_EDIT_BENCHMARK"] == "1" else {
                throw XCTSkip("Set SOUNDROOM_PITCH_EDIT_BENCHMARK=1 for machine-specific dense pitch qualification")
            }
            let frames = 144_000
            let asset = UUID()
            let input = voice(count: frames)
            let automation = (0..<12_000).map { AutomationPoint(frame: $0 * 12, value: 0.3 + Float($0 % 100) / 250) }
            let edits = (0..<12_000).map {
                PitchEditPoint(frame: $0 * 12, note: 69 + 0.25 * sin(Float($0) / 100), curve: .smooth)
            }
            var session = Session()
            session.tracks = (0..<8).map { index in
                var track = Track(name: "Dense \(index)", color: index)
                track.clips = [
                    Clip(assetID: asset, name: "Harmonic voice", start: 0, length: frames, edgeFadeFrames: 0)
                ]
                track.mixAutomation = ["volume": automation, "pan": automation]
                if index == 0 {
                    var pitch = effect(regions: [.init(range: 0..<frames, points: edits)])
                    pitch.parameterAutomation["strength"] = automation
                    track.realtimeEffects = [pitch]
                } else {
                    var filter = RealtimeEffect(
                        kind: .lowPass, parameters: RealtimeEffectKind.lowPass.defaultParameters)
                    filter.parameterAutomation["frequency"] = automation
                    track.realtimeEffects = [
                        filter,
                        RealtimeEffect(kind: .compressor, parameters: RealtimeEffectKind.compressor.defaultParameters),
                    ]
                }
                return track
            }
            let mixer = RealtimeMixer(session: session, assets: [asset: input])
            var left = [Float](repeating: 0, count: 128)
            var right = left
            var times: [Double] = []
            times.reserveCapacity(1000)
            var peak: Float = 0
            for _ in 0..<1000 {
                let began = ProcessInfo.processInfo.systemUptime
                left.withUnsafeMutableBufferPointer { l in
                    right.withUnsafeMutableBufferPointer { r in
                        mixer.render(intoLeft: l.baseAddress!, intoRight: r.baseAddress!, frameCount: 128)
                    }
                }
                times.append(ProcessInfo.processInfo.systemUptime - began)
                XCTAssertTrue(left.allSatisfy(\.isFinite))
                XCTAssertTrue(right.allSatisfy(\.isFinite))
                for sample in left { peak = max(peak, abs(sample)) }
            }
            XCTAssertGreaterThan(peak, 0.02)
            times.sort()
            let budget = 128 / sessionRate
            print(
                "Dense scoped pitch core qualification: one edited pitch + seven filter/compressor tracks; 288,000 automation + 12,000 fractional pitch points; 1000x128 frames; p95 \(times[950] * 1000)ms, max \(times.last! * 1000)ms, budget \(budget * 1000)ms. This fixture does not qualify arbitrary pitch instance counts."
            )
            XCTAssertLessThan(times[950], budget)
        #endif
    }
    func testPreparedDenseCursorSeeksAndLoopsWithoutCrossingGaps() {
        let regions = (0..<100).map { regionIndex in
            PitchEditRegion(
                range: (regionIndex * 1000)..<(regionIndex * 1000 + 900),
                points: (0..<90).map { index in
                    PitchEditPoint(
                        frame: regionIndex * 1000 + index * 10, note: 60 + Float(index) / 100,
                        curve: index % 2 == 0 ? .smooth : .linear)
                })
        }
        let cursor = PitchEditCursor(regions: regions)
        for frame in [0, 500, 899, 900, 999, 1000, 98_750, 99_000, 99_500, 100_000, 10, 950, 4000, 99_100, 0] {
            let expected = regions.first { $0.range.contains(frame) }?.note(at: frame)
            if let expected {
                XCTAssertEqual(cursor.note(at: frame) ?? -1, expected, accuracy: 0.00001)
            } else {
                XCTAssertNil(cursor.note(at: frame))
            }
        }
        let points = (0..<20_000).map { AutomationPoint(frame: $0 * 2, value: Float($0 % 100) / 100) }
        let automation = AutomationCursor(points: points, baseline: 0)
        for frame in [0, 39_997, 100, 25_000, 0] {
            XCTAssertEqual(
                automation.value(at: frame), AutomationInterpolation.value(points, at: frame, baseline: 0),
                accuracy: 0.000001)
        }
    }
    func testScopedLoopRenderingKeepsTargetsAndDryTimingAcrossBlockSizes() {
        let input = voice(count: 24_000)
        let insert = effect(regions: [
            PitchEditRegion(
                range: 4000..<16_000,
                points: [
                    .init(frame: 4000, note: 69.125, curve: .smooth), .init(frame: 16_000, note: 70.375),
                ])
        ])
        func loopRender(blockSize: Int) -> [Float] {
            let processor = EffectProcessor(effect: insert)
            processor.pitchMeterLoopRange = 0..<input.count
            var output: [Float] = []
            for _ in 0..<3 {
                var frame = 0
                while frame < input.count {
                    let count = min(blockSize, input.count - frame)
                    var left = (frame..<(frame + count)).map { input.sample(channel: 0, frame: $0) }
                    var right = left
                    left.withUnsafeMutableBufferPointer { l in
                        right.withUnsafeMutableBufferPointer { r in
                            processor.process(
                                left: l.baseAddress!, right: r.baseAddress!, count: count, position: frame)
                        }
                    }
                    output.append(contentsOf: left)
                    frame += count
                }
            }
            return output
        }
        let reference = loopRender(blockSize: 8192)
        XCTAssertEqual(loopRender(blockSize: 128), reference)
        XCTAssertEqual(loopRender(blockSize: 997), reference)
        for outputFrame in insert.latencyFrames..<reference.count {
            let sourceFrame = (outputFrame - insert.latencyFrames) % input.count
            if !(4000..<16_000).contains(sourceFrame) {
                XCTAssertEqual(reference[outputFrame], input.sample(channel: 0, frame: sourceFrame))
            }
        }
    }
    func testGenericBakedBypassIsExactForLegacyModernAndLatencyStages() {
        let input = voice(count: 16_000)
        var effects = [
            RealtimeEffect(kind: .gain, amount: 0.8), RealtimeEffect(kind: .echo, amount: 0.6),
            RealtimeEffect(kind: .warmth, amount: 0.7), RealtimeEffect(kind: .gain, parameters: ["gain": 12]),
            RealtimeEffect(kind: .warmth, parameters: RealtimeEffectKind.warmth.defaultParameters),
            effect(regions: [region(0..<16_000, 70.125)]),
        ]
        for index in effects.indices {
            effects[index].bakedRanges = [.init(startFrame: 4321, endFrame: 12_765)]
            let output = effects[index].process(input, startFrame: 1000)
            for frame in 3321..<11_765 {
                XCTAssertEqual(
                    output.sample(channel: 0, frame: frame), input.sample(channel: 0, frame: frame),
                    effects[index].kind.rawValue)
            }
        }
        let baked = PitchBakedRangeCursor(ranges: [
            .init(startFrame: 20, endFrame: 30), .init(startFrame: 0, endFrame: 25),
        ])
        for frame in [0, 15, 29, 30, 10, -1, 20] { XCTAssertEqual(baked.contains(frame), (0..<30).contains(frame)) }
    }
    func testThousandsOfDisjointRegionsPrepareWithoutChangingIdentityOrOverlapPriority() {
        let regions = (0..<5000).map { index in region((index * 100)..<(index * 100 + 50), 60 + Float(index % 12)) }
        let clean = PitchEditRegions.sanitized(Array(regions.reversed()))
        XCTAssertEqual(clean, regions)
        let cursor = PitchEditCursor(regions: clean)
        for index in 0..<5000 {
            XCTAssertEqual(cursor.note(at: index * 100 + 25) ?? -1, 60 + Float(index % 12), accuracy: 0.00001)
            XCTAssertNil(cursor.note(at: index * 100 + 75))
        }
        let first = region(0..<100, 60)
        let second = region(50..<150, 72)
        XCTAssertEqual(
            PitchEditRegions.sanitized([first, second]).first { $0.range.contains(75) }?.note(at: 75) ?? -1, 72,
            accuracy: 0.00001)
        XCTAssertEqual(
            PitchEditRegions.sanitized([second, first]).first { $0.range.contains(75) }?.note(at: 75) ?? -1, 60,
            accuracy: 0.00001)
    }
    func testScopeBoundaryRampsRemainInsideAndContinuousAdjacentTargetsStayWet() {
        let adjacent = PitchEditCursor(regions: [region(1000..<2000, 70.25), region(2000..<3000, 70.25)])
        XCTAssertNil(adjacent.sample(at: 999))
        XCTAssertNil(adjacent.sample(at: 3000))
        XCTAssertEqual(adjacent.sample(at: 1000)?.scopeBlend, 0)
        XCTAssertEqual(adjacent.sample(at: 2999)?.scopeBlend, 0)
        XCTAssertEqual(adjacent.sample(at: 1999)?.scopeBlend, 1)
        XCTAssertEqual(adjacent.sample(at: 2000)?.scopeBlend, 1)
        let discontinuous = PitchEditCursor(regions: [region(1000..<2000, 69), region(2000..<3000, 70)])
        XCTAssertEqual(discontinuous.sample(at: 1999)?.scopeBlend, 0)
        XCTAssertEqual(discontinuous.sample(at: 2000)?.scopeBlend, 0)
        let short = PitchEditCursor(regions: [region(100..<110, 70)])
        XCTAssertLessThan(short.sample(at: 104)?.scopeBlend ?? 1, 0.001)
        let input = voice(count: 28_000)
        let insert = effect(regions: [region(12_000..<20_000, 74.5)])
        let output = insert.process(input)
        for boundary in [12_000, 19_999] {
            XCTAssertEqual(output.sample(channel: 0, frame: boundary), input.sample(channel: 0, frame: boundary))
        }
        // Measure the change introduced by the edit rather than the voice's own
        // waveform slope. Raised-cosine gate derivatives vanish at both boundaries.
        let residual = (0..<input.count).map {
            output.sample(channel: 0, frame: $0) - input.sample(channel: 0, frame: $0)
        }
        for frame in [12_000, 12_001, 12_002, 19_997, 19_998, 19_999, 20_000] {
            XCTAssertLessThan(abs(residual[frame] - residual[frame - 1]), 0.001)
        }
    }
    func testContourResolutionIsConfigurableAndDefaultUnchanged() throws {
        let audio = voice(count: 80_000)
        XCTAssertEqual(
            try PitchContourAnalysis.analyze(audio), try PitchContourAnalysis.analyze(audio, maximumPoints: 1024))
        XCTAssertLessThanOrEqual(try PitchContourAnalysis.analyze(audio, maximumPoints: 10).count, 10)
        XCTAssertGreaterThan(
            try PitchContourAnalysis.analyze(audio, maximumPoints: 256).count,
            try PitchContourAnalysis.analyze(audio, maximumPoints: 10).count)
        XCTAssertLessThanOrEqual(try PitchContourAnalysis.analyze(audio, maximumPoints: Int.min).count, 1)
        XCTAssertEqual(
            try PitchContourAnalysis.analyze(audio, maximumPoints: Int.max),
            try PitchContourAnalysis.analyze(audio, maximumPoints: 65_536))
    }
}
