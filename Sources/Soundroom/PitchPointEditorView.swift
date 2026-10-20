import AppKit
import SoundroomCore
import SwiftUI

/// The same source-timeline editor is embedded in selection and live-insert inspectors.
/// The owner handles rendering/persistence; this view never starts the main transport.
///
/// Pointer-rate state (hover, a point being dragged) lives in small reference
/// models that only the chart layers and the readout observe, so moving the
/// mouse never rebuilds the controls, and the measured contours redraw only
/// when the viewport or the analysis changes.
struct PitchPointEditorView: View, Equatable {
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
    @State private var initialized = false
    @State private var showsDetails = false
    /// The style that produced the current targets, until a manual edit.
    @State private var style: PitchCurvePreset?
    @State private var pointer = PitchChartPointer()
    @State private var drag = PitchChartDrag()
    @State private var targetCache = PitchChartTargetCache()

    /// The callbacks only reach the owner's stable controller and store, so the
    /// displayed values decide whether the editor needs to redraw.
    static func == (a: Self, b: Self) -> Bool {
        a.regions == b.regions && a.before == b.before && a.after == b.after && a.scope == b.scope
            && a.reference == b.reference && a.busy == b.busy && a.isPreviewing == b.isPreviewing
            && a.canBake == b.canBake && a.title == b.title && a.context == b.context && a.bakeTitle == b.bakeTitle
            && a.error == b.error && a.expanded == b.expanded && a.canUndo == b.canUndo && a.canRedo == b.canRedo
    }

