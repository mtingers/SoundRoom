import XCTest
import AppKit
@testable import Soundroom

final class DesignerKeyboardRoutingTests: XCTestCase {
    @MainActor func testDesignerCommandsAreConfinedToItsSheet() {
        let editor = NSWindow(), designer = NSPanel()
        XCTAssertTrue(DesignerKeyboardRouting.accepts(eventWindow: designer, designerWindow: designer, isKey: true, hasNestedSheet: false, responder: NSView()))
        XCTAssertFalse(DesignerKeyboardRouting.accepts(eventWindow: editor, designerWindow: designer, isKey: true, hasNestedSheet: false, responder: NSView()))
        XCTAssertFalse(DesignerKeyboardRouting.accepts(eventWindow: designer, designerWindow: designer, isKey: false, hasNestedSheet: false, responder: NSView()))
    }
    @MainActor func testTypingAndNestedFilePanelsKeepNativeShortcuts() {
        let designer = NSPanel()
        XCTAssertFalse(DesignerKeyboardRouting.accepts(eventWindow: designer, designerWindow: designer, isKey: true, hasNestedSheet: false, responder: NSTextView()))
        XCTAssertFalse(DesignerKeyboardRouting.accepts(eventWindow: designer, designerWindow: designer, isKey: true, hasNestedSheet: true, responder: NSView()))
        XCTAssertFalse(DesignerKeyboardRouting.accepts(eventWindow: nil, designerWindow: designer, isKey: true, hasNestedSheet: false, responder: NSView()))
    }
}
