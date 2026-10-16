import SwiftUI
import Combine
import SoundroomCore

enum EqualizerBand: String, CaseIterable, Identifiable {
    case highPass, low, mid, band4, band5, band6, high, lowPass
    var id: String { rawValue }
    var name: String { switch self { case .highPass: return "HP"; case .low: return "Low"; case .mid: return "Mid"; case .high: return "High"; case .lowPass: return "LP"; case .band4: return "4"; case .band5: return "5"; case .band6: return "6" } }
    var groupID: String { self == .highPass || self == .lowPass ? "cuts" : rawValue }
    var frequencyKey: String { rawValue + "Frequency" }
    var gainKey: String? { self == .highPass || self == .lowPass ? nil : rawValue + "Gain" }
    var parameterIDs: [String] { [rawValue + "Enabled", frequencyKey] }
    func value(_ key: String, in values: [String: Float]) -> Float {
        guard let parameter = RealtimeEffectKind.equalizer.parameters.first(where: { $0.id == key }) else { return 0 }
        return parameter.clamped(values[key] ?? parameter.defaultValue)
    }
    func point(values: [String: Float], size: CGSize) -> CGPoint {
        CGPoint(x: log(Double(value(frequencyKey, in: values)) / 20) / log(1000) * size.width,
                y: size.height * (0.5 - Double(gainKey.map { value($0, in: values) } ?? 0) / 48))
    }
    func edits(at point: CGPoint, size: CGSize) -> [String: Float] {
        guard size.width > 0, size.height > 0 else { return [:] }
        let frequency = Float(20 * pow(1000, max(0, min(1, point.x / size.width))))
        let descriptor = RealtimeEffectKind.equalizer.parameters.first { $0.id == frequencyKey }!
        var result = [frequencyKey: descriptor.clamped(frequency)]
        if let gainKey { result[gainKey] = Float(max(-18, min(18, (0.5 - point.y / size.height) * 48))) }
        return result
    }
}

struct EqualizerResponseView: View {
    let values: [String: Float]
    var selectedBand: EqualizerBand = .mid
    var onSelect: (EqualizerBand) -> Void = { _ in }
    var onChange: (([String: Float]) -> Void)? = nil
    var onEditingChanged: (Bool) -> Void = { _ in }
    @State private var dragging: EqualizerBand?
    var body: some View {
        VStack(spacing: 5) {
            GeometryReader { geometry in
                Canvas { context, size in
                    let x: (Double) -> Double = { log($0 / 20) / log(1000) * size.width }
                    let y: (Double) -> Double = { size.height * (0.5 - max(-24, min(24, $0)) / 48) }
                    for db in [-24.0, -12, 0, 12, 24] {
                        var line = Path(); line.move(to: CGPoint(x: 0, y: y(db))); line.addLine(to: CGPoint(x: size.width, y: y(db)))
                        context.stroke(line, with: .color(Theme.line.opacity(db == 0 ? 1 : 0.5)), lineWidth: 1)
                    }
                    for hz in [100.0, 1000, 10000] {
                        var line = Path(); line.move(to: CGPoint(x: x(hz), y: 0)); line.addLine(to: CGPoint(x: x(hz), y: size.height))
                        context.stroke(line, with: .color(Theme.line.opacity(0.5)), lineWidth: 1)
                    }
                    let response = EQResponse.Curve(parameters: values)
                    var curve = Path()
                    for point in 0...240 {
                        let hz = 20 * pow(1000, Double(point) / 240)
                        let position = CGPoint(x: x(hz), y: y(response.decibels(at: hz)))
                        if point == 0 { curve.move(to: position) } else { curve.addLine(to: position) }
                    }
                    context.stroke(curve, with: .color(Theme.accent), lineWidth: 2)
                    if onChange != nil {
                        for (index, band) in EqualizerBand.allCases.enumerated() {
                            let point = band.point(values: values, size: size), radius: Double = band == selectedBand ? 7 : 5
                            let rect = CGRect(x: point.x - radius, y: point.y - radius, width: radius * 2, height: radius * 2)
                            let enabled = band.gainKey != nil || band.value(band.rawValue + "Enabled", in: values) >= 0.5
                            context.fill(Path(ellipseIn: rect), with: .color(enabled ? Theme.accent : Theme.raised))
                            context.stroke(Path(ellipseIn: rect), with: .color(band == selectedBand ? Theme.ink : Theme.secondary), lineWidth: 1.5)
                            context.draw(Text(band.name).font(.system(size: 9, weight: .medium)).foregroundColor(Theme.secondary),
                                         at: CGPoint(x: max(12, min(size.width - 12, point.x)), y: min(size.height-8, point.y + (index.isMultiple(of:2) ? 27 : 14))))
                        }
                    }
                }.contentShape(Rectangle()).gesture(DragGesture(minimumDistance: 0)
                    .onChanged { gesture in
                        guard let onChange else { return }
                        if dragging == nil {
                            let nearest = EqualizerBand.allCases.min { a, b in
                                distance(a.point(values: values, size: geometry.size), gesture.startLocation) < distance(b.point(values: values, size: geometry.size), gesture.startLocation)
                            }!
                            guard distance(nearest.point(values: values, size: geometry.size), gesture.startLocation) <= 26 else { return }
                            onSelect(nearest)
                            guard distance(gesture.location, gesture.startLocation) >= 3 else { return }
                            dragging = nearest
                            EffectWorkspaceFocus.finishNumberEntry {
                                onEditingChanged(true); onChange(nearest.edits(at: gesture.location, size: geometry.size))
                            }
                            return
                        }
                        if let dragging { onChange(dragging.edits(at: gesture.location, size: geometry.size)) }
                    }.onEnded { _ in if dragging != nil { onEditingChanged(false) }; dragging = nil })
            }.frame(height: onChange == nil ? 110 : 120).padding(.horizontal, 10)
                .background(Theme.raised, in: RoundedRectangle(cornerRadius: 5))
                .overlay(alignment: .topLeading) { Text("+24 dB").padding(3) }
                .overlay(alignment: .bottomLeading) { Text("−24 dB").padding(3) }
            HStack { Text("20 Hz"); Spacer(); Text("1 kHz"); Spacer(); Text("20 kHz") }
            if onChange != nil { Text("Drag a point · frequency ↔ gain ↕").frame(maxWidth: .infinity, alignment: .leading) }
        }.font(.system(size: 9)).foregroundStyle(Theme.secondary)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Equalizer setting response, 20 Hz to 20 kHz, plus or minus 24 decibels. Use band buttons and numeric fields for precise editing.")
            .help("Static response of current settings, including Output. Not a live spectrum. HP and LP points move horizontally; turn their filters on in the band controls.")
            .onDisappear { if dragging != nil { onEditingChanged(false) }; dragging = nil }
    }
    private func distance(_ a: CGPoint, _ b: CGPoint) -> Double { hypot(a.x - b.x, a.y - b.y) }
}

