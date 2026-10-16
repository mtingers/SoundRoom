import AppKit
import SwiftUI
import SoundroomCore
import SoundroomRemote

enum SeparatedSource: String, CaseIterable, Identifiable {
    case vocals, drums, bass, other, guitar, piano
    var id: Self { self }
    var name: String {
        switch self {
        case .vocals: return "Vocals"
        case .drums: return "Drums"
        case .bass: return "Bass"
        case .other: return "Other instruments"
        case .guitar: return "Guitar"
        case .piano: return "Piano (experimental)"
        }
    }
    var needsSixSources: Bool { self == .guitar || self == .piano }
}

enum SourceSeparationQuality: String, CaseIterable, Identifiable {
    case quality, standard, extended
    var id: Self { self }
    var name: String {
        switch self {
        case .quality: return "Highest quality · four sources"
        case .standard: return "Faster · four sources"
        case .extended: return "Six sources · experimental piano"
        }
    }
    var modelID: String {
        switch self {
        case .quality: return "htdemucs_ft"
        case .standard: return "htdemucs"
        case .extended: return "htdemucs_6s"
        }
    }
}

enum SourceSeparationOperation: String, CaseIterable, Identifiable {
    case extract, remove
    var id: Self { self }
    var name: String { self == .extract ? "Keep source" : "Remove source" }
}

private struct SourceSeparationRuntime: Decodable {
    struct Model: Decodable { var name: String?; var available: Bool }
    var available: Bool
    var models: Set<String>
    var message: String
    private enum CodingKeys: CodingKey { case available, models, message }
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        available = try container.decode(Bool.self, forKey: .available)
        message = try container.decode(String.self, forKey: .message)
        if let names = try? container.decode([String].self, forKey: .models) { models = Set(names) }
        else {
            let entries = try container.decode([String: Model].self, forKey: .models)
            models = Set(entries.filter { $0.value.available }.keys)
            for entry in entries.values where entry.available { if let name = entry.name { models.insert(name) } }
        }
    }
}

/// The sheet owns its opening audio range and revision. Timeline navigation,
/// model choices, and processing failures cannot redirect it to different audio.
@MainActor final class SourceSeparationController: ObservableObject, Identifiable {
    private static let previewFrames = 10 * 60 * Int(sessionRate)
    let id = UUID()
    let trackName: String
    let range: Range<Int>
    let context: RemoteEditorContext
    let source: RemoteWorkspaceSource
    private weak var store: SessionStore?
    @Published var target: SeparatedSource = .vocals {
        didSet { if target.needsSixSources { quality = .extended } }
    }
    @Published var quality: SourceSeparationQuality = .quality
    @Published var operation: SourceSeparationOperation = .extract
    @Published private(set) var checkingRuntime = false
    @Published private(set) var runtimeAvailable = false
    @Published private(set) var runtimeMessage = "Checking the local separation engine…"
    @Published private(set) var pending = false
    @Published private(set) var previewing = false
    @Published private(set) var completed = false
    @Published private(set) var cancellationRequested = false
    @Published private(set) var message = ""
    private var availableModels: Set<String> = []
    private var processingID: UUID?
    private var processingFence: RemoteGestureRevisionFence?

    private init(store: SessionStore, remote: RemoteGUIController, track: Track, range: Range<Int>) {
        self.store = store; trackName = track.name; self.range = range
        source = RemoteWorkspaceSource(remote: remote, trackID: track.id, clipID: store.currentClip?.id, frame: range.lowerBound)
        context = .init(trackID: track.id, clipID: store.currentClip?.id,
                        clipIDs: store.currentClip.map { [$0.id] } ?? [], cursorFrame: range.lowerBound,
                        rangeStart: range.lowerBound, rangeEnd: range.upperBound)
    }

    static func capture(store: SessionStore) -> SourceSeparationController? {
        guard store.effectMode != .master,
              let track = store.currentTrack, track.id != Session.masterTrackID,
              store.selection.map({ $0.range.isEmpty || $0.trackID == track.id }) ?? true,
              let range = store.selectedRange, !range.isEmpty, range.lowerBound >= 0, store.hasAudioSelection,
              !store.busy, !store.isRecording,
              let remote = store.remoteGUI, remote.connected,
              !remote.hasPendingEdits(for: track.id) else {
            store.status = "Select audio on a track and wait for pending edits before separating a source."
            return nil
        }
        guard case .connected(_, _, let role) = remote.state, role != .viewer else {
            store.status = "An Editor or Operator connection is required to separate a source."
            return nil
        }
        guard range.count <= 30 * 60 * Int(sessionRate) else {
            store.status = "Select at most 30 minutes for source separation."
            return nil
        }
        let request = SourceSeparationController(store: store, remote: remote, track: track, range: range)
        guard request.source.ready(in: remote) else {
            store.status = "Wait for the server's current track before separating a source."
            return nil
        }
        return request
    }

