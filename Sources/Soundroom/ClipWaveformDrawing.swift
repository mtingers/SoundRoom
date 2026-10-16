import SwiftUI

enum AudioChannelPresentation {
    static func label(channels: Int?) -> String? {
        guard let channels, channels > 0 else { return nil }
        switch channels { case 1: return "Mono"; case 2: return "Stereo"; default: return "\(channels) ch" }
    }
}

/// Keeps the two channels inside the existing clip and editing hit regions.
struct WaveformChannelGeometry {
    let top: Double
    let bottom: Double
    let channels: Int
    var channelHeight: Double { max(0, bottom - top) / Double(max(1, channels)) }
    var amplitude: Double { max(1, channelHeight / 2 - 4) }
    func center(_ channel: Int) -> Double { top + (Double(channel) + 0.5) * channelHeight }
}

enum ClipWaveformDrawing {
    static func draw(context: GraphicsContext, envelope: WaveformEnvelope,
                     source: Range<Int>, rect: CGRect, pixels: Range<Int>, color: Color, labelX: Double) {
        guard rect.width > 0, rect.height > 0, !source.isEmpty, !pixels.isEmpty else { return }
        let channels = envelope.channelValues.isEmpty ? [envelope.values] : envelope.channelValues
        let geometry = WaveformChannelGeometry(top: rect.minY, bottom: rect.maxY, channels: channels.count)
        let framesPerPixel = Double(source.count) / rect.width
        var drawing = context
        drawing.clip(to: Path(rect))
        for (channel, peaks) in channels.enumerated() {
            var waveform = Path()
            for pixel in pixels {
                let sourceFirst = source.lowerBound + Int(Double(pixel) * framesPerPixel)
                let sourceLast = source.lowerBound + min(source.count - 1, Int(Double(pixel + 1) * framesPerPixel))
                guard sourceLast >= envelope.firstFrame else { continue }
                let first = max(0, (sourceFirst - envelope.firstFrame) / envelope.framesPerBin)
                let last = min(peaks.count - 1, (sourceLast - envelope.firstFrame) / envelope.framesPerBin)
                guard first <= last, first < peaks.count else { continue }
                var low: Float = 0, high: Float = 0
                for bin in first...last { low = min(low, peaks[bin].0); high = max(high, peaks[bin].1) }
                let x = rect.minX + Double(pixel) + 0.5
                waveform.move(to: CGPoint(x: x, y: geometry.center(channel) - Double(min(1, high)) * geometry.amplitude))
                waveform.addLine(to: CGPoint(x: x, y: geometry.center(channel) - Double(max(-1, low)) * geometry.amplitude + 0.6))
            }
            drawing.stroke(waveform, with: .color(color), lineWidth: 1)
        }
        if channels.count == 2 {
            let middle = (rect.minY + rect.maxY) / 2
            var separator = Path()
            separator.move(to: CGPoint(x: rect.minX, y: middle)); separator.addLine(to: CGPoint(x: rect.maxX, y: middle))
            drawing.stroke(separator, with: .color(color.opacity(0.25)), lineWidth: 0.5)
            if rect.maxX - labelX >= 18 {
                for channel in 0..<2 {
                    let label = CGRect(x: labelX - 2, y: geometry.center(channel) - 7, width: 15, height: 14)
                    drawing.fill(Path(roundedRect: label, cornerRadius: 3), with: .color(Theme.background.opacity(0.9)))
                    drawing.draw(Text(channel == 0 ? "L" : "R").font(.system(size: 9, weight: .semibold)).foregroundStyle(color),
                                 at: CGPoint(x: label.midX, y: label.midY))
                }
            }
        }
    }
}
