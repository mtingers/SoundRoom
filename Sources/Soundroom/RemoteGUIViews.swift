import AppKit
import SwiftUI
import SoundroomCore
import SoundroomRemote
import UniformTypeIdentifiers

struct RemoteConnectionIndicator: View {
    @ObservedObject var remote: RemoteGUIController
    var body: some View {
        Button { remote.showConnection = true } label: {
            HStack(spacing: 5) {
                Circle().fill(remote.connected && remote.unsentEdit == nil ? Theme.accent : Color.orange).frame(width: 5, height: 5)
                Image(systemName: "server.rack")
                Text(remote.connectionLabel).lineLimit(1).frame(maxWidth: 90)
            }.font(.system(size: 10)).foregroundStyle(Theme.secondary)
        }.buttonStyle(.plain).accessibilityLabel("Server connection")
            .help(remote.unsentEdit == nil ? "Audio runs on the server. Selection, scroll and zoom stay in this window." : "An unsent edit is available for review in Server connection.")
    }
}

struct RemoteConnectionSheet: View {
    @ObservedObject var remote: RemoteGUIController
    @Environment(\.dismiss) private var dismiss
    @State private var showingConnect = false
    @State private var confirmingStop = false
    @State private var stopping = false
    @State private var stopNotice: String?
    @State private var showingBrowserAccess = false
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Label("Audio server", systemImage: "server.rack").font(.system(size: 20, weight: .semibold))
                Spacer(); Button("Done") { dismiss() }
            }
            Grid(alignment: .leading, horizontalSpacing: 18, verticalSpacing: 10) {
                GridRow { Text("Connection").foregroundStyle(Theme.secondary); Text(remote.connectionLabel) }
                GridRow { Text("Hardware host").foregroundStyle(Theme.secondary); Text(remote.serverName) }
                GridRow { Text("Address").foregroundStyle(Theme.secondary); Text("\(remote.host):\(String(remote.port))").textSelection(.enabled) }
                if remote.isLocalServer {
                    GridRow { Text("LAN hostname").foregroundStyle(Theme.secondary); Text("\(ProcessInfo.processInfo.hostName):\(String(remote.port))").textSelection(.enabled) }
                }
                GridRow { Text("Audio devices").foregroundStyle(Theme.secondary); Text("Owned by the server") }
            }.font(.system(size: 12))
            Divider().overlay(Theme.line)
            Text("Edits, routing and transport are shared. Track selection, scroll, zoom and editor drafts stay in each client.")
                .font(.system(size: 12)).foregroundStyle(Theme.secondary).fixedSize(horizontal: false, vertical: true)
            if !remote.peers.isEmpty {
                VStack(alignment: .leading, spacing: 7) {
                    Text("Connected clients").font(.system(size: 12, weight: .semibold))
                    ForEach(remote.peers) { peer in
                        HStack { Text(peer.name); Spacer(); Text(peer.role.rawValue.capitalized).foregroundStyle(Theme.secondary) }.font(.system(size: 11))
                    }
                }.padding(12).background(Theme.background, in: RoundedRectangle(cornerRadius: 8))
            }
            if let pairing = remote.pairingFile {
                HStack {
                    Label("Pair a GUI or terminal client", systemImage: "key.fill").font(.system(size: 12))
                    Spacer()
                    Button("Show credential file") { NSWorkspace.shared.activateFileViewerSelecting([pairing]) }.buttonStyle(RoomButtonStyle())
                }
                Text("Transfer the credential file securely to your other computer. A paired client can control this server’s hardware.")
                    .font(.system(size: 11)).foregroundStyle(Theme.secondary).fixedSize(horizontal: false, vertical: true)
            }
            if let error = remote.lastError { Text(error).font(.system(size: 11)).foregroundStyle(Color.orange).fixedSize(horizontal: false, vertical: true) }
            if let draft = remote.unsentEdit {
                VStack(alignment: .leading, spacing: 8) {
                    HStack {
                        Label("Unsent edit: \(draft.name)", systemImage: "exclamationmark.arrow.triangle.2.circlepath").font(.system(size: 12, weight: .semibold))
                        Spacer(); Button("Discard") { remote.discardUnsentEdit() }
                    }
                    Text("Copy a track, then paste it into a separate track for review.").font(.system(size: 11)).foregroundStyle(Theme.secondary)
                    ScrollView {
                        VStack(alignment: .leading, spacing: 6) {
                            ForEach(draft.tracks) { track in
                                HStack { Text(track.name).lineLimit(1); Spacer(); Button("Copy with automation") { remote.copyUnsentTrack(track.id) } }
                            }
                        }
                    }.frame(maxHeight: 90).font(.system(size: 11))
                }.padding(12).background(Theme.background, in: RoundedRectangle(cornerRadius: 8))
            }
            if let stopNotice { Text(stopNotice).font(.system(size: 11)).foregroundStyle(Theme.secondary).fixedSize(horizontal: false, vertical: true) }
            HStack {
                Button("Reconnect") { stopNotice = nil; remote.client.autoReconnect = true; remote.connect() }.buttonStyle(RoomButtonStyle()).disabled(stopping)
                Spacer()
                Button("Connect to another server…") { showingConnect = true }.buttonStyle(RoomButtonStyle()).disabled(stopping)
            }
            if remote.client.credential.role == .operator {
                HStack {
                    Button("Browser access…") { showingBrowserAccess = true }.buttonStyle(RoomButtonStyle()).disabled(!remote.connected || stopping)
                    Spacer()
                    Button(stopping ? "Stopping server…" : "Stop server…", role: .destructive) { confirmingStop = true }
                        .buttonStyle(RoomButtonStyle()).disabled(!remote.connected || stopping || remote.store?.isRecording == true || remote.store?.busy == true)
                }
            }
        }.padding(22).frame(width: 510).background(Theme.paper).foregroundStyle(Theme.ink).preferredColorScheme(.dark)
            .sheet(isPresented: $showingConnect) { RemoteConnectForm(current: remote) { dismiss() } }
            .sheet(isPresented: $showingBrowserAccess) { RemoteBrowserAccessSheet(remote: remote) }
            .alert("Stop the audio server?", isPresented: $confirmingStop) {
                Button("Cancel", role: .cancel) { }
                Button("Stop server", role: .destructive) { stopServer() }
            } message: {
                Text("Audio stops for every connected client. The current session is saved for recovery. Launch Soundroom on the hardware Mac again to start the server.")
            }
    }
    private func stopServer() {
        guard remote.connected, remote.client.credential.role == .operator,
              remote.store?.isRecording != true, remote.store?.busy != true else { return }
        stopping = true; stopNotice = nil
        let reconnect = remote.client.autoReconnect
        remote.client.autoReconnect = false
        remote.send(.editor(.init(operation: "server.shutdown"))) { reply in
            defer { stopping = false }
            if reply.accepted {
                remote.disconnect()
                stopNotice = "Server stopped. Launch Soundroom on the hardware Mac to start it again."
                remote.store?.status = stopNotice!
            } else if remote.connected {
                remote.client.autoReconnect = reconnect; stopNotice = reply.message
            } else {
                stopNotice = "The connection closed after the stop request. Launch Soundroom on the hardware Mac to reconnect."
                remote.store?.status = stopNotice!
            }
        }
    }
}

