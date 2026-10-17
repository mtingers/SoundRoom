import XCTest

@testable import SoundroomCore

final class FilterResponseTests: XCTestCase {
    func testResponseMatchesIndependentSteadyToneDSPAtMultipleFrequenciesAndQ() {
        let count = 24_000
        for kind in [RealtimeEffectKind.lowPass, .highPass] {
            let resonances: [Float] = [0.707, 1, 3]
            for q in resonances {
                let parameters: [String: Float] = ["frequency": 1200, "resonance": q]
                for frequency in [200.0, 1000, 8000] {
                    let signal = (0..<count).map {
                        Float(0.2 * sin(2 * Double.pi * frequency * Double($0) / sessionRate))
                    }
                    let output = RealtimeEffect(kind: kind, parameters: parameters).process(
                        AudioSamples(channels: [signal])
                    ).channels[0]
                    let inputPower = signal[(count / 2)...].reduce(0.0) { $0 + Double($1 * $1) }
                    let outputPower = output[(count / 2)...].reduce(0.0) { $0 + Double($1 * $1) }
                    let measured = 10 * log10(outputPower / inputPower)
                    XCTAssertEqual(
                        FilterResponse.decibels(at: frequency, kind: kind, parameters: parameters), measured,
                        accuracy: 0.02, "\(kind) Q\(q) at\(frequency)")
                }
            }
        }
    }
    func testCutoffAndResonanceAndInvalidInputs() {
        for kind in [RealtimeEffectKind.lowPass, .highPass] {
            XCTAssertEqual(
                FilterResponse.decibels(at: 1200, kind: kind, parameters: ["frequency": 1200, "resonance": 0.707]),
                -3.0116, accuracy: 0.01)
            XCTAssertGreaterThan(
                FilterResponse.decibels(at: 1200, kind: kind, parameters: ["frequency": 1200, "resonance": 3]), 9)
            for frequency in [0.0, 24_000, .nan, -.infinity] {
                XCTAssertTrue(
                    FilterResponse.decibels(
                        at: frequency, kind: kind, parameters: ["frequency": .nan, "resonance": .infinity]
                    ).isFinite)
            }
        }
        XCTAssertEqual(FilterResponse.decibels(at: 400, kind: .echo, parameters: [:]), 0)
    }
}
