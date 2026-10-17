import AppKit
import SoundroomCore
import SwiftUI
import UniformTypeIdentifiers

@MainActor enum PresetFilePanels {
    static func open(extension fileExtension: String, completion: @escaping (URL) -> Void) {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        panel.allowedContentTypes = [UTType(filenameExtension: fileExtension, conformingTo: .json) ?? .json, .json]
        panel.begin { response in if response == .OK, let url = panel.url { completion(url) } }
    }
    static func save(name: String, extension fileExtension: String, completion: @escaping (URL) -> Void) {
        let panel = NSSavePanel()
        panel.allowedContentTypes = [UTType(filenameExtension: fileExtension, conformingTo: .json) ?? .json]
        panel.isExtensionHidden = false
        panel.nameFieldStringValue = name + "." + fileExtension
        panel.begin { response in if response == .OK, let url = panel.url { completion(url) } }
    }
}

struct EffectChainControls: View {
    @ObservedObject var store: SessionStore
    @StateObject private var library = EffectChainLibrary()
    @State private var showing = false
    var body: some View {
        Button("Chain Presets…") { showing = true }.disabled(store.currentTrack == nil)
            .sheet(isPresented: $showing) { EffectChainSheet(store: store, library: library) }
    }
}

struct EffectChainSheet: View {
    @ObservedObject var store: SessionStore
    @ObservedObject var library: EffectChainLibrary
    @Environment(\.dismiss) private var dismiss
    @State private var selected: UUID?
    @State private var name = ""
    @State private var includeAutomation = false
    @State private var error: String?
    private var selection: SavedEffectChain? { library.chains.first { $0.id == selected } }
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Effect chains").font(.system(size: 20, weight: .semibold))
            if let loadError = library.loadError {
                Text(loadError).foregroundStyle(.red)
                Button("Try Again") { library.reload() }
            }
            List(selection: $selected) {
                ForEach(library.chains) { chain in
                    VStack(alignment: .leading, spacing: 3) {
                        Text(chain.name)
                        Text(
                            chain.effects.map { ($0.enabled ? "" : "Bypassed ") + $0.kind.name }.joined(
                                separator: " → ")
                        )
                        .font(.system(size: 11)).foregroundStyle(Theme.secondary)
                        if chain.includesAutomation {
                            Text("Includes automation at session times").font(.system(size: 10)).foregroundStyle(
                                Theme.secondary)
                        }
                    }.tag(chain.id)
                }
            }.frame(height: 190).accessibilityLabel("Saved effect chains")
            HStack {
                TextField("Chain name", text: $name).textFieldStyle(.roundedBorder)
                Button("Save As") { save(replacing: nil) }.disabled(
                    name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || library.loadError != nil)
                Button("Replace") { save(replacing: selected) }.disabled(selection == nil || library.loadError != nil)
            }
            Toggle("Include automation", isOn: $includeAutomation)
            HStack {
                Button("Import…") {
                    PresetFilePanels.open(extension: "soundchain") { url in
                        attempt { _ = try library.importChains(from: url) }
                    }
                }
                Button("Export…") {
                    if let chain = selection {
                        PresetFilePanels.save(name: chain.name, extension: "soundchain") { url in
                            attempt { try library.exportChain(chain.id, to: url) }
                        }
                    }
                }.disabled(selection == nil)
                Button("Delete") {
                    if let selected {
                        attempt { try library.delete(selected) }
                        self.selected = nil
                    }
                }.disabled(selection == nil)
                Button("Undo Change") { attempt { try library.undoChange() } }.disabled(!library.canUndoChange)
            }
            if let error {
                Text(error).foregroundStyle(.red).font(.system(size: 12)).fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                Button("Close") { dismiss() }.keyboardShortcut(.cancelAction)
                Spacer()
                Button("Recall on Track") {
                    if let selection {
                        store.applyEffectChain(selection)
                        dismiss()
                    }
                }.buttonStyle(RoomButtonStyle(prominent: true)).disabled(
                    selection == nil || store.currentTrack == nil || store.busy || store.isRecording)
            }
        }.padding(24).frame(width: 580).background(Theme.paper).foregroundStyle(Theme.ink).tint(Theme.accent)
            .preferredColorScheme(.dark)
            .onChange(of: selected) { _, _ in
                name = selection?.name ?? ""
                includeAutomation = selection?.includesAutomation ?? false
            }
    }
    private func save(replacing id: UUID?) {
        attempt {
            let saved = try library.save(
                name: name, effects: store.currentTrack?.realtimeEffects ?? [], includeAutomation: includeAutomation,
                replacing: id)
            selected = saved.id
        }
    }
    private func attempt(_ work: () throws -> Void) {
        do {
            try work()
            error = nil
        } catch { self.error = error.localizedDescription }
    }
}
