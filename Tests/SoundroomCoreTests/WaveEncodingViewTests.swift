import XCTest

@testable import SoundroomCore

final class WaveEncodingViewTests: XCTestCase {
    func testNestedLazyViewEncodesExactResidentParityAcrossBlockBoundaries() throws {
        var first = (0..<20_000).map { Float(sin(Double($0) * 0.013)) * 1.2 }
        first[8192] = .nan
        first[8193] = .infinity
        first[8194] = -.infinity
        let original = AudioSamples(channels: [first, first.map { -$0 }])
        let view = try XCTUnwrap(original.view(range: 31..<19_500, channels: [1, 0], padding: 17))
        let nested = try XCTUnwrap(view.view(range: 5..<18_700, channels: [0, 1], padding: 29))
        let materialized = AudioSamples(channels: nested.channels)
        XCTAssertFalse(nested.isStreamed)  // The formerly quadratic resident-view path.
        for float in [false, true] {
            XCTAssertEqual(WaveFile.encode(nested, float: float), WaveFile.encode(materialized, float: float))
        }
        let decoded = try XCTUnwrap(WaveFile.decodeFloat(WaveFile.encode(nested, float: true)))
        XCTAssertEqual(decoded.samples.count, nested.count)
        XCTAssertTrue(decoded.samples.channels.flatMap { $0 }.allSatisfy(\.isFinite))
    }
    func testSixSecondLazyTakeEncodingCompletesInLinearTime() throws {
        let frames = Int(sessionRate * 6)
        let source = AudioSamples(channels: [(0..<frames).map { Float($0 % 97) / 97 }])
        let take = try XCTUnwrap(source.view(range: 0..<frames, channels: [0], padding: 13))
        let started = ProcessInfo.processInfo.systemUptime
        let encoded = WaveFile.encode(take, float: true)
        let elapsed = ProcessInfo.processInfo.systemUptime - started
        XCTAssertEqual(encoded.count, 44 + take.count * 4)
        XCTAssertLessThan(elapsed, 5, "A short lazy take must not rematerialize all PCM per output sample")
        print("Six-second lazy-take Float32 WAV encode: \(elapsed * 1000) ms")
    }
}
