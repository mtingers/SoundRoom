import Foundation
import SoundroomRemote

extension WaveformEnvelope {
    init?(asset: RemoteAsset) {
        guard !asset.peaks.isEmpty, asset.peaks.count <= 16_384, asset.peaks.count.isMultiple(of: 2) else { return nil }
        func pairs(_ flat: [Float]) -> [WaveformPeak] {
            stride(from: 0, to: flat.count, by: 2).map {
                (flat[$0].isFinite ? flat[$0] : 0, flat[$0 + 1].isFinite ? flat[$0 + 1] : 0)
            }
        }
        let values = pairs(asset.peaks)
        let bin =
            asset.framesPerBin ?? Swift.max(1, asset.frames / values.count + (asset.frames % values.count == 0 ? 0 : 1))
        guard bin > 0, asset.waveformStartFrame >= 0, asset.waveformStartFrame < asset.frames else { return nil }
        // Ignore malformed or absent optional channel measurements, retaining
        // the combined view without inventing identical left/right waveforms.
        let channels =
            asset.channelPeaks.flatMap { channels -> [[WaveformPeak]]? in
                guard asset.channels == 2, channels.count == 2,
                    channels.allSatisfy({ $0.count == asset.peaks.count })
                else { return nil }
                return channels.map(pairs)
            } ?? []
        self.init(framesPerBin: bin, firstFrame: asset.waveformStartFrame, values: values, channelValues: channels)
    }
}

/// Display-only peaks. Scroll requests use stable, expanded buckets rather than
/// a different network request for every pixel. A full-asset overview keeps all
/// visible audio drawn while finer measurements arrive.
struct RemoteWaveformRequest: Hashable {
    let id: UUID
    let start: Int
    let end: Int
    let points: Int
    var isOverview: Bool { points <= 2048 && start == 0 }

    static func overview(id: UUID, frames: Int, points: Int = 2048) -> Self {
        .init(id: id, start: 0, end: frames, points: max(1, min(2048, points)))
    }
    static func detail(id: UUID, frames: Int, range: Range<Int>) -> Self {
        let lower = max(0, min(frames - 1, range.lowerBound))
        let upper = min(frames, max(lower + 1, range.upperBound))
        var bucket = 4096
        while bucket < upper - lower, bucket <= Int.max / 2 { bucket *= 2 }
        let anchor = lower / bucket
        let start = anchor > 0 ? (anchor - 1) * bucket : 0
        let remaining = frames - lower
        let end = bucket > remaining / 3 ? frames : min(frames, (anchor + 3) * bucket)
        return .init(id: id, start: start, end: end, points: 8192)
    }

    /// Wait for the overview, then refine only if its bins would be wider than
    /// a display pixel. Fetching 8192 bins for every small clip exhausted the
    /// cache and made stationary arrangements continually evict and reload.
    static func refinement(
        id: UUID, frames: Int, range: Range<Int>, pixels: Int,
        envelope: WaveformEnvelope?
    ) -> Self? {
        guard let envelope,
            envelope.framesPerBin > max(1, range.count / max(1, pixels))
        else { return nil }
        return detail(id: id, frames: frames, range: range)
    }
}

struct RemoteWaveformCache {
    private struct Entry {
        let id: UUID
        let envelope: WaveformEnvelope
        let end: Int
        let overview: Bool
        let bytes: Int
        var access: UInt64
    }
    private var entries: [UUID: [Entry]] = [:]
    private var entryCount = 0
    private var clock: UInt64 = 0
    private(set) var residentBytes = 0
    let maximumBytes: Int
    let maximumEntries: Int
    private(set) var overviewPoints = 2048
    init(maximumBytes: Int = 8 * 1024 * 1024, maximumEntries: Int = 4096) {
        self.maximumBytes = max(0, maximumBytes)
        self.maximumEntries = max(0, maximumEntries)
    }
    var count: Int { entryCount }
    mutating func removeAll() {
        entries.removeAll()
        entryCount = 0
        residentBytes = 0
    }
    mutating func retainAssets(_ ids: Set<UUID>) {
        entries = entries.filter { ids.contains($0.key) }
        recount()
    }
    private mutating func recount() {
        entryCount = entries.values.reduce(0) { $0 + $1.count }
        residentBytes = entries.values.reduce(0) { total, tiles in total + tiles.reduce(0) { $0 + $1.bytes } }
    }
    private mutating func remove(id: UUID, index: Int) {
        residentBytes -= entries[id]!.remove(at: index).bytes
        entryCount -= 1
        if entries[id]!.isEmpty { entries.removeValue(forKey: id) }
    }

