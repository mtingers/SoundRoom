import AppKit
import SwiftUI
import SoundroomCore

/// SwiftUI's Canvas has no per-clip native hit targets. Capture the secondary click
/// before menu construction, using the real lane view's coordinates rather than hover.
struct ClipContextPointer: NSViewRepresentable {
    let store: SessionStore
    let trackID: UUID
    func makeCoordinator() -> Coordinator { Coordinator(store: store, trackID: trackID) }
    func makeNSView(context: Context) -> NSView {
        let view = NSView(frame: .zero); context.coordinator.install(view); return view
    }
    func updateNSView(_ view: NSView, context: Context) { context.coordinator.store = store; context.coordinator.trackID = trackID }
    static func dismantleNSView(_ view: NSView, coordinator: Coordinator) { coordinator.remove() }
    @MainActor final class Coordinator: NSObject {
        var store: SessionStore
        var trackID: UUID
        weak var view: NSView?
        private var monitor: Any?
        init(store: SessionStore, trackID: UUID) { self.store = store; self.trackID = trackID; super.init() }
        func install(_ view: NSView) {
            self.view = view
            monitor = NSEvent.addLocalMonitorForEvents(matching: .rightMouseDown) { [weak self] event in
                guard let self, let view = self.view, let window = view.window, event.window === window else { return event }
                let point = view.convert(event.locationInWindow, from: nil)
                guard view.bounds.contains(point) else { return event }
                let frame = max(0, Int(point.x / self.store.zoom * sessionRate))
                self.store.timelineEditing.contextTrack = self.trackID
                self.store.timelineEditing.contextClip = self.store.session.tracks.first { $0.id == self.trackID }?.clips.last { $0.start <= frame && $0.end > frame }?.id
                self.store.timelineEditing.contextDate = Date()
                let clip = self.store.session.tracks.first { $0.id == self.trackID }?.clips.last { $0.start <= frame && $0.end > frame }
                if !self.store.busy, !self.store.isRecording { self.store.choose(track: self.trackID, clip: clip?.id) }
                // SwiftUI constructs Canvas context menus before the secondary click.
                // Build this native menu now, from the actual pointer target.
                NSMenu.popUpContextMenu(self.menu(for: clip), with: event, for: view)
                return nil
            }
        }
        func remove() { if let monitor { NSEvent.removeMonitor(monitor) }; monitor = nil }

        private enum Action {
            case history(UUID, UUID, UUID, Bool), split, copy, cut, paste, soundDesign, inlineSound, drumDesign, inlineDrums, enhance, separate, align, move(UUID), moveAutomation, delete
        }
        private func item(_ title: String, action: Action, enabled: Bool = true) -> NSMenuItem {
            let item = NSMenuItem(title: title, action: #selector(invoke(_:)), keyEquivalent: "")
            item.target = self; item.representedObject = action; item.isEnabled = enabled
            return item
        }
        private func menu(for clip: Clip?) -> NSMenu {
            let editable = !store.busy && !store.isRecording
            let menu = NSMenu(); menu.autoenablesItems = false
            menu.addItem(item("Enhance vocals…", action: .enhance, enabled: store.canUseTrackTools(trackID)))
            menu.addItem(item("Separate source…", action: .separate, enabled: store.canSeparateSource && clip != nil))
            menu.addItem(.separator())
            if let clip {
                let history = NSMenu(); history.autoenablesItems = false
                let states = Array((clip.previousStates ?? []).reversed())
                if states.isEmpty { let empty = NSMenuItem(title: "No previous states", action: nil, keyEquivalent: ""); empty.isEnabled = false; history.addItem(empty) }
                for (index, state) in states.enumerated() {
                    let actions = NSMenu(); actions.autoenablesItems = false
                    actions.addItem(item("Restore This State", action: .history(state.id, clip.id, trackID, false), enabled: editable))
                    actions.addItem(item("Create New Track From State", action: .history(state.id, clip.id, trackID, true), enabled: editable))
                    let entry = NSMenuItem(title: "\(index + 1). Before \(state.operation) · \(state.name)", action: nil, keyEquivalent: "")
                    entry.submenu = actions; history.addItem(entry)
                }
                let entry = NSMenuItem(title: "Clip History", action: nil, keyEquivalent: ""); entry.submenu = history
                menu.addItem(entry); menu.addItem(.separator())
            }
            menu.addItem(item("Split at playhead", action: .split, enabled: editable))
            menu.addItem(item("Copy", action: .copy, enabled: editable && clip != nil))
            menu.addItem(item("Cut", action: .cut, enabled: editable && clip != nil))
            menu.addItem(item("Paste", action: .paste, enabled: editable && store.canPaste))
            if clip?.editableSoundDesign != nil {
                menu.addItem(item("Edit notes inline", action: .inlineSound, enabled: editable))
                menu.addItem(item("Full sound designer…", action: .soundDesign, enabled: editable))
            }
            if clip?.editableDrumPattern != nil {
                menu.addItem(item("Edit beat inline", action: .inlineDrums, enabled: editable))
                menu.addItem(item("Full drum designer…", action: .drumDesign, enabled: editable))
            }
            menu.addItem(item("Align clip to playhead", action: .align, enabled: editable && clip != nil))
            let destinations = NSMenu(); destinations.autoenablesItems = false
            for track in store.session.tracks where track.id != trackID { destinations.addItem(item(track.name, action: .move(track.id), enabled: editable && clip != nil)) }
            let move = NSMenuItem(title: "Move clip to track", action: nil, keyEquivalent: ""); move.submenu = destinations
            move.isEnabled = editable && clip != nil && !destinations.items.isEmpty; menu.addItem(move)
            let automation = item("Move automation with clips", action: .moveAutomation, enabled: editable)
            automation.state = store.moveAutomationWithClips ? .on : .off; menu.addItem(automation)
            menu.addItem(item("Delete selected audio", action: .delete, enabled: editable && clip != nil))
            return menu
        }
        @objc private func invoke(_ sender: NSMenuItem) {
            guard !store.busy, !store.isRecording, let action = sender.representedObject as? Action else { return }
            switch action {
            case let .history(state, clip, track, newTrack): store.restoreClipState(state, clipID: clip, trackID: track, toNewTrack: newTrack)
            case .split: store.split()
            case .copy: store.copy()
            case .cut: store.cut()
            case .paste: store.paste()
            case .soundDesign: store.requestSoundDesigner(editClip: true)
            case .inlineSound: store.toggleInlineSound(trackID: trackID, clipID: store.selectedClip)
            case .drumDesign: store.requestDrumDesigner(editClip: true)
            case .inlineDrums: store.toggleInlineDrums(trackID: trackID, clipID: store.selectedClip)
            case .enhance: store.requestVocalEnhancement(trackID: trackID)
            case .separate: store.requestSourceSeparation()
            case .align: store.alignToPlayhead()
            case let .move(track): store.moveClipToTrack(track)
            case .moveAutomation: store.moveAutomationWithClips.toggle()
            case .delete: store.deleteSelectedClips()
            }
        }
    }
}
extension SessionStore {
    func clipForContextMenu(track: Track) -> Clip? {
        let pointer = timelineEditing
        if pointer.contextTrack == track.id, Date().timeIntervalSince(pointer.contextDate) < 2 {
            return track.clips.first { $0.id == pointer.contextClip }
        }
        return track.clips.first { $0.id == selectedClip }
    }
}
