import Combine
import Foundation
import SoundroomCore

struct SavedEffectChain: Codable, Equatable, Identifiable {
    var id = UUID()
    var name: String
    var effects: [RealtimeEffect]
    var includesAutomation: Bool
}

private struct EffectChainDocument: Codable {
    var version = 1
    var chains: [SavedEffectChain]
}

@MainActor final class EffectChainLibrary: ObservableObject {
    @Published private(set) var chains: [SavedEffectChain] = []
    @Published private(set) var loadError: String?
    @Published private(set) var canUndoChange = false
    private var changes: [[SavedEffectChain]] = []
    let url: URL
    init(
        url: URL = EffectPresetLibrary.defaultURL.deletingLastPathComponent().appendingPathComponent(
            "EffectChains.json")
    ) {
        self.url = url
        reload()
    }
    func reload() {
        do {
            guard FileManager.default.fileExists(atPath: url.path) else {
                chains = []
                loadError = nil
                return
            }
            chains = try Self.read(from: url)
            loadError = nil
        } catch { loadError = error.localizedDescription }
    }
    @discardableResult func save(
        name: String, effects: [RealtimeEffect], includeAutomation: Bool, replacing id: UUID? = nil
    ) throws -> SavedEffectChain {
        guard loadError == nil else { throw EffectPresetError.unavailable }
        guard effects.count <= 64 else { throw EffectPresetError.invalidLibrary }
        let name = try Self.validName(name)
        guard !chains.contains(where: { $0.id != id && Self.key($0.name) == Self.key(name) }) else {
            throw EffectPresetError.duplicateName
        }
        let chain = SavedEffectChain(
            id: id ?? UUID(), name: name, effects: Self.prepared(effects, includeAutomation: includeAutomation),
            includesAutomation: includeAutomation)
        var updated = chains
        if let id {
            guard let index = updated.firstIndex(where: { $0.id == id }) else { throw EffectPresetError.missingPreset }
            updated[index] = chain
        } else {
            updated.append(chain)
        }
        try persist(updated)
        return chain
    }
    func delete(_ id: UUID) throws { try persist(chains.filter { $0.id != id }) }
    func undoChange() throws {
        guard let previous = changes.last else { return }
        try persist(previous, remembering: false)
        changes.removeLast()
        canUndoChange = !changes.isEmpty
    }
    func exportChain(_ id: UUID, to destination: URL) throws {
        guard let chain = chains.first(where: { $0.id == id }) else { throw EffectPresetError.missingPreset }
        try Self.data([chain]).write(to: destination, options: .atomic)
    }
    @discardableResult func importChains(from source: URL) throws -> Int {
        let incoming = try Self.read(from: source)
        var updated = chains
        for var chain in incoming {
            let base = chain.name
            var suffix = 2
            while updated.contains(where: { Self.key($0.name) == Self.key(chain.name) }) {
                chain.name = String(base.prefix(68)) + " (\(suffix))"
                suffix += 1
            }
            chain.id = UUID()
            updated.append(chain)
        }
        try persist(updated)
        return incoming.count
    }
    private func persist(_ updated: [SavedEffectChain], remembering: Bool = true) throws {
        guard loadError == nil else { throw EffectPresetError.unavailable }
        let data = try Self.data(updated)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try data.write(to: url, options: .atomic)
        if remembering {
            changes.append(chains)
            if changes.count > 20 { changes.removeFirst() }
        }
        chains = updated
        canUndoChange = !changes.isEmpty
    }
    private static func data(_ chains: [SavedEffectChain]) throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        var document = EffectChainDocument(chains: chains)
        if chains.contains(where: { chain in
            chain.effects.contains { !$0.pitchEdits.isEmpty || $0.pitchEditsOnly || !$0.bakedRanges.isEmpty }
        }) {
            document.version = 2
        }
        return try encoder.encode(document)
    }
    private static func read(from source: URL) throws -> [SavedEffectChain] {
        let data = try Data(contentsOf: source, options: .mappedIfSafe)
        guard data.count <= 4_194_304 else { throw EffectPresetError.invalidLibrary }
        let document = try JSONDecoder().decode(EffectChainDocument.self, from: data)
        guard (1...2).contains(document.version) else { throw EffectPresetError.unsupportedVersion }
        guard document.chains.count <= 128 else { throw EffectPresetError.invalidLibrary }
        var ids: Set<UUID> = []
        var names: Set<String> = []
        return try document.chains.map { original in
            var chain = original
            chain.name = try validName(chain.name)
            guard ids.insert(chain.id).inserted, names.insert(key(chain.name)).inserted,
                chain.effects.count <= 64
            else { throw EffectPresetError.invalidLibrary }
            chain.effects = prepared(chain.effects, includeAutomation: chain.includesAutomation, regenerateIDs: false)
            return chain
        }
    }
    private static func validName(_ raw: String) throws -> String {
        let name = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, name.count <= 80, name.rangeOfCharacter(from: .controlCharacters) == nil else {
            throw EffectPresetError.invalidName
        }
        return name
    }
    private static func key(_ name: String) -> String {
        name.folding(options: [.caseInsensitive, .diacriticInsensitive], locale: Locale(identifier: "en_US_POSIX"))
    }
    static func prepared(_ effects: [RealtimeEffect], includeAutomation: Bool, regenerateIDs: Bool = true)
        -> [RealtimeEffect]
    {
        effects.map { original in
            var effect = original
            if regenerateIDs { effect.id = UUID() }
            if !effect.parameters.isEmpty {
                effect.parameters = Dictionary(
                    uniqueKeysWithValues: effect.kind.parameters.map {
                        ($0.id, $0.clamped(effect.parameters[$0.id] ?? $0.defaultValue))
                    })
            }
            effect.amount = effect.amount.isFinite ? max(0, min(1, effect.amount)) : effect.kind.defaultAmount
            if !includeAutomation {
                effect.automation = []
                effect.parameterAutomation = [:]
                effect.pitchEdits = []
                effect.pitchEditsOnly = false
                effect.bakedRanges = []
            } else {
                func clean(_ points: [AutomationPoint]) -> [AutomationPoint] {
                    var values: [Int: AutomationPoint] = [:]
                    for var point in points where point.frame >= 0 && point.value.isFinite {
                        point.value = max(0, min(1, point.value))
                        values[point.frame] = point
                    }
                    return values.values.sorted { $0.frame < $1.frame }
                }
                effect.automation = clean(effect.automation)
                effect.parameterAutomation = effect.parameterAutomation.filter { key, _ in
                    effect.kind.parameters.contains { $0.id == key }
                }.mapValues(clean)
                effect.pitchEdits = PitchEditRegions.relocated(effect.pitchEdits, by: 0, renewIDs: regenerateIDs)
                // A recalled rack processes different audio. Guards identify
                // source audio already rendered through an insert; copying the
                // settings must not bypass that insert on the destination.
                effect.bakedRanges = regenerateIDs ? [] : PitchEditRegions.sanitizedRanges(effect.bakedRanges)
            }
            return effect
        }
    }
}

extension SessionStore {
    func applyEffectChain(_ chain: SavedEffectChain) {
        guard let id = selectedTrack, !busy, !isRecording else { return }
        let effects = EffectChainLibrary.prepared(chain.effects, includeAutomation: chain.includesAutomation)
        updateTrack(id, actionName: "Chain \(chain.name)") { $0.realtimeEffects = effects }
        selectedRealtimeEffect = effects.first?.id
        automationTarget = .effect
        focusEditor()
    }
}
