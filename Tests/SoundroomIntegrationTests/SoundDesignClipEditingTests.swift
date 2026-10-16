import XCTest
import SoundroomCore
@testable import Soundroom

@MainActor final class SoundDesignClipEditingTests: XCTestCase {
    func testTrimCopyAndLegacyRecoveryKeepTheInstrumentOnItsOwnPCM() throws {
        let pattern = SoundDesignPattern(), store = SessionStore(headless: true)
        var clip = Clip(assetID: UUID(), name: "Verse keys", start: 4_800, length: pattern.sequenceFrames)
        clip.soundDesign = pattern
        store.assets[clip.assetID] = AudioSamples(channels: [Array(repeating: 0.1, count: clip.length)])
        store.session.tracks[0].clips = [clip]
        let track = store.session.tracks[0].id
        store.trimClip(clip.id, trackID: track, start: clip.start + 24_000, end: clip.end - 24_000)
        XCTAssertEqual(store.currentClip?.soundDesign, pattern)
        store.copy(); store.playhead = clip.end; store.paste()
        let pasted = store.session.tracks[0].clips.last!
        XCTAssertEqual(pasted.soundDesign, pattern); XCTAssertNotEqual(pasted.id, clip.id)
        try attachClientFixture(store)
        store.toggleInlineSound(trackID: track)
        XCTAssertEqual(store.inlineSound?.clipID, pasted.id)
        var oldTrim = pasted; oldTrim.soundDesign = nil; oldTrim.remember(clip, operation: "Trim clip")
        XCTAssertEqual(oldTrim.editableSoundDesign, pattern)
        oldTrim.assetID = UUID(); XCTAssertNil(oldTrim.editableSoundDesign)
    }
    func testReplacingASlicePreservesNameBoundsFadesAndCopyIndependence() {
        var pattern = SoundDesignPattern()
        var clip = Clip(assetID: UUID(), name: "Trimmed keys", start: 5_000, offset: 24_000, length: 48_000)
        clip.soundDesign = pattern; clip.fadeInFrames = 1_000
        let original = clip
        pattern.notes[0].velocity = 0.2
        let edited = clip.replacingSoundDesignRender(assetID: UUID(), pattern: pattern, frameCount: 200_000)
        XCTAssertEqual(edited.id, clip.id); XCTAssertEqual(edited.name, clip.name)
        XCTAssertEqual(edited.start, clip.start); XCTAssertEqual(edited.offset, 24_000); XCTAssertEqual(edited.length, 48_000)
        XCTAssertEqual(edited.fadeInFrames, 1_000); XCTAssertEqual(edited.soundDesign, pattern)
        XCTAssertEqual(clip, original); XCTAssertNotEqual(edited.assetID, clip.assetID)
        let shorter = clip.replacingSoundDesignRender(assetID: UUID(), pattern: pattern, frameCount: 30_000)
        XCTAssertEqual(shorter.offset, 24_000); XCTAssertEqual(shorter.length, 6_000)
    }
    func testNewDesignerInheritsSessionTempoAndMeterButExistingPatternKeepsItsTempo() {
        let store = SessionStore(headless: true)
        store.session.timing = MusicalTiming(tempo: 97, beatsPerBar: 7, beatUnit: 8)
        store.requestSoundDesigner()
        XCTAssertEqual(store.soundDesignRequest?.pattern.bpm, 97)
        XCTAssertEqual(store.soundDesignRequest?.pattern.beats, 3.5)
        var clip = Clip(assetID: UUID(), name: "Existing", start: 0, length: 96_000)
        clip.soundDesign = SoundDesignPattern(bpm: 120)
        store.session.tracks[0].clips = [clip]; store.selectedClip = clip.id
        store.requestSoundDesigner(editClip: true)
        XCTAssertEqual(store.soundDesignRequest?.pattern.bpm, 120)
    }
}
