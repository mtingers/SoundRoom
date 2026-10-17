import SoundroomCore
import XCTest

@testable import Soundroom

final class EqualizerEditingTests: XCTestCase {
    func testGraphMapsLogFrequencyAndClampsBothAxesToBandLimits() {
        let size = CGSize(width: 300, height: 150)
        let center = EqualizerBand.mid.edits(at: CGPoint(x: 150, y: 75), size: size)
        XCTAssertEqual(center["midFrequency"]!, Float(sqrt(20 * 20_000.0)), accuracy: 0.001)
        XCTAssertEqual(center["midGain"], 0)
        let outside = EqualizerBand.low.edits(at: CGPoint(x: 600, y: -150), size: size)
        XCTAssertEqual(outside["lowFrequency"], 1000)
        XCTAssertEqual(outside["lowGain"], 18)
        let cut = EqualizerBand.highPass.edits(at: CGPoint(x: -100, y: 200), size: size)
        XCTAssertEqual(cut, ["highPassFrequency": 20])
        let old = EqualizerBand.low.point(values: ["lowGain": 0], size: size)
        XCTAssertEqual(EqualizerBand.low.edits(at: old, size: size)["lowFrequency"]!, 100, accuracy: 0.0001)
    }
}
