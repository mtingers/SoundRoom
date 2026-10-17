import AppKit
import UniformTypeIdentifiers

enum ProjectFileSelection {
    // A project can arrive as an ordinary directory before Launch Services has
    // registered its package type. Accept both representations in the picker.
    static let contentTypes: [UTType] = [UTType(exportedAs: "com.soundroom.session", conformingTo: .package), .folder]

    static func validate(_ url: URL) throws {
        var directory: ObjCBool = false
        guard url.isFileURL, url.pathExtension == "soundroom",
            FileManager.default.fileExists(atPath: url.path, isDirectory: &directory), directory.boolValue
        else {
            throw NSError(
                domain: "Soundroom.ProjectOpen", code: 1,
                userInfo: [
                    NSLocalizedDescriptionKey:
                        "Choose a .soundroom project folder, rather than an audio file or its containing folder."
                ])
        }
        let document = url.appendingPathComponent("session.json")
        directory = false
        guard FileManager.default.fileExists(atPath: document.path, isDirectory: &directory), !directory.boolValue,
            FileManager.default.isReadableFile(atPath: document.path)
        else {
            throw NSError(
                domain: "Soundroom.ProjectOpen", code: 2,
                userInfo: [
                    NSLocalizedDescriptionKey:
                        "This project has no readable session.json. Choose the complete .soundroom project folder."
                ])
        }
    }
}

@MainActor final class ProjectOpenPanel: NSObject, NSOpenSavePanelDelegate {
    let panel = NSOpenPanel()
    override init() {
        super.init()
        panel.title = "Open Soundroom project"
        panel.message = "Choose a .soundroom project. Project folders can be opened directly."
        panel.allowedContentTypes = ProjectFileSelection.contentTypes
        panel.treatsFilePackagesAsDirectories = false
        // Set these after content types: newer AppKit versions derive/reset
        // the selection flags when the content types or package mode change.
        panel.canChooseFiles = true
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        panel.canCreateDirectories = false
        panel.delegate = self
    }
    func panel(_ sender: Any, validate url: URL) throws { try ProjectFileSelection.validate(url) }

    func runModal() -> URL? {
        withExtendedLifetime(self) { panel.runModal() == .OK ? panel.url : nil }
    }

    func begin(for window: NSWindow?, completion: @escaping (URL?) -> Void) {
        // NSOpenPanel's delegate is weak. Keep the validator alive until its
        // asynchronous completion, including after the importing method returns.
        let finished: (NSApplication.ModalResponse) -> Void = { response in
            completion(response == .OK ? self.panel.url : nil)
        }
        if let window {
            panel.beginSheetModal(for: window, completionHandler: finished)
        } else {
            panel.begin(completionHandler: finished)
        }
    }
}
