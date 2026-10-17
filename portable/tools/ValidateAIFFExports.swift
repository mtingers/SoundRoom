import AudioToolbox
// Only Apple AudioFile file APIs. No devices, AudioUnits or audio output.
import Foundation

let root = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
func checked(_ status: OSStatus) throws {
    if status != noErr { throw NSError(domain: "SoundroomAIFFExport", code: Int(status)) }
}
for (name, bits, isFloat) in [("pcm16", 16, false), ("pcm24", 24, false), ("float32", 32, true)] {
    var file: AudioFileID?
    try checked(AudioFileOpenURL(root.appendingPathComponent(name + ".aiff") as CFURL, .readPermission, 0, &file))
    defer { AudioFileClose(file!) }
    var description = AudioStreamBasicDescription()
    var size = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
    try checked(AudioFileGetProperty(file!, kAudioFilePropertyDataFormat, &size, &description))
    precondition(
        description.mSampleRate == 48000 && description.mChannelsPerFrame == 2 && description.mBitsPerChannel == bits)
    precondition(description.mFormatFlags & kAudioFormatFlagIsBigEndian != 0)
    precondition((description.mFormatFlags & kAudioFormatFlagIsFloat != 0) == isFloat)
    var count = UInt32(8 * 2 * bits / 8)
    var bytes = Array(repeating: UInt8(0), count: Int(count))
    try bytes.withUnsafeMutableBytes { try checked(AudioFileReadBytes(file!, false, 0, &count, $0.baseAddress!)) }
    let original: [Double] = [-2, -1, -0.5, 0, 0.125, 1, 2, 0]
    for frame in 0..<8 {
        let offset = frame * 2 * bits / 8
        let word = bytes[offset..<offset + bits / 8].reduce(UInt32(0)) { ($0 << 8) | UInt32($1) }
        let sample: Float
        if isFloat {
            sample = Float(bitPattern: word)
        } else {
            let signed = Int32(bitPattern: word << (32 - bits))
            sample = Float(signed) / 2147483648
        }
        let scale = pow(2, Double(bits - 1))
        let expected =
            isFloat
            ? Float(original[frame]) : Float(min(scale - 1, max(-scale, (original[frame] * scale).rounded())) / scale)
        precondition(sample == expected, "Native reader sample mismatch")
    }
    print(
        "PASS Apple AudioFile reads Rust \(name): rate48000 channels2 bits\(bits), all eight left samples match format limits/headroom"
    )
}
