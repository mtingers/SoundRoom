import Foundation

/// Transport-neutral message endpoint for native and WSS clients/servers.
/// All callbacks run off the audio thread; server admission, authentication,
/// identity and command scheduling belong to RemoteServer.
protocol RemoteServerChannel: AnyObject, Sendable {
    var id: UUID { get }
    var onReady: (@Sendable () -> Void)? { get set }
    var onMessage: (@Sendable (RemoteWireMessage, Int, @escaping @Sendable () -> Void) -> Void)? { get set }
    var onClose: (@Sendable (Error?) -> Void)? { get set }
    func start()
    func send(_ message: RemoteWireMessage, telemetryKey: String?)
    func close(_ error: Error?)
    func metrics(_ callback: @escaping @Sendable (RemoteNetworkMetrics) -> Void)
}

extension RemoteServerChannel {
    func send(_ message: RemoteWireMessage) { send(message, telemetryKey: nil) }
    func close() { close(nil) }
}
