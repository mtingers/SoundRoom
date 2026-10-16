import XCTest
import SoundroomCore
import SoundroomRemote
@testable import SoundroomTUI

final class TUIProjectionCacheTests: XCTestCase {
    func testFaderChangesReuseClipProjectionAndPreserveLocalNavigation() {
        var session = Session(); session.tracks = (0..<8).map { index in
            var track = Track(name: "Track \(index)", color: index)
            track.clips = (0..<512).map { Clip(assetID: UUID(), name: "Clip \($0)", start: $0 * 24_000, length: 24_000) }
            return track
        }
        var state = RemoteSnapshot(serverID: UUID(), serverName: "Studio", session: session)
        var cache = TUIProjectionCache(), model = TUIModel()
        model.update(cache.project(state)); _ = model.handle("j"); _ = model.handle("3"); _ = model.handle("l")
        let selected = model.trackID, cursor = model.cursor
        XCTAssertEqual(cache.rebuiltClips, 4096)
        for index in 0..<50 {
            state.session.tracks[0].gain = Float(index) / 50
            state.session.tracks[0].pan = -0.2
            state.revision += 1
            model.update(cache.project(state))
        }
        XCTAssertEqual(cache.rebuiltClips, 4096, "Continuous control changes must not remap thousands of clips")
        XCTAssertEqual(model.trackID, selected); XCTAssertEqual(model.cursor, cursor)
        XCTAssertEqual(model.session.tracks[0].gain, Double(Float(49) / 50))
        XCTAssertEqual(model.session.tracks[1].clips[10].start, 240_000)
        state.session.tracks[0].clips[0].start = 12_000
        model.update(cache.project(state))
        XCTAssertEqual(cache.rebuiltClips, 4608)
        XCTAssertEqual(model.session.tracks[0].clips[0].start, 12_000)
        XCTAssertEqual(model.trackID, selected); XCTAssertEqual(model.cursor, cursor)
    }
    func testTransportIntentPriorityDoesNotWaitBehindFileJobs() {
        for intent in [TUIIntent.stop, .togglePlayback, .play, .record, .startRecording, .seek(48_000), .command("takeover"), .command("cancel"), .command("play-track"), .command("loop")] {
            XCTAssertTrue(intent.isUrgentTransport)
        }
        XCTAssertFalse(TUIIntent.command("export Mix").isUrgentTransport)
        XCTAssertFalse(TUIIntent.command("save Session").isUrgentTransport)
        XCTAssertFalse(TUIIntent.undo.isUrgentTransport)
    }
}
