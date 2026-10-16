import SwiftUI
import AppKit

/// Shared by the event handler and keyboard labels; MIDI uses C4 = 60.
enum DesignerTypingKeyboard {
    static let keys = ["a", "w", "s", "e", "d", "f", "t", "g", "y", "h", "u", "j", "k"]
    static func pitch(for key: String, octave: Int) -> Int? {
        guard (0...10).contains(octave), let offset = keys.firstIndex(of: key.lowercased()) else { return nil }
        let pitch = octave * 12 + offset
        return pitch <= 127 ? pitch : nil
    }
    static func name(_ pitch: Int) -> String {
        let names = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"]
        return names[max(0, min(127, pitch)) % 12] + String(max(0, min(127, pitch)) / 12 - 1)
    }
}

/// A shared pitch stays held until its last input releases it. Store the original
/// pitch so changing octave while a computer key is down cannot leave a stuck note.
struct DesignerHeldKeys {
    enum Source: Hashable { case computer(UInt16), pointer(Int), midi(Int) }
    private var notes: [Source: Int] = [:]
    var pitches: Set<Int> { Set(notes.values) }
    func pitch(for source: Source) -> Int? { notes[source] }
    mutating func press(_ source: Source, pitch: Int) -> Bool {
        guard (0...127).contains(pitch), notes[source] == nil else { return false }
        let first = !notes.values.contains(pitch)
        notes[source] = pitch
        return first
    }
    mutating func release(_ source: Source) -> Int? {
        guard let pitch = notes.removeValue(forKey: source), !notes.values.contains(pitch) else { return nil }
        return pitch
    }
    mutating func releaseLocal() -> Set<Int> {
        let before = pitches
        notes = notes.filter { if case .midi = $0.key { return true }; return false }
        return before.subtracting(pitches)
    }
    mutating func clear() { notes.removeAll() }
}

struct DesignerVirtualKeyboard: View {
    @Binding var octave: Int
    @Binding var velocity: Double
    let active: Set<Int>
    let recording: Bool
    let onNote: (Int, Bool) -> Void
    let onRecord: () -> Void
    let onClose: () -> Void
    private let white = [0, 2, 4, 5, 7, 9, 11, 12]
    private let black = [1, 3, 6, 8, 10]
    private let blackPositions: [CGFloat] = [46, 110, 238, 302, 366]

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Label("Keyboard", systemImage: "pianokeys").font(.system(size: 14, weight: .semibold))
                Spacer()
                Button(action: onClose) { Image(systemName: "xmark") }.buttonStyle(.plain).accessibilityLabel("Close keyboard")
            }
            HStack(spacing: 10) {
                Button { octave = max(0, octave - 1) } label: { Image(systemName: "minus") }
                    .disabled(octave == 0).accessibilityLabel("Keyboard octave down").help("Octave down · Z")
                Text("C\(octave - 1)").font(.system(size: 12, weight: .semibold)).monospacedDigit().frame(width: 30)
                Button { octave = min(10, octave + 1) } label: { Image(systemName: "plus") }
                    .disabled(octave == 10).accessibilityLabel("Keyboard octave up").help("Octave up · X")
                Spacer()
                Text("Velocity").font(.system(size: 11)).foregroundStyle(Theme.secondary)
                RoomSlider(value: $velocity, in: 1...127, label: "Keyboard velocity").frame(width: 110)
                Text("\(Int(velocity))").font(.system(size: 11)).monospacedDigit().frame(width: 26)
            }.buttonStyle(.borderless)
            ZStack(alignment: .topLeading) {
                HStack(spacing: 2) {
                    ForEach(white, id: \.self) { offset in key(offset, dark: false).frame(width: 62, height: 126) }
                }
                ForEach(Array(black.enumerated()), id: \.element) { index, offset in
                    key(offset, dark: true).frame(width: 36, height: 79).offset(x: blackPositions[index])
                }
            }.frame(width: 510, height: 126)
            HStack {
                Button(action: onRecord) { Label(recording ? "Stop notes" : "Record notes", systemImage: recording ? "stop.fill" : "record.circle") }
                    .buttonStyle(RoomButtonStyle()).tint(recording ? .red : Theme.accent)
                Spacer()
                Text("Z / X  Octave    R  Record    Space  Play / Stop").font(.system(size: 10)).foregroundStyle(Theme.secondary)
            }
        }.padding(16).foregroundStyle(Theme.ink).background(Theme.paper).preferredColorScheme(.dark)
    }
    private func key(_ offset: Int, dark: Bool) -> some View {
        let pitch = octave * 12 + offset
        let enabled = pitch <= 127
        return DesignerPianoKey(pitch: pitch, typingKey: DesignerTypingKeyboard.keys[offset].uppercased(),
                                dark: dark, active: active.contains(pitch), enabled: enabled, onNote: onNote)
    }
}

