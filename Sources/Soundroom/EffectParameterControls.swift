import SoundroomCore
import SwiftUI

/// Shared physical controls for selection processing and live inserts.
struct EffectParameterControls: View {
    @ObservedObject var library: EffectPresetLibrary
    let kind: RealtimeEffectKind
    let values: [String: Float]
    let onChange: (String, Float) -> Void
    var onEditingChanged: (String, Bool) -> Void = { _, _ in }
    var onNumericSubmit: () -> Void = {}
    var onPreset: (([String: Float]) -> Void)? = nil
    var onGraphChange: (([String: Float]) -> Void)? = nil
    var legacyAmount: Float? = nil
    let onSavedPreset: (SavedEffectPreset) -> Void
    @State private var dialog: EffectPresetDialog?
    @State private var eqBand = EqualizerBand.mid
    @State private var reverbSection = "space"
    @State private var chosenPreset: ChosenEffectPreset?
    @State private var professionalSection: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            presetToolbar
            Text(presetState).font(.system(size: 10)).foregroundStyle(Theme.secondary)
                .lineLimit(2).accessibilityLabel("Preset: \(presetState)")
            if legacyAmount == nil {
                if kind == .pitchCorrection {
                    Text("Single voice · 85 ms look-ahead").font(.system(size: 10)).foregroundStyle(Theme.secondary)
                        .help(
                            "Designed for one voice or instrument, about 50–1000 Hz. Chords and breathy or irregular voices may bypass or produce artifacts. Retune changes note transitions; Humanize relaxes sustained notes; Formant changes voice color independently from pitch."
                        )
                    ForEach(visibleParameters) { parameter in parameterRow(parameter) }
                } else if kind == .guitarRig {
                    GuitarRigBuilder(values: values, selected: $professionalSection)
                    let groups = EffectControlPresentation.groups(for: kind)
                    if let group = groups.first(where: { $0.id == professionalSection })
                        ?? groups.first(where: { $0.id == "amp" })
                    {
                        groupCard(group)
                    }
                } else if kind == .soundPutty {
                    SoundPuttyEditor(
                        values: values,
                        onChange: { edits in
                            if let onGraphChange {
                                onGraphChange(edits)
                            } else {
                                for (key, value) in edits { onChange(key, value) }
                            }
                        }, onEditingChanged: onEditingChanged)
                    soundPuttyControls
                } else if kind == .equalizer {
                    equalizerControls
                } else if kind == .room {
                    reverbControls
                } else if kind == .resonance || kind == .warmth {
                    professionalControls
                } else {
                    EffectSettingVisualization(kind: kind, values: values)
                    ForEach(EffectControlPresentation.groups(for: kind)) { group in groupCard(group) }
                }
            }
        }.sheet(item: $dialog) { action in
            switch action {
            case .save: SaveEffectPresetSheet(library: library, kind: kind, values: values, legacyAmount: legacyAmount)
            case .manage:
                ManageEffectPresetsSheet(library: library, kind: kind, values: values, legacyAmount: legacyAmount)
            }
        }
    }
    private var soundPuttyControls: some View {
        let groups = EffectControlPresentation.groups(for: kind)
        let selected = groups.first { $0.id == professionalSection } ?? groups.first
        return VStack(alignment: .leading, spacing: 8) {
            LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 4) {
                ForEach(groups) { group in
                    Button(group.title) { professionalSection = group.id }
                        .buttonStyle(RoomButtonStyle(prominent: selected?.id == group.id))
                        .accessibilityLabel("Edit Sound Putty \(group.title.lowercased())")
                }
            }.font(.system(size: 10))
            if let selected { groupCard(selected) }
        }
    }
    private var professionalControls: some View {
        let groups = EffectControlPresentation.groups(for: kind)
        let selected = groups.first { $0.id == professionalSection } ?? groups.first
        return VStack(alignment: .leading, spacing: 10) {
            EffectSettingVisualization(kind: kind, values: values)
            LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 4) {
                ForEach(groups) { group in
                    Button {
                        professionalSection = group.id
                    } label: {
                        Text(group.id == "region" ? "Region" : group.id == "listen" ? "Listen" : group.title).frame(
                            maxWidth: .infinity)
                    }
                    .buttonStyle(RoomButtonStyle(prominent: selected?.id == group.id))
                    .accessibilityLabel("Edit \(kind.name.lowercased()) \(group.title.lowercased())")
                }
            }.font(.system(size: 10))
            if let selected { groupCard(selected) }
        }
    }
    private var reverbControls: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let character = kind.parameters.first(where: { $0.id == "character" }) { parameterRow(character) }
            EffectSettingVisualization(kind: kind, values: values)
            HStack(spacing: 3) {
                ForEach(EffectControlPresentation.groups(for: kind)) { group in
                    Button(group.id == "texture" ? "Reflections" : group.title) { reverbSection = group.id }
                        .buttonStyle(RoomButtonStyle(prominent: group.id == reverbSection))
                        .accessibilityLabel(
                            "Edit reverb \(group.id == "texture" ? "reflections" : group.title.lowercased())")
                }
            }.font(.system(size: 10))
            if let group = EffectControlPresentation.groups(for: kind).first(where: { $0.id == reverbSection }) {
                groupCard(EffectParameterGroup(group.id, group.title, group.parameterIDs.filter { $0 != "character" }))
            }
        }
    }
    private var equalizerControls: some View {
        VStack(alignment: .leading, spacing: 10) {
            EqualizerResponseView(
                values: values, selectedBand: eqBand, onSelect: { eqBand = $0 },
                onChange: { edits in
                    if let onGraphChange {
                        onGraphChange(edits)
                    } else {
                        for (key, value) in edits { onChange(key, value) }
                    }
                }, onEditingChanged: { onEditingChanged("graph", $0) })
            LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 4), count: 4), spacing: 4) {
                ForEach(EqualizerBand.allCases) { band in
                    Button {
                        eqBand = band
                    } label: {
                        Text(band.name).lineLimit(1).frame(maxWidth: .infinity)
                    }.buttonStyle(RoomButtonStyle(prominent: band == eqBand))
                        .accessibilityLabel("Edit \(band.name) EQ band")
                }
            }.font(.system(size: 10))
            if let group = EffectControlPresentation.groups(for: kind).first(where: { $0.id == eqBand.groupID }) {
                if eqBand == .highPass || eqBand == .lowPass {
                    groupCard(EffectParameterGroup(eqBand.id, eqBand.name + " · 12 dB/oct", eqBand.parameterIDs))
                } else {
                    groupCard(group)
                }
            }
            if [.band4, .band5, .band6].contains(eqBand),
                let timing = EffectControlPresentation.groups(for: kind).first(where: { $0.id == "dynamic" })
            {
                Text(
                    "Dynamic cut at 0 dB is off. The bell responds only above its threshold; the graph shows its static gain."
                ).font(.caption).foregroundStyle(Theme.secondary)
                groupCard(timing)
            }
            if let output = EffectControlPresentation.groups(for: kind).last { groupCard(output) }
        }
    }
    private var presetToolbar: some View {
        HStack {
            Menu("Presets") {
                Section("Built-in") {
                    Button("Default") { select("Default", parameters: kind.defaultParameters) }
                    ForEach(kind.presets) { preset in
                        Button(preset.name) { select(preset.name, parameters: preset.values) }
                    }
                }
                if !library.presets(for: kind).isEmpty {
                    Section("My Presets") {
                        ForEach(library.presets(for: kind)) { preset in
                            Button(preset.name) {
                                finishNumberEntry {
                                    chosenPreset = ChosenEffectPreset(
                                        name: preset.name, values: preset.parameters, amount: preset.legacyAmount)
                                    onSavedPreset(preset)
                                }
                            }
                        }
                    }
                }
                Divider()
                Button("Save Preset…") { finishNumberEntry { dialog = .save } }.disabled(library.loadError != nil)
                Button("Manage Presets…") { finishNumberEntry { dialog = .manage } }
            }.menuStyle(.borderlessButton).accessibilityLabel("\(kind.name) presets")
            Spacer()
            Button("Reset") { select("Default", parameters: kind.defaultParameters) }
                .font(.system(size: 10)).help("Reset parameter values; keep automation")
        }
    }
    private func groupCard(_ group: EffectParameterGroup) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(group.title).font(.system(size: 11, weight: .semibold)).foregroundStyle(Theme.secondary)
            ForEach(group.parameterIDs, id: \.self) { key in
                if let parameter = kind.parameters.first(where: { $0.id == key }) {
                    parameterRow(parameter)
                        .disabled(
                            kind == .room && (values["character"] ?? 0) < 0.5
                                && ["lowCut", "diffusion", "early", "width"].contains(key)
                        )
                        .disabled(kind == .soundPutty && key == "variationSeed" && (values["wildMode"] ?? 0) != 0)
                        .disabled(kind == .utility && key == "width" && (values["mono"] ?? 0) >= 0.5)
                        .disabled(kind == .guitarRig && GuitarRigBuilder.isInactive(key, values: values))
                }
            }
        }.padding(10).frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.card, in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).stroke(Theme.line))
    }
    @ViewBuilder private func parameterRow(_ parameter: EffectParameter) -> some View {
        if kind == .pitchCorrection && parameter.id == "mask" {
            if Int(values["scale"] ?? 0) == 3 {
                PitchTargetControls(
                    mask: Int(parameter.clamped(values["mask"] ?? parameter.defaultValue)),
                    onChange: { mask in
                        onEditingChanged("mask", true)
                        onChange("mask", Float(mask))
                        onEditingChanged("mask", false)
                    })
            }
        } else if let choices = parameter.choices {
            Picker(
                parameter.name,
                selection: Binding(
                    get: {
                        Int(parameter.clamped(values[parameter.id] ?? parameter.defaultValue))
                    },
                    set: { value in
                        onEditingChanged(parameter.id, true)
                        onChange(parameter.id, Float(value))
                        onEditingChanged(parameter.id, false)
                    })
            ) {
                ForEach(choices.indices, id: \.self) { index in Text(choices[index]).tag(index + Int(parameter.minimum))
                }
            }.font(.system(size: 11)).accessibilityLabel(parameter.name)
                .disabled(
                    kind == .pitchCorrection && parameter.id == "key" && ![1, 2].contains(Int(values["scale"] ?? 0))
                )
                .help(
                    kind == .pitchCorrection && parameter.id == "key"
                        ? "Key transposes major and minor scales. Custom note targets stay absolute." : hint(parameter))
        } else {
            VStack(spacing: 4) {
                HStack(spacing: 4) {
                    Text(parameter.name).lineLimit(1).minimumScaleFactor(0.9)
                    Spacer(minLength: 2)
                    CommittedNumberField(
                        parameter.name, value: physicalBinding(parameter), range: displayRange(parameter),
                        fractionDigits: EffectControlPresentation.fractionDigits(for: parameter),
                        onEditingChanged: { onEditingChanged(parameter.id, $0) }, onSubmit: onNumericSubmit
                    )
                    .textFieldStyle(.roundedBorder).frame(width: 74).multilineTextAlignment(.trailing)
                    .accessibilityLabel("\(parameter.name) value in \(parameter.unit)")
                    Text(parameter.unit).foregroundStyle(Theme.secondary).lineLimit(1).fixedSize().frame(
                        width: parameter.unit.count > 3 ? 34 : 20, alignment: .leading)
                }.font(.system(size: 11)).help(hint(parameter))
                RoomSlider(
                    value: Binding(
                        get: { Double(parameter.normalized(values[parameter.id] ?? parameter.defaultValue)) },
                        set: { onChange(parameter.id, parameter.physical(Float($0))) }), in: 0...1,
                    label: parameter.name,
                    onEditingChanged: { onEditingChanged(parameter.id, $0) }
                )
                .accessibilityLabel(parameter.name).accessibilityValue(
                    parameter.display(values[parameter.id] ?? parameter.defaultValue)
                )
                .help(hint(parameter))
            }
        }
    }
    private var visibleParameters: [EffectParameter] {
        EffectControlPresentation.visibleParameterIDs(for: kind).compactMap { id in
            kind.parameters.first { $0.id == id }
        }
    }
    private func hint(_ parameter: EffectParameter) -> String {
        let detail = EffectControlPresentation.hint(kind: kind, parameter: parameter.id)
        return detail.isEmpty ? parameter.display(values[parameter.id] ?? parameter.defaultValue) : detail
    }
    private func physicalBinding(_ parameter: EffectParameter) -> Binding<Double> {
        Binding(
            get: {
                Double(parameter.clamped(values[parameter.id] ?? parameter.defaultValue)) * displayScale(parameter)
            },
            set: { onChange(parameter.id, parameter.clamped(Float($0 / displayScale(parameter)))) })
    }
    private func displayScale(_ parameter: EffectParameter) -> Double { parameter.unit == "%" ? 100 : 1 }
    private func displayRange(_ parameter: EffectParameter) -> ClosedRange<Double> {
        Double(parameter.minimum) * displayScale(parameter)...Double(parameter.maximum) * displayScale(parameter)
    }
    private var presetState: String {
        if let chosenPreset {
            let matches: Bool
            if let legacyAmount {
                matches = chosenPreset.amount.map { abs($0 - legacyAmount) <= 0.000001 } ?? false
            } else {
                matches =
                    chosenPreset.amount == nil
                    && EffectControlPresentation.matches(kind: kind, values: values, presetValues: chosenPreset.values)
            }
            return chosenPreset.name + (matches ? "" : " · Modified")
                + (legacyAmount == nil ? "" : " · Legacy processor")
        }
        guard legacyAmount == nil else { return "Legacy processor" }
        if EffectControlPresentation.matches(kind: kind, values: values, presetValues: kind.defaultParameters) {
            return "Default"
        }
        if let preset = kind.presets.first(where: {
            EffectControlPresentation.matches(kind: kind, values: values, presetValues: $0.values)
        }) {
            return preset.name
        }
        if let preset = library.presets(for: kind).first(where: {
            $0.legacyAmount == nil
                && EffectControlPresentation.matches(kind: kind, values: values, presetValues: $0.parameters)
        }) {
            return preset.name
        }
        return "Custom settings"
    }
    private func select(_ name: String, parameters: [String: Float]) {
        finishNumberEntry {
            chosenPreset = ChosenEffectPreset(name: name, values: parameters, amount: nil)
            if let onPreset {
                onPreset(parameters)
            } else {
                for parameter in kind.parameters {
                    onChange(parameter.id, parameters[parameter.id] ?? parameter.defaultValue)
                }
            }
        }
    }
    /// Menu actions must finish a staged numeric transaction before changing values or opening a sheet.
    private func finishNumberEntry(_ action: @escaping () -> Void) {
        EffectWorkspaceFocus.finishNumberEntry(action)
    }
}
private struct ChosenEffectPreset {
    let name: String
    let values: [String: Float]
    let amount: Float?
}
