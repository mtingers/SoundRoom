import AppKit
import SoundroomCore
import SwiftUI

struct AutomationEditingTools: View {
    @ObservedObject var store: SessionStore
    @ObservedObject var editing: AutomationEditingState
    @State private var rangeStart = 0.0
    @State private var rangeEnd = 1.0
    @State private var moveSeconds = 0.0
    private let contextID: String
    init(store: SessionStore) {
        self.store = store
        self.editing = store.automationEditing
        contextID = store.automationContextID
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Picker(
                    "Automation mode",
                    selection: Binding(
                        get: { store.automationWrite ? .write : editing.mode }, set: store.setAutomationMode)
                ) {
                    ForEach(store.availableAutomationRecordModes) { Text($0.rawValue).tag($0) }
                }.pickerStyle(.segmented).labelsHidden()
            }.help(store.automationRecordingHelp)
            HStack {
                Text("At playhead")
                Spacer()
                AutomationPlayheadValue(store: store).foregroundStyle(Theme.accent)
            }.font(.system(size: 11))
            if store.automationTarget == .volume {
                Picker(
                    "Volume ramp",
                    selection: Binding(
                        get: { store.currentAutomationScale },
                        set: { scale in
                            if let id = store.selectedTrack {
                                store.updateTrack(id, actionName: "Volume interpolation") {
                                    $0.volumeAutomationScale = scale
                                }
                            }
                            store.focusEditor()
                        })
                ) { ForEach(AutomationValueScale.allCases) { Text($0.name).tag($0) } }.font(.system(size: 11))
                    .help(
                        "Decibels gives an even change in loudness. Silence uses a -80 dB ramp floor and reaches true zero at its endpoint."
                    )
            }
            HStack(spacing: 4) {
                CommittedNumberField(
                    "Start", value: $rangeStart, range: 0...(Double(Int.max / 4) / sessionRate), fractionDigits: 3
                ).accessibilityLabel("Automation range start seconds")
                Text("to")
                CommittedNumberField(
                    "End", value: $rangeEnd, range: 0...(Double(Int.max / 4) / sessionRate), fractionDigits: 3
                ).accessibilityLabel("Automation range end seconds")
                Text("s")
                Button("Select") { selectRange() }
            }.font(.system(size: 10)).textFieldStyle(.roundedBorder)
            HStack {
                Text("\(editing.selectedFrames.count) selected").foregroundStyle(Theme.secondary)
                Spacer()
                Button("Audio range") {
                    if let range = store.selectedRange {
                        store.selectAutomationRange(range)
                        updateRangeFields()
                    }
                }.disabled(store.selectedRange == nil)
            }.font(.system(size: 10))
            HStack(spacing: 8) {
                Button("Copy") { store.copyAutomationRange() }.disabled(store.automationSelectionRange == nil)
                Button("Paste") { store.pasteAutomation() }.disabled(!store.canPasteAutomation)
                Button("Duplicate") { store.duplicateAutomationRange() }.disabled(store.automationSelectionRange == nil)
                Button {
                    store.deleteSelectedAutomationPoints()
                } label: {
                    Image(systemName: "trash")
                }.disabled(editing.selectedFrames.isEmpty).help("Delete selected points")
            }.font(.system(size: 10))
            HStack(spacing: 6) {
                CommittedNumberField(
                    "Move seconds", value: $moveSeconds,
                    range: -(Double(Int.max / 4) / sessionRate)...(Double(Int.max / 4) / sessionRate),
                    fractionDigits: 3,
                    onSubmit: {
                        moveSelection()
                        store.focusEditor()
                    }
                ).frame(width: 65).accessibilityLabel("Move selected automation by seconds")
                Button("Move") { moveSelection() }.disabled(editing.selectedFrames.isEmpty)
                Menu("Curve") {
                    ForEach(AutomationCurve.allCases) { curve in Button(curve.name) { store.setAutomationCurve(curve) }
                    }
                }.disabled(editing.selectedFrames.isEmpty)
                Button("Simplify") { store.simplifyCurrentAutomation() }.disabled(store.automationPoints.count < 3)
                    .help(
                        "Reduce linear gesture points within 0.2% normalized value; decibel curves within 0.2 dB. Nonlinear transitions are preserved."
                    )
            }.font(.system(size: 10)).textFieldStyle(.roundedBorder)
        }.onAppear { updateRangeFields() }.onChange(of: editing.range) { _, _ in updateRangeFields() }
    }
    private func updateRangeFields() {
        guard let range = store.automationSelectionRange else { return }
        rangeStart = Double(range.lowerBound) / sessionRate
        rangeEnd = Double(range.upperBound) / sessionRate
    }
    private func selectRange() {
        finishNumberEntry {
            guard rangeStart.isFinite, rangeEnd.isFinite else { return }
            let first = Int(max(0, min(Double(Int.max / 2), rangeStart * sessionRate)))
            let end = Int(max(Double(first + 1), min(Double(Int.max / 2), rangeEnd * sessionRate)))
            store.selectAutomationRange(first..<end)
            store.focusEditor()
        }
    }
    private func moveSelection() {
        finishNumberEntry {
            guard moveSeconds.isFinite else { return }
            store.moveAutomationGroup(
                frames: editing.selectedFrames,
                by: Int(max(-Double(Int.max / 2), min(Double(Int.max / 2), moveSeconds * sessionRate))), valueDelta: 0)
            store.focusEditor()
        }
    }
    private func finishNumberEntry(_ action: @escaping () -> Void) {
        NSApp.keyWindow?.makeFirstResponder(nil)
        // Focus loss commits the draft during SwiftUI's update; read it afterward.
        DispatchQueue.main.async {
            guard store.automationContextID == contextID else { return }
            action()
        }
    }
}
