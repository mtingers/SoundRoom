import AppKit
import SoundroomCore
import SwiftUI

/// The same source-timeline editor is embedded in selection and live-insert inspectors.
/// The owner handles rendering/persistence; this view never starts the main transport.
struct PitchPointEditorView: View {
    @Binding var regions: [PitchEditRegion]
    let before: [PitchContourPoint]
    let after: [PitchContourPoint]?
    let scope: Range<Int>
    var reference: Float = 440
    var busy = false
    var isPreviewing = false
    var canBake = true
    var title = "Pitch editor"
    var context = "Measured source · targets are non-destructive"
    var bakeTitle = "Apply to audio"
    var error: String?
    var onBeginEdit: (String) -> Void = { _ in }
    var onEndEdit: () -> Void = {}
    var onAnalyze: () -> Void = {}
    var onRecheck: () -> Void = {}
    var onAudition: () -> Void = {}
    var onAuditionOriginal: () -> Void = {}
    var onStop: () -> Void = {}
    var onBake: () -> Void = {}
    var onResetTargets: (PitchCurvePreset) -> Void = { _ in }
    var expanded = false
    var canUndo = false
    var canRedo = false
    var onUndo: () -> Void = {}
    var onRedo: () -> Void = {}
    var onExpand: () -> Void = {}

    @State private var editing = PitchPointEditing()
    @State private var viewport = PitchPointViewport()
    @State private var snap = false
    @State private var smoothing = 0.5
    @State private var hoverFrame: Int?
    @State private var dragOriginal: [PitchEditRegion]?
    @State private var dragPreview: [PitchEditRegion]?
    @State private var dragPoint: UUID?
    @State private var dragStartFrame: Int?
    @State private var dragStarted = false
    @State private var initialized = false
    @State private var chartPoints: [PitchEditPoint] = []
    @State private var preparedTargets: [PreparedPitchChartRegion] = []

