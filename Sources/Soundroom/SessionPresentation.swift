import Foundation

enum SessionAbandonChoice { case save, cancel, discard }

struct SessionExportDestination {
    let name: String
    let format: String
    let stems: Bool
    let directory: URL?
}

/// Desktop presentation is supplied by the editor host. Headless session state
/// has no presenter, so file jobs require explicit paths and never open dialogs.
@MainActor protocol SessionPresentation: AnyObject {
    func report(_ message: String)
    func focusEditor()
    func chooseAudioFiles() -> [URL]?
    func chooseProjectToSave(name: String) -> URL?
    func chooseProjectToOpen(packagesOnly: Bool) -> URL?
    func chooseExportDestination(_ request: SessionExportDestination) -> URL?
    func confirmAbandon() -> SessionAbandonChoice
    func confirmRecovery() -> Bool
    func rememberSavedProject(_ url: URL)
}
