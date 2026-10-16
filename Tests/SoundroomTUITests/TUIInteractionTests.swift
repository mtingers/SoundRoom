import XCTest
import SoundroomCore
import SoundroomRemote
@testable import SoundroomTUI

final class TUIInteractionTests: XCTestCase {
    func fixture() -> TUIModel {
        var model = TUIModel()
        model.update(TUISession(name: "Studio", tracks: [
            TUITrack(id: UUID(), name: "Voice", clips: [TUIClip(id: UUID(), name: "Verse", start: 48_000, length: 48_000), TUIClip(id: UUID(), name: "Chorus", start: 144_000, length: 48_000)]),
            TUITrack(id: UUID(), name: "合唱 🎹 with a very long name", inserts: [RealtimeEffect(kind: .room, parameters: RealtimeEffectKind.room.defaultParameters)])
        ]))
        return model
    }
    func frame(_ model: TUIModel, width: Int = 120, height: Int = 30, color: Bool = false) -> TUIFrame {
        TUIRenderer.frame(model, host: "localhost", width: width, height: height, color: color)
    }
    func click(_ hit: TUIHit, model: inout TUIModel, frame: TUIFrame) -> [TUIIntent] {
        model.handleMouse(.init(action: .down, x: hit.x, y: hit.y), frame: frame)
    }
    func testIncrementalMouseDecoderCannotInjectMotionOrQuitFromCoordinates() {
        var decoder = TerminalInputDecoder()
        XCTAssertEqual(decoder.feed(Array("\u{1b}[<0;".utf8)), [])
        XCTAssertEqual(decoder.feed(Array("11;6M".utf8)), [.mouse(.init(action: .down, x: 10, y: 5))])
        XCTAssertEqual(decoder.feed(Array("\u{1b}[<36;12;6M".utf8)), [.mouse(.init(action: .drag, x: 11, y: 5, shift: true))])
        XCTAssertEqual(decoder.feed(Array("\u{1b}[<0;12;6m".utf8)), [.mouse(.init(action: .up, x: 11, y: 5))])
        XCTAssertEqual(decoder.feed(Array("\u{1b}[<80;12;6M".utf8)), [.mouse(.init(action: .scrollUp, x: 11, y: 5, control: true))])
        XCTAssertEqual(decoder.feed(Array("\u{1b}[<0;0;6M".utf8)), [])
    }
    func testEscapeExpirationAndModifiedArrowsAreBounded() {
        var d = TerminalInputDecoder()
        XCTAssertEqual(d.feed([27]), []); XCTAssertEqual(d.expireEscape(), [.key("\u{1b}")])
        XCTAssertEqual(d.feed(Array("\u{1b}[1;5A".utf8)), [.key("\u{f700}")])
        XCTAssertEqual(d.feed(Array("\u{1b}[123".utf8)), []); XCTAssertEqual(d.expireEscape(), [])
        XCTAssertEqual(d.feed(Array("é".utf8)), [.key("é")])
    }
    func testBracketedPasteCannotRunShortcutCommandsAndIsBounded() {
        var d = TerminalInputDecoder()
        XCTAssertEqual(d.feed(Array("\u{1b}[200~ddq:record\r".utf8)), [])
        XCTAssertEqual(d.feed(Array("\u{1b}[201~".utf8)), [.paste("ddq:record\r")])
        let pasted = d.feed(Array(("\u{1b}[200~" + String(repeating: "x", count: 10000) + "\u{1b}[201~").utf8))
        XCTAssertEqual(pasted, [.paste(String(repeating: "x", count: 4096))])
    }
    func testClickSelectsTrackWithoutChangingSharedAudio() {
        var m = fixture(); let f = frame(m)
        let hit = f.hits.last { $0.action == .track(m.session.tracks[1].id) }!
        XCTAssertTrue(click(hit, model: &m, frame: f).isEmpty)
        XCTAssertEqual(m.trackIndex, 1); XCTAssertEqual(m.session.frame, 0)
        XCTAssertEqual(m.cursor, 0)
    }
    func testClickingFlagTargetsClickedTrackAndShowsExactToggle() {
        var m = fixture(); let f = frame(m)
        XCTAssertEqual(m.trackIndex, 0)
        let hit = f.hits.last { $0.action == .trackCommand(m.session.tracks[1].id, "mute on") }!
        XCTAssertEqual(click(hit, model: &m, frame: f), [.command("mute on")]); XCTAssertEqual(m.trackIndex, 1)
    }
    func testTimelineClickShiftClickAndDragStayLocal() {
        var m = fixture(); let f = frame(m)
        func x(_ seconds: Double) -> Int { f.timelineX + Int(seconds / 10 * Double(f.timelineWidth)) + 1 }
        XCTAssertEqual(m.handleMouse(.init(action: .down, x: x(1), y: 5), frame: f), [])
        XCTAssertEqual(m.clipIDs.count, 1)
        _ = m.handleMouse(.init(action: .up, x: x(1), y: 5), frame: f)
        _ = m.handleMouse(.init(action: .down, x: x(3), y: 5, shift: true), frame: f)
        XCTAssertEqual(m.selectedClips.count, 2)
        _ = m.handleMouse(.init(action: .down, x: x(1), y: 5), frame: f)
        _ = m.handleMouse(.init(action: .drag, x: x(4), y: 5), frame: f)
        XCTAssertEqual(m.mode, .visual); XCTAssertNotNil(m.anchor); XCTAssertEqual(m.session.frame, 0)
    }
    func testMouseWheelNeverMovesSharedCursorOrSelection() {
        var m = fixture(); let f = frame(m), id = m.trackID
        _ = m.handleMouse(.init(action: .scrollDown, x: 40, y: 5, shift: true), frame: f)
        XCTAssertGreaterThan(m.viewStart, 0)
        let span = m.viewSpan
        _ = m.handleMouse(.init(action: .scrollUp, x: 40, y: 5, control: true), frame: f)
        XCTAssertLessThan(m.viewSpan, span); XCTAssertEqual(m.session.frame, 0); XCTAssertEqual(m.trackID, id)
    }
    func testInspectorEffectControlsUsePhysicalUnitsAndExactEntry() {
        var m = fixture(); _ = m.handle("j"); _ = m.handle("f")
        let f = frame(m), p = f.hits.first { $0.action == .parameter(1, "decay") }!
        let intents = m.handleMouse(.init(action: .down, x: p.x + p.width - 1, y: p.y), frame: f)
        XCTAssertEqual(intents, [.command("effect set 1 decay 10.0")])
        XCTAssertEqual(m.handle("\r"), []); XCTAssertEqual(m.mode, .command); XCTAssertEqual(m.commandText, "effect set 1 decay ")
        XCTAssertEqual(m.session.frame, 0)
    }
    func testKeyboardFocusAndEditingShortcuts() {
        var m = fixture(); _ = m.handle("\t")
        XCTAssertTrue(m.inspectorFocused); XCTAssertEqual(m.handle("l"), [.command("volume 1.05")]); XCTAssertEqual(m.cursor, 0)
        _ = m.handle("t"); XCTAssertFalse(m.inspectorFocused)
        XCTAssertEqual(m.handle("m"), [.command("mute")]); XCTAssertEqual(m.handle("x"), [.command("split")]); XCTAssertEqual(m.handle("C"), [.command("combine")]); XCTAssertEqual(m.handle("L"), [.command("loop")])
        _ = m.handle("F"); XCTAssertEqual(m.viewStart, 0); XCTAssertEqual(m.viewSpan, 204_000)
    }
    func testCommandArrowAndHistoryNeverInsertVimMotionLetters() {
        var m = fixture(); _ = m.handle(":"); for key in "mute on" { _ = m.handle(key) }; _ = m.handle("\r")
        _ = m.handle(":"); _ = m.handle("\u{f700}"); XCTAssertEqual(m.commandText, "mute on")
        _ = m.handle("\u{f703}"); XCTAssertEqual(m.commandText, "mute on")
    }
    func testLayoutIsBoundedAndHitCoordinatesMatchWideNames() {
        var m = fixture(); m.status = "\u{1b}[?1002lInjected"; m.session.name = "Bad\u{1b}[2J"
        for size in [(15, 5), (30, 10), (80, 24), (120, 30), (240, 100)] {
            let f = frame(m, width: size.0, height: size.1)
            let rows = f.output.dropFirst(3).components(separatedBy: "\r\n")
            XCTAssertEqual(rows.count, size.1)
            for row in rows { XCTAssertEqual(row.reduce(0) { $0 + TUIRenderer.cellWidth($1) }, size.0) }
            XCTAssertFalse(f.output.contains("\u{1b}[?1002l")); XCTAssertFalse(f.output.contains("Bad\u{1b}"))
            for hit in f.hits { XCTAssertGreaterThanOrEqual(hit.x, 0); XCTAssertLessThanOrEqual(hit.x + hit.width, size.0); XCTAssertLessThan(hit.y, size.1) }
        }
        XCTAssertEqual(TUIRenderer.cellWidth("合"), 2)
    }
    func testColorAndPlainModesHaveSameGeometry() {
        let m = fixture(), plain = frame(m), colored = frame(m, color: true)
        XCTAssertTrue(colored.output.contains("38;5;")); XCTAssertFalse(plain.output.contains("38;5;"))
        XCTAssertEqual(plain.hits.map(\.x), colored.hits.map(\.x))
        let options = try? TUIOptions.parse(["--no-color", "--no-mouse"])
        XCTAssertEqual(options?.color, false); XCTAssertEqual(options?.mouse, false)
    }
}
