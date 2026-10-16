import Foundation
import SoundroomCore

/// Client-owned navigation. No motion changes the hardware transport.
struct TUIClip: Equatable { var id: UUID; var name: String; var start: Int; var length: Int; var end: Int { start + length } }
struct TUITrack: Equatable {
    var id: UUID; var name: String; var clips: [TUIClip] = []
    var muted = false; var solo = false; var armed = false
    var gain: Double = 1; var pan: Double = 0; var effects: [String] = []
    var color = 0
    var inserts: [RealtimeEffect] = []
    var input = "Default input · ch 1"
    var clipIDSet: Set<UUID>?
}
struct TUISession: Equatable {
    var name = "Connecting…"; var tracks: [TUITrack] = []; var revision: UInt64 = 0
    var frame = 0; var playing = false; var recording = false; var sampleRate = 48_000.0
    var timing = MusicalTiming.standard
    var devices: [TUIDevice] = []
    var outputID: UInt32?
    var loopStart: Int?
    var loopEnd: Int?
    var operatorName: String?
    var end: Int { tracks.reduce(0) { max($0, $1.clips.reduce(0) { max($0, $1.end) }) } }
}
enum TUIIntent: Equatable {
    case togglePlayback, play, stop, record, startRecording, seek(Int), undo, redo
    case delete(trackID: UUID, clipIDs: [UUID])
    case paste(sourceTrackID: UUID, clipIDs: [UUID], destinationTrackID: UUID, frame: Int)
    case command(String), quit
}
struct TUIDevice: Equatable { var id: UInt32; var name: String; var inputs: Int; var outputs: Int }
struct TUIModel {
    enum Panel: String, CaseIterable { case mixer = "Mixer", effects = "Effects", routing = "Routing" }
    var panel: Panel = .mixer
    var inspectorFocused = false
    var inspectorIndex = 0
    var effectIndex = 0
    var trackScroll: Int?
    var inspectorScroll = 0
    var commandHistory: [String] = []
    private var mouseAnchor: (track: UUID, frame: Int, x: Int)?

