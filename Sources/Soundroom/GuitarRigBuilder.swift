import SwiftUI
import SoundroomCore

/// Each tile opens one part of the saved rig. Order is the signal path;
/// individual types can be bypassed without changing any other section.
struct GuitarRigBuilder: View {
    let values: [String: Float]
    @Binding var selected: String?
    private let blocks = ["input", "drive", "amp", "cab", "mod", "delay", "room", "output"]
    private let titles = ["Input", "Drive", "Amp", "Cabinet / mic", "Modulation", "Delay", "Room", "Output"]
    private let shortTitles = ["Input", "Drive", "Amp", "Cab", "Mod", "Delay", "Room", "Out"]
    private let symbols = ["cable.connector", "bolt.fill", "hifispeaker.fill", "speaker.wave.2.fill", "waveform", "repeat", "building.2", "speaker.wave.3"]
    private let colors: [Color] = [.gray, .orange, .yellow, .orange.opacity(0.8), .purple, .blue, .cyan, .gray]
    private var chosenSection: String {
        let section = selected ?? "amp"
        return blocks.contains(section) ? section : "amp"
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Signal path 1 → 8 · choose a section").font(.system(size: 10)).foregroundStyle(Theme.secondary)
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 64, maximum: 110), spacing: 5)], spacing: 5) {
                ForEach(blocks.indices, id: \.self) { index in
                    let block = blocks[index], active = chosenSection == block
                    Button { selected = block } label: {
                        HStack(spacing: 4) {
                                Image(systemName: symbols[index]).foregroundStyle(colors[index])
                                Text("\(index + 1) \(shortTitles[index])").font(.system(size: 10, weight: .semibold)).lineLimit(1)
                        }.font(.system(size: 10)).padding(.horizontal, 5).frame(maxWidth: .infinity, minHeight: 30)
                            .background(active ? Theme.accent.opacity(0.15) : Theme.card, in: RoundedRectangle(cornerRadius: 7))
                            .overlay(RoundedRectangle(cornerRadius: 7).stroke(active ? Theme.accent : Theme.line, lineWidth: active ? 1.5 : 1))
                    }.buttonStyle(.plain).accessibilityLabel("Edit guitar rig \(titles[index]), \(summary(block))")
                        .help("\(titles[index]): \(summary(block))")
                        .accessibilityAddTraits(active ? .isSelected : [])
                }
            }
        }
    }
    private func summary(_ block: String) -> String {
        let key: String
        switch block {
        case "drive": key = "pedal"
        case "amp": key = "amp"
        case "cab": key = "cabinet"
        case "mod": key = "modulation"
        case "delay": return (values["delayMix"] ?? 0) == 0 ? "Off · mix at zero" : "\(Int(values["delayTime"] ?? 380)) ms echoes"
        case "room": return (values["reverbMix"] ?? 0.08) == 0 ? "Off · mix at zero" : String(format: "%.1f s ambience", values["reverbDecay"] ?? 1.4)
        case "input": return String(format: "%+.1f dB · gate %@", values["input"] ?? 0, (values["gate"] ?? -75) <= -89.9 ? "off" : "on")
        default: return String(format: "%+.1f dB · %.0f%% rig", values["output"] ?? 0, (values["mix"] ?? 1) * 100)
        }
        guard let descriptor = RealtimeEffectKind.guitarRig.parameters.first(where: { $0.id == key }) else { return "" }
        return descriptor.display(values[key] ?? descriptor.defaultValue)
    }
    static func isInactive(_ key: String, values: [String: Float]) -> Bool {
        if ["pedalDrive", "pedalTone", "pedalLevel"].contains(key) { return (values["pedal"] ?? 0) == 0 }
        if ["ampDrive", "bass", "mid", "treble", "presence", "sag", "master"].contains(key) { return (values["amp"] ?? 1) == 0 }
        if ["mic", "lowCut", "highCut"].contains(key) { return (values["cabinet"] ?? 1) == 0 }
        if ["modRate", "modDepth", "modMix"].contains(key) { return (values["modulation"] ?? 0) == 0 }
        return false
    }
}