    private var selectedPoint: PitchEditPoint? { editing.selectedPoint(in: regions) }
    private var safeReference: Float { reference.isFinite && reference > 0 ? reference : 440 }
    private var hasTargets: Bool { !regions.isEmpty }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            header
            if !before.isEmpty || hasTargets {
                styles
                PitchChart(
                    editing: $editing, viewport: $viewport, drag: drag, pointer: pointer, cache: targetCache,
                    regions: regions, before: before, after: after, reference: safeReference, snap: snap,
                    busy: busy, height: expanded ? 360 : 180,
                    onBeginDrag: { onBeginEdit("Move pitch points") },
                    onFinishDrag: {
                        finishDrag()
                        focusChart()
                    },
                    onAddPoint: addPoint(frame:note:))
                chartFooter
                selectionTools
                if showsDetails { detailTools }
                actions
            } else {
                emptyState
            }
            if let error { Text(error).foregroundStyle(Theme.secondary).fixedSize(horizontal: false, vertical: true) }
        }.font(.system(size: 10)).padding(expanded ? 16 : 10)
            .background(Theme.raised, in: RoundedRectangle(cornerRadius: 7))
            .background {
                if expanded {
                    PitchEditorKeyboardBridge(
                        onAction: { action in
                            guard !busy else { return }
                            switch action {
                            case .space: if isPreviewing { onStop() } else if hasTargets { onAudition() }
                            case .undo: if canUndo { onUndo() }
                            case .redo: if canRedo { onRedo() }
                            case .delete: change("Delete pitch points", editing.deleting(regions))
                            case .nudge(let frames, let cents): nudge(frames: frames, notes: Float(cents) / 100)
                            case .selectAll: editing.selectAll(regions)
                            case .deselect: editing = PitchPointEditing()
                            case .consume: break
                            }
                        })
                }
            }
            .onAppear {
                if !before.isEmpty || hasTargets { fit() }
            }
            .onChange(of: scope) { _, _ in
                finishDrag(commit: false)
                editing = PitchPointEditing()
                style = nil
                fit()
            }
            .onChange(of: before) { _, _ in if !initialized, !before.isEmpty { fit() } }
            .onChange(of: regions) { _, value in
                if let original = drag.original, value != original { finishDrag(commit: false) }
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
                            "Tunes one voice or instrument (50–1000 Hz) by up to an octave. Targets follow the track timeline and play live until you apply them to the audio."
                        )
                }
                Spacer()
                ZStack {
                    if busy { ProgressView().controlSize(.mini).accessibilityLabel("Preparing pitch audio") }
                }.frame(width: 14, height: 14).accessibilityHidden(!busy)
                Button(before.isEmpty ? "Analyze" : "Re-analyze", action: onAnalyze)
                    .buttonStyle(.borderless).frame(width: 82).disabled(busy || scope.isEmpty)
                    .help("Measure the sung pitch of the selection. Never changes the audio.")
                if !expanded {
                    Button(action: onExpand) { Image(systemName: "arrow.up.left.and.arrow.down.right") }
                        .buttonStyle(.borderless).help("Open the large pitch editor").accessibilityLabel(
                            "Expand pitch editor")
                }
            }
            Text(context).font(.system(size: 9)).foregroundStyle(Theme.secondary)
                .lineLimit(expanded ? 1 : 2).frame(maxWidth: .infinity, alignment: .leading)
                .frame(height: expanded ? 12 : 24, alignment: .topLeading).help(context)
        }
    }
    private var emptyState: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Tune a vocal in three steps").fontWeight(.medium)
            Label("Select a vocal clip, or drag across part of its track", systemImage: "1.circle")
            Label("Click Analyze to see the sung pitch", systemImage: "2.circle")
            Label("Pick a style, then drag any note to fine-tune it", systemImage: "3.circle")
        }.foregroundStyle(Theme.secondary).fixedSize(horizontal: false, vertical: true)
    }
    /// One click retunes the whole selection; each style is one Undo step.
    private var styles: some View {
        HStack(spacing: 4) {
            ForEach(PitchCurvePreset.allCases) { preset in
                Button {
                    style = preset
                    onResetTargets(preset)
                } label: {
                    Text(preset.title).frame(maxWidth: .infinity, minHeight: 20)
                        .background(
                            style == preset ? Theme.accent.opacity(0.85) : Theme.background,
                            in: RoundedRectangle(cornerRadius: 5)
                        )
                        .foregroundStyle(style == preset ? Theme.background : Theme.ink)
                        .contentShape(Rectangle())
                }.buttonStyle(.plain).help(preset.help).accessibilityLabel("\(preset.title) tuning style")
                    .accessibilityAddTraits(style == preset ? .isSelected : [])
            }
        }.disabled(busy || before.isEmpty)
    }
    private var chartFooter: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 8) {
                Text("Sung").foregroundStyle(Theme.tracks[1])
                Text("Target").foregroundStyle(Theme.tracks[2])
                Text(after == nil ? "Result" : "Result · measured").foregroundStyle(
                    after == nil ? Theme.secondary : Theme.accent
                ).help("Measure result (in More) checks the corrected pitch without playing")
                Spacer(minLength: 0)
                icon("minus.magnifyingglass", "Zoom out (pinch or ⌘-scroll on the chart)") {
                    viewport.zoomTime(0.5, around: pointer.hoverFrame)
                }
                icon("plus.magnifyingglass", "Zoom in (pinch or ⌘-scroll on the chart)") {
                    viewport.zoomTime(2, around: selectedPoint?.frame ?? pointer.hoverFrame)
                }
                Button("Fit") { fit() }.buttonStyle(.borderless).help("Show the whole selection")
                Menu {
                    Button("Zoom to selected points") { viewport.fitSelected(selectedPoints) }
                        .disabled(editing.selected.isEmpty)
                    Divider()
                    Button("Earlier") { viewport.panTime(-1) }
                    Button("Later") { viewport.panTime(1) }
                    Divider()
                    Button("Zoom in pitch") { viewport.zoomPitch(2, around: pointer.hoverNote) }
                    Button("Zoom out pitch") { viewport.zoomPitch(0.5, around: pointer.hoverNote) }
                    Button("Higher notes") { viewport.panPitch(2) }
                    Button("Lower notes") { viewport.panPitch(-2) }
                } label: {
                    Image(systemName: "ellipsis.circle")
                }.menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
                    .help("More view controls. Scroll sideways to move in time; ⌥-scroll moves the pitch view.")
                    .accessibilityLabel("View controls")
            }.disabled(busy)
            PitchHoverReadout(
                pointer: pointer, before: before, after: after, regions: regions, reference: safeReference,
                expanded: expanded)
        }
    }
    private var selectionTools: some View {
        HStack(spacing: 6) {
            Text(editing.selected.isEmpty ? "No points selected" : "\(editing.selected.count) selected")
                .foregroundStyle(Theme.secondary).lineLimit(1)
            Spacer(minLength: 0)
            Toggle(isOn: $snap) { Image(systemName: "dot.squareshape.split.2x2") }.toggleStyle(.button)
                .help("Snap dragged notes to the nearest semitone").accessibilityLabel("Snap to notes")
            icon("trash", "Delete selected points (Delete key in the large editor)") {
                change("Delete pitch points", editing.deleting(regions))
            }.disabled(editing.selected.isEmpty)
            Toggle("Fine-tune", isOn: $showsDetails).toggleStyle(.button)
                .help("Exact time, note and cents, curve shape and smoothing for selected points")
        }.controlSize(.small).disabled(busy)
    }
    private var detailTools: some View {
        VStack(spacing: 5) {
            HStack(spacing: 4) {
                Text("Time").foregroundStyle(Theme.secondary).frame(width: 30, alignment: .leading)
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
                icon("chevron.left", "Earlier by one millisecond") { nudge(frames: -48, notes: 0) }
                icon("chevron.right", "Later by one millisecond") { nudge(frames: 48, notes: 0) }
            }
            HStack(spacing: 4) {
                Text("Note").foregroundStyle(Theme.secondary).frame(width: 30, alignment: .leading)
                icon("minus", "Down one semitone") { nudge(frames: 0, notes: -1) }
                Text(selectedPoint.map { Self.noteName($0.note) } ?? "—").monospacedDigit().frame(width: 34)
                    .accessibilityLabel("Selected note")
                icon("plus", "Up one semitone") { nudge(frames: 0, notes: 1) }
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
            }
            HStack(spacing: 5) {
                Menu("Curve") {
                    ForEach(AutomationCurve.allCases) { curve in
                        Button(curve.name) {
                            change("\(curve.name) pitch curve", editing.settingCurve(curve, in: regions))
                        }
                    }
                }.fixedSize().help("How the target moves from each selected point to the next")
                Text("Smooth").foregroundStyle(Theme.secondary)
                RoomSlider(value: $smoothing, in: 0...1, label: "Local pitch smoothing strength")
                Button("Apply") { change("Smooth pitch points", editing.smoothing(regions, amount: Float(smoothing))) }
                    .help("Smooth selected interior points, keeping region ends and timing")
            }
        }.textFieldStyle(.roundedBorder).controlSize(.small).disabled(busy || selectedPoint == nil)
    }
    private var actions: some View {
        HStack(spacing: 5) {
            Button {
                finishNumberEntry(isPreviewing ? onStop : onAudition)
            } label: {
                Label(isPreviewing ? "Stop" : "Listen", systemImage: isPreviewing ? "stop.fill" : "play.fill")
                    .frame(width: 58)
            }.help("Listen to the tuned selection (Space in the large editor)")
            Button("Original") { finishNumberEntry(onAuditionOriginal) }.help(
                "Listen to the selection without this pitch correction")
            Menu {
                Button("Measure result") { finishNumberEntry(onRecheck) }.disabled(!hasTargets)
                    .help("Measure the corrected pitch without playing; shown as Result")
            } label: {
                Text("More")
            }.menuStyle(.borderlessButton).fixedSize()
            Spacer(minLength: 0)
            Button(bakeTitle) { finishNumberEntry(onBake) }.disabled(!canBake).help(
                context + ". Undo and clip history keep the original audio.")
        }.controlSize(.small).disabled(busy || before.isEmpty)
    }
    private var selectedPoints: [PitchEditPoint] {
        regions.flatMap(\.points).filter { editing.selected.contains($0.id) }
    }
    private func icon(_ name: String, _ label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) { Image(systemName: name).frame(width: 12) }.buttonStyle(.borderless)
            .help(label).accessibilityLabel(label)
    }
    private func fit() {
        let reference = safeReference
        let midi = { (frequency: Float) in 69 + 12 * log2(frequency / reference) }
        viewport.fit(
            scope: scope,
            notes: before.filter(\.isVoiced).map { midi($0.frequency) }
                + (after ?? []).filter(\.isVoiced).map { midi($0.frequency) } + regions.flatMap(\.points).map(\.note))
        initialized = true
    }
    private func change(_ name: String, _ result: [PitchEditRegion]) {
        guard !busy, result != regions else { return }
        onBeginEdit(name)
        regions = result
        onEndEdit()
        editing.reconcile(result)
        style = nil
    }
    private func nudge(frames: Int, notes: Float) {
        change("Move pitch points", editing.moving(regions, frames: frames, notes: notes))
    }
    private func addPoint(frame: Int, note: Float) {
        let added = editing.adding(to: regions, frame: frame, note: snap ? note.rounded() : note)
        change("Add pitch point", added.regions)
        if let id = added.point { editing.select(id, extending: false) }
    }
    nonisolated static func noteName(_ note: Float, cents: Bool = false) -> String {
        guard note.isFinite else { return "—" }
        let n = Int(note.rounded())
        let names = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"]
        let name = "\(names[(n % 12 + 12) % 12])\(Int(floor(Double(n) / 12)) - 1)"
        return cents ? name + String(format: " %+.1f¢", (note - Float(n)) * 100) : name
    }
    private func finishDrag(commit: Bool = true) {
        let pending = drag.preview
        let started = drag.started
        drag.reset()
        // Draw the private gesture locally; author its final value once on release.
        if commit, started, let pending, pending != regions {
            regions = pending
            style = nil
        }
        if started { onEndEdit() }
    }
    private func focusChart() {
        guard expanded, let window = NSApp.keyWindow else { return }
        window.makeFirstResponder(nil)
    }
    private func finishNumberEntry(_ action: @escaping () -> Void) {
        EffectWorkspaceFocus.finishNumberEntry(action)
    }
}

