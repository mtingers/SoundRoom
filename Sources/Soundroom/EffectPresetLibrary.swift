import Combine
import Foundation
import SoundroomCore

struct SavedEffectPreset: Codable, Equatable, Identifiable {
    var id: UUID
    var name: String
    var kind: RealtimeEffectKind
    var parameters: [String: Float]
    /// Preserve the original signal path when saving an older one-knob effect.
    var legacyAmount: Float?
}

private struct EffectPresetDocument: Codable {
    var version = 1
    var presets: [SavedEffectPreset]
}

enum EffectPresetError: LocalizedError {
    case invalidName, duplicateName, missingPreset, unsupportedVersion, invalidLibrary, unavailable
    var errorDescription: String? {
        switch self {
        case .invalidName: return "Use a name of 1–80 characters on one line."
        case .duplicateName: return "A preset with that name already exists for this effect."
        case .missingPreset: return "That preset is no longer available."
        case .unsupportedVersion: return "These presets were saved by a newer version of Soundroom."
        case .invalidLibrary: return "The preset library contains invalid or duplicate entries."
        case .unavailable: return "The preset library could not be loaded. Its existing file has been kept."
        }
    }
}

/// A separate personal library: project undo and project saves never modify it.
/// Publish changes only after the atomic disk write succeeds.
@MainActor final class EffectPresetLibrary: ObservableObject {
    @Published private(set) var presets: [SavedEffectPreset] = []
    @Published private(set) var loadError: String?
    @Published private(set) var canUndoChange = false
    private var changes: [[SavedEffectPreset]] = []
    let url: URL

