import SwiftUI
import AppKit
import SoundroomCore

/// Tempo belongs beside transport regardless of the chosen ruler units.
struct ProjectTempoControls: View {
    @ObservedObject var store: SessionStore
    var body: some View {
        HStack(spacing: 7) {
            VStack(spacing: 0) {
                Text("BPM").font(.system(size: 8, weight: .semibold)).foregroundStyle(Theme.secondary)
                CommittedNumberField("BPM", value: Binding(get: { store.session.timing.tempo }, set: { store.setMusicalTiming(tempo: $0) }), range: 20...300, fractionDigits: 1, onSubmit: { store.focusEditor() })
                    .frame(width: 43).accessibilityLabel("Project tempo BPM")
                    .help("Project tempo for the beat grid")
            }
            Menu(store.session.timing.meter) {
                    ForEach(["2/4", "3/4", "4/4", "5/4", "6/8", "7/8", "9/8", "12/8"], id: \.self) { meter in
                        Button(meter) { let parts = meter.split(separator: "/").compactMap { Int($0) }; store.setMusicalTiming(beatsPerBar: parts[0], beatUnit: parts[1]) }
                    }
            }.fixedSize().font(.system(size: 10)).accessibilityLabel("Project time signature").accessibilityValue(store.session.timing.meter)
        }.textFieldStyle(.plain).font(.system(size: 13, weight: .medium, design: .monospaced))
            .padding(.horizontal, 7).padding(.vertical, 4).background(Theme.well, in: RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6).stroke(Theme.line))
            .disabled(store.busy || store.isRecording)
    }
}

struct LoopRangeControls: View {
    @ObservedObject var store: SessionStore
    @ObservedObject private var settings: TimelineEditingState
    init(store: SessionStore) { self.store = store; self.settings = store.timelineEditing }
    var body: some View {
        HStack(spacing: 10) {
            Label("Loop", systemImage: "repeat").foregroundStyle(Theme.loop)
            if let range = store.loopRange {
                if settings.timebase == .beats {
                    TimelineBeatField(title: "Loop start", frame: range.lowerBound, timing: store.session.timing) { frame in
                        guard store.loopRange == range else { return }
                        store.setLoopEdge(start: true, seconds: Double(frame) / sessionRate)
                        store.focusEditor()
                    }.id("loop.beats.start.\(range.lowerBound).\(range.upperBound)").frame(width: 90)
                } else {
                    CommittedNumberField("Loop start seconds", value: Binding(get: { Double(range.lowerBound) / sessionRate }, set: { value in
                        guard store.loopRange == range else { return }
                        store.setLoopEdge(start: true, seconds: value)
                    }), range: 0...(Double(Int.max / 4) / sessionRate), fractionDigits: 4, onSubmit: { store.focusEditor() })
                        .id("loop.\(range.lowerBound).\(range.upperBound).start").frame(width: 90).accessibilityLabel("Loop start in seconds")
                }
                Text("to").foregroundStyle(Theme.secondary)
                if settings.timebase == .beats {
                    TimelineBeatField(title: "Loop end", frame: range.upperBound, timing: store.session.timing) { frame in
                        guard store.loopRange == range else { return }
                        store.setLoopEdge(start: false, seconds: Double(frame) / sessionRate)
                        store.focusEditor()
                    }.id("loop.beats.end.\(range.lowerBound).\(range.upperBound)").frame(width: 90)
                } else {
                    CommittedNumberField("Loop end seconds", value: Binding(get: { Double(range.upperBound) / sessionRate }, set: { value in
                        guard store.loopRange == range else { return }
                        store.setLoopEdge(start: false, seconds: value)
                    }), range: 0...(Double(Int.max / 4) / sessionRate), fractionDigits: 4, onSubmit: { store.focusEditor() })
                        .id("loop.\(range.lowerBound).\(range.upperBound).end").frame(width: 90).accessibilityLabel("Loop end in seconds")
                }
            }
            Text(settings.timebase == .beats ? "bars.beats" : "seconds").foregroundStyle(Theme.secondary)
            Button("Use selection") { store.useSelectionForLoop(); store.focusEditor() }
                .disabled(store.timelineSelectionRange == nil).help("Set loop bounds from all selected clips or the selected time range")
            Spacer()
            Button("Turn off") { store.loopSelection = false; store.focusEditor() }
        }.font(.system(size: 11)).textFieldStyle(.roundedBorder).padding(.horizontal, 20).padding(.vertical, 7)
            .background(Theme.paper).disabled(store.busy || store.isRecording)
    }
}

