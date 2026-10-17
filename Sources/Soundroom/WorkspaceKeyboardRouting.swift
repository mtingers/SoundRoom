import AppKit

/// Unmodified workspace shortcuts belong to the editor surface. A focused
/// control owns its keys, including Space/Return activation and Delete. Test
/// identity rather than a control-class blacklist so custom SwiftUI focus
/// views and future native controls retain their standard keyboard behavior.
@MainActor enum WorkspaceKeyboardRouting {
    static func accepts(
        eventWindow: NSWindow?, editorWindow: NSWindow?, isKey: Bool,
        hasNestedSheet: Bool, responder: NSResponder?
    ) -> Bool {
        guard let editorWindow, eventWindow === editorWindow, isKey, !hasNestedSheet,
            let responder
        else { return false }
        if responder === editorWindow { return true }
        guard let editor = editorWindow.contentView else { return false }
        return responder === editor
    }
}
