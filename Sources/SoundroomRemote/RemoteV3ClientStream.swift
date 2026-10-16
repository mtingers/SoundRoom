import Foundation

/// Worker-owned v3 input state. Installs complete verified documents atomically;
/// telemetry remains responsive during pagination and is replayed after install.
struct RemoteV3ClientStream {
    private var assembler = RemoteSnapshotAssembler()
    private var pinnedRevision: Int?
    private var installed = false
    private var deltas: [(RemoteWireMessage, Int, UInt64)] = []
    private var telemetry: [String: (RemoteWireMessage, Int, UInt64)] = [:]
    private var arrival: UInt64 = 0
    private(set) var capabilities: RemoteCapabilities?
    private(set) var devices: [RemoteDevice] = []
    private(set) var retainedBytes = 0
    var retainedDocumentBytes: Int { assembler.nextOffset + retainedBytes }

    mutating func accept(_ data: Data) throws -> [RemoteWireMessage] {
        guard data.count <= RemoteProtocol.maximumFrameBytes,
              let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              let kind = object["kind"] as? String else { throw RemoteError.invalidFrame }
        if kind == "capabilities" { capabilities = try RemoteV3Codec.decodeCapabilities(data); return [] }
        if kind.hasPrefix("snapshot") {
            try RemoteV3ModelValidator.validateJSON(data, type: "RemoteSnapshotPage")
            let page = try JSONDecoder().decode(RemoteSnapshotPage.self, from: data)
            if page.kind == .snapshotBegin {
                guard pinnedRevision == nil else { throw RemoteError.invalidFrame }
                pinnedRevision = page.revision
            }
            guard let snapshot = try assembler.accept(page) else { return [] }
            pinnedRevision = nil
            return install(snapshot)
        }
        let message = try RemoteV3Codec.decodeMessage(data, devices: devices)
        arrival &+= 1
        if case .event(.devices(let value, _, _)) = message { devices = value }
        if case .event(.snapshot(let snapshot)) = message {
            guard pinnedRevision == nil else { throw RemoteError.invalidFrame }
            return install(snapshot)
        }
        if pinnedRevision != nil || !installed {
            if case .event(.delta) = message {
                guard deltas.count < 128 else { throw RemoteError.queueFull }
                try retain(data.count); deltas.append((message, data.count, arrival)); return []
            }
            let key: String?
            switch message {
            case .event(.transport): key = "transport"
            case .event(.devices): key = "devices"
            case .event(.presence): key = "presence"
            case .event(.status): key = "status"
            default: key = nil
            }
            if let key {
                if let old = telemetry.removeValue(forKey: key) { retainedBytes -= old.1 }
                try retain(data.count); telemetry[key] = (message, data.count, arrival)
            }
        }
        return [message]
    }
    private mutating func install(_ snapshot: RemoteSnapshot) -> [RemoteWireMessage] {
        devices = snapshot.devices; installed = true
        var messages: [RemoteWireMessage] = [.event(.snapshot(snapshot))]
        var latestDeltaStatusArrival: UInt64 = 0
        for (message, _, sequence) in deltas {
            if case .event(.delta(let delta)) = message, delta.revision > snapshot.revision {
                messages.append(message)
                if delta.status != nil { latestDeltaStatusArrival = sequence }
            }
        }
        for key in telemetry.keys.sorted() {
            if key == "status", telemetry[key]!.2 < latestDeltaStatusArrival { continue }
            let message = telemetry[key]!.0
            if case .event(.devices(let next, _, _)) = message { devices = next }
            messages.append(message)
        }
        deltas = []; telemetry = [:]; retainedBytes = 0
        return messages
    }
    private mutating func retain(_ count: Int) throws {
        guard retainedBytes <= RemoteProtocol.maximumQueuedBytes - count else { throw RemoteError.queueFull }
        retainedBytes += count
    }
}
