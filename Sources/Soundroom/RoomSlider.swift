import SwiftUI
import AppKit

/// A compact drawing over native slider tracking, focus and accessibility.
struct RoomSlider: NSViewRepresentable {
    @Binding var value: Double
    let range: ClosedRange<Double>
    var label: String = ""
    var onEditingChanged: (Bool) -> Void = { _ in }
    @Environment(\.isEnabled) private var enabled

    init(value: Binding<Double>, in range: ClosedRange<Double>, label: String = "", onEditingChanged: @escaping (Bool) -> Void = { _ in }) {
        self._value = value; self.range = range; self.label = label; self.onEditingChanged = onEditingChanged
    }
    func makeCoordinator() -> Coordinator { Coordinator(self) }
    func makeNSView(context: Context) -> RoomNativeSlider {
        let slider = RoomNativeSlider()
        slider.cell = RoomSliderCell()
        slider.sliderType = .linear
        slider.isContinuous = true
        slider.controlSize = .regular
        slider.target = context.coordinator
        slider.action = #selector(Coordinator.changed(_:))
        slider.onTrackingChanged = { [weak coordinator = context.coordinator] editing in
            coordinator?.parent.onEditingChanged(editing)
        }
        slider.setContentHuggingPriority(.defaultLow, for: .horizontal)
        slider.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        updateNSView(slider, context: context)
        return slider
    }
    func updateNSView(_ slider: RoomNativeSlider, context: Context) {
        context.coordinator.parent = self
        slider.minValue = range.lowerBound; slider.maxValue = range.upperBound
        slider.doubleValue = value.isFinite ? min(range.upperBound, max(range.lowerBound, value)) : range.lowerBound
        slider.isEnabled = enabled
        slider.setAccessibilityLabel(label.isEmpty ? nil : label)
    }
    static func dismantleNSView(_ slider: RoomNativeSlider, coordinator: Coordinator) {
        slider.finishTracking(); slider.onTrackingChanged = nil; slider.target = nil
    }
    final class Coordinator: NSObject {
        var parent: RoomSlider
        init(_ parent: RoomSlider) { self.parent = parent }
        @objc func changed(_ slider: RoomNativeSlider) {
            // Native arrow keys and accessibility actions arrive outside mouse tracking.
            let focusedWindow = !slider.trackingGesture && slider.window?.firstResponder === slider ? slider.window : nil
            if !slider.trackingGesture { parent.onEditingChanged(true) }
            parent.value = slider.doubleValue
            if !slider.trackingGesture { parent.onEditingChanged(false) }
            // Mouse release returns to the editor. Repeated arrow keys must stay on
            // the focused control even when an editing callback restores editor focus.
            if let focusedWindow, slider.window === focusedWindow { focusedWindow.makeFirstResponder(slider) }
        }
    }
}

final class RoomNativeSlider: NSSlider {
    private(set) var trackingGesture = false
    var onTrackingChanged: ((Bool) -> Void)?
    // A custom NSSliderCell is reported as an unknown role by modern AppKit.
    // Retain an explicit slider contract while using native mouse/key tracking.
    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .slider }
    override func accessibilityMinValue() -> Any? { minValue }
    override func accessibilityMaxValue() -> Any? { maxValue }
    override func accessibilityValue() -> Any? { doubleValue }
    override func setAccessibilityValue(_ value: Any?) {
        guard isEnabled, let number = value as? NSNumber, number.doubleValue.isFinite else { return }
        commitAccessibleValue(number.doubleValue)
    }
    override func accessibilityPerformIncrement() -> Bool { adjustAccessibleValue(direction: 1) }
    override func accessibilityPerformDecrement() -> Bool { adjustAccessibleValue(direction: -1) }
    private func adjustAccessibleValue(direction: Double) -> Bool {
        guard isEnabled else { return false }
        commitAccessibleValue(doubleValue + direction * (maxValue - minValue) / 100)
        return true
    }
    private func commitAccessibleValue(_ value: Double) {
        let next = min(maxValue, max(minValue, value))
        guard next != doubleValue else { return }
        doubleValue = next
        if let action { sendAction(action, to: target) }
    }
    override func mouseDown(with event: NSEvent) {
        guard isEnabled else { return }
        beginTracking()
        defer { finishTracking() }
        super.mouseDown(with: event)
    }
    func beginTracking() {
        guard !trackingGesture else { return }
        trackingGesture = true; onTrackingChanged?(true)
    }
    func finishTracking() {
        guard trackingGesture else { return }
        trackingGesture = false; onTrackingChanged?(false)
    }
}

final class RoomSliderCell: NSSliderCell {
    static let visualDiameter: CGFloat = 11
    override func drawBar(inside rect: NSRect, flipped: Bool) {
        let track = NSRect(x: rect.minX, y: rect.midY - 1.5, width: rect.width, height: 3)
        NSColor(calibratedRed: 0.23, green: 0.26, blue: 0.29, alpha: 1).setFill()
        NSBezierPath(roundedRect: track, xRadius: 1.5, yRadius: 1.5).fill()
        let position = min(track.maxX, max(track.minX, knobRect(flipped: flipped).midX))
        let rtl = controlView?.userInterfaceLayoutDirection == .rightToLeft
        let filled = NSRect(x: rtl ? position : track.minX, y: track.minY,
                            width: rtl ? track.maxX - position : position - track.minX, height: track.height)
        NSColor(Theme.accent).withAlphaComponent(isEnabled ? 1 : 0.35).setFill()
        NSBezierPath(roundedRect: filled, xRadius: 1.5, yRadius: 1.5).fill()
    }
    override func drawKnob(_ knobRect: NSRect) {
        // Do not override knobRect or hit-testing: the full native target remains.
        let diameter = Self.visualDiameter
        let visual = NSRect(x: knobRect.midX - diameter / 2, y: knobRect.midY - diameter / 2,
                            width: diameter, height: diameter)
        let path = NSBezierPath(ovalIn: visual)
        (isEnabled ? NSColor(white: 0.86, alpha: 1) : NSColor(white: 0.46, alpha: 1)).setFill()
        path.fill()
        NSColor(white: 0.1, alpha: 0.45).setStroke(); path.lineWidth = 0.5; path.stroke()
    }
}