    var effectiveQuality: SourceSeparationQuality { target.needsSixSources ? .extended : quality }
    var previewIsPartial: Bool { range.count > Self.previewFrames }
    var sourceIsCurrent: Bool { store?.remoteGUI.map { source.ready(in: $0) } == true }
    var selectedModelAvailable: Bool {
        availableModels.contains(effectiveQuality.modelID) || availableModels.contains(effectiveQuality.rawValue)
    }
    private var role: RemoteRole? {
        guard let remote = store?.remoteGUI, case .connected(_, _, let role) = remote.state else { return nil }
        return role
    }
    var canRun: Bool {
        guard let store, let remote = store.remoteGUI, let trackID = context.trackID else { return false }
        return runtimeAvailable && !checkingRuntime && !pending && !store.busy && !store.isRecording
            && source.ready(in: remote) && !remote.hasPendingEdits(for: trackID)
            && selectedModelAvailable && (role == .editor || role == .operator)
    }
    var canPreview: Bool { canRun }
    var canApply: Bool { canRun }

    func intent(apply: Bool) -> RemoteEditorIntent {
        var strings = ["kind": "sourceIsolator", "separationTarget": target.rawValue,
                       "separationModel": effectiveQuality.rawValue, "separationOperation": operation.rawValue]
        if let processingID { strings["separationJobID"] = processingID.uuidString }
        var capturedContext = context
        if !apply, previewIsPartial { capturedContext.rangeEnd = range.lowerBound + Self.previewFrames }
        return .init(operation: apply ? "effects.apply" : "effects.preview", context: capturedContext,
                     strings: strings, flags: ["separateSource": true], payload: Data("{}".utf8))
    }

    func checkRuntime() {
        guard let remote = store?.remoteGUI, !checkingRuntime, !pending else { return }
        checkingRuntime = true
        remote.route(.init(operation: "sources.status")) { [weak self] reply in
            guard let self else { return }
            checkingRuntime = false
            if reply.accepted, let payload = reply.payload, let status = try? JSONDecoder().decode(SourceSeparationRuntime.self, from: payload) {
                runtimeAvailable = status.available; availableModels = status.models; runtimeMessage = status.message
            } else {
                runtimeAvailable = false; availableModels = []
                runtimeMessage = reply.message.isEmpty ? "The connected server does not provide local source separation." : reply.message
            }
        }
    }

    func run(apply: Bool) {
        guard let store, let remote = store.remoteGUI, let trackID = context.trackID,
              let version = source.fence.versions[trackID] else { return }
        guard source.ready(in: remote) else {
            message = "The source changed. Your choices are retained; reopen the current audio to process it."
            return
        }
        guard role == .operator || role == .editor else {
            message = "An Editor or Operator connection is required to separate a source."
            return
        }
        guard canRun else { message = "Wait for pending work and the local separation engine before processing."; return }
        processingID = UUID()
        // A failed or cancelled run invalidates its command fence, while this
        // sheet retains its opening source and choices for a safe retry.
        let runFence = RemoteGestureRevisionFence(versions: source.fence.versions)
        processingFence = runFence
        pending = true; cancellationRequested = false; message = apply ? "Separating and applying…" : "Preparing separated preview…"
        store.workProgress = nil
        let command = intent(apply: apply)
        remote.send(.editor(command), expecting: [trackID: version], gesture: runFence) { [weak self] reply in
            guard let self else { return }
            pending = false; cancellationRequested = false; processingID = nil; processingFence = nil
            if reply.accepted {
                message = apply ? "Separated audio applied. Undo restores the source."
                    : previewIsPartial ? "Preview ready · first 10 minutes of the selection." : "Preview ready."
                if apply { completed = true; previewing = false }
                else { previewing = true }
                self.store?.activity.record(message, kind: .success)
            } else {
                message = reply.message.isEmpty ? "Processing did not complete. Your choices are retained." : reply.message
                self.store?.activity.record(message, kind: .failure, recoveryTitle: "Review and retry separation…", recovery: { [weak owner = self.store] in
                    owner?.sourceSeparationRequest = self
                })
            }
        }
    }

    func cancelProcessing() {
        guard pending, !cancellationRequested else { return }
        guard let remote = store?.remoteGUI, let jobID = processingID, let runFence = processingFence else { return }
        if remote.cancelQueuedSeparation(fence: runFence) { return }
        cancellationRequested = true; message = "Cancelling separation…"
        remote.route(.init(operation: "jobs.cancel", strings: ["id": jobID.uuidString])) { [weak self] reply in
            guard let self, processingID == jobID, pending, !reply.accepted else { return }
            cancellationRequested = false
            message = reply.message.isEmpty ? "Cancellation was not accepted. You can try again while processing continues." : reply.message
        }
    }

