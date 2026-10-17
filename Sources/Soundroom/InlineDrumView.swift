import AppKit
import SoundroomCore
import SwiftUI

struct InlineDrumHeader: View {
    @ObservedObject var store: SessionStore
    @ObservedObject var controller: InlineDrumController
    @ObservedObject var editor: DrumEditorModel
    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 7) {
                HStack {
                    Label("Beat editor", systemImage: "square.grid.3x3").fontWeight(.semibold)
                    Spacer()
                    Button {
                        store.inlineDrumExpanded = false
                    } label: {
                        Image(systemName: "chevron.up")
                    }
                    .buttonStyle(.plain).help("Collapse beat editor").accessibilityLabel("Collapse beat editor")
                }
                Menu {
                    if let track = store.session.tracks.first(where: { $0.id == controller.trackID }) {
                        ForEach(track.clips.filter { $0.editableDrumPattern != nil }) { clip in
                            Button(clip.name) { store.toggleInlineDrums(trackID: track.id, clipID: clip.id) }
                        }
                    }
                } label: {
                    Text(controller.currentClip?.name ?? "Clip unavailable").lineLimit(1)
                }
                .menuStyle(.borderlessButton).help("Choose the drum clip to edit. Other clips keep their own patterns.")
            }.padding(.horizontal, 14).frame(height: InlineDrumLayout.controls)
            Text("KIT PIECES").font(.system(size: 9, weight: .semibold)).foregroundStyle(Theme.secondary)
                .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 14).frame(
                    height: InlineDrumLayout.ruler)
            ForEach(DrumVoice.allCases) { voice in
                HStack(spacing: 8) {
                    Button(voice.title) { editor.selectedVoice = voice }
                        .buttonStyle(.plain).frame(maxWidth: .infinity, alignment: .leading)
                        .foregroundStyle(editor.selectedVoice == voice ? Theme.accent : Theme.ink)
                        .accessibilityLabel("Select \(voice.title) pattern lane").accessibilityAddTraits(
                            editor.selectedVoice == voice ? .isSelected : [])
                    Button {
                        editor.selectedVoice = voice
                        editor.setLane { $0.muted.toggle() }
                    } label: {
                        Text("M").font(.system(size: 9, weight: .semibold)).frame(width: 22, height: 20)
                            .background(
                                editor.pattern.lanes[voice.rawValue]?.muted == true ? Theme.warning : Theme.raised,
                                in: RoundedRectangle(cornerRadius: 4)
                            )
                            .foregroundStyle(
                                editor.pattern.lanes[voice.rawValue]?.muted == true ? Theme.background : Theme.secondary
                            )
                    }
                    .buttonStyle(.plain).help("Mute \(voice.title) in this pattern").accessibilityLabel(
                        "Mute \(voice.title) pattern lane"
                    )
                    .accessibilityValue(editor.pattern.lanes[voice.rawValue]?.muted == true ? "On" : "Off")
                    .disabled(!controller.canEdit)
                }.padding(.horizontal, 14).frame(height: InlineDrumLayout.row).padding(.bottom, InlineDrumLayout.gap)
            }
            Button {
                controller.openDesigner()
            } label: {
                Label("Full designer…", systemImage: "arrow.up.left.and.arrow.down.right")
            }
            .buttonStyle(.plain).frame(height: InlineDrumLayout.footer).disabled(controller.currentClip == nil)
            Color.clear.frame(height: 8)
        }.font(.system(size: 11)).background(Theme.paper)
            .overlay(alignment: .trailing) { Theme.edge.frame(width: 1) }
            .overlay(alignment: .bottom) { Theme.line.frame(height: 1) }
    }
}

