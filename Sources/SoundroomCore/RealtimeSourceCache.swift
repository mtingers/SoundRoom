import Foundation
import Darwin

/// A bounded float read-ahead cache shared by all clips in a prepared mixer.
/// Missing tiles produce silence and a diagnostic count. The device
/// thread never opens files, touches mapped PCM pages or waits for disk/cache locks.
final class RealtimeSourceCache {
    private struct Key: Hashable { let asset: UUID; let tile: Int }
    private struct Source { let clip: Clip; let id: UUID; let audio: AudioSamples; let renderOffset: Int }
    private final class Tile {
        let left: [Float], right: [Float]
        init(audio: AudioSamples, start: Int, frames: Int) {
            let read = audio.readFrames(start..<(start + frames), channels: audio.channelCount > 1 ? [0, 1] : [0])
            left = read[0]; right = audio.channelCount > 1 ? read[1] : left
        }
    }
    private final class Snapshot {
        let tiles: [Key: Tile]
        init(_ tiles: [Key: Tile]) { self.tiles = tiles }
    }
    static let tileFrames = 8192
    private let sources: [Source]
    private let loopRange: Range<Int>?
    private var tiles: [Key: Tile] = [:]
    private var order: [Key] = []
    private var retired: Unmanaged<Snapshot>?
    private let queue = DispatchQueue(label: "Soundroom.source-read-ahead", qos: .userInitiated)
    private var timer: DispatchSourceTimer?
    private let cursor = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    private let misses = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    private let published = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    private let readers = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    private let generation = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    private let maximumTiles = 256 // 16 MiB stereo, plus one worker tile and retirement.
    var missedBlocks: Int64 { OSAtomicAdd64Barrier(0, misses) }
    var publicationGeneration: Int64 { OSAtomicAdd64Barrier(0, generation) }
    var activeReaders: Int64 { OSAtomicAdd64Barrier(0, readers) }
    init(session: Session, assets: [UUID: AudioSamples], startFrame: Int, loopRange: Range<Int>? = nil) {
        self.loopRange = loopRange
        sources = session.tracks.flatMap { track in track.clips.compactMap { clip in
            guard let audio = assets[clip.assetID], audio.isStreamed else { return nil }
            return Source(clip: clip, id: clip.assetID, audio: audio, renderOffset: session.tracks.contains(where: { $0.routing != nil }) ? 0 : track.realtimeEffects.reduce(0) { $0 + $1.latencyFrames })
        } }
        cursor.initialize(to: Int64(startFrame)); misses.initialize(to: 0)
        published.initialize(to: 0); readers.initialize(to: 0); generation.initialize(to: 0)
        fill(at: startFrame)
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now() + .milliseconds(2), repeating: .milliseconds(2), leeway: .milliseconds(1))
        timer.setEventHandler { [weak self] in
            guard let self else { return }; self.fill(at: Int(OSAtomicAdd64Barrier(0, self.cursor)))
        }
        self.timer = timer; timer.resume()
    }
    deinit {
        timer?.cancel()
        let pointer = OSAtomicAdd64Barrier(0, published)
        if pointer != 0 { snapshot(pointer).release() }; retired?.release()
        cursor.deinitialize(count: 1); cursor.deallocate(); misses.deinitialize(count: 1); misses.deallocate()
        published.deinitialize(count: 1); published.deallocate(); readers.deinitialize(count: 1); readers.deallocate()
        generation.deinitialize(count: 1); generation.deallocate()
    }
    func advance(to frame: Int) {
        let old = OSAtomicAdd64Barrier(0, cursor)
        _ = OSAtomicCompareAndSwap64Barrier(old, Int64(frame), cursor)
    }
    /// Enter the reader barrier before loading the immutable pointer. A worker
    /// swap keeps the old snapshot alive until all readers leave; the callback
    /// never performs retirement or waits for worker locks.
    func withSamples(id: UUID, sourceFrame: Int, count: Int, body: (UnsafePointer<Float>, UnsafePointer<Float>, Int, Int) -> Void) {
        OSAtomicIncrement64Barrier(readers)
        let pointer = OSAtomicAdd64Barrier(0, published)
        guard pointer != 0 else { OSAtomicIncrement64Barrier(misses); OSAtomicDecrement64Barrier(readers); return }
        readSnapshot(pointer, id: id, sourceFrame: sourceFrame, count: count, body: body)
        OSAtomicDecrement64Barrier(readers)
    }
    /// A separate stack frame releases all temporary Swift references before
    /// lowering the barrier. Otherwise ARC epilogue ordering could let a worker
    /// retire its final retained reference before a callback's local is released.
    @inline(never) private func readSnapshot(_ pointer: Int64, id: UUID, sourceFrame: Int, count: Int, body: (UnsafePointer<Float>, UnsafePointer<Float>, Int, Int) -> Void) {
        let view = snapshot(pointer).takeUnretainedValue()
        var copied = 0
        while copied < count {
            let frame = sourceFrame + copied, tileIndex = frame / Self.tileFrames, local = frame % Self.tileFrames
            let length = min(count - copied, Self.tileFrames - local)
            if frame >= 0, let tile = view.tiles[Key(asset: id, tile: tileIndex)] {
                tile.left.withUnsafeBufferPointer { left in tile.right.withUnsafeBufferPointer { right in
                    body(left.baseAddress! + local, right.baseAddress! + local, copied, length)
                } }
            } else { OSAtomicIncrement64Barrier(misses) }
            copied += length
        }
    }
    private func fill(at frame: Int) {
        if let retired {
            guard OSAtomicAdd64Barrier(0, readers) == 0 else { return }
            self.retired = nil; retired.release()
        }
        var changed = false
        let lookAhead = Self.tileFrames * (loopRange == nil ? 8 : 4)
        var required: [Key: AudioSamples] = [:]
        for source in sources {
            for window in SourceReadAheadWindows.ranges(outputFrame: frame, latency: source.renderOffset, horizon: lookAhead, loop: loopRange) {
                let sourceAnchor = window.lowerBound, sourceUpper = window.upperBound
                guard source.clip.start < sourceUpper && source.clip.end > sourceAnchor else { continue }
                let first = source.clip.offset + max(sourceAnchor, source.clip.start) - source.clip.start
                let end = source.clip.offset + min(sourceUpper, source.clip.end) - source.clip.start
                for tile in (first / Self.tileFrames)...(max(first, end - 1) / Self.tileFrames) {
                    required[Key(asset: source.id, tile: tile)] = source.audio
                }
            }
        }
        for (key, audio) in required {
            if tiles[key] != nil { continue }
            let tile = Tile(audio: audio, start: key.tile * Self.tileFrames, frames: Self.tileFrames)
            if tiles[key] == nil {
                var attempts = order.count
                while tiles.count >= maximumTiles, !order.isEmpty, attempts > 0 {
                    attempts -= 1
                    let oldest = order.removeFirst()
                    if required[oldest] != nil { order.append(oldest); continue }
                    tiles[oldest] = nil
                }
                if tiles.count >= maximumTiles, let oldest = order.first { order.removeFirst(); tiles[oldest] = nil }
                tiles[key] = tile; order.append(key)
                changed = true
            }
            audio.discardReadPages()
        }
        if changed || OSAtomicAdd64Barrier(0, published) == 0 {
            let next = Unmanaged.passRetained(Snapshot(tiles))
            let pointer = Int64(bitPattern: UInt64(UInt(bitPattern: next.toOpaque())))
            let previous = OSAtomicAdd64Barrier(0, published)
            precondition(OSAtomicCompareAndSwap64Barrier(previous, pointer, published))
            OSAtomicIncrement64Barrier(generation)
            if previous != 0 { retired = snapshot(previous) }
            if OSAtomicAdd64Barrier(0, readers) == 0 { retired?.release(); retired = nil }
        }
    }
    private func snapshot(_ pointer: Int64) -> Unmanaged<Snapshot> {
        Unmanaged.fromOpaque(UnsafeRawPointer(bitPattern: UInt(UInt64(bitPattern: pointer)))!)
    }
}
