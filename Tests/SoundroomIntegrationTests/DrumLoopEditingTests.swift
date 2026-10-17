import SoundroomCore
import XCTest

@testable import Soundroom

@MainActor final class DrumLoopEditingTests: XCTestCase {
    private func beat() -> (DrumPattern, Clip) {
        var pattern = DrumPattern()
        pattern.apply(.backbeat)
        pattern.bpm = 120
        var clip = Clip(assetID: UUID(), name: "Verse beat", start: 4_800, length: pattern.renderedLoopFrames)
        clip.drumPattern = pattern
        return (pattern, clip)
    }
    func testAddingBarsRepeatsEveryHitAndItsExpressionWithoutLinkingTheCopies() throws {
        var (pattern, _) = beat()
        pattern.steps[0].pan = -0.3
        pattern.steps[0].probability = 0.7
        pattern.steps[0].articulation = .center
        let one = pattern.steps
        for bars in [2, 4, 8] {
            pattern.resizeBars(bars)
            XCTAssertEqual(pattern.steps.count, one.count * bars)
            for bar in 0..<bars {
                let hits = pattern.steps.filter { $0.index / 16 == bar }.map { hit in
                    var hit = hit
                    hit.index %= 16
                    return hit
                }
                XCTAssertEqual(hits, one)
            }
            try DrumEditorModel.validate(pattern)
        }
        pattern.steps[0].velocity = 0.1
        XCTAssertEqual(
            pattern.steps.first { $0.voice == one[0].voice && $0.index == one[0].index + 16 }?.velocity, one[0].velocity
        )
        pattern.resizeBars(1)
        XCTAssertEqual(pattern.steps.count, one.count)
        XCTAssertEqual(pattern.renderedLoopFrames, 96_000)
    }
    func testTrimSplitRangeCopyAndDeleteKeepTheEditableSource() throws {
        let (pattern, clip) = beat()
        let store = SessionStore(headless: true)
        store.assets[clip.assetID] = AudioSamples(channels: [Array(repeating: 0.1, count: clip.length)])
        store.session.tracks[0].clips = [clip]
        let track = store.session.tracks[0].id
        store.trimClip(clip.id, trackID: track, start: clip.start + 24_000, end: clip.end - 24_000)
        XCTAssertEqual(store.currentClip?.drumPattern, pattern)
        XCTAssertEqual(store.currentClip?.offset, 24_000)
        XCTAssertEqual(store.currentClip?.length, 48_000)
        store.copy()
        store.playhead = clip.end
        store.paste()
        let pasted = store.session.tracks[0].clips.last!
        XCTAssertEqual(pasted.drumPattern, pattern)
        XCTAssertNotEqual(pasted.id, clip.id)
        try attachClientFixture(store)
        store.toggleInlineDrums(trackID: track)
        XCTAssertEqual(store.inlineDrums?.clipID, pasted.id, "Edit beats must target the pasted selection")
        let parts = store.session.fragments(trackID: track, range: (clip.start + 30_000)..<(clip.start + 40_000))
        XCTAssertEqual(parts.first?.drumPattern, pattern)
        XCTAssertEqual(parts.first?.offset, 30_000)
        var session = Session()
        session.tracks[0].clips = [clip]
        session.split(trackID: session.tracks[0].id, clipID: clip.id, at: clip.start + 48_000)
        XCTAssertTrue(session.tracks[0].clips.allSatisfy { $0.drumPattern == pattern })
        session.delete(trackID: session.tracks[0].id, range: (clip.start + 12_000)..<(clip.start + 24_000))
        XCTAssertTrue(session.tracks[0].clips.allSatisfy { $0.drumPattern == pattern })
    }
    func testEditingASliceKeepsItsBoundsFadesAndIdentity() {
        var (pattern, clip) = beat()
        clip.offset = 24_000
        clip.length = 48_000
        clip.fadeInFrames = 1_000
        pattern.steps[0].velocity = 0.4
        let edited = clip.replacingDrumRender(assetID: UUID(), pattern: pattern, frameCount: pattern.renderedLoopFrames)
        XCTAssertEqual(edited.start, clip.start)
        XCTAssertEqual(edited.id, clip.id)
        XCTAssertEqual(edited.name, clip.name)
        XCTAssertEqual(edited.offset, 24_000)
        XCTAssertEqual(edited.length, 48_000)
        XCTAssertEqual(edited.fadeInFrames, 1_000)
        XCTAssertEqual(edited.drumPattern, pattern)
        XCTAssertNotEqual(edited.assetID, clip.assetID)
    }
    func testWholeLoopsFollowLengthAndOldTailDisappearsOnTheNextEdit() {
        var (pattern, clip) = beat()
        clip.length += 153_600  // Older Rust commits appended 3.2 seconds.
        pattern.resizeBars(2)
        let edited = clip.replacingDrumRender(assetID: UUID(), pattern: pattern, frameCount: pattern.renderedLoopFrames)
        XCTAssertEqual(edited.length, 192_000)
        XCTAssertEqual(edited.offset, 0)
        pattern.variationCycles = 4
        XCTAssertEqual(pattern.renderedLoopFrames, 768_000)
    }
    func testLegacyTrimRecoversOnlyTheUnchangedAudioRangeFromHistory() throws {
        let (pattern, source) = beat()
        var oldTrim = source
        oldTrim.offset = 24_000
        oldTrim.length = 48_000
        oldTrim.drumPattern = nil
        oldTrim.remember(source, operation: "Trim clip")
        XCTAssertEqual(oldTrim.editableDrumPattern, pattern)
        let store = SessionStore(headless: true)
        store.session.tracks[0].clips = [oldTrim]
        try attachClientFixture(store)
        store.toggleInlineDrums(trackID: store.session.tracks[0].id, clipID: oldTrim.id)
        XCTAssertEqual(store.inlineDrums?.pattern, pattern)
        oldTrim.assetID = UUID()
        XCTAssertNil(oldTrim.editableDrumPattern, "Processed audio must never resurrect an unrelated pattern")
        oldTrim.assetID = source.assetID
        oldTrim.length = source.length
        XCTAssertNil(oldTrim.editableDrumPattern, "A history range must contain the whole current window")
    }
}
