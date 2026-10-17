import AppKit
import XCTest

@testable import Soundroom

final class WorkspaceKeyboardRoutingTests: XCTestCase {
    @MainActor func testEditorAndWindowFocusRetainWorkspaceShortcuts() {
        let window = NSWindow()
        let editor = NSView()
        window.contentView = editor
        for responder in [editor, window] as [NSResponder] {
            XCTAssertTrue(
                WorkspaceKeyboardRouting.accepts(
                    eventWindow: window, editorWindow: window,
                    isKey: true, hasNestedSheet: false, responder: responder))
        }
    }

    @MainActor func testFocusedControlsOwnActivationDeletionAndNavigationKeys() {
        let window = NSWindow()
        let editor = NSView()
        window.contentView = editor
        // The same policy protects Space, Return, Delete, arrows and Tab;
        // custom focus views also own keys without needing a type blacklist.
        let controls: [NSView] = [
            NSButton(), NSSlider(), NSPopUpButton(), NSSegmentedControl(),
            NSStepper(), NSTextField(), NSTextView(), NSScrollView(), NSView(),
        ]
        for control in controls {
            editor.addSubview(control)
            XCTAssertFalse(
                WorkspaceKeyboardRouting.accepts(
                    eventWindow: window, editorWindow: window,
                    isKey: true, hasNestedSheet: false, responder: control),
                "Focused \(type(of: control)) must retain its keys")
        }
        XCTAssertFalse(
            WorkspaceKeyboardRouting.accepts(
                eventWindow: window, editorWindow: window,
                isKey: true, hasNestedSheet: false, responder: NSResponder()))
    }

    @MainActor func testOtherWindowsSheetsAndInactiveWindowsNeverReceiveWorkspaceShortcuts() {
        let window = NSWindow()
        let other = NSWindow()
        let editor = NSView()
        window.contentView = editor
        XCTAssertFalse(
            WorkspaceKeyboardRouting.accepts(
                eventWindow: other, editorWindow: window,
                isKey: true, hasNestedSheet: false, responder: editor))
        XCTAssertFalse(
            WorkspaceKeyboardRouting.accepts(
                eventWindow: nil, editorWindow: window,
                isKey: true, hasNestedSheet: false, responder: editor))
        XCTAssertFalse(
            WorkspaceKeyboardRouting.accepts(
                eventWindow: window, editorWindow: nil,
                isKey: true, hasNestedSheet: false, responder: editor))
        XCTAssertFalse(
            WorkspaceKeyboardRouting.accepts(
                eventWindow: window, editorWindow: window,
                isKey: false, hasNestedSheet: false, responder: editor))
        XCTAssertFalse(
            WorkspaceKeyboardRouting.accepts(
                eventWindow: window, editorWindow: window,
                isKey: true, hasNestedSheet: true, responder: editor))
    }

    @MainActor func testMissingFocusDoesNotMatchAnAbsentEditor() {
        let window = NSWindow()
        window.contentView = nil
        XCTAssertFalse(
            WorkspaceKeyboardRouting.accepts(
                eventWindow: window, editorWindow: window,
                isKey: true, hasNestedSheet: false, responder: nil))
    }
}