extension PitchCurvePreset {
    var title: String { self == .performance ? "As sung" : rawValue }
    var help: String {
        switch self {
        case .natural: return "Moves each note halfway to the nearest allowed pitch and keeps your vibrato"
        case .tight: return "Puts each note on pitch with smooth transitions"
        case .robotic: return "Holds exact notes with instant jumps, the classic hard-tune effect"
        case .performance: return "Targets follow exactly what was sung; a starting point for hand edits"
        }
    }
}

/// Hover position over the chart. Only the readout, the zoom anchor and the
/// scroll bridge read it.
final class PitchChartPointer: ObservableObject {
    @Published var hoverFrame: Int?
    @Published var hoverNote: Float?
}

/// A point drag in progress: the snapshot it started from and the preview drawn
/// while the pointer moves. Only the target layer observes the preview.
final class PitchChartDrag: ObservableObject {
    @Published var preview: [PitchEditRegion]?
    var original: [PitchEditRegion]?
    var point: UUID?
    var startFrame: Int?
    var started = false
    func reset() {
        if preview != nil { preview = nil }
        original = nil
        point = nil
        startFrame = nil
        started = false
    }
}

/// Prepared curves keyed by region; a drag only re-prepares the regions it moves.
final class PitchChartTargetCache {
    private var entries: [UUID: (region: PitchEditRegion, prepared: PreparedPitchChartRegion)] = [:]
    func prepared(_ regions: [PitchEditRegion]) -> [PreparedPitchChartRegion] {
        var next: [UUID: (region: PitchEditRegion, prepared: PreparedPitchChartRegion)] = [:]
        let result = regions.map { region in
            let prepared =
                entries[region.id].flatMap { $0.region == region ? $0.prepared : nil }
                ?? PreparedPitchChartRegion(region)
            next[region.id] = (region, prepared)
            return prepared
        }
        entries = next
        return result
    }
}

