import Foundation

/// Incremental 4-byte big-endian length framing. Rejects sizes before allocation.
struct RemoteFrameDecoder {
    private var storage = Data()
    private var offset = 0
    mutating func append(_ data: Data) throws -> [Data] {
        guard data.count <= RemoteProtocol.maximumFrameBytes + 4,
            storage.count - offset <= RemoteProtocol.maximumFrameBytes + 4 - data.count
        else { throw RemoteError.oversizedFrame }
        if offset > 0 {
            storage = Data(storage.dropFirst(offset))
            offset = 0
        }
        storage.append(data)
        var frames: [Data] = []
        while storage.count - offset >= 4 {
            let index = storage.startIndex + offset
            let size =
                (Int(storage[index]) << 24) | (Int(storage[index + 1]) << 16) | (Int(storage[index + 2]) << 8)
                | Int(storage[index + 3])
            guard size > 0 else { throw RemoteError.invalidFrame }
            guard size <= RemoteProtocol.maximumFrameBytes else { throw RemoteError.oversizedFrame }
            guard storage.count - offset - 4 >= size else { break }
            frames.append(storage.subdata(in: (offset + 4)..<(offset + 4 + size)))
            offset += 4 + size
        }
        if offset == storage.count {
            storage.removeAll(keepingCapacity: true)
            offset = 0
        }
        return frames
    }
    static func encode(_ data: Data) throws -> Data {
        guard !data.isEmpty else { throw RemoteError.invalidFrame }
        guard data.count <= RemoteProtocol.maximumFrameBytes else { throw RemoteError.oversizedFrame }
        let size = UInt32(data.count)
        var output = Data([UInt8(size >> 24), UInt8((size >> 16) & 255), UInt8((size >> 8) & 255), UInt8(size & 255)])
        output.append(data)
        return output
    }
}

struct RemoteOutboundQueue {
    private var reliable: [Data] = []
    private var index = 0
    private var telemetry: [String: Data] = [:]
    private(set) var bytes = 0
    private(set) var discardedTelemetry = 0
    var reliableCount: Int { reliable.count - index }
    var retainedFrameSlots: Int { reliable.count }
    mutating func append(_ frame: Data, telemetryKey: String? = nil) throws {
        if let telemetryKey {
            if let previous = telemetry[telemetryKey] {
                bytes -= previous.count
                discardedTelemetry += 1
            }
            guard bytes + frame.count <= RemoteProtocol.maximumQueuedBytes else {
                discardedTelemetry += 1
                telemetry.removeValue(forKey: telemetryKey)
                return
            }
            telemetry[telemetryKey] = frame
            bytes += frame.count
            return
        }
        guard reliableCount < 1024, bytes + frame.count <= RemoteProtocol.maximumQueuedBytes else {
            throw RemoteError.queueFull
        }
        reliable.append(frame)
        bytes += frame.count
    }
    mutating func next() -> Data? {
        if index < reliable.count {
            let result = reliable[index]
            reliable[index] = Data()
            index += 1
            bytes -= result.count
            if index == reliable.count {
                reliable.removeAll(keepingCapacity: true)
                index = 0
            } else if index >= 256 {
                reliable.removeFirst(index)
                index = 0
            }
            return result
        }
        // Transport takes priority over cosmetic metering.
        if let result = telemetry.removeValue(forKey: "transport") {
            bytes -= result.count
            return result
        }
        if let key = telemetry.keys.sorted().first, let result = telemetry.removeValue(forKey: key) {
            bytes -= result.count
            return result
        }
        return nil
    }
    mutating func clear() {
        reliable.removeAll()
        index = 0
        telemetry.removeAll()
        bytes = 0
    }
}

public struct RemoteNetworkMetrics: Sendable, Equatable {
    public var queuedBytes: Int
    public var queuedFrames: Int
    public var discardedTelemetry: Int
    public var receivedFrames: Int
    public var sentFrames: Int
    public init(
        queuedBytes: Int = 0, queuedFrames: Int = 0, discardedTelemetry: Int = 0, receivedFrames: Int = 0,
        sentFrames: Int = 0
    ) {
        self.queuedBytes = queuedBytes
        self.queuedFrames = queuedFrames
        self.discardedTelemetry = discardedTelemetry
        self.receivedFrames = receivedFrames
        self.sentFrames = sentFrames
    }
}

