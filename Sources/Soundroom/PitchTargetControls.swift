import SwiftUI

/// Absolute pitch classes: changing Key never silently transposes a saved custom mask.
struct PitchTargetControls: View {
    let mask: Int
    let onChange: (Int) -> Void
    private let names = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"]
    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack {
                Text("Target notes")
                Spacer()
                Button("All") { onChange(4095) }.disabled(mask == 4095)
                Button("None") { onChange(0) }.disabled(mask == 0)
            }.font(.system(size: 10)).buttonStyle(.borderless)
            LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 3), count: 6), spacing: 3) {
                ForEach(names.indices, id: \.self) { note in
                    let enabled = mask & (1 << note) != 0
                    Button { onChange(mask ^ (1 << note)) } label: {
                        Text(names[note]).font(.system(size: 11, weight: .semibold))
                            .frame(maxWidth: .infinity, minHeight: 24)
                            .background(enabled ? Color.accentColor.opacity(0.85) : Color.white.opacity(0.06), in: RoundedRectangle(cornerRadius: 4))
                            .foregroundStyle(enabled ? Color.white : Color.secondary)
                    }.buttonStyle(.plain)
                        .accessibilityLabel("Target note \(names[note])")
                        .accessibilityValue(enabled ? "Enabled" : "Disabled")
                        .help("\(enabled ? "Remove" : "Allow") \(names[note]) in every octave")
                }
            }
            if mask == 0 {
                Text("No notes: pitch unchanged").font(.system(size: 10)).foregroundStyle(.secondary)
            }
        }.accessibilityElement(children: .contain).accessibilityLabel("Custom pitch target notes")
    }
}