/// Chart coordinates shared by every layer and the gestures.
struct PitchChartGeometry {
    let size: CGSize
    let viewport: PitchPointViewport
    static let left: CGFloat = 31
    var plot: CGRect {
        CGRect(x: Self.left, y: 0, width: max(1, size.width - 35), height: max(1, size.height - 18))
    }
    private var width: CGFloat { max(1, size.width - 35) }
    private var height: CGFloat { max(1, size.height - 24) }
    func x(_ frame: Int) -> CGFloat {
        Self.left + CGFloat(frame - viewport.frames.lowerBound) / CGFloat(viewport.frames.count) * width
    }
    func y(_ note: Float) -> CGFloat {
        5 + CGFloat((viewport.upper - note) / (viewport.upper - viewport.lower)) * height
    }
    func point(_ frame: Int, _ note: Float) -> CGPoint { CGPoint(x: x(frame), y: y(note)) }
    func frame(at x: CGFloat) -> Int { viewport.frame(Double(x - Self.left) / Double(width)) }
    func note(at y: CGFloat) -> Float { viewport.note(Double(y - 5) / Double(height)) }
    func framesPerPoint() -> Double { Double(viewport.frames.count) / Double(width) }
}

private struct PitchChart: View {
    @Binding var editing: PitchPointEditing
    @Binding var viewport: PitchPointViewport
    @ObservedObject var drag: PitchChartDrag
    let pointer: PitchChartPointer
    let cache: PitchChartTargetCache
    let regions: [PitchEditRegion]
    let before: [PitchContourPoint]
    let after: [PitchContourPoint]?
    let reference: Float
    let snap: Bool
    let busy: Bool
    let height: CGFloat
    let onBeginDrag: () -> Void
    let onFinishDrag: () -> Void
    let onAddPoint: (Int, Float) -> Void

