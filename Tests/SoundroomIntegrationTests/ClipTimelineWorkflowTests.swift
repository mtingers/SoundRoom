import XCTest
@testable import Soundroom
@testable import SoundroomCore

@MainActor final class ClipTimelineWorkflowTests: XCTestCase {
    private func fixture() -> (SessionStore, UUID, Clip, Clip) {
        let store = SessionStore(); let asset = UUID()
        let a = Clip(assetID: asset, name: "Voice", start: 0, length: 48_000)
        let b = Clip(assetID: asset, name: "Voice 2", start: 36_000, offset: 0, length: 48_000)
        store.assets[asset] = AudioSamples(channels: [Array(repeating: 0.5, count: 96_000)])
        store.session.tracks[0].clips = [a, b]; store.selectedTrack = store.session.tracks[0].id
        return (store, store.session.tracks[0].id, a, b)
    }
    func testSnapToSelectionGridAndBothClipEdges() {
        let (store, track, a, _) = fixture()
        store.zoom = 100; store.playhead = 100_000
        store.selection = EditSelection(trackID: track, range: 120_000..<150_000)
        XCTAssertEqual(store.snappedFrame(121_000).frame, 120_000)
        store.timelineEditing.grid = .time; store.timelineEditing.timeDivision = 0.1
        XCTAssertEqual(store.snappedFrame(9_500).frame, 9_600)
        store.timelineEditing.grid = .beat; store.session.timing.tempo = 120; store.timelineEditing.beatDivision = 0.25
        XCTAssertEqual(store.snappedFrame(18_500).frame, 18_000)
        store.timelineEditing.grid = .off; store.selectedClips = [a.id]
        let proposal = store.clipMoveProposal(primary: a.id, to: 71_500)!
        XCTAssertEqual(a.end + proposal.shift, 120_000)
        store.snap = false; XCTAssertEqual(store.snappedFrame(121_000).frame, 121_000)
    }

}