struct InlineDrumView: View {
    @ObservedObject var store: SessionStore
    @ObservedObject var controller: InlineDrumController
    @ObservedObject var editor: DrumEditorModel
    @StateObject private var kits = DrumKitLibrary(owner: nil)
    private var pattern: DrumPattern { editor.pattern }
    var body: some View {
        VStack(spacing: 0) {
            controls.frame(height: InlineDrumLayout.controls)
            InlineDrumGrid(
                editor: editor, position: store.playbackPosition, pattern: pattern,
                clip: controller.currentClip, playing: store.isPlaying
            )
            .disabled(!controller.canEdit).frame(height: InlineDrumLayout.ruler + InlineDrumLayout.grid)
            footer.frame(height: InlineDrumLayout.footer)
            Color.clear.frame(height: 8)
        }.font(.system(size: 11)).background(Theme.well)
            .overlay(alignment: .bottom) { Theme.line.frame(height: 1) }
            .onAppear {
                kits.owner = store
                kits.refresh()
            }
            .onChange(of: pattern.stepCount) { _, count in editor.selectedStep = min(editor.selectedStep, count - 1) }
    }
    private var controls: some View {
        VStack(spacing: 6) {
            ScrollView(.horizontal) {
                HStack(spacing: 12) {
                    Menu {
                        ForEach(DrumKit.allCases) { kit in
                            Button(kit.title) {
                                editor.change {
                                    $0.kit = kit
                                    $0.lanes = [:]
                                }
                            }
                        }
                        if !kits.kits.isEmpty {
                            Section("Your kits") {
                                ForEach(kits.kits) { kit in Button(kit.name) { editor.change { $0.use(kit) } } }
                            }
                        }
                    } label: {
                        Text(
                            pattern.customKitID.flatMap { id in kits.kits.first { $0.id == id }?.name }
                                ?? pattern.kit.title
                        ).lineLimit(1)
                    }
                    .frame(width: 145).help("Drum kit")
                    Menu("Groove") {
                        ForEach(DrumPatternPreset.Category.allCases) { category in
                            Section(category.rawValue) {
                                ForEach(DrumPatternPreset.allCases.filter { $0.category == category }) { groove in
                                    Button(groove.rawValue) { editor.change { $0.apply(groove) } }
                                }
                            }
                        }
                    }.fixedSize()
                    Picker(
                        "Length",
                        selection: Binding(
                            get: { pattern.bars },
                            set: { bars in
                                editor.change { $0.resizeBars(bars) }
                            })
                    ) { ForEach([1, 2, 4, 8], id: \.self) { Text("\($0) \($0 == 1 ? "bar" : "bars")").tag($0) } }
                    .labelsHidden().frame(width: 80).help(
                        "Pattern length. Extra bars repeat the existing groove, ready to edit.")
                    Picker("Grid", selection: Binding(get: { pattern.stepsPerBeat }, set: changeGrid)) {
                        Text("1/8").tag(2)
                        Text("1/16").tag(4)
                        Text("1/32").tag(8)
                    }.labelsHidden().frame(width: 72).help("Step grid")
                    Text("BPM").foregroundStyle(Theme.secondary)
                    CommittedNumberField("Beat tempo", value: parameter(\.bpm), range: 30...300)
                        .textFieldStyle(.roundedBorder).frame(width: 50)
                }.padding(.horizontal, 12).frame(height: 28)
            }.scrollIndicators(.hidden)
            ScrollView(.horizontal) {
                HStack(spacing: 14) {
                    knob("Swing", value: parameter(\.swing), range: 0...0.7, display: "\(Int(pattern.swing * 100))%")
                    knob(
                        "Humanize", value: parameter(\.humanize), range: 0...0.025,
                        display: "\(Int(pattern.humanize * 1000)) ms")
                    knob(
                        "Variation",
                        value: Binding(
                            get: { pattern.velocityHumanize ?? 0 },
                            set: { value in editor.change { $0.velocityHumanize = value } }), range: 0...0.2,
                        display: "\(Int((pattern.velocityHumanize ?? 0) * 100))%")
                    knob(
                        "Level", value: parameter(\.masterGain), range: 0...1,
                        display: "\(Int(pattern.masterGain * 100))%")
                }.padding(.horizontal, 12).frame(height: 28)
            }.scrollIndicators(.hidden)
        }.disabled(!controller.canEdit)
    }
    private func knob(_ title: String, value: Binding<Double>, range: ClosedRange<Double>, display: String) -> some View
    {
        HStack(spacing: 6) {
            Text("\(title) \(display)").foregroundStyle(Theme.secondary)
            RoomSlider(
                value: value, in: range, label: "Beat \(title)",
                onEditingChanged: { $0 ? editor.beginGesture() : editor.endGesture() }
            ).frame(width: 64)
        }.fixedSize()
    }
    private var footer: some View {
        HStack(spacing: 8) {
            if let failure = controller.failure {
                Image(systemName: "exclamationmark.triangle").foregroundStyle(Color.orange)
                Text(failure).lineLimit(1).help(failure)
                Spacer(minLength: 0)
                Button("Retry") { controller.retry() }.disabled(!controller.canRetry)
                Menu("Draft") {
                    Button("Copy draft") { copyDraft() }
                    Button(
                        controller.currentClip?.editableDrumPattern == nil
                            ? "Discard draft & close" : "Discard draft & reload"
                    ) { controller.discardDraft() }.disabled(controller.sending)
                }.fixedSize()
            } else {
                Text("\(editor.selectedVoice.title) · \(editor.selectedStep + 1)").lineLimit(1)
                Text("Velocity").foregroundStyle(Theme.secondary)
                RoomSlider(
                    value: Binding(
                        get: { editor.currentStep?.velocity ?? 0.8 },
                        set: { value in editor.setStep { $0.velocity = value } }), in: 0...1,
                    label: "Selected drum step velocity",
                    onEditingChanged: { $0 ? editor.beginGesture() : editor.endGesture() }
                )
                .frame(width: 70).disabled(editor.currentStep == nil || !controller.canEdit)
                Text("\(Int((editor.currentStep?.velocity ?? 0) * 127))").monospacedDigit().frame(width: 23)
                Spacer(minLength: 4)
                if controller.unsettled { ProgressView().controlSize(.mini) }
                Image(systemName: controller.unsettled ? "arrow.triangle.2.circlepath" : "checkmark.circle")
                    .foregroundStyle(Theme.secondary).help(controller.status).accessibilityLabel(controller.status)
            }
        }.padding(.horizontal, 12)
    }
    private func parameter(_ key: WritableKeyPath<DrumPattern, Double>) -> Binding<Double> {
        Binding(get: { pattern[keyPath: key] }, set: { value in editor.change { $0[keyPath: key] = value } })
    }
    private func copyDraft() {
        do {
            let data = try editor.copyPattern()
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setData(data, forType: .init("org.soundroom.drum-pattern"))
            store.status = "Beat draft copied. Paste it in the full drum designer."
        } catch { store.report(error) }
    }
    private func changeGrid(_ divisions: Int) {
        editor.change { pattern in
            let old = pattern.stepsPerBeat
            pattern.stepsPerBeat = divisions
            var used: Set<String> = []
            pattern.steps = pattern.steps.compactMap { step in
                var step = step
                step.index = Int((Double(step.index) * Double(divisions) / Double(old)).rounded())
                guard step.index < pattern.stepCount, used.insert("\(step.voice.rawValue)-\(step.index)").inserted
                else { return nil }
                return step
            }
        }
        editor.selectedStep = min(editor.selectedStep, pattern.stepCount - 1)
    }
}

