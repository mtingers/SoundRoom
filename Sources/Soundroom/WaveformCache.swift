import Foundation
import Combine
import SoundroomCore

typealias WaveformPeak = (Float, Float)

struct WaveformEnvelope: RandomAccessCollection {
    typealias Index = Int
    let framesPerBin: Int
    var firstFrame: Int = 0
    let values: [WaveformPeak]
    /// Empty for mono or legacy combined measurements. Never infer two
    /// separate channels from a combined envelope.
    var channelValues: [[WaveformPeak]] = []
    var byteCount: Int { (values.count + channelValues.reduce(0) { $0 + $1.count }) * MemoryLayout<WaveformPeak>.stride }
    var startIndex: Int { values.startIndex }
    var endIndex: Int { values.endIndex }
    subscript(index: Int) -> (Float, Float) { values[index] }

    static func measured(framesPerBin: Int, firstFrame: Int = 0, channels: [[WaveformPeak]]) -> Self {
        let first = channels.first ?? []
        let combined = channels.dropFirst().reduce(first) { result, channel in
            zip(result, channel).map { (Swift.min($0.0, $1.0), Swift.max($0.1, $1.1)) }
        }
        return .init(framesPerBin: framesPerBin, firstFrame: firstFrame, values: combined,
                     channelValues: channels.count == 2 ? channels : [])
    }
}

/// Waveform arrivals repaint audio lanes, without invalidating the document,
/// mixer, inspector and every other SessionStore observer. Coalesce bursts.
@MainActor final class WaveformDisplay: ObservableObject {
    private var scheduled = false
    func invalidate() {
        guard !scheduled else { return }
        scheduled = true
        Task { [weak self] in
            try? await Task.sleep(nanoseconds: 16_000_000)
            guard let self else { return }
            scheduled = false
            objectWillChange.send()
        }
    }
}

