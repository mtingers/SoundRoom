import SoundroomCore
import SwiftUI

/// Saved kit-designer kits on the audio server (`drums.kits` / `drums.saveKit`
/// / `drums.deleteKit`). Designs are generated live while editing; saving
/// exports them once into samples on the server.
@MainActor final class DrumKitLibrary: ObservableObject {
    @Published private(set) var kits: [DrumKitDesign] = []
    @Published private(set) var saving = false
    @Published var message = ""
    weak var owner: SessionStore?
    init(owner: SessionStore?) { self.owner = owner }
    private var remote: RemoteGUIController? { owner?.remoteGUI }
    func refresh() {
        guard let remote else {
            message = "Saved kits need the audio server."
            return
        }
        remote.route(.init(operation: "drums.kits")) { [weak self] reply in
            guard let self, reply.accepted, let data = reply.payload,
                let kits = try? JSONDecoder().decode([DrumKitDesign].self, from: data)
            else { return }
            self.kits = kits
        }
    }
    func save(_ design: DrumKitDesign, done: @escaping (Bool) -> Void) {
        guard let remote, let payload = try? JSONEncoder().encode(design) else {
            message = "Saving kits needs the audio server."
            done(false)
            return
        }
        saving = true
        message = "Exporting “\(design.name)”…"
        remote.route(.init(operation: "drums.saveKit", payload: payload)) { [weak self] reply in
            guard let self else { return }
            saving = false
            if reply.accepted {
                message = "Saved “\(design.name)”."
                if let data = reply.payload, let result = try? JSONDecoder().decode(SaveResult.self, from: data) {
                    kits = result.kits
                } else {
                    refresh()
                }
            } else {
                message = reply.message
            }
            done(reply.accepted)
        }
    }
    func delete(_ id: UUID) {
        guard let remote else { return }
        remote.route(.init(operation: "drums.deleteKit", strings: ["id": id.uuidString])) { [weak self] reply in
            guard let self else { return }
            if reply.accepted, let data = reply.payload,
                let kits = try? JSONDecoder().decode([DrumKitDesign].self, from: data)
            {
                self.kits = kits
            } else if !reply.accepted {
                message = reply.message
            }
        }
    }
    private struct SaveResult: Decodable { var kits: [DrumKitDesign] }
}

extension DrumPattern {
    /// Every piece (except clap) plays `design`: its exported samples when
    /// installed on the server, else generated from the shapes.
    mutating func use(_ design: DrumKitDesign) {
        kit = .studioModeled
        lanes = [:]
        for voice in DrumVoice.allCases where voice != .clap {
            var lane = DrumLane(kit: .studioModeled)
            lane.shape = design.shapes[voice.rawValue]
            lane.customKit = design.id
            lanes[voice.rawValue] = lane
        }
        variationCycles = 4
        velocityHumanize = 0.035
    }
    /// The saved kit every shaped piece of this pattern plays, if any.
    var customKitID: UUID? {
        let ids = Set(DrumVoice.allCases.filter { $0 != .clap }.map { lanes[$0.rawValue]?.customKit })
        return ids.count == 1 ? ids.first ?? nil : nil
    }
}

