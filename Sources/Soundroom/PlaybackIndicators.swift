import Combine
import SoundroomCore
import SwiftUI

/// The render clock and meter are display telemetry, not document changes.
/// Only the small views that need them subscribe; editing the session still
/// invalidates the editor normally through SessionStore.
@MainActor final class PlaybackPosition: ObservableObject {
    @Published private(set) var frame = 0
    func update(_ frame: Int) { if self.frame != frame { self.frame = frame } }
}
@MainActor final class PlaybackLevel: ObservableObject {
    @Published private(set) var peak: Float = 0
    func update(_ peak: Float) {
        let value = peak.isFinite ? max(0, peak) : 0
        if self.peak != value { self.peak = value }
    }
}

/// Input telemetry redraws only the armed-track meter and recording overlay.
@MainActor final class InputActivity: ObservableObject {
    @Published var peaks: [Float] = []
    @Published var history = LivePeakHistory()
    func update(_ peaks: [Float], recording: Bool) {
        if self.peaks != peaks { self.peaks = peaks }
        if recording { history.append(peaks) }
    }
    func reset() {
        peaks = []
        history = LivePeakHistory()
    }
}

struct RecordingInputMeter: View {
    @ObservedObject var activity: InputActivity
    let channels: [Int]
    let trackName: String
    private var peak: Float {
        channels.reduce(Float(0)) { max($0, activity.peaks.indices.contains($1) ? activity.peaks[$1] : 0) }
    }
    var body: some View {
        GeometryReader { proxy in
            RoundedRectangle(cornerRadius: 2).fill(Theme.line)
            RoundedRectangle(cornerRadius: 2).fill(
                peak >= 1 ? Theme.recording : peak >= 0.8 ? Theme.warning : Theme.playback
            ).frame(width: proxy.size.width * CGFloat(min(1, peak)))
        }.frame(height: 4).accessibilityLabel("\(trackName) input level").accessibilityValue(
            peak >= 1 ? "Clipping" : String(format: "%.0f percent", peak * 100))
    }
}

struct TransportPositionReadout: View {
    @ObservedObject var position: PlaybackPosition
    let playing: Bool
    let recording: Bool
    var timing = MusicalTiming.standard
    @ObservedObject var settings: TimelineEditingState
    private var label: String {
        settings.timebase == .beats ? timing.position(position.frame) : timeString(position.frame)
    }
    var body: some View {
        Text(label).font(.system(size: 20, weight: .regular, design: .monospaced)).monospacedDigit()
            .foregroundStyle(recording ? Theme.recording : Theme.ink).frame(width: 108, alignment: .leading)
            .accessibilityLabel("\(recording ? "Recording" : playing ? "Playing" : "Playhead") \(label)")
    }
}
struct OutputLevelReadout: View {
    @ObservedObject var level: PlaybackLevel
    var compact = false
    static func spokenLevel(_ peak: Float) -> String {
        if peak >= 1 { return "Clipping" }
        if peak <= 0 { return "Silent" }
        return String(format: "%.0f decibels", 20 * log10(peak))
    }
    var body: some View {
        HStack(spacing: compact ? 1 : 2) {
            ForEach(0..<18) { index in
                RoundedRectangle(cornerRadius: 1).fill(
                    level.peak >= pow(10, Float(index - 17) * 2 / 20)
                        ? (index == 17 ? Theme.recording : index > 14 ? Theme.warning : Theme.playback) : Theme.line
                )
                .frame(width: compact ? 1 : 3, height: 14)
            }
        }.accessibilityElement(children: .ignore).accessibilityLabel("Output level").accessibilityValue(
            Self.spokenLevel(level.peak)
        )
        .help(
            level.peak >= 1
                ? "Output is clipping. Lower track levels." : "Output level · \(Self.spokenLevel(level.peak))")
    }
}

