import AppKit
import SoundroomCore
import SwiftUI

struct InlineSoundHeader: View {
    @ObservedObject var store: SessionStore
    @ObservedObject var controller: InlineSoundController
    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 7) {
                HStack {
                    Label("Sound editor", systemImage: "pianokeys").fontWeight(.semibold)
                    Spacer()
                    Button {
                        store.inlineSoundExpanded = false
                    } label: {
                        Image(systemName: "chevron.up")
                    }
                    .buttonStyle(.plain).accessibilityLabel("Collapse sound editor")
                }
                Menu {
                    if let track = store.session.tracks.first(where: { $0.id == controller.trackID }) {
                        ForEach(track.clips.filter { $0.editableSoundDesign != nil }) { clip in
                            Button(clip.name) { store.toggleInlineSound(trackID: track.id, clipID: clip.id) }
                        }
                    }
                } label: {
                    Text(controller.currentClip?.name ?? "Clip unavailable").lineLimit(1)
                }
                .menuStyle(.borderlessButton).accessibilityLabel("Instrument clip to edit")
            }.padding(.horizontal, 14).frame(height: InlineSoundLayout.controls)
            Picker("Visible octave", selection: $controller.octave) {
                ForEach(0...10, id: \.self) { Text("Octave \($0 - 1)").tag($0) }
            }.labelsHidden().padding(.horizontal, 14).frame(height: InlineSoundLayout.ruler)
            ForEach((0..<12).reversed(), id: \.self) { pitchClass in
                let pitch = controller.octave * 12 + pitchClass
                Text(pitch <= 127 ? SoundDesignNote(pitch: pitch, startBeat: 0).name : "—")
                    .font(.system(size: 10, design: .monospaced)).foregroundStyle(Theme.secondary)
                    .frame(maxWidth: .infinity, alignment: .trailing).padding(.trailing, 14)
                    .frame(height: InlineSoundLayout.row)
                    .background([1, 3, 6, 8, 10].contains(pitchClass) ? Theme.paper : Theme.raised)
            }
            Button {
                controller.openDesigner()
            } label: {
                Label("Full designer…", systemImage: "arrow.up.left.and.arrow.down.right")
            }
            .buttonStyle(.plain).frame(height: InlineSoundLayout.footer)
            .disabled(!controller.canEdit || controller.failure != nil)
        }.font(.system(size: 11)).background(Theme.paper)
            .overlay(alignment: .trailing) { Theme.edge.frame(width: 1) }
            .overlay(alignment: .bottom) { Theme.line.frame(height: 1) }
    }
}

struct InlineSoundView: View {
    @ObservedObject var store: SessionStore
    @ObservedObject var controller: InlineSoundController
    @State private var insertionBeat = 0.0
    @FocusState private var gridFocused: Bool
    private var pattern: SoundDesignPattern { controller.pattern }

