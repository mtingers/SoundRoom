import Foundation

public enum WaveFile {
    /// Read float WAV media directly so project audio never passes through a format converter.
    public static func decodeFloat(_ data: Data) -> (samples: AudioSamples, rate: Double)? {
        guard data.count >= 12, String(data: data.prefix(4), encoding: .ascii) == "RIFF",
              String(data: data[8..<12], encoding: .ascii) == "WAVE" else { return nil }
        func u16(_ offset: Int) -> UInt16 { UInt16(data[offset]) | UInt16(data[offset + 1]) << 8 }
        func u32(_ offset: Int) -> UInt32 {
            UInt32(data[offset]) | UInt32(data[offset + 1]) << 8 | UInt32(data[offset + 2]) << 16 | UInt32(data[offset + 3]) << 24
        }
        var channelCount = 0, rate: Double = 0, blockSize = 0, floatFormat = false
        var payload: Range<Int>?
        var cursor = 12
        while cursor <= data.count - 8 {
            let tag = String(data: data[cursor..<(cursor + 4)], encoding: .ascii)
            let size = Int(u32(cursor + 4)), start = cursor + 8
            guard size <= data.count - start else { return nil }
            if tag == "fmt ", size >= 16 {
                let format = u16(start)
                floatFormat = format == 3 || (format == 0xfffe && size >= 40 && u16(start + 24) == 3)
                channelCount = Int(u16(start + 2)); rate = Double(u32(start + 4)); blockSize = Int(u16(start + 12))
                guard u16(start + 14) == 32 else { return nil }
            } else if tag == "data" { payload = start..<(start + size) }
            cursor = start + size + size % 2
        }
        guard floatFormat, channelCount > 0, channelCount <= 64, rate > 0,
              blockSize == channelCount * 4, let payload, payload.count % blockSize == 0 else { return nil }
        let frames = payload.count / blockSize
        var channels = Array(repeating: Array(repeating: Float(0), count: frames), count: channelCount)
        for frame in 0..<frames {
            for channel in 0..<channelCount {
                channels[channel][frame] = Float(bitPattern: u32(payload.lowerBound + frame * blockSize + channel * 4))
            }
        }
        return (AudioSamples(channels: channels), rate)
    }
    public static func encode(_ audio: AudioSamples, float: Bool = false) -> Data {
        let channels = max(1, min(2, audio.channelCount)), bytes = float ? 4 : 3
        let dataSize = audio.count * channels * bytes
        var data = Data(); data.reserveCapacity(44 + dataSize + dataSize % 2)
        func ascii(_ s: String) { data.append(contentsOf: s.utf8) }
        func u16(_ v: UInt16) { data.append(UInt8(v & 255)); data.append(UInt8(v >> 8)) }
        func u32(_ v: UInt32) { for shift in stride(from: 0, through: 24, by: 8) { data.append(UInt8((v >> shift) & 255)) } }
        ascii("RIFF"); u32(UInt32(36 + dataSize)); ascii("WAVEfmt "); u32(16)
        u16(float ? 3 : 1); u16(UInt16(channels)); u32(UInt32(sessionRate))
        u32(UInt32(Int(sessionRate) * channels * bytes)); u16(UInt16(channels * bytes)); u16(UInt16(bytes * 8))
        ascii("data"); u32(UInt32(dataSize))
        for start in stride(from: 0, to: audio.count, by: 8192) {
            let block = audio.readFrames(start..<min(audio.count, start + 8192), channels: Array(0..<channels))
            for i in 0..<block[0].count {
                for c in 0..<channels {
                    let raw = block[c][i], value = raw.isFinite ? raw : 0
                    if float { u32(value.bitPattern) }
                    else {
                        let sample = Int32((max(-1, min(1, value)) * 8_388_607).rounded())
                        let bits = UInt32(bitPattern: sample)
                        for shift in [0, 8, 16] { data.append(UInt8((bits >> shift) & 255)) }
                    }
                }
            }
        }
        if dataSize % 2 != 0 { data.append(0); let size = UInt32(data.count - 8); for i in 0..<4 { data[4 + i] = UInt8((size >> (i * 8)) & 255) } }
        return data
    }
}
