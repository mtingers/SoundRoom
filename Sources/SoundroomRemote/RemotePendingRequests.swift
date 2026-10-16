import Foundation

/// Owner-confined request continuations. Ordinary work leaves a fixed reserve
/// for Stop, Pause, cancellation and live-note control, while the total bound
/// also applies to interrupt traffic. No command payload is retained here.
struct RemotePendingRequests<Value> {
    private var items: [UUID: Value] = [:]
    var count: Int { items.count }

    mutating func insert(_ value: Value, for id: UUID, command: RemoteCommand) throws {
        let limit = RemoteProtocol.maximumPendingRequests
            - (command.isInterrupt ? 0 : RemoteProtocol.reservedPendingInterruptRequests)
        guard items.count < limit, items[id] == nil else { throw RemoteError.queueFull }
        items[id] = value
    }

    mutating func removeValue(forKey id: UUID) -> Value? {
        items.removeValue(forKey: id)
    }

    mutating func removeAll() -> [Value] {
        let values = Array(items.values)
        items.removeAll()
        return values
    }
}
