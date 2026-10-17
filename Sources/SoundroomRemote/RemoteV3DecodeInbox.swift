import Foundation

/// Reader-owned wire backlog. Only one ordinary frame is decoded/delivered at
/// a time; small validated heartbeats/empty receipts bypass this backlog.
struct RemoteV3DecodeInbox {
    private var frames: [Data] = []
    private var index = 0
    private(set) var bytes = 0
    var count: Int { frames.count - index }
    let maximumBytes: Int
    let maximumFrames: Int

    init(maximumBytes: Int = RemoteProtocol.maximumQueuedBytes, maximumFrames: Int = 2048) {
        precondition(maximumBytes > 0 && maximumFrames > 0)
        self.maximumBytes = maximumBytes
        self.maximumFrames = maximumFrames
    }
    mutating func append(_ data: Data) throws {
        guard !data.isEmpty, data.count <= RemoteProtocol.maximumFrameBytes,
            count < maximumFrames, data.count <= maximumBytes - bytes
        else { throw RemoteError.queueFull }
        frames.append(data)
        bytes += data.count
    }
    mutating func next() -> Data? {
        guard index < frames.count else { return nil }
        let data = frames[index]
        frames[index] = Data()
        index += 1
        bytes -= data.count
        if index >= 64 && index * 2 >= frames.count {
            frames.removeFirst(index)
            index = 0
        }
        return data
    }
    mutating func clear() {
        frames = []
        index = 0
        bytes = 0
    }

    static func immediateControl(_ data: Data) throws -> RemoteWireMessage? {
        guard data.count <= 4096,
            let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
            let kind = object["kind"] as? String,
            kind == "heartbeat" || kind == "heartbeatReply" || (kind == "reply" && object["payload"] == nil)
        else { return nil }
        // Use the canonical strict codec, including version/field/UUID checks.
        // A claimed heartbeat must never bypass validation or execute a command.
        let message = try RemoteV3Codec.decodeMessage(data)
        switch message {
        case .heartbeat, .heartbeatReply, .reply: return message
        default: throw RemoteError.invalidFrame
        }
    }
}

/// Empty receipts can overtake document work, but a stalled controller must not
/// accumulate unbounded actor tasks. Tickets make duplicate acknowledgments safe.
struct RemoteV3ControlDeliveryBudget {
    private var tickets: [UUID: Int] = [:]
    private(set) var bytes = 0
    var count: Int { tickets.count }
    let maximumBytes: Int
    let maximumFrames: Int
    init(maximumBytes: Int = 256 * 1024, maximumFrames: Int = 128) {
        precondition(maximumBytes > 0 && maximumFrames > 0)
        self.maximumBytes = maximumBytes
        self.maximumFrames = maximumFrames
    }
    mutating func reserve(_ size: Int) throws -> UUID {
        guard size > 0, size <= 4096, count < maximumFrames, size <= maximumBytes - bytes else {
            throw RemoteError.queueFull
        }
        let ticket = UUID()
        tickets[ticket] = size
        bytes += size
        return ticket
    }
    mutating func release(_ ticket: UUID) {
        if let size = tickets.removeValue(forKey: ticket) { bytes -= size }
    }
    mutating func clear() {
        tickets = [:]
        bytes = 0
    }
}
