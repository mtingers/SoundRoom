import AppKit
import UniformTypeIdentifiers

@MainActor enum DesignerSoundFilePanel {
    static func makeOpenPanel() -> NSOpenPanel {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.json]
        panel.treatsFilePackagesAsDirectories = false
        // macOS 27 infers these flags as content types change; state them last.
        panel.canChooseFiles = true; panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        return panel
    }
}