private struct RemoteBrowserAccessSheet: View {
    @ObservedObject var remote: RemoteGUIController
    @Environment(\.dismiss) private var dismiss
    @State private var name = "Browser"
    @State private var role: RemoteRole = .editor
    @State private var codeLifetime = 1800
    @State private var clients: [RemoteWebClientRecord] = []
    @State private var grant: RemoteWebPairingGrant?
    @State private var expires: Date?
    @State private var notice: String?
    @State private var waiting = false
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack { Text("Browser access").font(.system(size: 20, weight: .semibold)); Spacer(); Button("Done") { dismiss() } }
            Text(remote.serverName).font(.system(size: 12)).foregroundStyle(Theme.secondary)
            Grid(alignment: .leading, horizontalSpacing: 14, verticalSpacing: 10) {
                GridRow { Text("Client name"); TextField("Browser", text: $name).textFieldStyle(.roundedBorder) }
                GridRow { Text("Access"); Picker("Access", selection: $role) {
                    Text("View").tag(RemoteRole.viewer); Text("Edit").tag(RemoteRole.editor); Text("Operate").tag(RemoteRole.operator)
                }.labelsHidden() }
                GridRow { Text("Code lifetime"); Picker("Code lifetime", selection: $codeLifetime) {
                    Text("2 minutes").tag(120); Text("30 minutes").tag(1800); Text("1 hour").tag(3600)
                }.labelsHidden() }
            }.font(.system(size: 12))
            Text("Remember this browser in trusted LAN mode to keep access until forgotten or revoked.").font(.system(size: 11)).foregroundStyle(Theme.secondary)
            Text(role == .viewer ? "View the session and server status." : role == .editor ? "Edit and import media. Hardware transport requires Operate access." : "Edit, control audio, and manage server access and shutdown.")
                .font(.system(size: 11)).foregroundStyle(Theme.secondary)
            Button("Create pairing code") { issue() }.buttonStyle(RoomButtonStyle(prominent: true)).disabled(waiting || !remote.connected || name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            if let grant, let expires {
                SwiftUI.TimelineView(.periodic(from: .now, by: 1)) { context in
                    let seconds = max(0, Int(expires.timeIntervalSince(context.date).rounded(.up)))
                    VStack(alignment: .leading, spacing: 7) {
                        Text(seconds > 0 ? "One use · expires in \(seconds)s · \(grant.name)" : "Code expired").font(.system(size: 11)).foregroundStyle(Theme.secondary)
                        Text(grant.code).font(.system(size: 12, design: .monospaced)).textSelection(.enabled).opacity(seconds > 0 ? 1 : 0.45)
                    }.padding(12).frame(maxWidth: .infinity, alignment: .leading).background(Theme.background, in: RoundedRectangle(cornerRadius: 8))
                }
            }
            Divider().overlay(Theme.line)
            HStack { Text("Paired browsers").font(.system(size: 12, weight: .semibold)); Spacer(); Button("Refresh") { refresh() }.disabled(waiting) }
            List {
                if clients.isEmpty { Text("No paired browsers").font(.system(size: 11)).foregroundStyle(Theme.secondary) }
                ForEach(clients) { client in
                    HStack {
                        VStack(alignment: .leading, spacing: 3) {
                            Text(client.name).font(.system(size: 12)); Text(client.role.rawValue.capitalized).font(.system(size: 10)).foregroundStyle(Theme.secondary)
                        }
                        Spacer(); Button("Revoke", role: .destructive) { revoke(client) }.buttonStyle(.borderless).disabled(waiting).accessibilityLabel("Revoke \(client.name)")
                    }.listRowBackground(Theme.paper)
                }
            }.listStyle(.plain).scrollContentBackground(.hidden).frame(height: 180)
                .accessibilityLabel("Paired browser credentials")
            if let notice { Text(notice).font(.system(size: 11)).foregroundStyle(Color.orange).fixedSize(horizontal: false, vertical: true) }
        }.padding(22).frame(width: 540).background(Theme.paper).foregroundStyle(Theme.ink).preferredColorScheme(.dark).onAppear { refresh() }
    }
    private func issue() {
        waiting = true; notice = nil; grant = nil
        remote.send(.editor(.init(operation: "server.webPairing", integers: ["expiresInSeconds": codeLifetime], strings: ["name": name.trimmingCharacters(in: .whitespacesAndNewlines), "role": role.rawValue]))) { reply in
            waiting = false
            if reply.accepted, let payload = reply.payload, let value = try? JSONDecoder().decode(RemoteWebPairingGrant.self, from: payload) { grant = value; expires = Date().addingTimeInterval(Double(value.expiresInSeconds)) }
            else { notice = reply.message.isEmpty ? "The server could not create a pairing code." : reply.message }
        }
    }
    private func refresh() {
        waiting = true; notice = nil
        remote.send(.editor(.init(operation: "server.webClients"))) { reply in
            waiting = false
            if reply.accepted, let payload = reply.payload, let values = try? JSONDecoder().decode([RemoteWebClientRecord].self, from: payload) { clients = values.filter { $0.id != remote.client.credential.credentialID } }
            else { notice = reply.message }
        }
    }
    private func revoke(_ client: RemoteWebClientRecord) {
        guard client.id != remote.client.credential.credentialID else {
            notice = "This connection's credential cannot be revoked from Browser access."; return
        }
        waiting = true; notice = nil
        remote.send(.editor(.init(operation: "server.revokeWebCredential", strings: ["credentialID": client.id.uuidString]))) { reply in
            waiting = false
            if reply.accepted { clients.removeAll { $0.id == client.id } }
            else { notice = reply.message }
        }
    }
}

