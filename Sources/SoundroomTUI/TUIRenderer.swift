import Darwin
import Foundation
import SoundroomCore

// Rendering and hit testing share a bounded cell layout, including wide Unicode names.
enum TUIHitAction: Equatable {
    case key(Character)
    case command(String)
    case prompt(String)
    case panel(TUIModel.Panel)
    case track(UUID)
    case trackCommand(UUID, String)
    case timeline(UUID)
    case effect(Int)
    case parameter(Int, String)
    case gain, pan
}
struct TUIHit {
    var x: Int
    var y: Int
    var width: Int
    var action: TUIHitAction
    func contains(_ column: Int, _ row: Int) -> Bool { row == y && column >= x && column < x + width }
}
struct TUIFrame {
    var output: String
    var hits: [TUIHit]
    var firstTrack: Int
    var visibleTracks: Int
    var timelineX: Int
    var timelineWidth: Int
    var inspectorX: Int
    var inspectorY: Int
    var width: Int
    var height: Int
    func inspectorContains(x: Int, y: Int) -> Bool { x >= inspectorX && y >= inspectorY && y < height - 3 }
    func timelineFrame(at x: Int, model: TUIModel) -> Int {
        model.viewStart
            + Int(
                Double(max(0, min(timelineWidth - 1, x - timelineX))) / Double(max(1, timelineWidth))
                    * Double(model.viewSpan))
    }
}
private struct TUIStyle: Equatable {
    var fg = 252
    var bg = 234
    var bold = false
    var ansi: String { "\u{1b}[0;38;5;\(fg);48;5;\(bg)\(bold ? ";1" : "")m" }
    static let normal = TUIStyle()
    static let dim = TUIStyle(fg: 244)
    static let accent = TUIStyle(fg: 81, bold: true)
    static let selected = TUIStyle(fg: 255, bg: 238, bold: true)
}
private struct TUICanvas {
    struct Cell {
        var text = " "
        var style = TUIStyle.normal
    }
    var width: Int
    var height: Int
    var cells: [Cell]
    init(width: Int, height: Int) {
        self.width = width
        self.height = height
        cells = Array(repeating: Cell(), count: width * height)
    }
    mutating func put(_ text: String, x: Int = 0, y: Int, limit: Int? = nil, style: TUIStyle = .normal) {
        guard y >= 0 && y < height && x >= 0 && x < width else { return }
        var column = x
        let end = min(width, x + (limit ?? width))
        for character in TUIRenderer.safe(text) {
            let size = TUIRenderer.cellWidth(character)
            guard column + size <= end else { break }
            cells[y * width + column] = Cell(text: String(character), style: style)
            if size == 2 { cells[y * width + column + 1] = Cell(text: "", style: style) }
            column += size
        }
    }
    func output(color: Bool) -> String {
        var out = "\u{1b}[H"
        var current: TUIStyle?
        for row in 0..<height {
            if row > 0 { out += "\r\n" }
            for col in 0..<width {
                let cell = cells[row * width + col]
                if color, current != cell.style {
                    out += cell.style.ansi
                    current = cell.style
                }
                out += cell.text
            }
        }
        // Every row is completely painted. Reset so shell output cannot inherit our palette.
        return out + (color ? "\u{1b}[0m" : "")
    }
}
struct TUIRenderer {
    static func safe(_ text: String) -> String {
        String(
            text.unicodeScalars.filter {
                $0.properties.generalCategory != .control && $0.value >= 32 && $0.value != 127 && $0.value != 0x2028
                    && $0.value != 0x2029
            })
    }
    static func cellWidth(_ character: Character) -> Int {
        // wcwidth depends on the process locale (often C in launchd/CLI tests).
        // Keep CJK and emoji geometry deterministic even without a UTF-8 locale.
        let widths = character.unicodeScalars.map { scalar -> Int in
            let n = scalar.value
            if scalar.properties.isEmojiPresentation
                || (n >= 0x1100
                    && (n <= 0x115f || n == 0x2329 || n == 0x232a || (0x2e80...0xa4cf).contains(n)
                        || (0xac00...0xd7a3).contains(n) || (0xf900...0xfaff).contains(n)
                        || (0xfe10...0xfe19).contains(n) || (0xfe30...0xfe6f).contains(n)
                        || (0xff01...0xff60).contains(n) || (0xffe0...0xffe6).contains(n)
                        || (0x20000...0x3fffd).contains(n)))
            {
                return 2
            }
            return max(0, Int(wcwidth(wchar_t(n))))
        }
        if character.unicodeScalars.contains(where: { $0.value == 0xfe0f }) { return 2 }
        return max(1, min(2, widths.max() ?? 1))
    }
    static func line(_ text: String, width: Int) -> String {
        var result = ""
        var used = 0
        for c in safe(text) {
            let size = cellWidth(c)
            if used + size > width { break }
            result.append(c)
            used += size
        }
        return result
    }
    static func render(_ model: TUIModel, host: String, width: Int, height: Int, color: Bool = true) -> String {
        frame(model, host: host, width: width, height: height, color: color).output
    }
    static func frame(_ m: TUIModel, host: String, width: Int, height: Int, color: Bool = true) -> TUIFrame {
        if width < 30 || height < 10 {
            let w = max(1, min(240, width))
            let h = max(1, min(100, height))
            var small = TUICanvas(width: w, height: h)
            small.put("Enlarge terminal (30×10)", y: 0, style: .accent)
            return .init(
                output: small.output(color: color), hits: [], firstTrack: 0, visibleTracks: 0, timelineX: 0,
                timelineWidth: 1, inspectorX: w, inspectorY: h, width: w, height: h)
        }
        let width = min(240, width)
        let height = min(100, height)
        var canvas = TUICanvas(width: width, height: height)
        var hits: [TUIHit] = []
        let side = width >= 100 && height >= 16
        let inspectorWidth = side ? 36 : width
        let inspectorX = side ? width - inspectorWidth : 0
        let inspectorY = side ? 3 : height >= 20 ? height - 11 : height - 3
        let timelineEnd = side ? inspectorX - 1 : width
        let labelWidth = min(26, max(12, timelineEnd / 3))
        let timelineX = labelWidth + 1
        let timelineWidth = max(1, timelineEnd - timelineX)
        let visible = max(1, (side ? height - 3 : inspectorY) - 5)
        let centered = max(0, min(m.trackIndex - visible / 2, m.session.tracks.count - visible))
        let first = max(0, min(m.trackScroll ?? centered, max(0, m.session.tracks.count - visible)))
        var result = TUIFrame(
            output: "", hits: [], firstTrack: first, visibleTracks: visible, timelineX: timelineX,
            timelineWidth: timelineWidth, inspectorX: inspectorX, inspectorY: inspectorY, width: width, height: height)
        func hit(_ text: String, x: Int, y: Int, action: TUIHitAction, style: TUIStyle = .accent) {
            let text = line(text, width: width - x)
            canvas.put(text, x: x, y: y, style: style)
            let length = text.reduce(0) { $0 + cellWidth($1) }
            hits.append(.init(x: x, y: y, width: length, action: action))
        }
        let transport = m.session.recording ? "REC" : m.session.playing ? "PLAY" : "STOP"
        canvas.put(" SOUNDROOM ", y: 0, style: .init(fg: 234, bg: 81, bold: true))
        canvas.put(
            " \(m.connected ? "● Connected" : "○ Disconnected")  \(m.role == "viewer" ? "Read only" : m.role.capitalized)  \(host)",
            x: 11, y: 0, style: .init(fg: m.connected ? 114 : 203))
        canvas.put(
            " \(m.session.name)  ·  \(transport) \(m.position(m.session.frame))  ·  \(m.session.operatorName ?? "unclaimed")",
            y: 1, style: .selected)
        let buttons: [(String, TUIHitAction)] = [
            ("[\(m.timebase == .beats ? "Beats" : "Time")]", .key("B")),
            (m.session.playing ? "[Pause]" : "[Play]", .key(" ")), ("[Stop]", .command("stop")),
            ("[Record]", .key("r")), ("[Seek]", .key("\r")), ("[Loop]", .key("L")), ("[Fit]", .key("F")),
            ("[−]", .key("-")), ("[+]", .key("+")), ("[Help]", .key("?")),
        ]
        var x = 1
        for (text, action) in buttons where x + text.count < width {
            hit(text, x: x, y: 2, action: action, style: text == "[Record]" ? .init(fg: 203, bold: true) : .accent)
            x += text.count + 1
        }
        if m.timebase == .beats {
            if x + 10 < width {
                hit("\(String(format: "%g", m.session.timing.tempo)) BPM", x: x, y: 2, action: .prompt("tempo "))
                x += 10
            }
            if x + 4 < width { hit(m.session.timing.meter, x: x, y: 2, action: .prompt("meter ")) }
        }
        canvas.put(
            " \(m.inspectorFocused ? "Timeline" : "TIMELINE")  cursor \(m.position(m.cursor))", y: 3,
            limit: timelineEnd, style: m.inspectorFocused ? .dim : .accent)
        canvas.put("R M S  Track", x: 2, y: 4, limit: labelWidth - 2, style: .dim)
        if m.timebase == .beats {
            let pixelsPerSecond = Double(timelineWidth) / (Double(m.viewSpan) / m.session.sampleRate)
            for tick in m.session.timing.rulerTicks(
                start: m.viewStart, end: m.viewStart + m.viewSpan, pixelsPerSecond: pixelsPerSecond, minimumPixels: 8,
                sampleRate: m.session.sampleRate)
            {
                let col = Int(Double(tick.frame - m.viewStart) / Double(m.viewSpan) * Double(timelineWidth))
                if col >= 0 && col + tick.label.count <= timelineWidth {
                    canvas.put(tick.label, x: timelineX + col, y: 4, style: tick.bar ? .accent : .dim)
                }
            }
        } else {
            for col in stride(from: 0, to: timelineWidth, by: 13) {
                let frame = m.viewStart + Int(Double(col) / Double(timelineWidth) * Double(m.viewSpan))
                if timelineWidth - col >= 8 {
                    canvas.put(
                        time(frame, rate: m.session.sampleRate), x: timelineX + col, y: 4,
                        limit: min(12, timelineWidth - col), style: .dim)
                }
            }
        }
        let selectedIDs = Set(m.selectedClips.map(\.id))
        let palette = [81, 141, 114, 215, 210, 117]
        let gridPixels = Double(timelineWidth) / (Double(m.viewSpan) / m.session.sampleRate)
        let musicalTicks =
            m.timebase == .beats
            ? m.session.timing.rulerTicks(
                start: m.viewStart, end: m.viewStart + m.viewSpan, pixelsPerSecond: gridPixels, minimumPixels: 3,
                sampleRate: m.session.sampleRate) : []
        let subdivisions =
            m.timebase == .beats && m.beatSnap
            ? m.session.timing.gridFrames(
                start: m.viewStart, end: m.viewStart + m.viewSpan, quarterNoteDivision: m.gridQuarterNotes,
                pixelsPerSecond: gridPixels, minimumPixels: 3, sampleRate: m.session.sampleRate) : []
        for (offset, track) in m.session.tracks.dropFirst(first).prefix(visible).enumerated() {
            let y = 5 + offset
            let selected = track.id == m.trackID
            let trackColor = palette[(track.color % palette.count + palette.count) % palette.count]
            canvas.put(String(repeating: " ", count: timelineEnd), y: y, style: selected ? .selected : .normal)
            hit(
                "\(selected ? "›" : " ")", x: 0, y: y, action: .track(track.id),
                style: .init(fg: trackColor, bg: selected ? 238 : 234, bold: selected))
            for (col, active, label, command, fg) in [
                (2, track.armed, "R", "arm", 203), (4, track.muted, "M", "mute", 215),
                (6, track.solo, "S", "solo", 114),
            ] {
                hit(
                    active ? label : label.lowercased(), x: col, y: y,
                    action: .command("\(command) \(active ? "off" : "on")"),
                    style: .init(fg: active ? fg : 242, bg: selected ? 238 : 234, bold: active))
                // Track flags must act on the clicked row, not an older local selection.
                hits[hits.count - 1].action = .trackCommand(track.id, "\(command) \(active ? "off" : "on")")
            }
            hit(
                line(track.name, width: labelWidth - 8), x: 8, y: y, action: .track(track.id),
                style: .init(fg: selected ? 255 : trackColor, bg: selected ? 238 : 234, bold: selected))
            // Bound long labels before they can cover the timeline.
            canvas.put("│", x: labelWidth, y: y, style: .dim)
            for col in 0..<timelineWidth {
                canvas.put("·", x: timelineX + col, y: y, style: .init(fg: 239, bg: selected ? 238 : 234))
            }
            if m.timebase == .beats {
                for frame in subdivisions {
                    let col = Int(Double(frame - m.viewStart) / Double(m.viewSpan) * Double(timelineWidth))
                    if col >= 0 && col < timelineWidth { canvas.put("┆", x: timelineX + col, y: y, style: .dim) }
                }
                for tick in musicalTicks {
                    let col = Int(Double(tick.frame - m.viewStart) / Double(m.viewSpan) * Double(timelineWidth))
                    if col >= 0 && col < timelineWidth {
                        canvas.put(tick.bar ? "┊" : "·", x: timelineX + col, y: y, style: tick.bar ? .dim : .normal)
                    }
                }
            }
            if let start = m.session.loopStart, let end = m.session.loopEnd {
                for col in 0..<timelineWidth {
                    let frame = m.viewStart + Int(Double(col) / Double(timelineWidth) * Double(m.viewSpan))
                    if frame >= start && frame < end {
                        canvas.put("·", x: timelineX + col, y: y, style: .init(fg: 60, bg: 236))
                    }
                }
            }
            for clip in track.clips where clip.end > m.viewStart && clip.start < m.viewStart + m.viewSpan {
                let from = max(
                    0,
                    min(
                        timelineWidth - 1,
                        Int(Double(clip.start - m.viewStart) / Double(m.viewSpan) * Double(timelineWidth))))
                let to = max(
                    from + 1,
                    min(
                        timelineWidth,
                        Int(ceil(Double(clip.end - m.viewStart) / Double(m.viewSpan) * Double(timelineWidth)))))
                let highlighted = selected && selectedIDs.contains(clip.id)
                let style = TUIStyle(
                    fg: highlighted ? 234 : trackColor, bg: highlighted ? trackColor : (selected ? 238 : 234),
                    bold: highlighted)
                canvas.put(
                    String(repeating: highlighted ? "━" : "─", count: to - from), x: timelineX + from, y: y,
                    style: style)
                if to - from > 5 {
                    canvas.put(clip.name, x: timelineX + from + 1, y: y, limit: to - from - 2, style: style)
                }
            }
            for (frame, text, style) in [(m.session.frame, "┃", TUIStyle(fg: 114)), (m.cursor, "│", .accent)]
            where frame >= m.viewStart && frame < m.viewStart + m.viewSpan {
                if text == "┃" || selected {
                    let col = Int(Double(frame - m.viewStart) / Double(m.viewSpan) * Double(timelineWidth))
                    canvas.put(text, x: timelineX + col, y: y, style: style)
                }
            }
            hits.append(.init(x: timelineX, y: y, width: timelineWidth, action: .timeline(track.id)))
            // Whole label selects its track; flags get specialized commands below in model.
            hits.insert(.init(x: 0, y: y, width: labelWidth, action: .track(track.id)), at: max(0, hits.count - 6))
        }
        if m.session.tracks.isEmpty { hit("[+ New track]", x: 1, y: 5, action: .prompt("new-track ")) }
        if inspectorY < height - 3 {
            if side { for y in 3..<height - 3 { canvas.put("│", x: inspectorX - 1, y: y, style: .dim) } }
            var tabX = inspectorX + 1
            for panel in TUIModel.Panel.allCases {
                hit(
                    " \(panel.rawValue) ", x: tabX, y: inspectorY, action: .panel(panel),
                    style: m.panel == panel ? .selected : .dim)
                tabX += panel.rawValue.count + 2
            }
            let bodyY = inspectorY + 1
            let bodyEnd = height - 3
            let innerX = inspectorX + 1
            let innerWidth = inspectorWidth - 2
            if let track = m.selectedTrack {
                canvas.put(
                    track.name + (m.inspectorFocused ? " ◂" : ""), x: innerX, y: bodyY, limit: innerWidth,
                    style: .accent)
                var rows: [(String, TUIHitAction?)] = []
                func bar(_ normalized: Double) -> String {
                    let position = max(0, min(innerWidth - 1, Int(normalized * Double(innerWidth - 1))))
                    return String(repeating: "━", count: position) + "●"
                        + String(repeating: "─", count: innerWidth - position - 1)
                }
                switch m.panel {
                case .mixer:
                    rows = [
                        ("\(track.armed ? "[R armed]" : "[R arm]")", .command("arm")),
                        ("\(track.muted ? "[M muted]" : "[M mute]")", .command("mute")),
                        ("\(track.solo ? "[S soloed]" : "[S solo]")", .command("solo")),
                        (
                            "\(m.inspectorFocused && m.inspectorIndex == 0 ? "›" : " ") Gain \(String(format: "%.2f", track.gain)) · 0…4",
                            .prompt("volume ")
                        ), (bar(track.gain / 4), .gain),
                        (
                            "\(m.inspectorFocused && m.inspectorIndex == 1 ? "›" : " ") Pan \(String(format: "%+.2f", track.pan)) · L…R",
                            .prompt("pan ")
                        ), (bar((track.pan + 1) / 2), .pan), ("In: \(track.input)", nil),
                        ("\(track.clips.count) clips · \(track.inserts.count) inserts", nil),
                        ("[Play track]", .command("play-track")), ("[Rename]", .prompt("rename ")),
                        ("[Duplicate]", .command("duplicate-track")),
                        ("[Duplicate + automation]", .command("duplicate-track automation")),
                        ("[New track]", .prompt("new-track ")),
                    ]
                case .effects:
                    rows.append(("[+ Add native effect]", .prompt("effect add ")))
                    for (i, effect) in track.inserts.enumerated() {
                        rows.append(
                            (
                                "\(i == m.effectIndex ? "›" : " ") \(i + 1) \(effect.kind.name)\(effect.enabled ? "" : " · bypass")",
                                .effect(i)
                            ))
                    }
                    if let effect = m.activeEffect {
                        rows.append(
                            (
                                "[\(effect.enabled ? "Bypass" : "Enable")]",
                                .command("effect bypass \(m.effectIndex + 1) \(effect.enabled ? "on" : "off")")
                            ))
                        rows.append(("[Factory presets]", .prompt("effect preset \(m.effectIndex + 1) ")))
                        for (index, p) in effect.kind.parameters.enumerated() {
                            let normalized = p.normalized(effect.parameters[p.id] ?? p.defaultValue)
                            rows.append(
                                (
                                    "\(m.inspectorFocused && index == m.inspectorIndex ? "›" : " ") \(p.name): \(p.display(effect.parameters[p.id] ?? p.defaultValue))",
                                    .prompt("effect set \(m.effectIndex + 1) \(p.id) ")
                                ))
                            let position = Int(normalized * Float(innerWidth - 1))
                            rows.append(
                                (
                                    String(repeating: "━", count: position) + "●"
                                        + String(repeating: "─", count: innerWidth - position - 1),
                                    .parameter(m.effectIndex + 1, p.id)
                                ))
                        }
                        rows.append(("Presets:", nil))
                        for (index, preset) in effect.kind.presets.enumerated() {
                            rows.append(
                                (
                                    "  \(index + 1) \(preset.name)",
                                    .command("effect preset \(m.effectIndex + 1) \(index + 1)")
                                ))
                        }
                    }
                case .routing:
                    rows = [("Track: \(track.input)", nil), ("[Follow default input]", .command("input default"))]
                    for device in m.session.devices where device.inputs > 0 {
                        rows.append(
                            ("IN \(device.id) \(device.name) · \(device.inputs)ch", .command("input \(device.id)")))
                    }
                    rows.append(("[Input channel / stereo…]", .prompt("input ")))
                    rows.append(("Output (server hardware):", nil))
                    for device in m.session.devices where device.outputs > 0 {
                        rows.append(
                            (
                                "\(device.id == m.session.outputID ? "›" : " ") OUT \(device.id) \(device.name)",
                                .command("output \(device.id)")
                            ))
                    }
                }
                // Focused controls remain visible while keyboard navigation changes them.
                let focusRow =
                    m.panel == .effects ? track.inserts.count + 3 + m.inspectorIndex * 2 : m.inspectorIndex + 3
                let desiredScroll =
                    m.inspectorFocused
                    ? max(m.inspectorScroll, focusRow - max(1, bodyEnd - bodyY - 2)) : m.inspectorScroll
                let maxScroll = max(0, rows.count - max(1, bodyEnd - bodyY - 1))
                let scroll = min(desiredScroll, maxScroll)
                for (offset, item) in rows.dropFirst(scroll).prefix(max(0, bodyEnd - bodyY - 1)).enumerated() {
                    let y = bodyY + offset + 1
                    if let action = item.1 {
                        hit(line(item.0, width: innerWidth), x: innerX, y: y, action: action, style: .normal)
                    } else {
                        canvas.put(item.0, x: innerX, y: y, limit: innerWidth, style: .dim)
                    }
                }
                if rows.count > bodyEnd - bodyY - 1 { canvas.put("↕", x: width - 1, y: bodyEnd - 1, style: .accent) }
            }
        }
        canvas.put(String(repeating: "─", count: width), y: height - 3, style: .dim)
        let editButtons: [(String, TUIHitAction)] = [
            ("Split x", .key("x")), ("Combine C", .key("C")), ("Copy y", .key("y")), ("Paste p", .key("p")),
            ("Undo u", .key("u")),
        ]
        x = 1
        for (text, action) in editButtons where x + text.count + 2 < width {
            hit(" \(text) ", x: x, y: height - 3, action: action)
            x += text.count + 3
        }
        canvas.put(
            m.mode == .command ? ":" + m.commandText : " \(String(describing: m.mode).uppercased())  \(m.status)",
            y: height - 2, style: m.mode == .command ? .selected : .normal)
        canvas.put(
            " Tab focus · j/k move · h/l adjust · Space play · Enter seeks server · ? help · q detach", y: height - 1,
            style: .dim)
        if m.mode == .help {
            let help = [
                "SOUNDROOM · keyboard + mouse", "B Time/Beats · :tempo 123 · :meter 7/8 · :grid 1/16", "",
                "Mouse: click track / clip · Shift-click adds clips",
                "Drag timeline: time range · wheel: scroll tracks / inspector",
                "Shift-wheel: timeline pan · Ctrl-wheel: zoom", "Click R/M/S flags, toolbar, mixer and effect controls",
                "", "Tab focus timeline/inspector · t return to timeline",
                "j/k tracks or inspector control · h/l cursor or value",
                "w/b clip edges · gg/G start/end · F fit · +/- zoom",
                "Space play/pause · Enter seeks server · r record/stop",
                "m mute · s solo · a arm · e next pane · f effects",
                "Effects: J/K choose insert · j/k parameter · Enter exact value",
                "v range · V clip · y copy · p paste · dd delete",
                "x split · C combine · [/] nudge clips · L loop selection", "u your Undo · Ctrl-R your Redo",
                ": + Tab completion · Ctrl-P recalls last command",
                ":move seconds · :trim start end · :duplicate-track [automation]",
                ":loop start end|off · :effect preset 1 NUMBER|NAME",
                "q detach; audio continues · ? / Escape close help",
            ]
            for y in 3..<height - 2 {
                canvas.put(String(repeating: " ", count: width), y: y)
                if y - 3 < help.count { canvas.put(help[y - 3], x: 1, y: y, style: y == 3 ? .accent : .normal) }
            }
            hits = []
        }
        result.hits = hits
        result.output = canvas.output(color: color)
        return result
    }
    static func time(_ frame: Int, rate: Double) -> String {
        let seconds = max(0, Double(frame) / max(1, rate))
        return String(format: "%02d:%05.2f", Int(seconds) / 60, seconds.truncatingRemainder(dividingBy: 60))
    }
}