    var body: some View {
        let displayed = drag.preview ?? regions
        let points = Self.sortedPoints(displayed)
        ZStack {
            PitchChartBackdrop(before: before, after: after, viewport: viewport, reference: reference).equatable()
            PitchChartTargets(
                prepared: cache.prepared(displayed), points: points, editing: editing, viewport: viewport, snap: snap)
        }
        .frame(height: height).background(Theme.background, in: RoundedRectangle(cornerRadius: 4))
        .overlay {
            GeometryReader { geometry in
                Color.clear.contentShape(Rectangle())
                    .onContinuousHover { phase in
                        let chart = PitchChartGeometry(size: geometry.size, viewport: viewport)
                        switch phase {
                        case .active(let location):
                            pointer.hoverFrame = chart.frame(at: location.x)
                            pointer.hoverNote = chart.note(at: location.y)
                        case .ended:
                            pointer.hoverFrame = nil
                            pointer.hoverNote = nil
                        }
                    }
                    .gesture(dragGesture(size: geometry.size, points: points))
                    .background(
                        PitchChartScrollBridge { event in scroll(event, size: geometry.size) }
                    )
            }
        }
        .contextMenu {
            Button("Select all points") { editing.selectAll(regions) }
            Button("Select none") { editing = PitchPointEditing() }.disabled(editing.selected.isEmpty)
        }
        .accessibilityLabel(
            "Pitch chart. Drag a target point to move it in time and pitch; Shift adds to the selection. Drag empty space to select a time range. Double-click to add a point. Fine-tune offers exact values."
        )
    }
    static func sortedPoints(_ regions: [PitchEditRegion]) -> [PitchEditPoint] {
        let values = regions.flatMap(\.points)
        return zip(values, values.dropFirst()).allSatisfy { $0.frame <= $1.frame }
            ? values : values.sorted { $0.frame < $1.frame }
    }
    private func nearestPoint(_ location: CGPoint, chart: PitchChartGeometry, points: [PitchEditPoint])
        -> PitchEditPoint?
    {
        let frame = chart.frame(at: location.x)
        let margin = max(1, Int(chart.framesPerPoint() * 11))
        let start = PitchChartSampling.lowerBound(points, frame: frame - margin, time: { $0.frame })
        let end = PitchChartSampling.lowerBound(points, frame: frame + margin + 1, time: { $0.frame })
        guard start < end else { return nil }
        let nearest = points[start..<end].map { point in
            let value = chart.point(point.frame, point.note)
            return (point, hypot(value.x - location.x, value.y - location.y))
        }.min { $0.1 < $1.1 }
        guard let nearest, nearest.1 <= 10 else { return nil }
        return nearest.0
    }
    private func dragGesture(size: CGSize, points: [PitchEditPoint]) -> some Gesture {
        DragGesture(minimumDistance: 0).onChanged { value in
            guard !busy else { return }
            let chart = PitchChartGeometry(size: size, viewport: viewport)
            if drag.original == nil {
                drag.original = regions
                drag.startFrame = chart.frame(at: value.startLocation.x)
                let event = NSApp.currentEvent
                if let point = nearestPoint(value.startLocation, chart: chart, points: points) {
                    let shift = (event?.modifierFlags ?? NSEvent.modifierFlags).contains(.shift)
                    editing.select(point.id, extending: shift)
                    drag.point = editing.selected.contains(point.id) ? point.id : nil
                } else if chart.plot.contains(value.startLocation), let frame = drag.startFrame {
                    let note = chart.note(at: value.startLocation.y)
                    if (event?.clickCount ?? 1) >= 2 {
                        onAddPoint(frame, note)
                        drag.original = nil
                        return
                    }
                    editing.placeInsertion(frame: frame, note: note)
                }
            }
            if drag.point != nil, let original = drag.original {
                guard abs(value.translation.width) + abs(value.translation.height) >= 2 else { return }
                let frames = Int(Double(value.translation.width) * chart.framesPerPoint())
                let notes =
                    -Float(value.translation.height / max(1, size.height - 24)) * (viewport.upper - viewport.lower)
                let moved = editing.moving(original, frames: frames, notes: notes, snap: snap)
                if moved != (drag.preview ?? regions) {
                    if !drag.started {
                        onBeginDrag()
                        drag.started = true
                    }
                    drag.preview = moved
                }
            } else if let start = drag.startFrame, abs(value.translation.width) > 3 {
                let end = chart.frame(at: value.location.x)
                editing.select(min(start, end)..<(max(start, end) + 1), regions: regions)
            }
        }.onEnded { _ in onFinishDrag() }
    }
    /// Pinch or ⌘-scroll zooms time around the pointer, sideways scrolling pans
    /// time and ⌥-scroll pans pitch. Plain vertical scrolling still scrolls the
    /// surrounding inspector.
    private func scroll(_ event: NSEvent, size: CGSize) -> Bool {
        guard !busy else { return false }
        let anchor = pointer.hoverFrame
        if event.type == .magnify {
            viewport.zoomTime(1 + Double(event.magnification), around: anchor)
            return true
        }
        let dx = Double(event.scrollingDeltaX)
        let dy = Double(event.scrollingDeltaY)
        let scale = event.hasPreciseScrollingDeltas ? 1.0 : 12.0
        if event.modifierFlags.contains(.command), dy != 0 {
            viewport.zoomTime(pow(1.01, dy * scale / 2), around: anchor)
            return true
        }
        if event.modifierFlags.contains(.option), dy != 0 {
            viewport.panPitch(Float(dy * scale) / 40)
            return true
        }
        let sideways = event.modifierFlags.contains(.shift) ? dy : dx
        guard abs(sideways) > abs(event.modifierFlags.contains(.shift) ? 0 : dy) else { return false }
        viewport.panTime(fraction: -sideways * scale / Double(max(1, size.width - 35)))
        return true
    }
}

