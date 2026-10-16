import SwiftUI
import SoundroomCore

struct MasteringAssistantControls: View {
    @ObservedObject var store: SessionStore
    @State private var review = false
    @State private var analyzeOnOpen = false
    var body: some View {
        HStack(spacing: 8) {
            Button { analyzeOnOpen = true; review = true } label: { Label("Analyze mix", systemImage: "waveform.path.ecg") }
                .buttonStyle(RoomButtonStyle()).disabled(store.session.end == 0 || store.busy || store.isRecording)
            if store.session.masteringAnalysis != nil {
                Button("Review") { analyzeOnOpen = false; review = true }.buttonStyle(RoomButtonStyle())
            }
            Spacer(minLength: 0)
        }.font(.system(size: 11)).sheet(isPresented: $review) { MasteringReviewSheet(store: store, analyzeOnOpen: analyzeOnOpen) }
    }
}

struct MasteringReviewSheet: View {
    @ObservedObject var store: SessionStore
    @Environment(\.dismiss) private var dismiss
    @StateObject private var controller: MasteringReviewController
    let analyzeOnOpen: Bool
    init(store: SessionStore, analyzeOnOpen: Bool = false) {
        self.store = store; self.analyzeOnOpen = analyzeOnOpen
        _controller = StateObject(wrappedValue: MasteringReviewController(owner: store))
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack {
                Text("Master the mix").font(.system(size: 21, weight: .semibold)); Spacer()
                ProgressView().controlSize(.small).frame(width: 16, height: 16).opacity(controller.working || controller.verifying ? 1 : 0)
                Button("Done") { controller.close(); dismiss() }
            }
            if controller.working && controller.report == nil {
                VStack(alignment: .leading, spacing: 12) {
                    ProgressView(value: store.workProgress ?? 0).accessibilityLabel("Master analysis progress")
                    Text("Measuring loudness, true peaks, spectral peaks and broad tonal balance.").font(.system(size: 12)).foregroundStyle(Theme.secondary)
                    Button("Cancel analysis") { store.cancelBackgroundWork() }.buttonStyle(RoomButtonStyle())
                }.frame(maxWidth: .infinity, minHeight: 210, alignment: .center)
            } else if let report = controller.report {
                HStack(spacing: 20) {
                    metric("Integrated", String(format: "%.1f LUFS", report.integratedLUFS))
                    metric("True peak", String(format: "%.1f dBTP", report.truePeakDB))
                    metric("Crest", String(format: "%.1f dB", report.crestDB))
                }
                MasteringSectionChart(report: report)
                HStack {
                    Picker("Loudness aim", selection: $controller.target) { Text("Dynamic · −18 LUFS").tag(-18.0); Text("Balanced · −16 LUFS").tag(-16.0); Text("Forward · −14 LUFS").tag(-14.0) }.frame(width: 300)
                    Spacer()
                    Text("Amount").font(.system(size: 11))
                    RoomSlider(value: $controller.intensity, in: 0...1, label: "Mastering proposal amount").frame(width: 110)
                    Text(String(format: "%.0f%%", controller.intensity * 100)).monospacedDigit().frame(width: 36)
                }.disabled(controller.working)
                VStack(spacing: 10) {
                    ForEach(controller.proposed.indices, id: \.self) { i in
                        HStack(alignment: .top, spacing: 10) {
                            Toggle("", isOn: Binding(get: { controller.proposed[i].enabled }, set: { controller.setIncluded(i, $0) })).labelsHidden().accessibilityLabel("Include \(controller.proposed[i].kind.name)")
                            VStack(alignment: .leading, spacing: 4) {
                                Text(controller.proposed[i].kind.name).font(.system(size: 12, weight: .medium))
                                Text(reason(controller.proposed[i])).font(.system(size: 11)).foregroundStyle(Theme.secondary).fixedSize(horizontal: false, vertical: true)
                            }
                            Spacer()
                        }
                    }
                }.padding(14).background(Theme.background, in: RoundedRectangle(cornerRadius: 8)).disabled(controller.working)
                Text("A conservative starting point. Loudness aim is a reference, not a guaranteed result. Input and resonance automation preserve section dynamics; every point and setting stays editable.")
                    .font(.system(size: 11)).foregroundStyle(Theme.secondary).fixedSize(horizontal: false, vertical: true)
                HStack {
                    Button("Analyze again") { controller.analyze() }.buttonStyle(RoomButtonStyle()).disabled(controller.working || controller.verifying || store.busy)
                    Button("Reload review") { controller.reload() }.buttonStyle(RoomButtonStyle()).disabled(controller.working || controller.verifying)
                    Spacer()
                    Button("Use proposal") { controller.apply() }.buttonStyle(RoomButtonStyle(prominent: true)).disabled(!controller.ready).keyboardShortcut(.defaultAction)
                }
            } else { ContentUnavailableView("No analysis", systemImage: "waveform", description: Text("Analyze a session with audio to suggest a master rack.")) }
            Text(controller.message).font(.system(size: 11)).foregroundStyle(controller.sourceChanged ? Color.orange : Theme.secondary)
                .frame(maxWidth: .infinity, minHeight: 30, alignment: .leading).lineLimit(2)
        }.padding(24).frame(width: 690).background(Theme.paper).foregroundStyle(Theme.ink).tint(Theme.accent).preferredColorScheme(.dark).disabled(false)
            .onAppear { if analyzeOnOpen || store.session.masteringAnalysis == nil { controller.analyze() } else { controller.reload() } }
            .onChange(of: store.session.masteringAnalysis) { _, _ in controller.synchronize() }
            .onChange(of: store.revision) { _, _ in controller.synchronize() }
            .onChange(of: store.remoteGUI?.revision) { _, _ in controller.synchronize() }
            .onChange(of: store.remoteGUI?.connectionGeneration) { _, _ in controller.synchronize() }
            .onChange(of: controller.completedApplyID) { _, value in if value != nil { dismiss() } }
            .onDisappear { controller.close() }
    }
    private func metric(_ title: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 5) { Text(title).font(.system(size: 10)).foregroundStyle(Theme.secondary); Text(value).font(.system(size: 17, weight: .medium, design: .monospaced)) }.frame(maxWidth: .infinity, alignment: .leading)
    }
    private func reason(_ effect: RealtimeEffect) -> String {
        switch effect.kind {
        case .equalizer: return effect.enabled ? String(format: "Broad balance: low shelf %+.1f dB, high shelf %+.1f dB. Small cuts only.", effect.parameters["lowGain"] ?? 0, effect.parameters["highGain"] ?? 0) : "Broad tonal balance needs no suggested shelf correction."
        case .resonance: return effect.enabled ? "Local spectral peaks detected. Bounded reduction follows an editable section curve." : "No strong spectral peaks detected; available for manual use."
        case .warmth: return "Optional Tape color at a low blend, with level matching. Enable if it suits the song."
        case .limiter: return "−1 dBTP ceiling; measured peaks bound input-gain automation to avoid excessive limiting."
        default: return ""
        }
    }
}