private struct RemoteConnectForm: View {
    @ObservedObject var current: RemoteGUIController
    let onConnected: () -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var host = ""
    @State private var port = String(RemoteProtocol.defaultPort)
    @State private var credentialFile: URL?
    @State private var error: String?
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Connect to an audio server").font(.system(size: 20, weight: .semibold))
            Grid(alignment: .leading, horizontalSpacing: 14, verticalSpacing: 12) {
                GridRow { Text("Hostname or IP"); TextField("studio-mac.local", text: $host).frame(width: 270) }
                GridRow { Text("Port"); TextField("49152", text: $port).frame(width: 85) }
            }.font(.system(size: 12)).textFieldStyle(.roundedBorder)
            HStack {
                Button("Choose pairing file…") { chooseCredential() }.buttonStyle(RoomButtonStyle())
                Text(credentialFile?.lastPathComponent ?? "No pairing file selected").font(.system(size: 11)).foregroundStyle(Theme.secondary).lineLimit(1)
            }
            Text("The other Mac owns the inputs and outputs. This window controls its session and does not use this computer’s audio devices.")
                .font(.system(size: 11)).foregroundStyle(Theme.secondary).fixedSize(horizontal: false, vertical: true)
            if let error { Text(error).font(.system(size: 11)).foregroundStyle(Color.orange).fixedSize(horizontal: false, vertical: true) }
            HStack {
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Spacer()
                Button("Connect") { connect() }.buttonStyle(RoomButtonStyle(prominent: true)).keyboardShortcut(.defaultAction)
                    .disabled(host.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || UInt16(port) == nil || credentialFile == nil)
            }
        }.padding(22).frame(width: 495).background(Theme.paper).foregroundStyle(Theme.ink).preferredColorScheme(.dark)
    }
    private func chooseCredential() {
        let panel = NSOpenPanel(); panel.allowedContentTypes = [.json]; panel.allowsMultipleSelection = false
        panel.canChooseFiles = true; panel.canChooseDirectories = false
        guard let window = NSApp.keyWindow else { return }
        panel.beginSheetModal(for: window) { result in if result == .OK { credentialFile = panel.url; error = nil } }
    }
    private func connect() {
        guard let file = credentialFile, let port = UInt16(port), port > 0, let store = current.store else { return }
        do {
            let credential = try RemoteCredential.load(from: file)
            let next = RemoteGUIController(store: store, credential: credential, host: host.trimmingCharacters(in: .whitespacesAndNewlines), port: port)
            current.disconnect(); store.remoteGUI = next; store.objectWillChange.send(); next.connect()
            dismiss(); onConnected()
        } catch { self.error = error.localizedDescription }
    }
}

