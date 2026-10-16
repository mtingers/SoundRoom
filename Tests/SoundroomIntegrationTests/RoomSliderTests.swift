import XCTest
import AppKit
import SwiftUI
import SoundroomCore
@testable import Soundroom

final class RoomSliderTests: XCTestCase {
    @MainActor func testMouseTrackingGroupsValuesInsideOneEditAndFinishesOnce() {
        var value = 0.0, events: [String] = []
        let control = RoomSlider(value: Binding(get: { value }, set: { value = $0; events.append("value") }), in: 0...1,
                                 onEditingChanged: { events.append($0 ? "begin" : "end") })
        let coordinator = control.makeCoordinator()
        let slider = RoomNativeSlider()
        slider.minValue = 0; slider.maxValue = 1
        slider.onTrackingChanged = control.onEditingChanged
        slider.beginTracking(); slider.beginTracking()
        slider.doubleValue = 0.25; coordinator.changed(slider)
        slider.doubleValue = 0.75; coordinator.changed(slider)
        slider.finishTracking(); slider.finishTracking()
        XCTAssertEqual(value, 0.75)
        XCTAssertEqual(events, ["begin", "value", "value", "end"])
        XCTAssertFalse(slider.trackingGesture)
    }
    @MainActor func testKeyboardAndAccessibilityActionTargetsBracketValuesWithEditingCallbacks() {
        var value = 0.0, events: [String] = []
        let control = RoomSlider(value: Binding(get: { value }, set: { value = $0; events.append("value") }), in: -1...1,
                                 label: "Track pan", onEditingChanged: { events.append($0 ? "begin" : "end") })
        let coordinator = control.makeCoordinator()
        let slider = RoomNativeSlider(frame: NSRect(x: 0, y: 0, width: 220, height: 24))
        slider.cell = RoomSliderCell(); slider.minValue = -1; slider.maxValue = 1
        slider.target = coordinator; slider.action = #selector(RoomSlider.Coordinator.changed(_:))
        slider.setAccessibilityLabel("Track pan")
        slider.doubleValue = 0.5
        XCTAssertTrue(slider.sendAction(slider.action!, to: coordinator))
        XCTAssertEqual(value, 0.5)
        XCTAssertEqual(events, ["begin", "value", "end"])
        XCTAssertEqual(slider.accessibilityLabel(), "Track pan")
        XCTAssertEqual(slider.accessibilityRole(), .slider)
        XCTAssertEqual(slider.accessibilityMinValue() as? Double, -1)
        XCTAssertEqual(slider.accessibilityMaxValue() as? Double, 1)
        XCTAssertTrue(slider.acceptsFirstResponder)
    }
    @MainActor func testSmallDrawingRetainsNativeKnobGeometry() {
        let slider = RoomNativeSlider(frame: NSRect(x: 0, y: 0, width: 220, height: 24))
        let cell = RoomSliderCell()
        slider.cell = cell; slider.minValue = 0; slider.maxValue = 1; slider.doubleValue = 0.5
        let knob = cell.knobRect(flipped: false)
        XCTAssertGreaterThan(knob.width, RoomSliderCell.visualDiameter)
        XCTAssertGreaterThan(knob.height, RoomSliderCell.visualDiameter)
    }
    @MainActor func testDiscreteChangesRetainFocusedSliderWithoutStealingUnfocusedControl() {
        let window = NSWindow(), content = NSView(frame: NSRect(x: 0, y: 0, width: 300, height: 100))
        let slider = RoomNativeSlider(frame: NSRect(x: 10, y: 10, width: 220, height: 24)), field = NSTextView()
        content.addSubview(slider); content.addSubview(field); window.contentView = content
        slider.minValue = 0; slider.maxValue = 1
        var value = 0.0
        let control = RoomSlider(value: Binding(get: { value }, set: { value = $0 }), in: 0...1,
                                 onEditingChanged: { editing in if !editing { window.makeFirstResponder(field) } })
        let coordinator = control.makeCoordinator()
        XCTAssertTrue(window.makeFirstResponder(slider))
        slider.doubleValue = 0.25; coordinator.changed(slider)
        XCTAssertTrue(window.firstResponder === slider, "A second arrow must adjust the slider rather than nudge selected audio")
        XCTAssertTrue(window.makeFirstResponder(field))
        slider.doubleValue = 0.5; coordinator.changed(slider)
        XCTAssertTrue(window.firstResponder === field, "An unfocused accessibility action must not steal typing focus")
    }
    @MainActor func testActualAccessibilityAdjustmentAndSetUseTransactionsAndRespectBounds() {
        // NSControl routes target/action through NSApp, including AX adjustments.
        // This test runs before the NSWindow fixtures that otherwise create it.
        _ = NSApplication.shared
        var value = 5.0, events: [String] = []
        let control = RoomSlider(value: Binding(get: { value }, set: { value = $0; events.append("value") }), in: 0...10,
                                 onEditingChanged: { events.append($0 ? "begin" : "end") })
        let coordinator = control.makeCoordinator(), slider = RoomNativeSlider()
        slider.cell = RoomSliderCell(); slider.minValue = 0; slider.maxValue = 10; slider.doubleValue = 5
        slider.target = coordinator; slider.action = #selector(RoomSlider.Coordinator.changed(_:))
        // NSControl's target is weak. SwiftUI owns this coordinator in production;
        // this standalone fixture must retain it across optimized native actions.
        withExtendedLifetime(coordinator) {
            XCTAssertTrue(slider.accessibilityPerformIncrement()); XCTAssertGreaterThan(value, 5)
            XCTAssertTrue(slider.accessibilityPerformDecrement()); XCTAssertEqual(value, 5, accuracy: 0.000001)
            slider.setAccessibilityValue(NSNumber(value: 12)); XCTAssertEqual(value, 10)
            XCTAssertEqual(events, Array(repeating: ["begin", "value", "end"], count: 3).flatMap { $0 })
            XCTAssertTrue(slider.accessibilityPerformIncrement()); XCTAssertEqual(events.count, 9, "A bound must not create an edit")
            slider.setAccessibilityValue(NSNumber(value: Double.nan)); XCTAssertEqual(value, 10)
            slider.isEnabled = false
            XCTAssertFalse(slider.accessibilityPerformDecrement())
            slider.setAccessibilityValue(NSNumber(value: 0)); XCTAssertEqual(value, 10)
            XCTAssertEqual(events.count, 9)
        }
    }

    func testDrawerClampsNonfiniteAndExtremePreferencesWithoutTakingTimelineSpace() {
        for available: CGFloat in [950, 1024, 1260, 1600] {
            for preferred in [-100.0, 0, 320, 460, 1000, .nan, .infinity] {
                let width = EffectDrawerLayout.width(preferred: preferred, available: available)
                XCTAssertTrue(width.isFinite)
                XCTAssertGreaterThanOrEqual(width, EffectDrawerLayout.minimumWidth)
                XCTAssertLessThanOrEqual(width, EffectDrawerLayout.maximumWidth)
                XCTAssertGreaterThanOrEqual(available - width - EffectDrawerLayout.dividerWidth, 620)
            }
        }
        XCTAssertEqual(EffectDrawerLayout.width(preferred: 460, available: 950), 324)
        XCTAssertEqual(EffectDrawerLayout.width(preferred: .nan, available: 1260), 320)
    }
}
