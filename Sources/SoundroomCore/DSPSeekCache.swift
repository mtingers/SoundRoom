import Foundation

struct EffectSeekState {
    let kind: RealtimeEffectKind
    let state: EffectState
    let values: [Float]
    let cachedValues: [Float]
    let lastLegacyAmount: Float
    let bypassBlend: Float
    var bytes: Int { state.seekStorageBytes + values.count * 8 + 128 }
}

final class DSPSeekCheckpoint {
    let frame: Int
    let tracks: [UUID: [UUID: EffectSeekState]]
    let bytes: Int
    let master: MasterBusProcessor?
    init(frame: Int, tracks: [UUID: [UUID: EffectSeekState]], master: MasterBusProcessor? = nil) {
        self.frame = frame
        self.tracks = tracks
        self.master = master
        bytes = tracks.values.reduce(256 + (master?.storageBytes ?? 0)) { $0 + $1.values.reduce(128) { $0 + $1.bytes } }
    }
}

/// Prepared-worker snapshots only. Callback-owned states are never copied or
/// shared with checkpoints. Eviction uses a global byte budget across documents.
public final class DSPSeekCache: @unchecked Sendable {
    public let intervalFrames: Int
    public let maximumBytes: Int
    private let lock = NSLock()
    private struct Entry {
        let checkpoint: DSPSeekCheckpoint
        var accessed: UInt64
    }
    private var values: [Data: [Int: Entry]] = [:]
    private var clock: UInt64 = 0
    private var bytes = 0
    public var residentBytes: Int {
        lock.lock()
        defer { lock.unlock() }
        return bytes
    }
    public var checkpointCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return values.values.reduce(0) { $0 + $1.count }
    }
    public init(intervalFrames: Int = Int(sessionRate * 30), maximumBytes: Int = 64 * 1024 * 1024) {
        self.intervalFrames = max(1, intervalFrames)
        self.maximumBytes = max(0, maximumBytes)
    }
    func key(
        session: Session, assets: [UUID: AudioSamples], includeTails: Bool, end: Int?, loopRange: Range<Int>? = nil
    ) -> Data? {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        guard var key = try? encoder.encode(session) else { return nil }
        for id in assets.keys.sorted(by: { $0.uuidString < $1.uuidString }) {
            if let audio = assets[id] {
                key.append(Data("|\(id):\(audio.cacheIdentity):\(audio.count):\(audio.channelCount)".utf8))
            }
        }
        key.append(
            Data("|\(includeTails):\(end ?? -1):\(loopRange?.lowerBound ?? -1):\(loopRange?.upperBound ?? -1)".utf8))
        return key
    }
    func nearest(key: Data, frame: Int) -> DSPSeekCheckpoint? {
        lock.lock()
        defer { lock.unlock() }
        clock &+= 1
        guard let candidate = values[key]?.keys.filter({ $0 <= frame }).max(), var entry = values[key]?[candidate]
        else { return nil }
        entry.accessed = clock
        values[key]?[candidate] = entry
        return entry.checkpoint
    }
    func store(key: Data, checkpoint: DSPSeekCheckpoint) {
        guard checkpoint.bytes + key.count <= maximumBytes else { return }
        lock.lock()
        defer { lock.unlock() }
        if let previous = values[key]?[checkpoint.frame] {
            bytes -= previous.checkpoint.bytes
            values[key]?[checkpoint.frame] = nil
        }
        while bytes + checkpoint.bytes + (values[key] == nil ? key.count : 0) > maximumBytes {
            var oldest: (Data, Int, Entry)?
            for (key, entries) in values {
                for (frame, entry) in entries {
                    if oldest == nil || entry.accessed < oldest!.2.accessed { oldest = (key, frame, entry) }
                }
            }
            guard let oldest else { break }
            bytes -= oldest.2.checkpoint.bytes
            values[oldest.0]?[oldest.1] = nil
            if values[oldest.0]?.isEmpty == true {
                values[oldest.0] = nil
                bytes -= oldest.0.count
            }
        }
        if values[key] == nil { bytes += key.count }
        clock &+= 1
        values[key, default: [:]][checkpoint.frame] = Entry(checkpoint: checkpoint, accessed: clock)
        bytes += checkpoint.bytes
    }
    public func removeAll() {
        lock.lock()
        values.removeAll()
        bytes = 0
        lock.unlock()
    }
}

