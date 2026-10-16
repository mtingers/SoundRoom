import XCTest
import SoundroomCore
import SoundroomRemote
@testable import SoundroomTUI

final class TUIAdvancedCommandTests: XCTestCase {
    func fixture() -> (TUIModel, RemoteSnapshot) {
        var track = Track(name: "Voice", color: 0)
        track.clips = [Clip(assetID: UUID(), name: "A", start: 48_000, length: 48_000), Clip(assetID: UUID(), name: "B", start: 144_000, length: 48_000)]
        track.mixAutomation["pan"] = [.init(frame: 48_000, value: 0.3)]
        track.realtimeEffects = [RealtimeEffect(kind: .room, parameters: RealtimeEffectKind.room.defaultParameters)]
        track.realtimeEffects[0].parameterAutomation["mix"] = [.init(frame: 0, value: 0.5)]
        var document = Session(); document.tracks = [track, Track(name: "Bass", color: 1)]
        let snapshot = RemoteSnapshot(serverID: UUID(), serverName: "Studio", session: document, trackRevisions: [track.id: 1, document.tracks[1].id: 2])
        var cache = TUIProjectionCache(), model = TUIModel(); model.update(cache.project(snapshot))
        return (model, snapshot)
    }
    func testLoopSelectionCombinesMultipleClipsAndRangeAndOff() throws {
        var (m, s) = fixture(); m.clipIDs = Set(m.session.tracks[0].clips.map(\.id)); m.mode = .clips
        XCTAssertEqual(try TUICommandAdapter.plan(.command("loop"), model: m, snapshot: s).command, .setLoop(start: 48_000, end: 192_000))
        m.mode = .visual; m.anchor = 60_000; m.cursor = 180_000
        XCTAssertEqual(try TUICommandAdapter.plan(.command("loop"), model: m, snapshot: s).command, .setLoop(start: 60_000, end: 180_000))
        XCTAssertEqual(try TUICommandAdapter.plan(.command("loop off"), model: m, snapshot: s).command, .setLoop(start: nil, end: nil))
        for line in ["loop 5 1", "loop nan 3", "loop -1 4"] { XCTAssertThrowsError(try TUICommandAdapter.plan(.command(line), model: m, snapshot: s)) }
    }
    func testCombineRoutesSelectedIDsToServerRenderNotClientAudio() throws {
        var (m, s) = fixture(); m.mode = .clips; m.clipIDs = Set(m.session.tracks[0].clips.map(\.id))
        let plan = try TUICommandAdapter.plan(.command("combine"), model: m, snapshot: s)
        guard case .editor(let editor) = plan.command else { return XCTFail("Expected server edit") }
        XCTAssertEqual(editor.operation, "clips.combine"); XCTAssertEqual(editor.context.trackID, m.trackID)
        XCTAssertEqual(Set(editor.context.clipIDs), m.clipIDs); XCTAssertEqual(plan.trackIDs, [m.trackID!])
    }
    func testMovePreservesClipSpacingAndRefusesNegativeStart() throws {
        var (m, s) = fixture(); m.mode = .clips; m.clipIDs = Set(m.session.tracks[0].clips.map(\.id))
        let plan = try TUICommandAdapter.plan(.command("move 10"), model: m, snapshot: s)
        guard case .replaceTracks(let tracks, _, _) = plan.command else { return XCTFail("Expected explicit track edit") }
        XCTAssertEqual(tracks[0].clips.map(\.start), [480_000, 576_000])
        XCTAssertEqual(tracks[0].clips.map(\.id), s.session.tracks[0].clips.map(\.id))
        XCTAssertThrowsError(try TUICommandAdapter.plan(.command("nudge -2"), model: m, snapshot: s))
    }
    func testTrimPreservesOtherClipsAndProtectsNewBoundaries() throws {
        var (m, s) = fixture(); m.cursor = 60_000
        let plan = try TUICommandAdapter.plan(.command("trim 1.25 1.75"), model: m, snapshot: s)
        guard case .replaceTracks(let tracks, _, _) = plan.command else { return XCTFail("Expected trim") }
        let clipped = tracks[0].clips[0]
        XCTAssertEqual(clipped.id, s.session.tracks[0].clips[0].id)
        XCTAssertEqual(clipped.start, 60_000); XCTAssertEqual(clipped.offset, 12_000); XCTAssertEqual(clipped.length, 24_000)
        XCTAssertEqual(clipped.previousStates?.last?.length, 48_000); XCTAssertEqual(clipped.cutFadeInFrames, Clip.defaultEdgeFadeFrames); XCTAssertEqual(clipped.cutFadeOutFrames, Clip.defaultEdgeFadeFrames)
        XCTAssertEqual(tracks[0].clips[1], s.session.tracks[0].clips[1])
        XCTAssertThrowsError(try TUICommandAdapter.plan(.command("trim 0 3"), model: m, snapshot: s))
        var changed = s; changed.session.tracks[0].clips[0].length = 24_000
        XCTAssertThrowsError(try TUICommandAdapter.plan(.command("trim 1.25 1.75"), model: m, snapshot: changed), "A queued trim must respect the current clip's bounds")
    }
    func testDuplicateVariantsPreserveAudioAndUseAllOrderRevisions() throws {
        let (m, s) = fixture()
        for withAutomation in [false, true] {
            let plan = try TUICommandAdapter.plan(.command("duplicate-track" + (withAutomation ? " automation" : "")), model: m, snapshot: s)
            guard case .editTracks(let edit) = plan.command else { return XCTFail("Expected atomic insertion") }
            let copied = edit.tracks[0], original = s.session.tracks[0]
            XCTAssertNotEqual(copied.id, original.id); XCTAssertNotEqual(copied.clips[0].id, original.clips[0].id)
            XCTAssertEqual(copied.clips[0].assetID, original.clips[0].assetID)
            XCTAssertNotEqual(copied.realtimeEffects[0].id, original.realtimeEffects[0].id)
            XCTAssertEqual(edit.trackOrder, [original.id, copied.id, s.session.tracks[1].id])
            XCTAssertEqual(Set(plan.trackIDs), Set(s.session.tracks.map(\.id)))
            XCTAssertEqual(copied.mixAutomation.isEmpty, !withAutomation)
            XCTAssertEqual(copied.realtimeEffects[0].parameterAutomation.isEmpty, !withAutomation)
        }
    }
    func testFactoryPresetsAcceptNumberOrNameAndRetainAutomation() throws {
        let (m, s) = fixture(); let kind = RealtimeEffectKind.room
        for line in ["effect preset 1 1", "effect preset 1 \(kind.presets[0].name)"] {
            let plan = try TUICommandAdapter.plan(.command(line), model: m, snapshot: s)
            guard case .replaceTracks(let tracks, _, _) = plan.command else { return XCTFail("Expected preset") }
            XCTAssertEqual(tracks[0].realtimeEffects[0].parameters, kind.presets[0].values)
            XCTAssertEqual(tracks[0].realtimeEffects[0].parameterAutomation, s.session.tracks[0].realtimeEffects[0].parameterAutomation)
        }
        XCTAssertThrowsError(try TUICommandAdapter.plan(.command("effect preset 1 999"), model: m, snapshot: s))
    }
    func testPlayTrackAndServerDeviceProjection() throws {
        var (m, s) = fixture()
        XCTAssertEqual(try TUICommandAdapter.plan(.command("play-track"), model: m, snapshot: s).command, .play(trackOnly: m.trackID!))
        s.devices = [.init(id: 42, uid: "usb", name: "USB", inputChannels: 2, outputChannels: 2)]; s.outputDeviceID = 42
        s.session.tracks[0].inputDeviceUID = "usb"; s.session.tracks[0].inputChannel = 1
        var cache = TUIProjectionCache(); m.update(cache.project(s))
        XCTAssertEqual(m.session.outputID, 42); XCTAssertEqual(m.session.devices[0].inputs, 2)
        XCTAssertTrue(m.session.tracks[0].input.contains("ch 2"))
    }
}
