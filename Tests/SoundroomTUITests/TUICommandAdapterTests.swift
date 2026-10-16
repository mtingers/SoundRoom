import XCTest
import SoundroomCore
import SoundroomRemote
@testable import SoundroomTUI

final class TUICommandAdapterTests: XCTestCase {
    func testWSSOptionsRequireSecureCompleteUnambiguousConnection() throws {
        let options = try TUIOptions.parse(["--wss", "wss://studio.local:49155/", "--origin", "https://studio.local:49154", "--status"])
        XCTAssertEqual(options.websocketURL?.scheme, "wss"); XCTAssertEqual(options.origin?.scheme, "https")
        for arguments in [["--wss", "wss://studio.local/"], ["--origin", "https://studio.local"],
                          ["--wss", "ws://studio.local/", "--origin", "https://studio.local"],
                          ["--wss", "wss://studio.local/?token=secret", "--origin", "https://studio.local"],
                          ["--wss", "wss://studio.local/", "--origin", "https://studio.local", "--host", "other.local"]] {
            XCTAssertThrowsError(try TUIOptions.parse(arguments))
        }
    }
    func testApprovedClientNameIsExplicitAndPreserved() throws {
        let name = String(repeating: "T", count: 100)
        XCTAssertEqual(try TUIOptions.parse(["--client-name", name]).clientName, name)
        XCTAssertThrowsError(try TUIOptions.parse(["--client-name", "\n"]))
        XCTAssertThrowsError(try TUIOptions.parse(["--client-name"]))
    }
    func fixture() -> (TUIModel, RemoteSnapshot) {
        var track = Track(name: "Voice", color: 0)
        track.clips = [Clip(assetID: UUID(), name: "Phrase", start: 48_000, length: 48_000)]
        var session = Session(); session.tracks = [track, Track(name: "Layers", color: 1)]
        let state = RemoteSnapshot(serverID: UUID(), serverName: "Studio", session: session, revision: 2, trackRevisions: [track.id: 2])
        var model = TUIModel(); model.update(TUISession(name: "Studio", tracks: session.tracks.map { t in .init(id: t.id, name: t.name, clips: t.clips.map { .init(id: $0.id, name: $0.name, start: $0.start, length: $0.length) }) }))
        return (model, state)
    }
    func testMuteTargetsExplicitTrackAndDoesNotChangeSelection() throws {
        let (model, snapshot) = fixture()
        let plan = try TUICommandAdapter.plan(.command("mute on"), model: model, snapshot: snapshot)
        XCTAssertEqual(plan.command, .setTrackMute(trackID: model.trackID!, muted: true))
        XCTAssertEqual(plan.trackIDs, [model.trackID!])
    }
    func testPastePreservesAssetAndUsesNewClipIdentityAtLocalDestination() throws {
        let (model, snapshot) = fixture()
        let source = snapshot.session.tracks[0], destination = snapshot.session.tracks[1]
        let intent = TUIIntent.paste(sourceTrackID: source.id, clipIDs: [source.clips[0].id], destinationTrackID: destination.id, frame: 240_000)
        let plan = try TUICommandAdapter.plan(intent, model: model, snapshot: snapshot)
        guard case .replaceTracks(let tracks, _, _) = plan.command else { return XCTFail("Expected explicit edit") }
        XCTAssertEqual(tracks.count, 1); XCTAssertEqual(tracks[0].id, destination.id)
        XCTAssertEqual(tracks[0].clips[0].start, 240_000)
        XCTAssertEqual(tracks[0].clips[0].assetID, source.clips[0].assetID)
        XCTAssertNotEqual(tracks[0].clips[0].id, source.clips[0].id)
        XCTAssertEqual(Set(plan.trackIDs), [source.id, destination.id])
    }
    func testRoutingResolvesServerDeviceAndOneBasedHumanChannel() throws {
        let (model, original) = fixture(); var snapshot = original
        snapshot.devices = [.init(id: 42, uid: "usb-device", name: "USB", inputChannels: 4, outputChannels: 2)]
        let plan = try TUICommandAdapter.plan(.command("input 42 2 stereo"), model: model, snapshot: snapshot)
        XCTAssertEqual(plan.command, .setTrackInput(trackID: model.trackID!, deviceUID: "usb-device", channel: 1, stereo: true))
        XCTAssertThrowsError(try TUICommandAdapter.plan(.command("input 42 0"), model: model, snapshot: snapshot))
        XCTAssertThrowsError(try TUICommandAdapter.plan(.command("output bad"), model: model, snapshot: snapshot))
    }
    func testInvalidAndDestructiveCommandsRequireClearIntent() throws {
        let (model, snapshot) = fixture()
        for line in ["volume nan", "volume 5", "pan -2", "remove-track", "rename", "save", "effect add imaginary"] {
            XCTAssertThrowsError(try TUICommandAdapter.plan(.command(line), model: model, snapshot: snapshot), line)
        }
        XCTAssertEqual(try TUICommandAdapter.plan(.command("remove-track confirm"), model: model, snapshot: snapshot).command, .removeTrack(trackID: model.trackID!))
    }
    func testEffectAddUsesNativeModernParameterSet() throws {
        let (model, snapshot) = fixture()
        let plan = try TUICommandAdapter.plan(.command("effect add echo"), model: model, snapshot: snapshot)
        guard case .replaceTracks(let tracks, _, _) = plan.command else { return XCTFail("Expected effect edit") }
        XCTAssertEqual(tracks[0].realtimeEffects[0].kind, .echo)
        XCTAssertEqual(tracks[0].realtimeEffects[0].parameters, RealtimeEffectKind.echo.defaultParameters)
        XCTAssertEqual(plan.trackIDs, [model.trackID!])
    }
    func testSpacePausesAtServerPositionWithoutSeekingLocalCursor() throws {
        var (model, snapshot) = fixture(); model.cursor = 240_000
        snapshot.transport = .init(frame: 120_000, isPlaying: true)
        let plan = try TUICommandAdapter.plan(.togglePlayback, model: model, snapshot: snapshot)
        XCTAssertEqual(plan.command, .editor(.init(operation: "transport.pause", context: .init(cursorFrame: 120_000))))
        snapshot.transport.isPlaying = false
        XCTAssertEqual(try TUICommandAdapter.plan(.togglePlayback, model: model, snapshot: snapshot).command, .play(trackOnly: nil))
    }
    func testCLIOptionParsingKeepsCredentialsInFiles() throws {
        let options = try TUIOptions.parse(["--host", "192.168.1.2", "--port", "50123", "--credential", "/tmp/pair.json", "--track", "Voice", "--send", "arm on"])
        XCTAssertEqual(options.host, "192.168.1.2"); XCTAssertEqual(options.port, 50123)
        XCTAssertEqual(options.credential.path, "/tmp/pair.json"); XCTAssertEqual(options.send, "arm on")
        XCTAssertThrowsError(try TUIOptions.parse(["--token", "secret"]))
        XCTAssertThrowsError(try TUIOptions.parse(["--status", "--send", "play"]))
        XCTAssertThrowsError(try TUIOptions.parse(["--port", "0"]))
    }
    func testBrowserAccessCommandsUseNamedExplicitRoleAndCredentialIdentity() throws {
        let (model, snapshot) = fixture()
        XCTAssertEqual(try TUICommandAdapter.plan(.command("web-pair viewer Studio browser"), model: model, snapshot: snapshot).command,
            .editor(.init(operation: "server.webPairing", strings: ["name": "Studio browser", "role": "viewer"])))
        XCTAssertEqual(try TUICommandAdapter.plan(.command("web-clients"), model: model, snapshot: snapshot).command, .editor(.init(operation: "server.webClients")))
        for line in ["web-pair", "web-pair admin Browser", "web-revoke bad-id", "web-clients extra"] {
            XCTAssertThrowsError(try TUICommandAdapter.plan(.command(line), model: model, snapshot: snapshot), line)
        }
    }
}
