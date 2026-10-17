import Darwin
import Foundation
import SoundroomCore
import SoundroomEngineControl

/// A downloaded project stays beside its destination until validation succeeds.
/// No source PCM is decoded: immutable WAVs are mapped for bounds validation.
enum ProjectPackageDownload {
    static func commit(_ archive: URL, to target: URL, replaceExisting: Bool) async throws -> Session {
        let work = Task.detached(priority: .utility) {
            let fm = FileManager.default
            let parent = target.deletingLastPathComponent()
            try fm.createDirectory(at: parent, withIntermediateDirectories: true)
            let staging = parent.appendingPathComponent(".soundroom-download-\(UUID().uuidString)", isDirectory: true)
            try fm.createDirectory(at: staging, withIntermediateDirectories: false)
            defer { try? fm.removeItem(at: staging) }
            let previous = try identity(target)
            if let previous {
                guard replaceExisting, previous.directory, !previous.link else {
                    throw SoundroomError.message(
                        "A file already exists at this destination. Choose a new project filename.")
                }
            }
            let originalMetadata = previous == nil ? nil : try metadata(target)
            let package = try ServerFiles.extractProjectArchive(archive, directory: staging)
            let document = try JSONDecoder().decode(Session.self, from: metadata(package))
            guard (1...11).contains(document.version) else {
                throw SoundroomError.message("This project requires a newer Soundroom version.")
            }
            var assets: [UUID: AudioSamples] = [:]
            for id in document.referencedAssetIDs {
                try Task.checkCancellation()
                let audio = package.appendingPathComponent("Audio")
                let upper = audio.appendingPathComponent("\(id.uuidString).wav")
                let file =
                    fm.fileExists(atPath: upper.path)
                    ? upper : audio.appendingPathComponent("\(id.uuidString.lowercased()).wav")
                assets[id] = try AudioSamples(mappedWaveFile: file)
            }
            try SoundroomEngineControl.ProjectPersistence.validate(session: document, assets: assets)
            if previous != nil {
                // Saving must not silently perform project media cleanup.
                let oldAudio = target.appendingPathComponent("Audio")
                let newAudio = package.appendingPathComponent("Audio")
                let audioValues = try oldAudio.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
                guard audioValues.isDirectory == true, audioValues.isSymbolicLink != true else {
                    throw SoundroomError.message("The saved project's Audio folder is not a regular directory.")
                }
                for file in try fm.contentsOfDirectory(
                    at: oldAudio, includingPropertiesForKeys: [.isRegularFileKey, .isSymbolicLinkKey])
                {
                    let values = try file.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey])
                    guard values.isRegularFile == true, values.isSymbolicLink != true else {
                        throw SoundroomError.message("The saved project's Audio folder contains an unsafe file.")
                    }
                    let destination = newAudio.appendingPathComponent(file.lastPathComponent)
                    if !fm.fileExists(atPath: destination.path) {
                        do { try fm.linkItem(at: file, to: destination) } catch {
                            try fm.copyItem(at: file, to: destination)
                        }
                    }
                }
            }
            try Task.checkCancellation()
            guard try identity(target) == previous,
                originalMetadata == (previous == nil ? nil : try metadata(target))
            else {
                throw SoundroomError.message(
                    "The project on disk changed during saving. Save a copy to keep both versions.")
            }
            let result = package.withUnsafeFileSystemRepresentation { source in
                target.withUnsafeFileSystemRepresentation { destination in
                    renamex_np(source!, destination!, UInt32(previous == nil ? RENAME_EXCL : RENAME_SWAP))
                }
            }
            guard result == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
            return document
        }
        return try await withTaskCancellationHandler(operation: { try await work.value }, onCancel: { work.cancel() })
    }
    private struct Identity: Equatable {
        let device: UInt64, inode: UInt64
        let directory: Bool, link: Bool
    }
    private static func identity(_ url: URL) throws -> Identity? {
        do {
            let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
            return Identity(
                device: (attributes[.systemNumber] as? NSNumber)?.uint64Value ?? 0,
                inode: (attributes[.systemFileNumber] as? NSNumber)?.uint64Value ?? 0,
                directory: attributes[.type] as? FileAttributeType == .typeDirectory,
                link: attributes[.type] as? FileAttributeType == .typeSymbolicLink)
        } catch let error as NSError where error.domain == NSCocoaErrorDomain && error.code == NSFileReadNoSuchFileError
        { return nil }
    }
    private static func metadata(_ package: URL) throws -> Data {
        let url = package.appendingPathComponent("session.json")
        let values = try url.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey])
        guard values.isRegularFile == true, values.isSymbolicLink != true, let size = values.fileSize,
            size <= 64 * 1024 * 1024
        else {
            throw SoundroomError.message("The project's session metadata is missing or invalid.")
        }
        return try Data(contentsOf: url)
    }
}