    enum Mode: Equatable { case normal, visual, clips, command, help }
    var session = TUISession()
    var trackID: UUID?
    var cursor = 0
    var stepFrames = 12_000
    var timebase = TimelineTimebase.time
    var beatDivision: Double = 1
    var beatSnap = true
    var gridQuarterNotes: Double { beatDivision == 0 ? session.timing.quarterNotesPerBar : beatDivision }
    var navigationStep: Int { timebase == .beats ? max(1, session.timing.frame(atQuarterNote: gridQuarterNotes, sampleRate: session.sampleRate)) : stepFrames }
    func position(_ frame: Int, ticks: Bool = true) -> String { timebase == .beats ? session.timing.position(frame, sampleRate: session.sampleRate, ticks: ticks) : TUIRenderer.time(frame, rate: session.sampleRate) }
    func snapped(_ frame: Int) -> Int { timebase == .beats && beatSnap ? session.timing.snapped(frame, quarterNoteDivision: gridQuarterNotes, sampleRate: session.sampleRate) : frame }
    func coordinateFrame(_ text: String) -> Int? {
        if timebase == .beats {
            let position = text.contains(".") ? text : text + ".1"
            guard let frame = session.timing.frame(atPosition: position, sampleRate: session.sampleRate), frame <= maximumFrame else { return nil }; return frame
        }
        guard let seconds = Double(text), seconds.isFinite, (0...86400).contains(seconds) else { return nil }
        return Int(seconds * session.sampleRate)
    }
    var viewStart = 0
    var viewSpan = 480_000
    var mode: Mode = .normal
    var anchor: Int?
    var clipIDs: Set<UUID> = []
    var clipboard: (track: UUID, clips: [UUID])?
    var commandText = ""
    var status = "Connecting…"
    var connected = false
    var role = "operator"
    private(set) var countText = ""
    private var pending: Character?
    static let commands = ["arm", "cancel", "grid", "meter", "tempo", "timebase", "clips", "combine", "delete", "duplicate-track", "fit", "effect", "export", "help", "input", "inputs", "load", "loop", "move", "mute", "nudge", "new-track", "output", "outputs", "pan", "play", "play-track", "projects", "quit", "record", "redo", "remove-track", "rename", "save", "seek", "solo", "split", "stop", "takeover", "tracks", "trim", "undo", "volume", "web-clients", "web-pair", "web-revoke", "zoom"]
    var maximumFrame: Int { Int(86_400 * session.sampleRate) }
    var selectedTrack: TUITrack? { session.tracks.first { $0.id == trackID } }
    var trackIndex: Int { session.tracks.firstIndex { $0.id == trackID } ?? 0 }
    var selectedClips: [TUIClip] {
        guard let track = selectedTrack else { return [] }
        if mode == .visual, let anchor {
            let low = min(anchor, cursor), high = max(anchor, cursor)
            return track.clips.filter { $0.start <= high && $0.end > low }
        }
        if !clipIDs.isEmpty { return track.clips.filter { clipIDs.contains($0.id) } }
        return track.clips.filter { $0.start <= cursor && $0.end > cursor }
    }
    mutating func update(_ snapshot: TUISession) {
        session = snapshot
        if !snapshot.tracks.contains(where: { $0.id == trackID }) { trackID = snapshot.tracks.first?.id }
        let liveIDs = selectedTrack?.clipIDSet ?? Set(selectedTrack?.clips.map(\.id) ?? [])
        clipIDs.formIntersection(liveIDs)
        effectIndex = max(0, min(effectIndex, max(0, (selectedTrack?.inserts.count ?? 0) - 1)))
        if panel == .effects { inspectorIndex = max(0, min(inspectorIndex, max(0, (activeEffect?.kind.parameters.count ?? 0) - 1))) }
    }
    mutating func handle(_ receivedKey: Character) -> [TUIIntent] {
        let arrows: [Character: Character] = ["\u{f700}": "k", "\u{f701}": "j", "\u{f702}": "h", "\u{f703}": "l"]
        let key = mode == .command ? receivedKey : (arrows[receivedKey] ?? receivedKey)
        if mode == .help { if key == "\u{1b}" || key == "?" || key == "q" { mode = .normal }; return [] }
        if mode == .command {
            if key == "\u{1b}" { mode = .normal; commandText = ""; return [] }
            if key == "\r" || key == "\n" { let line = commandText; if !line.isEmpty { commandHistory.append(line); commandHistory = Array(commandHistory.suffix(50)) }; mode = .normal; commandText = ""; return execute(line) }
            if key == "\u{7f}" || key == "\u{8}" { if !commandText.isEmpty { commandText.removeLast() }; return [] }
            if key == "\t" { complete(); return [] }
            if key == "\u{10}" || key == "\u{f700}" { commandText = commandHistory.last ?? commandText; return [] }
            if key == "\u{f701}" { commandText = ""; return [] }
            if key == "\u{f702}" || key == "\u{f703}" { return [] }
            if key.unicodeScalars.allSatisfy({ $0.value >= 32 }) && commandText.utf8.count < 4096 { commandText.append(key) }
            return []
        }
        if key == "\u{1b}" { mode = .normal; anchor = nil; clipIDs = []; pending = nil; countText = ""; return [] }
        if key.isNumber && key.isASCII { if countText.count < 4 { countText.append(key) }; return [] }
        let count = max(1, min(9999, Int(countText) ?? 1)); countText = ""
        if let prefix = pending {
            pending = nil
            if prefix == "g" && key == "g" { cursor = 0; ensureVisible(); return [] }
            if prefix == "d" && key == "d" { return deletion() }
        }
        if key == "\t" { inspectorFocused.toggle(); inspectorIndex = 0; return [] }
        if inspectorFocused, [Character("j"), "k", "h", "l", "+", "-", "\r", "\n"].contains(key) {
            return inspectorKey(key)
        }
        switch key {
        case "j", "k":
            if !session.tracks.isEmpty { let next = min(session.tracks.count - 1, max(0, trackIndex + (key == "j" ? count : -count))); trackID = session.tracks[next].id; clipIDs = []; trackScroll = nil; effectIndex = 0; inspectorIndex = 0; inspectorScroll = 0 }
        case "h", "l":
            if timebase == .beats {
                let q = session.timing.quarterNotes(at: cursor, sampleRate: session.sampleRate)
                let grid = q / gridQuarterNotes
                let tolerance = session.timing.valid.tempo / 60 / session.sampleRate / gridQuarterNotes
                let next = key == "l" ? floor(grid + tolerance) + Double(count) : ceil(grid - tolerance) - Double(count)
                cursor = session.timing.frame(atQuarterNote: max(0, next) * gridQuarterNotes, sampleRate: session.sampleRate)
            } else { cursor = max(0, cursor + (key == "l" ? 1 : -1) * count * stepFrames) }
            ensureVisible()
        case "w", "b":
            let boundaries = Set(selectedTrack?.clips.flatMap { [$0.start, $0.end] } ?? []).sorted()
            for _ in 0..<count { if key == "w" { cursor = boundaries.first { $0 > cursor } ?? session.end } else { cursor = boundaries.last { $0 < cursor } ?? 0 } }; ensureVisible()
        case "g", "d": pending = key
        case "G": cursor = session.end; ensureVisible()
        case "v": if mode == .visual { mode = .normal; anchor = nil } else { mode = .visual; anchor = cursor; clipIDs = [] }
        case "V": mode = .clips; clipIDs = Set(selectedClips.map(\.id)); status = "Clip selection: y copies; dd deletes; Escape clears."
        case "y":
            if let trackID, !selectedClips.isEmpty { clipboard = (trackID, selectedClips.map(\.id)); status = "Copied \(selectedClips.count) clip reference(s)."; mode = .normal; anchor = nil }
            else { status = "Place the cursor on a clip or select a range first." }
        case "p":
            guard let clipboard, let trackID else { status = "Clipboard is empty."; return [] }
            return [.paste(sourceTrackID: clipboard.track, clipIDs: clipboard.clips, destinationTrackID: trackID, frame: cursor)]
        case "B": timebase = timebase == .time ? .beats : .time
        case "m": return [.command("mute")]
        case "s": return [.command("solo")]
        case "a": return [.command("arm")]
        case "x": return [.command("split")]
        case "C": return [.command("combine")]
        case "L": return [.command("loop")]
        case "F": fit()
        case "e":
            panel = TUIModel.Panel.allCases[(TUIModel.Panel.allCases.firstIndex(of: panel)! + 1) % TUIModel.Panel.allCases.count]; inspectorFocused = true; inspectorScroll = 0; inspectorIndex = 0
        case "J", "K":
            guard inspectorFocused && panel == .effects else { return [] }
            effectIndex = max(0, min(max(0, (selectedTrack?.inserts.count ?? 0) - 1), effectIndex + (key == "J" ? 1 : -1))); inspectorIndex = 0; inspectorScroll = 0
        case "f": panel = .effects; inspectorFocused = true; inspectorIndex = 0
        case "t": inspectorFocused = false
        case "[", "]": let step = timebase == .beats ? gridQuarterNotes / session.timing.quarterNotesPerBeat : Double(stepFrames) / session.sampleRate; return [.command("nudge \(key == "[" ? -step : step)")]
        case "u": return [.undo]
        case "\u{12}": return [.redo]
        case " ": return [.togglePlayback]
        case "\r", "\n": return [.seek(cursor)]
        case "r": return [.record]
        case ":": mode = .command; commandText = ""
        case "?": mode = .help
        case "+", "=": viewSpan = max(stepFrames * 4, viewSpan / 2); ensureVisible()
        case "-": viewSpan = min(maximumFrame, viewSpan * 2); ensureVisible()
        case "q": return [.quit]
        default: break
        }
        return []
    }
    mutating func execute(_ line: String) -> [TUIIntent] {
        let pieces = line.split(maxSplits: 1, whereSeparator: \.isWhitespace)
        guard let first = pieces.first else { return [] }
        let name = first.lowercased(), argument = pieces.count > 1 ? String(pieces[1]) : ""
        switch name {
        case "q", "quit": return [.quit]
        case "h", "help": mode = .help; return []
        case "fit": fit(); return []
        case "tracks": status = "\(session.tracks.count) tracks; j/k changes only your selection."; return []
        case "clips": status = selectedTrack?.clips.map { "\($0.name) @\(String(format: "%.2f", Double($0.start) / session.sampleRate))s" }.joined(separator: " · ") ?? "No track selected."; return []
        case "play": return [.play]
        case "stop": return [.stop]
        case "record": return [.startRecording]
        case "undo": return [.undo]
        case "redo": return [.redo]
        case "delete": return deletion()
        case "timebase":
            guard let value = TimelineTimebase.allCases.first(where: { $0.rawValue.lowercased() == argument.lowercased() }) else { status = "Usage: :timebase time|beats"; return [] }
            timebase = value; status = value == .beats ? "Bars.beats.ticks · 960 ticks/quarter note · B toggles time" : "Seconds · B toggles beats"; return []
        case "grid":
            let divisions: [String: Double] = ["1/4": 1, "1/8": 0.5, "1/16": 0.25, "1/32": 0.125, "triplet": 1.0/3, "bar": 0]
            if argument == "off" { beatSnap = false; return [] }
            guard let value = divisions[argument] else { status = "Usage: :grid 1/4|1/8|1/16|1/32|triplet|bar|off"; return [] }
            beatDivision = value; beatSnap = true; return []
        case "seek":
            guard let frame = coordinateFrame(argument) else { status = timebase == .beats ? "Usage: :seek bar.beat[.tick]" : "Usage: :seek seconds (0…86400)"; return [] }
            cursor = frame; ensureVisible(); return [.seek(cursor)]
        case "zoom":
            guard let seconds = Double(argument), seconds.isFinite, (0.1...86_400).contains(seconds) else { status = "Usage: :zoom seconds (0.1…86400); changes only your view."; return [] }
            viewSpan = Int(seconds * session.sampleRate); ensureVisible(); return []
        default: return [.command(line)]
        }
    }
    mutating func ensureVisible() {
        cursor = max(0, min(maximumFrame, cursor)); viewSpan = max(1, min(maximumFrame, viewSpan))
        if cursor < viewStart { viewStart = cursor }
        else if cursor >= viewStart + viewSpan { viewStart = max(0, cursor - viewSpan + stepFrames) }
    }
    mutating func fit() {
        viewStart = 0; viewSpan = max(stepFrames * 4, session.end + stepFrames)
    }
    var activeEffect: RealtimeEffect? {
        guard let track = selectedTrack, track.inserts.indices.contains(effectIndex) else { return nil }; return track.inserts[effectIndex]
    }
    mutating func prompt(_ text: String) { mode = .command; commandText = text; inspectorFocused = false }
    private mutating func inspectorKey(_ key: Character) -> [TUIIntent] {
        if key == "j" || key == "k" {
            let limit = panel == .effects ? max(0, (activeEffect?.kind.parameters.count ?? 0) - 1) : panel == .routing ? max(0, session.devices.count - 1) : 1
            inspectorIndex = max(0, min(limit, inspectorIndex + (key == "j" ? 1 : -1))); return []
        }
        let direction: Float = key == "h" || key == "-" ? -1 : 1
        if panel == .effects, let effect = activeEffect, effect.kind.parameters.indices.contains(inspectorIndex) {
            let parameter = effect.kind.parameters[inspectorIndex]
            let value = effect.parameters[parameter.id] ?? parameter.defaultValue
            if key == "\r" || key == "\n" { prompt("effect set \(effectIndex + 1) \(parameter.id) "); return [] }
            let next = parameter.choices != nil || parameter.integer ? parameter.clamped(value + direction) : parameter.physical(parameter.normalized(value) + direction * 0.02)
            return [.command("effect set \(effectIndex + 1) \(parameter.id) \(next)")]
        }
        if panel == .mixer, let track = selectedTrack {
            if key == "\r" || key == "\n" { prompt(inspectorIndex == 0 ? "volume " : "pan "); return [] }
            return [.command(inspectorIndex == 0 ? "volume \(max(0, min(4, track.gain + Double(direction) * 0.05)))" : "pan \(max(-1, min(1, track.pan + Double(direction) * 0.05)))")]
        }
        if panel == .routing, session.devices.indices.contains(inspectorIndex), key == "\r" || key == "\n" {
            let device = session.devices[inspectorIndex]
            if device.inputs > 0 { return [.command("input \(device.id)")] }
        }
        return []
    }
    mutating func handleMouse(_ event: TUIMouseEvent, frame: TUIFrame) -> [TUIIntent] {
        guard mode != .help && mode != .command else { return [] }
        if event.action == .scrollUp || event.action == .scrollDown {
            let direction = event.action == .scrollUp ? -1 : 1
            if event.control { viewSpan = direction < 0 ? max(stepFrames * 4, viewSpan / 2) : min(maximumFrame, viewSpan * 2); ensureVisible() }
            else if event.shift { viewStart = max(0, min(maximumFrame - viewSpan, viewStart + direction * max(1, viewSpan / 8))) }
            else if frame.inspectorContains(x: event.x, y: event.y) { inspectorFocused = false; inspectorScroll = max(0, inspectorScroll + direction * 3) }
            else { trackScroll = max(0, min(max(0, session.tracks.count - frame.visibleTracks), (trackScroll ?? frame.firstTrack) + direction * 3)) }
            return []
        }
        if event.action == .up { mouseAnchor = nil; return [] }
        if event.action == .drag, let drag = mouseAnchor {
            let position = snapped(frame.timelineFrame(at: event.x, model: self))
            guard event.x != drag.x else { return [] }
            trackID = drag.track; mode = .visual; anchor = drag.frame; cursor = position; clipIDs = []; return []
        }
        guard event.action == .down, event.button == 0, let hit = frame.hits.last(where: { $0.contains(event.x, event.y) }) else { return [] }
        switch hit.action {
        case .key(let key): return handle(key)
        case .command(let text): return execute(text)
        case .prompt(let text): prompt(text)
        case .panel(let value): panel = value; inspectorScroll = 0; inspectorIndex = 0; inspectorFocused = true
        case .track(let id): trackID = id; clipIDs = []; anchor = nil; mode = .normal; inspectorFocused = false; effectIndex = 0; inspectorScroll = 0
        case .trackCommand(let id, let text): if trackID != id { effectIndex = 0; inspectorIndex = 0; inspectorScroll = 0 }; trackID = id; inspectorFocused = false; return execute(text)
        case .timeline(let id):
            let previousTrack = trackID; trackID = id; inspectorFocused = false
            let position = snapped(frame.timelineFrame(at: event.x, model: self)); cursor = position
            let raw = frame.timelineFrame(at: event.x, model: self)
            let clip = selectedTrack?.clips.last { $0.start <= raw && $0.end > raw }
            if event.shift, let clip { if previousTrack != id { clipIDs = [] }; if clipIDs.contains(clip.id) { clipIDs.remove(clip.id) } else { clipIDs.insert(clip.id) }; mode = .clips }
            else { clipIDs = clip.map { [$0.id] } ?? []; anchor = nil; mode = clip == nil ? .normal : .clips }
            if !event.shift { mouseAnchor = (id, position, event.x) }
        case .effect(let index): effectIndex = index; inspectorIndex = 0; inspectorScroll = 0; inspectorFocused = true
        case .parameter(let number, let id):
            guard let effect = activeEffect, let parameter = effect.kind.parameters.first(where: { $0.id == id }) else { return [] }
            inspectorFocused = true; inspectorIndex = effect.kind.parameters.firstIndex(where: { $0.id == id }) ?? 0
            let fraction = Float(event.x - hit.x) / Float(max(1, hit.width - 1))
            return [.command("effect set \(number) \(id) \(parameter.physical(fraction))")]
        case .gain, .pan:
            inspectorFocused = true; inspectorIndex = hit.action == .gain ? 0 : 1
            let fraction = Double(event.x - hit.x) / Double(max(1, hit.width - 1))
            return [.command(hit.action == .gain ? "volume \(fraction * 4)" : "pan \(fraction * 2 - 1)")]
        }
        return []
    }
    private mutating func deletion() -> [TUIIntent] {
        guard let trackID, !selectedClips.isEmpty else { status = "No clip selected. dd deletes clips, never a track."; return [] }
        return [.delete(trackID: trackID, clipIDs: selectedClips.map(\.id))]
    }
    private mutating func complete() {
        guard !commandText.contains(where: \.isWhitespace) else { return }
        let matches = Self.commands.filter { $0.hasPrefix(commandText.lowercased()) }
        if matches.count == 1 { commandText = matches[0] + " " }
        else if !matches.isEmpty { status = matches.joined(separator: "  ") }
    }
}
