import XCTest

@testable import SoundroomTUI

final class TUIModelTests: XCTestCase {
    func fixture() -> TUIModel {
        var m = TUIModel()
        m.update(
            TUISession(
                name: "Shared",
                tracks: [
                    TUITrack(
                        id: UUID(), name: "Voice",
                        clips: [
                            TUIClip(id: UUID(), name: "A", start: 48_000, length: 48_000),
                            TUIClip(id: UUID(), name: "B", start: 192_000, length: 48_000),
                        ]), TUITrack(id: UUID(), name: "Drums"),
                ], revision: 4))
        return m
    }
    func testMotionsNeverSeekSharedTransport() {
        var a = fixture()
        var b = a
        for key in "2j10l" { XCTAssertTrue(a.handle(key).isEmpty) }
        XCTAssertEqual(a.trackIndex, 1)
        XCTAssertEqual(a.cursor, 120_000)
        XCTAssertEqual(b.trackIndex, 0)
        XCTAssertEqual(b.cursor, 0)
        XCTAssertEqual(a.session.frame, b.session.frame)
        XCTAssertEqual(a.handle("\r"), [.seek(120_000)])
        b.update(a.session)
        XCTAssertEqual(b.cursor, 0)
        XCTAssertEqual(b.trackIndex, 0)
    }
    func testBoundaryAndStartEndMotions() {
        var m = fixture()
        XCTAssertTrue(m.handle("w").isEmpty)
        XCTAssertEqual(m.cursor, 48_000)
        _ = m.handle("2")
        _ = m.handle("w")
        XCTAssertEqual(m.cursor, 192_000)
        _ = m.handle("b")
        XCTAssertEqual(m.cursor, 96_000)
        _ = m.handle("g")
        _ = m.handle("g")
        XCTAssertEqual(m.cursor, 0)
        _ = m.handle("G")
        XCTAssertEqual(m.cursor, 240_000)
    }
    func testClipboardIsPrivateAndPasteNamesExplicitDestination() {
        var m = fixture()
        let source = m.trackID!
        _ = m.handle("w")
        _ = m.handle("y")
        let copied = m.clipboard!.clips
        _ = m.handle("j")
        let destination = m.trackID!
        XCTAssertEqual(
            m.handle("p"),
            [.paste(sourceTrackID: source, clipIDs: copied, destinationTrackID: destination, frame: 48_000)])
        var another = TUIModel()
        another.update(m.session)
        XCTAssertNil(another.clipboard)
    }
    func testVisualRangeSelectsIntersectingClipsAndDDNeverTrack() {
        var m = fixture()
        _ = m.handle("v")
        _ = m.handle("G")
        XCTAssertEqual(m.selectedClips.count, 2)
        _ = m.handle("d")
        XCTAssertEqual(m.handle("d"), [.delete(trackID: m.trackID!, clipIDs: m.selectedClips.map(\.id))])
    }
    func testDeletedServerTrackAndClipInvalidateOnlySelection() {
        var m = fixture()
        _ = m.handle("w")
        _ = m.handle("V")
        var snapshot = m.session
        snapshot.tracks[0].clips = []
        m.update(snapshot)
        XCTAssertTrue(m.clipIDs.isEmpty)
        XCTAssertEqual(m.cursor, 48_000)
        snapshot.tracks.removeFirst()
        m.update(snapshot)
        XCTAssertEqual(m.trackID, snapshot.tracks[0].id)
    }
    func testCommandModeAndCompletionDoNotConsumeEditingCharactersAsMotions() {
        var m = fixture()
        _ = m.handle(":")
        for key in "vo" { _ = m.handle(key) }
        _ = m.handle("\t")
        XCTAssertEqual(m.commandText, "volume ")
        for key in "0.8" { _ = m.handle(key) }
        XCTAssertEqual(m.handle("\r"), [.command("volume 0.8")])
        XCTAssertEqual(m.cursor, 0)
        _ = m.handle(":")
        _ = m.handle("j")
        _ = m.handle("\u{1b}")
        XCTAssertEqual(m.trackIndex, 0)
    }
    func testSeekAndZoomValidateBounds() {
        var m = fixture()
        XCTAssertTrue(m.execute("seek nan").isEmpty)
        XCTAssertTrue(m.execute("seek -2").isEmpty)
        XCTAssertEqual(m.execute("seek 1.5"), [.seek(72_000)])
        XCTAssertTrue(m.execute("zoom 2").isEmpty)
        XCTAssertEqual(m.viewSpan, 96_000)
        XCTAssertTrue(m.execute("zoom infinity").isEmpty)
        XCTAssertEqual(m.viewSpan, 96_000)
    }
    func testTransportAndAuthoredUndoIntents() {
        var m = fixture()
        XCTAssertEqual(m.handle(" "), [.togglePlayback])
        XCTAssertEqual(m.handle("r"), [.record])
        XCTAssertEqual(m.handle("u"), [.undo])
        XCTAssertEqual(m.handle("\u{12}"), [.redo])
        XCTAssertEqual(m.handle("q"), [.quit])
    }
    func testRendererRemovesUntrustedTerminalControlSequences() {
        var m = fixture()
        m.session.name = "Bad\u{1b}[2J\nname"
        m.status = "\u{1b}[?1049l"
        let output = TUIRenderer.render(m, host: "localhost", width: 80, height: 24)
        XCTAssertFalse(output.contains("Bad\u{1b}"))
        XCTAssertFalse(output.contains("\u{1b}[?1049l"))
        XCTAssertEqual(output.components(separatedBy: "\r\n").count, 24)
        XCTAssertTrue(output.contains("Enter seeks server"))
    }
}
