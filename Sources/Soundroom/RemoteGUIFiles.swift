import Foundation
import AppKit
import UniformTypeIdentifiers
import CryptoKit
import SoundroomCore
import SoundroomRemote

private actor RemoteFileReader {
    let handle: FileHandle
    init(_ url: URL) throws { handle = try FileHandle(forReadingFrom: url) }
    func digest() throws -> (Int, String) {
        try handle.seek(toOffset: 0); var hash = SHA256(); var bytes = 0
        while let chunk = try handle.read(upToCount: RemoteProtocol.maximumUploadChunkBytes), !chunk.isEmpty {
            try Task.checkCancellation(); hash.update(data: chunk); bytes += chunk.count
        }
        try handle.seek(toOffset: 0)
        return (bytes, hash.finalize().map { String(format: "%02x", $0) }.joined())
    }
    func read() throws -> Data { try handle.read(upToCount: RemoteProtocol.maximumUploadChunkBytes) ?? Data() }
    func close() { try? handle.close() }
}

private actor RemoteFileWriter {
    let handle: FileHandle
    private var hash = SHA256()
    private var bytes = 0
    init(_ url: URL) throws {
        guard FileManager.default.createFile(atPath: url.path, contents: nil) else { throw CocoaError(.fileWriteUnknown) }
        handle = try FileHandle(forWritingTo: url)
    }
    func append(_ data: Data) throws { try handle.write(contentsOf: data); hash.update(data: data); bytes += data.count }
    func finish(bytes expected: Int, sha256: String) throws {
        try handle.synchronize(); try handle.close()
        let digest = hash.finalize().map { String(format: "%02x", $0) }.joined()
        guard bytes == expected, digest == sha256 else { throw SoundroomError.message("The exported file failed its integrity check. Download it again.") }
    }
}