struct SelectionPrecisionBar: View {
    @ObservedObject var store: SessionStore
    @ObservedObject private var settings: TimelineEditingState
    init(store: SessionStore) { self.store = store; self.settings = store.timelineEditing }
    var body: some View {
        HStack(spacing: 7) {
            if store.selectedRange != nil { Text("Selection").foregroundStyle(Theme.secondary) }
            if let range = store.selectedRange, let trackID = store.selectedTrack {
                if settings.timebase == .beats {
                    TimelineBeatField(title: "Selection start", frame: range.lowerBound, timing: store.session.timing) { frame in
                        guard store.selectedTrack == trackID, store.selectedRange == range else { return }
                        store.setSelectionEdge(start: true, seconds: Double(frame) / sessionRate)
                        store.focusEditor()
                    }.id("selection.beats.start.\(range.lowerBound).\(range.upperBound)").frame(width: 90)
                } else {
                    CommittedNumberField("Start seconds", value: Binding(get: { Double(range.lowerBound) / sessionRate }, set: { seconds in
                        guard store.selectedTrack == trackID, store.selectedRange == range else { return }
                        store.setSelectionEdge(start: true, seconds: seconds)
                    }), range: 0...(Double(Int.max / 4) / sessionRate), fractionDigits: 4, onSubmit: { store.focusEditor() })
                        .id("\(trackID).\(range.lowerBound).\(range.upperBound).start")
                        .frame(width: 90).accessibilityLabel("Selection start in seconds")
                }
                Text("to").foregroundStyle(Theme.secondary)
                if settings.timebase == .beats {
                    TimelineBeatField(title: "Selection end", frame: range.upperBound, timing: store.session.timing) { frame in
                        guard store.selectedTrack == trackID, store.selectedRange == range else { return }
                        store.setSelectionEdge(start: false, seconds: Double(frame) / sessionRate)
                        store.focusEditor()
                    }.id("selection.beats.end.\(range.lowerBound).\(range.upperBound)").frame(width: 90)
                } else {
                    CommittedNumberField("End seconds", value: Binding(get: { Double(range.upperBound) / sessionRate }, set: { seconds in
                        guard store.selectedTrack == trackID, store.selectedRange == range else { return }
                        store.setSelectionEdge(start: false, seconds: seconds)
                    }), range: 0...(Double(Int.max / 4) / sessionRate), fractionDigits: 4, onSubmit: { store.focusEditor() })
                        .id("\(trackID).\(range.lowerBound).\(range.upperBound).end")
                        .frame(width: 90).accessibilityLabel("Selection end in seconds")
                }
            }
            Spacer()
            TimelineGridControls(store: store, settings: store.timelineEditing)
            Button { store.showCommandSearch = true } label: { Label("Commands", systemImage: "command") }.help("Command search (⌘K)")
        }.font(.system(size: 12)).textFieldStyle(.roundedBorder).padding(.horizontal, 20).padding(.vertical, 8).background(Theme.paper)
    }
}