struct RemoteProjectPicker: View {
    @ObservedObject var remote: RemoteGUIController
    @Environment(\.dismiss) private var dismiss
    @State private var selected: String?
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack { Text("Open a server project").font(.system(size: 20, weight: .semibold)); Spacer(); Button("Cancel") { dismiss() } }
            Text(remote.serverName).font(.system(size: 12)).foregroundStyle(Theme.secondary)
            if remote.projects.isEmpty {
                ContentUnavailableView("No saved projects", systemImage: "folder", description: Text("Import a project from this computer or save the current session on the server."))
            } else {
                List(remote.projects, id: \.self, selection: $selected) { name in
                    Label(name, systemImage: "waveform").tag(name)
                }.listStyle(.inset).frame(minHeight: 180, maxHeight: 280)
            }
            HStack {
                Button("Import from this computer…") { dismiss(); DispatchQueue.main.async { remote.importProjectFile() } }.buttonStyle(RoomButtonStyle())
                Spacer()
                Button("Open") { if let selected { remote.openServerProject(selected); dismiss() } }
                    .buttonStyle(RoomButtonStyle(prominent: true)).disabled(selected == nil).keyboardShortcut(.defaultAction)
            }
        }.padding(22).frame(width: 570, height: 380).background(Theme.paper).foregroundStyle(Theme.ink).preferredColorScheme(.dark)
    }
}

/// A closure boundary makes it impossible for a connected Designer to
/// accidentally prepare its own audio output or attach its own MIDI ports.
struct RemoteDesignerPerformance {
    let noteOn: (SoundDesignPattern, Int, Double, Double?) -> Void
    let noteOff: (Int) -> Void
    let controller: (SoundDesignControllerKind, Double) -> Void
    let allNotesOff: () -> Void
    let attachMIDI: (DesignerMIDIInput, SoundDesignPattern) -> Void
    let detachMIDI: () -> Void
    let updateMIDI: (SoundDesignPattern) -> Void
}

extension SessionStore {
    var remoteDesignerPerformance: RemoteDesignerPerformance {
        return .init(noteOn: { [weak self] pattern, pitch, velocity, beat in
            var numbers = ["velocity": velocity]; if let beat { numbers["beat"] = beat }
            var patch = pattern; patch.notes = []
            self?.remoteRoute("designer.noteOn", integers: ["pitch": pitch], numbers: numbers, payload: try? JSONEncoder().encode(patch))
        }, noteOff: { [weak self] pitch in
            self?.remoteRoute("designer.noteOff", integers: ["pitch": pitch])
        }, controller: { [weak self] kind, value in
            self?.remoteRoute("designer.controller", numbers: ["value": value], strings: ["kind": kind.rawValue])
        }, allNotesOff: { [weak self] in self?.remoteRoute("designer.allNotesOff") },
                     attachMIDI: { [weak self] midi, pattern in self?.remoteGUI?.attachMIDI(midi, pattern: pattern) }, detachMIDI: { [weak self] in self?.remoteGUI?.detachMIDI() }, updateMIDI: { [weak self] pattern in
            var patch = pattern; patch.notes = []
            self?.remoteRoute("designer.configure", payload: try? JSONEncoder().encode(patch))
        })
    }
}
