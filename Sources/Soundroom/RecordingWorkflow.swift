import SwiftUI
import AppKit
import SoundroomCore

extension SessionStore {
    var recordingSetupSummary: String {
        let count = countInBars == 0 ? "" : "\(countInBars)-bar count-in"
        let roll = preRollSeconds <= 0 ? "" : "\(preRollSeconds.formatted(.number.precision(.fractionLength(1)))) s pre-roll"
        return [punchRecording ? "Punch selection" : "Record at playhead", count, roll].filter { !$0.isEmpty }.joined(separator: " · ")
    }
    func useTake(_ id: UUID, trackID: UUID, range requestedRange: Range<Int>? = nil) {
        guard !busy, !isRecording, let t = session.tracks.firstIndex(where: { $0.id == trackID }),
              let take = session.tracks[t].recordingTakes.first(where: { $0.id == id }), let fullRange = take.range else { return }
        let range = requestedRange ?? fullRange
        guard !range.isEmpty else { return }
        var source = session; source.tracks[t].clips = take.clips
        let pieces = source.fragments(trackID: trackID, range: range).map { value -> Clip in
            var clip = value; clip.start += range.lowerBound; return clip
        }
        guard !pieces.isEmpty else { status = "This take has no audio in the selection"; return }
        edit(requestedRange == nil ? "Use \(take.name)" : "Comp \(take.name)") {
            session.delete(trackID: trackID, range: range)
            session.tracks[t].clips.append(contentsOf: pieces)
            session.tracks[t].clips.sort { $0.start < $1.start }
            session.tracks[t].activeTakeID = requestedRange == nil ? id : nil
            selectedTrack = trackID; selectedClip = pieces.first?.id; selectedClips = Set(pieces.map(\.id)); playhead = range.lowerBound
        }
    }
}

struct RecordingSetupView: View {
    @ObservedObject var store: SessionStore
    @Environment(\.dismiss) private var dismiss
    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            Text("Recording").font(.title2.weight(.semibold))
            Toggle("Punch in and out at the selection", isOn: $store.punchRecording)
            HStack { Text("Pre-roll"); Spacer(); TextField("Seconds", value: $store.preRollSeconds, format: .number).frame(width: 80); Text("seconds") }
            Picker("Count-in", selection: $store.countInBars) { Text("Off").tag(0); Text("1 bar").tag(1); Text("2 bars").tag(2); Text("4 bars").tag(4) }
            HStack { Text("Tempo"); Spacer(); CommittedNumberField("BPM", value: Binding(get: { store.session.timing.tempo }, set: { store.setMusicalTiming(tempo: $0) }), range: 20...300, onSubmit: { store.focusEditor() }).frame(width: 80); Text("BPM") }
            Toggle("Metronome while recording", isOn: $store.recordingMetronome)
            Toggle("Monitor armed inputs through output", isOn: $store.inputMonitoring)
                .help("Armed inputs mix using the volume and pan set when recording starts. Reduce track levels if the output meter clips.")
            Text("Use headphones for input monitoring. Previous takes stay available for comping.").font(.caption).foregroundStyle(Theme.secondary)
            HStack { Spacer(); Button("Done") {
                NSApp.keyWindow?.makeFirstResponder(nil)
                store.preRollSeconds = store.preRollSeconds.isFinite ? min(30, max(0, store.preRollSeconds)) : 0
                store.recordingTempo = store.session.timing.tempo
                dismiss(); store.focusEditor()
            }.keyboardShortcut(.defaultAction).buttonStyle(RoomButtonStyle(prominent: true)) }
        }.textFieldStyle(.roundedBorder).padding(24).frame(width: 460)
            .background(Theme.paper).foregroundStyle(Theme.ink).tint(Theme.accent).preferredColorScheme(.dark)
    }
}