    /// Reserve half the byte budget for details. Large documents get smaller
    /// full-asset overviews rather than endlessly evicting each other's audio.
    /// Close zoom still requests the same exact fine tiles on demand.
    mutating func configure(assetCount: Int) {
        let count = max(1, min(maximumEntries, assetCount))
        let perAsset = max(0, maximumBytes / 2 / count - 128)
        var points = 2048
        while points > 32 && points * 3 * MemoryLayout<WaveformPeak>.stride > perAsset { points /= 2 }
        guard points != overviewPoints else { return }
        overviewPoints = points
        // Recompact retained overviews when the asset population grows. Fine
        // tiles remain disposable; previously drawn full assets stay available.
        let overviews = entries.values.flatMap { $0.filter(\.overview) }.sorted { $0.access < $1.access }
        entries = entries.mapValues { $0.filter { !$0.overview } }.filter { !$0.value.isEmpty }
        recount()
        for entry in overviews {
            install(
                id: entry.id, frames: entry.end,
                envelope: entry.envelope.count > overviewPoints ? compact(entry.envelope) : entry.envelope)
        }
    }

    private func compact(_ envelope: WaveformEnvelope) -> WaveformEnvelope {
        let factor = 1 + (envelope.count - 1) / overviewPoints
        func fold(_ peaks: [WaveformPeak]) -> [WaveformPeak] {
            stride(from: 0, to: peaks.count, by: factor).map { first in
                peaks[first..<min(peaks.count, first + factor)].reduce(WaveformPeak(0, 0)) {
                    (min($0.0, $1.0), max($0.1, $1.1))
                }
            }
        }
        return .init(
            framesPerBin: envelope.framesPerBin * factor, values: fold(envelope.values),
            channelValues: envelope.channelValues.map(fold))
    }

