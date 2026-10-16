import SwiftUI
import SoundroomCore

/// Read-only setting diagrams. These intentionally do not animate or claim to meter audio.
struct EffectSettingVisualization: View {
    let kind: RealtimeEffectKind
    let values: [String: Float]
    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            switch kind {
            case .lowPass, .highPass:
                title("Setting response")
                SettingResponsePlot(kind: kind, values: values)
            case .equalizer:
                title("Setting response")
                EqualizerResponseView(values: values)
            case .compressor:
                title("Static transfer · output dBFS")
                CompressorSettingPlot(values: values)
                    .help("Stereo-linked peak compression at steady level, including Makeup and Mix. Attack and Release affect transitions, not this curve. This is not live gain reduction.")
            case .echo:
                title("Nominal repeats")
                EchoSettingPlot(values: values)
                    .help("Repeat levels before feedback-loop tone attenuation. The first wet repeat is undamped; subsequent repeats are frequency dependent.")
            case .chorus:
                title("Delay sweep · one cycle")
                ChorusSettingPlot(values: values)
            case .limiter:
                title("Output protection")
                readout("Look-ahead", "10.67 ms · stereo linked")
                readout("Ceiling", String(format: "%.1f dBTP", value("ceiling")))
            case .resonance:
                title("Spectral region · stereo linked")
                readout("Range", String(format: "%.0f–%.0f Hz", value("low"), value("high")))
                readout("Look-ahead", "42.67 ms · four-way overlap")
            case .vocalEnhancer:
                title("Vocal restoration & character")
                readout("Signal path", "Cleanup → de-ess → level → tone → peak protection")
                readout("Latency", "3 ms · stereo linked")
            case .sourceIsolator:
                title("Component focus · stereo linked")
                readout("Range", String(format: "%.0f–%.0f Hz", value("low"), value("high")))
                readout("Latency", "42.67 ms · four-way overlap")
            case .gain:
                readout("Amplitude", String(format: "×%.3f", EffectSettingMath.gainAmplitude(decibels: value("gain"))))
            case .room:
                title("Nominal decay envelope")
                ReverbSettingPlot(values: values)
                    .help("A setting diagram, not a measured impulse response. Damping shortens high-frequency decay; early reflections and diffusion shape the onset.")
                readout("Nominal decay", String(format: "%.2f s to −60 dB", value("decay")))
            case .warmth:
                title("Saturation settings")
                readout("Drive", String(format: "%.2f dB", value("drive")))
                readout("Level match", value("match") >= 0.5 ? "Dynamic · on" : "Off")
                readout("Output", String(format: "%+.2f dB", value("output")))
            case .dither:
                title("Resolution settings")
                let step = EffectSettingMath.quantizationStep(bits: value("bits"))
                readout("One step", String(format: "%.3g full scale", step))
                readout("TPDF RMS", String(format: "%.3g full scale", step * value("amount") / sqrt(6)))
                    .help("Theoretical TPDF noise RMS before optional quantization; this is not a measured noise floor.")
                readout("Rounding", value("quantize") >= 0.5 ? "On" : "Off")
                    .help("Theoretical TPDF noise RMS before optional quantization; this is not a measured noise floor.")
            case .gate:
                readout("Detection", "Stereo linked · hysteresis and hold")
                readout("Range", String(format: "%.1f dB maximum reduction", value("range")))
            case .utility:
                readout("Alignment", "Intentional delay · uncompensated")
                readout("Mono", value("mono") >= 0.5 ? "On" : "Off")
            case .soundPutty, .pitchCorrection, .guitarRig: EmptyView()
            }
        }.padding(10).background(Theme.card, in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).stroke(Theme.line))
    }
    private func value(_ key: String) -> Double { EffectSettingMath.value(key, kind: kind, values: values) }
    private func title(_ text: String) -> some View {
        Text(text).font(.system(size: 10, weight: .medium)).foregroundStyle(Theme.secondary)
    }
    private func readout(_ name: String, _ text: String) -> some View {
        HStack { Text(name).foregroundStyle(Theme.secondary); Spacer(minLength: 8); Text(text).monospacedDigit() }
            .font(.system(size: 11)).accessibilityElement(children: .combine)
    }
}

