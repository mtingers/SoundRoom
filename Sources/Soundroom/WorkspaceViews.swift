import SoundroomCore
import SwiftUI

struct EditingScopeView: View {
    let scope: EditingScope
    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "scope").foregroundStyle(Theme.accent)
            Text(scope.trackName).fontWeight(.semibold).lineLimit(1)
            Text("·").foregroundStyle(Theme.secondary)
            Text(scope.detail).lineLimit(1).foregroundStyle(Theme.secondary)
            if let time = scope.timeLabel { Text(time).monospacedDigit().foregroundStyle(Theme.accent).lineLimit(1) }
        }.font(.system(size: 12)).padding(.horizontal, 12).padding(.vertical, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.accent.opacity(0.065), in: RoundedRectangle(cornerRadius: 6))
            .accessibilityElement(children: .combine)
    }
}

struct TimelineNavigationBar: View {
    @ObservedObject var store: SessionStore
    var body: some View {
        HStack(spacing: 12) {
            Button("Fit Session") {
                store.fitSession()
                store.focusEditor()
            }.help("Fit the whole session (⌘0)")
            Button("Fit Selection") {
                store.zoomToSelection()
                store.focusEditor()
            }.disabled(store.selectedRange == nil).help("Fit selected audio (⌘J)")
            Button {
                store.revealPlayhead()
                store.focusEditor()
            } label: {
                Label("Playhead", systemImage: "location")
            }.help("Reveal playhead (F)")
            Toggle("Follow", isOn: $store.followPlayhead).toggleStyle(.checkbox).help(
                "Keep playback visible. Manual scrolling turns Follow off.")
            Spacer(minLength: 8)
            Menu {
                Picker("Track height", selection: $store.trackSize) {
                    ForEach(TrackSize.allCases) { Text($0.rawValue).tag($0) }
                }
            } label: {
                Label("Track height", systemImage: "arrow.up.and.down")
            }.menuStyle(.borderlessButton).fixedSize().help("Adjust track height")
        }.font(.system(size: 12)).buttonStyle(.plain).padding(.horizontal, 16).frame(height: 34).background(Theme.paper)
    }
}

struct TimelineOverview: View {
    @ObservedObject var store: SessionStore
    let viewport: TimelineViewport
    @State private var grabOffset: Double?
    var body: some View {
        HStack(spacing: 0) {
            HStack {
                Image(systemName: "rectangle.inset.filled")
                Text("Session overview")
            }
            .font(.system(size: 11, weight: .medium)).foregroundStyle(Theme.secondary).frame(width: 210)
            GeometryReader { geometry in
                let model = TimelineOverviewGeometry(
                    totalSeconds: store.timelineSeconds, viewport: viewport, zoom: store.zoom,
                    width: geometry.size.width)
                ZStack(alignment: .topLeading) {
                    Canvas { context, size in
                        let total = max(1, store.timelineSeconds * sessionRate)
                        let count = max(1, store.session.tracks.count)
                        let row = min(5, size.height / Double(count))
                        for (index, track) in store.session.tracks.enumerated() {
                            for clip in track.clips {
                                let rect = CGRect(
                                    x: Double(clip.start) / total * size.width, y: Double(index) * row,
                                    width: max(1, Double(clip.length) / total * size.width), height: max(1, row - 1))
                                context.fill(
                                    Path(roundedRect: rect, cornerRadius: 1),
                                    with: .color(Theme.track(track.color).opacity(track.muted ? 0.25 : 0.65)))
                            }
                        }
                    }
                    RoundedRectangle(cornerRadius: 3).fill(Theme.ink.opacity(0.08))
                        .overlay(RoundedRectangle(cornerRadius: 3).stroke(Theme.accent, lineWidth: 1.5))
                        .frame(width: model.thumbWidth).offset(x: model.thumbX)
                    OverviewCursor(position: store.playbackPosition, seconds: store.timelineSeconds)
                }.background(Theme.well).contentShape(Rectangle())
                    .gesture(
                        DragGesture(minimumDistance: 0).onChanged { value in
                            if grabOffset == nil {
                                grabOffset =
                                    value.startLocation.x >= model.thumbX
                                        && value.startLocation.x <= model.thumbX + model.thumbWidth
                                    ? value.startLocation.x - model.thumbX : model.thumbWidth / 2
                            }
                            store.requestTimelineScroll(model.offset(at: value.location.x, grabOffset: grabOffset ?? 0))
                        }.onEnded { _ in
                            grabOffset = nil
                            store.focusEditor()
                        }
                    )
                    .accessibilityElement(children: .ignore).accessibilityLabel(
                        "Session overview, visible timeline region"
                    )
                    .accessibilityValue("\(Int(max(0, viewport.offset / max(1, store.zoom)))) seconds from start")
                    .accessibilityAdjustableAction { direction in
                        store.requestTimelineScroll(
                            viewport.offset + (direction == .increment ? 1 : -1) * viewport.width * 0.8)
                    }
                    .help("Click or drag to move the visible region without moving the playhead")
            }.padding(.vertical, 6).padding(.trailing, 12)
        }.frame(height: 36).background(Theme.paper)
    }
}

