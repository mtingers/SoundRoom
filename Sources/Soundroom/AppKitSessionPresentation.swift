import AppKit
import UniformTypeIdentifiers

@MainActor final class AppKitSessionPresentation: SessionPresentation {
    func report(_ message: String) {
        let alert = NSAlert()
        alert.messageText = "Soundroom couldn’t finish that"; alert.informativeText = message
        alert.runModal()
    }
    func focusEditor() {
        guard let window = NSApp?.keyWindow, window.identifier?.rawValue == "SoundroomEditor", window.attachedSheet == nil else { return }
        window.makeFirstResponder(window.contentView)
    }
    func chooseAudioFiles() -> [URL]? {
        let panel = NSOpenPanel(); panel.allowedContentTypes = [.audio]; panel.allowsMultipleSelection = true
        return panel.runModal() == .OK ? panel.urls : nil
    }
    func chooseProjectToSave(name: String) -> URL? {
        let panel = NSSavePanel(); panel.nameFieldStringValue = "\(name).soundroom"; panel.canCreateDirectories = true
        return panel.runModal() == .OK ? panel.url : nil
    }
    func chooseProjectToOpen(packagesOnly: Bool) -> URL? {
        if packagesOnly { return ProjectOpenPanel().runModal() }
        let panel = NSOpenPanel(); panel.canChooseDirectories = true; panel.canChooseFiles = true
        return panel.runModal() == .OK ? panel.url : nil
    }
    func chooseExportDestination(_ request: SessionExportDestination) -> URL? {
        let panel = NSSavePanel(); panel.canCreateDirectories = true
        panel.nameFieldStringValue = request.stems ? "\(request.name) tracks" : "\(request.name).\(request.format)"
        if !request.stems, let type = UTType(filenameExtension: request.format) { panel.allowedContentTypes = [type] }
        if let directory = request.directory { panel.directoryURL = directory }
        return panel.runModal() == .OK ? panel.url : nil
    }
    func confirmAbandon() -> SessionAbandonChoice {
        let alert = NSAlert(); alert.messageText = "Save your session?"
        alert.informativeText = "Your recordings and edits haven’t been saved yet."
        alert.addButton(withTitle: "Save"); alert.addButton(withTitle: "Cancel"); alert.addButton(withTitle: "Discard")
        switch alert.runModal() {
        case .alertFirstButtonReturn: return .save
        case .alertThirdButtonReturn: return .discard
        default: return .cancel
        }
    }
    func confirmRecovery() -> Bool {
        let alert = NSAlert(); alert.messageText = "Recover your previous session?"
        alert.informativeText = "Unsaved edits or interrupted recordings are available. Your manually saved project is unchanged."
        alert.addButton(withTitle: "Recover"); alert.addButton(withTitle: "Discard")
        return alert.runModal() == .alertFirstButtonReturn
    }
    func rememberSavedProject(_ url: URL) {
        NSDocumentController.shared.noteNewRecentDocumentURL(url)
    }
}
