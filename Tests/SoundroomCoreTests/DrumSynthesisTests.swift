import XCTest
@testable import SoundroomCore

final class DrumSynthesisTests: XCTestCase {
    func testEveryPackSampleIsFiniteDryHeadroomAndSilentEdges() {
        for kit in DrumKit.allCases {
            for voice in DrumVoice.allCases {
                for velocity in [42.0 / 127, 85.0 / 127, 1] {
                    for rr in 0..<3 {
                        let sound = DrumSynthesis.render(kit: kit, voice: voice, velocity: velocity, roundRobin: rr)
                        let sample = sound.channels[0]
                        XCTAssertEqual(sample.count, Int((voice.duration(for: kit) * sessionRate).rounded()))
                        XCTAssertEqual(sound.channels.count, 1)
                        XCTAssertTrue(sample.allSatisfy { $0.isFinite && abs($0) < 1 }, "\(kit) \(voice) v\(velocity) rr\(rr)")
                        XCTAssertEqual(sample.first, 0); XCTAssertEqual(sample.last, 0)
                        XCTAssertGreaterThan(sample.map(abs).max()!, 0.005)
                        XCTAssertLessThan(sample.suffix(240).map(abs).max()!, 0.001)
                    }
                }
            }
        }
    }
    func testAcousticVelocityLayersChangeTimbralShapeNotJustGainAndRRVary() {
        for voice in DrumVoice.allCases {
            let soft = DrumSynthesis.render(kit: .acoustic, voice: voice, velocity: 42.0 / 127).channels[0]
            let hard = DrumSynthesis.render(kit: .acoustic, voice: voice, velocity: 1).channels[0]
            let robin = DrumSynthesis.render(kit: .acoustic, voice: voice, velocity: 1, roundRobin: 1).channels[0]
            let softPeak = soft.map(abs).max()!, hardPeak = hard.map(abs).max()!, rrPeak = robin.map(abs).max()!
            XCTAssertGreaterThan(hardPeak, softPeak * 2)
            let difference = zip(soft.prefix(12000), hard.prefix(12000)).reduce(0.0) { $0 + pow(Double($1.0 / softPeak - $1.1 / hardPeak), 2) }
            let robinDifference = zip(robin.prefix(12000), hard.prefix(12000)).reduce(0.0) { $0 + pow(Double($1.0 / rrPeak - $1.1 / hardPeak), 2) }
            XCTAssertGreaterThan(sqrt(difference / 12000), 0.002, "\(voice) layer timbre")
            XCTAssertGreaterThan(sqrt(robinDifference / 12000), 0.005, "\(voice) round robin")
        }
    }
    func testDeterministicSilentVelocityAndDistinctKits() {
        for voice in DrumVoice.allCases {
            let acoustic = DrumSynthesis.render(kit: .acoustic, voice: voice)
            XCTAssertEqual(acoustic, DrumSynthesis.render(kit: .acoustic, voice: voice))
            XCTAssertNotEqual(acoustic, DrumSynthesis.render(kit: .electronic, voice: voice))
            XCTAssertTrue(DrumSynthesis.render(kit: .acoustic, voice: voice, velocity: 0).channels[0].allSatisfy { $0 == 0 })
        }
    }
    func testWavePackFormatIs24Bit48000MonoAndKeepsHeadroom() {
        let samples = DrumSynthesis.render(kit: .acoustic, voice: .kick)
        let data = WaveFile.encode(samples)
        func u16(_ at: Int) -> Int { Int(data[at]) | Int(data[at + 1]) << 8 }
        func u32(_ at: Int) -> Int { (0..<4).reduce(0) { $0 | Int(data[at + $1]) << ($1 * 8) } }
        XCTAssertEqual(String(data: data.prefix(4), encoding: .ascii), "RIFF")
        XCTAssertEqual(u16(20), 1); XCTAssertEqual(u16(22), 1); XCTAssertEqual(u32(24), 48000); XCTAssertEqual(u16(34), 24)
        XCTAssertEqual(u32(40), samples.count * 3)
        for index in stride(from: 0, to: samples.count, by: 997) {
            let offset = 44 + index * 3
            var bits = Int32(data[offset]) | Int32(data[offset + 1]) << 8 | Int32(data[offset + 2]) << 16
            if bits & 0x800000 != 0 { bits |= ~0xffffff }
            XCTAssertEqual(Float(bits) / 8_388_607, samples.channels[0][index], accuracy: 0.000001)
        }
    }
    func testSwingGridAndCyclicLongTailRendering() {
        var pattern = DrumPattern(); pattern.bpm = 300; pattern.swing = 0.4
        XCTAssertEqual(pattern.timeForStep(1), 0.07, accuracy: 0.000001); XCTAssertEqual(pattern.timeForStep(2), 0.1, accuracy: 0.000001)
        pattern.steps = [.init(voice: .crash, index: 15, velocity: 0.65), .init(voice: .kick, index: 0)]
        var lane = DrumLane(kit: .acoustic); lane.tuning = -12; pattern.lanes[DrumVoice.crash.rawValue] = lane
        let once = pattern.render(), twice = pattern.render(repeats: 2)
        XCTAssertEqual(once.count, 38400); XCTAssertEqual(twice.count, 76800)
        for channel in 0..<2 {
            XCTAssertEqual(once.channels[channel], Array(twice.channels[channel][0..<once.count]))
            XCTAssertEqual(once.channels[channel], Array(twice.channels[channel][once.count..<twice.count]))
            XCTAssertLessThan(abs(once.channels[channel].last! - once.channels[channel].first!), 0.005)
        }
    }
    func testPerVoiceKitTuningMutePanAndChance() {
        var pattern = DrumPattern(); pattern.steps = [.init(voice: .kick, index: 0, velocity: 0.8, pan: -1)]
        let dry = pattern.render(); XCTAssertTrue(dry.channels[1].allSatisfy { abs($0) < 0.000001 })
        var lane = DrumLane(kit: .electronic); lane.tuning = 7; pattern.lanes["kick"] = lane
        XCTAssertNotEqual(pattern.render(), dry)
        pattern.steps[0].probability = 0; XCTAssertTrue(pattern.render().channels.flatMap { $0 }.allSatisfy { $0 == 0 })
        pattern.steps[0].probability = 1; lane.muted = true; pattern.lanes["kick"] = lane
        XCTAssertTrue(pattern.render().channels.flatMap { $0 }.allSatisfy { $0 == 0 })
    }
    func testAuditionMatchesSoundTuningPanAndGain() {
        var pattern = DrumPattern(); var lane = DrumLane(kit: .electronic)
        lane.tuning = 12; lane.pan = 1; lane.gain = 0.5; pattern.lanes["kick"] = lane
        let audition = pattern.audition(.kick)
        XCTAssertEqual(audition.count, Int(DrumVoice.kick.duration * sessionRate) / 2)
        XCTAssertTrue(audition.channels[0].allSatisfy { abs($0) < 0.000001 })
        XCTAssertGreaterThan(audition.channels[1].map(abs).max()!, 0.1)
        lane.gain = 0; pattern.lanes["kick"] = lane
        XCTAssertTrue(pattern.audition(.kick).channels.flatMap { $0 }.allSatisfy { $0 == 0 })
    }
    func testHatChokeAndPresetPatternsAreLoopableWithoutClipping() {
        var pattern = DrumPattern(); pattern.steps = [.init(voice: .openHat, index: 0)]
        let open = pattern.render()
        pattern.steps.append(.init(voice: .closedHat, index: 1, velocity: 0))
        let choked = pattern.render()
        let after = Int(pattern.timeForStep(1) * sessionRate) + 240
        XCTAssertGreaterThan(open.channels[0][after..<after + 3000].map(abs).max()!, 0.001)
        XCTAssertTrue(choked.channels[0][after...].allSatisfy { $0 == 0 })
        for kit in DrumKit.allCases {
            for preset in DrumPatternPreset.allCases {
                pattern.apply(preset); pattern.kit = kit
                XCTAssertTrue(pattern.render().channels.flatMap { $0 }.allSatisfy { $0.isFinite && abs($0) < 1 })
            }
        }
    }
}
