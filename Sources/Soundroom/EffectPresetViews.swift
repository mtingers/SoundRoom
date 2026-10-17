import AppKit
import SoundroomCore
import SwiftUI

enum EffectPresetDialog: String, Identifiable {
    case save, manage
    var id: String { rawValue }
}

struct SaveEffectPresetSheet: View {
    @ObservedObject var library: EffectPresetLibrary
    let kind: RealtimeEffectKind
    let values: [String: Float]
    let legacyAmount: Float?
    @Environment(\.dismiss) private var dismiss
    @FocusState private var naming: Bool
    @State private var name = ""
    @State private var error: String?
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                EffectIdentity(name: kind.name, rawValue: kind.rawValue)
                Spacer()
                Text("Save preset").font(.system(size: 12)).foregroundStyle(Theme.secondary)
            }
            Text(
                legacyAmount.map { "Legacy · \(kind.parameterDisplay($0))" }
                    ?? EffectControlPresentation.summary(kind: kind, values: values)
            )
            .font(.system(size: 11, design: .monospaced)).foregroundStyle(Theme.secondary)
            .fixedSize(horizontal: false, vertical: true)
            TextField("Preset name", text: $name).textFieldStyle(.roundedBorder).focused($naming)
                .accessibilityLabel("Preset name").onSubmit(save)
            Text("Available in all projects").font(.system(size: 11)).foregroundStyle(Theme.secondary)
                .help(
                    "Saves parameter values for Selection and Real-time. Timeline automation and pitch targets stay in the project; use an effect chain with automation to copy them."
                )
            if let error {
                Text(error).font(.system(size: 12)).foregroundStyle(.red).fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Spacer()
                Button("Save Preset", action: save).buttonStyle(RoomButtonStyle(prominent: true))
                    .keyboardShortcut(.defaultAction).disabled(
                        name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || library.loadError != nil)
            }
        }.padding(24).frame(width: 420).background(Theme.paper).foregroundStyle(Theme.ink).tint(Theme.accent)
            .preferredColorScheme(.dark)
            .onAppear { naming = true }
    }
    private func save() {
        do {
            try library.save(name: name, kind: kind, parameters: values, legacyAmount: legacyAmount)
            dismiss()
        } catch { self.error = error.localizedDescription }
    }
}

struct ManageEffectPresetsSheet: View {
    @ObservedObject var library: EffectPresetLibrary
    let kind: RealtimeEffectKind
    let values: [String: Float]
    let legacyAmount: Float?
    @Environment(\.dismiss) private var dismiss
    @State private var selected: UUID?
    @State private var name = ""
    @State private var error: String?
    @State private var deleted: [SavedEffectPreset] = []
    private var selectedPreset: SavedEffectPreset? { library.presets(for: kind).first { $0.id == selected } }
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack {
                EffectIdentity(name: kind.name, rawValue: kind.rawValue)
                Spacer()
                Text("My presets").font(.system(size: 12)).foregroundStyle(Theme.secondary)
            }
            if let loadError = library.loadError {
                Text(loadError + " The existing file has been kept.").font(.system(size: 12)).foregroundStyle(.red)
                Button("Try Again") { library.reload() }
            } else {
                List(selection: $selected) {
                    ForEach(library.presets(for: kind)) { preset in
                        VStack(alignment: .leading, spacing: 4) {
                            Text(preset.name).font(.system(size: 12, weight: .medium))
                            Text(
                                preset.legacyAmount.map { "Legacy · \(kind.parameterDisplay($0))" }
                                    ?? EffectControlPresentation.summary(kind: kind, values: preset.parameters)
                            )
                            .font(.system(size: 10)).foregroundStyle(Theme.secondary).lineLimit(2)
                        }.padding(.vertical, 3).tag(preset.id)
                    }
                }.frame(height: 180).overlay {
                    if library.presets(for: kind).isEmpty {
                        Text("No custom presets yet.").foregroundStyle(Theme.secondary).allowsHitTesting(false)
                    }
                }.accessibilityLabel("Custom \(kind.name) presets")
                HStack {
                    TextField("Preset name", text: $name).textFieldStyle(.roundedBorder).accessibilityLabel(
                        "Rename preset"
                    )
                    .disabled(selectedPreset == nil).onSubmit(rename)
                    Button("Rename", action: rename).disabled(
                        selectedPreset == nil || name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                            || selectedPreset?.name == name)
                }
                HStack {
                    Button("Delete", action: remove).disabled(selectedPreset == nil)
                    Button("Undo Delete", action: restore).disabled(deleted.isEmpty)
                    Spacer()
                }
                HStack {
                    Button("Replace with Current") {
                        attempt {
                            if let selected {
                                try library.replace(selected, parameters: values, legacyAmount: legacyAmount)
                            }
                        }
                    }
                    .disabled(selectedPreset == nil)
                    Button("Undo Change") { attempt { try library.undoChange() } }.disabled(!library.canUndoChange)
                }
                HStack {
                    Button("Import…") {
                        PresetFilePanels.open(extension: "soundpreset") { url in
                            attempt { _ = try library.importPresets(from: url) }
                        }
                    }
                    Button("Export…") {
                        if let preset = selectedPreset {
                            PresetFilePanels.save(name: preset.name, extension: "soundpreset") { url in
                                attempt { try library.exportPreset(preset.id, to: url) }
                            }
                        }
                    }.disabled(selectedPreset == nil)
                }
            }
            if let error {
                Text(error).font(.system(size: 12)).foregroundStyle(.red).fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                Spacer()
                Button("Done") { dismiss() }.buttonStyle(RoomButtonStyle(prominent: true)).keyboardShortcut(
                    .cancelAction)
            }
        }.padding(24).frame(width: 460).background(Theme.paper).foregroundStyle(Theme.ink).tint(Theme.accent)
            .preferredColorScheme(.dark)
            .onChange(of: selected) { _, _ in
                name = selectedPreset?.name ?? ""
                error = nil
            }
    }
    private func rename() {
        guard let selected else { return }
        do {
            try library.rename(selected, to: name)
            name = selectedPreset?.name ?? ""
            error = nil
        } catch { self.error = error.localizedDescription }
    }
    private func attempt(_ work: () throws -> Void) {
        do {
            try work()
            error = nil
        } catch { self.error = error.localizedDescription }
    }
    private func remove() {
        guard let selected else { return }
        do {
            deleted.append(try library.delete(selected))
            self.selected = nil
            name = ""
            error = nil
        } catch { self.error = error.localizedDescription }
    }
    private func restore() {
        guard let preset = deleted.last else { return }
        do {
            try library.restore(preset)
            deleted.removeLast()
            selected = preset.id
            name = preset.name
            error = nil
        } catch { self.error = error.localizedDescription }
    }
}
