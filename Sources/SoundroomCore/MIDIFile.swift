import Foundation

public enum MIDIFileError: LocalizedError, Equatable {
    case invalid, unsupportedTimeDivision, variableTempo, tooLarge, noNotes, resourceLimit, unsupportedTempo
    public var errorDescription: String? {
        switch self {
        case .invalid: return "This MIDI file is incomplete or unsupported. Use a standard format 0 or 1 MIDI file."
        case .unsupportedTimeDivision: return "SMPTE-timed MIDI is not supported. Export MIDI using beats and ticks."
        case .variableTempo:
            return "This file changes tempo. The designer uses one tempo; export a constant-tempo MIDI file."
        case .tooLarge:
            return
                "This MIDI selection exceeds the designer limit of 256 notes and 64 beats. Choose a shorter section or one MIDI part."
        case .noNotes:
            return "This MIDI selection has no playable notes. Choose a MIDI part and beat range containing notes."
        case .resourceLimit:
            return "This MIDI file is too large to import. Export a smaller MIDI arrangement (up to 4 MB)."
        case .unsupportedTempo:
            return
                "This MIDI file has a tempo outside the designer range of 30–300 BPM. Export it at a supported tempo."
        }
    }
}

/// A MIDI part retains its source track and channel, so a multi-instrument arrangement
/// can be imported intentionally into the designer's current instrument.
public struct MIDIFilePart: Identifiable, Sendable {
    public var id: Int
    public var name: String
    public var notes: [SoundDesignNote]
    public var controllers: [SoundDesignController]
}

public struct MIDIFileContent: Sendable {
    public var parts: [MIDIFilePart]
    public var bpm: Double
    public var beats: Double

    /// Explicitly extracts a reviewed beat range. Notes crossing either boundary are
    /// clipped; controller values active at the start are carried into the new pattern.
    public func pattern(partID: Int? = nil, startBeat: Double = 0, length: Double? = nil) throws -> SoundDesignPattern {
        let length = length ?? beats
        guard startBeat.isFinite, length.isFinite, startBeat >= 0, startBeat < beats,
            length > 0, length <= 64, startBeat + length <= beats + 0.000001
        else { throw MIDIFileError.tooLarge }
        let chosen = parts.filter { partID == nil || $0.id == partID }
        let end = startBeat + length
        let notes = chosen.flatMap(\.notes).compactMap { source -> SoundDesignNote? in
            let noteEnd = min(end, source.startBeat + source.durationBeats)
            let noteStart = max(startBeat, source.startBeat)
            guard noteEnd > noteStart else { return nil }
            var note = source
            note.startBeat = noteStart - startBeat
            note.durationBeats = noteEnd - noteStart
            return note
        }.sorted { $0.startBeat == $1.startBeat ? $0.pitch < $1.pitch : $0.startBeat < $1.startBeat }
        guard !notes.isEmpty else { throw MIDIFileError.noNotes }
        guard notes.count <= 256 else { throw MIDIFileError.tooLarge }
        var controllers: [SoundDesignController] = []
        for part in chosen {
            for kind in SoundDesignControllerKind.allCases {
                if let previous = part.controllers.last(where: { $0.kind == kind && $0.beat < startBeat }) {
                    var carried = previous
                    carried.beat = 0
                    controllers.append(carried)
                }
            }
            controllers += part.controllers.filter { $0.beat >= startBeat && $0.beat <= end }.map {
                var point = $0
                point.beat -= startBeat
                return point
            }
        }
        guard controllers.count <= 4096 else { throw MIDIFileError.tooLarge }
        var pattern = SoundDesignPattern(notes: notes, bpm: bpm, beats: length)
        pattern.controllers = controllers.sorted { $0.beat < $1.beat }
        return pattern
    }
}

/// Standard MIDI interchange, preserving pitch, velocity, rests, simultaneous notes,
/// note durations, sustain/modulation/expression/pitch-bend controllers, and a constant
/// tempo. Instrument programs are ignored: Soundroom owns its native synth.
public enum MIDIFile {
    public static func decode(_ data: Data) throws -> SoundDesignPattern {
        try inspect(data).pattern()
    }

