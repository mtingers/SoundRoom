import Darwin
import Foundation

/// Read-only mapped project media. The file must be immutable for this lifetime:
/// app import caches and atomic project packages supply that ownership guarantee.
/// PCM pages are pulled by a preparation/read-ahead worker, not the device callback.
final class MappedWaveSamples: @unchecked Sendable {
    let url: URL
    let frameCount: Int
    let channelCount: Int
    let byteCount: Int
    private let mapping: UnsafeMutableRawPointer
    private let payload: Int
    private let bytesPerSample: Int
    private let floating: Bool
    private let deleteOnRelease: Bool
    private let block: Int
    private let descriptor: Int32
    init(url: URL, deleteOnRelease: Bool = false) throws {
        self.deleteOnRelease = deleteOnRelease
        self.url = url
        let fd = url.withUnsafeFileSystemRepresentation { open($0!, O_RDONLY) }
        guard fd >= 0 else { throw CocoaError(.fileReadNoSuchFile) }
        var retained = false
        defer { if !retained { close(fd) } }
        var info = stat()
        guard fstat(fd, &info) == 0, info.st_size >= 44, info.st_size <= Int.max,
            let base = mmap(nil, Int(info.st_size), PROT_READ, MAP_PRIVATE, fd, 0), base != MAP_FAILED
        else { throw CocoaError(.fileReadCorruptFile) }
        let length = Int(info.st_size)
        func u16(_ index: Int) -> UInt16 {
            UInt16(littleEndian: base.loadUnaligned(fromByteOffset: index, as: UInt16.self))
        }
        func u32(_ index: Int) -> UInt32 {
            UInt32(littleEndian: base.loadUnaligned(fromByteOffset: index, as: UInt32.self))
        }
        func tag(_ index: Int) -> UInt32 { u32(index) }
        var offset = 12
        var channels = 0
        var stride = 0
        var bits = 0
        var format = 0
        var dataStart = 0
        var dataLength = 0
        var valid = tag(0) == 0x46464952 && tag(8) == 0x45564157  // RIFF/WAVE
        while valid && offset <= length - 8 {
            let size = Int(u32(offset + 4))
            let start = offset + 8
            guard size <= length - start else {
                valid = false
                break
            }
            if tag(offset) == 0x20746d66 && size >= 16 {
                format = Int(u16(start))
                channels = Int(u16(start + 2))
                stride = Int(u16(start + 12))
                bits = Int(u16(start + 14))
                if format == 0xfffe && size >= 40 { format = Int(u16(start + 24)) }
                if u32(start + 4) != UInt32(sessionRate) {
                    valid = false
                    break
                }
            } else if tag(offset) == 0x61746164 {
                dataStart = start
                dataLength = size
            }
            offset = start + size + size % 2
        }
        let sampleBytes = bits / 8
        guard valid, channels > 0, channels <= 64, dataStart > 0, dataLength > 0,
            (format == 3 && bits == 32 || format == 1 && [16, 24, 32].contains(bits)),
            stride == channels * sampleBytes, dataLength % stride == 0
        else {
            munmap(base, length)
            throw CocoaError(.fileReadCorruptFile)
        }
        mapping = base
        byteCount = length
        payload = dataStart
        channelCount = channels
        frameCount = dataLength / stride
        block = stride
        bytesPerSample = sampleBytes
        floating = format == 3
        descriptor = fd
        retained = true
        madvise(base, length, MADV_RANDOM)
    }
    deinit {
        munmap(mapping, byteCount)
        close(descriptor)
        if deleteOnRelease { try? FileManager.default.removeItem(at: url) }
    }
    /// Worker-only bounded reads avoid Darwin retaining every touched mmap page
    /// during a full-session warm-up. pread is independent across workers.
    func readFrames(_ range: Range<Int>, channels: [Int]) -> [[Float]] {
        var output = channels.map { _ in Array(repeating: Float(0), count: range.count) }
        guard !range.isEmpty else { return output }
        let lo = max(0, range.lowerBound)
        let hi = min(frameCount, range.upperBound)
        guard hi > lo else { return output }
        var bytes = Data(count: (hi - lo) * block)
        bytes.withUnsafeMutableBytes { raw in
            var read = 0
            while read < raw.count {
                let value = pread(
                    descriptor, raw.baseAddress! + read, raw.count - read, off_t(payload + lo * block + read))
                if value < 0 && errno == EINTR { continue }
                if value <= 0 { break }
                read += value
            }
        }
        bytes.withUnsafeBytes { raw in
            for (index, channel) in channels.enumerated() where channel >= 0 && channel < channelCount {
                for frame in lo..<hi {
                    output[index][frame - range.lowerBound] = decode(
                        raw.baseAddress!, at: (frame - lo) * block + channel * bytesPerSample)
                }
            }
        }
        return output
    }
    @inline(__always) func sample(channel: Int, frame: Int) -> Float {
        guard frame >= 0, frame < frameCount, channel >= 0, channel < channelCount else { return 0 }
        return decode(UnsafeRawPointer(mapping), at: payload + frame * block + channel * bytesPerSample)
    }
    @inline(__always) private func decode(_ pointer: UnsafeRawPointer, at position: Int) -> Float {
        if floating {
            return Float(
                bitPattern: UInt32(littleEndian: pointer.loadUnaligned(fromByteOffset: position, as: UInt32.self)))
        }
        if bytesPerSample == 2 {
            return Float(Int16(littleEndian: pointer.loadUnaligned(fromByteOffset: position, as: Int16.self))) / 32768
        }
        if bytesPerSample == 4 {
            return Float(Int32(littleEndian: pointer.loadUnaligned(fromByteOffset: position, as: Int32.self)))
                / 2147483648
        }
        let bytes = pointer.assumingMemoryBound(to: UInt8.self)
        let value = Int32(bytes[position]) | Int32(bytes[position + 1]) << 8 | Int32(bytes[position + 2]) << 16
        let signed = value & 0x800000 == 0 ? value : value | ~0xffffff
        return Float(signed) / 8388608
    }
    /// Best-effort worker hint. Darwin may retain these pages; bounded workers
    /// therefore use readFrames rather than relying on mmap reclamation.
    func discardPages() { madvise(mapping, byteCount, MADV_DONTNEED) }
}

extension AudioSamples {
    public init(mappedWaveFile url: URL, deleteOnRelease: Bool = false) throws {
        self.init(mapped: try MappedWaveSamples(url: url, deleteOnRelease: deleteOnRelease))
    }
}