    private var points: [PitchEditPoint] { chartPoints }
    private var selectedPoint: PitchEditPoint? { editing.selectedPoint(in: dragPreview ?? regions) }
    private var safeReference: Float { reference.isFinite && reference > 0 ? reference : 440 }
    private var selectedPoints: [PitchEditPoint] { points.filter { editing.selected.contains($0.id) } }
    private var hasTargets: Bool { !regions.isEmpty }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            header
            if !before.isEmpty || hasTargets {
                zoomTools.disabled(busy)
                chart
                legend
                editingTools
                numericTools
                smoothingTools
                actions
            } else {
                Text("Select audio, then Analyze to edit measured pitch while stopped.").foregroundStyle(
                    Theme.secondary)
            }
            if let error { Text(error).foregroundStyle(Theme.secondary).fixedSize(horizontal: false, vertical: true) }
        }.font(.system(size: 10)).padding(expanded ? 16 : 10)
            .background(Theme.raised, in: RoundedRectangle(cornerRadius: 7))
            .background {
                if expanded {
                    PitchEditorKeyboardBridge(
                        onSpace: {
                            guard !busy else { return }
                            if isPreviewing { onStop() } else if hasTargets { onAudition() }
                        }, onUndo: { if canUndo && !busy { onUndo() } }, onRedo: { if canRedo && !busy { onRedo() } })
                }
            }
            .onAppear {
                prepareTargets()
                if !before.isEmpty || hasTargets { fit() }
            }
            .onChange(of: scope) { _, _ in
                finishDrag(commit: false)
                editing = PitchPointEditing()
                prepareTargets()
                fit()
            }
            .onChange(of: before) { _, _ in if !initialized, !before.isEmpty { fit() } }
            .onChange(of: regions) { _, value in
                if let original = dragOriginal, value != original { finishDrag(commit: false) }
                prepareTargets()
                editing.reconcile(value)
            }
            .onChange(of: busy) { _, value in if value { finishDrag(commit: false) } }
            .onDisappear {
                finishDrag()
                if expanded && isPreviewing { onStop() }
            }
    }
    private var header: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                if !expanded {
                    Text(title).fontWeight(.medium)
                        .help(
                            "Single voice or instrument, 50–1000 Hz. Correction is limited to one octave. Targets use track timeline time and follow Move with automation. Recheck independently measures the output."
                        )
                }
                Spacer()
                ZStack {
                    if busy { ProgressView().controlSize(.mini).accessibilityLabel("Preparing pitch audio") }
                }.frame(width: 14, height: 14).accessibilityHidden(!busy)
                Button(before.isEmpty ? "Analyze" : "Analyze again", action: onAnalyze)
                    .buttonStyle(.borderless).frame(width: 82).disabled(busy || scope.isEmpty)
                if !expanded {
                    Button(action: onExpand) { Image(systemName: "arrow.up.left.and.arrow.down.right") }
                        .buttonStyle(.borderless).help("Open larger pitch editor").accessibilityLabel(
                            "Expand pitch editor")
                }
            }
            Text(context).font(.system(size: 9)).foregroundStyle(Theme.secondary)
                .lineLimit(expanded ? 1 : 2).frame(maxWidth: .infinity, alignment: .leading)
                .frame(height: expanded ? 12 : 24, alignment: .topLeading).help(context)
        }
    }
    private var zoomTools: some View {
        VStack(spacing: 4) {
            HStack(spacing: 4) {
                Text("Time").frame(width: 30, alignment: .leading)
                icon("minus", "Zoom out in time") { viewport.zoomTime(0.5) }
                icon("plus", "Zoom in in time") { viewport.zoomTime(2, around: selectedPoint?.frame ?? hoverFrame) }
                icon("chevron.left", "Earlier time") { viewport.panTime(-1) }
                icon("chevron.right", "Later time") { viewport.panTime(1) }
                Spacer(minLength: 0)
                Button("Fit") { fit() }.help("Fit measured pitches and targets across the selection")
                Button("Selected") { viewport.fitSelected(selectedPoints) }.disabled(editing.selected.isEmpty).help(
                    "Zoom to selected points")
            }
            HStack(spacing: 4) {
                Text("Pitch").frame(width: 30, alignment: .leading)
                icon("minus", "Zoom out in pitch") { viewport.zoomPitch(0.5) }
                icon("plus", "Zoom in in pitch") { viewport.zoomPitch(2) }
                icon("chevron.down", "Show lower pitches") { viewport.panPitch(-1) }
                icon("chevron.up", "Show higher pitches") { viewport.panPitch(1) }
                Spacer(minLength: 0)
                Text(
                    String(
                        format: "%.3f–%.3f s", Double(viewport.frames.lowerBound) / sessionRate,
                        Double(viewport.frames.upperBound) / sessionRate)
                ).font(.system(size: 8)).monospacedDigit()
            }
        }.controlSize(.mini)
    }
    private var legend: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 8) {
                Text("Input").foregroundStyle(Theme.tracks[1])
                Text("Target").foregroundStyle(Theme.tracks[2])
                Text(after == nil ? "Output · recheck" : "Output · measured").foregroundStyle(
                    after == nil ? Theme.secondary : Theme.accent)
                Spacer(minLength: 0)
            }
            Text(
                hoverFrame.map { reading(at: $0) }
                    ?? "Click a location, then Add point · Drag points · Shift selects several · drag empty space to select"
            )
            .font(.system(size: 9)).foregroundStyle(Theme.secondary).monospacedDigit()
            .lineLimit(expanded ? 1 : 2).frame(maxWidth: .infinity, alignment: .leading)
            .frame(height: expanded ? 12 : 24, alignment: .topLeading)
            .accessibilityLabel(
                hoverFrame.map { reading(at: $0) }
                    ?? "Click a location, then Add point. Drag points. Shift adds to selection. Drag empty space to select."
            )
        }
    }
    private var editingTools: some View {
        VStack(spacing: 5) {
            HStack(spacing: 5) {
                Text("\(editing.selected.count) selected").foregroundStyle(Theme.secondary)
                Spacer(minLength: 0)
                Toggle("Note snap", isOn: $snap).toggleStyle(.checkbox).help(
                    "Snap the dragged anchor to a semitone; selected points preserve their relative pitch")
            }
            HStack(spacing: 5) {
                Button("Add point") { addPoint() }.disabled(!canAdd).help(
                    "Click a location in the chart, then add a point at that time and pitch. Points stay within measured voiced regions."
                )
                Button("Delete") { change("Delete pitch points", editing.deleting(regions)) }.disabled(
                    editing.selected.isEmpty)
                Spacer(minLength: 0)
            }.controlSize(.small)
            HStack(spacing: 5) {
                Menu("Curve") {
                    ForEach(AutomationCurve.allCases) { curve in
                        Button(curve.name) {
                            change("\(curve.name) pitch curve", editing.settingCurve(curve, in: regions))
                        }
                    }
                }.disabled(editing.selected.isEmpty).help("Curve follows each selected point until its next point")
                Menu("Targets") {
                    ForEach(PitchCurvePreset.allCases) { preset in Button(preset.rawValue) { onResetTargets(preset) } }
                }.help("Rebuild targets from measured input; leaves input audio intact")
            }.controlSize(.small)
        }.disabled(busy)
    }
    private var numericTools: some View {
        VStack(spacing: 4) {
            HStack(spacing: 4) {
                Text("Time").foregroundStyle(Theme.secondary)
                CommittedNumberField(
                    "Pitch point time",
                    value: Binding(
                        get: { Double(selectedPoint?.frame ?? scope.lowerBound) / sessionRate },
                        set: { value in
                            guard let point = selectedPoint, value.isFinite else { return }
                            let frame = Int(
                                max(Double(scope.lowerBound), min(Double(scope.upperBound), value * sessionRate)))
                            change(
                                "Move pitch points in time",
                                editing.moving(regions, frames: frame - point.frame, notes: 0))
                        }),
                    range: Double(scope.lowerBound) / sessionRate...Double(max(scope.lowerBound, scope.upperBound))
                        / sessionRate,
                    fractionDigits: 4, onSubmit: focusChart
                ).frame(width: 76).accessibilityLabel("Selected pitch point time in seconds")
                Text("s").foregroundStyle(Theme.secondary)
                Spacer(minLength: 0)
                icon("chevron.left", "Move selected points earlier by one millisecond") { nudge(frames: -48, notes: 0) }
                icon("chevron.right", "Move selected points later by one millisecond") { nudge(frames: 48, notes: 0) }
            }
            HStack(spacing: 4) {
                Text("Note").foregroundStyle(Theme.secondary)
                Picker(
                    "Selected pitch note",
                    selection: Binding(
                        get: { Int((selectedPoint?.note ?? 69).rounded()) },
                        set: { note in
                            guard let point = selectedPoint else { return }
                            let cents = point.note - point.note.rounded()
                            nudge(frames: 0, notes: Float(note) + cents - point.note)
                        })
                ) { ForEach(0...95, id: \.self) { note in Text(Self.noteName(Float(note))).tag(note) } }.labelsHidden()
                    .frame(width: 65)
                CommittedNumberField(
                    "Pitch cents",
                    value: Binding(
                        get: { Double(((selectedPoint?.note ?? 69) - (selectedPoint?.note ?? 69).rounded()) * 100) },
                        set: { cents in
                            guard let point = selectedPoint, cents.isFinite else { return }
                            nudge(frames: 0, notes: point.note.rounded() + Float(cents) / 100 - point.note)
                        }), range: -50...50, fractionDigits: 1, onSubmit: focusChart
                ).frame(width: 50).accessibilityLabel("Selected pitch point cents from note")
                Text("¢").foregroundStyle(Theme.secondary)
                Spacer(minLength: 0)
                icon("minus", "Lower selected points by one semitone") { nudge(frames: 0, notes: -1) }
                icon("plus", "Raise selected points by one semitone") { nudge(frames: 0, notes: 1) }
            }
        }.textFieldStyle(.roundedBorder).controlSize(.small).disabled(busy || selectedPoint == nil)
    }
    private var smoothingTools: some View {
        HStack(spacing: 5) {
            Text("Smooth").foregroundStyle(Theme.secondary)
            RoomSlider(value: $smoothing, in: 0...1, label: "Local pitch smoothing strength")
            Text("\(Int(smoothing * 100))%").monospacedDigit().frame(width: 30)
            Button("Apply") { change("Smooth pitch points", editing.smoothing(regions, amount: Float(smoothing))) }
                .help(
                    "Smooth selected interior targets, keeping region endpoints and times. Separate from Retune and Humanize."
                )
        }.controlSize(.small).disabled(busy || editing.selected.isEmpty)
    }
    private var actions: some View {
        VStack(spacing: 5) {
            HStack(spacing: 5) {
                Button("Original") { finishNumberEntry(onAuditionOriginal) }.help(
                    "Audition the same chain with this pitch effect removed")
                Button(isPreviewing ? "Stop" : "Audition") { finishNumberEntry(isPreviewing ? onStop : onAudition) }
                    .frame(width: 72).help("Listen to this selection")
                Spacer(minLength: 0)
            }
            HStack(spacing: 5) {
                Button("Recheck") { finishNumberEntry(onRecheck) }.help("Measure the rendered result while stopped")
                    .disabled(!hasTargets)
                Spacer(minLength: 0)
                Button(bakeTitle) { finishNumberEntry(onBake) }.disabled(!canBake).help(
                    context + ". Undo and clip history retain the original audio.")
            }
        }.controlSize(.small).disabled(busy || before.isEmpty)
    }
    private var canAdd: Bool {
        let target = editing.insertionTarget(
            in: regions, defaultFrame: viewport.frames.lowerBound + viewport.frames.count / 2)
        return regions.contains { $0.range.contains(target.frame) && !$0.points.contains { $0.frame == target.frame } }
    }
    private func icon(_ name: String, _ label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) { Image(systemName: name).frame(width: 10) }.help(label).accessibilityLabel(label)
    }
    private func fit() {
        viewport.fit(
            scope: scope,
            notes: before.filter(\.isVoiced).map { midi($0.frequency) }
                + (after ?? []).filter(\.isVoiced).map { midi($0.frequency) } + points.map(\.note))
        initialized = true
    }
    private func prepareTargets(_ preview: [PitchEditRegion]? = nil) {
        let displayed = preview ?? regions
        preparedTargets = displayed.map(PreparedPitchChartRegion.init)
        let values = displayed.flatMap(\.points)
        chartPoints =
            zip(values, values.dropFirst()).allSatisfy { $0.frame <= $1.frame }
            ? values : values.sorted { $0.frame < $1.frame }
    }
    private func change(_ name: String, _ result: [PitchEditRegion]) {
        guard !busy, result != regions else { return }
        onBeginEdit(name)
        regions = result
        onEndEdit()
        editing.reconcile(result)
    }
    private func nudge(frames: Int, notes: Float) {
        change("Move pitch points", editing.moving(regions, frames: frames, notes: notes))
    }
    private func addPoint() {
        let target = editing.insertionTarget(
            in: regions, defaultFrame: viewport.frames.lowerBound + viewport.frames.count / 2)
        let added = editing.adding(to: regions, frame: target.frame, note: snap ? target.note.rounded() : target.note)
        change("Add pitch point", added.regions)
        if let id = added.point { editing.select(id, extending: false) }
    }
    private func midi(_ frequency: Float) -> Float { 69 + 12 * log2(frequency / safeReference) }
    nonisolated static func noteName(_ note: Float, cents: Bool = false) -> String {
        guard note.isFinite else { return "—" }
        let n = Int(note.rounded())
        let names = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"]
        let name = "\(names[(n % 12 + 12) % 12])\(Int(floor(Double(n) / 12)) - 1)"
        return cents ? name + String(format: " %+.1f¢", (note - Float(n)) * 100) : name
    }
    private func reading(at frame: Int) -> String {
        func nearest(_ contour: [PitchContourPoint]) -> String {
            guard
                let point = contour.min(by: {
                    abs(Double($0.frame) - Double(frame)) < abs(Double($1.frame) - Double(frame))
                }), point.isVoiced
            else { return "unvoiced" }
            return Self.noteName(midi(point.frequency), cents: true)
        }
        let target =
            regions.first { $0.range.contains(frame) }?.note(at: frame).map { Self.noteName($0, cents: true) }
            ?? "automatic"
        return String(
            format: "%.4f s · %@ · target %@ · %@", Double(frame) / sessionRate, nearest(before), target,
            after.map(nearest) ?? "recheck output")
    }
    private func finishDrag(commit: Bool = true) {
        let pending = dragPreview
        let started = dragStarted
        dragOriginal = nil
        dragPreview = nil
        dragPoint = nil
        dragStartFrame = nil
        dragStarted = false
        // Draw the private gesture locally; author its final value once on release.
        if commit, started, let pending, pending != regions { regions = pending }
        if started { onEndEdit() }
        prepareTargets()
    }
    private func focusChart() {
        guard expanded, let window = NSApp.keyWindow else { return }
        window.makeFirstResponder(nil)
    }
    private func finishNumberEntry(_ action: @escaping () -> Void) {
        EffectWorkspaceFocus.finishNumberEntry(action)
    }
}