private struct OverviewCursor: View {
    @ObservedObject var position: PlaybackPosition
    let seconds: Double
    var body: some View {
        GeometryReader { geometry in
            Rectangle().fill(Theme.ink).frame(width: 1)
                .offset(
                    x: min(
                        geometry.size.width - 1,
                        max(0, Double(position.frame) / sessionRate / max(1, seconds) * geometry.size.width)))
        }.allowsHitTesting(false).accessibilityHidden(true)
    }
}

struct WorkspaceStatusBar: View {
    @ObservedObject var store: SessionStore
    @ObservedObject private var activity: WorkspaceActivity
    @State private var showingActivity = false
    init(store: SessionStore) {
        self.store = store
        self.activity = store.activity
    }
    var body: some View {
        HStack(spacing: 12) {
            if let remote = store.remoteGUI {
                RemoteSaveBadge(remote: remote)
                AudioConnectionBadge(remote: remote)
                RemoteProcessingBadge(remote: remote, progress: store.workProgress)
            } else {
                Label(
                    store.projectURL == nil ? "Not saved yet" : store.dirty ? "Unsaved changes" : "Saved",
                    systemImage: store.dirty || store.projectURL == nil ? "doc" : "checkmark.circle"
                ).foregroundStyle(store.dirty || store.projectURL == nil ? Theme.secondary : Theme.success)
            }
            Divider().frame(height: 14)
            if store.busy {
                if let progress = store.workProgress {
                    ProgressView(value: progress).frame(width: 60)
                } else {
                    ProgressView().controlSize(.mini)
                }
                if store.canCancelBackgroundWork {
                    Button("Cancel") { store.cancelBackgroundWork() }.buttonStyle(.plain)
                }
            }
            FileTransferStatus(progress: store.fileTransfer, fallback: store.status)
            Spacer(minLength: 4)
            Button {
                showingActivity = true
                activity.markRead()
            } label: {
                Label(
                    activity.unreadFailures > 0 ? "Activity · \(activity.unreadFailures)" : "Activity",
                    systemImage: activity.unreadFailures > 0 ? "exclamationmark.circle.fill" : "list.bullet.rectangle"
                )
                .foregroundStyle(activity.unreadFailures > 0 ? Theme.warning : Theme.secondary)
            }.buttonStyle(.plain).popover(isPresented: $showingActivity, arrowEdge: .top) {
                WorkspaceActivityPanel(store: store, activity: activity)
            }
        }.font(.system(size: 12)).padding(.horizontal, 16).frame(height: 34).background(Theme.paper)
            .overlay(alignment: .top) { Theme.line.frame(height: 1) }
    }
}

private struct FileTransferStatus: View {
    @ObservedObject var progress: FileTransferProgress
    let fallback: String
    var body: some View {
        if let state = progress.snapshot {
            HStack(spacing: 6) {
                if state.phase == .uploading, state.total > 0 {
                    ProgressView(value: Double(state.bytes), total: Double(state.total)).frame(width: 60)
                } else {
                    ProgressView().controlSize(.mini)
                }
                Text(state.label).lineLimit(1).help(state.label)
            }.accessibilityElement(children: .combine)
        } else {
            Text(fallback).lineLimit(1).help(fallback)
        }
    }
}