/// Grid, labels and measured contours. Equatable, so a drag or hover leaves it
/// alone; it redraws only for a new viewport or analysis.
private struct PitchChartBackdrop: View, Equatable {
    let before: [PitchContourPoint]
    let after: [PitchContourPoint]?
    let viewport: PitchPointViewport
    let reference: Float
    var body: some View {
        Canvas { context, size in
            let chart = PitchChartGeometry(size: size, viewport: viewport)
            let plotRect = chart.plot
            var plot = context
            plot.clip(to: Path(plotRect))
            let span = viewport.upper - viewport.lower
            let pitchStep: Float = span <= 2 ? 0.25 : max(1, ceil(span / 8))
            for note in stride(
                from: ceil(viewport.lower / pitchStep) * pitchStep, through: viewport.upper, by: pitchStep)
            {
                let y = chart.y(note)
                var grid = Path()
                grid.move(to: CGPoint(x: PitchChartGeometry.left, y: y))
                grid.addLine(to: CGPoint(x: size.width - 4, y: y))
                context.stroke(
                    grid, with: .color(Theme.line.opacity(note.rounded() == note ? 0.7 : 0.35)), lineWidth: 1)
                context.draw(
                    Text(PitchPointEditorView.noteName(note, cents: pitchStep < 1)).font(
                        .system(size: pitchStep < 1 ? 7 : 8)
                    ).foregroundColor(Theme.secondary), at: CGPoint(x: 1, y: y), anchor: .leading)
            }
            for (contour, color) in [(before, Theme.tracks[1]), (after ?? [], Theme.accent)] {
                var path = Path()
                var previous = false
                for point in PitchChartSampling.contour(contour, frames: viewport.frames, pixels: Int(plotRect.width)) {
                    guard point.isVoiced else {
                        previous = false
                        continue
                    }
                    let p = chart.point(point.frame, 69 + 12 * log2(point.frequency / reference))
                    if previous { path.addLine(to: p) } else { path.move(to: p) }
                    previous = true
                }
                plot.stroke(path, with: .color(color.opacity(0.8)), lineWidth: 1.5)
            }
            for index in 0...4 {
                let fraction = Double(index) / 4
                let frame = viewport.frame(fraction)
                context.draw(
                    Text(String(format: "%.3f", Double(frame) / sessionRate)).font(.system(size: 8)).foregroundColor(
                        Theme.secondary),
                    at: CGPoint(x: PitchChartGeometry.left + CGFloat(fraction) * plotRect.width, y: size.height - 6),
                    anchor: index == 0 ? .leading : index == 4 ? .trailing : .center)
            }
        }
    }
}