    var body: some View {
        VStack(spacing: 0) {
            controls.frame(height: InlineSoundLayout.controls)
            GeometryReader { geometry in
                let beatWidth = max(64, geometry.size.width / max(1, pattern.beats))
                ScrollView(.horizontal) {
                    VStack(spacing: 0) {
                        ruler(beatWidth: beatWidth)
                        grid(beatWidth: beatWidth)
                    }.frame(width: pattern.beats * beatWidth)
                }.scrollIndicators(.visible)
            }.frame(height: InlineSoundLayout.ruler + InlineSoundLayout.row * 12)
            footer.frame(height: InlineSoundLayout.footer)
        }.font(.system(size: 11)).background(Theme.well)
            .overlay(alignment: .bottom) { Theme.line.frame(height: 1) }
            .onDisappear { controller.endGesture() }
    }
    private var controls: some View {
        VStack(spacing: 6) {
            ScrollView(.horizontal) {
                HStack(spacing: 12) {
                    Picker(
                        "Instrument",
                        selection: Binding(
                            get: { pattern.instrument },
                            set: { instrument in controller.change { $0 = instrument.applying(to: $0) } })
                    ) {
                        ForEach(MusicalInstrument.allCases) { Text($0.title).tag($0) }
                    }.labelsHidden().frame(width: 180)
                    Menu("Preset") {
                        ForEach(SoundDesignPreset.allCases) { preset in
                            Button(preset.rawValue) { controller.change { $0 = preset.applying(to: $0) } }
                        }
                    }.fixedSize()
                    Text("Beats").foregroundStyle(Theme.secondary)
                    CommittedNumberField(
                        "Sequence length in beats",
                        value: Binding(get: { pattern.beats }, set: { controller.resizeSequence($0) }),
                        range: 0.25...64, fractionDigits: 2
                    )
                    .textFieldStyle(.roundedBorder).frame(width: 48)
                    Picker("Note grid", selection: $controller.grid) {
                        Text("1/8").tag(0.5)
                        Text("1/16").tag(0.25)
                        Text("1/32").tag(0.125)
                    }.labelsHidden().frame(width: 72)
                    Text("BPM").foregroundStyle(Theme.secondary)
                    CommittedNumberField("Instrument tempo", value: parameter(\.bpm), range: 30...300)
                        .textFieldStyle(.roundedBorder).frame(width: 46)
                }.padding(.horizontal, 12).frame(height: 28)
            }.scrollIndicators(.hidden)
            ScrollView(.horizontal) {
                HStack(spacing: 16) {
                    knob("Level", value: parameter(\.baseGain), range: 0...1)
                    if !pattern.instrument.isRecorded {
                        knob("Tone", value: parameter(\.tone), range: 0...1)
                        knob("Attack", value: parameter(\.attack), range: 0...2)
                        knob("Release", value: parameter(\.release), range: 0...2)
                    } else {
                        Text("Recorded instrument · shape sound in Full designer").foregroundStyle(Theme.secondary)
                    }
                }.padding(.horizontal, 12).frame(height: 28)
            }.scrollIndicators(.hidden)
        }.disabled(!controller.canEdit)
    }
    private func parameter(_ key: WritableKeyPath<SoundDesignPattern, Double>) -> Binding<Double> {
        Binding(get: { pattern[keyPath: key] }, set: { value in controller.change { $0[keyPath: key] = value } })
    }
    private func knob(_ name: String, value: Binding<Double>, range: ClosedRange<Double>) -> some View {
        HStack(spacing: 6) {
            Text(name).foregroundStyle(Theme.secondary)
            RoomSlider(
                value: value, in: range, label: "Instrument \(name)",
                onEditingChanged: { $0 ? controller.beginGesture() : controller.endGesture() }
            ).frame(width: 70)
        }.fixedSize()
    }
    private func ruler(beatWidth: Double) -> some View {
        HStack(spacing: 0) {
            ForEach(0..<Int(ceil(pattern.beats)), id: \.self) { beat in
                Text("\(beat + 1)").font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(Theme.secondary).frame(
                        width: min(1, pattern.beats - Double(beat)) * beatWidth, height: InlineSoundLayout.ruler,
                        alignment: .leading)
            }
        }.accessibilityHidden(true)
    }
    private func grid(beatWidth: Double) -> some View {
        ZStack(alignment: .topLeading) {
            Canvas { context, size in
                for row in 0..<12 {
                    let pitchClass = 11 - row
                    context.fill(
                        Path(
                            CGRect(
                                x: 0, y: Double(row) * InlineSoundLayout.row, width: size.width,
                                height: InlineSoundLayout.row)),
                        with: .color([1, 3, 6, 8, 10].contains(pitchClass) ? Theme.paper : Theme.raised))
                }
                var lines = Path()
                for row in 0...12 {
                    let y = Double(row) * InlineSoundLayout.row
                    lines.move(to: CGPoint(x: 0, y: y))
                    lines.addLine(to: CGPoint(x: size.width, y: y))
                }
                for step in 0...Int(ceil(pattern.beats / controller.grid)) {
                    let x = Double(step) * controller.grid * beatWidth
                    lines.move(to: CGPoint(x: x, y: 0))
                    lines.addLine(to: CGPoint(x: x, y: size.height))
                }
                context.stroke(lines, with: .color(Theme.background), lineWidth: 0.7)
                if let clip = controller.currentClip, clip.offset > 0 || clip.length < pattern.sequenceFrames {
                    let start = Double(clip.offset) / sessionRate * pattern.bpm / 60 * beatWidth
                    let end = Double(clip.offset + clip.length) / sessionRate * pattern.bpm / 60 * beatWidth
                    for rect in [
                        CGRect(x: 0, y: 0, width: min(start, size.width), height: size.height),
                        CGRect(x: min(end, size.width), y: 0, width: max(0, size.width - end), height: size.height),
                    ] {
                        context.fill(Path(rect), with: .color(Theme.background.opacity(0.65)))
                    }
                }
            }.contentShape(Rectangle()).onTapGesture { location in
                gridFocused = true
                insertionBeat = max(
                    0,
                    min(
                        pattern.beats - controller.grid,
                        floor(location.x / beatWidth / controller.grid) * controller.grid))
                let pitch = controller.octave * 12 + 11 - min(11, max(0, Int(location.y / InlineSoundLayout.row)))
                controller.addNote(pitch: pitch, beat: insertionBeat)
            }.accessibilityHidden(true)
            ForEach(pattern.notes.filter { $0.pitch / 12 == controller.octave }) { note in
                noteView(note, beatWidth: beatWidth)
            }
            InlineSoundPlayhead(
                position: store.playbackPosition, clip: controller.currentClip, bpm: pattern.bpm,
                beatWidth: beatWidth, beats: pattern.beats, playing: store.isPlaying)
        }.frame(height: InlineSoundLayout.row * 12).clipped()
            .focusable().focused($gridFocused).focusEffectDisabled()
            .overlay {
                Rectangle().stroke(gridFocused ? Theme.accent.opacity(0.7) : Color.clear).allowsHitTesting(false)
            }
            .onKeyPress(phases: [.down, .repeat], action: keyPress)
            .accessibilityElement(children: .contain).accessibilityLabel("Instrument note grid")
            .accessibilityHint(
                "Click to add notes. Drag to move; drag the right edge to resize. Shift-click selects multiple notes."
            )
            .disabled(!controller.canEdit)
    }
    private func noteView(_ note: SoundDesignNote, beatWidth: Double) -> some View {
        RoundedRectangle(cornerRadius: 3)
            .fill(Theme.accent.opacity(0.4 + note.velocity * 0.6))
            .overlay(
                RoundedRectangle(cornerRadius: 3).stroke(
                    controller.selectedNotes.contains(note.id) ? Theme.ink : Color.clear, lineWidth: 1.5)
            )
            .overlay(alignment: .trailing) {
                Rectangle().fill(Theme.ink.opacity(0.55)).frame(width: 4).padding(.vertical, 3)
                    .frame(width: 12, alignment: .trailing).contentShape(Rectangle())
                    .highPriorityGesture(drag(note, beatWidth: beatWidth, resizing: true)).help(
                        "Drag to change note length")
            }
            .frame(width: max(10, note.durationBeats * beatWidth - 2), height: InlineSoundLayout.row - 2)
            .contentShape(Rectangle()).onTapGesture {
                gridFocused = true
                insertionBeat = note.startBeat
                controller.select(note.id, extending: NSEvent.modifierFlags.contains(.shift))
            }.gesture(drag(note, beatWidth: beatWidth, resizing: false))
            .offset(x: note.startBeat * beatWidth + 1, y: Double(11 - note.pitch % 12) * InlineSoundLayout.row + 1)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(
                "\(note.name), beat \((note.startBeat + 1).formatted()), length \(note.durationBeats.formatted()) beats"
            )
            .accessibilityValue("Velocity \(Int(note.velocity * 127))")
            .accessibilityAddTraits(controller.selectedNotes.contains(note.id) ? [.isButton, .isSelected] : .isButton)
            .accessibilityAction {
                controller.select(note.id)
                gridFocused = true
            }
            .accessibilityActions {
                Button("Add to selection") { controller.select(note.id, extending: true) }
                Button("Delete note") {
                    controller.select(note.id)
                    controller.deleteNotes()
                }
            }
            .contextMenu {
                Button("Copy note") {
                    controller.select(note.id)
                    controller.copyNotes()
                }
                Button("Delete note", role: .destructive) {
                    controller.select(note.id)
                    controller.deleteNotes()
                }
            }
    }
    private func drag(_ note: SoundDesignNote, beatWidth: Double, resizing: Bool) -> some Gesture {
        DragGesture(minimumDistance: 3).onChanged { value in
            gridFocused = true
            if !controller.selectedNotes.contains(note.id) { controller.select(note.id) }
            controller.beginGesture()
            controller.drag(
                by: value.translation.width / beatWidth,
                semitones: -Int((value.translation.height / InlineSoundLayout.row).rounded()), resizing: resizing)
        }.onEnded { _ in controller.endGesture() }
    }
    private func keyPress(_ press: KeyPress) -> KeyPress.Result {
        guard controller.canEdit else { return .ignored }
        if press.modifiers.contains(.command) {
            switch press.characters.lowercased() {
            case "a":
                controller.selectedNotes = Set(pattern.notes.map(\.id))
                return .handled
            case "c":
                controller.copyNotes()
                return .handled
            case "v":
                controller.pasteNotes(at: insertionBeat)
                return .handled
            default: return .ignored
            }
        }
        if press.key == .delete {
            controller.deleteNotes()
            return .handled
        }
        let beats = press.key == .leftArrow ? -controller.grid : press.key == .rightArrow ? controller.grid : 0
        let pitchStep = press.modifiers.contains(.shift) ? 12 : 1
        let pitches = press.key == .upArrow ? pitchStep : press.key == .downArrow ? -pitchStep : 0
        if beats != 0 || pitches != 0 {
            controller.beginGesture()
            controller.drag(by: beats, semitones: pitches, resizing: beats != 0 && press.modifiers.contains(.shift))
            controller.endGesture()
            return .handled
        }
        return .ignored
    }
    private var footer: some View {
        VStack(alignment: .leading, spacing: 4) {
            if let failure = controller.failure {
                HStack {
                    Image(systemName: "exclamationmark.triangle").foregroundStyle(Theme.warning)
                    Text(failure).lineLimit(1).help(failure)
                    Spacer(minLength: 0)
                    Button("Retry") { controller.retry() }.disabled(!controller.canRetry)
                    Menu("Draft") {
                        Button("Copy draft as JSON") {
                            if let data = try? JSONEncoder().encode(pattern),
                                let text = String(data: data, encoding: .utf8)
                            {
                                NSPasteboard.general.clearContents()
                                NSPasteboard.general.setString(text, forType: .string)
                            }
                        }
                        Button(
                            controller.currentClip?.editableSoundDesign == nil
                                ? "Discard draft & close" : "Discard draft & reload"
                        ) { controller.discardDraft() }.disabled(controller.sending)
                    }.fixedSize()
                }
            } else {
                ScrollView(.horizontal) {
                    HStack(spacing: 10) {
                        Text(
                            controller.selected.map {
                                controller.selectedNotes.count == 1
                                    ? $0.name : "\(controller.selectedNotes.count) notes"
                            } ?? "Click grid to add notes"
                        ).foregroundStyle(Theme.secondary)
                        Button("Add note") {
                            controller.addNote(
                                pitch: min(127, controller.selected?.pitch ?? controller.octave * 12),
                                beat: insertionBeat)
                        }.disabled(pattern.notes.count >= 256)
                        Text("Velocity")
                        RoomSlider(
                            value: Binding(
                                get: { controller.selected?.velocity ?? 0.8 },
                                set: { value in
                                    controller.change { p in
                                        for i in p.notes.indices where controller.selectedNotes.contains(p.notes[i].id)
                                        { p.notes[i].velocity = value }
                                    }
                                }), in: 0...1, label: "Selected note velocity",
                            onEditingChanged: { $0 ? controller.beginGesture() : controller.endGesture() }
                        ).frame(width: 65).disabled(controller.selected == nil)
                        Button("Copy") { controller.copyNotes() }.disabled(controller.selected == nil)
                        Button("Paste") { controller.pasteNotes(at: insertionBeat) }
                        Button("Delete") { controller.deleteNotes() }.disabled(controller.selected == nil)
                        Text(controller.status).foregroundStyle(Theme.secondary)
                    }.frame(height: 24)
                }.scrollIndicators(.hidden).disabled(!controller.canEdit)
                Text("Drag notes to move · right edge changes length · Shift-click selects together").font(
                    .system(size: 9)
                ).foregroundStyle(Theme.secondary)
            }
        }.padding(.horizontal, 12)
    }
}

private struct InlineSoundPlayhead: View {
    @ObservedObject var position: PlaybackPosition
    let clip: Clip?
    let bpm: Double
    let beatWidth: Double
    let beats: Double
    let playing: Bool
    var body: some View {
        if playing, let clip, position.frame >= clip.start, position.frame < clip.end {
            let beat = Double(position.frame - clip.start + clip.offset) / sessionRate * bpm / 60
            if beat < beats {
                Theme.playback.frame(width: 1).offset(x: beat * beatWidth).allowsHitTesting(false).accessibilityHidden(
                    true)
            }
        }
    }
}