struct CompressorMeterView: View {
    let snapshot: () -> EffectMeterSnapshot?
    @State private var reading = EffectMeterSnapshot()
    private let timer = Timer.publish(every: 0.08, on: .main, in: .common).autoconnect()
    var body: some View {
        VStack(spacing: 5) {
            meter("In", reading.inputPeak, rms: reading.inputRMS)
            meter("Out", reading.outputPeak, rms: reading.outputRMS)
            HStack {
                Text("Reduction").frame(width: 50, alignment: .leading)
                GeometryReader { size in
                    ZStack(alignment: .leading) {
                        Capsule().fill(Theme.line)
                        Capsule().fill(Theme.accent).frame(width: size.size.width * CGFloat(min(1, reading.gainReductionDB / 24)))
                    }
                }.frame(height: 5)
                Text(String(format: "%.1f dB", reading.gainReductionDB)).monospacedDigit().frame(width: 55, alignment: .trailing)
            }
        }.font(.system(size: 9)).foregroundStyle(Theme.secondary)
            .help("Solid bar: RMS. Lighter bar and value: peak. Reduction excludes makeup gain.")
            .onReceive(timer) { _ in reading = snapshot() ?? EffectMeterSnapshot() }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(String(format: "Compressor input %.1f dBFS, output %.1f dBFS, reduction %.1f dB", db(reading.inputPeak), db(reading.outputPeak), reading.gainReductionDB))
    }
    private func db(_ value: Float) -> Float { max(-60, 20 * log10(max(0.001, value))) }
    private func meter(_ label: String, _ peak: Float, rms: Float) -> some View {
        HStack {
            Text(label).frame(width: 50, alignment: .leading)
            GeometryReader { size in
                ZStack(alignment: .leading) {
                    Capsule().fill(Theme.line)
                    Capsule().fill(Theme.accent.opacity(0.45)).frame(width: size.size.width * CGFloat(min(1, max(0, (db(peak) + 60) / 60))))
                    Capsule().fill(Theme.accent).frame(width: size.size.width * CGFloat(min(1, max(0, (db(rms) + 60) / 60))))
                }
            }.frame(height: 5)
            Text(peak < 0.001 ? "−∞ dBFS" : String(format: "%.1f dBFS", db(peak))).monospacedDigit().frame(width: 55, alignment: .trailing)
        }
    }
}