/// One overlay spans the ruler, audio lanes and automation lanes. Advancing it
/// never invalidates a clip Canvas or reaggregates its waveform envelope.
struct TimelinePlayhead: View {
    @ObservedObject var position: PlaybackPosition
    let zoom: Double
    let recording: Bool
    let laneHeight: Double
    var body: some View {
        Canvas { context, size in
            let x = 5.0
            var tip = Path()
            tip.move(to: CGPoint(x: x - 5, y: 0))
            tip.addLine(to: CGPoint(x: x + 5, y: 0))
            tip.addLine(to: CGPoint(x: x + 5, y: 22))
            tip.addLine(to: CGPoint(x: x, y: 28))
            tip.addLine(to: CGPoint(x: x - 5, y: 22))
            tip.closeSubpath()
            context.fill(tip, with: .color(recording ? Theme.recording : Theme.playback))
            var line = Path()
            line.move(to: CGPoint(x: x, y: 34))
            line.addLine(to: CGPoint(x: x, y: min(size.height, 34 + laneHeight)))
            context.stroke(line, with: .color(recording ? Theme.recording : Theme.playback), lineWidth: 1)
        }.frame(width: 10, height: 34 + laneHeight)
            .offset(x: Double(position.frame) / sessionRate * zoom - 5)
            .allowsHitTesting(false).accessibilityHidden(true)
    }
}

struct TrackMixControl: View {
    @ObservedObject var store: SessionStore
    let track: Track
    let parameter: TrackAutomationParameter
    @State private var displayedFrame = 0
    private var canReset: Bool {
        (track.mixAutomation[parameter.rawValue] ?? []).isEmpty && !store.isRecording && !store.busy
    }
    private var value: Float {
        guard !(track.mixAutomation[parameter.rawValue] ?? []).isEmpty else {
            return parameter == .volume ? track.gain : track.pan
        }
        return parameter.physical(track.value(at: displayedFrame, parameter: parameter))
    }
    var body: some View {
        HStack(spacing: 6) {
            RoomSlider(
                value: Binding(
                    get: { Double(value) },
                    set: { store.setTrackMixValue(track.id, parameter: parameter, value: Float($0)) }),
                in: parameter == .volume ? 0...1.5 : -1...1,
                label: "\(track.name) \(parameter == .volume ? "volume" : "pan")",
                onEditingChanged: { editing in
                    if editing {
                        store.beginContinuousEdit("\(track.id).\(parameter == .volume ? "gain" : "pan")")
                    } else {
                        store.endContinuousEdit()
                    }
                }
            )
            .help(
                parameter == .volume
                    ? "Track volume. Enable Write to record fader movements."
                    : "Pan left / right. Center = 0. Enable Write to record movements."
            )
            .disabled(
                store.isPlaying && !store.automationWrite && !(track.mixAutomation[parameter.rawValue] ?? []).isEmpty)
            if parameter == .pan {
                // L/R amount in percent, C at center (as on a mixing desk).
                Text(Self.panLabel(value)).font(.system(size: 9, design: .monospaced)).foregroundStyle(Theme.secondary)
                    .frame(width: 23)
                    .contentShape(Rectangle())
                    .onTapGesture(count: 2) { resetToUnity() }
                    .help("Pan \(Self.panLabel(value)). Double-click to center.")
                    .accessibilityLabel("\(track.name) pan \(Self.panLabel(value))")
                    .accessibilityHint(
                        canReset ? "Reset to center" : "Reset unavailable while automated, recording or processing"
                    )
                    .accessibilityActions { if canReset { Button("Reset pan to center") { resetToUnity() } } }
            }
            if parameter == .volume {
                Text(value <= 0 ? "−∞" : String(format: "%+.0f", 20 * log10(value))).font(
                    .system(size: 9, design: .monospaced)
                ).foregroundStyle(Theme.secondary).frame(width: 23)
                    .contentShape(Rectangle())
                    .onTapGesture(count: 2) { resetToUnity() }
                    .help(
                        value <= 0
                            ? "Volume −∞ dB (silent). Double-click to reset to 0 dB."
                            : String(format: "Volume %+.1f dB. Double-click to reset to 0 dB.", 20 * log10(value))
                    )
                    .accessibilityLabel(
                        value <= 0
                            ? "\(track.name) volume silent"
                            : String(format: "%@ volume %.1f decibels", track.name, 20 * log10(value))
                    )
                    .accessibilityHint(
                        canReset
                            ? "Reset to zero decibels" : "Reset unavailable while automated, recording or processing"
                    )
                    .accessibilityActions { if canReset { Button("Reset volume to zero decibels") { resetToUnity() } } }
            }
        }
        .onReceive(store.playbackPosition.$frame) { frame in
            // Constant automation segments leave AppKit's sliders alone.
            let next =
                (track.mixAutomation[parameter.rawValue] ?? []).isEmpty
                ? (parameter == .volume ? track.gain : track.pan)
                : parameter.physical(track.value(at: frame, parameter: parameter))
            if next != value { displayedFrame = frame }
        }
    }
}

