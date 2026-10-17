import SoundroomCore
import SwiftUI

/// All 300 samples are editable. Curves are stored with the effect and included
/// in ordinary project Undo, selection preview/apply and personal presets.
struct SoundPuttyEditor: View {
    let values: [String: Float]
    let onChange: ([String: Float]) -> Void
    let onEditingChanged: (String, Bool) -> Void
    @State private var axis = SoundPuttyShape.Axis.pitch
    @State private var brushWidth: Double = 22
    @State private var brushLane = 145
    @State private var brushTarget: Double = 0
    @State private var draft: [String: Float]?
    @State private var painting = false
    @State private var movingWildcard = false
    private var current: [String: Float] { draft ?? values }
    private var color: Color { axis == .pitch ? .blue : axis == .energy ? .orange : .purple }

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            HStack {
                Text("300-lane field · settings").font(.system(size: 11, weight: .semibold))
                Spacer()
                Button("New variation") { onChange(["variationSeed": Float(UInt32.random(in: 0...16_777_215))]) }
                    .buttonStyle(RoomButtonStyle()).font(.system(size: 10))
                    .disabled((values["wildMode"] ?? 0) != 0)
                    .help("Choose a new repeatable wildcard detuning pattern. Keeps all sculpt curves.")
            }
            field
            Text("Track audio excites the field. Drag the gold band to move the wildcard.")
                .font(.system(size: 10)).foregroundStyle(Theme.secondary).fixedSize(horizontal: false, vertical: true)
            Picker("Sculpt", selection: $axis) {
                ForEach(SoundPuttyShape.Axis.allCases) { axis in Text(axis.name).tag(axis) }
            }.pickerStyle(.segmented).accessibilityLabel("Sculpted property")
            curve
            HStack {
                Text("Lane 0")
                Spacer()
                Text("Lane 299")
            }.font(.system(size: 9)).foregroundStyle(Theme.secondary)
            HStack {
                Text("Brush width")
                Spacer()
                Text("\(Int(brushWidth)) lanes").monospacedDigit()
            }.font(.system(size: 10))
            RoomSlider(value: $brushWidth, in: 2...90, label: "Sculpt brush width")
            DisclosureGroup("Keyboard sculpting") {
                VStack(alignment: .leading, spacing: 7) {
                    Stepper("Brush lane \(brushLane)", value: $brushLane, in: 0...299).accessibilityLabel("Brush lane")
                    HStack {
                        Text("Target")
                        Spacer()
                        Text(String(format: "%+.2f", brushTarget)).monospacedDigit()
                    }
                    Slider(value: $brushTarget, in: -1...1, step: 0.05).accessibilityLabel(
                        "Brush target for \(axis.name.lowercased())")
                    Button("Apply brush") {
                        onEditingChanged("graph", true)
                        onChange(
                            SoundPuttyShape(parameters: values).brush(
                                axis: axis, lane: Double(brushLane), target: brushTarget, radius: brushWidth))
                        onEditingChanged("graph", false)
                    }.buttonStyle(RoomButtonStyle())
                }.font(.system(size: 11)).padding(.top, 6)
            }.font(.system(size: 11))
            HStack {
                Button("Flatten \(axis.name.lowercased())") {
                    onChange(
                        Dictionary(uniqueKeysWithValues: (0..<300).map { (SoundPuttyShape.key(axis, $0), Float(0)) }))
                }
                Spacer()
                Button("Reset all curves") { onChange(SoundPuttyShape.resetParameters) }
            }.buttonStyle(RoomButtonStyle()).font(.system(size: 10))
        }.padding(10).background(Theme.card, in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).stroke(Theme.line))
            .onDisappear {
                if painting { onEditingChanged("graph", false) }
                if movingWildcard { onEditingChanged("wild", false) }
            }
    }
    private var field: some View {
        let shape = SoundPuttyShape(parameters: current)
        let frequencies = shape.frequencies(parameters: current)
        let wild = Double(current["wild"] ?? 160)
        let width = Double(current["width"] ?? 22)
        let intensity = Double(current["intensity"] ?? 0.7)
        return Canvas { context, size in
            for i in 0..<300 {
                let zone = exp(-0.5 * pow((Double(i) - wild) / max(1, width), 2)) * intensity
                let y = 4 + Double(i) / 299 * (size.height - 8)
                var path = Path()
                for step in 0...32 {
                    let x = Double(step) / 32 * size.width
                    let offset =
                        sin(Double(step) / 32 * 2 * .pi * frequencies[i] / 560 * 4 + Double(i) * 0.13)
                        * (0.2 + zone * 0.45)
                    if step == 0 {
                        path.move(to: CGPoint(x: x, y: y + offset))
                    } else {
                        path.addLine(to: CGPoint(x: x, y: y + offset))
                    }
                }
                context.stroke(
                    path, with: .color(zone > 0.06 ? .orange.opacity(0.25 + zone * 0.6) : .blue.opacity(0.16)),
                    lineWidth: 0.5)
            }
            let y = 4 + wild / 299 * (size.height - 8)
            var marker = Path()
            marker.move(to: CGPoint(x: 0, y: y))
            marker.addLine(to: CGPoint(x: size.width, y: y))
            context.stroke(marker, with: .color(.orange), style: StrokeStyle(lineWidth: 1.5, dash: [4, 3]))
        }.frame(height: 120).background(Theme.paper, in: RoundedRectangle(cornerRadius: 6))
            .contentShape(Rectangle())
            .overlay(alignment: .topTrailing) {
                Text("Wildcard \(Int(wild))").font(.system(size: 9)).padding(4).background(Theme.paper.opacity(0.9))
                    .allowsHitTesting(false)
            }
            .gesture(
                DragGesture(minimumDistance: 0).onChanged { event in
                    if !movingWildcard {
                        movingWildcard = true
                        onEditingChanged("wild", true)
                    }
                    onChange(["wild": Float(max(0, min(299, ((event.location.y - 4) / 112 * 299).rounded())))])
                }.onEnded { _ in
                    movingWildcard = false
                    onEditingChanged("wild", false)
                }
            )
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("300 coupled resonators, wildcard at lane \(Int(wild))")
            .accessibilityHint("Use the Wildcard lane control to move the band with a keyboard.")
    }
    private var curve: some View {
        let row = SoundPuttyShape(parameters: current).rows[axis.rawValue]
        return GeometryReader { geometry in
            Canvas { context, size in
                for fraction in [0.15, 0.5, 0.85] {
                    var grid = Path()
                    grid.move(to: CGPoint(x: 0, y: size.height * fraction))
                    grid.addLine(to: CGPoint(x: size.width, y: size.height * fraction))
                    context.stroke(grid, with: .color(Theme.line), lineWidth: 0.5)
                }
                var path = Path()
                for i in 0..<300 {
                    let point = CGPoint(
                        x: 8 + Double(i) / 299 * (size.width - 16), y: size.height * 0.5 - row[i] * size.height * 0.35)
                    if i == 0 { path.move(to: point) } else { path.addLine(to: point) }
                }
                context.stroke(
                    path, with: .color(color), style: StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round))
            }.contentShape(Rectangle()).gesture(
                DragGesture(minimumDistance: 0).onChanged { event in
                    if !painting {
                        draft = values
                        painting = true
                        onEditingChanged("graph", true)
                    }
                    let lane = max(0, min(299, (event.location.x - 8) / max(1, geometry.size.width - 16) * 299))
                    let target = max(
                        -1, min(1, (geometry.size.height * 0.5 - event.location.y) / (geometry.size.height * 0.35)))
                    let edits = SoundPuttyShape(parameters: current).brush(
                        axis: axis, lane: lane, target: target, radius: brushWidth)
                    draft = current.merging(edits) { _, new in new }
                    onChange(edits)
                }.onEnded { _ in
                    painting = false
                    draft = nil
                    onEditingChanged("graph", false)
                })
        }.frame(height: 110).background(Theme.paper, in: RoundedRectangle(cornerRadius: 6))
            .accessibilityElement(children: .ignore).accessibilityLabel("\(axis.name) sculpting curve, 300 lanes")
            .accessibilityHint("Use Keyboard sculpting to choose a lane, target and brush width, then Apply brush.")
    }
}
