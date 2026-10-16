import SwiftUI
import SoundroomCore

struct TimelineView: View {
    @ObservedObject var store: SessionStore
    @State private var viewport = TimelineViewport()
    @StateObject private var interaction = TimelineScrollInteraction()
    @State private var scrollDestination: Double = 0
    @State private var pendingScroll: Double?
    @State private var pendingScrollAt: TimeInterval = 0
    var body: some View {
        VStack(spacing: 0) {
        TimelineNavigationBar(store: store)
        TimelineOverview(store: store, viewport: currentViewport)
        GeometryReader { geometry in
            ScrollViewReader { verticalProxy in
            ScrollView(.vertical) {
                HStack(alignment: .top, spacing: 0) {
                    trackColumn
                    ScrollViewReader { proxy in
                        ScrollView(.horizontal) {
                            lanes(viewportWidth: max(100, Double(geometry.size.width) - 210))
                                .overlay(alignment: .topLeading) { revealAnchor }
                                .background {
                                    GeometryReader { content in
                                        Color.clear.preference(key: TimelineViewportPreference.self,
                                            value: TimelineViewportMeasurement(origin: content.frame(in: .named("timelineViewport")).minX,
                                                contentWidth: content.size.width))
                                    }
                                }
                        }
                        .coordinateSpace(name: "timelineViewport")
                        .background { TimelineViewportInput(interaction: interaction).allowsHitTesting(false) }
                        .onAppear {
                            interaction.requestScroll = { destination in requestScroll(destination, proxy: proxy) }
                            interaction.zoom = { factor, pointerX in zoom(by: factor, pointerX: pointerX, proxy: proxy) }
                            interaction.manualScroll = {
                                pendingScroll = nil
                                if store.followPlayhead { store.followPlayhead = false }
                            }
                        }
                        .onDisappear { interaction.tearDown() }
                        .background {
                            GeometryReader { visible in
                                Color.clear.preference(key: TimelineViewportPreference.self,
                                    value: TimelineViewportMeasurement(viewportWidth: visible.size.width))
                            }
                        }
                        .onPreferenceChange(TimelineViewportPreference.self) { measurement in
                            guard let origin = measurement.origin, let width = measurement.viewportWidth,
                                  let contentWidth = measurement.contentWidth, width > 0 else { return }
                            let measured = TimelineViewport(offset: max(0, -origin), width: width, contentWidth: contentWidth)
                            if store.isPlaying, store.followPlayhead, measured.isUnrequestedScroll(from: viewport, pendingDestination: pendingScroll) {
                                store.followPlayhead = false
                            }
                            if viewport != measured { viewport = measured }
                            interaction.viewport = measured
                            if abs(store.visibleTimelineWidth - width) > 0.5 { store.visibleTimelineWidth = width }
                            if let pendingScroll, abs(measured.offset - pendingScroll) < 2 { self.pendingScroll = nil }
                        }
                        .onChange(of: store.revealRequestSerial) { _, _ in reveal(proxy: proxy) }
                        .onChange(of: store.timelineZoomSerial) { _, _ in zoom(by: store.timelineZoomFactor, pointerX: nil, proxy: proxy) }
                        .onChange(of: store.timelineScrollSerial) { _, _ in
                            requestScroll(min(max(0, currentViewport.contentWidth - currentViewport.width), store.timelineScrollOffset), proxy: proxy)
                        }
                        .onReceive(store.playbackPosition.$frame) { frame in follow(proxy: proxy, frame: frame) }
                        .onChange(of: store.followPlayhead) { _, _ in follow(proxy: proxy) }
                        .onChange(of: store.isPlaying) { _, playing in
                            // A transport stop can coincide with an explicit
                            // reveal (Return to Start); let that scroll finish.
                            if playing { follow(proxy: proxy) }
                        }
                    }
                }.frame(minHeight: geometry.size.height, alignment: .top)
            }
            .onChange(of: store.inlineDrumExpanded ? store.inlineDrums?.id : nil) { _, target in
                guard let target else { return }
                DispatchQueue.main.async { verticalProxy.scrollTo(target, anchor: .center) }
            }
            .onChange(of: store.inlineSoundExpanded ? store.inlineSound?.id : nil) { _, target in
                guard let target else { return }
                DispatchQueue.main.async { verticalProxy.scrollTo(target, anchor: .center) }
            }
            }
        }.background(Theme.background)
        }
        .overlay(alignment: .bottomLeading) {
            if let preview = store.clipMovePreview,
               let clip = store.session.tracks.flatMap(\.clips).first(where: { preview.ids.contains($0.id) }) {
                Text(String(format: "Start %.4f s · Move %+.4f s%@", Double(clip.start + preview.shift) / sessionRate, Double(preview.shift) / sessionRate, preview.guide == nil ? "" : " · Snapped"))
                    .font(.system(size: 11, design: .monospaced)).padding(8).background(Theme.paper, in: RoundedRectangle(cornerRadius: 5)).padding(10).allowsHitTesting(false)
            }
        }
    }
    private var revealAnchor: some View {
        HStack(spacing: 0) {
            Color.clear.frame(width: scrollDestination, height: 1)
            Color.clear.frame(width: 1, height: 1).id("timelineReveal")
        }.allowsHitTesting(false)
    }
    private var currentViewport: TimelineViewport {
        let width = viewport.width > 0 ? viewport.width : store.visibleTimelineWidth
        return TimelineViewport(offset: viewport.offset, width: width,
            contentWidth: max(width, store.timelineSeconds * store.zoom))
    }
    private func follow(proxy: ScrollViewProxy, frame: Int? = nil) {
        if pendingScroll != nil {
            // A missed/interrupted scroll must not disable Follow for the rest
            // of playback. Retry against measured geometry after a short grace.
            guard ProcessInfo.processInfo.systemUptime - pendingScrollAt >= 0.25 else { return }
            pendingScroll = nil
        }
        let loop = store.loopSelection ? (store.isPreviewing ? store.selectedRange : store.automationPlaybackLoopRange) : nil
        if let destination = currentViewport.followDestination(frame: frame ?? store.playhead, zoom: store.zoom,
            playing: store.isPlaying, enabled: store.followPlayhead, loop: loop) {
            requestScroll(destination, proxy: proxy)
        }
    }
    /// Zooms keeping one time under a fixed viewport x: the pointer for pinch or
    /// ⌘-scroll, else the playhead when visible, else the viewport center.
    private func zoom(by factor: Double, pointerX: Double?, proxy: ScrollViewProxy) {
        let old = store.zoom, new = SessionStore.clampedZoom(old * factor)
        guard factor.isFinite, abs(new - old) > 0.0001 else { return }
        let view = currentViewport
        let x: Double
        if let pointerX, pointerX.isFinite { x = max(0, min(view.width, pointerX)) }
        else {
            let frame = store.isPlaying ? store.playbackPosition.frame : store.playhead
            let playheadX = Double(frame) / sessionRate * old - view.offset
            x = playheadX >= 0 && playheadX <= view.width ? playheadX : view.width / 2
        }
        let seconds = (view.offset + x) / old
        store.zoom = new
        let maximum = max(0, max(view.width, store.timelineSeconds * new) - view.width)
        requestScroll(max(0, min(maximum, seconds * new - x)), proxy: proxy)
    }
    private func reveal(proxy: ScrollViewProxy) {
        guard let frame = store.revealFrame else { return }
        if store.revealOnlyIfHidden {
            let x = Double(frame) / sessionRate * store.zoom - currentViewport.offset
            if x >= 0, x <= currentViewport.width - 8 { return }
        }
        requestScroll(currentViewport.revealDestination(frame: frame, zoom: store.zoom), proxy: proxy)
    }
    private func requestScroll(_ destination: Double, proxy: ScrollViewProxy) {
        scrollDestination = destination
        pendingScroll = destination
        pendingScrollAt = ProcessInfo.processInfo.systemUptime
        // Let the new anchor position reach layout before resolving scrollTo.
        DispatchQueue.main.async {
            guard pendingScroll == destination else { return }
            proxy.scrollTo("timelineReveal", anchor: .leading)
            if pendingScroll == destination, abs(viewport.offset - destination) < 2 { pendingScroll = nil }
        }
    }
    private var playheadLaneHeight: Double {
        let audioHeight = Double(store.session.tracks.count + 1) * store.trackSize.height
        let count = store.session.tracks.filter { store.showsAutomation($0) }.count
        let master = store.showsAutomation(store.session.masterTrack) ? 1 : 0
        return audioHeight + Double((count + master) * 84) + store.session.tracks.reduce(0) { $0 + store.inlineDrumHeight(for: $1) + store.inlineSoundHeight(for: $1) }
    }
    private var trackColumn: some View {
        VStack(spacing: 0) {
            HStack {
                Text("TRACKS").font(.system(size: 9, weight: .semibold)).tracking(1.4)
                Spacer(); Text("\(store.session.tracks.count)").font(.system(size: 10))
            }.foregroundStyle(Theme.secondary).padding(.horizontal, 18).frame(height: 34)
            ForEach(store.session.tracks) { track in
                TrackControls(store: store, track: track).frame(height: store.trackSize.height)
                if store.showsAutomation(track) { AutomationLaneHeader(store: store).frame(height: 84) }
                if store.inlineDrumHeight(for: track) > 0, let drums = store.inlineDrums {
                    InlineDrumHeader(store: store, controller: drums, editor: drums.editor).frame(height: InlineDrumLayout.height).id(drums.id)
                }
                if store.inlineSoundHeight(for: track) > 0, let sound = store.inlineSound {
                    InlineSoundHeader(store: store, controller: sound).frame(height: InlineSoundLayout.height).id(sound.id)
                }
            }
            MasterOutputControls(store: store).frame(height: store.trackSize.height)
            if store.showsAutomation(store.session.masterTrack) { AutomationLaneHeader(store: store).frame(height: 84) }
            Button { store.addTrack() } label: { Label("Add audio track", systemImage: "plus").font(.system(size: 12)).frame(maxWidth: .infinity).frame(height: 58) }
                .buttonStyle(.plain).foregroundStyle(Theme.secondary).disabled(store.busy || store.isRecording)
        }.frame(width: 210).background(Theme.paper)
    }
    private func lanes(viewportWidth: Double) -> some View {
        VStack(spacing: 0) {
            RulerView(store: store, visiblePixels: max(0, viewport.offset - 80)..<(viewport.offset + viewportWidth + 80)).frame(height: 34)
            ForEach(store.session.tracks) { track in
                LaneView(store: store, track: track, visiblePixels: max(0, viewport.offset - 80)..<(viewport.offset + viewportWidth + 80), viewportPixels: viewport.offset..<(viewport.offset + viewportWidth)).frame(height: store.trackSize.height)
                if store.showsAutomation(track) { AutomationLaneView(store: store).frame(height: 84) }
                if store.inlineDrumHeight(for: track) > 0, let drums = store.inlineDrums {
                    ZStack(alignment: .leading) {
                        Theme.well
                        InlineDrumView(store: store, controller: drums, editor: drums.editor)
                            .frame(width: viewportWidth).offset(x: viewport.offset)
                    }.frame(height: InlineDrumLayout.height)
                }
                if store.inlineSoundHeight(for: track) > 0, let sound = store.inlineSound {
                    ZStack(alignment: .leading) {
                        Theme.well
                        InlineSoundView(store: store, controller: sound).frame(width: viewportWidth).offset(x: viewport.offset)
                    }.frame(height: InlineSoundLayout.height)
                }
            }
            MasterOutputLane(store: store).frame(height: store.trackSize.height)
            if store.showsAutomation(store.session.masterTrack) { AutomationLaneView(store: store).frame(height: 84) }
            if store.session.end == 0 { emptyState(width: viewportWidth) }
            else { Color.clear.frame(height: 58) }
            Spacer(minLength: 0)
        }.frame(width: max(viewportWidth, store.timelineSeconds * store.zoom), alignment: .leading)
            .environment(\.timelineScrollInteraction, interaction)
            .overlay(alignment: .topLeading) {
                TimelinePlayhead(position: store.playbackPosition, zoom: store.zoom, recording: store.isRecording,
                    laneHeight: playheadLaneHeight)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .mask(alignment: .topLeading) { playheadMask }
            }
    }
    private var playheadMask: some View {
        VStack(spacing: 0) {
            Color.white.frame(height: 34)
            ForEach(store.session.tracks) { track in
                Color.white.frame(height: store.trackSize.height)
                if store.showsAutomation(track) { Color.white.frame(height: 84) }
                if store.inlineDrumHeight(for: track) > 0 { Color.clear.frame(height: InlineDrumLayout.height) }
                if store.inlineSoundHeight(for: track) > 0 { Color.clear.frame(height: InlineSoundLayout.height) }
            }
            Color.white.frame(height: store.trackSize.height)
            if store.showsAutomation(store.session.masterTrack) { Color.white.frame(height: 84) }
        }
    }
    private func emptyState(width: Double) -> some View {
        VStack(spacing: 12) {
            Image(systemName: "waveform.path").font(.system(size: 37, weight: .ultraLight)).foregroundStyle(Theme.accent)
            Text("Record or import audio").font(.system(size: 20, weight: .medium))
            Text("R to record · Drop files to import").font(.system(size: 12)).foregroundStyle(Theme.secondary)
            ActionButton(label: "Import audio", icon: "arrow.down.doc") { store.importAudio() }.padding(.top, 3)
        }.frame(width: max(350, width), height: 220).frame(maxWidth: .infinity, alignment: .leading).disabled(store.busy)
    }
}

private struct TimelineViewportMeasurement: Equatable {
    var origin: Double?
    var contentWidth: Double?
    var viewportWidth: Double?
}
private struct TimelineViewportPreference: PreferenceKey {
    static let defaultValue = TimelineViewportMeasurement()
    static func reduce(value: inout TimelineViewportMeasurement, nextValue: () -> TimelineViewportMeasurement) {
        let next = nextValue()
        if let origin = next.origin { value.origin = origin }
        if let width = next.contentWidth { value.contentWidth = width }
        if let width = next.viewportWidth { value.viewportWidth = width }
    }
}