private extension PitchPointEditorView {
    func position(_ point: PitchEditPoint, in size: CGSize) -> CGPoint {
        CGPoint(
            x: 31 + CGFloat(point.frame - viewport.frames.lowerBound) / CGFloat(viewport.frames.count)
                * max(1, size.width - 35),
            y: 5 + CGFloat((viewport.upper - point.note) / (viewport.upper - viewport.lower)) * max(1, size.height - 24)
        )
    }
    func nearestPoint(_ location: CGPoint, size: CGSize) -> PitchEditPoint? {
        let frame = viewport.frame(Double(location.x - 31) / Double(max(1, size.width - 35)))
        let margin = max(1, Int(Double(viewport.frames.count) * 11 / Double(max(1, size.width - 35))))
        let start = PitchChartSampling.lowerBound(points, frame: frame - margin, time: { $0.frame })
        let end = PitchChartSampling.lowerBound(points, frame: frame + margin + 1, time: { $0.frame })
        let distances = points[start..<end].map { point -> (PitchEditPoint, CGFloat) in
            let value = position(point, in: size)
            return (point, hypot(value.x - location.x, value.y - location.y))
        }
        guard let nearest = distances.min(by: { $0.1 < $1.1 }), nearest.1 <= 10 else { return nil }
        return nearest.0
    }
    var chart: some View {
        Canvas { context, size in
            let plotRect = CGRect(x: 31, y: 0, width: max(1, size.width - 35), height: max(1, size.height - 18))
            let coordinate: (Int, Float) -> CGPoint = { frame, note in
                CGPoint(
                    x: 31 + CGFloat(frame - viewport.frames.lowerBound) / CGFloat(viewport.frames.count)
                        * max(1, size.width - 35),
                    y: 5 + CGFloat((viewport.upper - note) / (viewport.upper - viewport.lower))
                        * max(1, size.height - 24))
            }
            var plot = context
            plot.clip(to: Path(plotRect))
            let pitchStep: Float =
                viewport.upper - viewport.lower <= 2 ? 0.25 : max(1, ceil((viewport.upper - viewport.lower) / 8))
            for note in stride(
                from: ceil(viewport.lower / pitchStep) * pitchStep, through: viewport.upper, by: pitchStep)
            {
                let y =
                    5 + CGFloat((viewport.upper - note) / (viewport.upper - viewport.lower)) * max(1, size.height - 24)
                var grid = Path()
                grid.move(to: CGPoint(x: 31, y: y))
                grid.addLine(to: CGPoint(x: size.width - 4, y: y))
                context.stroke(
                    grid, with: .color(Theme.line.opacity(note.rounded() == note ? 0.7 : 0.35)), lineWidth: 1)
                context.draw(
                    Text(Self.noteName(note, cents: pitchStep < 1)).font(.system(size: pitchStep < 1 ? 7 : 8))
                        .foregroundColor(Theme.secondary), at: CGPoint(x: 1, y: y), anchor: .leading)
            }
            for (contour, color) in [(before, Theme.tracks[1]), (after ?? [], Theme.accent)] {
                var path = Path()
                var previous = false
                for point in PitchChartSampling.contour(contour, frames: viewport.frames, pixels: Int(plotRect.width)) {
                    guard point.isVoiced else {
                        previous = false
                        continue
                    }
                    let p = coordinate(point.frame, midi(point.frequency))
                    if previous { path.addLine(to: p) } else { path.move(to: p) }
                    previous = true
                }
                plot.stroke(path, with: .color(color.opacity(0.8)), lineWidth: 1.5)
            }
            if let range = editing.range {
                let start = coordinate(range.lowerBound, 69).x
                let end = coordinate(range.upperBound, 69).x
                plot.fill(
                    Path(CGRect(x: start, y: 0, width: end - start, height: plotRect.height)),
                    with: .color(Theme.accent.opacity(0.12)))
            }
            for region in preparedTargets
            where region.range.upperBound >= viewport.frames.lowerBound
                && region.range.lowerBound < viewport.frames.upperBound
            {
                var path = Path()
                var first = true
                for sample in region.samples(frames: viewport.frames, pixels: Int(plotRect.width)) {
                    let p = coordinate(sample.frame, sample.note)
                    if first {
                        path.move(to: p)
                        first = false
                    } else {
                        path.addLine(to: p)
                    }
                }
                plot.stroke(path, with: .color(Theme.tracks[2]), lineWidth: 1.7)
            }
            if let frame = editing.insertionFrame, let note = editing.insertionNote, viewport.frames.contains(frame) {
                let p = coordinate(frame, snap ? note.rounded() : note)
                if plotRect.contains(p) {
                    var guides = Path()
                    guides.move(to: CGPoint(x: p.x, y: 0))
                    guides.addLine(to: CGPoint(x: p.x, y: plotRect.maxY))
                    guides.move(to: CGPoint(x: plotRect.minX, y: p.y))
                    guides.addLine(to: CGPoint(x: plotRect.maxX, y: p.y))
                    plot.stroke(
                        guides, with: .color(Theme.secondary.opacity(0.6)),
                        style: StrokeStyle(lineWidth: 1, dash: [3, 3]))
                    plot.stroke(
                        Path(ellipseIn: CGRect(x: p.x - 5, y: p.y - 5, width: 10, height: 10)), with: .color(Theme.ink),
                        lineWidth: 1.5)
                }
            }
            var visiblePoints = PitchChartSampling.anchors(
                points, frames: viewport.frames, pixels: max(1, Int(plotRect.width / 5)))
            if let primary = selectedPoint, !visiblePoints.contains(where: { $0.id == primary.id }) {
                visiblePoints.append(primary)
            }
            for point in visiblePoints {
                let p = position(point, in: size)
                guard plotRect.insetBy(dx: -5, dy: -5).contains(p) else { continue }
                let selected = editing.selected.contains(point.id)
                let radius: CGFloat = selected ? 4.5 : 3
                plot.fill(
                    Path(ellipseIn: CGRect(x: p.x - radius, y: p.y - radius, width: radius * 2, height: radius * 2)),
                    with: .color(selected ? Theme.ink : Theme.tracks[2]))
                if selected {
                    plot.stroke(
                        Path(ellipseIn: CGRect(x: p.x - 6, y: p.y - 6, width: 12, height: 12)),
                        with: .color(Theme.tracks[2]), lineWidth: 1)
                }
            }
            for index in 0...4 {
                let fraction = Double(index) / 4
                let frame = viewport.frame(fraction)
                let x = 31 + CGFloat(fraction) * plotRect.width
                context.draw(
                    Text(String(format: "%.3f", Double(frame) / sessionRate)).font(.system(size: 8)).foregroundColor(
                        Theme.secondary),
                    at: CGPoint(x: x, y: size.height - 6),
                    anchor: index == 0 ? .leading : index == 4 ? .trailing : .center)
            }
        }.frame(height: expanded ? 360 : 180).background(Theme.background, in: RoundedRectangle(cornerRadius: 4))
            .overlay {
                GeometryReader { geometry in
                    Color.clear.contentShape(Rectangle())
                        .onContinuousHover { phase in
                            switch phase {
                            case .active(let location):
                                hoverFrame = viewport.frame(
                                    Double(location.x - 31) / Double(max(1, geometry.size.width - 35)))
                            case .ended: hoverFrame = nil
                            }
                        }
                        .gesture(
                            DragGesture(minimumDistance: 0).onChanged { value in
                                guard !busy else { return }
                                let size = geometry.size
                                if dragOriginal == nil {
                                    dragOriginal = regions
                                    dragStartFrame = viewport.frame(
                                        Double(value.startLocation.x - 31) / Double(max(1, size.width - 35)))
                                    if let point = nearestPoint(value.startLocation, size: size) {
                                        let shift = (NSApp.currentEvent?.modifierFlags ?? NSEvent.modifierFlags)
                                            .contains(.shift)
                                        editing.select(point.id, extending: shift)
                                        dragPoint = editing.selected.contains(point.id) ? point.id : nil
                                    } else {
                                        let note = viewport.note(
                                            Double(value.startLocation.y - 5) / Double(max(1, size.height - 24)))
                                        let plot = CGRect(
                                            x: 31, y: 5, width: max(1, size.width - 35),
                                            height: max(1, size.height - 24))
                                        if plot.contains(value.startLocation), let frame = dragStartFrame {
                                            editing.placeInsertion(frame: frame, note: note)
                                        }
                                    }
                                }
                                if dragPoint != nil, let original = dragOriginal {
                                    guard abs(value.translation.width) + abs(value.translation.height) >= 2 else {
                                        return
                                    }
                                    let frames = Int(
                                        Double(value.translation.width) / Double(max(1, size.width - 35))
                                            * Double(viewport.frames.count))
                                    let notes =
                                        -Float(value.translation.height / max(1, size.height - 24))
                                        * (viewport.upper - viewport.lower)
                                    let moved = editing.moving(original, frames: frames, notes: notes, snap: snap)
                                    if moved != (dragPreview ?? regions) {
                                        if !dragStarted {
                                            onBeginEdit("Move pitch points")
                                            dragStarted = true
                                        }
                                        dragPreview = moved
                                        prepareTargets(moved)
                                    }
                                } else if let start = dragStartFrame {
                                    let end = viewport.frame(
                                        Double(value.location.x - 31) / Double(max(1, size.width - 35)))
                                    if abs(value.translation.width) > 3 {
                                        editing.select(min(start, end)..<(max(start, end) + 1), regions: regions)
                                    }
                                }
                            }.onEnded { _ in
                                finishDrag()
                                focusChart()
                            })
                }
            }.accessibilityLabel(
                "Pitch target chart. Click a location, then Add point at that time and pitch. Drag target points in time and pitch; Shift selects several. Release to apply the move. All edits are also available in numeric controls."
            )
    }
}

