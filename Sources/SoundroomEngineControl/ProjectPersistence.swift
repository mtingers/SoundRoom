import Darwin
import Foundation
import SoundroomCore

public struct LoadedProject: Sendable {
    public var session: Session
    public var assets: [UUID: AudioSamples]
    public init(session: Session, assets: [UUID: AudioSamples]) {
        self.session = session
        self.assets = assets
    }
}

public struct ProjectSaveReport: Sendable {
    public var encodedAssets: Int
    public var reusedAssets: Int
    public var bytes: Int64
}

/// Packages are staged beside their destination and committed with one atomic rename.
/// Asset identities designate immutable source audio; unchanged assets are hard-linked.
public enum ProjectPersistence {
    private struct MediaInfo: Codable {
        var frames: Int
        var channels: Int
    }
    private struct ProjectHeader: Decodable { var version: Int? }

    public static func save(session: Session, assets: [UUID: AudioSamples], to target: URL, media: any ProjectMediaIO)
        async throws -> ProjectSaveReport
    {
        try Task.checkCancellation()
        let worker = Task.detached(priority: .utility) {
            try saveSynchronously(session: session, assets: assets, to: target, media: media)
        }
        return try await withTaskCancellationHandler(
            operation: { try await worker.value }, onCancel: { worker.cancel() })
    }

    public static func load(from target: URL, media: any ProjectMediaIO) async throws -> LoadedProject {
        try Task.checkCancellation()
        let worker = Task.detached(priority: .userInitiated) { try loadSynchronously(from: target, media: media) }
        return try await withTaskCancellationHandler(
            operation: { try await worker.value }, onCancel: { worker.cancel() })
    }

    @discardableResult public static func saveSynchronously(
        session: Session, assets: [UUID: AudioSamples], to target: URL, recoveryMetadata: Data? = nil,
        media: any ProjectMediaIO
    ) throws -> ProjectSaveReport {
        let manager = FileManager.default
        try validate(session: session, assets: assets)
        try Task.checkCancellation()
        try manager.createDirectory(at: target.deletingLastPathComponent(), withIntermediateDirectories: true)
        let staging = target.deletingLastPathComponent().appendingPathComponent(
            ".soundroom-stage-\(UUID().uuidString)", isDirectory: true)
        defer { try? manager.removeItem(at: staging) }
        let audio = staging.appendingPathComponent("Audio", isDirectory: true)
        try manager.createDirectory(at: audio, withIntermediateDirectories: true)
        let oldInfo =
            (try? JSONDecoder().decode(
                [UUID: MediaInfo].self, from: Data(contentsOf: target.appendingPathComponent("media.json")))) ?? [:]
        var info: [UUID: MediaInfo] = [:]
        var report = ProjectSaveReport(encodedAssets: 0, reusedAssets: 0, bytes: 0)
        for (id, samples) in assets {
            try Task.checkCancellation()
            guard samples.channelCount > 0, samples.hasConsistentChannels else {
                throw ProjectStorageError.message("An audio source has inconsistent channels.")
            }
            let name = "\(id.uuidString).wav"
            let destination = audio.appendingPathComponent(name)
            let existing = target.appendingPathComponent("Audio").appendingPathComponent(name)
            let previous = oldInfo[id]
            if previous?.frames == samples.count, previous?.channels == samples.channelCount,
                manager.fileExists(atPath: existing.path), (try? manager.linkItem(at: existing, to: destination)) != nil
            {
                report.reusedAssets += 1
            } else {
                // Aligned takes can be lazy views of resident PCM too. Always
                // stream bounded blocks instead of materializing channel arrays.
                try media.writeFloatAudio(samples, to: destination)
                report.encodedAssets += 1
            }
            info[id] = MediaInfo(frames: samples.count, channels: samples.channelCount)
            let attributes = try manager.attributesOfItem(atPath: destination.path)
            report.bytes += (attributes[.size] as? NSNumber)?.int64Value ?? 0
        }
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        var document = session
        document.version = session.requiredProjectVersion
        let sessionData = try encoder.encode(document)
        let infoData = try encoder.encode(info)
        try sessionData.write(to: staging.appendingPathComponent("session.json"))
        try infoData.write(to: staging.appendingPathComponent("media.json"))
        report.bytes += Int64(sessionData.count + infoData.count)
        if let recoveryMetadata {
            try recoveryMetadata.write(to: staging.appendingPathComponent("recovery.json"))
            report.bytes += Int64(recoveryMetadata.count)
        }
        try Task.checkCancellation()
        try commitDirectory(staging, to: target)
        return report
    }