extension RemoteGUIController {
    func importAudio(urls: [URL]? = nil) {
        if let urls { uploadFiles(urls); return }
        let panel = NSOpenPanel(); panel.allowedContentTypes = [.audio]; panel.allowsMultipleSelection = true
        panel.canChooseFiles = true; panel.canChooseDirectories = false
        present(panel) { [weak self] response in if response == .OK { self?.uploadFiles(panel.urls) } }
    }
    func importProjectFile() {
        let chooser = ProjectOpenPanel()
        chooser.begin(for: NSApp.keyWindow) { [weak self] url in
            if let url { self?.openProject(url: url) }
        }
    }
    func openProject(url: URL?) {
        if let url { uploadFiles([url]); return }
        showProjectPicker = true
        Task { [weak self] in
            guard let self else { return }
            do {
                let reply = try await client.request(.listProjects)
                if let data = reply.payload, let names = try? JSONDecoder().decode([String].self, from: data) { projects = names }
                if !reply.accepted { store?.status = reply.message }
            } catch { store?.status = error.localizedDescription }
        }
    }
    func openServerProject(_ id: String) {
        showProjectPicker = false
        send(.openProject(id: id), expectedRevision: revision) { [weak self] reply in
            if reply.accepted { self?.adoptProjectDestination(serverName: id, file: nil); self?.store?.rememberRecent(.server(id)) }
        }
    }
    func saveProject(saveAs: Bool) {
        guard let store, connected, !isSavingProject else { return }
        if !saveAs, let file = projectFileURL { saveProjectFile(to: file, replaceExisting: true); return }
        if !saveAs, let name = serverProjectName { saveServerProject(name: name, replaceExisting: true); return }
        if isLocalServer || projectFileURL != nil {
            guard let selected = store.chooseProjectSaveDestination() else { return }
            let target = selected.pathExtension == "soundroom" ? selected : selected.appendingPathExtension("soundroom")
            saveProjectFile(to: target, replaceExisting: FileManager.default.fileExists(atPath: target.path)); return
        }
        let openingRevision = revision, openingGeneration = connectionGeneration
        let alert = NSAlert(); alert.messageText = "Save a new server project"
        alert.informativeText = "The project and audio are stored on \(serverName)."
        let field = NSTextField(string: store.session.name); field.frame = NSRect(x: 0, y: 0, width: 310, height: 24)
        alert.accessoryView = field; alert.addButton(withTitle: "Save"); alert.addButton(withTitle: "Cancel")
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        guard connected, connectionGeneration == openingGeneration else { store.status = "Connection changed. Reopen Save for the current session."; return }
        do {
            let name = try SessionName.projectTitle(field.stringValue)
            guard revision == openingRevision else { store.status = "The session changed. Reopen Save for the current session."; return }
            saveServerProject(name: name, replaceExisting: false)
        }
        catch { store.status = error.localizedDescription }
    }
    func saveServerProject(name: String, replaceExisting: Bool) {
        guard connected, !isSavingProject else { return }
        isSavingProject = true
        let navigation = projectNavigationGeneration
        send(.saveProject(name: name, replaceExisting: replaceExisting)) { [weak self] reply in
            guard let self else { return }; isSavingProject = false
            if reply.accepted, navigation == projectNavigationGeneration {
                adoptProjectDestination(serverName: name, file: nil)
                store?.rememberRecent(.server(serverProjectName ?? name))
                store?.status = "Saved on audio server: \(serverProjectName ?? name)"
            }
        }
    }
    func saveProjectFile(to target: URL, replaceExisting: Bool) {
        guard connected, !isSavingProject else { return }
        isSavingProject = true
        store?.status = "Saving \(target.lastPathComponent)…"
        let generation = connectionGeneration, navigation = projectNavigationGeneration
        send(.editor(.init(operation: "project.package"))) { [weak self] reply in
            guard let self else { return }
            guard reply.accepted else { isSavingProject = false; return }
            guard let payload = reply.payload else { isSavingProject = false; store?.recordFailure("Save failed: the audio server did not return a project."); return }
            Task { [weak self] in
                guard let self else { return }
                defer { isSavingProject = false }
                let archive = FileManager.default.temporaryDirectory.appendingPathComponent("\(UUID().uuidString).soundroom.zip")
                defer { try? FileManager.default.removeItem(at: archive) }
                do {
                    let artifact = try JSONDecoder().decode(RemoteArtifact.self, from: payload)
                    defer { Task { _ = try? await self.requestFile(.editor(.init(operation: "project.package", strings: ["release": artifact.id.uuidString]))) } }
                    try await download(artifact, to: archive)
                    try requireImportConnection(generation)
                    guard navigation == projectNavigationGeneration else { throw SoundroomError.message("Another project was opened. The previous save was cancelled.") }
                    let written = try await ProjectPackageDownload.commit(archive, to: target, replaceExisting: replaceExisting)
                    guard generation == connectionGeneration, navigation == projectNavigationGeneration else { return }
                    adoptProjectDestination(serverName: serverProjectName, file: target)
                    confirmProjectFileSaved(written, at: target)
                } catch { store?.recordFailure("Save failed: \(error.localizedDescription)") }
            }
        }
    }
    func exportMix() {
        guard let store else { return }
        let panel = NSSavePanel()
        let stems = store.exportTracks
        panel.allowedContentTypes = stems ? [] : [UTType(filenameExtension: store.exportFormat) ?? .audio]
        panel.nameFieldStringValue = store.session.name + (stems ? " Stems" : "." + store.exportFormat)
        let format = store.exportFormat, tail = store.exportIncludeTails, selection = store.exportSelection
        let trackIDs = Array(store.exportTrackIDs), context = store.remoteContext
        present(panel) { [weak self, weak store] response in
            guard response == .OK, let destination = panel.url, let self else { return }
            store?.showExport = false
            Task { [weak self] in
                guard let self else { return }
                do {
                    let reply = try await client.request(.editor(.init(operation: "project.export", context: context,
                        strings: ["format": format, "name": destination.lastPathComponent], flags: ["tracks": stems, "tails": tail, "selection": selection], payload: try? JSONEncoder().encode(trackIDs))), timeout: 600)
                    guard reply.accepted, let payload = reply.payload else { throw SoundroomError.message(reply.message) }
                    if let artifact = try? JSONDecoder().decode(RemoteArtifact.self, from: payload) {
                        try await download(artifact, to: destination)
                    } else {
                        let artifacts = try JSONDecoder().decode([RemoteArtifact].self, from: payload)
                        guard stems, !artifacts.isEmpty else { throw SoundroomError.message("The server returned no export files.") }
                        guard !FileManager.default.fileExists(atPath: destination.path) else { throw SoundroomError.message("Choose a new folder for the stem export.") }
                        try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
                        for artifact in artifacts {
                            guard artifact.name == URL(fileURLWithPath: artifact.name).lastPathComponent else { throw SoundroomError.message("The export filename is invalid.") }
                            try await download(artifact, to: destination.appendingPathComponent(artifact.name))
                        }
                    }
                    store?.status = stems ? "Tracks exported" : "Mix exported"
                } catch { store?.status = "Export failed: \(error.localizedDescription)" }
            }
        }
    }
    func exportDrumPattern(_ pattern: DrumPattern, to url: URL, completion: @escaping (String) -> Void) {
        Task { [weak self] in
            guard let self else { return }
            do {
                let reply = try await client.request(.editor(.init(operation: "drums.export", context: store?.remoteContext ?? .init(), payload: try JSONEncoder().encode(pattern))), timeout: 600)
                guard reply.accepted, let payload = reply.payload else { throw SoundroomError.message(reply.message) }
                let artifact = try JSONDecoder().decode(RemoteArtifact.self, from: payload)
                try await download(artifact, to: url); completion("24-bit WAV exported")
            } catch { completion(error.localizedDescription) }
        }
    }
    func download(_ artifact: RemoteArtifact, to destination: URL, parallelism: Int = 3) async throws {
        guard artifact.bytes >= 0, artifact.bytes <= 2 * 1024 * 1024 * 1024 else { throw RemoteError.invalidFrame }
        let temporary = destination.deletingLastPathComponent().appendingPathComponent(".soundroom-download-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: temporary) }
        let writer = try RemoteFileWriter(temporary)
        let chunkBytes = RemoteProtocol.maximumUploadChunkBytes
        // Keep one ordinary server job slot free. The window includes completed
        // out-of-order chunks, so a stalled first chunk cannot grow memory.
        let window = max(1, min(3, parallelism)) * chunkBytes
        try await withThrowingTaskGroup(of: RemoteArtifactChunk.self) { group in
            var scheduled = 0, written = 0, received: [Int: Data] = [:]
            while scheduled < min(artifact.bytes, window) {
                let offset = scheduled
                group.addTask { try await self.downloadChunk(artifact, at: offset) }
                scheduled += min(chunkBytes, artifact.bytes - scheduled)
            }
            while let chunk = try await group.next() {
                try Task.checkCancellation()
                received[chunk.offset] = chunk.data
                while let data = received.removeValue(forKey: written) {
                    try await writer.append(data); written += data.count
                }
                while scheduled < artifact.bytes && scheduled < written + window {
                    let offset = scheduled
                    group.addTask { try await self.downloadChunk(artifact, at: offset) }
                    scheduled += min(chunkBytes, artifact.bytes - scheduled)
                }
            }
            guard written == artifact.bytes else { throw RemoteError.invalidFrame }
        }
        try await writer.finish(bytes: artifact.bytes, sha256: artifact.sha256)
        if FileManager.default.fileExists(atPath: destination.path) { _ = try FileManager.default.replaceItemAt(destination, withItemAt: temporary) }
        else { try FileManager.default.moveItem(at: temporary, to: destination) }
    }
    private func downloadChunk(_ artifact: RemoteArtifact, at offset: Int) async throws -> RemoteArtifactChunk {
        try Task.checkCancellation()
        let count = min(RemoteProtocol.maximumUploadChunkBytes, artifact.bytes - offset)
        let reply = try await requestFile(.downloadArtifact(id: artifact.id, offset: offset, bytes: count))
        try Task.checkCancellation()
        guard reply.accepted, let payload = reply.payload else { throw SoundroomError.message(reply.message) }
        let chunk = try JSONDecoder().decode(RemoteArtifactChunk.self, from: payload)
        guard chunk.offset == offset, chunk.data.count == count, chunk.complete == (offset + count == artifact.bytes) else { throw RemoteError.invalidFrame }
        return chunk
    }
    @discardableResult func uploadFiles(_ urls: [URL]) -> Task<Void, Never>? {
        guard connected, let store, let first = urls.first else { return nil }
        let progress = store.fileTransfer
        guard !progress.isActive else { store.status = "An import is already in progress."; return nil }
        var context = store.remoteContext
        if store.currentTrack?.acceptsClips == false { context.trackID = nil; context.clipID = nil }
        let generation = connectionGeneration
        let transfer = progress.begin(name: first.lastPathComponent)
        return Task { [weak self, weak store] in
            guard let self else { progress.finish(transfer); return }
            defer { progress.finish(transfer) }
            do {
                for original in urls {
                    try requireImportConnection(generation)
                    var file = original; var archive: URL?
                    // Register cleanup before preparation: failed archives can
                    // otherwise leave a large partial ZIP in temporary storage.
                    defer { if let archive { try? FileManager.default.removeItem(at: archive) } }
                    store?.status = "Importing \(original.lastPathComponent)…"
                    progress.update(transfer, name: original.lastPathComponent, phase: .preparing)
                    if original.pathExtension.lowercased() == "soundroom" {
                        let zip = FileManager.default.temporaryDirectory.appendingPathComponent("\(UUID().uuidString).soundroom.zip")
                        archive = zip
                        try await ProjectUploadArchive.create(original, to: zip, isLocal: isLocalServer)
                        file = zip
                    }
                    try requireImportConnection(generation)
                    progress.update(transfer, name: original.lastPathComponent, phase: .checking)
                    let reader = try RemoteFileReader(file)
                    let id = UUID()
                    do {
                        let (size, digest) = try await reader.digest()
                        try requireImportConnection(generation)
                        let name = archive == nil ? original.lastPathComponent : original.lastPathComponent + ".zip"
                        let reply = try await requestFile(.beginUpload(id: id, name: name, bytes: size, sha256: digest))
                        guard reply.accepted else { throw SoundroomError.message(reply.message) }
                        try requireImportConnection(generation)
                        progress.update(transfer, name: original.lastPathComponent, phase: .uploading, total: size)
                        var offset = 0
                        while offset < size {
                            try Task.checkCancellation()
                            var chunks: [Data] = [], next = offset
                            while chunks.count < 3 && next < size {
                                let chunk = try await reader.read()
                                guard !chunk.isEmpty, chunk.count <= size - next else { throw RemoteError.invalidFrame }
                                chunks.append(chunk); next += chunk.count
                            }
                            try requireImportConnection(generation)
                            let replies = try await requestUploadBatch(id: id, offset: offset, chunks: chunks)
                            guard replies.count == chunks.count, replies.allSatisfy(\.accepted) else {
                                throw SoundroomError.message(replies.first(where: { !$0.accepted })?.message ?? "The upload acknowledgment is incomplete.")
                            }
                            try Task.checkCancellation(); try requireImportConnection(generation)
                            offset = next
                            progress.update(transfer, name: original.lastPathComponent, phase: .uploading, bytes: offset, total: size)
                        }
                        await reader.close()
                    } catch { await reader.close(); throw error }
                    progress.update(transfer, name: original.lastPathComponent, phase: .opening)
                    try requireImportConnection(generation)
                    let reply = try await requestFile(.finishUpload(id: id, trackID: context.trackID, frame: context.cursorFrame), timeout: 600)
                    guard reply.accepted else { throw SoundroomError.message(reply.message) }
                    try requireImportConnection(generation)
                    store?.status = reply.message.isEmpty ? "Imported \(original.lastPathComponent)" : reply.message
                    if archive != nil {
                        let identity = reply.payload.flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: String] }
                        openedProjectDestination(serverName: identity?["projectName"] ?? serverProjectName, file: original, projectID: identity?["projectID"].flatMap(UUID.init(uuidString:)))
                        store?.rememberRecent(.file(original))
                    }
                }
            } catch {
                if connectionGeneration == generation { store?.status = "Import failed: \(error.localizedDescription)" }
            }
        }
    }
    private func present(_ panel: NSSavePanel, completion: @escaping (NSApplication.ModalResponse) -> Void) {
        if let window = NSApp.keyWindow { panel.beginSheetModal(for: window, completionHandler: completion) }
        else { panel.begin(completionHandler: completion) }
    }
}
