import SoundroomCore
import XCTest

@testable import Soundroom

final class FeatureWorkflowTests: XCTestCase {
    func testProjectPackageRetainsInstrumentNotesAndAutomation() throws {
        let destination = FileManager.default.temporaryDirectory.appendingPathComponent(
            "\(UUID().uuidString).soundroom")
        defer { try? FileManager.default.removeItem(at: destination) }
        var document = Session()
        var effect = RealtimeEffect(kind: .echo)
        effect.parameterAutomation["mix"] = [.init(frame: 20, value: 0.2), .init(frame: 100, value: 0.8)]
        let asset = UUID()
        let audio = AudioSamples(channels: [Array(repeating: Float(0.1), count: 1000)])
        var clip = Clip(assetID: asset, name: "Keys", start: 10, length: 1000)
        clip.soundDesign = SoundDesignPattern(notes: [.init(pitch: 60, startBeat: 0, durationBeats: 0.125)])
        document.tracks[0].clips = [clip]
        document.tracks[0].realtimeEffects = [effect]
        document.version = document.requiredProjectVersion
        _ = try ProjectPersistence.saveSynchronously(session: document, assets: [asset: audio], to: destination)
        let loaded = try ProjectPersistence.loadSynchronously(from: destination)
        XCTAssertEqual(loaded.session, document)
        XCTAssertEqual(loaded.assets[asset], audio)
    }
    func testVersionOneProjectReopensWithoutTimingChanges() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(
            "\(UUID().uuidString).soundroom", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let audioDirectory = directory.appendingPathComponent("Audio", isDirectory: true)
        try FileManager.default.createDirectory(at: audioDirectory, withIntermediateDirectories: true)
        var session = Session()
        session.version = 1
        let asset = UUID()
        let samples = AudioSamples(channels: [[0.1, 0.2, 0.3, 0.4, 0.5]])
        session.tracks[0].clips = [
            Clip(assetID: asset, name: "Legacy take", start: 9, offset: 1, length: 3, edgeFadeFrames: 0)
        ]
        var json = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(session)) as? [String: Any])
        var tracks = try XCTUnwrap(json["tracks"] as? [[String: Any]])
        tracks[0].removeValue(forKey: "realtimeEffects")
        json["tracks"] = tracks
        try JSONSerialization.data(withJSONObject: json).write(to: directory.appendingPathComponent("session.json"))
        try WaveFile.encode(samples, float: true).write(
            to: audioDirectory.appendingPathComponent("\(asset.uuidString).wav"))
        let loaded = try ProjectPersistence.loadSynchronously(from: directory)
        XCTAssertEqual(loaded.session, session)
        XCTAssertEqual(loaded.assets[asset], samples)
        XCTAssertEqual(
            Mixer.render(session: loaded.session, assets: loaded.assets, range: 8..<13).channels[0],
            [0, 0.2, 0.3, 0.4, 0])
    }
}