enum PitchEditorKeyboardRouting {
    enum Action: Equatable { case space, undo, redo, consume }
    static func action(keyCode: UInt16, modifiers: NSEvent.ModifierFlags, repeated: Bool) -> Action? {
        let commandModifiers = modifiers.intersection([.command, .control, .option, .shift])
        if keyCode == 6 {
            if commandModifiers == .command { return .undo }
            if commandModifiers == [.command, .shift] { return .redo }
        }
        if keyCode == 49 && modifiers.intersection([.command, .control, .option]).isEmpty {
            return repeated ? .consume : .space
        }
        return nil
    }
    static func accepts(
        eventWindow: NSWindow?, editorWindow: NSWindow?, isKey: Bool,
        hasNestedSheet: Bool, responder: NSResponder?
    ) -> Bool {
        guard let editorWindow, eventWindow === editorWindow, isKey, !hasNestedSheet else { return false }
        return !(responder is NSTextView) && !(responder is NSTextField)
    }
    static func accepts(_ action: Action, responder: NSResponder?) -> Bool {
        if action == .space || action == .consume {
            return !(responder is NSPopUpButton) && !(responder is NSSlider)
        }
        return true
    }
}

private struct PitchEditorKeyboardBridge: NSViewRepresentable {
    var onSpace: () -> Void
    var onUndo: () -> Void
    var onRedo: () -> Void
    final class Coordinator {
        weak var view: NSView?
        var onSpace: () -> Void
        var onUndo: () -> Void
        var onRedo: () -> Void
        var monitor: Any?
        init(onSpace: @escaping () -> Void, onUndo: @escaping () -> Void, onRedo: @escaping () -> Void) {
            self.onSpace = onSpace
            self.onUndo = onUndo
            self.onRedo = onRedo
        }
        func install() {
            monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
                guard let self, let window = self.view?.window,
                    PitchEditorKeyboardRouting.accepts(
                        eventWindow: event.window, editorWindow: window,
                        isKey: window.isKeyWindow, hasNestedSheet: window.attachedSheet != nil,
                        responder: window.firstResponder),
                    let action = PitchEditorKeyboardRouting.action(
                        keyCode: event.keyCode, modifiers: event.modifierFlags, repeated: event.isARepeat),
                    PitchEditorKeyboardRouting.accepts(action, responder: window.firstResponder)
                else { return event }
                switch action {
                case .space: self.onSpace()
                case .undo: self.onUndo()
                case .redo: self.onRedo()
                case .consume: break
                }
                return nil
            }
        }
    }
    func makeCoordinator() -> Coordinator { Coordinator(onSpace: onSpace, onUndo: onUndo, onRedo: onRedo) }
    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        context.coordinator.view = view
        context.coordinator.install()
        return view
    }
    func updateNSView(_ view: NSView, context: Context) {
        context.coordinator.onSpace = onSpace
        context.coordinator.onUndo = onUndo
        context.coordinator.onRedo = onRedo
    }
    static func dismantleNSView(_ view: NSView, coordinator: Coordinator) {
        if let monitor = coordinator.monitor { NSEvent.removeMonitor(monitor) }
        coordinator.monitor = nil
    }
}
