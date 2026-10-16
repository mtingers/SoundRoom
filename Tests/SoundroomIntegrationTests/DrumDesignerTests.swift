import XCTest
import AppKit
@testable import Soundroom
import SoundroomCore

final class DrumDesignerTests: XCTestCase {
    func testStepEditingGesturesPatternClipboardAndHistory() throws {
        try MainActor.assumeIsolated {
            var pattern = DrumPattern(); pattern.apply(.backbeat)
            let editor = DrumEditorModel(pattern: pattern)
            editor.toggle(voice: .kick, index: 1); XCTAssertEqual(editor.undoHistory.count, 1)
            let previous = editor.pattern
            editor.beginGesture(); editor.setStep { $0.velocity = 0.5 }; editor.setStep { $0.velocity = 0.3 }; editor.endGesture()
            XCTAssertEqual(editor.undoHistory.count, 2); editor.undo(); XCTAssertEqual(editor.pattern, previous); editor.redo(); XCTAssertEqual(editor.currentStep?.velocity, 0.3)
            let copied = try editor.copyPattern(); editor.change { $0.steps = [] }; try editor.pastePattern(copied)
            XCTAssertEqual(editor.pattern.steps.count, previous.steps.count); XCTAssertEqual(editor.currentStep?.velocity, 0.3)
            editor.undo(); XCTAssertTrue(editor.pattern.steps.isEmpty)
        }
    }
    func testPatternBankSaveReopenValidationAndAtomicUndo() throws {
        try MainActor.assumeIsolated {
            var pattern = DrumPattern(); pattern.apply(.house); pattern.kit = .electronic
            let editor = DrumEditorModel(pattern: pattern)
            editor.newPattern(duplicate: true); editor.change { $0.bpm = 140; $0.swing = 0.2 }
            let data = try editor.saveDocument()
            let reopened = DrumEditorModel(pattern: DrumPattern()); try reopened.loadDocument(data)
            XCTAssertEqual(reopened.document, editor.document); XCTAssertEqual(reopened.undoHistory.count, 1)
            reopened.undo(); XCTAssertTrue(reopened.pattern.steps.isEmpty); reopened.redo(); XCTAssertEqual(reopened.pattern.bpm, 140)
            var invalid = pattern; invalid.steps = [.init(voice: .kick, index: 10000)]
            XCTAssertThrowsError(try reopened.pastePattern(JSONEncoder().encode(invalid))); XCTAssertEqual(reopened.document, editor.document)
            var document = editor.document; document.version = 3
            XCTAssertThrowsError(try reopened.loadDocument(JSONEncoder().encode(document)))
        }
    }
    func testExpandedPatternBanksPreserveControlsAndRejectMalformedNewValues() throws {
        try MainActor.assumeIsolated {
            var p = DrumPattern(); p.kit = .rockBirch; p.variationCycles = 8; p.velocityHumanize = 0.04
            p.steps = [.init(voice: .snare, index: 4)]; p.steps[0].articulation = .rimshot
            p.lanes["snare"] = DrumLane(kit: .vintageDry); p.lanes["snare"]?.damping = 0.65
            let editor = DrumEditorModel(pattern: p), reopened = DrumEditorModel(pattern: DrumPattern())
            let data = try editor.saveDocument(); try reopened.loadDocument(data)
            XCTAssertEqual(reopened.pattern, p); XCTAssertEqual(reopened.document.version, 2)
            var bad = p; bad.velocityHumanize = 1e100
            XCTAssertThrowsError(try DrumEditorModel.validate(bad))
            bad = p; bad.variationCycles = 3; XCTAssertThrowsError(try DrumEditorModel.validate(bad))
            bad = p; bad.lanes["snare"]?.damping = -1; XCTAssertThrowsError(try DrumEditorModel.validate(bad))
            bad = p; bad.steps[0].articulation = .bell; XCTAssertThrowsError(try DrumEditorModel.validate(bad))
        }
    }
    func testDrumMetadataProjectRoundtripAndEditableFragments() throws {
        var pattern = DrumPattern(); pattern.apply(.breakbeat); pattern.swing = 0.24
        let asset = UUID(), samples = pattern.render()
        var session = Session(); var clip = Clip(assetID: asset, name: pattern.name, start: 300, length: samples.count, edgeFadeFrames: 0)
        clip.drumPattern = pattern; session.tracks[0].clips = [clip]
        let decoded = try JSONDecoder().decode(Session.self, from: JSONEncoder().encode(session))
        XCTAssertEqual(decoded.tracks[0].clips[0].drumPattern, pattern)
        session.split(trackID: session.tracks[0].id, clipID: clip.id, at: 8000)
        XCTAssertTrue(session.tracks[0].clips.allSatisfy { $0.drumPattern == pattern })
        let legacy = "{\"assetID\":\"\(asset.uuidString)\",\"id\":\"\(clip.id.uuidString)\",\"name\":\"Old\",\"start\":0,\"offset\":0,\"length\":100}"
        XCTAssertNil(try JSONDecoder().decode(Clip.self, from: Data(legacy.utf8)).drumPattern)
    }
    func testDrumDesignerRequestStopsMainTransportAndOwnShortcutsAreScoped() {
        MainActor.assumeIsolated {
            let store = SessionStore(); store.requestDrumDesigner()
            XCTAssertNotNil(store.drumDesignRequest); XCTAssertFalse(store.isPlaying); XCTAssertFalse(store.drumDesigner.previewing)
            let window = NSWindow(); let other = NSWindow()
            XCTAssertTrue(DesignerKeyboardRouting.accepts(eventWindow: window, designerWindow: window, isKey: true, hasNestedSheet: false, responder: NSView()))
            XCTAssertFalse(DesignerKeyboardRouting.accepts(eventWindow: other, designerWindow: window, isKey: true, hasNestedSheet: false, responder: NSView()))
            XCTAssertFalse(DesignerKeyboardRouting.accepts(eventWindow: window, designerWindow: window, isKey: true, hasNestedSheet: true, responder: NSView()))
            XCTAssertFalse(DesignerKeyboardRouting.accepts(eventWindow: window, designerWindow: window, isKey: true, hasNestedSheet: false, responder: NSTextView()))
            store.drumDesigner.close(); XCTAssertNil(store.drumDesignRequest)
        }
    }

}