private struct ReverbSettingPlot: View {
    let values: [String: Float]
    var body: some View {
        let decay = EffectSettingMath.value("decay", kind: .room, values: values)
        let pre = EffectSettingMath.value("predelay", kind: .room, values: values) / 1000
        let end = pre + decay + 0.15
        VStack(spacing: 3) {
            Canvas { context, size in
                let area = CGRect(x: 25, y: 4, width: max(1, size.width - 29), height: max(1, size.height - 8))
                SettingPlotDrawing.grid(context: &context, area: area, levels: [-60, -30, 0], lower: -60, upper: 0)
                var envelope = Path()
                envelope.move(to: CGPoint(x: area.minX + pre / end * area.width, y: area.minY))
                envelope.addLine(to: CGPoint(x: area.minX + (pre + decay) / end * area.width, y: area.maxY))
                context.stroke(envelope, with: .color(Theme.accent), style: StrokeStyle(lineWidth: 2))
                var delay = Path()
                let x = area.minX + pre / end * area.width
                delay.move(to: CGPoint(x: x, y: area.minY)); delay.addLine(to: CGPoint(x: x, y: area.maxY))
                context.stroke(delay, with: .color(Theme.secondary), style: StrokeStyle(lineWidth: 1, dash: [3, 3]))
            }.frame(height: 60)
            HStack { Text("0 s"); Spacer(); Text(String(format: "Pre-delay %.1f ms", pre * 1000)); Spacer(); Text(String(format: "%.2f s", end)) }
                .font(.system(size: 9)).foregroundStyle(Theme.secondary).padding(.leading, 25)
        }.accessibilityElement(children: .ignore)
            .accessibilityLabel(String(format: "Nominal reverb envelope: %.2f milliseconds pre-delay, %.2f seconds to minus 60 decibels. Not a measured response.", pre * 1000, decay))
    }
}

private struct SettingResponsePlot: View {
    let kind: RealtimeEffectKind
    let values: [String: Float]
    var body: some View {
        VStack(spacing: 3) {
            Canvas { context, size in
                let area = CGRect(x: 25, y: 3, width: max(1, size.width - 29), height: max(1, size.height - 8))
                SettingPlotDrawing.grid(context: &context, area: area, levels: [-48, -24, 0, 24], lower: -48, upper: 24)
                var response = Path()
                for index in 0...180 {
                    let t = Double(index) / 180, frequency = 20 * pow(1000, t)
                    let db = FilterResponse.decibels(at: frequency, kind: kind, parameters: values)
                    let point = CGPoint(x: area.minX + t * area.width,
                                        y: SettingPlotDrawing.y(db, area: area, lower: -48, upper: 24))
                    if index == 0 { response.move(to: point) } else { response.addLine(to: point) }
                }
                context.stroke(response, with: .color(Theme.accent), style: StrokeStyle(lineWidth: 1.8))
            }.frame(height: 98)
            HStack { Text("20 Hz"); Spacer(); Text("20 kHz") }
                .font(.system(size: 9)).foregroundStyle(Theme.secondary).padding(.leading, 25)
        }.accessibilityElement(children: .ignore)
            .accessibilityLabel("Static \(kind.name) magnitude response. \(EffectControlPresentation.summary(kind: kind, values: values))")
    }
}

private struct CompressorSettingPlot: View {
    let values: [String: Float]
    var body: some View {
        VStack(spacing: 3) {
            Canvas { context, size in
                let area = CGRect(x: 25, y: 4, width: max(1, size.width - 30), height: max(1, size.height - 9))
                SettingPlotDrawing.grid(context: &context, area: area, levels: [-60, -30, 0, 24], lower: -60, upper: 24)
                let model = EffectSettingMath.Compressor(values: values)
                var reference = Path(), curve = Path()
                for index in 0...120 {
                    let input = -60 + Double(index) * 0.5
                    let x = area.minX + Double(index) / 120 * area.width
                    let dry = CGPoint(x: x, y: SettingPlotDrawing.y(input, area: area, lower: -60, upper: 24))
                    let wet = CGPoint(x: x, y: SettingPlotDrawing.y(model.outputDB(inputDB: input), area: area, lower: -60, upper: 24))
                    if index == 0 { reference.move(to: dry); curve.move(to: wet) }
                    else { reference.addLine(to: dry); curve.addLine(to: wet) }
                }
                context.stroke(reference, with: .color(Theme.secondary.opacity(0.5)), style: StrokeStyle(lineWidth: 1, dash: [3, 3]))
                context.stroke(curve, with: .color(Theme.accent), style: StrokeStyle(lineWidth: 1.8))
            }.frame(height: 110)
            HStack { Text("−60"); Spacer(); Text("Input dBFS"); Spacer(); Text("0") }
                .font(.system(size: 9)).foregroundStyle(Theme.secondary).padding(.leading, 25)
        }.accessibilityElement(children: .ignore)
            .accessibilityLabel("Static compressor input to output curve, including Makeup and Mix. \(EffectControlPresentation.summary(kind: .compressor, values: values))")
    }
}