    public static func loadSynchronously(from target: URL, media: any ProjectMediaIO) throws -> LoadedProject {
        let data = try Data(contentsOf: target.appendingPathComponent("session.json"))
        let decoder = JSONDecoder()
        // Check compatibility before decoding instruments/effects introduced in
        // later formats, which otherwise produce an unhelpful Foundation error.
        let version: Int
        do { version = try decoder.decode(ProjectHeader.self, from: data).version ?? 1 } catch is DecodingError {
            throw ProjectStorageError.message(
                "The project's session metadata could not be read. It may be damaged or use an unsupported Soundroom format."
            )
        }
        guard (1...11).contains(version) else {
            throw ProjectStorageError.message(
                version > 11
                    ? "This project uses a newer Soundroom format (\(version)). Update Soundroom to open it."
                    : "This project has an invalid Soundroom format version (\(version)).")
        }
        let document: Session
        do { document = try decoder.decode(Session.self, from: data) } catch is DecodingError {
            throw ProjectStorageError.message(
                "This project's audio or instrument data could not be read. Open it with the latest Soundroom build; if it still fails, restore a saved project copy."
            )
        }
        var assets: [UUID: AudioSamples] = [:]
        for file in try FileManager.default.contentsOfDirectory(
            at: target.appendingPathComponent("Audio"), includingPropertiesForKeys: nil, options: .skipsHiddenFiles)
        {
            try Task.checkCancellation()
            if file.pathExtension.lowercased() == "wav",
                let id = UUID(uuidString: file.deletingPathExtension().lastPathComponent)
            {
                assets[id] = try media.readAudio(from: file)
            }
        }
        try validate(session: document, assets: assets)
        return LoadedProject(session: document, assets: assets)
    }

    public static func validate(session: Session, assets: [UUID: AudioSamples]) throws {
        _ = try TrackRoutingPlan(session: session)
        guard session.masterTrack.realtimeEffects.reduce(0, { $0 + $1.latencyFrames }) <= 4096 else {
            throw ProjectStorageError.message("The master rack exceeds the 85 ms processing look-ahead budget.")
        }
        for clip in session.referencedClips {
            guard clip.start >= 0, clip.offset >= 0, clip.length > 0,
                let audio = assets[clip.assetID], audio.channelCount > 0,
                audio.hasConsistentChannels,
                clip.offset <= audio.count, clip.length <= audio.count - clip.offset,
                clip.start <= Int.max - clip.length
            else {
                throw ProjectStorageError.message("The project contains an invalid or missing audio source.")
            }
            for state in clip.previousStates ?? [] {
                guard let source = assets[state.assetID], source.channelCount > 0,
                    source.hasConsistentChannels,
                    state.offset >= 0, state.length > 0, state.offset <= source.count,
                    state.length <= source.count - state.offset
                else {
                    throw ProjectStorageError.message("The project contains an invalid or missing clip history source.")
                }
            }
        }
    }

    /// Existing nonempty directories cannot be replaced by ordinary rename. macOS
    /// RENAME_SWAP atomically exchanges them; the old package remains in staging.
    private static func commitDirectory(_ staging: URL, to target: URL) throws {
        let exists = FileManager.default.fileExists(atPath: target.path)
        let result = staging.withUnsafeFileSystemRepresentation { source in
            target.withUnsafeFileSystemRepresentation { destination in
                exists ? renamex_np(source!, destination!, UInt32(RENAME_SWAP)) : rename(source!, destination!)
            }
        }
        guard result == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    }
}

public struct RecoverySnapshot: Sendable {
    public var project: LoadedProject
    public var projectURL: URL?
    public var revision: UUID
    public var date: Date
}