    func stopPreview() {
        guard previewing else { return }
        store?.remoteGUI?.route(.init(operation: "audition.stop", strings: ["kind": "effects"])) { [weak self] reply in
            guard let self else { return }
            if reply.accepted { previewing = false }
            else { message = reply.message.isEmpty ? "The separated preview could not be stopped." : reply.message }
        }
    }
}

struct SourceSeparationSheet: View {
    @ObservedObject var store: SessionStore
    @ObservedObject var request: SourceSeparationController
    @Environment(\.dismiss) private var dismiss
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Label("Separate source", systemImage: "waveform.path").font(.system(size: 21, weight: .semibold))
                Spacer()
                Button("Done") { request.stopPreview(); dismiss() }.disabled(request.pending)
            }
            Text("\(request.trackName) · \(String(format: "%.2f", Double(request.range.count) / sessionRate)) seconds")
                .font(.system(size: 12)).foregroundStyle(Theme.secondary)
            Text("Separate an instrument or voice from the selected audio. Results can contain bleed or artifacts.")
                .font(.system(size: 13)).fixedSize(horizontal: false, vertical: true)
            Picker("Source", selection: $request.target) { ForEach(SeparatedSource.allCases) { Text($0.name).tag($0) } }
                .disabled(request.pending)
            Picker("Model", selection: $request.quality) {
                ForEach(SourceSeparationQuality.allCases) { Text($0.name).tag($0) }
            }.disabled(request.target.needsSixSources || request.pending)
            Picker("Result", selection: $request.operation) { ForEach(SourceSeparationOperation.allCases) { Text($0.name).tag($0) } }
                .pickerStyle(.segmented).disabled(request.pending)
            if request.target == .piano {
                Text("Piano separation is experimental and often retains other instruments. Preview before applying.")
                    .font(.system(size: 12)).foregroundStyle(Theme.secondary)
            }
            Text("Processing runs locally and can take several minutes. Install model weights once before processing.")
                .font(.system(size: 12)).foregroundStyle(Theme.secondary)
            if request.previewIsPartial {
                Text("Preview processes the first 10 minutes. Apply processes the entire selected range.")
                    .font(.system(size: 12)).foregroundStyle(Theme.secondary)
            }
            if request.checkingRuntime { ProgressView().controlSize(.small) }
            else if !request.runtimeAvailable || !request.selectedModelAvailable {
                VStack(alignment: .leading, spacing: 8) {
                    Text(request.runtimeAvailable ? "Install the selected source model." : "Install the local separation engine once.")
                        .font(.system(size: 12, weight: .semibold))
                    Text("bash scripts/setup-source-separation.sh --models all")
                        .font(.system(size: 11, design: .monospaced)).textSelection(.enabled)
                    Text("Run this command from the Soundroom project folder.").font(.system(size: 11)).foregroundStyle(Theme.secondary)
                    Text(request.runtimeAvailable ? "The selected model is not installed." : request.runtimeMessage)
                        .font(.system(size: 11)).foregroundStyle(Theme.secondary).textSelection(.enabled)
                    Button("Check again") { request.checkRuntime() }.disabled(request.pending)
                }.padding(12).frame(maxWidth: .infinity, alignment: .leading).background(Theme.well, in: RoundedRectangle(cornerRadius: 8))
            }
            if !request.sourceIsCurrent {
                Text("The source changed. Close this sheet and reopen the current audio before processing.")
                    .font(.system(size: 12)).foregroundStyle(Theme.secondary)
            }
            if !request.message.isEmpty { Text(request.message).font(.system(size: 12)).textSelection(.enabled) }
            if request.pending {
                HStack {
                    if let progress = store.workProgress { ProgressView(value: progress) }
                    else { ProgressView().controlSize(.small); Spacer() }
                    Button(request.cancellationRequested ? "Cancelling…" : "Cancel processing") { request.cancelProcessing() }
                        .disabled(request.cancellationRequested)
                }
            }
            HStack {
                Button("Preview") { request.run(apply: false) }.disabled(!request.canPreview)
                if request.previewing { Button("Stop preview") { request.stopPreview() }.disabled(request.pending) }
                Spacer()
                Button("Apply to audio") { request.run(apply: true) }.disabled(!request.canApply).keyboardShortcut(.defaultAction)
            }
        }.padding(24).frame(width: 510).background(Theme.paper)
            .interactiveDismissDisabled(request.pending)
            .task { request.checkRuntime() }
            .onChange(of: request.completed) { _, done in if done { dismiss() } }
            .onDisappear { request.stopPreview() }
    }
}