/// Admission precedes dispatch/JSON encoding, so a slow encoder cannot accumulate
/// an unbounded GCD backlog of retained session values. Caller holds a control
/// mutex; these values and this lock are never accessed by an audio callback.
struct RemoteMessageMailbox {
    private var reliable: [(RemoteWireMessage, Int)?] = []
    private var index = 0
    private var latest: [String: (RemoteWireMessage, Int)] = [:]
    private(set) var bytes = 0
    private(set) var discardedTelemetry = 0
    var count: Int { reliable.count - index + latest.count }
    mutating func append(_ message: RemoteWireMessage, telemetryKey: String?) throws {
        let cost = message.admissionCost
        if let telemetryKey {
            if let old = latest.removeValue(forKey: telemetryKey) {
                bytes -= old.1
                discardedTelemetry += 1
            }
            guard latest.count < 32, bytes + cost <= RemoteProtocol.maximumQueuedBytes else {
                discardedTelemetry += 1
                return
            }
            latest[telemetryKey] = (message, cost)
            bytes += cost
            return
        }
        guard reliable.count - index < RemoteProtocol.maximumPendingRequests,
            bytes + cost <= RemoteProtocol.maximumQueuedBytes
        else { throw RemoteError.queueFull }
        reliable.append((message, cost))
        bytes += cost
    }
    mutating func next() -> (message: RemoteWireMessage, telemetryKey: String?)? {
        if index < reliable.count, let item = reliable[index] {
            reliable[index] = nil
            index += 1
            bytes -= item.1
            if index == reliable.count {
                reliable.removeAll(keepingCapacity: true)
                index = 0
            } else if index >= 64 {
                reliable.removeFirst(index)
                index = 0
            }
            return (item.0, nil)
        }
        if let item = latest.removeValue(forKey: "transport") {
            bytes -= item.1
            return (item.0, "transport")
        }
        if let key = latest.keys.sorted().first, let item = latest.removeValue(forKey: key) {
            bytes -= item.1
            return (item.0, key)
        }
        return nil
    }
    mutating func clear() {
        reliable.removeAll()
        latest.removeAll()
        index = 0
        bytes = 0
    }
}

private extension RemoteWireMessage {
    var admissionCost: Int {
        let cap = RemoteProtocol.maximumFrameBytes
        switch self {
        case .request(let request):
            switch request.command {
            case .editTracks, .replaceTracks, .updateTrackControls: return cap
            case .uploadChunk(_, _, let data): return min(cap, 65_536 + data.count * 2)
            case .editor(let intent):
                guard intent.numbers.count <= 32, intent.strings.count <= 32, intent.integers.count <= 32,
                    intent.flags.count <= 32,
                    intent.strings.values.allSatisfy({ $0.utf8.count <= 512 })
                else { return cap }
                return min(cap, 65_536 + (intent.payload?.count ?? 0) * 2)
            default: return 16_384
            }
        case .reply(let reply): return min(cap, 16_384 + (reply.payload?.count ?? 0) * 2)
        case .event(let event):
            switch event {
            case .snapshot, .analysis: return cap
            case .delta(let delta):
                // Scalar patches retain no clip, note or automation arrays. Charge
                // their bounded shape without encoding under the admission mutex.
                // Any document-bearing field retains the conservative frame cost.
                guard delta.tracks.isEmpty, delta.controls.isEmpty, delta.assets.isEmpty,
                    delta.masteringAnalysis == nil, delta.trackOrder == nil,
                    delta.name == nil, delta.timing == nil, delta.sessionVersion == nil,
                    (delta.controlPatches?.count ?? 0) <= 1024,
                    delta.removedTrackIDs.count <= 1024, delta.trackRevisions.count <= 1024,
                    (delta.status?.utf8.count ?? 0) <= 4096
                else { return cap }
                return min(
                    cap,
                    16_384 + (delta.controlPatches?.count ?? 0) * 256
                        + delta.removedTrackIDs.count * 128 + delta.trackRevisions.count * 128
                        + (delta.status?.utf8.count ?? 0) * 6)
            case .effectMeters(let values): return min(cap, 4096 + values.count * 4096)
            case .waveform(let asset): return min(cap, 4096 + asset.peaks.count * 64)
            case .meters: return 512
            case .transport, .audition: return 1024
            default: return 16_384
            }
        default: return 16_384
        }
    }
}
