import AppKit
import SoundroomCore
import SwiftUI

/// A track header being dragged by its grip; `offset` is the vertical drag distance.
struct TrackDrag: Equatable {
    var id: UUID
    var offset: Double
}
/// Drag state changes at pointer rate, so only the track headers subscribe;
/// the rest of the editor does not redraw while a header follows the pointer.
@MainActor final class TrackDragState: ObservableObject {
    @Published var current: TrackDrag?
}

/// Row geometry for dragging a track header among rows of differing heights
/// (automation and inline editors make some rows taller).
enum TrackReordering {
    /// The dragged row's final index. A neighbor swaps places once the dragged
    /// row's leading edge (bottom moving down, top moving up) passes its middle.
    static func destination(from index: Int, offset: Double, heights: [Double]) -> Int {
        guard heights.indices.contains(index), offset.isFinite else { return index }
        let tops = rowTops(heights)
        let top = tops[index] + offset
        let bottom = top + heights[index]
        return heights.indices.filter { row in
            let middle = tops[row] + heights[row] / 2
            return row < index ? top > middle : row > index && bottom > middle
        }.count
    }
    /// Where the insertion marker is drawn, measured from the first row's top;
    /// nil when the drop would leave the order unchanged.
    static func markerY(from index: Int, to destination: Int, heights: [Double]) -> Double? {
        guard destination != index, heights.indices.contains(index), heights.indices.contains(destination) else {
            return nil
        }
        // Moving down inserts below the row now at the destination.
        return heights[..<(destination < index ? destination : destination + 1)].reduce(0, +)
    }
    private static func rowTops(_ heights: [Double]) -> [Double] {
        var top = 0.0
        return heights.map { height in
            defer { top += height }
            return top
        }
    }
}

extension SessionStore {
    var trackDrag: TrackDrag? {
        get { trackDragging.current }
        set { trackDragging.current = newValue }
    }
    /// The height a track occupies in the editor, including its open lanes.
    func trackRowHeight(_ track: Track) -> Double {
        trackSize.height + (showsAutomation(track) ? 84 : 0) + inlineDrumHeight(for: track)
            + inlineSoundHeight(for: track)
    }
    var trackRowHeights: [Double] { session.tracks.map(trackRowHeight) }
    var canReorderTracks: Bool { !busy && !isRecording }
    func canMoveTrack(_ id: UUID, by offset: Int) -> Bool {
        guard canReorderTracks, let index = session.tracks.firstIndex(where: { $0.id == id }) else { return false }
        return session.tracks.indices.contains(index + offset)
    }
    func moveTrack(_ id: UUID, by offset: Int) {
        guard canMoveTrack(id, by: offset), let index = session.tracks.firstIndex(where: { $0.id == id }) else {
            return
        }
        moveTrack(id, to: index + offset)
    }
    /// Moves a track to `destination` in the final order. One undo step.
    func moveTrack(_ id: UUID, to destination: Int) {
        guard let index = session.tracks.firstIndex(where: { $0.id == id }),
            session.tracks.indices.contains(destination), destination != index
        else { return }
        edit("Move track") {
            let track = session.tracks.remove(at: index)
            session.tracks.insert(track, at: destination)
            selectedTrack = id
        }
    }
    func finishTrackDrag() {
        defer { trackDrag = nil }
        guard let drag = trackDrag, let index = session.tracks.firstIndex(where: { $0.id == drag.id }) else { return }
        moveTrack(drag.id, to: TrackReordering.destination(from: index, offset: drag.offset, heights: trackRowHeights))
        focusEditor()
    }
}

/// The track headers with their open lanes. The dragged header follows the
/// pointer above its neighbors while a marker shows where it will land.
struct TrackHeaderList: View {
    @ObservedObject var store: SessionStore
    @ObservedObject var dragging: TrackDragState
    var body: some View {
        VStack(spacing: 0) {
            ForEach(store.session.tracks) { track in
                let drag = dragging.current?.id == track.id ? dragging.current : nil
                TrackControls(store: store, track: track).frame(height: store.trackSize.height)
                    .background(drag != nil ? Theme.raised : Color.clear)
                    .opacity(drag != nil ? 0.85 : 1)
                    .shadow(color: .black.opacity(drag != nil ? 0.4 : 0), radius: 6, y: 2)
                    .offset(y: drag?.offset ?? 0)
                    .zIndex(drag != nil ? 1 : 0)
                if store.showsAutomation(track) { AutomationLaneHeader(store: store).frame(height: 84) }
                if store.inlineDrumHeight(for: track) > 0, let drums = store.inlineDrums {
                    InlineDrumHeader(store: store, controller: drums, editor: drums.editor).frame(
                        height: InlineDrumLayout.height
                    ).id(drums.id)
                }
                if store.inlineSoundHeight(for: track) > 0, let sound = store.inlineSound {
                    InlineSoundHeader(store: store, controller: sound).frame(height: InlineSoundLayout.height).id(
                        sound.id)
                }
            }
        }.overlay(alignment: .topLeading) { marker }
    }
    @ViewBuilder private var marker: some View {
        if let drag = dragging.current, let index = store.session.tracks.firstIndex(where: { $0.id == drag.id }) {
            let heights = store.trackRowHeights
            let destination = TrackReordering.destination(from: index, offset: drag.offset, heights: heights)
            if let y = TrackReordering.markerY(from: index, to: destination, heights: heights) {
                Theme.accent.frame(height: 2).offset(y: y - 1).allowsHitTesting(false)
            }
        }
    }
}

/// The grip at a track header's leading edge: drag it up or down to reorder.
struct TrackReorderHandle: View {
    @ObservedObject var store: SessionStore
    let track: Track
    @State private var hovering = false
    @State private var dragging = false
    /// Whether this view has a cursor on AppKit's stack, so every push is
    /// popped exactly once, including when the header disappears mid-hover.
    @State private var cursorPushed = false
    var body: some View {
        Image(systemName: "line.3.horizontal").font(.system(size: 10, weight: .semibold))
            .foregroundStyle(hovering || dragging ? Theme.ink : Theme.secondary.opacity(0.7))
            .frame(width: 14, height: 22).contentShape(Rectangle())
            .onHover { inside in
                hovering = inside
                if !dragging { showCursor(inside && store.canReorderTracks ? .openHand : nil) }
            }
            .gesture(
                DragGesture(minimumDistance: 3, coordinateSpace: .global)
                    .onChanged { value in
                        guard dragging || store.canReorderTracks else { return }
                        if !dragging {
                            dragging = true
                            showCursor(.closedHand)
                            store.choose(track: track.id)
                        }
                        store.trackDrag = TrackDrag(id: track.id, offset: value.translation.height)
                    }
                    .onEnded { _ in
                        guard dragging else { return }
                        dragging = false
                        showCursor(hovering ? .openHand : nil)
                        store.finishTrackDrag()
                    }
            )
            .onDisappear { showCursor(nil) }
            .help("Drag to reorder tracks")
            .accessibilityElement()
            .accessibilityLabel("Reorder \(track.name)")
            .accessibilityAction(named: "Move up") { store.moveTrack(track.id, by: -1) }
            .accessibilityAction(named: "Move down") { store.moveTrack(track.id, by: 1) }
    }
    private func showCursor(_ cursor: NSCursor?) {
        if cursorPushed {
            NSCursor.pop()
            cursorPushed = false
        }
        if let cursor {
            cursor.push()
            cursorPushed = true
        }
    }
}
