import XCTest
import SoundroomCore
@testable import Soundroom

final class PitchTimelineMetadataTests: XCTestCase {
    @MainActor private func fixture() -> SessionStore {
        let store = SessionStore(), asset = UUID()
        store.snap = false; store.moveAutomationWithClips = true
        store.assets[asset] = AudioSamples(channels: [[Float](repeating: 0.1, count: 2000)])
        let clip = Clip(assetID: asset, name: "Voice", start: 100, length: 200, edgeFadeFrames: 0)
        store.session.tracks[0].clips = [clip]
        let region = PitchEditRegion(range: 0..<1000, points: [
            .init(frame: 0, note: 60.125, curve: .easeIn), .init(frame: 1000, note: 72.875)])
        var effect = RealtimeEffect(kind: .pitchCorrection, parameters: RealtimeEffectKind.pitchCorrection.defaultParameters,
                                    pitchEdits: [region], pitchEditsOnly: true)
        effect.bakedRanges = [.init(startFrame: 50, endFrame: 800)]
        store.session.tracks[0].realtimeEffects = [effect]
        store.choose(track: store.session.tracks[0].id, clip: clip.id)
        return store
    }
    private func note(_ effect: RealtimeEffect, at frame: Int) -> Float? {
        effect.pitchEdits.first { $0.range.contains(frame) }?.note(at: frame)
    }

    func testDuplicateRenewsMetadataIDsAndReplacesDestinationWithoutOverlaps() throws {
        try MainActor.assumeIsolated {
            let store = fixture(), original = store.session.tracks[0].realtimeEffects[0], clip = store.currentClip!
            store.moveSelectedClips(primary: clip.id, to: 400, duplicate: true)
            let result = store.session.tracks[0].realtimeEffects[0]
            for frame in 100..<300 { XCTAssertEqual(try XCTUnwrap(note(result, at: frame)), try XCTUnwrap(note(original, at: frame)), accuracy: 0.00002) }
            let copied = try XCTUnwrap(result.pitchEdits.first { $0.range == 400..<600 })
            XCTAssertNotEqual(copied.id, original.pitchEdits[0].id)
            XCTAssertTrue(Set(copied.points.map(\.id)).isDisjoint(with: original.pitchEdits.flatMap { $0.points.map(\.id) }))
            for frame in 400..<600 { XCTAssertEqual(try XCTUnwrap(note(result, at: frame)), try XCTUnwrap(note(original, at: frame - 300)), accuracy: 0.00002) }
            for (before, after) in zip(result.pitchEdits, result.pitchEdits.dropFirst()) { XCTAssertLessThanOrEqual(before.endFrame, after.startFrame) }
            XCTAssertEqual(result.bakedRanges.map(\.range), [50..<800])
        }
    }
    func testCrossTrackCopyCarriesOnlySelectedScopeAndRequiresMatchingScopeMode() throws {
        try MainActor.assumeIsolated {
            for matchingMode in [false, true] {
                let store = fixture(), source = store.session.tracks[0].realtimeEffects[0], clip = store.currentClip!
                var destination = Track(name: "Destination", color: 1)
                var existing = source; existing.id = UUID(); existing.pitchEdits = []; existing.bakedRanges = []
                existing.pitchEditsOnly = matchingMode
                destination.realtimeEffects = [existing]; store.session.tracks.append(destination)
                store.moveSelectedClips(primary: clip.id, to: 400, targetTrack: destination.id, duplicate: true)
                XCTAssertEqual(store.session.tracks[0].realtimeEffects[0], source)
                XCTAssertEqual(store.session.tracks[1].realtimeEffects.count, matchingMode ? 1 : 2)
                let copied = try XCTUnwrap(store.session.tracks[1].realtimeEffects.last)
                XCTAssertTrue(copied.pitchEditsOnly)
                XCTAssertEqual(copied.pitchEdits.map(\.range), [400..<600])
                XCTAssertEqual(copied.bakedRanges.map(\.range), [400..<600])
                for frame in 400..<600 { XCTAssertEqual(try XCTUnwrap(note(copied, at: frame)), try XCTUnwrap(note(source, at: frame - 300)), accuracy: 0.00002) }
            }
        }
    }
    func testCrossTrackMoveAndMulticlipSelectionKeepGapsAndNeverMoveIncomingTwice() throws {
        try MainActor.assumeIsolated {
            let store = fixture(), first = store.currentClip!, source = store.session.tracks[0].realtimeEffects[0]
            var second = first; second.id = UUID(); second.start = 700; second.length = 100
            store.session.tracks[0].clips.append(second)
            let destination = Track(name: "Destination", color: 1); store.session.tracks.append(destination)
            store.selectedClips = [first.id, second.id]
            store.moveSelectedClips(primary: first.id, to: 0, targetTrack: destination.id)
            let moved = store.session.tracks[1].realtimeEffects[0]
            XCTAssertEqual(moved.pitchEdits.map(\.range), [0..<200, 600..<700])
            XCTAssertNil(note(moved, at: 300))
            for frame in [0, 50, 199, 600, 650, 699] { XCTAssertEqual(try XCTUnwrap(note(moved, at: frame)), try XCTUnwrap(note(source, at: frame + 100)), accuracy: 0.00002) }
            let remaining = store.session.tracks[0].realtimeEffects[0]
            XCTAssertNil(note(remaining, at: 150)); XCTAssertNil(note(remaining, at: 750))
            XCTAssertEqual(Set(moved.pitchEdits.map(\.id)).count, 2)
        }
    }
    func testAutomationCarryOptOutLeavesPitchMetadataUnchanged() {
        MainActor.assumeIsolated {
            let store = fixture(), original = store.session.tracks[0].realtimeEffects[0], clip = store.currentClip!
            store.moveAutomationWithClips = false
            store.moveSelectedClips(primary: clip.id, to: 400)
            XCTAssertEqual(store.session.tracks[0].realtimeEffects[0], original)
        }
    }
    func testPortableChainClearsMetadataOrRenewsEveryIncludedID() throws {
        try MainActor.assumeIsolated {
            let effect = fixture().session.tracks[0].realtimeEffects[0]
            let plain = try XCTUnwrap(EffectChainLibrary.prepared([effect], includeAutomation: false).first)
            XCTAssertEqual(plain.pitchEdits, []); XCTAssertEqual(plain.bakedRanges, [])
            let copied = try XCTUnwrap(EffectChainLibrary.prepared([effect], includeAutomation: true).first)
            XCTAssertNotEqual(copied.id, effect.id); XCTAssertNotEqual(copied.pitchEdits[0].id, effect.pitchEdits[0].id)
            XCTAssertTrue(Set(copied.pitchEdits.flatMap { $0.points.map(\.id) }).isDisjoint(with: effect.pitchEdits.flatMap { $0.points.map(\.id) }))
            // Source-audio guards never travel with a recalled rack (see NativeChainGuardTests).
            XCTAssertEqual(copied.bakedRanges, [])
            XCTAssertEqual(EffectChainLibrary.prepared([effect], includeAutomation: true, regenerateIDs: false), [effect])
            let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            defer { try? FileManager.default.removeItem(at: directory) }
            let library = EffectChainLibrary(url: directory.appendingPathComponent("chains.json"))
            let saved = try library.save(name: "Pitch targets", effects: [effect], includeAutomation: true)
            XCTAssertEqual(EffectChainLibrary(url: library.url).chains, [saved])
        }
    }
}
