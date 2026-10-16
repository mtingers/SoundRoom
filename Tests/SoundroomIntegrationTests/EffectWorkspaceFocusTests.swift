import XCTest
import AppKit
@testable import Soundroom

final class EffectWorkspaceFocusTests: XCTestCase {
    @MainActor func testNumericBlurCommitsBeforeResetAndSynchronizesVisibleDraft() async {
        let window = NSWindow(), content = NSView(frame: NSRect(x: 0, y: 0, width: 300, height: 100))
        let field = ResigningDraftTextView(frame: content.bounds)
        content.addSubview(field); window.contentView = content
        XCTAssertFalse(content.acceptsFirstResponder, "Hosting content is not a reliable focus destination")
        XCTAssertTrue(window.makeFirstResponder(field))
        var draft = CommittedNumberDraft(value: 0, fractionDigits: 2, locale: Locale(identifier: "en_US"))
        var value = 0.0, events: [String] = []
        _ = draft.begin(); draft.text = "3"
        field.onResign = {
            // SwiftUI observes focus loss during its next update, rather than
            // changing the binding in the native resign call itself.
            DispatchQueue.main.async {
                if let committed = draft.commit(currentValue: value, range: -24...24, fractionDigits: 2, locale: Locale(identifier: "en_US")) {
                    value = committed; events.append("commit")
                }
            }
        }
        let reset = expectation(description: "Reset after draft commit")
        EffectWorkspaceFocus.finishNumberEntry(in: window) {
            XCTAssertFalse(window.firstResponder === field)
            XCTAssertEqual(value, 3)
            XCTAssertFalse(draft.editing)
            value = 0; events.append("reset")
            draft.synchronize(value: value, fractionDigits: 2, locale: Locale(identifier: "en_US"))
            reset.fulfill()
        }
        XCTAssertFalse(window.firstResponder === field, "Actions must actually resign native text focus")
        XCTAssertTrue(events.isEmpty, "The command waits for the numeric focus update")
        await fulfillment(of: [reset], timeout: 2)
        XCTAssertEqual(events, ["commit", "reset"])
        XCTAssertEqual(value, 0)
        XCTAssertEqual(draft.text, "0.00", "The old typed draft must not survive Reset")
    }
    @MainActor func testNonTextActionDoesNotStealSliderFocusOrDefer() {
        let window = NSWindow(), slider = RoomNativeSlider(frame: NSRect(x: 0, y: 0, width: 200, height: 24))
        window.contentView = slider
        XCTAssertTrue(window.makeFirstResponder(slider))
        var calls = 0
        EffectWorkspaceFocus.finishNumberEntry(in: window) { calls += 1 }
        XCTAssertEqual(calls, 1)
        XCTAssertTrue(window.firstResponder === slider)
    }
    @MainActor func testNoWindowRunsActionOnce() {
        var calls = 0
        EffectWorkspaceFocus.finishNumberEntry(in: nil) { calls += 1 }
        XCTAssertEqual(calls, 1)
    }
}

private final class ResigningDraftTextView: NSTextView {
    var onResign: (() -> Void)?
    override func resignFirstResponder() -> Bool {
        let accepted = super.resignFirstResponder()
        if accepted { onResign?() }
        return accepted
    }
}
