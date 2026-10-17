import XCTest

@testable import SoundroomCore

final class SourceIsolatorTests: XCTestCase {
    private func tone(_ frame: Int, _ hz: Float) -> Float {
        sin(2 * Float.pi * hz * Float(frame) / Float(sessionRate))
    }
    private func process(_ input: [(Float, Float)], _ controls: [Float]) -> [(Float, Float)] {
        let dsp = SourceIsolatorDSP()
        dsp.configure(controls)
        var output = [(Float, Float)]()
        output.reserveCapacity(input.count + SourceIsolatorDSP.latencyFrames)
        for x in input { output.append(dsp.process(left: x.0, right: x.1)) }
        for _ in 0..<SourceIsolatorDSP.latencyFrames { output.append(dsp.process(left: 0, right: 0)) }
        return Array(output.dropFirst(SourceIsolatorDSP.latencyFrames))
    }
    private func rms(_ x: [(Float, Float)], _ range: Range<Int>) -> Float {
        sqrt(x[range].reduce(Float(0)) { $0 + $1.0 * $1.0 } / Float(range.count))
    }

    func testNeutralControlsPreserveExactDelayedSamples() {
        let input = (0..<8192).map { (tone($0, 711) * 0.4, tone($0, 999) * 0.3) }
        for controls: [Float] in [[4, 0, -55, 20, 20000, 1, 0], [1, 1, -55, 20, 20000, 0, 0]] {
            let output = process(input, controls)
            for i in input.indices {
                XCTAssertEqual(output[i].0, input[i].0)
                XCTAssertEqual(output[i].1, input[i].1)
            }
        }
    }
    func testCenterAndSideComponentsRejectTheOppositeImage() {
        let input = (0..<24000).map { i -> (Float, Float) in
            let center = 0.25 * tone(i, 750)
            let side = 0.2 * tone(i, 2250)
            return (center + side, center - side)
        }
        let center = process(input, [3, 1, -55, 20, 20000, 1, 0])
        let removed = process(input, [3, 1, -55, 20, 20000, 1, 1])
        let sides = process(input, [4, 1, -55, 20, 20000, 1, 0])
        for i in 4096..<20000 {
            XCTAssertEqual(center[i].0, 0.25 * tone(i, 750), accuracy: 0.00005)
            XCTAssertEqual(center[i].0, center[i].1, accuracy: 0.000001)
            XCTAssertEqual(sides[i].0, 0.2 * tone(i, 2250), accuracy: 0.00005)
            XCTAssertEqual(sides[i].0, -sides[i].1, accuracy: 0.000001)
            XCTAssertEqual(center[i].0 + removed[i].0, input[i].0, accuracy: 0.000001)
            XCTAssertEqual(center[i].1 + removed[i].1, input[i].1, accuracy: 0.000001)
        }
    }
    func testNoiseFloorSuppressesNoiseAndPreservesTheStrongTone() {
        var seed: UInt64 = 0x123456789
        var colored = [[Float]](repeating: [0, 0], count: 2)
        func noise(_ channel: Int) -> Float {
            seed ^= seed << 13
            seed ^= seed >> 7
            seed ^= seed << 17
            let white = (Float(seed >> 40) / Float(1 << 24) * 2 - 1) * 0.025
            colored[channel][0] += 0.5 * (white - colored[channel][0])
            colored[channel][1] += 0.5 * (colored[channel][0] - colored[channel][1])
            return colored[channel][1]
        }
        let input = (0..<48000).map { i -> (Float, Float) in
            let music: Float = i < 24000 ? 0.3 * tone(i, 1500) : 0
            return (music + noise(0), music + noise(1))
        }
        let output = process(input, [0, 1, -30, 20, 20000, 1, 0])
        XCTAssertLessThan(rms(output, 30000..<44000), rms(input, 30000..<44000) * 0.25)
        let amplitude = 2 * (6000..<20000).reduce(Float(0)) { $0 + output[$1].0 * tone($1, 1500) } / 14000
        XCTAssertGreaterThan(amplitude, 0.29)
    }
    func testHarmonicAndPercussiveMasksDiscriminateToneAndAttacks() {
        let input = (0..<48000).map { i -> (Float, Float) in
            let value = 0.2 * tone(i, 1500) + (i % 4800 == 2400 ? Float(0.7) : 0)
            return (value, value)
        }
        let harmonic = process(input, [1, 1, -55, 20, 20000, 1, 0])
        let percussive = process(input, [2, 1, -55, 20, 20000, 1, 0])
        XCTAssertGreaterThan(rms(harmonic, 12000..<20000), rms(percussive, 12000..<20000) * 5)
        let harmonicAttack = stride(from: 12000, to: 43000, by: 4800).reduce(Float(0)) { $0 + abs(harmonic[$1].0) }
        let percussiveAttack = stride(from: 12000, to: 43000, by: 4800).reduce(Float(0)) { $0 + abs(percussive[$1].0) }
        XCTAssertGreaterThan(percussiveAttack, harmonicAttack * 2)
    }
    func testDeepCopyOwnsIndependentIdenticalHistoryAndNonfiniteInputRecovers() {
        let a = SourceIsolatorDSP()
        a.configure([1, 0.75, -55, 20, 20000, 1, 0])
        for i in 0..<6000 { _ = a.process(left: tone(i, 333), right: tone(i, 333)) }
        let b = a.deepCopy()
        for i in 6000..<12000 {
            let x = a.process(left: tone(i, 333), right: tone(i, 333))
            let y = b.process(left: tone(i, 333), right: tone(i, 333))
            XCTAssertEqual(x.0, y.0)
            XCTAssertEqual(x.1, y.1)
        }
        a.configure([Float](repeating: .nan, count: 7))
        for i in 0..<10000 {
            let x = a.process(left: i == 0 ? .nan : tone(i, 333), right: i == 0 ? .infinity : tone(i, 333))
            XCTAssertTrue(x.0.isFinite && x.1.isFinite)
        }
        XCTAssertLessThan(a.storageBytes, 256 * 1024)
    }

    func testMeterIgnoresEmptyBinsAndMeasuresStereoComponentRemoval() {
        let dsp = SourceIsolatorDSP()
        dsp.configure([3, 1, -55, 20, 20000, 1, 0])
        for i in 0..<8192 {
            let side = 0.2 * tone(i, 1500)
            _ = dsp.process(left: side, right: -side)
        }
        XCTAssertGreaterThan(dsp.reductionDB, 50)
        for _ in 0..<8192 { _ = dsp.process(left: 0, right: 0) }
        XCTAssertEqual(dsp.reductionDB, 0)
        XCTAssertTrue(dsp.display.bands.allSatisfy { $0 == 0 })
    }

    func testExtremeFiniteInputCannotOverflowOrPoisonLaterSilence() {
        for mode in 0...4 {
            let dsp = SourceIsolatorDSP()
            dsp.configure([Float(mode), 1, -55, 20, 20000, 1, 0])
            for i in 0..<16000 {
                let out = dsp.process(left: i == 0 ? .greatestFiniteMagnitude : 0, right: i == 0 ? 1e20 : 0)
                XCTAssertTrue(out.0.isFinite && out.1.isFinite, "Mode \(mode), frame \(i)")
            }
            let settled = dsp.process(left: 0, right: 0)
            XCTAssertLessThan(abs(settled.0), 1e-12)
            XCTAssertLessThan(abs(settled.1), 1e-12)
            XCTAssertTrue(dsp.display.bands.allSatisfy { $0.isFinite })
        }
    }
}
