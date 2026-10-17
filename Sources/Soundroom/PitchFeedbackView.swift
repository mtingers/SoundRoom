import SoundroomCore
import SwiftUI

struct PitchFeedbackView: View {
    let snapshot: () -> EffectMeterSnapshot?
    let running: Bool
    let bypassed: Bool
    @State private var history = PitchFeedbackHistory()
    @State private var traceExpanded = false
    private let inputColor = Theme.tracks[1]
    private let wetColor = Theme.accent

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack {
                Text("Pitch").fontWeight(.medium)
                Spacer()
                Text(status).foregroundStyle(Theme.secondary)
            }
            HStack(alignment: .top, spacing: 4) {
                readout("Input", frequency: voiced?.inputFrequency, color: inputColor)
                readout("Target", frequency: voiced?.targetFrequency, color: Theme.secondary)
                readout("Wet estimate", frequency: voiced?.estimatedWetFrequency, color: wetColor)
            }
            HStack {
                Text(shiftLabel).monospacedDigit()
                Spacer()
                Text(confidenceLabel).monospacedDigit()
            }.foregroundStyle(Theme.secondary)
            DisclosureGroup("Pitch trace", isExpanded: $traceExpanded) {
                VStack(spacing: 5) {
                    HStack(spacing: 8) {
                        Button {
                            history.zoomScale(0.5)
                        } label: {
                            Image(systemName: "plus.magnifyingglass")
                        }
                        .accessibilityLabel("Zoom pitch range in")
                        Button {
                            history.zoomScale(2)
                        } label: {
                            Image(systemName: "minus.magnifyingglass")
                        }
                        .accessibilityLabel("Zoom pitch range out")
                        Button {
                            history.moveScale(6)
                        } label: {
                            Image(systemName: "arrow.up")
                        }
                        .accessibilityLabel("Move pitch range up")
                        Button {
                            history.moveScale(-6)
                        } label: {
                            Image(systemName: "arrow.down")
                        }
                        .accessibilityLabel("Move pitch range down")
                        Spacer()
                        Button("Fit") { history.fitScale() }.disabled(history.notes.isEmpty)
                            .accessibilityLabel("Fit pitch trace range")
                    }.buttonStyle(.borderless)
                    pitchTrace
                    if history.hasNotesOutsideScale {
                        Text("Pitch outside view · use Fit").foregroundStyle(Theme.secondary)
                    }
                }.padding(.top, 5)
            }
        }.font(.system(size: 10))
            .padding(10).background(Theme.raised, in: RoundedRectangle(cornerRadius: 6))
            .help(
                "Blue: detected input. Gray: target. Green: estimated wet pitch from the synthesis shift, not measured output. Dry/wet mixtures can contain both pitches. Trace shows eight seconds; gaps mean unvoiced audio. The vertical scale stays fixed; use zoom, arrows or Fit to change it."
            )
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Pitch feedback. \(status). \(shiftLabel). \(confidenceLabel)")
            .task(id: running && !bypassed) {
                guard running && !bypassed else {
                    history.clear()
                    return
                }
                while !Task.isCancelled {
                    let value = snapshot()
                    history.update(
                        value?.pitch, at: Date.timeIntervalSinceReferenceDate,
                        running: true, revision: value?.revision)
                    do { try await Task.sleep(for: .milliseconds(80)) } catch { return }
                }
            }
            .onChange(of: running) { _, value in if !value { history.clear() } }
            .onChange(of: bypassed) { _, value in if value { history.clear() } }
    }

    private var voiced: PitchMeterSnapshot? {
        guard running, !bypassed, let pitch = history.reading, pitch.isVoiced else { return nil }
        return pitch
    }
    private var status: String {
        if bypassed { return "Bypassed" }
        if !running { return "Play to see pitch" }
        guard let reading = history.reading else { return "Waiting for audio" }
        return reading.isVoiced ? "Voiced" : "Unvoiced"
    }
    private var shiftLabel: String {
        guard let pitch = voiced else { return "Shift —" }
        return String(format: "Wet shift %+.0f¢ · %.0f%%", pitch.correctionCents, pitch.wetBlend * 100)
    }
    private var confidenceLabel: String {
        guard let pitch = history.reading, running, !bypassed else { return "Confidence —" }
        return String(format: "Confidence %.0f%%", max(0, min(1, pitch.confidence)) * 100)
    }
    private func noteName(_ midi: Int) -> String {
        let names = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"]
        return "\(names[(midi % 12 + 12) % 12])\(Int(floor(Double(midi) / 12)) - 1)"
    }
    private func readout(_ title: String, frequency: Float?, color: Color) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title).foregroundStyle(color)
            if let frequency, let midi = voiced?.midiNote(for: frequency) {
                Text(noteName(Int(midi.rounded()))).font(.system(size: 15, weight: .medium)).foregroundStyle(Theme.ink)
                Text(String(format: "%.1f Hz", frequency)).monospacedDigit().foregroundStyle(Theme.secondary)
            } else {
                Text("—").font(.system(size: 15, weight: .medium)).foregroundStyle(Theme.ink)
                Text("— Hz").foregroundStyle(Theme.secondary)
            }
        }.frame(maxWidth: .infinity, alignment: .leading)
    }

    private var pitchTrace: some View {
        Canvas { context, size in
            let points = history.points
            let lower = history.scale.lower
            let upper = history.scale.upper
            let span = history.scale.span
            let now = points.last?.time ?? 0
            let x: (TimeInterval) -> CGFloat = {
                25 + (size.width - 27) * CGFloat(max(0, min(1, 1 - (now - $0) / PitchFeedbackHistory.duration)))
            }
            let y: (Float) -> CGFloat = { 4 + (size.height - 8) * CGFloat(1 - ($0 - lower) / span) }
            let step = max(1, Int(ceil(span / 6)))
            for note in stride(from: Int(lower), through: Int(upper), by: step) {
                var line = Path()
                line.move(to: CGPoint(x: 25, y: y(Float(note))))
                line.addLine(to: CGPoint(x: size.width, y: y(Float(note))))
                context.stroke(line, with: .color(Theme.line.opacity(0.6)), lineWidth: 1)
                context.draw(
                    Text(noteName(note)).font(.system(size: 8)).foregroundColor(Theme.secondary),
                    at: CGPoint(x: 0, y: y(Float(note))), anchor: .leading)
            }
            for (frequency, color, dashed) in [
                (\PitchMeterSnapshot.inputFrequency, inputColor, false),
                (\PitchMeterSnapshot.targetFrequency, Theme.secondary, true),
                (\PitchMeterSnapshot.estimatedWetFrequency, wetColor, false),
            ] {
                var path = Path()
                var previous: PitchFeedbackHistory.Point?
                for point in points {
                    guard point.pitch.isVoiced, let midi = point.pitch.midiNote(for: point.pitch[keyPath: frequency])
                    else {
                        previous = nil
                        continue
                    }
                    let position = CGPoint(x: x(point.time), y: y(midi))
                    if let previous, point.time - previous.time < 0.25 {
                        path.addLine(to: position)
                    } else {
                        path.move(to: position)
                    }
                    previous = point
                }
                context.stroke(
                    path, with: .color(color),
                    style: StrokeStyle(lineWidth: dashed ? 1 : 1.6, dash: dashed ? [3, 3] : []))
            }
        }.frame(height: 106).clipped()
            .accessibilityLabel("Eight second pitch trace: blue input, gray target, green wet estimate")
    }
}