extension EffectState {
    var seekStorageBytes: Int {
        var result =
            (utility == nil ? 0 : 4801 * 8) + 256
            + (delayL.count + delayR.count + advancedDelayL.count + advancedDelayR.count) * MemoryLayout<Float>.stride
        result += (biquadsL.count + biquadsR.count) * MemoryLayout<Biquad>.stride
        for room in [roomL, roomR].compactMap({ $0 }) {
            result +=
                (room.lines.reduce(0) { $0 + $1.count } + room.allpasses.reduce(0) { $0 + $1.count })
                * MemoryLayout<Float>.stride + 256
        }
        result += (guitarRig?.storageBytes ?? 0)
        result += (putty?.storageBytes ?? 0) + (vocal?.storageBytes ?? 0) + (isolation?.storageBytes ?? 0)
        return result + (stereoRoom?.storageBytes ?? 0) + (chorus?.seekStorageBytes ?? 0)
            + (equalizer?.storageBytes ?? 0) + (warmth?.storageBytes ?? 0) + (limiter?.storageBytes ?? 0)
            + (resonance?.storageBytes ?? 0) + (pitch?.storageBytes ?? 0) + (pitchMeterDelay?.storageBytes ?? 0)
    }
    /// Every backing array is unique before reaching a callback, avoiding COW
    /// allocations on the first delayed/reverberant sample after a cached seek.
    func seekCopy(kind: RealtimeEffectKind) -> EffectState {
        let copy = EffectState(kind: kind)
        func floats(_ source: [Float]) -> [Float] { source.withUnsafeBufferPointer { Array($0) } }
        func room(_ source: RoomNetwork?) -> RoomNetwork? {
            guard var source else { return nil }
            source.lines = source.lines.map(floats)
            source.allpasses = source.allpasses.map(floats)
            source.positions = source.positions.withUnsafeBufferPointer { Array($0) }
            source.feedback = floats(source.feedback)
            source.damping = floats(source.damping)
            source.allpassPositions = source.allpassPositions.withUnsafeBufferPointer { Array($0) }
            return source
        }
        copy.guitarRig = guitarRig?.deepCopy()
        copy.gate = gate?.deepCopy()
        copy.utility = utility?.deepCopy()
        copy.limiter = limiter?.deepCopy()
        copy.resonance = resonance?.deepCopy()
        copy.putty = putty?.deepCopy()
        copy.vocal = vocal?.deepCopy()
        copy.isolation = isolation?.deepCopy()
        copy.filterL = filterL
        copy.filterR = filterR
        copy.delayL = floats(delayL)
        copy.delayR = floats(delayR)
        copy.delayIndex = delayIndex
        copy.advancedDelayL = floats(advancedDelayL)
        copy.advancedDelayR = floats(advancedDelayR)
        copy.advancedDelayIndex = advancedDelayIndex
        copy.biquadsL = biquadsL.withUnsafeBufferPointer { Array($0) }
        copy.biquadsR = biquadsR.withUnsafeBufferPointer { Array($0) }
        copy.envelope = envelope
        copy.toneL = toneL
        copy.toneR = toneR
        copy.stereoRoom = stereoRoom?.deepCopy()
        copy.roomL = room(roomL)
        copy.roomR = room(roomR)
        copy.chorus = chorus?.seekCopy()
        copy.equalizer = equalizer?.deepCopy()
        copy.warmth = warmth?.deepCopy()
        copy.pitch = pitch?.deepCopy()
        copy.pitchMeterDelay = pitchMeterDelay?.deepCopy()
        return copy
    }
}

extension EffectProcessor {
    func seekSnapshot() -> EffectSeekState {
        EffectSeekState(
            kind: effect.kind, state: state.seekCopy(kind: effect.kind),
            values: values.withUnsafeBufferPointer { Array($0) },
            cachedValues: cachedValues.withUnsafeBufferPointer { Array($0) }, lastLegacyAmount: lastLegacyAmount,
            bypassBlend: bypassBlend)
    }
    func restoreSeek(_ saved: EffectSeekState) {
        state = saved.state.seekCopy(kind: saved.kind)
        values = saved.values.withUnsafeBufferPointer { Array($0) }
        cachedValues = saved.cachedValues.withUnsafeBufferPointer { Array($0) }
        lastLegacyAmount = saved.lastLegacyAmount
        bypassBlend = saved.bypassBlend
    }
}
