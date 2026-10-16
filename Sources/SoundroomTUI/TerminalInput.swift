import Foundation

struct TUIMouseEvent: Equatable {
    enum Action: Equatable { case down, up, drag, scrollUp, scrollDown }
    var action: Action
    var x: Int
    var y: Int
    var button = 0
    var shift = false
    var control = false
}
enum TerminalInputEvent: Equatable { case key(Character), mouse(TUIMouseEvent), paste(String) }

/// Incremental and bounded: split reads must never turn mouse coordinates into keystrokes.
struct TerminalInputDecoder {
    private var sequence: [UInt8] = []
    private var utf8: [UInt8] = []
    private var pasteBytes: [UInt8]?
    private var pasteSuffix: [UInt8] = []
    var hasPendingEscape: Bool { !sequence.isEmpty }
    mutating func feed(_ bytes: [UInt8]) -> [TerminalInputEvent] {
        var events: [TerminalInputEvent] = []
        for byte in bytes {
            if pasteBytes != nil {
                pasteSuffix.append(byte)
                let terminator: [UInt8] = [27, 91, 50, 48, 49, 126]
                while !pasteSuffix.isEmpty && !terminator.starts(with: pasteSuffix) {
                    if pasteBytes!.count < 4096 { pasteBytes!.append(pasteSuffix.removeFirst()) }
                    else { pasteSuffix.removeFirst() }
                }
                if pasteSuffix == terminator {
                    events.append(.paste(String(decoding: pasteBytes!, as: UTF8.self))); pasteBytes = nil; pasteSuffix = []
                }
                continue
            }
            if !sequence.isEmpty {
                sequence.append(byte)
                if sequence.count == 2 && byte != 91 && byte != 79 {
                    sequence = []; events.append(.key("\u{1b}")); consume(byte, into: &events)
                } else if sequence.count > 2 && (64...126).contains(byte) {
                    if sequence == [27, 91, 50, 48, 48, 126] { pasteBytes = []; pasteSuffix = [] }
                    else if let event = decode(sequence) { events.append(event) }; sequence = []
                } else if sequence.count >= 64 { sequence = [] }
            } else { consume(byte, into: &events) }
        }
        return events
    }
    mutating func expireEscape() -> [TerminalInputEvent] {
        defer { sequence = [] }
        return sequence == [27] ? [.key("\u{1b}")] : []
    }
    private mutating func consume(_ byte: UInt8, into events: inout [TerminalInputEvent]) {
        if byte == 27 { utf8 = []; sequence = [27] }
        else if byte < 128 { utf8 = []; events.append(.key(Character(UnicodeScalar(byte)))) }
        else {
            utf8.append(byte)
            if let text = String(bytes: utf8, encoding: .utf8) { events += text.map { .key($0) }; utf8 = [] }
            else if utf8.count >= 4 { utf8 = [] }
        }
    }
    private func decode(_ bytes: [UInt8]) -> TerminalInputEvent? {
        guard let last = bytes.last else { return nil }
        if bytes.starts(with: [27, 91, 60]), last == 77 || last == 109 {
            let text = String(bytes: bytes.dropFirst(3).dropLast(), encoding: .ascii) ?? ""
            let pieces = text.split(separator: ";").compactMap { Int($0) }
            guard pieces.count == 3, (0...127).contains(pieces[0]), (1...10000).contains(pieces[1]), (1...10000).contains(pieces[2]) else { return nil }
            let code = pieces[0]
            let action: TUIMouseEvent.Action = code & 64 != 0 ? (code & 1 == 0 ? .scrollUp : .scrollDown) : last == 109 ? .up : code & 32 != 0 ? .drag : .down
            return .mouse(.init(action: action, x: pieces[1] - 1, y: pieces[2] - 1, button: code & 3, shift: code & 4 != 0, control: code & 16 != 0))
        }
        let arrows: [UInt8: Character] = [65: "\u{f700}", 66: "\u{f701}", 67: "\u{f703}", 68: "\u{f702}"]
        if let key = arrows[last] { return .key(key) }
        if last == 90 { return .key("\t") }
        return nil
    }
}