private struct InlineDrumGrid: View {
    @ObservedObject var editor: DrumEditorModel
    @ObservedObject var position: PlaybackPosition
    let pattern: DrumPattern
    let clip: Clip?
    let playing: Bool
    @State private var scrollOffset = 0.0
    private var playStep: Int {
        guard playing, let clip, position.frame >= clip.start, position.frame < clip.end else { return -1 }
        let phase = (Double(position.frame - clip.start + clip.offset) / sessionRate).truncatingRemainder(
            dividingBy: pattern.duration)
        return (0..<pattern.stepCount).last { pattern.timeForStep($0) <= phase } ?? -1
    }
    var body: some View {
        GeometryReader { geometry in
            let count = pattern.stepCount
            let perBar = pattern.stepsPerBeat * 4
            let width = max(18, min(30, (geometry.size.width - 24 - Double(count - 1) * 4) / Double(count)))
            let barWidth = Double(perBar) * (width + 4)
            let contentWidth = Double(count) * (width + 4) + 20
            let indices = Array(0..<count)
            ScrollViewReader { proxy in
                ScrollView(.horizontal) {
                    VStack(spacing: 0) {
                        HStack(spacing: 4) {
                            ForEach(0..<pattern.bars, id: \.self) { bar in
                                Text("Bar \(bar + 1)").font(.system(size: 9, weight: .semibold)).foregroundStyle(
                                    Theme.accent
                                )
                                .frame(width: barWidth - 4, height: 18, alignment: .leading).id(bar)
                            }
                        }
                        HStack(spacing: 4) {
                            ForEach(indices, id: \.self) { index in
                                Text(
                                    index % pattern.stepsPerBeat == 0
                                        ? "\((index % perBar) / pattern.stepsPerBeat + 1)" : "·"
                                )
                                .foregroundStyle(Theme.secondary).frame(
                                    width: width, height: InlineDrumLayout.ruler - 18)
                            }
                        }
                        ForEach(DrumVoice.allCases) { voice in
                            HStack(spacing: 4) {
                                ForEach(indices, id: \.self) { index in
                                    InlineDrumStep(
                                        editor: editor, voice: voice, index: index, active: playStep == index
                                    )
                                    .frame(width: width, height: InlineDrumLayout.row)
                                }
                            }.padding(.bottom, InlineDrumLayout.gap)
                        }
                    }.padding(.horizontal, 12)
                        .background {
                            GeometryReader { content in
                                Color.clear.preference(
                                    key: InlineDrumScrollOffset.self,
                                    value: max(0, geometry.frame(in: .global).minX - content.frame(in: .global).minX))
                            }
                        }
                }.scrollIndicators(.visible)
                    .onPreferenceChange(InlineDrumScrollOffset.self) { scrollOffset = $0 }
                    .overlay(alignment: .topTrailing) {
                        if contentWidth > geometry.size.width {
                            HStack(spacing: 8) {
                                Button {
                                    reveal(
                                        max(0, Int(max(0, scrollOffset - 12) / barWidth) - 1), barWidth: barWidth,
                                        maximum: contentWidth - geometry.size.width, proxy: proxy)
                                } label: {
                                    Image(systemName: "chevron.left")
                                }
                                .disabled(scrollOffset <= 12).accessibilityLabel("Scroll to earlier pattern bars")
                                Text("Scroll pattern")
                                Button {
                                    reveal(
                                        min(pattern.bars - 1, Int(max(0, scrollOffset - 12) / barWidth) + 1),
                                        barWidth: barWidth, maximum: contentWidth - geometry.size.width, proxy: proxy)
                                } label: {
                                    Image(systemName: "chevron.right")
                                }
                                .disabled(scrollOffset >= contentWidth - geometry.size.width - 1).accessibilityLabel(
                                    "Scroll to later pattern bars")
                            }.buttonStyle(.plain).font(.system(size: 10)).padding(.horizontal, 8)
                                .frame(height: 18).background(Theme.well)
                        }
                    }
            }
        }
    }
    private func reveal(_ bar: Int, barWidth: Double, maximum: Double, proxy: ScrollViewProxy) {
        // Keep button navigation responsive while native scroll geometry settles.
        scrollOffset = min(maximum, Double(bar) * barWidth + 12)
        proxy.scrollTo(bar, anchor: .leading)
    }
}