struct ClipHandleLayer: View {
    @ObservedObject var store: SessionStore
    let track: Track
    @State private var dragOffsets: [String: Double] = [:]
    @State private var protectionPopover: UUID?
    @State private var selectionDragAnchors: [String: Double] = [:]
    @Environment(\.timelineScrollInteraction) private var scrollInteraction
    private var lane: AudioLaneGeometry { AudioLaneGeometry(height: store.trackSize.height) }
    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .topLeading) {
                ForEach(track.clips.filter { store.selectedClips.contains($0.id) || store.selectedClip == $0.id }) { clip in
                    let x = Double(clip.start) / sessionRate * store.zoom
                    let width = Double(clip.length) / sessionRate * store.zoom
                    handle("Trim clip start", key: "\(clip.id).start", x: x, y: lane.bottom - 32, icon: "chevron.left") { value in
                        store.trimClip(clip.id, trackID: track.id, start: store.snappedFrame(clip.start + Int(value.translation.width / store.zoom * sessionRate), excluding: [clip.id]).frame)
                    }
                    handle("Trim clip end", key: "\(clip.id).end", x: x + width, y: lane.bottom - 32, icon: "chevron.right") { value in
                        store.trimClip(clip.id, trackID: track.id, end: store.snappedFrame(clip.end + Int(value.translation.width / store.zoom * sessionRate), excluding: [clip.id]).frame)
                    }
                    fadeHandle(clip: clip, fadeIn: true, x: x + Double(min(clip.length, max(0, (clip.fadeInFrames ?? 0) - (clip.fadeReferenceOffset ?? 0)))) / sessionRate * store.zoom, y: 41)
                    fadeHandle(clip: clip, fadeIn: false, x: x + Double(min(clip.length, max(0, (clip.fadeReferenceLength ?? clip.length) - (clip.fadeOutFrames ?? 0) - (clip.fadeReferenceOffset ?? 0)))) / sessionRate * store.zoom, y: 41)
                    Image(systemName: "arrow.up.and.down.and.arrow.left.and.right")
                        .font(.system(size: 10)).foregroundStyle(Theme.ink).frame(width: 24, height: 20)
                        .background(Theme.raised, in: RoundedRectangle(cornerRadius: 3)).contentShape(Rectangle())
                        .onDrag { NSItemProvider(object: clip.id.uuidString as NSString) }.help("Drag to another track").position(x: x + min(width / 2, 32), y: lane.bottom - 30)
                    Path { path in
                        let envelope = ClipEnvelope.resolve(track: track)[clip.id]
                        guard envelope != nil else { return }
                        let samples = max(2, min(128, Int(width / 4)))
                        for i in 0...samples {
                            let fraction = Double(i) / Double(samples)
                            let local = min(clip.length - 1, Int(fraction * Double(clip.length)))
                            let point = CGPoint(x: x + fraction * width, y: lane.bottom - Double(envelope!.gain(at: local)) * (lane.bottom - 37))
                            if i == 0 { path.move(to: point) } else { path.addLine(to: point) }
                        }
                    }.stroke(Theme.accent.opacity(0.6), lineWidth: 1).allowsHitTesting(false)
                }
                if let selection = store.selection, selection.trackID == track.id {
                    selectionHandle(selection: selection, start: true)
                    selectionHandle(selection: selection, start: false)
                }
                crossfadeHandles
                if let clip = track.clips.first(where: { store.selectedClip == $0.id }) {
                    let x = Double(clip.start) / sessionRate * store.zoom
                    let width = Double(clip.length) / sessionRate * store.zoom
                    Button { protectionPopover = clip.id } label: {
                        Image(systemName: clip.edgeProtectionDisabled == true ? "shield.slash" : "shield.checkered").font(.system(size: 10)).frame(width: 20, height: 18).background(Theme.paper, in: RoundedRectangle(cornerRadius: 3))
                    }.buttonStyle(.plain).accessibilityLabel("Clip edge protection").help("Clip edge protection settings")
                        .popover(isPresented: Binding(get: { protectionPopover == clip.id }, set: { if !$0 { protectionPopover = nil } })) {
                            VStack(alignment: .leading, spacing: 10) {
                                Toggle("Edge protection", isOn: Binding(get: { clip.edgeProtectionDisabled != true }, set: { store.setClipProtection(clip.id, trackID: track.id, enabled: $0) })).toggleStyle(.checkbox)
                                HStack {
                                    Text("Fade length")
                                    CommittedNumberField("ms", value: Binding(get: { Double(clip.edgeFadeFrames ?? 0) / sessionRate * 1000 }, set: { store.setClipProtection(clip.id, trackID: track.id, enabled: clip.edgeProtectionDisabled != true, milliseconds: $0) }), range: 0...100, fractionDigits: 1, onEditingChanged: { editing in if !editing { store.endContinuousEdit() } }).frame(width: 50).accessibilityLabel("Clip edge fade milliseconds")
                                    Text("ms")
                                }
                            }.font(.system(size: 11)).textFieldStyle(.roundedBorder).padding(14).frame(width: 230).background(Theme.paper).preferredColorScheme(.dark)
                                .onAppear { store.timelineEditing.clipSettingsVisible = true }
                                .onDisappear { store.timelineEditing.clipSettingsVisible = false; store.focusEditor() }
                        }.position(x: x + max(10, min(width - 10, 12)), y: lane.bottom - 13)
                }
            }.frame(width: geometry.size.width, height: geometry.size.height, alignment: .topLeading).coordinateSpace(name: track.id)
        }.disabled(store.busy || store.isRecording).opacity(store.clipMovePreview == nil ? 1 : 0)
    }
    private func updateSelectionHandle(identity: String, contentX: Double) {
        guard let anchor = selectionDragAnchors[identity], !store.busy, !store.isRecording else { scrollInteraction?.endDrag(); return }
        dragOffsets[identity] = contentX - anchor
    }
    private func selectionHandle(selection: EditSelection, start: Bool) -> some View {
        let identity = start ? "Selection start" : "Selection end"
        let original = start ? selection.range.lowerBound : selection.range.upperBound
        let x = Double(original) / sessionRate * store.zoom
        return Image(systemName: start ? "arrowtriangle.right.fill" : "arrowtriangle.left.fill")
            .font(.system(size: 9, weight: .bold)).foregroundStyle(Theme.ink)
            .frame(width: 14, height: 22).background(Theme.raised, in: RoundedRectangle(cornerRadius: 3)).contentShape(Rectangle())
            .gesture(DragGesture(minimumDistance: 1, coordinateSpace: .named(track.id)).onChanged { value in
                guard !store.busy, !store.isRecording else { return }
                if selectionDragAnchors[identity] == nil { selectionDragAnchors[identity] = value.startLocation.x }
                scrollInteraction?.beginDrag { contentX in updateSelectionHandle(identity: identity, contentX: contentX) }
                if let scrollInteraction { scrollInteraction.updateDragPosition(contentX: value.location.x) }
                else { updateSelectionHandle(identity: identity, contentX: value.location.x) }
            }.onEnded { _ in
                scrollInteraction?.endDrag()
                if store.selection == selection {
                    let raw = original + Int((dragOffsets[identity] ?? 0) / store.zoom * sessionRate)
                    store.setSelectionEdge(start: start, seconds: Double(store.snappedFrame(raw, useSelection: false).frame) / sessionRate)
                }
                dragOffsets[identity] = nil; selectionDragAnchors[identity] = nil; store.focusEditor()
            })
            .onDisappear { scrollInteraction?.endDrag() }
            .help(identity).accessibilityLabel(identity)
            .position(x: max(7, x + (dragOffsets[identity] ?? 0)), y: store.trackSize.height - 3)
    }
    private var crossfadeHandles: some View {
        let envelopes = ClipEnvelope.resolve(track: track)
        return ForEach(track.clips) { clip in
            if let overlap = envelopes[clip.id]?.crossfadeOut,
               let incoming = track.clips.first(where: { $0.start == overlap.lowerBound && $0.id != clip.id }) {
                let start = Double(overlap.lowerBound) / sessionRate * store.zoom
                let end = Double(overlap.upperBound) / sessionRate * store.zoom
                Path { p in
                    p.move(to: CGPoint(x: start, y: 36)); p.addLine(to: CGPoint(x: end, y: lane.bottom))
                    p.move(to: CGPoint(x: start, y: lane.bottom)); p.addLine(to: CGPoint(x: end, y: 36))
                }.stroke(Theme.accent.opacity(0.65), lineWidth: 1).allowsHitTesting(false)
                handle("Crossfade", key: "\(clip.id).crossfade", x: end, y: 58, icon: "arrow.left.and.right") { value in
                    let target = store.snappedFrame(clip.end + Int(value.translation.width / store.zoom * sessionRate), excluding: [clip.id]).frame
                    store.setCrossfadeLength(outgoing: clip.id, incoming: incoming.id, trackID: track.id, frames: target - incoming.start)
                }
            }
        }
    }
    private func handle(_ title: String, key: String? = nil, x: Double, y: Double, icon: String, commit: @escaping (DragGesture.Value) -> Void) -> some View {
        let identity = key ?? title
        return Image(systemName: icon).font(.system(size: 9, weight: .bold)).foregroundStyle(Theme.ink)
            .frame(width: 14, height: 22).background(Theme.raised, in: RoundedRectangle(cornerRadius: 3)).contentShape(Rectangle())
            .overlay(alignment: .top) {
                if let offset = dragOffsets[identity] {
                    Text(String(format: "%@ %.4f s (%+.4f)", title, (x + offset) / store.zoom, offset / store.zoom)).font(.system(size: 10, design: .monospaced)).fixedSize().padding(4).background(Theme.paper, in: RoundedRectangle(cornerRadius: 4)).offset(y: -25).allowsHitTesting(false)
                }
            }
            .gesture(DragGesture(minimumDistance: 1, coordinateSpace: .named(track.id)).onChanged { value in dragOffsets[identity] = value.translation.width }.onEnded { value in dragOffsets[identity] = nil; commit(value); store.focusEditor() }).help(title).accessibilityLabel(title).position(x: max(7, x + (dragOffsets[identity] ?? 0)), y: y)
    }
    private func fadeHandle(clip: Clip, fadeIn: Bool, x: Double, y: Double) -> some View {
        let title = fadeIn ? "Fade in" : "Fade out"
        let identity = "\(clip.id).\(fadeIn)"
        return Image(systemName: "diamond.fill").font(.system(size: 9)).foregroundStyle(Theme.accent)
            .frame(width: 18, height: 18).contentShape(Rectangle())
            .gesture(DragGesture(minimumDistance: 1, coordinateSpace: .named(track.id)).onChanged { value in dragOffsets[identity] = value.translation.width }.onEnded { value in
                dragOffsets[identity] = nil
                let old = fadeIn ? clip.fadeInFrames ?? 0 : clip.fadeOutFrames ?? 0
                let change = Int(value.translation.width / store.zoom * sessionRate) * (fadeIn ? 1 : -1)
                store.setClipFade(clip.id, trackID: track.id, fadeIn: fadeIn, frames: old + change)
                store.endContinuousEdit()
            }).help(title).accessibilityLabel(title).position(x: max(9, x + (dragOffsets[identity] ?? 0)), y: y)
    }
}

