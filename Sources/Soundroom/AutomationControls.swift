import SwiftUI
import SoundroomCore

struct AutomationControls: View {
    @ObservedObject var store: SessionStore
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Divider()
            HStack {
                Text(store.automationTitle).font(.system(size: 12, weight: .medium))
                Spacer()
                if !store.automationPoints.isEmpty { Button("Clear") { store.clearCurrentAutomation() }.font(.system(size: 10)) }
            }
            AutomationEditingTools(store: store)
            Button { store.putCurrentAutomationPoint(frame: store.playhead, value: store.automationValueAtPlayhead); store.focusEditor() } label: {
                Label("Point at playhead", systemImage: "plus.circle").frame(maxWidth: .infinity)
            }.buttonStyle(RoomButtonStyle()).font(.system(size: 11))
            if store.automationTarget == .volume {
                HStack {
                    Button("Fade In") { store.fadeVolumeSelection(out: false) }
                    Button("Fade Out") { store.fadeVolumeSelection(out: true) }
                }.font(.system(size: 11)).disabled((store.selectedRange?.count ?? 0) < 2)
                    .help("Create an editable volume ramp across the selected audio.")
            }
            ForEach(store.automationPoints.sorted { $0.frame < $1.frame }) { point in
                AutomationPointControls(store: store, point: point)
            }
            if store.automationPoints.isEmpty {
                Text("Click the lane to add points.").font(.system(size: 11)).foregroundStyle(Theme.secondary)
            }
        }
    }
}

private struct AutomationPointControls: View {
    @ObservedObject var store: SessionStore
    @ObservedObject var editing: AutomationEditingState
    let point: AutomationPoint
    private let contextID: String
    init(store: SessionStore, point: AutomationPoint) {
        self.store = store; self.editing = store.automationEditing; self.point = point
        contextID = store.automationContextID
    }
    private var currentValue: Float { store.automationPoints.first { $0.frame == point.frame }?.value ?? point.value }
    private var unit: String {
        if let parameter = store.automationTarget.mixParameter { return parameter == .volume ? "dB" : "%" }
        return descriptor?.unit ?? "%"
    }
    private var descriptor: EffectParameter? {
        guard let effect = store.currentRealtimeEffect, let key = effect.selectedAutomationParameter else { return nil }
        return effect.kind.parameters.first { $0.id == key }
    }
    private var number: Double {
        if let parameter = store.automationTarget.mixParameter {
            let physical = parameter.physical(currentValue)
            return parameter == .volume ? (physical > 0 ? Double(20 * log10(physical)) : -80) : Double(physical * 100)
        }
        if let descriptor { return Double(descriptor.physical(currentValue) * (descriptor.unit == "%" ? 100 : 1)) }
        return Double(currentValue * 100)
    }
    private var numberRange: ClosedRange<Double> {
        if let parameter = store.automationTarget.mixParameter {
            return parameter == .volume ? -80...Double(20 * log10(Float(1.5))) : -100...100
        }
        if let descriptor {
            let scale: Double = descriptor.unit == "%" ? 100 : 1
            return Double(descriptor.minimum) * scale...Double(descriptor.maximum) * scale
        }
        return 0...100
    }
    var body: some View {
        VStack(spacing: 4) {
            HStack(spacing: 4) {
                Toggle("Select", isOn: Binding(get: { store.automationEditing.selectedFrames.contains(point.frame) }, set: { _ in store.selectAutomationPoint(point.frame, extending: true) })).labelsHidden().toggleStyle(.checkbox).accessibilityLabel("Select automation point at \(timeString(point.frame))")
                CommittedNumberField("Time", value: Binding(get: { Double(point.frame) / sessionRate }, set: { seconds in
                    guard store.automationContextID == contextID,
                          store.automationPoints.contains(where: { $0.frame == point.frame }) else { return }
                    store.putCurrentAutomationPoint(frame: Int((seconds * sessionRate).rounded()), value: currentValue, replacing: point.frame)
                }), range: 0...(Double(Int.max / 4) / sessionRate), fractionDigits: 3, onSubmit: { store.focusEditor() })
                    .frame(width: 58).accessibilityLabel("Automation time in seconds at \(timeString(point.frame))")
                Text("s").foregroundStyle(Theme.secondary)
                Spacer(minLength: 0)
                CommittedNumberField("Value", value: Binding(get: { number }, set: setNumber), range: numberRange, fractionDigits: 1, onSubmit: { store.focusEditor() })
                    .frame(width: 56).accessibilityLabel("Automation value in \(unit) at \(timeString(point.frame))")
                Text(unit).foregroundStyle(Theme.secondary)
                Button { store.deleteCurrentAutomationPoint(frame: point.frame) } label: { Image(systemName: "minus.circle") }
                    .buttonStyle(.plain).help("Delete point").accessibilityLabel("Delete point at \(timeString(point.frame))")
            }.font(.system(size: 10)).textFieldStyle(.roundedBorder)
            Picker("Transition", selection: Binding(get: { store.automationPoints.first { $0.frame == point.frame }?.curve ?? point.curve }, set: { store.setAutomationCurve($0, frames: [point.frame]) })) {
                ForEach(AutomationCurve.allCases) { Text($0.name).tag($0) }
            }.font(.system(size: 10)).accessibilityLabel("Transition after point at \(timeString(point.frame))")
            RoomSlider(value: Binding(get: { Double(currentValue) }, set: { store.putCurrentAutomationPoint(frame: point.frame, value: Float($0), replacing: point.frame, coalescing: true) }), in: 0...1, label: "\(store.automationTitle) at \(timeString(point.frame))",
                onEditingChanged: { editing in
                    if editing { store.beginContinuousEdit(store.automationEditKey(frame: point.frame)) }
                    else { store.endContinuousEdit(); store.focusEditor() }
                }).accessibilityLabel("Automation value at \(timeString(point.frame))").accessibilityValue(store.automationDisplay(currentValue))
        }
    }
    private func setNumber(_ number: Double) {
        guard number.isFinite, store.automationContextID == contextID,
              store.automationPoints.contains(where: { $0.frame == point.frame }) else { return }
        let value: Float
        if let parameter = store.automationTarget.mixParameter {
            let physical: Float = parameter == .volume ? (number <= -80 ? 0 : pow(10, Float(min(4, number)) / 20)) : Float(number / 100)
            value = parameter.normalized(physical)
        } else if let descriptor { value = descriptor.normalized(Float(number) / (descriptor.unit == "%" ? 100 : 1)) }
        else { value = Float(number / 100) }
        store.putCurrentAutomationPoint(frame: point.frame, value: value, replacing: point.frame)
    }
}
