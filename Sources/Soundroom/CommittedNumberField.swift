import SwiftUI

/// A text-editing transaction stays separate from the valid numeric model.
/// Finishing the transaction consumes it, so Return followed by blur commits once.
struct CommittedNumberDraft {
    var text: String
    private(set) var editing = false
    private var initialText: String

    init(value: Double, fractionDigits: Int, locale: Locale) {
        text = Self.display(value, fractionDigits: fractionDigits, locale: locale)
        initialText = text
    }
    mutating func begin() -> Bool {
        guard !editing else { return false }
        initialText = text
        editing = true
        return true
    }
    mutating func synchronize(value: Double, fractionDigits: Int, locale: Locale) {
        guard !editing else { return }
        text = Self.display(value, fractionDigits: fractionDigits, locale: locale)
    }
    mutating func commit(currentValue: Double, range: ClosedRange<Double>, fractionDigits: Int, locale: Locale) -> Double? {
        guard editing else { return nil }
        editing = false
        // Formatting is presentation, not an edit: focusing "0.71" must not
        // round an untouched 0.707 model or overwrite a newer external value.
        let candidate = text == initialText ? currentValue : (Self.parse(text, locale: locale) ?? currentValue)
        let parsed = candidate.isFinite ? candidate : range.lowerBound
        let result = min(range.upperBound, max(range.lowerBound, parsed))
        text = Self.display(result, fractionDigits: fractionDigits, locale: locale)
        return result
    }
    static func parse(_ text: String, locale: Locale) -> Double? {
        var canonical = text.trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: "−", with: "-")
        if let decimal = locale.decimalSeparator, decimal != "." {
            canonical = canonical.replacingOccurrences(of: decimal, with: ".")
        }
        canonical = canonical.map { character -> String in
            if let digit = character.wholeNumberValue, (0...9).contains(digit) { return String(digit) }
            return String(character)
        }.joined()
        // Require the whole input to be numeric: partial numbers, trailing units,
        // NaN and infinity must not silently become an unintended model value.
        let syntax = #"^[+-]?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?$"#
        guard canonical.range(of: syntax, options: .regularExpression) != nil,
              let value = Double(canonical), value.isFinite else { return nil }
        return value
    }
    static func display(_ value: Double, fractionDigits: Int, locale: Locale) -> String {
        let formatter = NumberFormatter()
        formatter.locale = locale; formatter.numberStyle = .decimal; formatter.usesGroupingSeparator = false
        formatter.minimumFractionDigits = max(0, min(12, fractionDigits))
        formatter.maximumFractionDigits = formatter.minimumFractionDigits
        return formatter.string(from: NSNumber(value: value)) ?? String(value)
    }
}

/// Typed digits are validated on Return or focus loss, never while the user is
/// entering an intermediate value such as "2" on the way to "240".
struct CommittedNumberField: View {
    let title: String
    @Binding private var value: Double
    let range: ClosedRange<Double>
    let fractionDigits: Int
    let onEditingChanged: (Bool) -> Void
    let onSubmit: () -> Void
    @State private var draft: CommittedNumberDraft
    @FocusState private var focused: Bool
    @Environment(\.locale) private var locale

    init(_ title: String, value: Binding<Double>, range: ClosedRange<Double>, fractionDigits: Int = 0,
         onEditingChanged: @escaping (Bool) -> Void = { _ in }, onSubmit: @escaping () -> Void = {}) {
        self.title = title; _value = value; self.range = range; self.fractionDigits = fractionDigits
        self.onEditingChanged = onEditingChanged; self.onSubmit = onSubmit
        _draft = State(initialValue: CommittedNumberDraft(value: value.wrappedValue, fractionDigits: fractionDigits, locale: .current))
    }
    var body: some View {
        TextField(title, text: $draft.text)
            .focused($focused)
            .onAppear { draft.synchronize(value: value, fractionDigits: fractionDigits, locale: locale) }
            .onChange(of: focused) { _, new in
                if new { if draft.begin() { onEditingChanged(true) } }
                else { commit() }
            }
            .onChange(of: value) { _, new in draft.synchronize(value: new, fractionDigits: fractionDigits, locale: locale) }
            .onChange(of: locale) { _, new in draft.synchronize(value: value, fractionDigits: fractionDigits, locale: new) }
            .onSubmit {
                commit()
                focused = false
                onSubmit()
            }
            .onDisappear { commit() }
    }
    private func commit() {
        guard let next = draft.commit(currentValue: value, range: range, fractionDigits: fractionDigits, locale: locale) else { return }
        if next != value { value = next }
        onEditingChanged(false)
    }
}