private struct DesignerPianoKey: NSViewRepresentable {
    let pitch: Int
    let typingKey: String
    let dark: Bool
    let active: Bool
    let enabled: Bool
    let onNote: (Int, Bool) -> Void
    func makeNSView(context: Context) -> DesignerNativePianoKey { DesignerNativePianoKey() }
    func updateNSView(_ view: DesignerNativePianoKey, context: Context) {
        view.pitch = pitch; view.typingKey = typingKey; view.dark = dark
        view.active = active; view.enabled = enabled; view.onNote = onNote
        view.needsDisplay = true
    }
    static func dismantleNSView(_ view: DesignerNativePianoKey, coordinator: ()) { view.release() }
}

private final class DesignerNativePianoKey: NSView {
    var pitch = 60
    var typingKey = "A"
    var dark = false
    var active = false
    var enabled = true
    var onNote: (Int, Bool) -> Void = { _, _ in }
    private var pressedPitch: Int?
    private var tapTask: Task<Void, Never>?
    override var isFlipped: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func mouseDown(with event: NSEvent) {
        guard enabled else { return }
        release(); pressedPitch = pitch; onNote(pitch, true)
    }
    override func mouseUp(with event: NSEvent) { release() }
    override func viewWillMove(toWindow newWindow: NSWindow?) {
        if newWindow == nil { release() }
        super.viewWillMove(toWindow: newWindow)
    }
    func release() {
        tapTask?.cancel(); tapTask = nil
        guard let held = pressedPitch else { return }
        pressedPitch = nil; onNote(held, false)
    }
    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .button }
    override func accessibilityLabel() -> String? {
        enabled ? "\(DesignerTypingKeyboard.name(pitch)), key \(typingKey)" : "Unavailable key \(typingKey)"
    }
    override func accessibilityValue() -> Any? { active ? "Held" : "Released" }
    override func isAccessibilityEnabled() -> Bool { enabled }
    override func accessibilityPerformPress() -> Bool {
        guard enabled else { return false }
        release(); pressedPitch = pitch; onNote(pitch, true)
        tapTask = Task { @MainActor [weak self] in
            try? await Task.sleep(for: .milliseconds(180))
            guard !Task.isCancelled else { return }
            self?.release()
        }
        return true
    }
    override func draw(_ dirtyRect: NSRect) {
        let rect = bounds.insetBy(dx: 0.5, dy: 0.5)
        let shape = NSBezierPath(roundedRect: rect, xRadius: 5, yRadius: 5)
        let fill = active ? NSColor(Theme.accent)
                          : NSColor(white: dark ? 0.12 : 0.88, alpha: 1)
        fill.withAlphaComponent(enabled ? 1 : 0.3).setFill(); shape.fill()
        NSColor.black.withAlphaComponent(0.4).setStroke(); shape.lineWidth = 1; shape.stroke()
        let color = (dark && !active ? NSColor.white : .black).withAlphaComponent(enabled ? 1 : 0.3)
        drawText(enabled ? DesignerTypingKeyboard.name(pitch) : "—", y: bounds.height - 43,
                 font: .systemFont(ofSize: 10, weight: .medium), color: color)
        drawText(typingKey, y: bounds.height - 24,
                 font: .monospacedSystemFont(ofSize: 12, weight: .semibold), color: color)
    }
    private func drawText(_ text: String, y: CGFloat, font: NSFont, color: NSColor) {
        let string = NSAttributedString(string: text, attributes: [.font: font, .foregroundColor: color])
        string.draw(at: NSPoint(x: (bounds.width - string.size().width) / 2, y: y))
    }
}