    /// Parses the complete file before any destructive designer edit. Length and note
    /// count limits apply to the selected excerpt, rather than the whole arrangement.
    public static func inspect(_ data: Data) throws -> MIDIFileContent {
        guard data.count <= 4_194_304 else { throw MIDIFileError.resourceLimit }
        var reader = Reader(bytes: Array(data))
        guard try reader.ascii(4) == "MThd" else { throw MIDIFileError.invalid }
        let headerLength = try reader.uint32()
        guard headerLength >= 6 else { throw MIDIFileError.invalid }
        let format = try reader.uint16()
        let trackCount = try reader.uint16()
        let division = try reader.uint16()
        guard format <= 1, trackCount > 0, trackCount <= 256, (format != 0 || trackCount == 1), division > 0 else {
            throw MIDIFileError.invalid
        }
        guard division & 0x8000 == 0 else { throw MIDIFileError.unsupportedTimeDivision }
        try reader.skip(headerLength - 6)
        var tempos: [(Int, Int)] = []
        var events: [Event] = []
        var controllers: [Int: [SoundDesignController]] = [:]
        var trackNames: [Int: String] = [:]
        var sequence = 0
        var lastTick = 0
        for track in 0..<trackCount {
            guard try reader.ascii(4) == "MTrk" else { throw MIDIFileError.invalid }
            let count = try reader.uint32()
            var chunk = Reader(bytes: try reader.take(count))
            var tick = 0
            var running: UInt8 = 0
            var ended = false
            while !chunk.atEnd {
                let delta = try chunk.variable()
                let total = tick.addingReportingOverflow(delta)
                guard !total.overflow, total.partialValue <= Int(division) * 65_536 else {
                    throw MIDIFileError.resourceLimit
                }
                tick = total.partialValue
                lastTick = max(lastTick, tick)
                var status = try chunk.byte()
                var first: UInt8?
                if status < 0x80 {
                    guard running >= 0x80 && running < 0xF0 else { throw MIDIFileError.invalid }
                    first = status
                    status = running
                } else if status < 0xF0 {
                    running = status
                } else {
                    running = 0
                }
                if status == 0xFF {
                    let kind = try chunk.byte()
                    let length = try chunk.variable()
                    let payload = try chunk.take(length)
                    if kind == 0x51 {
                        guard length == 3 else { throw MIDIFileError.invalid }
                        let tempo = Int(payload[0]) << 16 | Int(payload[1]) << 8 | Int(payload[2])
                        guard tempo > 0 else { throw MIDIFileError.invalid }
                        tempos.append((tick, tempo))
                    }
                    if kind == 0x03, let name = String(bytes: payload, encoding: .utf8), !name.isEmpty {
                        trackNames[track] = name
                    }
                    if kind == 0x2F {
                        guard length == 0, chunk.atEnd else { throw MIDIFileError.invalid }
                        ended = true
                        break
                    }
                } else if status == 0xF0 || status == 0xF7 {
                    try chunk.skip(chunk.variable())
                } else {
                    let command = status & 0xF0
                    guard command >= 0x80 && command <= 0xE0 else { throw MIDIFileError.invalid }
                    let one = try first ?? chunk.byte()
                    let two: UInt8 = command == 0xC0 || command == 0xD0 ? 0 : try chunk.byte()
                    guard one < 128 && two < 128 else { throw MIDIFileError.invalid }
                    if command == 0x90 || command == 0x80 {
                        events.append(
                            Event(
                                tick: tick, track: track, channel: Int(status & 15), pitch: Int(one),
                                velocity: Int(two), on: command == 0x90 && two > 0, order: sequence))
                        sequence += 1
                        guard events.count <= 131_072 else { throw MIDIFileError.resourceLimit }
                    } else if command == 0xB0 || command == 0xE0 {
                        let kind: SoundDesignControllerKind?
                        var value = Double(two) / 127
                        if command == 0xE0 {
                            kind = .pitchBend
                            let raw = Int(one) | Int(two) << 7
                            value = raw <= 8192 ? Double(raw) / 16384 : 0.5 + Double(raw - 8192) / 16382
                        } else {
                            switch one {
                            case 1: kind = .modulation
                            case 7, 11: kind = .expression
                            case 64: kind = .sustain
                            default: kind = nil
                            }
                        }
                        if let kind {
                            let part = track * 16 + Int(status & 15)
                            controllers[part, default: []].append(
                                SoundDesignController(kind: kind, beat: Double(tick) / Double(division), value: value))
                            guard controllers[part]!.count <= 65_536 else { throw MIDIFileError.resourceLimit }
                        }
                    }
                }
            }
            guard ended else { throw MIDIFileError.invalid }
        }
        guard reader.atEnd else { throw MIDIFileError.invalid }
        let firstTempo = tempos.sorted { $0.0 < $1.0 }.first
        let tempo = firstTempo?.1 ?? 500_000
        if let firstTempo, firstTempo.0 > 0 && tempo != 500_000 { throw MIDIFileError.variableTempo }
        guard tempos.allSatisfy({ $0.1 == tempo }) else { throw MIDIFileError.variableTempo }
        let bpm = 60_000_000 / Double(tempo)
        guard (30...300).contains(bpm) else { throw MIDIFileError.unsupportedTempo }
        struct Key: Hashable {
            var track: Int
            var channel: Int
            var pitch: Int
        }
        final class PendingNotes {
            var starts: [Event] = []
            var next = 0
        }
        var active: [Key: PendingNotes] = [:]
        var notes: [Int: [SoundDesignNote]] = [:]
        let ordered = events.sorted { $0.tick == $1.tick ? $0.order < $1.order : $0.tick < $1.tick }
        for event in ordered {
            let key = Key(track: event.track, channel: event.channel, pitch: event.pitch)
            if event.on {
                let pending = active[key] ?? PendingNotes()
                pending.starts.append(event)
                active[key] = pending
            } else if let pending = active[key], pending.next < pending.starts.count {
                let start = pending.starts[pending.next]
                pending.next += 1
                if event.tick > start.tick {
                    notes[event.track * 16 + event.channel, default: []].append(
                        SoundDesignNote(
                            pitch: event.pitch, velocity: Double(start.velocity) / 127,
                            startBeat: Double(start.tick) / Double(division),
                            durationBeats: Double(event.tick - start.tick) / Double(division)))
                }
            }
        }
        // Missing note-offs are rejected rather than inventing a long or stuck note.
        guard active.values.allSatisfy({ $0.next == $0.starts.count }) else { throw MIDIFileError.invalid }
        guard !notes.isEmpty else { throw MIDIFileError.noNotes }
        let last = max(
            notes.values.flatMap { $0 }.map { $0.startBeat + $0.durationBeats }.max() ?? 0,
            Double(lastTick) / Double(division))
        let parts = notes.keys.sorted().map { id in
            MIDIFilePart(
                id: id,
                name:
                    "\(trackNames[id / 16] ?? "Track \(id / 16 + 1)") · \(id % 16 == 9 ? "Drums" : "Channel \(id % 16 + 1)")",
                notes: notes[id]!.sorted {
                    $0.startBeat == $1.startBeat ? $0.pitch < $1.pitch : $0.startBeat < $1.startBeat
                },
                controllers: (controllers[id] ?? []).sorted { $0.beat < $1.beat })
        }
        return MIDIFileContent(parts: parts, bpm: bpm, beats: max(0.25, last))
    }