/// Target curves, anchors, the time selection and the insertion guide.
private struct PitchChartTargets: View {
    let prepared: [PreparedPitchChartRegion]
    let points: [PitchEditPoint]
    let editing: PitchPointEditing
    let viewport: PitchPointViewport
    let snap: Bool
    var body: some View {
        Canvas { context, size in
            let chart = PitchChartGeometry(size: size, viewport: viewport)
            let plotRect = chart.plot
            var plot = context
            plot.clip(to: Path(plotRect))
            if let range = editing.range {
                let start = chart.x(range.lowerBound)
                plot.fill(
                    Path(CGRect(x: start, y: 0, width: chart.x(range.upperBound) - start, height: plotRect.height)),
                    with: .color(Theme.accent.opacity(0.12)))
            }
            for region in prepared
            where region.range.upperBound >= viewport.frames.lowerBound
                && region.range.lowerBound < viewport.frames.upperBound
            {
                var path = Path()
                for (index, sample) in region.samples(frames: viewport.frames, pixels: Int(plotRect.width)).enumerated()
                {
                    let p = chart.point(sample.frame, sample.note)
                    if index == 0 { path.move(to: p) } else { path.addLine(to: p) }
                }
                plot.stroke(path, with: .color(Theme.tracks[2]), lineWidth: 1.7)
            }
            if let frame = editing.insertionFrame, let note = editing.insertionNote, viewport.frames.contains(frame) {
                let p = chart.point(frame, snap ? note.rounded() : note)
                if plotRect.contains(p) {
                    var guides = Path()
                    guides.move(to: CGPoint(x: p.x, y: 0))
                    guides.addLine(to: CGPoint(x: p.x, y: plotRect.maxY))
                    guides.move(to: CGPoint(x: plotRect.minX, y: p.y))
                    guides.addLine(to: CGPoint(x: plotRect.maxX, y: p.y))
                    plot.stroke(
                        guides, with: .color(Theme.secondary.opacity(0.6)),
                        style: StrokeStyle(lineWidth: 1, dash: [3, 3]))
                }
            }
            var visible = PitchChartSampling.anchors(
                points, frames: viewport.frames, pixels: max(1, Int(plotRect.width / 5)))
            if let primary = editing.primary, !visible.contains(where: { $0.id == primary }),
                let point = points.first(where: { $0.id == primary })
            {
                visible.append(point)
            }
            for point in visible {
                let p = chart.point(point.frame, point.note)
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
        }.allowsHitTesting(false)
    }
}

/// The pitch under the pointer. Binary search keeps it cheap at pointer rate.
private struct PitchHoverReadout: View {
    @ObservedObject var pointer: PitchChartPointer
    let before: [PitchContourPoint]
    let after: [PitchContourPoint]?
    let regions: [PitchEditRegion]
    let reference: Float
    let expanded: Bool
    var body: some View {
        let text =
            pointer.hoverFrame.map(reading)
            ?? "Drag a point to retune · Shift-click selects several · drag empty space to select · double-click adds a point"
        Text(text).font(.system(size: 9)).foregroundStyle(Theme.secondary).monospacedDigit()
            .lineLimit(expanded ? 1 : 2).frame(maxWidth: .infinity, alignment: .leading)
            .frame(height: expanded ? 12 : 24, alignment: .topLeading).accessibilityLabel(text)
    }
    private func reading(at frame: Int) -> String {
        func nearest(_ contour: [PitchContourPoint]) -> String {
            let index = PitchChartSampling.lowerBound(contour, frame: frame, time: { $0.frame })
            let candidates = [index - 1, index].filter { contour.indices.contains($0) }.map { contour[$0] }
            guard let point = candidates.min(by: { abs($0.frame - frame) < abs($1.frame - frame) }), point.isVoiced
            else { return "unvoiced" }
            return PitchPointEditorView.noteName(69 + 12 * log2(point.frequency / reference), cents: true)
        }
        let target =
            regions.first { $0.range.contains(frame) }?.note(at: frame).map {
                PitchPointEditorView.noteName($0, cents: true)
            } ?? "none"
        return String(
            format: "%.3f s · sung %@ · target %@ · result %@", Double(frame) / sessionRate, nearest(before), target,
            after.map(nearest) ?? "not measured")
    }
}

/// Scroll and pinch over the chart, delivered while the pointer is inside it.
private struct PitchChartScrollBridge: NSViewRepresentable {
    var onEvent: (NSEvent) -> Bool
    final class Coordinator {
        weak var view: NSView?
        var onEvent: (NSEvent) -> Bool
        var monitor: Any?
        init(onEvent: @escaping (NSEvent) -> Bool) { self.onEvent = onEvent }
        func install() {
            monitor = NSEvent.addLocalMonitorForEvents(matching: [.scrollWheel, .magnify]) { [weak self] event in
                guard let self, let view = self.view, let window = view.window, event.window === window,
                    window.attachedSheet == nil,
                    view.bounds.contains(view.convert(event.locationInWindow, from: nil))
                else { return event }
                return self.onEvent(event) ? nil : event
            }
        }
    }
    func makeCoordinator() -> Coordinator { Coordinator(onEvent: onEvent) }
    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        context.coordinator.view = view
        context.coordinator.install()
        return view
    }
    func updateNSView(_ view: NSView, context: Context) { context.coordinator.onEvent = onEvent }
    static func dismantleNSView(_ view: NSView, coordinator: Coordinator) {
        if let monitor = coordinator.monitor { NSEvent.removeMonitor(monitor) }
        coordinator.monitor = nil
    }
}

