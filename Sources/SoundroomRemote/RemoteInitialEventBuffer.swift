import Foundation

/// Events produced while an asynchronous connection snapshot is being prepared
/// follow that base document. Reliable edits stay ordered; telemetry coalesces.
struct RemoteInitialEventBuffer {
    private var snapshotQueued = false
    private var pending = RemoteMessageMailbox()

    mutating func submit(_ event: RemoteEvent, telemetryKey: String?, send: (RemoteWireMessage, String?) -> Void) throws
    {
        if snapshotQueued {
            send(.event(event), telemetryKey)
        } else {
            try pending.append(.event(event), telemetryKey: telemetryKey)
        }
    }

    mutating func install(_ snapshot: RemoteSnapshot, send: (RemoteWireMessage, String?) -> Void) {
        send(.event(.snapshot(snapshot)), nil)
        snapshotQueued = true
        while let item = pending.next() { send(item.message, item.telemetryKey) }
    }
}
