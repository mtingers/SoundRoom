import Foundation
import XCTest
import SoundroomCore
@testable import Soundroom

@MainActor private final class SessionPresentationSpy: SessionPresentation {
    var calls: [String] = []
    var projectDestination: URL?
    func report(_ message: String) { calls.append("report") }
    func focusEditor() { calls.append("focus") }
    func chooseAudioFiles() -> [URL]? { calls.append("audio"); return nil }
    func chooseProjectToSave(name: String) -> URL? { calls.append("save"); return projectDestination }
    func chooseProjectToOpen(packagesOnly: Bool) -> URL? { calls.append("open"); return nil }
    func chooseExportDestination(_ request: SessionExportDestination) -> URL? { calls.append("export"); return nil }
    func confirmAbandon() -> SessionAbandonChoice { calls.append("abandon"); return .cancel }
    func confirmRecovery() -> Bool { calls.append("recovery"); return false }
    func rememberSavedProject(_ url: URL) { calls.append("recent") }
}

final class SessionPresentationTests: XCTestCase {

    @MainActor func testClientFocusUsesOnlyEditorPresentation() {
        let spy = SessionPresentationSpy()
        let store = SessionStore(presentation: spy)
        store.focusEditor()
        XCTAssertEqual(spy.calls, ["focus"])
        store.report(NSError(domain: "Fixture", code: 2))
        XCTAssertEqual(spy.calls, ["focus"])
        XCTAssertFalse(store.status.isEmpty)
    }

}
