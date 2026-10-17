import XCTest

@testable import SoundroomCore

final class TrackAutomationTests: XCTestCase {
    private func fixture() -> (Session, [UUID: AudioSamples]) {
        var session = Session()
        let id = UUID()
        session.tracks[0].clips = [Clip(assetID: id, name: "Constant", start: 0, length: 20, edgeFadeFrames: 0)]
        return (session, [id: AudioSamples(channels: [Array(repeating: 1, count: 20)])])
    }
    func testVolumeFadeReachesSilenceAndPanMovesBetweenChannels() {
        var (session, assets) = fixture()
        session.tracks[0].mixAutomation = [
            "volume": [.init(frame: 0, value: 2 / 3), .init(frame: 10, value: 0)],
            "pan": [.init(frame: 0, value: 0), .init(frame: 10, value: 1)],
        ]
        let audio = Mixer.render(session: session, assets: assets, range: 0..<20)
        for i in 0..<20 {
            let gain = max(0, 1 - Float(i) / 10)
            let pan = min(1, Float(i) / 5 - 1)
            XCTAssertEqual(audio.channels[0][i], gain * (pan > 0 ? 1 - pan : 1), accuracy: 0.000001)
            XCTAssertEqual(audio.channels[1][i], gain * (pan < 0 ? 1 + pan : 1), accuracy: 0.000001)
        }
    }
    func testAutomationRunsAfterEffectsAndBlockSeekLoopResultsMatch() {
        var (session, assets) = fixture()
        session.tracks[0].mixAutomation = [
            "volume": [.init(frame: 0, value: 1 / 3), .init(frame: 19, value: 0)],
            "pan": [.init(frame: 0, value: 0), .init(frame: 19, value: 1)],
        ]
        session.tracks[0].realtimeEffects = [RealtimeEffect(kind: .gain, parameters: ["gain": 6.0206])]
        let full = Mixer.render(session: session, assets: assets, range: 0..<20)
        XCTAssertEqual(full.channels[0][0], 1, accuracy: 0.00001)
        let mixer = RealtimeMixer(session: session, assets: assets)
        var left = [Float](repeating: 0, count: 20)
        var right = left
        for range in [0..<3, 3..<7, 7..<20] {
            left.withUnsafeMutableBufferPointer { l in
                right.withUnsafeMutableBufferPointer { r in
                    mixer.render(
                        intoLeft: l.baseAddress! + range.lowerBound, intoRight: r.baseAddress! + range.lowerBound,
                        frameCount: range.count)
                }
            }
        }
        XCTAssertEqual(left, full.channels[0])
        XCTAssertEqual(right, full.channels[1])
        XCTAssertEqual(Mixer.render(session: session, assets: assets, range: 7..<20).channels[0], Array(left[7..<20]))
        mixer.rewindForLoop(to: 0)
        left.withUnsafeMutableBufferPointer { l in
            right.withUnsafeMutableBufferPointer { r in
                mixer.render(intoLeft: l.baseAddress!, intoRight: r.baseAddress!, frameCount: 20)
            }
        }
        XCTAssertEqual(left, full.channels[0])
        XCTAssertEqual(right, full.channels[1])
    }
    func testPreparedUpdateChangesCurveAtCurrentPosition() {
        var (session, assets) = fixture()
        let mixer = RealtimeMixer(session: session, assets: assets, startFrame: 5)
        session.tracks[0].mixAutomation["volume"] = [.init(frame: 0, value: 2 / 3), .init(frame: 10, value: 0)]
        let prepared = RealtimeMixer.prepareUpdate(session: session, assets: assets)
        XCTAssertTrue(mixer.applyPreparedUpdate(prepared, smoothManualChanges: true))
        var left = [Float](repeating: 0, count: 3)
        var right = left
        left.withUnsafeMutableBufferPointer { l in
            right.withUnsafeMutableBufferPointer { r in
                mixer.render(intoLeft: l.baseAddress!, intoRight: r.baseAddress!, frameCount: 3)
            }
        }
        for i in 0..<3 { XCTAssertEqual(left[i], 0.5 - Float(i) / 10, accuracy: 0.000001) }
        prepared.releaseRetiredResources()
    }
    func testRoundTripAndOldProjectDefaults() throws {
        var (session, _) = fixture()
        session.tracks[0].mixAutomation["pan"] = [.init(frame: 0, value: 0.2), .init(frame: 5, value: 0.7)]
        let data = try JSONEncoder().encode(session)
        XCTAssertEqual(try JSONDecoder().decode(Session.self, from: data), session)
        var json = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        var tracks = try XCTUnwrap(json["tracks"] as? [[String: Any]])
        tracks[0].removeValue(forKey: "mixAutomation")
        json["tracks"] = tracks
        json["version"] = 3
        let old = try JSONDecoder().decode(Session.self, from: JSONSerialization.data(withJSONObject: json))
        XCTAssertTrue(old.tracks[0].mixAutomation.isEmpty)
    }
}
