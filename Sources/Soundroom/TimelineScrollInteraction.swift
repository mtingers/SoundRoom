import AppKit
import SwiftUI
import SoundroomCore

/// UI-only interaction state. The timer exists only while a timeline gesture is
/// active and never participates in the audio clock or server control stream.
@MainActor final class TimelineScrollInteraction: ObservableObject {
    var viewport = TimelineViewport()
    var requestScroll: ((Double) -> Void)?
    var manualScroll: (() -> Void)?
    /// Zoom by a factor around a viewport-pixel x (nil: playhead or center).
    var zoom: ((Double, Double?) -> Void)?
    private weak var pointerView: NSView?
    private var updateDrag: ((Double) -> Void)?
    private var timer: Timer?
    private var dragClock = TimelineDragScrollClock()

    func attach(_ view: NSView) { pointerView = view }
    func pointerDown(at timestamp: TimeInterval) { dragClock.begin(at: timestamp) }
    func beginDrag(update: @escaping (Double) -> Void) {
        if updateDrag == nil {
            manualScroll?()
            if dragClock.lastTimestamp == nil { dragClock.begin(at: ProcessInfo.processInfo.systemUptime) }
            let timer = Timer(timeInterval: 1 / 30, repeats: true) { [weak self] _ in
                MainActor.assumeIsolated { self?.tick() }
            }
            self.timer = timer
            RunLoop.main.add(timer, forMode: .common)
        }
        updateDrag = update
    }
    func endDrag() {
        timer?.invalidate(); timer = nil; updateDrag = nil; dragClock.reset()
    }
    func tearDown() {
        endDrag(); pointerView = nil; requestScroll = nil; manualScroll = nil; zoom = nil
    }
    /// Pointer events get an immediate response; the held-pointer timer consumes
    /// the same clock budget, so dense events cannot double the scrolling speed.
    func updateDragPosition(contentX: Double, at timestamp: TimeInterval = ProcessInfo.processInfo.systemUptime) {
        guard let updateDrag else { return }
        let pointerX = contentX - viewport.offset
        let elapsed = dragClock.consume(at: timestamp)
        if let destination = viewport.dragScrollDestination(pointerX: pointerX, elapsed: elapsed) {
            viewport.offset = destination; requestScroll?(destination)
        }
        updateDrag(max(0, min(viewport.contentWidth, viewport.offset + pointerX)))
    }
    private func tick() {
        guard let updateDrag, let view = pointerView, let window = view.window else { endDrag(); return }
        // Mouse-up outside the app must still terminate the gesture timer.
        guard NSEvent.pressedMouseButtons & 1 != 0 else { endDrag(); return }
        let elapsed = dragClock.consume(at: ProcessInfo.processInfo.systemUptime)
        let windowPoint = window.convertPoint(fromScreen: NSEvent.mouseLocation)
        let local = view.convert(windowPoint, from: nil)
        guard let destination = viewport.dragScrollDestination(pointerX: local.x, elapsed: elapsed) else { return }
        viewport.offset = destination
        requestScroll?(destination)
        updateDrag(max(0, min(viewport.contentWidth, destination + local.x)))
    }

}

private struct TimelineScrollInteractionKey: EnvironmentKey {
    static let defaultValue: TimelineScrollInteraction? = nil
}
extension EnvironmentValues {
    var timelineScrollInteraction: TimelineScrollInteraction? {
        get { self[TimelineScrollInteractionKey.self] }
        set { self[TimelineScrollInteractionKey.self] = newValue }
    }
}

/// Observe real user wheel/scroller input before it changes geometry. Programmatic
/// Follow/reveal scrolling never passes through this path, so it cannot disable itself.
struct TimelineViewportInput: NSViewRepresentable {
    let interaction: TimelineScrollInteraction
    func makeCoordinator() -> Coordinator { Coordinator(interaction: interaction) }
    func makeNSView(context: Context) -> NSView {
        let view = NSView(frame: .zero)
        context.coordinator.install(view); return view
    }
    func updateNSView(_ view: NSView, context: Context) { interaction.attach(view) }
    static func dismantleNSView(_ view: NSView, coordinator: Coordinator) { coordinator.remove() }
    @MainActor final class Coordinator {
        let interaction: TimelineScrollInteraction
        weak var view: NSView?
        var monitor: Any?
        init(interaction: TimelineScrollInteraction) { self.interaction = interaction }
        func install(_ view: NSView) {
            self.view = view; interaction.attach(view)
            monitor = NSEvent.addLocalMonitorForEvents(matching: [.scrollWheel, .leftMouseDown, .magnify]) { [weak self] event in
                guard let self, let view = self.view, let window = view.window, event.window === window,
                      window.attachedSheet == nil else { return event }
                let point = view.convert(event.locationInWindow, from: nil)
                guard view.bounds.contains(point) else { return event }
                // Trackpad pinch, or ⌘ + scroll wheel, zooms around the pointer.
                if event.type == .magnify {
                    if event.magnification.isFinite { self.interaction.zoom?(max(0.5, 1 + event.magnification), point.x) }
                    return nil
                }
                if event.type == .scrollWheel, event.modifierFlags.contains(.command) {
                    let delta = abs(event.scrollingDeltaY) >= abs(event.scrollingDeltaX) ? event.scrollingDeltaY : event.scrollingDeltaX
                    if abs(delta) > 0.01 {
                        let step = event.hasPreciseScrollingDeltas ? delta * 0.01 : delta * 0.1
                        self.interaction.zoom?(Foundation.pow(2.0, Double(max(-1, min(1, step)))), point.x)
                    }
                    return nil
                }
                guard self.interaction.viewport.contentWidth > self.interaction.viewport.width else { return event }
                if event.type == .scrollWheel {
                    if abs(event.scrollingDeltaX) > 0.01 || (event.modifierFlags.contains(.shift) && abs(event.scrollingDeltaY) > 0.01) {
                        self.interaction.manualScroll?()
                    }
                } else if let root = window.contentView {
                    self.interaction.pointerDown(at: event.timestamp)
                    var hit: NSView? = root.hitTest(root.convert(event.locationInWindow, from: nil))
                    while let candidate = hit {
                        if let scroller = candidate as? NSScroller, scroller.bounds.width > scroller.bounds.height {
                            self.interaction.manualScroll?(); break
                        }
                        hit = candidate.superview
                    }
                }
                return event
            }
        }
        func remove() {
            if let monitor { NSEvent.removeMonitor(monitor) }; monitor = nil
            interaction.endDrag()
        }
    }
}

@MainActor extension SessionStore {
    /// Selection and the edit cursor belong to this controller. Continuous
    /// pointer updates must not enqueue transport commands on the hardware host.
    func selectTimelineDrag(track: UUID, start: Int, end: Int) {
        guard !isRecording, !busy else { return }
        automationEditing.hasFocus = false
        selectedTrack = track; selectedClip = nil; selectedClips = []
        let range = max(0, min(start, end))..<max(0, max(start, end))
        selection = EditSelection(trackID: track, range: range)
        playhead = range.lowerBound
    }
}
