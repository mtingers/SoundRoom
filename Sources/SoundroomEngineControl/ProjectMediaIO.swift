import Foundation
import SoundroomCore

/// Synchronous worker-only media operations. The engine owns project staging,
/// validation and recovery; a platform host provides the audio codec.
public protocol ProjectMediaIO: Sendable {
    func readAudio(from url: URL) throws -> AudioSamples
    func writeFloatAudio(_ samples: AudioSamples, to url: URL) throws
}

public struct ClosureProjectMediaIO: ProjectMediaIO {
    private let read: @Sendable (URL) throws -> AudioSamples
    private let write: @Sendable (AudioSamples, URL) throws -> Void
    public init(read: @escaping @Sendable (URL) throws -> AudioSamples,
                write: @escaping @Sendable (AudioSamples, URL) throws -> Void) {
        self.read = read; self.write = write
    }
    public func readAudio(from url: URL) throws -> AudioSamples { try read(url) }
    public func writeFloatAudio(_ samples: AudioSamples, to url: URL) throws { try write(samples, url) }
}

public enum ProjectStorageError: Error, LocalizedError, Sendable {
    case message(String)
    public var errorDescription: String? { switch self { case .message(let text): text } }
}