private struct EchoSettingPlot: View {
    let values: [String: Float]
    var body: some View {
        let repeats = EffectSettingMath.echoRepeats(values: values)
        VStack(spacing: 3) {
            Canvas { context, size in
                let area = CGRect(x: 25, y: 4, width: max(1, size.width - 29), height: max(1, size.height - 8))
                SettingPlotDrawing.grid(context: &context, area: area, levels: [-60, -30, 0], lower: -60, upper: 0)
                for (index, repeatValue) in repeats.enumerated() {
                    let db = 20 * log10(max(0.001, repeatValue.amplitude))
                    let x = area.minX + Double(index + 1) / Double(repeats.count + 1) * area.width
                    var line = Path(); line.move(to: CGPoint(x: x, y: area.maxY))
                    line.addLine(to: CGPoint(x: x, y: SettingPlotDrawing.y(db, area: area, lower: -60, upper: 0)))
                    context.stroke(line, with: .color(Theme.accent.opacity(0.95 - Double(index) * 0.08)), style: StrokeStyle(lineWidth: 3, lineCap: .round))
                }
            }.frame(height: 80)
            HStack {
                Text("First: \(Int(repeats[0].milliseconds.rounded())) ms"); Spacer()
                Text("Last shown: \(String(format: "%.2f", repeats.last!.milliseconds / 1000)) s")
            }.font(.system(size: 9)).foregroundStyle(Theme.secondary)
        }.accessibilityElement(children: .ignore)
            .accessibilityLabel("Nominal echo repeat levels before tone attenuation. \(EffectControlPresentation.summary(kind: .echo, values: values))")
    }
}

private struct ChorusSettingPlot: View {
    let values: [String: Float]
    var body: some View {
        let base = EffectSettingMath.value("delay", kind: .chorus, values: values)
        let depth = EffectSettingMath.value("depth", kind: .chorus, values: values)
        let rate = EffectSettingMath.value("rate", kind: .chorus, values: values)
        VStack(spacing: 3) {
            Canvas { context, size in
                let area = CGRect(x: 30, y: 4, width: max(1, size.width - 34), height: max(1, size.height - 8))
                let lower = base - 1, upper = base + max(1, depth) + 1
                SettingPlotDrawing.grid(context: &context, area: area, levels: [base, base + depth], lower: lower, upper: upper, suffix: "")
                for right in [false, true] {
                    var curve = Path()
                    for index in 0...120 {
                        let t = Double(index) / 120
                        let delay = EffectSettingMath.chorusDelay(phase: t * 2 * .pi, right: right, values: values)
                        let point = CGPoint(x: area.minX + t * area.width, y: SettingPlotDrawing.y(delay, area: area, lower: lower, upper: upper))
                        if index == 0 { curve.move(to: point) } else { curve.addLine(to: point) }
                    }
                    context.stroke(curve, with: .color(right ? Theme.tracks[1] : Theme.accent), style: StrokeStyle(lineWidth: 1.6, dash: right ? [4, 2] : []))
                }
            }.frame(height: 80)
            HStack { Text("L solid / R dashed · ms"); Spacer(); Text(String(format: "%.2f s", 1 / rate)) }
                .font(.system(size: 9)).foregroundStyle(Theme.secondary)
        }.accessibilityElement(children: .ignore)
            .accessibilityLabel("Static left and right delay sweeps over one chorus cycle. \(EffectControlPresentation.summary(kind: .chorus, values: values))")
    }
}

private enum SettingPlotDrawing {
    static func y(_ value: Double, area: CGRect, lower: Double, upper: Double) -> Double {
        area.maxY - max(0, min(1, (value - lower) / (upper - lower))) * area.height
    }
    static func grid(context: inout GraphicsContext, area: CGRect, levels: [Double], lower: Double, upper: Double, suffix: String = "") {
        for level in levels {
            let position = y(level, area: area, lower: lower, upper: upper)
            var line = Path(); line.move(to: CGPoint(x: area.minX, y: position)); line.addLine(to: CGPoint(x: area.maxX, y: position))
            context.stroke(line, with: .color(Theme.line.opacity(0.6)), lineWidth: 0.5)
            context.draw(Text(String(format: "%.0f", level) + suffix).font(.system(size: 8)).foregroundColor(Theme.secondary),
                         at: CGPoint(x: area.minX - 4, y: position), anchor: .trailing)
        }
    }
}