    /// Never return an arbitrary partial tile: that made the waveform disappear
    /// as the viewport crossed its edge. Choose the finest complete measurement.
    mutating func envelope(id: UUID, range: Range<Int>) -> WaveformEnvelope? {
        guard let tiles = entries[id] else { return nil }
        var best: Int?
        for index in tiles.indices {
            let tile = tiles[index]
            guard tile.envelope.firstFrame <= range.lowerBound, tile.end >= range.upperBound else { continue }
            if let previous = best,
                tiles[previous].envelope.framesPerBin < tile.envelope.framesPerBin
                    || (tiles[previous].envelope.framesPerBin == tile.envelope.framesPerBin
                        && tiles[previous].access > tile.access)
            {
                continue
            }
            best = index
        }
        guard let best else { return nil }
        clock &+= 1
        entries[id]![best].access = clock
        return tiles[best].envelope
    }
    func contains(_ request: RemoteWaveformRequest) -> Bool {
        let targetBin = max(1, (request.end - request.start + request.points - 1) / request.points)
        return entries[request.id]?.contains {
            $0.envelope.firstFrame <= request.start && $0.end >= request.end
                && (request.isOverview || $0.envelope.framesPerBin <= targetBin)
        } ?? false
    }
    @discardableResult mutating func install(id: UUID, frames: Int, envelope: WaveformEnvelope) -> Bool {
        guard frames > 0, envelope.framesPerBin > 0, !envelope.values.isEmpty,
            envelope.firstFrame >= 0, envelope.firstFrame < frames
        else { return false }
        let span = envelope.count.multipliedReportingOverflow(by: envelope.framesPerBin)
        guard !span.overflow else { return false }
        let end = envelope.firstFrame + min(frames - envelope.firstFrame, span.partialValue)
        guard
            envelope.channelValues.isEmpty
                || (envelope.channelValues.count == 2
                    && envelope.channelValues.allSatisfy({ $0.count == envelope.count }))
        else { return false }
        let fullAsset = envelope.firstFrame == 0 && end == frames
        let cost = envelope.byteCount + 128
        guard maximumEntries > 0 else { return false }
        let overview = fullAsset && envelope.count <= overviewPoints
        var fallbackChanged = false
        if fullAsset && !overview {
            // A full-range detail is still expendable detail. Keep a separate
            // small fallback so evicting it never blanks the entire clip.
            fallbackChanged = install(id: id, frames: frames, envelope: compact(envelope))
        }
        guard cost <= maximumBytes else { return fallbackChanged }
        if !overview {
            var protectedCount = 0
            var protectedBytes = cost
            for tiles in entries.values {
                for tile in tiles where tile.overview {
                    protectedCount += 1
                    protectedBytes += tile.bytes
                }
            }
            guard protectedCount < maximumEntries, protectedBytes <= maximumBytes else { return fallbackChanged }
        }
        // Duplicate event/reply deliveries must not invalidate or downgrade peaks.
        if let tiles = entries[id],
            let index = tiles.firstIndex(where: {
                $0.envelope.firstFrame == envelope.firstFrame && $0.end == end && $0.overview == overview
            })
        {
            let previous = tiles[index].envelope
            if previous.framesPerBin < envelope.framesPerBin
                || (previous.framesPerBin == envelope.framesPerBin
                    && previous.channelValues.count >= envelope.channelValues.count)
            {
                return fallbackChanged
            }
            remove(id: id, index: index)
        }
        while entryCount > 0 && (residentBytes + cost > maximumBytes || entryCount >= maximumEntries) {
            // Keep an overview for each asset whenever finer tiles can be evicted.
            var oldest: (id: UUID, index: Int, overview: Bool, access: UInt64)?
            for (asset, tiles) in entries {
                for (index, tile) in tiles.enumerated() {
                    if let previous = oldest {
                        if !previous.overview && tile.overview { continue }
                        if previous.overview == tile.overview && previous.access <= tile.access { continue }
                    }
                    oldest = (asset, index, tile.overview, tile.access)
                }
            }
            guard let oldest else { break }
            remove(id: oldest.id, index: oldest.index)
        }
        clock &+= 1
        entries[id, default: []].append(
            .init(id: id, envelope: envelope, end: end, overview: overview, bytes: cost, access: clock))
        entryCount += 1
        residentBytes += cost
        return true
    }
}

/// At most one waiting overview and one waiting detail per asset. A fast scroll
/// replaces stale detail work instead of building minutes of FIFO backlog.
struct RemoteWaveformQueue {
    private(set) var pending: [RemoteWaveformRequest] = []
    private(set) var active: Set<RemoteWaveformRequest> = []
    let maximumPending = 128
    mutating func enqueue(_ request: RemoteWaveformRequest) {
        if pending.contains(request) { return }
        pending.removeAll { $0.id == request.id && $0.isOverview == request.isOverview }
        guard !active.contains(request) else { return }
        guard pending.count < maximumPending else { return }
        if request.isOverview, let detail = pending.firstIndex(where: { $0.id == request.id && !$0.isOverview }) {
            pending.insert(request, at: detail)
        } else {
            pending.append(request)
        }
    }
    mutating func next() -> RemoteWaveformRequest? {
        guard !pending.isEmpty else { return nil }
        let request = pending.removeFirst()
        active.insert(request)
        return request
    }
    mutating func complete(_ request: RemoteWaveformRequest) { active.remove(request) }
    mutating func removeAll() {
        pending.removeAll()
        active.removeAll()
    }
}
