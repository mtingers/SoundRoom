import Foundation
import SoundroomCore
import SoundroomRemote

struct TUICommandPlan {
    var command: RemoteCommand?
    var trackIDs: [UUID] = []
    var message: String?
}
struct TUICommandAdapter {
    static func plan(_ intent: TUIIntent, model: TUIModel, snapshot: RemoteSnapshot) throws -> TUICommandPlan {
        let track = snapshot.session.tracks.first { $0.id == model.trackID }
        switch intent {
        case .togglePlayback:
            if snapshot.transport.isPlaying || snapshot.transport.isRecording || snapshot.transport.isPreparing {
                return .init(
                    command: .editor(
                        .init(operation: "transport.pause", context: .init(cursorFrame: snapshot.transport.frame))))
            }
            return .init(command: .play(trackOnly: nil))
        case .play: return .init(command: .play(trackOnly: nil))
        case .stop: return .init(command: .stop)
        case .record: return .init(command: snapshot.transport.isRecording ? .stop : .record)
        case .startRecording: return .init(command: .record)
        case .seek(let frame): return .init(command: .seek(frame: frame))
        case .undo: return .init(command: .undo)
        case .redo: return .init(command: .redo)
        case .delete(let trackID, let clipIDs):
            return .init(command: .deleteClips(trackID: trackID, clipIDs: clipIDs), trackIDs: [trackID])
        case .paste(let sourceID, let clipIDs, let destinationID, let frame):
            guard let source = snapshot.session.tracks.first(where: { $0.id == sourceID }),
                var destination = snapshot.session.tracks.first(where: { $0.id == destinationID })
            else { throw CommandError("A clipboard track no longer exists.") }
            let clips = source.clips.filter { clipIDs.contains($0.id) }
            guard clips.count == clipIDs.count, let start = clips.map(\.start).min() else {
                throw CommandError("A copied clip was removed; copy the current clips again.")
            }
            guard destination.clips.count + clips.count <= 4096 else {
                throw CommandError("This paste would exceed 4096 clips on one track.")
            }
            destination.clips += clips.map { original in
                var clip = original
                clip.id = UUID()
                clip.start = frame + original.start - start
                return clip
            }
            return .init(
                command: .replaceTracks(tracks: [destination], name: nil, gestureID: nil),
                trackIDs: Array(Set([sourceID, destinationID])))
        case .quit: return .init(message: "Disconnected.")
        case .command(let line): return try command(line, track: track, model: model, snapshot: snapshot)
        }
    }
    private static func command(_ line: String, track: Track?, model: TUIModel, snapshot: RemoteSnapshot) throws
        -> TUICommandPlan
    {
        let tokens = line.split(whereSeparator: \.isWhitespace).map(String.init)
        guard let first = tokens.first else { return .init() }
        let arguments = Array(tokens.dropFirst())
        let name = first.lowercased()
        func requireTrack() throws -> Track {
            guard let track else { throw CommandError("Select a track with j/k first.") }
            return track
        }
        func bool(_ current: Bool) throws -> Bool {
            guard let argument = arguments.first else { return !current }
            if ["on", "1", "true"].contains(argument) { return true }
            if ["off", "0", "false"].contains(argument) { return false }
            throw CommandError("Usage: :\(name) on|off")
        }
        func number(_ range: ClosedRange<Double>) throws -> Double {
            guard arguments.count == 1, let value = Double(arguments[0]), value.isFinite, range.contains(value) else {
                throw CommandError("Usage: :\(name) \(range.lowerBound)…\(range.upperBound)")
            }
            return value
        }
        switch name {
        case "web-clients":
            guard arguments.isEmpty else { throw CommandError("Usage: :web-clients") }
            return .init(command: .editor(.init(operation: "server.webClients")))
        case "web-pair":
            guard arguments.count >= 2, let role = RemoteRole(rawValue: arguments[0]) else {
                throw CommandError("Usage: :web-pair viewer|editor|operator client name")
            }
            return .init(
                command: .editor(
                    .init(
                        operation: "server.webPairing",
                        strings: ["name": arguments.dropFirst().joined(separator: " "), "role": role.rawValue])))
        case "web-revoke":
            guard arguments.count == 1, let id = UUID(uuidString: arguments[0]) else {
                throw CommandError("Usage: :web-revoke credential-UUID (use :web-clients)")
            }
            return .init(
                command: .editor(
                    .init(operation: "server.revokeWebCredential", strings: ["credentialID": id.uuidString])))
        case "mute":
            let t = try requireTrack()
            return .init(command: .setTrackMute(trackID: t.id, muted: try bool(t.muted)), trackIDs: [t.id])
        case "solo":
            let t = try requireTrack()
            return .init(command: .setTrackSolo(trackID: t.id, solo: try bool(t.solo)), trackIDs: [t.id])
        case "arm":
            let t = try requireTrack()
            return .init(command: .setTrackArm(trackID: t.id, armed: try bool(t.armed)), trackIDs: [t.id])
        case "volume":
            let t = try requireTrack()
            return .init(command: .setTrackGain(trackID: t.id, gain: Float(try number(0...4))), trackIDs: [t.id])
        case "pan":
            let t = try requireTrack()
            return .init(command: .setTrackPan(trackID: t.id, pan: Float(try number(-1...1))), trackIDs: [t.id])
        case "rename":
            let t = try requireTrack()
            guard !arguments.isEmpty else { throw CommandError("Usage: :rename track name") }
            return .init(
                command: .setTrackName(trackID: t.id, name: arguments.joined(separator: " ")), trackIDs: [t.id])
        case "new-track":
            return .init(command: .addTrack(name: arguments.isEmpty ? "Audio" : arguments.joined(separator: " ")))
        case "remove-track":
            let t = try requireTrack()
            guard arguments == ["confirm"] else {
                throw CommandError("This removes \(t.name) and all its clips. Use :remove-track confirm.")
            }
            return .init(command: .removeTrack(trackID: t.id), trackIDs: [t.id])
        case "split":
            let t = try requireTrack()
            guard let clip = model.selectedClips.first, model.selectedClips.count == 1 else {
                throw CommandError("Place the cursor inside one clip first.")
            }
            return .init(command: .splitClip(trackID: t.id, clipID: clip.id, frame: model.cursor), trackIDs: [t.id])
        case "tempo":
            let bpm = try number(20...300)
            return .init(command: .editor(.init(operation: "session.timing", numbers: ["tempo": bpm])))
        case "meter":
            guard arguments.count == 1 else {
                throw CommandError("Usage: :meter 4/4 (numerator 1…16; denominator 2,4,8,16)")
            }
            let parts = arguments[0].split(separator: "/", omittingEmptySubsequences: false)
            guard parts.count == 2, let numerator = Int(parts[0]), let denominator = Int(parts[1]),
                (1...16).contains(numerator), MusicalTiming.beatUnits.contains(denominator)
            else { throw CommandError("Usage: :meter 4/4 (numerator 1…16; denominator 2,4,8,16)") }
            return .init(
                command: .editor(
                    .init(operation: "session.timing", integers: ["beatsPerBar": numerator, "beatUnit": denominator])))
        case "play-track":
            let t = try requireTrack()
            return .init(command: .play(trackOnly: t.id))
        case "loop":
            if arguments == ["off"] { return .init(command: .setLoop(start: nil, end: nil)) }
            if arguments.isEmpty, snapshot.transport.loopStart != nil {
                return .init(command: .setLoop(start: nil, end: nil))
            }
            let start: Int
            let end: Int
            if arguments.isEmpty {
                if model.mode == .visual, let anchor = model.anchor {
                    start = min(anchor, model.cursor)
                    end = max(anchor, model.cursor)
                } else {
                    start = model.selectedClips.map(\.start).min() ?? 0
                    end = model.selectedClips.map(\.end).max() ?? 0
                }
            } else {
                guard arguments.count == 2, let a = model.coordinateFrame(arguments[0]),
                    let b = model.coordinateFrame(arguments[1]), b > a
                else { throw CommandError("Usage: :loop start end|off (seconds in Time; bar.beat[.tick] in Beats)") }
                start = a
                end = b
            }
            guard end > start else { throw CommandError("Select clips or drag a time range to loop first.") }
            return .init(command: .setLoop(start: start, end: end))
        case "combine", "merge":
            let t = try requireTrack()
            let clips = model.selectedClips
            guard clips.count >= 2 else {
                throw CommandError("Select at least two clips with Shift-click or v + motion to combine.")
            }
            return .init(
                command: .editor(
                    .init(
                        operation: "clips.combine",
                        context: .init(trackID: t.id, clipIDs: clips.map(\.id), cursorFrame: model.cursor))),
                trackIDs: [t.id])
        case "move", "nudge":
            var t = try requireTrack()
            let ids = Set(model.selectedClips.map(\.id))
            guard !ids.isEmpty, arguments.count == 1 else {
                throw CommandError("Usage: :\(name) position/amount; first select clips.")
            }
            let first = model.selectedClips.map(\.start).min() ?? 0
            let delta: Int
            if name == "move" {
                guard let frame = model.coordinateFrame(arguments[0]) else {
                    throw CommandError("Move requires seconds in Time or bar.beat[.tick] in Beats.")
                }
                delta = model.snapped(frame) - first
            } else {
                guard let amount = Double(arguments[0]), amount.isFinite, abs(amount) <= 86400 else {
                    throw CommandError("Nudge requires a finite amount: seconds in Time, meter beats in Beats.")
                }
                if model.timebase == .beats {
                    let timing = model.session.timing
                    var origin = timing.quarterNotes(at: first, sampleRate: model.session.sampleRate)
                    let division = abs(amount * timing.quarterNotesPerBeat)
                    if division > 0 {
                        let boundary = (origin / division).rounded() * division
                        let frameTolerance = timing.valid.tempo / 60 / model.session.sampleRate
                        if abs(boundary - origin) <= frameTolerance { origin = boundary }
                    }
                    let target = origin + amount * timing.quarterNotesPerBeat
                    guard target >= 0 else { throw CommandError("Clips cannot move before the session start.") }
                    delta = timing.frame(atQuarterNote: target, sampleRate: model.session.sampleRate) - first
                } else {
                    delta = Int((amount * model.session.sampleRate).rounded())
                }
            }
            guard first + delta >= 0 else { throw CommandError("Clips cannot move before the session start.") }
            for i in t.clips.indices where ids.contains(t.clips[i].id) { t.clips[i].start += delta }
            return .init(command: .replaceTracks(tracks: [t], name: "Move clips", gestureID: nil), trackIDs: [t.id])
        case "trim":
            var t = try requireTrack()
            let clips = model.selectedClips
            guard clips.count == 1, arguments.count == 2, let start = model.coordinateFrame(arguments[0]),
                let end = model.coordinateFrame(arguments[1]), end > start
            else { throw CommandError("Usage: :trim start end (seconds in Time; bar.beat[.tick] in Beats)") }
            guard let clip = t.clips.first(where: { $0.id == clips[0].id }) else {
                throw CommandError("The selected clip was removed; select the current audio again.")
            }
            guard start >= clip.start, end <= clip.end, end > start else {
                throw CommandError("Trim endpoints must be inside the selected clip.")
            }
            let originals = t.clips.filter { $0.start < end && $0.end > start }
            let fragments = snapshot.session.fragments(trackID: t.id, range: start..<end)
            guard let index = originals.firstIndex(where: { $0.id == clip.id }), fragments.indices.contains(index)
            else { throw CommandError("Cannot trim this clip.") }
            let original = originals[index]
            var fragment = fragments[index]
            fragment.id = original.id
            fragment.start += start
            fragment.remember(original, operation: "Trim clip")
            t.clips = t.clips.map { $0.id == clip.id ? fragment : $0 }
            return .init(command: .replaceTracks(tracks: [t], name: "Trim clip", gestureID: nil), trackIDs: [t.id])
        case "duplicate-track":
            let original = try requireTrack()
            guard arguments.isEmpty || arguments == ["automation"] else {
                throw CommandError("Usage: :duplicate-track [automation]")
            }
            var t = original
            t.id = UUID()
            t.name += " copy"
            t.armed = false
            t.clips = t.clips.map {
                var c = $0
                c.id = UUID()
                return c
            }
            t.recordingTakes = []
            t.activeTakeID = nil
            for i in t.realtimeEffects.indices {
                t.realtimeEffects[i].id = UUID()
                t.realtimeEffects[i].pitchEdits = PitchEditRegions.relocated(
                    t.realtimeEffects[i].pitchEdits, by: 0, renewIDs: true)
            }
            if arguments.isEmpty {
                t.mixAutomation = [:]
                for i in t.realtimeEffects.indices {
                    t.realtimeEffects[i].automation = []
                    t.realtimeEffects[i].parameterAutomation = [:]
                    t.realtimeEffects[i].pitchEdits = []
                    t.realtimeEffects[i].bakedRanges = []
                }
            }
            return .init(
                command: .editTracks(
                    .init(
                        tracks: [t],
                        trackOrder: snapshot.session.tracks.flatMap { $0.id == original.id ? [$0.id, t.id] : [$0.id] },
                        name: "Duplicate track")), trackIDs: snapshot.session.tracks.map(\.id))
        case "inputs", "outputs":
            let inputs = name == "inputs"
            return .init(
                message: snapshot.devices.filter { inputs ? $0.inputChannels > 0 : $0.outputChannels > 0 }.map {
                    "\($0.id) \($0.name) [\($0.uid)] \(inputs ? $0.inputChannels : $0.outputChannels)ch"
                }.joined(separator: " · "))
        case "input":
            let t = try requireTrack()
            guard let deviceArg = arguments.first else {
                throw CommandError("Usage: :input deviceID|UID|default [channel 1…] [stereo]")
            }
            guard arguments.count <= 3, arguments.count < 3 || arguments[2] == "stereo" else {
                throw CommandError("Usage: :input deviceID|UID|default [channel] [stereo]")
            }
            let channel = arguments.count > 1 ? Int(arguments[1]) : 1
            guard let channel, channel >= 1, channel <= 128 else { throw CommandError("Input channels start at 1.") }
            let device = snapshot.devices.first { String($0.id) == deviceArg || $0.uid == deviceArg }
            guard deviceArg == "default" || (device?.inputChannels ?? 0) > 0 else {
                throw CommandError("Unknown input. Use :inputs.")
            }
            let stereo = arguments.count > 2 && arguments[2] == "stereo"
            if let device, channel + (stereo ? 1 : 0) > device.inputChannels {
                throw CommandError("\(device.name) has only \(device.inputChannels) input channels.")
            }
            return .init(
                command: .setTrackInput(trackID: t.id, deviceUID: device?.uid, channel: channel - 1, stereo: stereo),
                trackIDs: [t.id])
        case "output":
            guard let argument = arguments.first,
                let device = snapshot.devices.first(where: { String($0.id) == argument || $0.uid == argument }),
                device.outputChannels > 0
            else { throw CommandError("Usage: :output deviceID|UID; use :outputs to list them.") }
            return .init(command: .setDevices(inputID: nil, outputID: device.id))
        case "cancel": return .init(command: .editor(.init(operation: "jobs.cancel")))
        case "takeover": return .init(command: .takeTransport)
        case "projects": return .init(command: .listProjects)
        case "load":
            guard !arguments.isEmpty else { throw CommandError("Usage: :load server-project-id; :projects lists IDs.") }
            return .init(command: .openProject(id: arguments.joined(separator: " ")))
        case "save":
            guard !arguments.isEmpty else { throw CommandError("Usage: :save project-name (server storage)") }
            return .init(command: .saveProject(name: arguments.joined(separator: " ")))
        case "export":
            guard !arguments.isEmpty else { throw CommandError("Usage: :export mix-name (server storage)") }
            return .init(command: .exportMix(name: arguments.joined(separator: " ")))
        case "effect": return try effect(arguments, track: requireTrack())
        default: throw CommandError("Unknown command '\(first)'. Type :help or press Tab after :.")
        }
    }
    private static func effect(_ arguments: [String], track: Track) throws -> TUICommandPlan {
        guard let action = arguments.first else {
            return .init(
                message: track.realtimeEffects.enumerated().map {
                    "\($0.offset + 1):\($0.element.kind.rawValue)\($0.element.enabled ? "" : "[bypass]")"
                }.joined(separator: " · "))
        }
        var edited = track
        if action == "add" {
            guard arguments.count == 2, let kind = RealtimeEffectKind(rawValue: arguments[1]) else {
                throw CommandError(
                    "Usage: :effect add \(RealtimeEffectKind.allCases.map(\.rawValue).joined(separator: "|"))")
            }
            edited.realtimeEffects.append(RealtimeEffect(kind: kind, parameters: kind.defaultParameters))
        } else {
            guard arguments.count >= 2, let number = Int(arguments[1]),
                edited.realtimeEffects.indices.contains(number - 1)
            else { throw CommandError("Use :effect to list effect numbers.") }
            let index = number - 1
            switch action {
            case "presets":
                return .init(
                    message: edited.realtimeEffects[index].kind.presets.enumerated().map {
                        "\($0.offset + 1): \($0.element.name)"
                    }.joined(separator: " · "))
            case "preset":
                let choices = edited.realtimeEffects[index].kind.presets
                let query = arguments.dropFirst(2).joined(separator: " ")
                let preset =
                    Int(query).flatMap { choices.indices.contains($0 - 1) ? choices[$0 - 1] : nil }
                    ?? choices.first { $0.name.lowercased() == query.lowercased() }
                guard let preset else {
                    throw CommandError(
                        "Use :effect presets \(index + 1) to list presets, then :effect preset \(index + 1) NUMBER|NAME."
                    )
                }
                edited.realtimeEffects[index].parameters = preset.values
            case "remove": edited.realtimeEffects.remove(at: index)
            case "bypass":
                guard arguments.count == 3, ["on", "off"].contains(arguments[2]) else {
                    throw CommandError("Usage: :effect bypass number on|off")
                }
                edited.realtimeEffects[index].enabled = arguments[2] == "off"
            case "set":
                guard arguments.count == 4, let value = Float(arguments[3]), value.isFinite,
                    let descriptor = edited.realtimeEffects[index].kind.parameters.first(where: {
                        $0.id == arguments[2]
                    })
                else {
                    throw CommandError(
                        "Usage: :effect set number parameter value; parameters: \(edited.realtimeEffects[index].kind.parameters.map(\.id).joined(separator: ", "))"
                    )
                }
                guard value >= descriptor.minimum, value <= descriptor.maximum else {
                    throw CommandError(
                        "\(descriptor.name) must be \(descriptor.minimum)…\(descriptor.maximum) \(descriptor.unit).")
                }
                edited.realtimeEffects[index].parameters[arguments[2]] = descriptor.clamped(value)
            default: throw CommandError("Usage: :effect [add|remove|bypass|set] ...")
            }
        }
        return .init(command: .replaceTracks(tracks: [edited], name: nil, gestureID: nil), trackIDs: [track.id])
    }
    struct CommandError: Error, CustomStringConvertible {
        let description: String
        init(_ text: String) { description = text }
    }
}