/// Explicitly enabled by the app, never by model/test-store construction.
public actor RecoveryStore {
    struct Metadata: Codable {
        var projectPath: String?
        var revision: UUID
        var date: Date
    }
    public let directory: URL
    private let media: any ProjectMediaIO
    private var pending: Task<Void, Never>?
    private var pendingRevision: UUID?
    private var lastFailure: Error?

    public init(directory: URL = RecoveryStore.defaultDirectory, media: any ProjectMediaIO) {
        self.directory = directory
        self.media = media
    }
    public static var defaultDirectory: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Soundroom/Recovery", isDirectory: true)
    }
    public var snapshotURL: URL { directory.appendingPathComponent("Unsaved.soundroom", isDirectory: true) }

    public func schedule(
        session: Session, assets: [UUID: AudioSamples], projectURL: URL?, revision: UUID, delay: TimeInterval = 2
    ) {
        pending?.cancel()
        pendingRevision = revision
        pending = Task { [weak self] in
            do {
                try await Task.sleep(nanoseconds: UInt64(max(0, delay) * 1_000_000_000))
                try Task.checkCancellation()
                try await self?.writeScheduled(
                    session: session, assets: assets, projectURL: projectURL, revision: revision)
            } catch is CancellationError {} catch { await self?.recordFailure(error) }
        }
    }
    private func recordFailure(_ error: Error) { lastFailure = error }
    public func failureDescription() -> String? { lastFailure?.localizedDescription }

    /// Call before starting a take or before a controlled quit to flush immediately.
    public func write(session: Session, assets: [UUID: AudioSamples], projectURL: URL?, revision: UUID) async throws {
        pending?.cancel()
        pending = nil
        pendingRevision = nil
        try commitSnapshot(session: session, assets: assets, projectURL: projectURL, revision: revision)
    }
    private func writeScheduled(session: Session, assets: [UUID: AudioSamples], projectURL: URL?, revision: UUID) throws
    {
        try Task.checkCancellation()
        pending = nil
        pendingRevision = nil
        try commitSnapshot(session: session, assets: assets, projectURL: projectURL, revision: revision)
    }
    private func commitSnapshot(session: Session, assets: [UUID: AudioSamples], projectURL: URL?, revision: UUID) throws
    {
        let metadata = Metadata(projectPath: projectURL?.path, revision: revision, date: Date())
        // Stay on this actor while committing so discard/new snapshots cannot interleave.
        _ = try ProjectPersistence.saveSynchronously(
            session: session, assets: assets, to: snapshotURL, recoveryMetadata: JSONEncoder().encode(metadata),
            media: media)
        lastFailure = nil
    }
    public func recover() async throws -> RecoverySnapshot? {
        guard FileManager.default.fileExists(atPath: snapshotURL.path) else { return nil }
        let project = try ProjectPersistence.loadSynchronously(from: snapshotURL, media: media)
        let metadata = try? JSONDecoder().decode(
            Metadata.self, from: Data(contentsOf: snapshotURL.appendingPathComponent("recovery.json")))
        return RecoverySnapshot(
            project: project, projectURL: metadata?.projectPath.map { URL(fileURLWithPath: $0) },
            revision: metadata?.revision ?? UUID(), date: metadata?.date ?? Date())
    }
    public func discard() throws {
        pending?.cancel()
        pending = nil
        pendingRevision = nil
        if FileManager.default.fileExists(atPath: directory.path) { try FileManager.default.removeItem(at: directory) }
        lastFailure = nil
    }
    public func clearIfRevision(_ revision: UUID) throws {
        if pendingRevision == revision {
            pending?.cancel()
            pending = nil
            pendingRevision = nil
        }
        guard
            let metadata = try? JSONDecoder().decode(
                Metadata.self, from: Data(contentsOf: snapshotURL.appendingPathComponent("recovery.json"))),
            metadata.revision == revision
        else { return }
        if FileManager.default.fileExists(atPath: snapshotURL.path) {
            try FileManager.default.removeItem(at: snapshotURL)
        }
    }
}
