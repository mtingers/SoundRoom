import AppKit
import SoundroomCore
import SoundroomRemote
import SwiftUI

@main struct SoundroomApp {
    @MainActor static func main() {
        let configuration = ServerLaunchConfiguration()
        do { try configuration.validate() } catch {
            FileHandle.standardError.write(Data((error.localizedDescription + "\n").utf8))
            exit(2)
        }
        let app = NSApplication.shared
        let delegate = AppDelegate(configuration: configuration)
        app.delegate = delegate
        app.setActivationPolicy(.regular)
        app.run()
        withExtendedLifetime(delegate) {}
    }
}

@MainActor final class EditorHostingView: NSHostingView<MainView> {
    required init(rootView: MainView) {
        super.init(rootView: rootView)
        // NSWindow owns the editor's size limits. Probing this geometry-driven
        // root for intrinsic/min/max sizes creates a repeated layout cycle.
        sizingOptions = []
    }
    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("Use the root-view initializer") }
    override var acceptsFirstResponder: Bool { true }
}

@MainActor
final class AppDelegate: NSResponder, NSApplicationDelegate, NSWindowDelegate, NSMenuItemValidation, NSMenuDelegate {
    let store: SessionStore
    let configuration: ServerLaunchConfiguration
    private var remoteController: RemoteGUIController?
    private var queuedOpenURLs: [URL] = []
    init(configuration: ServerLaunchConfiguration = .init()) {
        self.configuration = configuration
        store = SessionStore(presentation: AppKitSessionPresentation())
        super.init()
    }
    required init?(coder: NSCoder) { fatalError("Use the configured app initializer") }
    private var window: NSWindow!
    private var keyMonitor: Any?
    private var recentMenu: NSMenu?
    private var closingApproved = false
    private var closingRequestInFlight = false
    func applicationDidFinishLaunching(_ notification: Notification) {
        window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1260, height: 790),
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView], backing: .buffered,
            defer: false)
        window.title = "Soundroom"
        window.titlebarAppearsTransparent = true
        window.identifier = NSUserInterfaceItemIdentifier("SoundroomEditor")
        NSApp.appearance = NSAppearance(named: .darkAqua)
        window.appearance = NSAppearance(named: .darkAqua)
        window.backgroundColor = NSColor(calibratedRed: 0.075, green: 0.085, blue: 0.105, alpha: 1)
        window.minSize = NSSize(width: 950, height: 660)
        window.contentView = EditorHostingView(rootView: MainView(store: store))
        window.delegate = self
        window.center()
        window.setFrameAutosaveName("SoundroomMainWindow")
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        buildMenus()
        Task {
            store.status = "Connecting to the hardware server…"
            do {
                remoteController = try await ServerBootstrap.connect(store: store, configuration: configuration)
                for _ in 0..<200 {
                    if store.remoteGUI?.connected == true && store.remoteGUI?.client.snapshot != nil { break }
                    try await Task.sleep(for: .milliseconds(100))
                }
                if !queuedOpenURLs.isEmpty, store.remoteGUI?.connected == true {
                    let urls = queuedOpenURLs
                    queuedOpenURLs = []
                    application(NSApp, open: urls)
                }
            } catch { store.report(error) }
        }
        window.makeFirstResponder(window.contentView)
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self,
                WorkspaceKeyboardRouting.accepts(
                    eventWindow: event.window, editorWindow: self.window,
                    isKey: self.window.isKeyWindow, hasNestedSheet: self.window.attachedSheet != nil,
                    responder: self.window.firstResponder),
                !self.store.timelineEditing.clipSettingsVisible,
                (!self.store.busy || event.keyCode == 49 && self.store.canToggleTransport
                    || event.keyCode == 15 && self.store.canToggleRecording),
                event.modifierFlags.intersection([.command, .control, .option]).isEmpty
            else { return event }
            switch event.keyCode {
            case 49: self.store.togglePlay()
            case 15: self.store.record()
            case 36: self.store.returnToStart()
            case 51, 117:
                if self.store.automationKeyboardFocused {
                    self.store.deleteSelectedAutomationPoints()
                } else {
                    self.store.deleteSelectedClips()
                }
            case 123, 124, 125, 126:
                if self.store.automationKeyboardFocused {
                    self.store.nudgeSelectedAutomation(
                        event.keyCode == 123 || event.keyCode == 125 ? -1 : 1,
                        large: event.modifierFlags.contains(.shift),
                        vertically: event.keyCode == 125 || event.keyCode == 126)
                } else if event.keyCode == 123 || event.keyCode == 124 {
                    self.store.nudge(event.keyCode == 123 ? -1 : 1, large: event.modifierFlags.contains(.shift))
                } else {
                    return event
                }
            case 37: if !self.store.isRecording && self.store.canToggleLoop { self.store.loopSelection.toggle() }
            case 40: self.store.toggleMetronome()
            case 48:
                self.store.jumpToClipEdge(forward: !event.modifierFlags.contains(.shift))
            case 3: self.store.revealPlayhead()
            case 53:
                self.store.stopTransport()
                self.store.selection = nil
                self.store.selectedClip = nil
                self.store.selectedClips = []
            default: return event
            }
            return nil
        }
    }
    private var editingAllowed: Bool {
        window?.isKeyWindow == true && window.attachedSheet == nil && NSApp.modalWindow == nil && !store.busy
            && !store.isRecording
    }
    func validateMenuItem(_ item: NSMenuItem) -> Bool {
        let action = item.action
        if let text = NSApp.keyWindow?.firstResponder as? NSTextView {
            if action == #selector(cut(_:)) { return text.isEditable && text.selectedRange().length > 0 }
            if action == #selector(copy(_:)) { return text.isSelectable && text.selectedRange().length > 0 }
            if action == #selector(paste(_:)) { return text.isEditable }
            if action == #selector(selectAll(_:)) { return text.isSelectable && !text.string.isEmpty }
            if action == #selector(deleteAudio) { return text.isEditable && text.selectedRange().length > 0 }
            // ⌘⌫ deletes to the line start while typing.
            if action == #selector(deleteTrack) { return false }
        }
        if action == #selector(undo) || action == #selector(redo) {
            if let text = NSApp.keyWindow?.firstResponder as? NSTextView {
                item.title = action == #selector(undo) ? "Undo Typing" : "Redo Typing"
                return action == #selector(undo) ? text.undoManager?.canUndo == true : text.undoManager?.canRedo == true
            }
            item.title = action == #selector(undo) ? store.undoTitle : store.redoTitle
            return editingAllowed && (action == #selector(undo) ? store.canUndo : store.canRedo)
        }
        if store.automationKeyboardFocused {
            if action == #selector(copy(_:)) { return editingAllowed && store.automationSelectionRange != nil }
            if action == #selector(cut(_:)) || action == #selector(deleteAudio) {
                return editingAllowed && !store.automationEditing.selectedFrames.isEmpty
            }
            if action == #selector(paste(_:)) { return editingAllowed && store.canPasteAutomation }
        }
        if action == #selector(cut(_:)) || action == #selector(copy(_:)) || action == #selector(deleteAudio) {
            return editingAllowed && (store.hasAudioSelection || !store.selectedClips.isEmpty)
        }
        if action == #selector(paste(_:)) { return editingAllowed && store.canPaste }
        if action == #selector(copyTrack) || action == #selector(copyTrackWithAutomation) {
            return editingAllowed && store.currentTrack != nil
        }
        if action == #selector(deleteTrack) { return editingAllowed && store.canDeleteSelectedTrack }
        if action == #selector(moveTrackUp) || action == #selector(moveTrackDown) {
            return editingAllowed
                && store.selectedTrack.map { store.canMoveTrack($0, by: action == #selector(moveTrackUp) ? -1 : 1) }
                    == true
        }
        if action == #selector(pasteTrack) { return editingAllowed && store.canPasteTrack }
        if action == #selector(commandSearch) { return editingAllowed }
        if action == #selector(selectAll(_:)) { return editingAllowed && !(store.currentTrack?.clips.isEmpty ?? true) }
        if action == #selector(trim) { return editingAllowed && store.hasAudioSelection && store.selection != nil }
        if action == #selector(align) { return editingAllowed && store.currentClip != nil }
        if action == #selector(split) {
            return editingAllowed
                && store.currentTrack?.clips.contains(where: { $0.start < store.playhead && $0.end > store.playhead })
                    == true
        }
        if action == #selector(combine) {
            return editingAllowed && !(NSApp.keyWindow?.firstResponder is NSTextView) && store.canCombineClips
        }
        if action == #selector(addTrack) || action == #selector(new) || action == #selector(open)
            || action == #selector(importAudio) || action == #selector(soundDesigner)
            || action == #selector(drumDesigner)
        {
            return editingAllowed
        }
        if action == #selector(export) { return editingAllowed && store.session.end > 0 }
        if action == #selector(save) || action == #selector(saveAs) {
            return !store.busy && window.attachedSheet == nil && NSApp.modalWindow == nil
        }
        if action == #selector(renameSession) { return editingAllowed && store.canRenameSession }
        if action == #selector(metronome) {
            item.state = store.metronome ? .on : .off
            return true
        }
        if action == #selector(zoomIn) { return editingAllowed && store.zoom < SessionStore.zoomRange.upperBound }
        if action == #selector(zoomOut) { return editingAllowed && store.zoom > SessionStore.zoomRange.lowerBound }
        if [
            #selector(zoomSelection), #selector(fit), #selector(reveal), #selector(follow), #selector(storage),
            #selector(loop),
        ].contains(action) {
            if action == #selector(follow) { item.state = store.followPlayhead ? .on : .off }
            if action == #selector(loop) { item.state = store.loopSelection ? .on : .off }
            return editingAllowed
                && (action == #selector(loop)
                    ? store.canToggleLoop : action != #selector(zoomSelection) || store.selectedRange != nil)
        }
        return true
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        if closingApproved { return true }
        guard !closingRequestInFlight else { return false }
        closingRequestInFlight = true
        Task {
            defer { closingRequestInFlight = false }
            if await store.confirmAbandonAsync() {
                closingApproved = true
                store.rememberPreferences()
                sender.performClose(nil)
            }
        }
        return false
    }
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        Task {
            let approved = closingApproved ? true : await store.confirmAbandonAsync()
            if approved {
                store.rememberPreferences()
                await store.prepareToQuit()
            }
            sender.reply(toApplicationShouldTerminate: approved)
        }
        return .terminateLater
    }
    func application(_ sender: NSApplication, open urls: [URL]) {
        if store.remoteGUI?.connected != true {
            queuedOpenURLs.append(contentsOf: urls)
            return
        }
        if let project = urls.first(where: { $0.pathExtension == "soundroom" }) {
            store.requestOpen(url: project)
        } else {
            store.importAudio(urls: urls)
        }
    }
    private func buildMenus() {
        let menu = NSMenu()
        func submenu(_ name: String) -> NSMenu {
            let parent = NSMenuItem()
            parent.title = name
            let sub = NSMenu(title: name)
            parent.submenu = sub
            menu.addItem(parent)
            return sub
        }
        func item(_ menu: NSMenu, _ title: String, _ selector: Selector, _ key: String = "", shift: Bool = false) {
            let entry = NSMenuItem(title: title, action: selector, keyEquivalent: key)
            entry.target = self
            if shift { entry.keyEquivalentModifierMask = [.command, .shift] }
            menu.addItem(entry)
        }
        let app = submenu("Soundroom")
        item(app, "About Soundroom", #selector(about))
        app.addItem(.separator())
        item(app, "Audio Connections…", #selector(connections), ",")
        app.addItem(.separator())
        item(app, "Hide Soundroom", #selector(hide), "h")
        item(app, "Quit Soundroom", #selector(quit), "q")
        let file = submenu("File")
        item(file, "New Session", #selector(new), "n")
        item(file, "Open Session…", #selector(open), "o")
        let recent = NSMenu(title: "Open Recent")
        let recentItem = NSMenuItem(title: "Open Recent", action: nil, keyEquivalent: "")
        recentItem.submenu = recent
        file.addItem(recentItem)
        recentMenu = recent
        recent.delegate = self
        rebuildRecentMenu(recent)
        item(file, "Save", #selector(save), "s")
        item(file, "Save As…", #selector(saveAs), "s", shift: true)
        item(file, "Rename Session…", #selector(renameSession))
        file.addItem(.separator())
        item(file, "Import Audio…", #selector(importAudio), "i")
        item(file, "Export Mix…", #selector(export), "e", shift: true)
        let edit = submenu("Edit")
        item(edit, "Undo", #selector(undo), "z")
        item(edit, "Redo", #selector(redo), "z", shift: true)
        edit.addItem(.separator())
        // Standard responder-chain actions preserve native text-field editing.
        for (title, action, key) in [
            ("Cut", "cut:", "x"), ("Copy", "copy:", "c"), ("Paste", "paste:", "v"), ("Select All", "selectAll:", "a"),
        ] {
            edit.addItem(NSMenuItem(title: title, action: NSSelectorFromString(action), keyEquivalent: key))
        }
        item(edit, "Delete Selected Audio", #selector(deleteAudio))
        edit.addItem(.separator())
        item(edit, "Copy Track", #selector(copyTrack))
        item(edit, "Copy Track + Automation", #selector(copyTrackWithAutomation))
        item(edit, "Paste Track", #selector(pasteTrack))
        // ⌘⌫ deletes the selected track (plain Delete removes selected audio).
        item(edit, "Delete Track", #selector(deleteTrack), "\u{8}")
        for (title, selector, arrow) in [
            ("Move Track Up", #selector(moveTrackUp), NSUpArrowFunctionKey),
            ("Move Track Down", #selector(moveTrackDown), NSDownArrowFunctionKey),
        ] {
            item(edit, title, selector, String(Character(UnicodeScalar(UInt16(arrow))!)))
            edit.items.last?.keyEquivalentModifierMask = [.command, .option]
        }
        edit.addItem(.separator())
        item(edit, "Command Search…", #selector(commandSearch), "k")
        item(edit, "Split at Playhead", #selector(split), "e")
        item(edit, "Trim Track to Selection", #selector(trim), "t")
        item(edit, "Combine Selected Clips", #selector(combine), "j", shift: true)
        item(edit, "Align Clip to Playhead", #selector(align), "l")
        item(edit, "Add Audio Track", #selector(addTrack), "n", shift: true)
        item(edit, "Sound Designer…", #selector(soundDesigner), "d", shift: true)
        item(edit, "Drum Loop Designer…", #selector(drumDesigner), "b", shift: true)
        let view = submenu("View")
        item(view, "Zoom In", #selector(zoomIn), "=")
        item(view, "Zoom Out", #selector(zoomOut), "-")
        // ⌘+ (⇧= on most layouts) as a hidden alias of Zoom In.
        item(view, "Zoom In", #selector(zoomIn), "+")
        view.items.last?.isHidden = true
        item(view, "Zoom to Selection", #selector(zoomSelection), "j")
        item(view, "Fit Session", #selector(fit), "0")
        item(view, "Reveal Playhead (F)", #selector(reveal))
        item(view, "Follow Playhead", #selector(follow))
        item(view, "Project Storage…", #selector(storage))
        let transport = submenu("Transport")
        item(transport, "Play / Pause (Space)", #selector(play))
        item(transport, "Record / Finish (R)", #selector(record))
        item(transport, "Return to Start (Return)", #selector(start))
        item(transport, "Loop (L)", #selector(loop))
        item(transport, "Metronome (K)", #selector(metronome))
        let help = submenu("Help")
        item(help, "Soundroom Quick Guide", #selector(guide))
        NSApp.mainMenu = menu
        // Forward standard edit actions from the responder chain to the session.
        window.nextResponder = self
    }
    func menuNeedsUpdate(_ menu: NSMenu) { if menu === recentMenu { rebuildRecentMenu(menu) } }
    private func rebuildRecentMenu(_ menu: NSMenu) {
        menu.removeAllItems()
        for recent in store.recentProjectEntries {
            let title: String
            let detail: String
            switch recent {
            case .file(let url):
                title = url.deletingPathExtension().lastPathComponent
                detail = url.deletingLastPathComponent().path
            case .server(let name):
                title = (name as NSString).deletingPathExtension
                detail = "Saved on the audio server"
                if store.remoteGUI == nil { continue }
            }
            let entry = NSMenuItem(title: title, action: #selector(openRecent(_:)), keyEquivalent: "")
            entry.target = self
            entry.representedObject = recent.key
            entry.toolTip = detail
            menu.addItem(entry)
        }
        if menu.items.isEmpty {
            let empty = NSMenuItem(title: "No recent projects", action: nil, keyEquivalent: "")
            empty.isEnabled = false
            menu.addItem(empty)
        }
    }
    @objc func new() { store.requestNew() }
    @objc func openRecent(_ sender: NSMenuItem) {
        guard editingAllowed, let key = sender.representedObject as? String else { return }
        switch SessionStore.RecentProject(key: key) {
        case .file(let url): store.requestOpen(url: url)
        case .server(let name): store.remoteGUI?.openServerProject(name)
        }
    }
    @objc func open() { store.requestOpen() }
    @objc func save() { store.requestSave() }
    @objc func saveAs() { store.requestSave(as: true) }
    @objc func renameSession() { store.requestSessionRename() }
    @objc func importAudio() { store.importAudio() }
    @objc func export() { if !store.busy && !store.isRecording && store.session.end > 0 { store.showExport = true } }
    @objc func undo() {
        if let text = NSApp.keyWindow?.firstResponder as? NSTextView {
            text.undoManager?.undo()
        } else if editingAllowed {
            store.undo()
        }
    }
    @objc func redo() {
        if let text = NSApp.keyWindow?.firstResponder as? NSTextView {
            text.undoManager?.redo()
        } else if editingAllowed {
            store.redo()
        }
    }
    @objc func cut(_ sender: Any?) {
        if let text = NSApp.keyWindow?.firstResponder as? NSTextView {
            text.cut(sender)
        } else if editingAllowed {
            if store.automationKeyboardFocused {
                store.copyAutomationRange()
                store.deleteSelectedAutomationPoints()
            } else {
                store.cut()
            }
        }
    }
    @objc func copy(_ sender: Any?) {
        if let text = NSApp.keyWindow?.firstResponder as? NSTextView {
            text.copy(sender)
        } else if editingAllowed {
            if store.automationKeyboardFocused { store.copyAutomationRange() } else { store.copy() }
        }
    }
    @objc func paste(_ sender: Any?) {
        if let text = NSApp.keyWindow?.firstResponder as? NSTextView {
            text.paste(sender)
        } else if editingAllowed {
            if store.automationKeyboardFocused {
                store.pasteAutomation()
            } else if !(NSApp.currentEvent?.isARepeat == true && store.pasteAwaitsServer) {
                store.paste()
            }
        }
    }
    @objc func copyTrack() { if editingAllowed { store.copyTrack() } }
    @objc func copyTrackWithAutomation() { if editingAllowed { store.copyTrack(includeAutomation: true) } }
    @objc func pasteTrack() { if editingAllowed { store.pasteTrack() } }
    @objc func moveTrackUp() { if editingAllowed, let id = store.selectedTrack { store.moveTrack(id, by: -1) } }
    @objc func moveTrackDown() { if editingAllowed, let id = store.selectedTrack { store.moveTrack(id, by: 1) } }
    @objc func deleteTrack() { if editingAllowed { store.deleteSelectedTrack() } }
    override func selectAll(_ sender: Any?) {
        if let text = NSApp.keyWindow?.firstResponder as? NSTextView {
            text.selectAll(sender)
        } else if editingAllowed {
            store.selectAll()
        }
    }
    @objc func deleteAudio() {
        if let text = NSApp.keyWindow?.firstResponder as? NSTextView {
            text.deleteBackward(nil)
        } else if editingAllowed {
            if store.automationKeyboardFocused {
                store.deleteSelectedAutomationPoints()
            } else {
                store.deleteSelectedClips()
            }
        }
    }
    @objc func combine() {
        if editingAllowed && !(NSApp.keyWindow?.firstResponder is NSTextView) && store.canCombineClips {
            store.combineClips()
        }
    }
    @objc func zoomSelection() { if editingAllowed { store.zoomToSelection() } }
    @objc func zoomIn() { if editingAllowed { store.requestTimelineZoom(1.5) } }
    @objc func zoomOut() { if editingAllowed { store.requestTimelineZoom(1 / 1.5) } }
    @objc func fit() { if editingAllowed { store.fitSession() } }
    @objc func reveal() { if editingAllowed { store.revealPlayhead() } }
    @objc func follow() { if editingAllowed { store.followPlayhead.toggle() } }
    @objc func loop() { if editingAllowed { store.loopSelection.toggle() } }
    @objc func metronome() { store.toggleMetronome() }
    @objc func storage() {
        guard editingAllowed else { return }
        let alert = NSAlert()
        alert.messageText = "Project storage"
        alert.informativeText =
            "Audio: \(ByteCountFormatter.string(fromByteCount: Int64(store.projectAudioBytes), countStyle: .file))\nUnused media: \(store.unusedAssetCount) files"
        alert.addButton(withTitle: "Done")
        if store.unusedAssetCount > 0 { alert.addButton(withTitle: "Review unused media…") }
        if alert.runModal() == .alertSecondButtonReturn { store.reviewUnusedMedia() }
    }
    @objc func split() { store.split() }
    @objc func trim() { store.trim() }
    @objc func align() { store.alignToPlayhead() }
    @objc func addTrack() { store.addTrack() }
    @objc func soundDesigner() { store.requestSoundDesigner() }
    @objc func drumDesigner() { store.requestDrumDesigner() }
    @objc func play() {
        if window?.isKeyWindow == true && window.attachedSheet == nil && NSApp.modalWindow == nil
            && store.canToggleTransport
        {
            store.togglePlay()
        }
    }
    @objc func commandSearch() { if editingAllowed { store.showCommandSearch = true } }
    @objc func record() {
        if window.isKeyWindow && window.attachedSheet == nil && store.canToggleRecording { store.record() }
    }
    @objc func start() { if editingAllowed { store.returnToStart() } }
    @objc func connections() {
        if !store.isPlaying && !store.busy {
            store.refreshDevices()
            store.showDevices = true
        }
    }
    @objc func hide() { NSApp.hide(nil) }
    @objc func quit() { NSApp.terminate(nil) }
    @objc func about() {
        NSApp.orderFrontStandardAboutPanel(options: [
            .applicationName: "Soundroom",
            .applicationVersion: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
                ?? "0.5",
            .version: Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "5",
        ])
    }
    @objc func guide() {
        let alert = NSAlert()
        alert.messageText = "Soundroom shortcuts"
        alert.informativeText =
            "Choose input and output from the device button. Arm tracks and press R to record; R or Space finishes.\n\nEffects — live Track / Master inserts; Play track / Play mix; Apply to audio bakes the reviewed scope\nAutomation — track A button shows Lane / Parameter controls\nAudio edits — select audio, then Normalize, fades or Reverse\nSound designer — Space plays/stops; arrows move notes; ⇧arrows resize; ⌘D duplicates\n\nSpace — play / pause\nReturn — back to the play start while playing; session start when stopped\n⌘E — split at playhead\n⌘X / C / V — cut / copy / paste\nDelete — remove selected audio\n⌘T — trim to selection\n⌘L — align clip to playhead\n← / → — nudge 10 ms; Shift — 1 second\nTab / ⇧Tab — next / previous clip edge\n⇧⌘D — sound designer\n⌘Z / ⇧⌘Z — undo / redo\n⌘= / ⌘− — zoom in / out (or pinch, ⌘-scroll)\n⌘J / ⌘0 — zoom selection / fit session\nL — loop selection; F — reveal playhead\nCopy Track / Copy Track + Automation — Edit menu; ⌘V pastes a new track\n⌘S — save session\n⇧⌘E — export mix\n\nUse headphones while overdubbing. Track automation uses session time. Input monitoring is optional in Audio Connections. Designer supports MIDI input and files."
        alert.runModal()
    }
}