/// Take rows share one time scale. Drag in a source lane, then promote that range
/// into the playable comp. This never layers takes on top of each other.
struct TakeBrowserView: View {
    @ObservedObject var store: SessionStore
    @Environment(\.dismiss) private var dismiss
    @StateObject private var audition = TakeAudition()
    @ObservedObject private var waveformCache = WaveformCache.shared
    @FocusState private var keyboardFocus: Bool
    @State private var selectedTake: UUID?
    @State private var compRange: Range<Int>?
    @State private var anchor: Int?
    private var track: Track? { store.currentTrack }
    private var end: Int { max(1, track?.recordingTakes.flatMap(\.clips).map(\.end).max() ?? 1) }
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack { Text("\(track?.name ?? "Track") takes").font(.title2.weight(.semibold)); Spacer(); Button("Done") { dismiss(); store.focusEditor() }.keyboardShortcut(.cancelAction) }
            if let track, !track.recordingTakes.isEmpty {
                ScrollView {
                    VStack(spacing: 8) {
                        ForEach(track.recordingTakes) { take in
                            HStack(spacing: 12) {
                                Text(take.name).font(.callout).frame(width: 110, alignment: .leading)
                                GeometryReader { geo in
                                    Canvas { context, size in
                                        context.fill(Path(CGRect(origin: .zero, size: size)), with: .color(Theme.background))
                                        for clip in take.clips {
                                            let x = Double(clip.start) / Double(end) * size.width, width = Double(clip.length) / Double(end) * size.width
                                            let rect = CGRect(x: x, y: 3, width: max(1, width), height: size.height - 6)
                                            context.fill(Path(roundedRect: rect, cornerRadius: 3), with: .color(Theme.track(track.color).opacity(0.25)))
                                            if let peaks = store.waveformEnvelope(clip.assetID) {
                                                var waveform = Path(); let pixels = max(1, min(2000, Int(width)))
                                                for pixel in 0..<pixels {
                                                    let first = max(0, (clip.offset + pixel * clip.length / pixels) / peaks.framesPerBin)
                                                    let last = min(peaks.count - 1, (clip.offset + min(clip.length - 1, (pixel + 1) * clip.length / pixels)) / peaks.framesPerBin)
                                                    guard first <= last, first < peaks.count else { continue }
                                                    var low: Float = 0, high: Float = 0
                                                    for bin in first...last { low = min(low, peaks[bin].0); high = max(high, peaks[bin].1) }
                                                    let px = x + Double(pixel) / Double(pixels) * width, scale = size.height / 2 - 6
                                                    waveform.move(to: CGPoint(x: px, y: size.height / 2 - Double(min(1, high)) * scale))
                                                    waveform.addLine(to: CGPoint(x: px, y: size.height / 2 - Double(max(-1, low)) * scale))
                                                }
                                                context.stroke(waveform, with: .color(Theme.track(track.color)), lineWidth: 1)
                                            }
                                        }
                                        if selectedTake == take.id, let range = compRange {
                                            let rect = CGRect(x: Double(range.lowerBound) / Double(end) * size.width, y: 0, width: Double(range.count) / Double(end) * size.width, height: size.height)
                                            context.fill(Path(rect), with: .color(Theme.accent.opacity(0.25))); context.stroke(Path(rect), with: .color(Theme.accent), lineWidth: 1)
                                        }
                                    }.gesture(DragGesture(minimumDistance: 0).onChanged { value in
                                        let frame = max(0, min(end, Int(value.location.x / max(1, geo.size.width) * Double(end))))
                                        if anchor == nil { anchor = max(0, min(end, Int(value.startLocation.x / max(1, geo.size.width) * Double(end)))) }
                                        selectedTake = take.id; compRange = min(anchor!, frame)..<max(anchor!, frame)
                                    }.onEnded { _ in anchor = nil })
                                }.frame(height: 58).accessibilityLabel("\(take.name) source take; drag to select a comp range")
                                Button("Use take") { audition.stop(); store.useTake(take.id, trackID: track.id) }.buttonStyle(RoomButtonStyle())
                            }
                        }
                    }
                }.frame(minHeight: 120, maxHeight: 400)
                HStack {
                    Button(audition.playing || audition.preparing ? "Stop" : "Play take") { audition.toggle(store: store, takeID: selectedTake, range: compRange) }.buttonStyle(RoomButtonStyle()).help("Play or stop take audition (Space)")
                    Text(compRange.map { "\(timeString($0.lowerBound)) – \(timeString($0.upperBound))" } ?? "Drag in a take to select a comp range").font(.caption).foregroundStyle(Theme.secondary)
                    Spacer()
                    Button("Comp selection") { if let take = selectedTake, let range = compRange { audition.stop(); store.useTake(take, trackID: track.id, range: range) } }.buttonStyle(RoomButtonStyle(prominent: true)).disabled(compRange?.isEmpty != false)
                }
                HStack { Button(store.undoTitle) { audition.stop(); store.undo() }.disabled(!store.canUndo).keyboardShortcut("z", modifiers: .command); Button(store.redoTitle) { audition.stop(); store.redo() }.disabled(!store.canRedo).keyboardShortcut("z", modifiers: [.command, .shift]); Spacer() }
                if let error = audition.error { Text(error).font(.caption).foregroundStyle(.orange) }
            } else { Text("Record another pass to create take lanes.").foregroundStyle(Theme.secondary).frame(height: 160) }
        }.padding(24).frame(width: 850).background(Theme.paper).foregroundStyle(Theme.ink).tint(Theme.accent).preferredColorScheme(.dark)
            .focusable().focused($keyboardFocus)
            .onAppear { store.stopTransport(); keyboardFocus = true }
            .onKeyPress(.space) { audition.toggle(store: store, takeID: selectedTake, range: compRange); return .handled }
            .onChange(of: selectedTake) { _, _ in audition.stop() }
            .onDisappear { audition.stop() }
    }
}