/// A byte-bounded LRU, including per-entry overhead. Very long assets use wider
/// bins to cap one envelope's allocation; drawing uses that actual bin width.
final class WaveformCache: ObservableObject {
    static let shared = WaveformCache()
    private struct Entry { let envelope: WaveformEnvelope; let bytes: Int; var accessed: UInt64 }
    private let lock = NSCondition()
    private var values: [UUID: Entry] = [:]
    private var pending: Set<UUID> = []
    private var computing: Set<UUID> = []
    private let analysisQueue = DispatchQueue(label: "Soundroom.waveform-analysis", qos: .utility)
    private var clock: UInt64 = 0
    private var bytes = 0
    let maximumBytes: Int
    let maximumEnvelopeBytes: Int
    /// Includes hits and worker requests; useful for verifying that playback
    /// telemetry leaves cached, stationary clip drawings untouched.
    var accessCount: UInt64 { lock.lock(); defer { lock.unlock() }; return clock }
    var residentBytes: Int { lock.lock(); defer { lock.unlock() }; return bytes }
    var cachedAssetCount: Int { lock.lock(); defer { lock.unlock() }; return values.count }
    init(maximumBytes: Int = 32 * 1024 * 1024, maximumEnvelopeBytes: Int = 4 * 1024 * 1024) {
        self.maximumBytes = max(0, maximumBytes); self.maximumEnvelopeBytes = max(8, min(maximumEnvelopeBytes, max(8, maximumBytes)))
    }
    /// UI lookup schedules missing analysis on a worker; Canvas never scans an
    /// hour of media on the main thread. The synchronous method serves that worker.
    func cachedEnvelope(id: UUID, audio: AudioSamples) -> WaveformEnvelope? {
        lock.lock(); clock &+= 1
        if var entry = values[id] { entry.accessed = clock; values[id] = entry; lock.unlock(); return entry.envelope }
        let schedule = pending.insert(id).inserted; lock.unlock()
        if schedule {
            analysisQueue.async { [weak self] in
                guard let self else { return }
                _ = self.envelope(id: id, audio: audio)
                self.lock.lock(); self.pending.remove(id); self.lock.unlock()
                DispatchQueue.main.async { [weak self] in self?.objectWillChange.send() }
            }
        }
        return nil
    }
    func envelope(id: UUID, audio: AudioSamples) -> WaveformEnvelope {
        lock.lock(); clock &+= 1
        while computing.contains(id) { lock.wait() }
        if var entry = values[id] { entry.accessed = clock; values[id] = entry; lock.unlock(); return entry.envelope }
        computing.insert(id); lock.unlock()
        let channelCount = min(2, max(1, audio.channelCount))
        let maximumBins = maximumBins(channels: channelCount)
        let bin = max(128, audio.count / maximumBins + (audio.count % maximumBins == 0 ? 0 : 1))
        var result = Array(repeating: Array(repeating: WaveformPeak(0, 0), count: audio.count / bin + (audio.count % bin == 0 ? 0 : 1)), count: channelCount)
        for start in stride(from: 0, to: audio.count, by: 8192) {
            let block = audio.readFrames(start..<min(start + 8192, audio.count), channels: Array(0..<channelCount))
            for (channel, samples) in block.enumerated() { for (index, raw) in samples.enumerated() {
                let sample = raw.isFinite ? raw : 0, binIndex = (start + index) / bin
                result[channel][binIndex].0 = min(result[channel][binIndex].0, sample); result[channel][binIndex].1 = max(result[channel][binIndex].1, sample)
            } }
        }
        let envelope = WaveformEnvelope.measured(framesPerBin: bin, channels: result)
        let cost = envelope.byteCount + 128
        lock.lock(); defer { computing.remove(id); lock.broadcast(); lock.unlock() }
        if let existing = values[id] { return existing.envelope }
        while !values.isEmpty && (bytes + cost > maximumBytes || values.count >= 512) {
            if let oldest = values.min(by: { $0.value.accessed < $1.value.accessed }) {
                bytes -= oldest.value.bytes; values[oldest.key] = nil
            }
        }
        if cost <= maximumBytes { clock &+= 1; values[id] = Entry(envelope: envelope, bytes: cost, accessed: clock); bytes += cost }
        return envelope
    }
    /// Called only on utility workers. Coarse requests reuse the immutable full
    /// asset envelope; close zoom scans just the requested range for exact detail.
    func rangeEnvelope(id: UUID, audio: AudioSamples, range: Range<Int>, points: Int) -> WaveformEnvelope {
        let bin = max(1, (range.count + points - 1) / points)
        let channelCount = min(2, max(1, audio.channelCount))
        var peaks = Array(repeating: Array(repeating: WaveformPeak(0, 0), count: (range.count + bin - 1) / bin), count: channelCount)
        let maximumBins = maximumBins(channels: channelCount)
        let cachedBin = max(128, audio.count / maximumBins + (audio.count % maximumBins == 0 ? 0 : 1))
        if bin >= cachedBin {
            let whole = envelope(id: id, audio: audio)
            let channels = whole.channelValues.isEmpty ? [whole.values] : whole.channelValues
            peaks = channels.map { _ in Array(repeating: WaveformPeak(0, 0), count: (range.count + bin - 1) / bin) }
            for index in peaks[0].indices {
                let first = (range.lowerBound + index * bin) / whole.framesPerBin
                let last = min(whole.count - 1, (min(range.upperBound, range.lowerBound + (index + 1) * bin) - 1) / whole.framesPerBin)
                if first <= last { for source in first...last {
                    for channel in channels.indices {
                        peaks[channel][index].0 = min(peaks[channel][index].0, channels[channel][source].0)
                        peaks[channel][index].1 = max(peaks[channel][index].1, channels[channel][source].1)
                    }
                } }
            }
        } else {
            for start in stride(from: range.lowerBound, to: range.upperBound, by: 8192) {
                let block = audio.readFrames(start..<min(start + 8192, range.upperBound), channels: Array(0..<channelCount))
                for (channel, samples) in block.enumerated() { for (index, raw) in samples.enumerated() where raw.isFinite {
                    let slot = (start + index - range.lowerBound) / bin
                    peaks[channel][slot].0 = min(peaks[channel][slot].0, raw); peaks[channel][slot].1 = max(peaks[channel][slot].1, raw)
                } }
            }
        }
        return .measured(framesPerBin: bin, firstFrame: range.lowerBound, channels: peaks)
    }
    private func maximumBins(channels: Int) -> Int {
        // Stereo retains L, R and the compatibility envelope under one budget.
        max(1, maximumEnvelopeBytes / (MemoryLayout<WaveformPeak>.stride * (channels == 2 ? 3 : 1)))
    }
    func removeAll() { lock.lock(); values.removeAll(); bytes = 0; lock.unlock() }
}
