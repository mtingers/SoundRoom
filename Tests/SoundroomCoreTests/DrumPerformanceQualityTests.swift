import XCTest
@testable import SoundroomCore
final class DrumPerformanceQualityTests: XCTestCase {
    func testSnareAndCymbalArticulationsHaveDistinctDynamicsAndSpectra() {
        for kit in DrumKit.allCases where kit.isAcoustic && !kit.isRecorded {
            for voice in [DrumVoice.snare, .ride] {
                let center = DrumSynthesis.render(kit: kit, voice: voice).channels[0]
                for articulation in DrumArticulation.available(for: voice) where articulation != .center {
                    let sound = DrumSynthesis.render(kit: kit, voice: voice, articulation: articulation).channels[0]
                    XCTAssertTrue(sound != center, "\(kit) \(voice) \(articulation) must change the excitation")
                    XCTAssertTrue(sound.allSatisfy { $0.isFinite && abs($0) < 1 })
                    XCTAssertEqual(sound.first, 0); XCTAssertEqual(sound.last, 0)
                }
            }
        }
    }
    func testPerformanceCyclesVaryStrikesAndRemainDeterministicAndLoopable() throws {
        var p = DrumPattern(); p.kit = .jazzMaple; p.apply(.backbeat); p.variationCycles = 4; p.velocityHumanize = 0.04; p.humanize = 0.004
        let audio = p.render()
        XCTAssertEqual(audio.count, Int((p.duration * sessionRate).rounded()) * 4)
        XCTAssertEqual(p.render(), audio)
        XCTAssertEqual(try JSONDecoder().decode(DrumPattern.self, from: JSONEncoder().encode(p)), p)
        let length = audio.count / 4
        XCTAssertNotEqual(Array(audio.channels[0][0..<length]), Array(audio.channels[0][length..<2 * length]))
        XCTAssertLessThan(abs(audio.channels[0].first! - audio.channels[0].last!), 0.01)
        XCTAssertTrue(audio.channels.flatMap { $0 }.allSatisfy { $0.isFinite && abs($0) < 1 })
    }
    func testEachAcousticKitChangesItsResonanceAndEightStrikesAreUnique() {
        for voice in [DrumVoice.kick, .snare, .ride] {
            let reference = DrumSynthesis.render(kit: .acoustic, voice: voice)
            for kit in DrumKit.allCases where kit.isAcoustic && kit != .acoustic && !kit.isRecorded {
                XCTAssertNotEqual(DrumSynthesis.render(kit: kit, voice: voice), reference)
            }
            var strikes: [[Float]] = []
            for rr in 0..<8 {
                let next = Array(DrumSynthesis.render(kit: .jazzMaple, voice: voice, roundRobin: rr).channels[0].prefix(4096))
                XCTAssertFalse(strikes.contains(next)); strikes.append(next)
            }
        }
    }
    func testEightHitLaneStillUsesDifferentStrikesAcrossPerformancePasses() {
        var p = DrumPattern(); p.kit = .jazzMaple; p.variationCycles = 4
        p.steps = stride(from: 0, to: 16, by: 2).map { DrumStep(voice: .closedHat, index: $0) }
        let audio = p.render(), frames = audio.count / 4
        let first = Array(audio.channels[0][0..<frames])
        for pass in 1..<4 { XCTAssertTrue(first != Array(audio.channels[0][pass * frames..<(pass + 1) * frames])) }
    }
}