enum PitchEditorKeyboardRouting {
    enum Action: Equatable {
        case space, undo, redo, consume, delete, selectAll, deselect
        /// Selected points: frames in time, cents in pitch.
        case nudge(frames: Int, cents: Int)
    }
    static func action(keyCode: UInt16, modifiers: NSEvent.ModifierFlags, repeated: Bool) -> Action? {
        let commandModifiers = modifiers.intersection([.command, .control, .option, .shift])
        let fine = commandModifiers == .option
        let plain = commandModifiers.isEmpty
        switch keyCode {
        case 6:
            if commandModifiers == .command { return .undo }
            if commandModifiers == [.command, .shift] { return .redo }
        case 0 where commandModifiers == .command: return .selectAll
        case 49 where modifiers.intersection([.command, .control, .option]).isEmpty:
            return repeated ? .consume : .space
        case 51 where plain, 117 where plain: return .delete
        case 53 where plain: return .deselect
        case 123 where plain || fine: return .nudge(frames: fine ? -480 : -48, cents: 0)
        case 124 where plain || fine: return .nudge(frames: fine ? 480 : 48, cents: 0)
        case 125 where plain || fine: return .nudge(frames: 0, cents: fine ? -10 : -100)
        case 126 where plain || fine: return .nudge(frames: 0, cents: fine ? 10 : 100)
        default: break
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
    /// Controls keep their own Space, arrow and Delete behaviour.
    static func accepts(_ action: Action, responder: NSResponder?) -> Bool {
        switch action {
        case .undo, .redo, .selectAll: return true
        default: return !(responder is NSPopUpButton) && !(responder is NSSlider)
        }
    }
}

private struct PitchEditorKeyboardBridge: NSViewRepresentable {
    var onAction: (PitchEditorKeyboardRouting.Action) -> Void
    final class Coordinator {
        weak var view: NSView?
        var onAction: (PitchEditorKeyboardRouting.Action) -> Void
        var monitor: Any?
        init(onAction: @escaping (PitchEditorKeyboardRouting.Action) -> Void) { self.onAction = onAction }
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
                self.onAction(action)
                return nil
            }
        }
    }
    func makeCoordinator() -> Coordinator { Coordinator(onAction: onAction) }
    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        context.coordinator.view = view
        context.coordinator.install()
        return view
    }
    func updateNSView(_ view: NSView, context: Context) { context.coordinator.onAction = onAction }
    static func dismantleNSView(_ view: NSView, coordinator: Coordinator) {
        if let monitor = coordinator.monitor { NSEvent.removeMonitor(monitor) }
        coordinator.monitor = nil
    }
}
