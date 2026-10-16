import XCTest
@testable import SoundroomCore

final class SoundPuttyTests: XCTestCase {
    private func controls(_ edits: [String: Float] = [:]) -> [Float] {
        let values = RealtimeEffectKind.soundPutty.defaultParameters.merging(edits) { _, new in new }
        return RealtimeEffectKind.soundPutty.parameters.map { $0.clamped(values[$0.id] ?? $0.defaultValue) }
    }
    func testSculptBrushEditsAll300LanesAndSurvivesProjectEncoding() throws {
        let flat = SoundPuttyShape(parameters: [:])
        let edits = flat.brush(axis: .pitch, lane: 299, target: 1, radius: 22)
        XCTAssertEqual(edits["puttyPitch299"]!, 0.3, accuracy: 1e-6)
        XCTAssertGreaterThan(edits["puttyPitch298"]!, 0)
        XCTAssertFalse(SoundPuttyShape.isShapeKey("puttyPitch300")); XCTAssertFalse(SoundPuttyShape.isShapeKey("puttyPitch01"))
        let original = RealtimeEffect(kind: .soundPutty, parameters: RealtimeEffectKind.soundPutty.defaultParameters.merging(edits) { _, new in new })
        let decoded = try JSONDecoder().decode(RealtimeEffect.self, from: JSONEncoder().encode(original))
        XCTAssertEqual(decoded, original)
        let shape = SoundPuttyShape(parameters: decoded.parameters)
        XCTAssertGreaterThan(shape.frequency(lane: 299, parameters: ["intensity": 0]), 560)
        XCTAssertEqual(SoundPuttyShape.resetParameters.count, 900)
        XCTAssertEqual(SoundPuttyShape(parameters: ["puttyEnergy299": .infinity]).rows[1][299], 0)
    }
    func testNoSelfExcitationDryIdentityStereoIndependenceAndExactCopy() {
        let dsp = SoundPuttyDSP(); dsp.configure(controls(["mix": 1]))
        for _ in 0..<512 { let y = dsp.process(left: 0, right: 0); XCTAssertEqual(y.0, 0); XCTAssertEqual(y.1, 0) }
        for i in 0..<1024 { let y = dsp.process(left: i == 0 ? 1 : 0, right: 0); XCTAssertTrue(y.0.isFinite); XCTAssertEqual(y.1, 0) }
        let copy = dsp.deepCopy()
        for _ in 0..<1024 { let a = dsp.process(left: 0, right: 0), b = copy.process(left: 0, right: 0); XCTAssertEqual(a.0, b.0); XCTAssertEqual(a.1, b.1) }
        dsp.configure(controls(["mix": 0, "output": 12]))
        for i in 0..<512 { let x = Float(sin(Double(i) * 0.1)); let y = dsp.process(left: x, right: -x); XCTAssertEqual(y.0, x); XCTAssertEqual(y.1, -x) }
    }
    func testEveryExcitationAndWildcardModeProducesBoundedFiniteAudio() {
        for mode in 0..<4 { for excitation in 0..<3 {
            let dsp = SoundPuttyDSP()
            dsp.setShape(SoundPuttyShape(parameters: SoundPuttyShape.resetParameters.merging(["puttyPitch299": 1, "puttyEnergy299": 1, "puttyDamping299": -1]) { _, new in new }))
            dsp.configure(controls(["wildMode": Float(mode), "excite": Float(excitation), "coupling": 0.3, "start": 800, "end": 1600, "decay": 5, "drive": 24, "mix": 1, "output": 12]))
            for i in 0..<1024 { let y = dsp.process(left: Float(sin(Double(i) * 0.3)) * 4, right: 0); XCTAssertTrue(y.0.isFinite); XCTAssertLessThanOrEqual(abs(y.0), 4); XCTAssertEqual(y.1, 0) }
        } }
    }
    func testActualEffectGraphPreservesLengthDryGainAndTailOptIn() {
        let source = AudioSamples(channels: [(0..<2048).map { Float(sin(Double($0) * 0.03) * 0.2) }])
        let dry = RealtimeEffect(kind: .soundPutty, parameters: ["mix": 0]).process(source)
        XCTAssertEqual(dry.channels[0], source.channels[0]); XCTAssertEqual(dry.count, source.count)
        let effect = RealtimeEffect(kind: .soundPutty, parameters: ["decay": 0.1, "mix": 1])
        let wet = effect.process(source)
        XCTAssertEqual(wet.count, source.count); XCTAssertNotEqual(wet.channels[0], source.channels[0])
        XCTAssertGreaterThan(effect.tailFrames, 0); XCTAssertLessThanOrEqual(effect.tailFrames, 30 * 48000)
        XCTAssertEqual(effect.process(source, includeTail: true).count, source.count + effect.tailFrames)
    }
}
