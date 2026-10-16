import XCTest
@testable import Soundroom

final class CommittedNumberFieldTests: XCTestCase {
    private let english = Locale(identifier: "en_US")
    func testTypingTempoDoesNotClampIntermediateDigitsAndReturnBlurCommitsOnce() {
        var draft = CommittedNumberDraft(value: 120, fractionDigits: 0, locale: english)
        var model = 120.0, writes = 0
        XCTAssertTrue(draft.begin()); XCTAssertFalse(draft.begin())
        for text in ["2", "24", "240"] { draft.text = text; XCTAssertEqual(model, 120) }
        if let next = draft.commit(currentValue: model, range: 30...300, fractionDigits: 0, locale: english) { model = next; writes += 1 }
        XCTAssertEqual(model, 240); XCTAssertEqual(draft.text, "240")
        XCTAssertNil(draft.commit(currentValue: model, range: 30...300, fractionDigits: 0, locale: english), "The following blur must not commit a second transaction")
        XCTAssertEqual(writes, 1)
    }
    func testCommitClampsFinalValueAndInvalidInputRetainsValidModel() {
        for (text, expected) in [("2", 30.0), ("301", 300.0), ("-50", 30.0), ("", 120.0), ("240ms", 120.0), ("nan", 120.0), ("inf", 120.0), ("1e999", 120.0), ("2.4e2", 240.0)] {
            var draft = CommittedNumberDraft(value: 120, fractionDigits: 0, locale: english)
            _ = draft.begin(); draft.text = text
            XCTAssertEqual(draft.commit(currentValue: 120, range: 30...300, fractionDigits: 0, locale: english), expected, text)
        }
    }
    func testDecimalDraftSupportsNegativeIntermediateTextAndLocale() {
        let german = Locale(identifier: "de_DE")
        var draft = CommittedNumberDraft(value: 0, fractionDigits: 2, locale: german)
        XCTAssertEqual(draft.text, "0,00")
        _ = draft.begin(); draft.text = "−"; draft.text = "−1,25"
        XCTAssertEqual(draft.commit(currentValue: 0, range: -12...12, fractionDigits: 2, locale: german), -1.25)
        XCTAssertEqual(draft.text, "-1,25")
        XCTAssertEqual(CommittedNumberDraft.parse(".125", locale: english), 0.125)
        XCTAssertNil(CommittedNumberDraft.parse("1.2.3", locale: english))
        XCTAssertNil(CommittedNumberDraft.parse("1 2", locale: english))
    }
    func testExternalSliderSynchronizesOnlyWhenFieldIsNotBeingEdited() {
        var draft = CommittedNumberDraft(value: 120, fractionDigits: 0, locale: english)
        draft.synchronize(value: 140, fractionDigits: 0, locale: english); XCTAssertEqual(draft.text, "140")
        _ = draft.begin(); draft.text = "24"
        draft.synchronize(value: 180, fractionDigits: 0, locale: english); XCTAssertEqual(draft.text, "24")
        draft.text = "240"
        XCTAssertEqual(draft.commit(currentValue: 180, range: 30...300, fractionDigits: 0, locale: english), 240)
        draft.synchronize(value: 200, fractionDigits: 0, locale: english); XCTAssertEqual(draft.text, "200")
    }
    func testEffectNumericRangesRetainTypedPrecisionAndDisplayUnitValues() {
        var cutoff = CommittedNumberDraft(value: 80, fractionDigits: 0, locale: english)
        _ = cutoff.begin()
        for text in ["2", "24", "240", "2400"] { cutoff.text = text }
        XCTAssertEqual(cutoff.commit(currentValue: 80, range: 20...12_000, fractionDigits: 0, locale: english), 2400)
        var attack = CommittedNumberDraft(value: 10, fractionDigits: 2, locale: english)
        _ = attack.begin(); attack.text = "0.25"
        XCTAssertEqual(attack.commit(currentValue: 10, range: 0.1...200, fractionDigits: 2, locale: english), 0.25)
        XCTAssertEqual(attack.text, "0.25")
        var threshold = CommittedNumberDraft(value: -18, fractionDigits: 1, locale: english)
        _ = threshold.begin(); threshold.text = "-12.5"
        XCTAssertEqual(threshold.commit(currentValue: -18, range: -60...0, fractionDigits: 1, locale: english), -12.5)
        var feedback = CommittedNumberDraft(value: 30, fractionDigits: 0, locale: english)
        _ = feedback.begin(); feedback.text = "95"
        let displayValue = feedback.commit(currentValue: 30, range: 0...95, fractionDigits: 0, locale: english)
        XCTAssertEqual(displayValue.map { $0 / 100 }, 0.95)
    }
    func testUntouchedRoundedDisplayPreservesPreciseModelAndProducesNoWrite() {
        // Match Float effect values and percentage scaling used by the live UI.
        for (model, range) in [(Double(Float(0.707)), 0.05...20.0), (Double(Float(0.3)) * 100, 0...95.0)] {
            var draft = CommittedNumberDraft(value: model, fractionDigits: 2, locale: english)
            _ = draft.begin()
            let committed = draft.commit(currentValue: model, range: range, fractionDigits: 2, locale: english)
            XCTAssertEqual(committed, model)
            XCTAssertFalse(committed != model, "The field's binding setter must not run and create an undo or Write automation point")
            XCTAssertNil(draft.commit(currentValue: model, range: range, fractionDigits: 2, locale: english))
        }
    }
    func testUntouchedDraftPreservesNewExternalValueDespiteOldDisplayedText() {
        var draft = CommittedNumberDraft(value: 0.707, fractionDigits: 2, locale: english)
        _ = draft.begin()
        draft.synchronize(value: 0.91357, fractionDigits: 2, locale: english)
        XCTAssertEqual(draft.text, "0.71")
        XCTAssertEqual(draft.commit(currentValue: 0.91357, range: 0.05...20, fractionDigits: 2, locale: english), 0.91357)
        XCTAssertEqual(draft.text, "0.91")
    }
}