private struct MasteringSectionChart: View {
    let report: MasteringAnalysis
    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text("Measured section loudness · 3-second windows").font(.system(size: 10)).foregroundStyle(Theme.secondary)
            Canvas { context, size in
                for level in [-36.0, -24, -12] {
                    let y = (0 - level) / 48 * size.height
                    var line = Path(); line.move(to: CGPoint(x: 0, y: y)); line.addLine(to: CGPoint(x: size.width, y: y))
                    context.stroke(line, with: .color(Theme.line), lineWidth: 0.5)
                }
                var curve = Path()
                for (index, section) in report.sections.enumerated() {
                    let p = CGPoint(x: CGFloat(section.start + section.end) / CGFloat(max(1, report.analyzedFrames) * 2) * size.width,
                        y: CGFloat(min(48, max(0, -section.loudness))) / 48 * size.height)
                    if index == 0 { curve.move(to: p) } else { curve.addLine(to: p) }
                }
                context.stroke(curve, with: .color(Theme.accent), lineWidth: 2)
            }.frame(height: 85).background(Theme.background, in: RoundedRectangle(cornerRadius: 5))
            HStack { Text("0:00"); Spacer(); Text(timeString(report.analyzedFrames)) }.font(.system(size: 9)).foregroundStyle(Theme.secondary)
        }.accessibilityElement(children: .ignore).accessibilityLabel("Measured loudness across \(report.sections.count) sections, over \(timeString(report.analyzedFrames)).")
    }
}

struct MasterOutputControls: View {
    @ObservedObject var store: SessionStore
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Button {
                store.choose(track: Session.masterTrackID)
                store.showEffects = true; store.selectedRealtimeEffect = store.session.masterTrack.realtimeEffects.last?.id; store.focusEditor()
            } label: {
                HStack { Image(systemName: "waveform.path").foregroundStyle(Theme.accent); Text("Master").font(.system(size: 13, weight: .semibold)); Spacer(); Image(systemName: "slider.horizontal.3").foregroundStyle(Theme.secondary) }.contentShape(Rectangle())
            }.buttonStyle(.plain).accessibilityLabel("Select Master output track").accessibilityAddTraits(store.selectedTrack == Session.masterTrackID ? .isSelected : [])
            HStack {
                TrackAutomationToggle(store: store, trackID: Session.masterTrackID)
                Text("Trim").font(.system(size: 10)).foregroundStyle(Theme.secondary)
                TrackMixControl(store: store, track: store.session.masterTrack, parameter: .volume)
            }
            HStack { Text("\(store.session.masterTrack.realtimeEffects.count) inserts"); Spacer(); Text("Final mix") }.font(.system(size: 10)).foregroundStyle(Theme.secondary)
        }.padding(.horizontal, 14).frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(store.selectedTrack == Session.masterTrackID ? Theme.accent.opacity(0.13) : Theme.paper)
            .overlay(alignment: .leading) { if store.selectedTrack == Session.masterTrackID { Theme.accent.frame(width: 3) } }
            .overlay(alignment: .bottom) { Theme.line.frame(height: 1) }.overlay(alignment: .trailing) { Theme.edge.frame(width: 1) }
            .disabled(store.busy || store.isRecording)
    }
}
struct MasterOutputLane: View {
    @ObservedObject var store: SessionStore
    var body: some View {
        Button { store.choose(track: Session.masterTrackID); store.showEffects = true } label: {
        HStack(spacing: 12) {
            Image(systemName: "arrow.turn.down.right").foregroundStyle(Theme.accent)
            Text("Mix → Master → Output").font(.system(size: 11)).foregroundStyle(Theme.secondary)
            Spacer()
        }.padding(.horizontal, 18).frame(maxWidth: .infinity, maxHeight: .infinity).contentShape(Rectangle())
        }.buttonStyle(.plain).background(Theme.paper.opacity(0.4))
            .overlay(alignment: .bottom) { Theme.line.frame(height: 1) }
            .accessibilityLabel("Select Master output track").accessibilityAddTraits(store.selectedTrack == Session.masterTrackID ? .isSelected : [])
            .disabled(store.busy || store.isRecording)
    }
}
