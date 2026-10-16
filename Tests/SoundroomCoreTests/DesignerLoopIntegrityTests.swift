import XCTest
@testable import SoundroomCore

final class DesignerLoopIntegrityTests: XCTestCase {
    func testBulkLoopBodyAndTailFoldMatchFullRenderForMonoAndOrganicStereo() {
        let source = SoundDesignPattern(notes: [.init(pitch: 60, startBeat: 0.5, durationBeats: 0.5)], bpm: 120, beats: 1, release: 1.2)
        for p in [source, OrganicPatch.duskStrings.applying(to: source)] {
            let full = p.render(), loop = p.renderLoop()
            let frames = Int(p.beats * 60 / p.bpm * sessionRate)
            XCTAssertEqual(loop.count, frames); XCTAssertEqual(loop.channelCount, full.channelCount)
            for c in 0..<full.channelCount {
                var reference = [Float](repeating: 0, count: frames)
                for f in 0..<full.count { reference[f % frames] += full.channels[c][f] }
                XCTAssertEqual(loop.channels[c], reference)
            }
        }
    }
    func testHistoryRetainsRequiredOrganicProjectVersionAfterCutOrBake() {
        var session = Session(), clip = Clip(assetID: UUID(), name: "Organic", start: 0, length: 100)
        clip.soundDesign = OrganicPatch.feltGlass.applying(to: SoundDesignPattern())
        let old = clip; clip.soundDesign = nil; clip.remember(old, operation: "Apply audio")
        session.tracks[0].clips = [clip]
        XCTAssertEqual(session.requiredProjectVersion, 8)
        clip.previousStates = nil; session.tracks[0].clips = [clip]
        XCTAssertEqual(session.requiredProjectVersion, 5)
    }
}