private struct RemoteSaveBadge: View {
    @ObservedObject var remote: RemoteGUIController
    var body: some View {
        Label(
            remote.projectSaveStatus.rawValue,
            systemImage: remote.projectSaveStatus == .saved ? "checkmark.circle" : "doc"
        )
        .foregroundStyle(remote.projectSaveStatus == .saved ? Theme.success : Theme.secondary)
        .help(remote.projectSaveHelp)
    }
}

private struct AudioConnectionBadge: View {
    @ObservedObject var remote: RemoteGUIController
    var body: some View {
        Button {
            remote.showConnection = true
        } label: {
            Label(
                remote.connected ? "Audio connected" : "Audio offline",
                systemImage: remote.connected ? "checkmark.circle.fill" : "exclamationmark.triangle"
            )
            .foregroundStyle(remote.connected ? Theme.success : Theme.warning)
        }.buttonStyle(.plain).help("Review audio connection and recover unsent edits")
    }
}

private struct RemoteProcessingBadge: View {
    @ObservedObject var remote: RemoteGUIController
    let progress: Double?
    var body: some View {
        if let label = remote.activeWork {
            HStack(spacing: 5) {
                if let progress {
                    ProgressView(value: max(0, min(1, progress))).frame(width: 52)
                } else {
                    ProgressView().controlSize(.mini)
                }
                Text(label).lineLimit(1)
            }.help("Processing on the audio server. Progress and failures remain in Activity.")
        }
    }
}

private struct WorkspaceActivityPanel: View {
    @ObservedObject var store: SessionStore
    @ObservedObject var activity: WorkspaceActivity
    @Environment(\.dismiss) private var dismiss
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Activity").font(.system(size: 18, weight: .semibold))
                Spacer()
                Button("Clear") { activity.clear() }.disabled(activity.entries.isEmpty)
            }
            if store.busy {
                HStack {
                    ProgressView().controlSize(.small)
                    Text(store.status)
                    Spacer()
                    if store.canCancelBackgroundWork { Button("Cancel processing") { store.cancelBackgroundWork() } }
                }.font(.system(size: 12))
            }
            if let remote = store.remoteGUI {
                RemoteProcessingBadge(remote: remote, progress: store.workProgress)
            }
            if let remote = store.remoteGUI, !remote.connected || remote.unsentEdit != nil || remote.lastError != nil {
                Button("Review connection and unsent edits…") {
                    dismiss()
                    DispatchQueue.main.async { remote.showConnection = true }
                }.buttonStyle(RoomButtonStyle())
            }
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 10) {
                    if activity.entries.isEmpty { Text("No recent activity.").foregroundStyle(Theme.secondary) }
                    ForEach(activity.entries) { entry in
                        HStack(alignment: .top, spacing: 9) {
                            Image(
                                systemName: entry.kind == .failure
                                    ? "exclamationmark.triangle.fill"
                                    : entry.kind == .success ? "checkmark.circle" : "info.circle"
                            )
                            .foregroundStyle(entry.kind == .failure ? Color.orange : Theme.secondary)
                            VStack(alignment: .leading, spacing: 3) {
                                Text(entry.message).textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
                                Text(entry.date, style: .time).foregroundStyle(Theme.secondary)
                                if let title = entry.recoveryTitle, let recovery = entry.recovery {
                                    Button(title) {
                                        dismiss()
                                        DispatchQueue.main.async { recovery() }
                                    }.buttonStyle(.link)
                                }
                            }
                        }.font(.system(size: 12)).padding(10).frame(maxWidth: .infinity, alignment: .leading)
                            .background(Theme.well, in: RoundedRectangle(cornerRadius: 6))
                    }
                }
            }
        }.padding(18).frame(width: 510, height: 420).background(Theme.paper).foregroundStyle(Theme.ink)
    }
}
