import Combine
import SoundroomCore
import SwiftUI

/// Local 12.5 Hz polling keeps measurement animation out of the document store.
struct ProfessionalEffectMeterView: View {
    let kind: RealtimeEffectKind
    let snapshot: () -> EffectMeterSnapshot?
    @State private var reading = EffectMeterSnapshot()
    @State private var history: [Float] = []
    private let timer = Timer.publish(every: 0.08, on: .main, in: .common).autoconnect()
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if kind == .compressor {
                CompressorMeterView(snapshot: snapshot)
            } else {
                HStack {
                    Text("Live reduction")
                    Spacer()
                    Text(String(format: "%.1f dB", max(0, reading.gainReductionDB))).monospacedDigit()
                }
                Canvas { context, size in
                    if kind == .resonance || kind == .sourceIsolator {
                        let values = reading.resonance.bands
                        for i in values.indices {
                            let height = size.height * CGFloat(min(1, values[i] / 18))
                            let rect = CGRect(
                                x: CGFloat(i) / 12 * size.width + 1, y: 0, width: max(1, size.width / 12 - 2),
                                height: height)
                            context.fill(Path(rect), with: .color(Theme.accent.opacity(0.8)))
                        }
                    } else {
                        var curve = Path()
                        for i in history.indices {
                            let point = CGPoint(
                                x: CGFloat(i) / 119 * size.width, y: size.height * CGFloat(min(1, history[i] / 18)))
                            if i == 0 { curve.move(to: point) } else { curve.addLine(to: point) }
                        }
                        context.stroke(curve, with: .color(Theme.accent), lineWidth: 1.5)
                    }
                }.frame(height: 52).background(Theme.background, in: RoundedRectangle(cornerRadius: 4))
                if kind == .resonance || kind == .sourceIsolator {
                    HStack {
                        Text("80 Hz")
                        Spacer()
                        Text("1 kHz")
                        Spacer()
                        Text("20 kHz")
                    }
                } else if kind == .vocalEnhancer {
                    HStack {
                        Text("Output sample peak")
                        Spacer()
                        Text(level(reading.outputPeak)).monospacedDigit()
                    }
                    HStack {
                        Text("Input / output RMS")
                        Spacer()
                        Text("\(level(reading.inputRMS)) / \(level(reading.outputRMS))").monospacedDigit()
                    }
                } else {
                    HStack {
                        Text("True peak")
                        Spacer()
                        Text(
                            reading.truePeak < 0.000001
                                ? "−∞ dBTP" : String(format: "%.1f dBTP", 20 * log10(reading.truePeak))
                        ).monospacedDigit()
                    }
                    HStack {
                        Text("Momentary / short-term")
                        Spacer()
                        Text(String(format: "%.1f / %.1f LUFS", reading.momentaryLUFS, reading.shortTermLUFS))
                            .monospacedDigit()
                    }
                }
            }
        }.font(.system(size: 10)).foregroundStyle(Theme.secondary)
            .onReceive(timer) { _ in
                guard let next = snapshot(), next.revision != reading.revision else { return }
                reading = next
                history.append(next.gainReductionDB)
                if history.count > 120 { history.removeFirst(history.count - 120) }
            }.accessibilityElement(children: .combine)
            .help(
                kind == .resonance || kind == .sourceIsolator
                    ? "Measured reduction in twelve logarithmic bands. Channels share processing decisions; center and sides intentionally change stereo content."
                    : kind == .vocalEnhancer
                        ? "Measured processing reduction and sample levels. Peak protection is built into the vocal processor; the output readout is a sample peak measurement."
                        : "Measured gain reduction history over approximately 9.6 seconds. Momentary: 400 ms; short-term: 3 s. These are not integrated loudness readings."
            )
    }
    private func level(_ value: Float) -> String {
        value < 0.000001 ? "−∞ dBFS" : String(format: "%.1f dBFS", 20 * log10(value))
    }
}

struct EffectAuditionControls: View {
    @ObservedObject var store: SessionStore
    var body: some View {
        HStack(spacing: 8) {
            if let track = store.selectedTrack, track != Session.masterTrackID {
                Button {
                    EffectWorkspaceFocus.finishNumberEntry { store.toggleEffectAudition(trackID: track) }
                } label: {
                    Label(
                        store.isPlaying && store.auditionTrackID == track ? "Pause track" : "Play track",
                        systemImage: "headphones")
                }
                .buttonStyle(RoomButtonStyle()).accessibilityLabel(
                    "\(store.isPlaying && store.auditionTrackID == track ? "Pause" : "Play") track audition")
            }
            Button {
                EffectWorkspaceFocus.finishNumberEntry { store.toggleEffectAudition(trackID: nil) }
            } label: {
                Label(
                    store.isPlaying && store.auditionTrackID == nil ? "Pause mix" : "Play mix", systemImage: "play.fill"
                )
            }.buttonStyle(RoomButtonStyle(prominent: true))
            Spacer(minLength: 0)
        }.font(.system(size: 11)).disabled(store.busy || store.isRecording || store.session.end == 0)
    }
}