    nonisolated static var defaultURL: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Soundroom", isDirectory: true).appendingPathComponent("EffectPresets.json")
    }
    init(url: URL = EffectPresetLibrary.defaultURL) {
        self.url = url
        reload()
    }
    func reload() {
        do {
            guard FileManager.default.fileExists(atPath: url.path) else {
                presets = []
                loadError = nil
                return
            }
            let document = try JSONDecoder().decode(EffectPresetDocument.self, from: Data(contentsOf: url))
            guard document.version == 1 else { throw EffectPresetError.unsupportedVersion }
            var ids: Set<UUID> = []
            var names: Set<String> = []
            var loaded: [SavedEffectPreset] = []
            for var preset in document.presets {
                preset.name = try Self.validName(preset.name)
                guard ids.insert(preset.id).inserted,
                    names.insert(preset.kind.rawValue + ":" + Self.nameKey(preset.name)).inserted
                else { throw EffectPresetError.invalidLibrary }
                preset.parameters = Self.sanitized(preset.parameters, kind: preset.kind)
                if let amount = preset.legacyAmount { preset.legacyAmount = Self.safeAmount(amount) }
                loaded.append(preset)
            }
            presets = loaded
            loadError = nil
        } catch { loadError = error.localizedDescription }
    }
    func presets(for kind: RealtimeEffectKind) -> [SavedEffectPreset] {
        presets.filter { $0.kind == kind }.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
    }
    @discardableResult func save(
        name: String, kind: RealtimeEffectKind, parameters: [String: Float], legacyAmount: Float? = nil
    ) throws -> SavedEffectPreset {
        let name = try checkedName(name, kind: kind)
        let preset = SavedEffectPreset(
            id: UUID(), name: name, kind: kind,
            parameters: Self.sanitized(parameters, kind: kind), legacyAmount: legacyAmount.map(Self.safeAmount))
        try persist(presets + [preset])
        return preset
    }
    func rename(_ id: UUID, to name: String) throws {
        guard let index = presets.firstIndex(where: { $0.id == id }) else { throw EffectPresetError.missingPreset }
        let name = try checkedName(name, kind: presets[index].kind, excluding: id)
        var updated = presets
        updated[index].name = name
        try persist(updated)
    }
    @discardableResult func delete(_ id: UUID) throws -> SavedEffectPreset {
        guard let preset = presets.first(where: { $0.id == id }) else { throw EffectPresetError.missingPreset }
        try persist(presets.filter { $0.id != id })
        return preset
    }
    func restore(_ preset: SavedEffectPreset) throws {
        _ = try checkedName(preset.name, kind: preset.kind)
        guard !presets.contains(where: { $0.id == preset.id }) else { throw EffectPresetError.invalidLibrary }
        try persist(presets + [preset])
    }
    func replace(_ id: UUID, parameters: [String: Float], legacyAmount: Float?) throws {
        guard let index = presets.firstIndex(where: { $0.id == id }) else { throw EffectPresetError.missingPreset }
        var updated = presets
        updated[index].parameters = Self.sanitized(parameters, kind: updated[index].kind)
        updated[index].legacyAmount = legacyAmount.map(Self.safeAmount)
        try persist(updated)
    }
    func exportPreset(_ id: UUID, to destination: URL) throws {
        guard let preset = presets.first(where: { $0.id == id }) else { throw EffectPresetError.missingPreset }
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        try encoder.encode(EffectPresetDocument(presets: [preset])).write(to: destination, options: .atomic)
    }
    @discardableResult func importPresets(from source: URL) throws -> Int {
        let data = try Data(contentsOf: source, options: .mappedIfSafe)
        guard data.count <= 1_048_576 else { throw EffectPresetError.invalidLibrary }
        let document = try JSONDecoder().decode(EffectPresetDocument.self, from: data)
        guard document.version == 1 else { throw EffectPresetError.unsupportedVersion }
        guard !document.presets.isEmpty, document.presets.count <= 128 else { throw EffectPresetError.invalidLibrary }
        var updated = presets
        for incoming in document.presets {
            var item = incoming
            let base = try Self.validName(item.name)
            var name = base
            var suffix = 2
            while updated.contains(where: { $0.kind == item.kind && Self.nameKey($0.name) == Self.nameKey(name) }) {
                name = String(base.prefix(68)) + " (\(suffix))"
                suffix += 1
            }
            item.id = UUID()
            item.name = name
            item.parameters = Self.sanitized(item.parameters, kind: item.kind)
            item.legacyAmount = item.legacyAmount.map(Self.safeAmount)
            updated.append(item)
        }
        try persist(updated)
        return document.presets.count
    }
    func undoChange() throws {
        guard let previous = changes.last else { return }
        try persist(previous, remembering: false)
        changes.removeLast()
        canUndoChange = !changes.isEmpty
    }
    private func checkedName(_ raw: String, kind: RealtimeEffectKind, excluding id: UUID? = nil) throws -> String {
        guard loadError == nil else { throw EffectPresetError.unavailable }
        let name = try Self.validName(raw)
        guard
            !presets.contains(where: { $0.kind == kind && $0.id != id && Self.nameKey($0.name) == Self.nameKey(name) })
        else { throw EffectPresetError.duplicateName }
        return name
    }
    private func persist(_ updated: [SavedEffectPreset], remembering: Bool = true) throws {
        guard loadError == nil else { throw EffectPresetError.unavailable }
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        let data = try encoder.encode(EffectPresetDocument(presets: updated))
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try data.write(to: url, options: .atomic)
        if remembering {
            changes.append(presets)
            if changes.count > 20 { changes.removeFirst() }
        }
        presets = updated
        canUndoChange = !changes.isEmpty
    }
    private static func validName(_ raw: String) throws -> String {
        let name = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, name.count <= 80, name.rangeOfCharacter(from: .controlCharacters) == nil else {
            throw EffectPresetError.invalidName
        }
        return name
    }
    private static func nameKey(_ name: String) -> String {
        name.folding(options: [.caseInsensitive, .diacriticInsensitive], locale: Locale(identifier: "en_US_POSIX"))
    }
    private static func safeAmount(_ amount: Float) -> Float { amount.isFinite ? max(0, min(1, amount)) : 0.5 }
    private static func sanitized(_ values: [String: Float], kind: RealtimeEffectKind) -> [String: Float] {
        var result = Dictionary(
            uniqueKeysWithValues: kind.parameters.map { ($0.id, $0.clamped(values[$0.id] ?? $0.defaultValue)) })
        if kind == .soundPutty { result.merge(SoundPuttyShape.sanitizedParameters(values)) { _, new in new } }
        return result
    }
}
