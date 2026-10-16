import XCTest
import SoundroomCore
import SoundroomRemote
@testable import SoundroomTUI

final class TUIMusicalTimingTests: XCTestCase {
    func testRepeatedBeatNudgesStayOnAbsoluteBoundaries() throws {
        var (model, snapshot) = TUIAdvancedCommandTests().fixture()
        snapshot.session.timing = MusicalTiming(tempo: 123)
        snapshot.session.tracks[0].clips[0].start = 0
        var cache = TUIProjectionCache(); model.update(cache.project(snapshot)); model.timebase = .beats
        model.mode = .clips; model.clipIDs = [snapshot.session.tracks[0].clips[0].id]
        for beat in 1...100 {
            let plan = try TUICommandAdapter.plan(.command("nudge 1"), model: model, snapshot: snapshot)
            guard case .replaceTracks(let tracks, _, _) = plan.command else { return XCTFail("Expected move") }
            XCTAssertEqual(tracks[0].clips[0].start, snapshot.session.timing.frame(atQuarterNote: Double(beat)))
            snapshot.session.tracks[0] = tracks[0]; snapshot.trackRevisions[tracks[0].id, default: 0] += 1; snapshot.revision += 1
            model.update(cache.project(snapshot))
        }
    }
    func testMusicalMotionsArePrivateAndDoNotDrift() {
        var model = TUIModel()
        model.session.timing = MusicalTiming(tempo: 123, beatsPerBar: 7, beatUnit: 8)
        XCTAssertTrue(model.handle("B").isEmpty)
        for _ in 0..<1000 { XCTAssertTrue(model.handle("l").isEmpty) }
        XCTAssertEqual(model.cursor, model.session.timing.frame(atQuarterNote: 1000))
        for _ in 0..<1000 { _ = model.handle("h") }
        XCTAssertEqual(model.cursor, 0); XCTAssertEqual(model.session.frame, 0)
        _ = model.execute("grid bar"); _ = model.handle("l")
        XCTAssertEqual(model.cursor, 81_951)
        XCTAssertEqual(model.coordinateFrame("2.1"), model.cursor)
        XCTAssertEqual(model.execute("seek 2.1"), [.seek(model.cursor)])
    }
    func testTimingCommandsAndBeatMovesRouteToAuthoritativeServer() throws {
        var (model, snapshot) = TUIAdvancedCommandTests().fixture()
        snapshot.session.timing = MusicalTiming(tempo: 123)
        var cache = TUIProjectionCache(); model.update(cache.project(snapshot)); model.timebase = .beats
        let tempo = try TUICommandAdapter.plan(.command("tempo 87.5"), model: model, snapshot: snapshot)
        guard case .editor(let intent) = tempo.command else { return XCTFail("Expected server timing command") }
        XCTAssertEqual(intent.operation, "session.timing"); XCTAssertEqual(intent.numbers["tempo"], 87.5)
        for meter in ["6/3", "4/foo/4", "4//4", "4/", "/4"] {
            XCTAssertThrowsError(try TUICommandAdapter.plan(.command("meter " + meter), model: model, snapshot: snapshot))
        }
        model.mode = .clips; model.clipIDs = Set(model.session.tracks[0].clips.map(\.id))
        let move = try TUICommandAdapter.plan(.command("move 3.1"), model: model, snapshot: snapshot)
        guard case .replaceTracks(let tracks, _, _) = move.command else { return XCTFail("Expected track edit") }
        XCTAssertEqual(tracks[0].clips[0].start, snapshot.session.timing.frame(atQuarterNote: 8))
        XCTAssertEqual(tracks[0].clips[1].start - tracks[0].clips[0].start, 96_000)
        let loop = try TUICommandAdapter.plan(.command("loop 2.1 3.1"), model: model, snapshot: snapshot)
        XCTAssertEqual(loop.command, .setLoop(start: snapshot.session.timing.frame(atQuarterNote: 4), end: snapshot.session.timing.frame(atQuarterNote: 8)))
        let rendered = TUIRenderer.render(model, host: "localhost", width: 120, height: 30)
        XCTAssertTrue(rendered.contains("123")); XCTAssertTrue(rendered.contains("Beats"))
    }
}
