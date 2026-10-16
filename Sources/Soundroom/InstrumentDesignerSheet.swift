import SwiftUI
import SoundroomCore

/// Shapes a recorded instrument's measured model. While the sheet is open the
/// instrument is generated (Designed); "Use recording" returns to the
/// recorded samples. Changes are heard live while the designer loop plays.
struct InstrumentDesignerSheet: View {
    @Binding var pattern: SoundDesignPattern
    var playing: Bool
    var onPlay: () -> Void
    var onClose: () -> Void
    private var shape: SoundDesignInstrumentShape { pattern.instrumentShape ?? SoundDesignInstrumentShape() }
    private var stringed: Bool { [.studioPiano, .uprightPiano, .electricGuitar, .acousticGuitar, .pickedBass, .uprightBass, .concertHarp].contains(pattern.instrument) }
    private var sustained: Bool { [.soloCello, .stringEnsemble, .woodenFlute].contains(pattern.instrument) }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                VStack(alignment: .leading, spacing: 3) {
                    Text("Instrument designer · \(pattern.instrument.title)").font(.system(size: 19, weight: .semibold, design: .rounded))
                    Text("Generated from measurements of the recorded \(pattern.instrument.title.lowercased()): its harmonics, decays, attack noise and vibrato. Shape it into your own version.")
                        .font(.system(size: 11)).foregroundStyle(Theme.secondary).fixedSize(horizontal: false, vertical: true)
                }
                Spacer()
            }
            Picker("Sound", selection: Binding(get: { pattern.instrumentShape != nil }, set: { designed in pattern.instrumentShape = designed ? (pattern.instrumentShape ?? SoundDesignInstrumentShape()) : nil })) {
                Text("Recording").tag(false); Text("Designed").tag(true)
            }.pickerStyle(.segmented).frame(width: 260)
            Group {
                section("Tone") {
                    slider("Brightness", \.brightness, -1...1, percent, help: "Darker or brighter upper harmonics")
                    slider("Body", \.body, -18...12, db, help: "Low harmonics below 600 Hz: the instrument's body")
                    slider("Detune", \.detune, 0...25, cents, help: "Spreads the harmonics slightly for a richer, chorused tone")
                }
                section("Attack & decay") {
                    slider(sustained ? "Bow / breath noise" : "Attack noise", \.noise, -36...12, db, help: "Hammer, pick, bow or breath noise")
                    slider("Softness", \.softness, 0...1, percent, help: "Softer, slower attack")
                    slider(sustained ? "Release" : "Sustain", \.sustain, 0.2...4, times, help: "How long notes ring and release")
                }
                section("Pitch") {
                    if stringed { slider("String stiffness", \.stiffness, 0...4, times, help: "Inharmonicity: how far the upper harmonics stretch sharp, like a piano or steel string") }
                    slider("Vibrato", \.vibrato, 0...60, cents, help: "Extra vibrato depth")
                    slider("Vibrato rate", \.vibratoRate, 2...9, hertz, help: "Vibrato speed").disabled(shape.vibrato == 0).opacity(shape.vibrato == 0 ? 0.45 : 1)
                }
            }.disabled(pattern.instrumentShape == nil).opacity(pattern.instrumentShape == nil ? 0.5 : 1)
            Spacer(minLength: 0)
            HStack {
                Button(action: onPlay) { Label(playing ? "Stop" : "Play loop", systemImage: playing ? "stop.fill" : "play.fill") }
                Button("Reset") { pattern.instrumentShape = SoundDesignInstrumentShape() }.disabled(pattern.instrumentShape == nil || pattern.instrumentShape == SoundDesignInstrumentShape())
                Spacer()
                Button("Done", action: onClose).buttonStyle(RoomButtonStyle(prominent: true))
            }.buttonStyle(RoomButtonStyle())
        }
        .padding(20).frame(width: 640, height: 560)
        .foregroundStyle(Theme.ink).background(Theme.paper).tint(Theme.accent).preferredColorScheme(.dark)
    }
    private func section<Content: View>(_ title: String, @ViewBuilder _ content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).font(.system(size: 12, weight: .semibold)).foregroundStyle(Theme.secondary)
            HStack(alignment: .top, spacing: 18) { content() }
        }
    }
    private func slider(_ title: String, _ key: WritableKeyPath<SoundDesignInstrumentShape, Double>, _ range: ClosedRange<Double>, _ format: (Double) -> String, help: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack { Text(title); Spacer(); Text(format(shape[keyPath: key])).foregroundStyle(Theme.secondary).monospacedDigit() }.font(.system(size: 11))
            RoomSlider(value: Binding(get: { shape[keyPath: key] }, set: { value in
                var next = pattern.instrumentShape ?? SoundDesignInstrumentShape()
                next[keyPath: key] = min(range.upperBound, max(range.lowerBound, value))
                pattern.instrumentShape = next
            }), in: range, label: title)
        }.frame(width: 180).help(help)
    }
    private func percent(_ v: Double) -> String { v == 0 ? "as recorded" : String(format: "%+.0f%%", v * 100) }
    private func db(_ v: Double) -> String { v == 0 ? "0 dB" : String(format: "%+.1f dB", v) }
    private func times(_ v: Double) -> String { String(format: "×%.2f", v) }
    private func cents(_ v: Double) -> String { String(format: "%.0f cents", v) }
    private func hertz(_ v: Double) -> String { String(format: "%.1f Hz", v) }
}
