import AppKit

let directory = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
var chunks = Data()
func bigEndian(_ value: UInt32) -> Data {
    Data([UInt8((value >> 24) & 255), UInt8((value >> 16) & 255), UInt8((value >> 8) & 255), UInt8(value & 255)])
}
let types = [
    16: ["icp4", "ic11"], 32: ["icp5", "ic12"], 128: ["ic07", "ic13"], 256: ["ic08", "ic14"], 512: ["ic09", "ic10"],
]
for size in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let pixels = size * scale
        let rep = NSBitmapImageRep(
            bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels, bitsPerSample: 8, samplesPerPixel: 4,
            hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        let s = CGFloat(pixels)
        NSColor(calibratedRed: 0.96, green: 0.95, blue: 0.91, alpha: 1).setFill()
        NSBezierPath(
            roundedRect: NSRect(x: s * 0.05, y: s * 0.05, width: s * 0.9, height: s * 0.9), xRadius: s * 0.21,
            yRadius: s * 0.21
        ).fill()
        NSColor(calibratedRed: 0.19, green: 0.39, blue: 0.32, alpha: 1).setFill()
        let heights: [CGFloat] = [0.16, 0.35, 0.55, 0.40, 0.25]
        for (i, height) in heights.enumerated() {
            NSBezierPath(
                roundedRect: NSRect(
                    x: s * (0.245 + CGFloat(i) * 0.11), y: s * (0.5 - height / 2), width: s * 0.07, height: s * height),
                xRadius: s * 0.035, yRadius: s * 0.035
            ).fill()
        }
        NSGraphicsContext.restoreGraphicsState()
        let suffix = scale == 2 ? "@2x" : ""
        let png = rep.representation(using: .png, properties: [:])!
        try png.write(to: directory.appendingPathComponent("icon_\(size)x\(size)\(suffix).png"))
        chunks.append(contentsOf: types[size]![scale - 1].utf8)
        chunks.append(bigEndian(UInt32(png.count + 8)))
        chunks.append(png)
    }
}
var icon = Data("icns".utf8)
icon.append(bigEndian(UInt32(chunks.count + 8)))
icon.append(chunks)
try icon.write(to: URL(fileURLWithPath: CommandLine.arguments[2]))