    public static func encode(_ pattern: SoundDesignPattern) -> Data {
        let ppq = 480
        let safe = SoundDesignPattern(notes: pattern.notes, bpm: pattern.bpm, beats: pattern.beats)
        let tempo = Int((60_000_000 / safe.bpm).rounded())
        var track: [UInt8] = [
            0, 0xFF, 0x51, 3, UInt8((tempo >> 16) & 255), UInt8((tempo >> 8) & 255), UInt8(tempo & 255),
        ]
        struct Output {
            var tick: Int
            var bytes: [UInt8]
            var order: Int
        }
        var events: [Output] = []
        var articulationPattern = pattern
        articulationPattern.controllers = []
        for note in articulationPattern.performedNotes().prefix(256) {
            guard note.startBeat.isFinite, note.startBeat >= 0, note.startBeat < safe.beats,
                note.durationBeats.isFinite, note.durationBeats > 0, note.velocity.isFinite, note.velocity > 0
            else { continue }
            let start = Int((note.startBeat * Double(ppq)).rounded())
            let end = max(
                start + 1, Int((min(safe.beats, note.startBeat + note.durationBeats) * Double(ppq)).rounded()))
            let pitch = max(0, min(127, note.pitch))
            let velocity = max(1, min(127, Int((note.velocity * 127).rounded())))
            events.append(Output(tick: start, bytes: [0x90, UInt8(pitch), UInt8(velocity)], order: 2))
            events.append(Output(tick: end, bytes: [0x80, UInt8(pitch), 0], order: 0))
        }
        for controller in pattern.controllers.prefix(4096) {
            guard controller.beat.isFinite, controller.beat >= 0, controller.beat <= safe.beats,
                controller.value.isFinite
            else { continue }
            let value = max(0, min(1, controller.value))
            let tick = Int((controller.beat * Double(ppq)).rounded())
            let bytes: [UInt8]
            if controller.kind == .pitchBend {
                let raw = Int((value <= 0.5 ? value * 16384 : 8192 + (value - 0.5) * 16382).rounded())
                bytes = [0xE0, UInt8(raw & 127), UInt8((raw >> 7) & 127)]
            } else {
                let cc: UInt8 = controller.kind == .sustain ? 64 : controller.kind == .modulation ? 1 : 11
                bytes = [0xB0, cc, UInt8((value * 127).rounded())]
            }
            events.append(Output(tick: tick, bytes: bytes, order: 1))
        }
        events.sort { a, b in
            if a.tick != b.tick { return a.tick < b.tick }
            if a.order != b.order { return a.order < b.order }
            return a.bytes.lexicographicallyPrecedes(b.bytes)
        }
        var previous = 0
        for event in events {
            track += variable(event.tick - previous)
            track += event.bytes
            previous = event.tick
        }
        track += variable(max(0, Int((safe.beats * Double(ppq)).rounded()) - previous)) + [0xFF, 0x2F, 0]
        var output = Array("MThd".utf8) + bigEndian(6, bytes: 4) + [0, 0, 0, 1] + bigEndian(ppq, bytes: 2)
        output += Array("MTrk".utf8) + bigEndian(track.count, bytes: 4) + track
        return Data(output)
    }
    private struct Event {
        var tick: Int
        var track: Int
        var channel: Int
        var pitch: Int
        var velocity: Int
        var on: Bool
        var order: Int
    }
    private static func bigEndian(_ value: Int, bytes: Int) -> [UInt8] {
        (0..<bytes).reversed().map { UInt8((value >> ($0 * 8)) & 255) }
    }
    private static func variable(_ value: Int) -> [UInt8] {
        var value = max(0, value)
        var result = [UInt8(value & 127)]
        value >>= 7
        while value > 0 {
            result.insert(UInt8(value & 127) | 128, at: 0)
            value >>= 7
        }
        return result
    }
    private struct Reader {
        var bytes: [UInt8]
        var index = 0
        var atEnd: Bool { index >= bytes.count }
        mutating func byte() throws -> UInt8 {
            guard index < bytes.count else { throw MIDIFileError.invalid }
            defer { index += 1 }
            return bytes[index]
        }
        mutating func take(_ count: Int) throws -> [UInt8] {
            guard count >= 0, count <= bytes.count - index else { throw MIDIFileError.invalid }
            defer { index += count }
            return Array(bytes[index..<index + count])
        }
        mutating func skip(_ count: Int) throws { _ = try take(count) }
        mutating func ascii(_ count: Int) throws -> String { String(bytes: try take(count), encoding: .ascii) ?? "" }
        mutating func uint16() throws -> Int {
            let data = try take(2)
            return Int(data[0]) << 8 | Int(data[1])
        }
        mutating func uint32() throws -> Int {
            let data = try take(4)
            return data.reduce(0) { $0 << 8 | Int($1) }
        }
        mutating func variable() throws -> Int {
            var result = 0
            for _ in 0..<4 {
                let byte = try byte()
                result = result << 7 | Int(byte & 127)
                if byte & 128 == 0 { return result }
            }
            throw MIDIFileError.invalid
        }
    }
}