/// Step-by-step kit designer: name the kit, shape each piece with instant
/// soft / medium / hard auditions, review, save (export).
struct DrumKitDesignerView: View {
    @ObservedObject var library: DrumKitLibrary
    @ObservedObject var controller: DrumDesignerController
    var onUse: (DrumKitDesign) -> Void
    var onClose: () -> Void
    @State private var design: DrumKitDesign
    @State private var step = 0
    @State private var grooveOn = false
    @State private var showAll = false
    @State private var pendingGroove: DispatchWorkItem?
    init(
        library: DrumKitLibrary, controller: DrumDesignerController, editing: DrumKitDesign?,
        onUse: @escaping (DrumKitDesign) -> Void, onClose: @escaping () -> Void
    ) {
        self.library = library
        self.controller = controller
        self.onUse = onUse
        self.onClose = onClose
        _design = State(initialValue: editing ?? DrumKitDesign(name: "My kit"))
    }
    private static let pieces: [DrumVoice] = [.kick, .snare, .closedHat, .openHat, .highTom, .lowTom, .ride, .crash]
    private var voice: DrumVoice? { step < Self.pieces.count ? Self.pieces[step] : nil }
    private var shape: DrumShape { voice.flatMap { design.shapes[$0.rawValue] } ?? DrumShape() }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            header
            HStack(alignment: .top, spacing: 18) {
                steps
                Divider().overlay(Theme.line)
                Group { if let voice { piece(voice) } else { review } }.frame(
                    maxWidth: .infinity, alignment: .topLeading)
            }
            Divider().overlay(Theme.line)
            footer
        }
        .padding(18).frame(width: 900, height: 640)
        .foregroundStyle(Theme.ink).background(Theme.paper).tint(Theme.accent).preferredColorScheme(.dark)
        .onAppear { library.refresh() }
        .onDisappear { controller.stop() }
        .onChange(of: design) { _, _ in if grooveOn { scheduleGroove() } }
    }
    private var header: some View {
        HStack(spacing: 12) {
            Text("Kit designer").font(.system(size: 21, weight: .semibold, design: .rounded))
            TextField("Kit name", text: $design.name).textFieldStyle(.roundedBorder).frame(width: 220)
            Menu("Open kit") {
                Button("New kit") {
                    design = DrumKitDesign(name: "My kit")
                    step = 0
                }
                if !library.kits.isEmpty { Divider() }
                ForEach(library.kits) { kit in
                    Button(kit.name) {
                        design = kit
                        step = 0
                    }
                }
            }.frame(width: 120)
            Spacer()
            Text(library.message).font(.system(size: 11)).foregroundStyle(Theme.secondary).lineLimit(1)
        }.buttonStyle(RoomButtonStyle())
    }
    private var steps: some View {
        VStack(alignment: .leading, spacing: 4) {
            ForEach(Array(Self.pieces.enumerated()), id: \.offset) { index, piece in
                Button {
                    step = index
                    controller.stop()
                    grooveOn = false
                } label: {
                    HStack {
                        Image(systemName: design.shapes[piece.rawValue] == nil ? "circle" : "checkmark.circle.fill")
                            .font(.system(size: 11))
                        Text(piece.title)
                    }.frame(maxWidth: .infinity, alignment: .leading).padding(.vertical, 5).padding(.horizontal, 8)
                        .background(step == index ? Theme.raised : Color.clear, in: RoundedRectangle(cornerRadius: 5))
                }.buttonStyle(.plain).foregroundStyle(step == index ? Theme.accent : Theme.ink)
            }
            Button {
                step = Self.pieces.count
                controller.stop()
                grooveOn = false
            } label: {
                Label("Review & save", systemImage: "square.and.arrow.down").frame(
                    maxWidth: .infinity, alignment: .leading
                ).padding(.vertical, 5).padding(.horizontal, 8)
                    .background(voice == nil ? Theme.raised : Color.clear, in: RoundedRectangle(cornerRadius: 5))
            }.buttonStyle(.plain).foregroundStyle(voice == nil ? Theme.accent : Theme.ink).padding(.top, 6)
        }.font(.system(size: 12)).frame(width: 160)
    }

    // MARK: Piece page
    private func piece(_ voice: DrumVoice) -> some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack {
                VStack(alignment: .leading, spacing: 3) {
                    Text(voice.title).font(.system(size: 17, weight: .semibold))
                    Text(Self.blurb(voice)).font(.system(size: 11)).foregroundStyle(Theme.secondary)
                }
                Spacer()
                Button("Reset") {
                    update { _ in DrumShape() }
                    design.shapes[voice.rawValue] = nil
                }.disabled(design.shapes[voice.rawValue] == nil)
            }
            HStack(spacing: 10) {
                ForEach([("Soft", 0.3), ("Medium", 0.6), ("Hard", 0.95)], id: \.0) { title, velocity in
                    Button {
                        hit(voice, velocity)
                    } label: {
                        Label(title, systemImage: "speaker.wave.2").frame(width: 92)
                    }
                }
                Toggle(
                    "Play in a groove",
                    isOn: Binding(
                        get: { grooveOn },
                        set: { on in
                            grooveOn = on
                            if on { playGroove() } else { controller.stop() }
                        })
                )
                .toggleStyle(.switch).font(.system(size: 11)).padding(.leading, 8)
            }.buttonStyle(RoomButtonStyle())
            quickControls(voice)
            DisclosureGroup("All controls", isExpanded: $showAll) { allControls(voice).padding(.top, 8) }.font(
                .system(size: 12))
            Spacer(minLength: 0)
        }
    }
    @ViewBuilder private func quickControls(_ voice: DrumVoice) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            switch voice {
            case .kick:
                picker(
                    "Beater", ["Felt", "Plastic", "Wood"],
                    selection: Binding(
                        get: { shape.snap < -3 ? 0 : shape.snap > 3 ? 2 : 1 },
                        set: { i in
                            update {
                                var s = $0
                                (s.snap, s.tone) = [(-8, -0.25), (0, 0), (6, 0.25)][i]
                                return s
                            }
                        }))
                row {
                    slider("Resonant head ring", \.sustain, 0.25...3, times)
                    slider("Body", \.body, -24...12, db)
                    slider("Pitch drop", \.pitchDrop, 0...3, times)
                }
            case .snare:
                Toggle(
                    "Snare wires",
                    isOn: Binding(
                        get: { shape.wires > -48 },
                        set: { on in
                            update {
                                var s = $0
                                s.wires = on ? 0 : -60
                                return s
                            }
                        })
                ).toggleStyle(.switch).font(.system(size: 12))
                row {
                    slider("Wires", \.wires, -24...12, db, disabled: shape.wires <= -48)
                    slider("Tension", \.tension, -12...12, semitones)
                    slider("Ring", \.sustain, 0.25...3, times)
                }
            case .highTom, .lowTom:
                row {
                    slider("Tension", \.tension, -12...12, semitones)
                    slider("Ring", \.sustain, 0.25...3, times)
                    slider("Pitch drop", \.pitchDrop, 0...3, times)
                }
            case .closedHat, .openHat:
                row {
                    slider(voice == .closedHat ? "Tightness" : "Openness", \.sustain, 0.25...3, times)
                    slider("Brightness", \.tone, -1...1, percent)
                    slider("Stick", \.snap, -24...12, db)
                }
            case .ride, .crash:
                row {
                    sliderView(
                        "Size",
                        value: Binding(
                            get: { -shape.tension / 6 },
                            set: { size in
                                update {
                                    var s = $0
                                    s.tension = -6 * size
                                    s.sustain = max(0.25, 1 + 0.6 * size)
                                    return s
                                }
                            }), range: -1...1, text: String(format: "%+.0f%%", -shape.tension / 6 * 100),
                        help: "Bigger cymbals are lower and ring longer")
                    slider("Brightness", \.tone, -1...1, percent)
                    slider("Wash", \.wires, -24...12, db)
                }
            case .clap: EmptyView()
            }
        }
    }
    private func allControls(_ voice: DrumVoice) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            row {
                slider("Tension", \.tension, -12...12, semitones)
                slider("Sustain", \.sustain, 0.25...3, times)
                slider("Body", \.body, -24...12, db)
            }
            row {
                slider("Snap", \.snap, -24...12, db)
                slider(voice == .snare ? "Wires" : "Wash / rasp", \.wires, -60...12, db)
                slider("Tone", \.tone, -1...1, percent)
            }
            row {
                slider("Pitch drop", \.pitchDrop, 0...3, times)
                slider("Room", \.room, -36...9, db)
                slider("Room size", \.roomSize, 0.4...2.5, times)
            }
            row { slider("Variation", \.variation, 0...2.5, times) }
        }
    }

    // MARK: Review page
    private var review: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Review “\(design.name)”").font(.system(size: 17, weight: .semibold))
            Text(
                "Saving renders every piece at five strengths with alternate takes on the server, so the kit plays instantly in the drum loop designer. The settings are kept with the kit and can be edited later."
            )
            .font(.system(size: 11)).foregroundStyle(Theme.secondary).fixedSize(horizontal: false, vertical: true)
            ForEach(Self.pieces, id: \.self) { piece in
                HStack {
                    Text(piece.title).frame(width: 110, alignment: .leading)
                    Text(Self.summary(design.shapes[piece.rawValue])).foregroundStyle(Theme.secondary)
                }.font(.system(size: 12))
            }
            HStack(spacing: 10) {
                Button {
                    save(andUse: false)
                } label: {
                    Label("Save kit", systemImage: "square.and.arrow.down")
                }
                Button {
                    save(andUse: true)
                } label: {
                    Text("Save and use in this loop")
                }.buttonStyle(RoomButtonStyle(prominent: true))
                if library.kits.contains(where: { $0.id == design.id }) {
                    Button("Delete kit", role: .destructive) {
                        library.delete(design.id)
                        design = DrumKitDesign(name: "My kit")
                        step = 0
                    }
                }
                if library.saving { ProgressView().controlSize(.small) }
            }.buttonStyle(RoomButtonStyle()).disabled(
                library.saving || design.name.trimmingCharacters(in: .whitespaces).isEmpty)
            Spacer(minLength: 0)
        }
    }
    private var footer: some View {
        HStack {
            Button("Back") {
                step = max(0, step - 1)
                controller.stop()
                grooveOn = false
            }.disabled(step == 0)
            Button(voice == nil ? "Done" : "Next") {
                if voice == nil {
                    onClose()
                } else {
                    step += 1
                    controller.stop()
                    grooveOn = false
                }
            }
            Spacer()
            Button("Close") { onClose() }
        }.buttonStyle(RoomButtonStyle())
    }

    // MARK: Controls
    private func row<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        HStack(alignment: .top, spacing: 18) { content() }
    }
    private func picker(_ title: String, _ options: [String], selection: Binding<Int>) -> some View {
        HStack {
            Text(title).font(.system(size: 12))
            Picker(title, selection: selection) { ForEach(options.indices, id: \.self) { Text(options[$0]).tag($0) } }
                .pickerStyle(.segmented).labelsHidden().frame(width: 260)
        }
    }
    private func slider(
        _ title: String, _ key: WritableKeyPath<DrumShape, Double>, _ range: ClosedRange<Double>,
        _ format: (Double) -> String, disabled: Bool = false
    ) -> some View {
        sliderView(
            title,
            value: Binding(
                get: { shape[keyPath: key] },
                set: { value in
                    update {
                        var s = $0
                        s[keyPath: key] = min(range.upperBound, max(range.lowerBound, value))
                        return s
                    }
                }),
            range: range, text: format(shape[keyPath: key]), help: nil
        ).disabled(disabled).opacity(disabled ? 0.45 : 1)
    }
    private func sliderView(
        _ title: String, value: Binding<Double>, range: ClosedRange<Double>, text: String, help: String?
    ) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                Text(title)
                Spacer()
                Text(text).foregroundStyle(Theme.secondary).monospacedDigit()
            }.font(.system(size: 11))
            RoomSlider(
                value: value, in: range, label: title,
                onEditingChanged: { editing in if !editing, let voice { hit(voice, 0.8) } })
        }.frame(width: 190).help(help ?? title)
    }
    private func db(_ v: Double) -> String { v <= -48 ? "off" : v == 0 ? "0 dB" : String(format: "%+.1f dB", v) }
    private func times(_ v: Double) -> String { String(format: "×%.2f", v) }
    private func semitones(_ v: Double) -> String { String(format: "%+.1f st", v) }
    private func percent(_ v: Double) -> String { v == 0 ? "neutral" : String(format: "%+.0f%%", v * 100) }
    private func update(_ change: (DrumShape) -> DrumShape) {
        guard let voice else { return }
        let next = change(shape)
        design.shapes[voice.rawValue] = next == DrumShape() ? nil : next
    }

    // MARK: Audition
    private var designPattern: DrumPattern {
        var p = DrumPattern()
        p.use(DrumKitDesign(id: design.id, name: design.name, shapes: design.shapes))
        // Live design: generate from the shapes, not a previously saved export.
        for key in p.lanes.keys { p.lanes[key]?.customKit = nil }
        return p
    }
    private func hit(_ voice: DrumVoice, _ velocity: Double) {
        grooveOn = false
        var p = designPattern
        p.steps = [DrumStep(voice: voice, index: 0, velocity: velocity)]
        controller.preview(p, audition: voice, velocity: velocity)
    }
    private func playGroove() {
        var p = designPattern
        let lanes = p.lanes
        p.apply(.rock)
        p.kit = .studioModeled
        p.lanes = lanes
        p.variationCycles = 2
        p.velocityHumanize = 0.03
        controller.preview(p)
    }
    /// While the groove plays, re-render a moment after the last change.
    private func scheduleGroove() {
        pendingGroove?.cancel()
        let work = DispatchWorkItem { if grooveOn { playGroove() } }
        pendingGroove = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.35, execute: work)
    }
    private func save(andUse: Bool) {
        var saved = design
        saved.name = saved.name.trimmingCharacters(in: .whitespacesAndNewlines)
        controller.stop()
        grooveOn = false
        library.save(saved) { ok in if ok && andUse { onUse(saved) } }
    }

    // MARK: Text
    private static func blurb(_ voice: DrumVoice) -> String {
        switch voice {
        case .kick: return "Beater material, how long the resonant head rings, and the low body."
        case .snare: return "Wires on or off, head tension and ring. Turn wires off for a tom-like snare."
        case .closedHat, .openHat: return "How tight or open the hats sound, their brightness and stick attack."
        case .highTom, .lowTom: return "Tuning, how long the tom rings, and the pitch drop after the hit."
        case .ride, .crash: return "Cymbal size (bigger is lower and longer), brightness and wash."
        case .clap: return ""
        }
    }
    private static func summary(_ shape: DrumShape?) -> String {
        guard let s = shape else { return "As measured" }
        var parts: [String] = []
        if s.wires <= -48 { parts.append("wires off") }
        if s.tension != 0 { parts.append(String(format: "tension %+.1f st", s.tension)) }
        if s.sustain != 1 { parts.append(String(format: "ring ×%.2f", s.sustain)) }
        if s.body != 0 { parts.append(String(format: "body %+.0f dB", s.body)) }
        if s.snap != 0 { parts.append(String(format: "snap %+.0f dB", s.snap)) }
        if s.wires != 0 && s.wires > -48 { parts.append(String(format: "wires %+.0f dB", s.wires)) }
        if s.tone != 0 { parts.append(String(format: "tone %+.0f%%", s.tone * 100)) }
        if s.room != 0 || s.roomSize != 1 { parts.append("room changed") }
        return parts.isEmpty ? "As measured" : parts.joined(separator: " · ")
    }
}
