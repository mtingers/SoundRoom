import SoundroomCore
import XCTest

@testable import Soundroom

final class TrackReorderingTests: XCTestCase {
    func testDragDestinationCountsRowsWhoseMiddleWasPassed() {
        let heights: [Double] = [100, 100, 184, 100]
        XCTAssertEqual(TrackReordering.destination(from: 0, offset: 0, heights: heights), 0)
        XCTAssertEqual(TrackReordering.destination(from: 0, offset: 49, heights: heights), 0)
        XCTAssertEqual(TrackReordering.destination(from: 0, offset: 51, heights: heights), 1)
        // The taller third row (open automation lane) needs a longer drag.
        XCTAssertEqual(TrackReordering.destination(from: 0, offset: 150, heights: heights), 1)
        XCTAssertEqual(TrackReordering.destination(from: 0, offset: 193, heights: heights), 2)
        XCTAssertEqual(TrackReordering.destination(from: 0, offset: 10_000, heights: heights), 3)
        XCTAssertEqual(TrackReordering.destination(from: 3, offset: -10_000, heights: heights), 0)
        XCTAssertEqual(TrackReordering.destination(from: 2, offset: -.infinity, heights: heights), 2)
    }
    func testMarkerSitsBetweenTheRowsTheDropLandsBetween() {
        let heights: [Double] = [100, 100, 184, 100]
        XCTAssertNil(TrackReordering.markerY(from: 1, to: 1, heights: heights))
        XCTAssertEqual(TrackReordering.markerY(from: 2, to: 0, heights: heights), 0)
        XCTAssertEqual(TrackReordering.markerY(from: 3, to: 1, heights: heights), 100)
        XCTAssertEqual(TrackReordering.markerY(from: 0, to: 2, heights: heights), 384)
        XCTAssertEqual(TrackReordering.markerY(from: 0, to: 3, heights: heights), 484)
    }
    @MainActor func testMovingATrackKeepsItsContentAndSelectsIt() {
        let store = SessionStore(headless: true)
        store.session.tracks = ["A", "B", "C"].map { Track(name: $0, color: 0) }
        let a = store.session.tracks[0]
        XCTAssertFalse(store.canMoveTrack(a.id, by: -1))
        store.moveTrack(a.id, by: 1)
        XCTAssertEqual(store.session.tracks.map(\.name), ["B", "A", "C"])
        XCTAssertEqual(store.session.tracks[1], a)
        XCTAssertEqual(store.selectedTrack, a.id)
        store.trackDrag = TrackDrag(id: a.id, offset: store.trackSize.height)
        store.finishTrackDrag()
        XCTAssertEqual(store.session.tracks.map(\.name), ["B", "C", "A"])
        XCTAssertNil(store.trackDrag)
        XCTAssertFalse(store.canMoveTrack(a.id, by: 1))
        store.moveTrack(a.id, to: 0)
        XCTAssertEqual(store.session.tracks.map(\.name), ["A", "B", "C"])
    }
}
