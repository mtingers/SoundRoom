import Foundation

/// Owned by a peer's encoding worker. Large entity changes replace the document
/// through the same digest-verified pages used for the initial snapshot. Small
/// control patches retain their bounded incremental envelope.
struct RemoteWebDocumentState: Sendable {
    enum Output: Sendable { case frame(Data), snapshot(RemoteSnapshot) }
    private(set) var snapshot: RemoteSnapshot?
    var devices: [RemoteDevice] { snapshot?.devices ?? [] }

    mutating func encode(_ message: RemoteWireMessage) throws -> Output {
        try apply(message)
        return try encodeUpdated(message)
    }

    mutating func apply(_ message: RemoteWireMessage) throws {
        if case .event(let event) = message {
            switch event {
            case .snapshot(let document):
                snapshot = document
            case .delta(let delta):
                guard var document = snapshot else { throw RemoteError.invalidFrame }
                // A missing change cannot be repaired from one track replacement.
                guard delta.revision <= document.revision || delta.revision == document.revision + 1 else {
                    throw RemoteError.invalidFrame
                }
                if delta.revision < document.revision { return }
                guard document.apply(delta) else { throw RemoteError.invalidFrame }; snapshot = document
            case .transport(let transport): snapshot?.transport = transport
            case .devices(let devices, let input, let output):
                snapshot?.devices = devices; snapshot?.inputDeviceID = input; snapshot?.outputDeviceID = output
            case .presence(let peers): snapshot?.peers = peers
            case .status(let status): snapshot?.status = status
            case .waveform(let asset):
                if let index = snapshot?.assets.firstIndex(where: { $0.id == asset.id }) { snapshot?.assets[index] = asset }
                else { snapshot?.assets.append(asset) }
            default: break
            }
        }
    }

    func encodeUpdated(_ message: RemoteWireMessage) throws -> Output {
        if case .event(.snapshot) = message, let snapshot { return .snapshot(snapshot) }
        do { return .frame(try RemoteV3Codec.encodeMessage(message, devices: devices)) }
        catch RemoteError.oversizedFrame {
            guard case .event(.delta) = message, let snapshot else { throw RemoteError.oversizedFrame }
            return .snapshot(snapshot)
        }
    }
}