private struct InlineDrumScrollOffset: PreferenceKey {
    static let defaultValue = 0.0
    static func reduce(value: inout Double, nextValue: () -> Double) { value = max(value, nextValue()) }
}

private struct InlineDrumStep: View {
    @ObservedObject var editor: DrumEditorModel
    let voice: DrumVoice
    let index: Int
    let active: Bool
    @State private var dragVelocity: Double?
    private var step: DrumStep? { editor.pattern.steps.first { $0.voice == voice && $0.index == index } }
    private var selected: Bool { editor.selectedVoice == voice && editor.selectedStep == index }
    var body: some View {
        Button {
            if NSEvent.modifierFlags.contains(.option) {
                editor.selectedVoice = voice
                editor.selectedStep = index
            } else {
                editor.toggle(voice: voice, index: index)
            }
        } label: {
            RoundedRectangle(cornerRadius: 4)
                .fill(
                    step == nil
                        ? (index % editor.pattern.stepsPerBeat == 0 ? Theme.line : Theme.raised)
                        : Theme.accent.opacity(0.35 + (step?.velocity ?? 0) * 0.65)
                )
                .overlay(alignment: .bottom) {
                    if let step {
                        Rectangle().fill(Theme.background.opacity(0.6)).frame(height: max(2, step.velocity * 11))
                            .padding(3)
                    }
                }
                .overlay(
                    RoundedRectangle(cornerRadius: 4).stroke(
                        active ? Theme.playback : selected ? Theme.ink : Color.clear, lineWidth: active ? 2.5 : 1.5))
        }.buttonStyle(.plain)
            .help("\(voice.title), step \(index + 1). Click toggles; drag up/down for velocity; Option-click selects.")
            .accessibilityLabel("\(voice.title) step \(index + 1)")
            .accessibilityValue(step.map { "On, velocity \(Int($0.velocity * 127))" } ?? "Off")
            .highPriorityGesture(
                DragGesture(minimumDistance: 4).onChanged { value in
                    if dragVelocity == nil {
                        editor.beginGesture()
                        editor.selectedVoice = voice
                        editor.selectedStep = index
                        if step == nil { editor.toggle(voice: voice, index: index) }
                        dragVelocity = step?.velocity ?? 0.8
                    }
                    let velocity = min(1, max(0, (dragVelocity ?? 0.8) - value.translation.height / 70))
                    editor.setStep { $0.velocity = velocity }
                }.onEnded { _ in
                    dragVelocity = nil
                    editor.endGesture()
                })
    }
}
