import Foundation
import Combine
import SoundroomCore
import SoundroomRemote

/// A review is a private proposal bound to its measured mix and opening Master
/// rack. Fingerprinting runs once on a worker, never while drawing a slider.
@MainActor final class MasteringReviewController: ObservableObject {
    typealias Verifier = @Sendable (MasteringAnalysis, Session) -> Bool
    private weak var owner: SessionStore?
    private let verify: Verifier
    @Published private(set) var report: MasteringAnalysis?
    @Published private(set) var proposed: [RealtimeEffect] = []
    @Published private(set) var message = ""
    @Published private(set) var working = false
    @Published private(set) var verifying = false
    @Published private(set) var sourceChanged = false
    @Published private(set) var completedApplyID: UUID?
    @Published var target: Double = -14 { didSet { rebuild() } }
    @Published var intensity: Double = 0.65 { didSet { rebuild() } }
    private var included: [RealtimeEffectKind: Bool] = [:]
    private var verified = false
    private var versions: [UUID: Int] = [:]
    private var trackIDs: [UUID] = []
    private var epoch: Int?
    private var generation = UUID()
    private var isOpen = true
    private var verificationTask: Task<Void, Never>?
    init(owner: SessionStore, verify: @escaping Verifier = { $0.matches($1) }) { self.owner = owner; self.verify = verify }
    var ready: Bool {
        guard isOpen, let owner, let report, !report.isSilent, verified, !sourceChanged, !working,
              !owner.busy, !owner.isRecording else { return false }
        guard let remote = owner.remoteGUI, remote.connected, remote.client.credential.role != .viewer else { return false }
        return remote.client.capabilities?.supports(.editor(.init(operation: "master.applyProposal"))) != false
    }
    private func currentDocument() -> Session? {
        guard let owner else { return nil }
        return owner.remoteGUI?.authoritativeSession
    }
    private func sourceIsCurrent(excludingMaster: Bool = false) -> Bool {
        guard let owner, let document = currentDocument(), document.tracks.map(\.id) == trackIDs else { return false }
        guard let remote = owner.remoteGUI, remote.connected, remote.connectionGeneration == epoch else { return false }
        return versions.allSatisfy { id, version in excludingMaster && id == Session.masterTrackID || remote.trackRevisions[id] == version }
    }
    func synchronize() {
        guard report != nil else { return }
        guard sourceIsCurrent(), currentDocument()?.masteringAnalysis == report else {
            sourceChanged = true
            if !working { message = "The mix or Master rack changed. Your private proposal is retained; reload the review or analyze again." }
            return
        }
    }
    func reload() {
        guard !working, !verifying, let owner, let document = currentDocument() else { return }
        generation = UUID(); verificationTask?.cancel()
        isOpen = true
        let token = generation
        report = document.masteringAnalysis; included = [:]; sourceChanged = false; verified = false
        message = ""; trackIDs = document.tracks.map(\.id)
        let currentIDs = Set(trackIDs).union([Session.masterTrackID])
        versions = owner.remoteGUI?.trackRevisions.filter { currentIDs.contains($0.key) } ?? [:]; epoch = owner.remoteGUI?.connectionGeneration
        rebuild()
        guard let report else { verifying = false; return }
        if !currentIDs.allSatisfy({ versions[$0] != nil }) {
            sourceChanged = true; verifying = false; message = "Refresh the server snapshot before using this review."; return
        }
        verifying = true
        let verify = self.verify
        verificationTask = Task { [weak self] in
            let worker = Task.detached(priority: .utility) { () -> Bool in
                guard !Task.isCancelled else { return false }
                let matches = verify(report, document)
                return !Task.isCancelled && matches
            }
            let matches = await withTaskCancellationHandler(operation: { await worker.value }, onCancel: { worker.cancel() })
            guard let self, token == generation, !Task.isCancelled else { return }
            verifying = false; verified = matches
            synchronize()
            if !matches { sourceChanged = true; message = "The analysis belongs to an earlier mix. Analyze again; your private proposal is retained." }
        }
    }
    func waitForSourceVerification() async { await verificationTask?.value }
    private func rebuild() {
        guard let report else { proposed = []; return }
        let aim = target.isFinite ? max(-24, min(-8, target)) : -14
        let amount = intensity.isFinite ? max(0, min(1, intensity)) : 0.65
        proposed = report.proposal(targetLUFS: Float(aim), intensity: Float(amount)).map { original in
            var effect = original; if let enabled = included[effect.kind] { effect.enabled = enabled }; return effect
        }
    }
    func setIncluded(_ index: Int, _ enabled: Bool) {
        guard isOpen, !working, proposed.indices.contains(index) else { return }
        included[proposed[index].kind] = enabled; proposed[index].enabled = enabled
    }
    func analyze() {
        guard isOpen, let owner, !working, !verifying, !owner.busy, !owner.isRecording else { return }
        guard owner.session.end > 0 else { message = "Add audio before analyzing the mix."; return }
        generation = UUID(); verificationTask?.cancel(); verifying = false
        let token = generation; working = true; message = "Analyzing mix…"


        guard let remote = owner.remoteGUI, let document = remote.authoritativeSession,
              remote.connected, remote.client.credential.role != .viewer else {
            working = false; message = "Connect with edit access to analyze this mix."; return
        }
        let openingIDs = document.tracks.map(\.id), guardedIDs = Set(openingIDs).union([Session.masterTrackID])
        let openingVersions = remote.trackRevisions.filter { guardedIDs.contains($0.key) }, openingEpoch = remote.connectionGeneration
        let fence = RemoteGestureRevisionFence(versions: openingVersions)
        remote.send(.editor(.init(operation: "master.analyze")), expecting: openingVersions, gesture: fence, expectedRevision: remote.revision) { [weak self, weak remote] reply in
            guard let self, token == generation else { return }
            working = false
            guard reply.accepted, let remote else { message = reply.message; return }
            guard remote.connectionGeneration == openingEpoch, remote.revision == reply.revision,
                  remote.authoritativeSession?.tracks.map(\.id) == openingIDs,
                  openingVersions.allSatisfy({ remote.trackRevisions[$0.key] == $0.value }) else {
                message = "The source changed after analysis. Your private proposal is retained; reload the current review."; sourceChanged = true; return
            }
            reload()
        }
    }
    func apply() {
        synchronize()
        guard let owner, ready else { return }
        let effects = proposed, token = generation


        guard let remote = owner.remoteGUI, let payload = try? JSONEncoder().encode(effects) else { return }
        working = true; message = "Applying proposal…"
        let fence = RemoteGestureRevisionFence(versions: versions)
        remote.send(.editor(.init(operation: "master.applyProposal", context: .init(trackID: Session.masterTrackID), payload: payload)), expecting: versions, gesture: fence, expectedRevision: remote.revision) { [weak self, weak remote] reply in
            guard let self, token == generation else { return }
            working = false
            guard reply.accepted, let remote else { message = reply.message; return }
            guard sourceIsCurrent(excludingMaster: true), remote.trackRevisions[Session.masterTrackID] == reply.revision,
                  remote.authoritativeSession?.masterTrack.realtimeEffects == effects else {
                sourceChanged = true; message = "The source changed after Apply. Your private proposal is retained for review."; return
            }
            completedApplyID = UUID(); message = "Mastering proposal applied"
        }
    }
    func close() { isOpen = false; generation = UUID(); verificationTask?.cancel(); verifying = false; working = false }
}