private struct TimelineGridControls: View {
    @ObservedObject var store: SessionStore
    @ObservedObject var settings: TimelineEditingState
    var body: some View {
        HStack(spacing: 5) {
            // Labeled, so the two pickers say what they change.
            Text("Ruler").foregroundStyle(Theme.secondary)
            Picker("Timeline units", selection: $settings.timebase) { ForEach(TimelineTimebase.allCases, id: \.self) { Text($0 == .beats ? "Bars & beats" : "Time").tag($0) } }.labelsHidden().frame(width: 104).accessibilityLabel("Ruler units")
                .help("Show the ruler in minutes and seconds, or in bars and beats")
                .onChange(of: settings.timebase) { _, value in if value == .beats { settings.grid = .beat; settings.beatDivision = 1; store.snap = true } }
            Text("Snap to").foregroundStyle(Theme.secondary).padding(.leading, 6)
            Picker("Snap grid", selection: $settings.grid) { ForEach(TimelineGrid.allCases) { Text($0.title).tag($0) } }.labelsHidden().frame(width: 100).accessibilityLabel("Snap to")
                .help("What selections and moved clips line up with: other clips' edges, a time grid, or the beat grid")
            if settings.grid == .time {
                Picker("Time division", selection: $settings.timeDivision) { ForEach([0.01, 0.1, 0.5, 1.0], id: \.self) { Text(String(format: "%g s", $0)).tag($0) } }.labelsHidden().frame(width: 70)
            } else if settings.grid == .beat {
                Picker("Beat division", selection: $settings.beatDivision) { Text("1/16").tag(0.25); Text("1/8").tag(0.5); Text("1/4").tag(1.0); Text("Bar").tag(0.0) }.labelsHidden().frame(width: 65)
            }
        }
    }
}

/// Commit musical coordinates only after entry is complete. Untouched drafts
/// cannot overwrite a newer selection, timing or another client's loop edit.
private struct TimelineBeatField: View {
    let title: String
    let frame: Int
    let timing: MusicalTiming
    let commitFrame: (Int) -> Void
    @State private var text = ""
    @State private var initialText = ""
    @State private var editing = false
    @FocusState private var focused: Bool
    var body: some View {
        TextField(title, text: $text).font(.system(size: 11, design: .monospaced)).focused($focused)
            .accessibilityLabel(title + " in bars, beats and ticks")
            .help("Enter bar.beat.tick; bars and beats start at 1, with 960 ticks per quarter note")
            .onAppear { synchronize() }
            .onChange(of: frame) { _, _ in synchronize() }
            .onChange(of: timing) { _, _ in synchronize() }
            .onChange(of: focused) { _, value in
                if value { editing = true; initialText = text } else { commit() }
            }
            .onSubmit { commit(); focused = false }
            .onDisappear { commit() }
    }
    private func synchronize() { if !editing { text = timing.position(frame) } }
    private func commit() {
        guard editing else { return }; editing = false
        if text != initialText, let next = timing.frame(atPosition: text), next != frame { commitFrame(next) }
        synchronize()
    }
}