extension TrackMixControl {
    static func panLabel(_ value: Float) -> String {
        let percent = Int((abs(value) * 100).rounded())
        return percent == 0 ? "C" : (value < 0 ? "L" : "R") + "\(percent)"
    }
    /// 0 dB for volume, center for pan (unless the lane is automated).
    func resetToUnity() {
        guard canReset else { return }
        store.setTrackMixValue(track.id, parameter: parameter, value: parameter == .volume ? 1 : 0)
    }
}

struct AutomationPlayheadValue: View {
    @ObservedObject var store: SessionStore
    @ObservedObject var position: PlaybackPosition
    init(store: SessionStore) {
        self.store = store
        self.position = store.playbackPosition
    }
    var body: some View {
        // Reading the position here scopes invalidation to this text.
        let _ = position.frame
        Text(store.automationDisplay(store.automationValueAtPlayhead)).monospacedDigit()
    }
}

struct RecordingLaneOverlay: View {
    @ObservedObject var store: SessionStore
    @ObservedObject var position: PlaybackPosition
    @ObservedObject var activity: InputActivity
    let track: Track
    init(store: SessionStore, track: Track) {
        self.store = store
        self.position = store.playbackPosition
        self.activity = store.inputActivity
        self.track = track
    }
    var body: some View {
        Canvas { context, size in
            let lane = AudioLaneGeometry(height: size.height)
            let cursor = Double(position.frame) / sessionRate * store.zoom
            let start = Double(store.recordingStartFrame) / sessionRate * store.zoom
            let width = max(1, cursor - start)
            context.fill(
                Path(CGRect(x: start, y: 8, width: width, height: lane.bottom - 8)),
                with: .color(Color.red.opacity(0.12)))
            let route = store.recordingInputChannels(for: track)
            let channels = activity.history.channelPeaks(channels: route)
            let geometry = WaveformChannelGeometry(top: 36, bottom: lane.bottom, channels: max(1, channels.count))
            var waveform = Path()
            for (channel, peaks) in channels.enumerated() {
                for (index, peak) in peaks.enumerated() {
                    let x = start + Double(index) / Double(max(1, peaks.count)) * width
                    waveform.move(
                        to: CGPoint(x: x, y: geometry.center(channel) - Double(min(1, peak)) * geometry.amplitude))
                    waveform.addLine(
                        to: CGPoint(x: x, y: geometry.center(channel) + Double(min(1, peak)) * geometry.amplitude))
                }
            }
            context.stroke(waveform, with: .color(.red.opacity(0.7)), lineWidth: 1)
            if channels.count == 2, width > 24 {
                for channel in 0..<2 {
                    let label = CGRect(x: start + 3, y: geometry.center(channel) - 7, width: 15, height: 14)
                    context.fill(Path(roundedRect: label, cornerRadius: 3), with: .color(Theme.background.opacity(0.9)))
                    context.draw(
                        Text(channel == 0 ? "L" : "R").font(.system(size: 9, weight: .semibold)).foregroundStyle(
                            Color.red), at: CGPoint(x: label.midX, y: label.midY))
                }
            }
            let format = AudioChannelPresentation.label(channels: route.count) ?? "Audio"
            context.draw(
                Text("Recording · \(format)").font(.system(size: 11, weight: .medium)).foregroundStyle(Color.red),
                at: CGPoint(x: start + 8, y: 21), anchor: .leading)
        }.allowsHitTesting(false).accessibilityHidden(true)
    }
}
