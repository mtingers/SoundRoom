import Foundation
import SoundroomCore
import XCTest

@testable import SoundroomRemote

final class RemoteV3CodecTests: XCTestCase {
    private let id = UUID(uuidString: "12345678-1234-4234-8234-123456789abc")!
    func testMaximumUploadChunkRetainsCanonicalWireBytes() throws {
        let bytes = Data((0..<RemoteProtocol.maximumUploadChunkBytes).map { UInt8(truncatingIfNeeded: $0) })
        let request = RemoteRequest(
            id: id, clientID: id, expectedRevision: 7, expectedTrackRevisions: [id: 3],
            command: .uploadChunk(id: id, offset: 262_144, data: bytes))
        let actual = try RemoteV3Codec.encodeMessage(.request(request))
        let expected: [String: Any] = [
            "version": 3, "kind": "request", "id": id.uuidString, "clientID": id.uuidString,
            "expectedRevision": 7, "expectedTrackRevisions": [id.uuidString: 3],
            "command": [
                "type": "uploadChunk", "id": id.uuidString, "offset": 262_144, "data": bytes.base64EncodedString(),
            ],
        ]
        XCTAssertEqual(
            actual, try JSONSerialization.data(withJSONObject: expected, options: [.sortedKeys, .fragmentsAllowed]))
        guard case .request(let decoded) = try RemoteV3Codec.decodeMessage(actual) else {
            return XCTFail("Expected request")
        }
        XCTAssertEqual(decoded, request)
    }
    func testAllCommandsHaveExplicitTagsAndRoundTripThroughLegacyAuthorityModel() throws {
        let controls = RemoteTrackControls(track: Track(name: "Voice", color: 1))
        let track = Track(name: "Organ", color: 2)
        let commands: [RemoteCommand] = [
            .snapshot, .play(trackOnly: nil), .stop, .seek(frame: 48000), .record, .takeTransport,
            .setLoop(start: 0, end: 96000), .updateTrackControls(controls: controls, name: "Level", gestureID: "fader"),
            .setTrackName(trackID: id, name: "Voice"), .setTrackGain(trackID: id, gain: 0.5),
            .setTrackPan(trackID: id, pan: -0.5),
            .setTrackMute(trackID: id, muted: true), .setTrackSolo(trackID: id, solo: false),
            .setTrackArm(trackID: id, armed: true),
            .setDevices(inputID: 4, outputID: 4),
            .setTrackInput(trackID: id, deviceUID: "opaque", channel: 0, stereo: false),
            .addTrack(name: "New"), .removeTrack(trackID: id), .splitClip(trackID: id, clipID: id, frame: 24000),
            .deleteClips(trackID: id, clipIDs: [id]), .moveClip(trackID: id, clipID: id, start: 12000),
            .replaceTracks(tracks: [track], name: nil, gestureID: nil),
            .editTracks(.init(tracks: [track], name: "Edit")),
            .editor(.init(operation: "session.timing", numbers: ["tempo": 123])), .undo, .redo, .listProjects,
            .openProject(id: "project"), .saveProject(name: "Session", replaceExisting: true, title: "Preserved title"),
            .exportMix(name: "Mix"),
            .beginUpload(id: id, name: "clip.wav", bytes: 20, sha256: String(repeating: "a", count: 64)),
            .uploadChunk(id: id, offset: 0, data: Data([1, 2])), .finishUpload(id: id, trackID: nil, frame: 0),
            .downloadArtifact(id: id, offset: 0, bytes: 2), .readArtifact(id: "opaque", offset: 2, bytes: 2),
            .waveform(assetID: id, start: 0, end: 48000, points: 2048), .ping(clientTime: 1.5),
        ]
        XCTAssertEqual(commands.count, RemoteV3Descriptor.commands.count, "Fixture must cover every command")
        let devices = [RemoteDevice(id: 4, uid: "opaque", name: "Mic", inputChannels: 1, outputChannels: 2)]
        for command in commands {
            let request = RemoteRequest(
                id: id, clientID: id, expectedRevision: 2, expectedTrackRevisions: [id: 1], command: command)
            let data = try RemoteV3Codec.encodeMessage(.request(request), devices: devices)
            let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
            XCTAssertEqual(object["kind"] as? String, "request")
            XCTAssertNotNil((object["command"] as? [String: Any])?["type"])
            guard case .request(let decoded) = try RemoteV3Codec.decodeMessage(data, devices: devices) else {
                return XCTFail("Not a request")
            }
            XCTAssertEqual(decoded, request)
        }
    }
    func testEventsHandshakeRepliesAndSeedsArePortable() throws {
        var session = Session()
        var pattern = SoundDesignPattern()
        pattern.organic.seed = UInt64.max
        var layer = SoundDesignLayer()
        layer.organicSeed = UInt64.max
        pattern.layers = [layer]
        var clip = Clip(assetID: id, name: "Layer", start: 0, length: 100)
        clip.soundDesign = pattern
        session.tracks[0].clips = [clip]
        let events: [RemoteEvent] = [
            .snapshot(.init(serverID: id, serverName: "Mac", session: session, revision: 3, trackRevisions: [id: 2])),
            .delta(.init(revision: 4, tracks: session.tracks, trackRevisions: [id: 3])),
            .transport(.init(frame: 42)), .meters(left: 0.1, right: 0.2, input: 0.3), .effectMeters([:]),
            .audition(kind: "designer", ownerID: id, playing: true, preparing: false, position: 0.5),
            .devices([], inputID: 0, outputID: 0), .presence([]), .midiSources([]),
            .midiNote(pitch: 60, velocity: 127, on: true),
            .midiController(kind: .modulation, value: 0.2), .midiAllNotesOff,
            .history(canUndo: true, canRedo: false, undoName: "Edit", redoName: nil), .status("Ready"),
            .waveform(.init(id: id, frames: 100, channels: 1)), .analysis(.init()), .projects(["project"]),
            .job(id: id, progress: 0.5, message: "Render"),
            .automationWrite(update: .init(gestureID: id, phase: .cancelled, reason: "Host Write not enabled")),
        ]
        XCTAssertEqual(events.count, RemoteV3Descriptor.events.count)
        for (index, event) in events.enumerated() {
            do {
                let data = try RemoteV3Codec.encodeMessage(.event(event))
                guard case .event(let decoded) = try RemoteV3Codec.decodeMessage(data) else {
                    return XCTFail("Not event")
                }
                XCTAssertEqual(event, decoded)
            } catch { XCTFail("Event \(index): \(error)") }
        }
        let messages: [RemoteWireMessage] = [
            .challenge(.init(serverID: id, nonce: Data([1]))),
            .hello(
                .init(
                    serverID: id, peer: .init(id: id, name: "Browser", role: .viewer), credentialID: id, proof: "proof")
            ),
            .welcome(.init(serverID: id, name: "Mac", role: .editor)),
            .reply(.init(requestID: id, revision: 4, payload: Data([2]))), .failure("No"), .heartbeat(id),
            .heartbeatReply(id),
        ]
        for message in messages {
            let encoded = try RemoteV3Codec.encodeMessage(message)
            let decoded = try RemoteV3Codec.decodeMessage(encoded)
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.sortedKeys]
            XCTAssertEqual(try encoder.encode(decoded), try encoder.encode(message))
        }
    }
    func testReplyPayloadRetainsArtifactBytesEvenWhenTheyAreValidJSON() throws {
        let bytes = Data(" {\"seed\":18446744073709551615}\n".utf8)
        let reply = RemoteReply(requestID: id, revision: 1, payload: bytes)
        guard case .reply(let decoded) = try RemoteV3Codec.decodeMessage(RemoteV3Codec.encodeMessage(.reply(reply)))
        else { return XCTFail() }
        XCTAssertEqual(decoded.payload, bytes)
    }
    func testDesignerPayloadRestoresExactPortableSeedsBeforeAuthoritativeDecode() throws {
        var pattern = SoundDesignPattern()
        pattern.organic.seed = UInt64.max
        var layer = SoundDesignLayer()
        layer.organicSeed = UInt64.max
        pattern.layers = [layer]
        let request = RemoteRequest(
            clientID: id,
            command: .editor(.init(operation: "designer.commit", payload: try JSONEncoder().encode(pattern))))
        let encoded = try RemoteV3Codec.encodeMessage(.request(request))
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
        let command = try XCTUnwrap(object["command"] as? [String: Any])
        let intent = try XCTUnwrap(command["intent"] as? [String: Any])
        let text = try XCTUnwrap(intent["payload"] as? String)
        let payload = try XCTUnwrap(Data(base64Encoded: text))
        let patternObject = try XCTUnwrap(JSONSerialization.jsonObject(with: payload) as? [String: Any])
        XCTAssertEqual((patternObject["organic"] as? [String: Any])?["seed"] as? String, String(UInt64.max))
        guard case .request(let decoded) = try RemoteV3Codec.decodeMessage(encoded),
            case .editor(let result) = decoded.command
        else { return XCTFail() }
        XCTAssertEqual(try JSONDecoder().decode(SoundDesignPattern.self, from: XCTUnwrap(result.payload)), pattern)
    }
    func testRejectsUnknownFieldsUnsafeIntegersWrongTypesAndOpaqueDeviceFailures() throws {
        let base: [String: Any] = [
            "version": 3, "kind": "request", "id": id.uuidString, "clientID": id.uuidString,
            "command": ["type": "seek", "frame": 1],
        ]
        var invalid = base
        invalid["admin"] = true
        try rejected(invalid)
        invalid = base
        invalid["expectedRevision"] = 9_007_199_254_740_992 as UInt64
        try rejected(invalid)
        invalid = base
        invalid["command"] = ["type": "seek", "frame": 1.5]
        try rejected(invalid)
        invalid = base
        invalid["command"] = ["type": "seek", "frame": true]
        try rejected(invalid)
        invalid = base
        invalid["command"] = ["type": "seek", "frame": -1]
        try rejected(invalid)
        invalid = base
        invalid["command"] = ["type": "seek", "frame": 1, "selector": "delete:"]
        try rejected(invalid)
        invalid = base
        invalid["clientID"] = "invalid"
        try rejected(invalid)
        invalid = base
        invalid["version"] = 2
        try rejected(invalid)
        invalid = base
        invalid["command"] = ["type": "setDevices", "inputUID": "missing"]
        try rejected(invalid)
        invalid = base
        invalid["command"] = [
            "type": "uploadChunk", "id": id.uuidString, "offset": 0,
            "data": Data(repeating: 0, count: 262145).base64EncodedString(),
        ]
        try rejected(invalid)
        invalid = base
        invalid["command"] = [
            "type": "editor",
            "intent": [
                "operation": "run.shell", "context": ["cursorFrame": 0, "clipIDs": []], "integers": [:], "numbers": [:],
                "strings": [:], "flags": [:],
            ],
        ]
        try rejected(invalid)
    }
    func testAdapterPreservesAuthorshipRoleAndExpectedTrackRevisions() throws {
        let request = RemoteRequest(
            id: id, clientID: id, expectedRevision: 8, expectedTrackRevisions: [id: 6], command: .undo)
        guard
            case .request(let decoded) = try RemoteV3Codec.decodeMessage(RemoteV3Codec.encodeMessage(.request(request)))
        else { return XCTFail() }
        XCTAssertEqual(decoded, request)
        let hello = RemoteHello(
            serverID: id, peer: .init(id: id, name: "Editor", role: .viewer), credentialID: id, proof: "unverified")
        guard case .hello(let result) = try RemoteV3Codec.decodeMessage(RemoteV3Codec.encodeMessage(.hello(hello)))
        else { return XCTFail() }
        XCTAssertEqual(result.version, 2)
        XCTAssertEqual(result.peer.role, .viewer)
        XCTAssertEqual(result.proof, "unverified")
    }
    private func rejected(_ value: [String: Any]) throws {
        XCTAssertThrowsError(try RemoteV3Codec.decodeMessage(JSONSerialization.data(withJSONObject: value)))
    }
}
