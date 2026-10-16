import SwiftUI

struct DesignerEnvelopePreview: View {
    let attack: Double, decay: Double, sustain: Double, release: Double
    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Amplitude envelope").font(.system(size: 10)).foregroundStyle(Theme.secondary)
            Canvas { context, size in
                let held = 0.35, total = max(0.001, attack + decay + held + release)
                func point(_ time: Double, _ level: Double) -> CGPoint { CGPoint(x: 3 + time / total * (size.width - 6), y: 4 + (1 - level) * (size.height - 8)) }
                var line = Path(); line.move(to: point(0, 0)); line.addLine(to: point(attack, 1))
                line.addLine(to: point(attack + decay, sustain)); line.addLine(to: point(attack + decay + held, sustain)); line.addLine(to: point(total, 0))
                var area = line; area.closeSubpath()
                context.fill(area, with: .color(Theme.accent.opacity(0.1)))
                context.stroke(line, with: .color(Theme.accent), lineWidth: 1.5)
                for time in [attack, attack + decay, attack + decay + held] {
                    let x = point(time, 0).x
                    var guide = Path(); guide.move(to: CGPoint(x: x, y: 0)); guide.addLine(to: CGPoint(x: x, y: size.height))
                    context.stroke(guide, with: .color(Theme.secondary.opacity(0.2)), style: StrokeStyle(lineWidth: 0.5, dash: [2, 3]))
                }
            }.frame(height: 44).background(Theme.paper, in: RoundedRectangle(cornerRadius: 4))
                .accessibilityLabel("Amplitude envelope").accessibilityValue("Attack \(Int(attack * 1000)) milliseconds, decay \(Int(decay * 1000)) milliseconds, sustain \(Int(sustain * 100)) percent, release \(Int(release * 1000)) milliseconds")
        }.help("ADSR shape with an illustrative 350 ms held section. Instrument resonance can also decay within the envelope.")
    }
}
