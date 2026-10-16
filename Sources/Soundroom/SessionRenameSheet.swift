import SwiftUI
import SoundroomCore
import SoundroomRemote

@MainActor final class SessionRenameDraft: ObservableObject, Identifiable {
    let id = UUID()
    @Published var name: String
    @Published private(set) var pending = false
    @Published private(set) var message = ""
    private weak var store: SessionStore?
    private weak var remote: RemoteGUIController?
    private let generation: Int?
    private let serverRevision: Int?
    init(store: SessionStore) {
        self.store = store; name = store.session.name
        remote = store.remoteGUI; generation = remote?.connectionGeneration; serverRevision = remote?.revision
    }
    var canSubmit: Bool { !pending && (try? SessionName.validated(name)) != nil && store?.canRenameSession == true }
    func submit() {
        guard !pending, let store else { return }
        let title: String
        do { title = try SessionName.validated(name) }
        catch { message = error.localizedDescription; return }
        guard store.canRenameSession else { message = "Finish recording or pending work before renaming."; return }
        guard let remote, store.remoteGUI === remote, remote.connected,
              remote.connectionGeneration == generation, remote.revision == serverRevision else {
            message = "The session changed. Your name is retained; reopen Rename Session for the current session."; return
        }
        pending = true
        remote.send(.editor(.init(operation: "session.rename", strings: ["name": title])), expectedRevision: serverRevision) { [weak self] reply in
            guard let self else { return }; pending = false
            if reply.accepted { store.sessionRenameDraft = nil; store.focusEditor() }
            else { message = reply.message.isEmpty ? "The name was not changed. Your text is retained." : reply.message }
        }
    }
}
extension SessionStore {
    var canRenameSession: Bool {
        guard !busy, !isRecording else { return false }
        guard let remoteGUI, case .connected(_, _, let role) = remoteGUI.state else { return false }
        return role == .editor || role == .operator
    }
    func requestSessionRename() {
        guard canRenameSession else { return }
        endContinuousEdit(); sessionRenameDraft = SessionRenameDraft(store: self)
    }
}
struct SessionRenameSheet: View {
    @ObservedObject var store: SessionStore
    @ObservedObject var draft: SessionRenameDraft
    @FocusState private var nameFocused: Bool
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Rename Session").font(.system(size: 20, weight: .semibold))
            TextField("Session name", text: $draft.name).textFieldStyle(.roundedBorder)
                .focused($nameFocused).disabled(draft.pending).onSubmit { draft.submit() }
            Text("Changes the title shown to all connected clients. The existing project folder keeps its filename; Save As stores a named project copy.")
                .font(.system(size: 12)).foregroundStyle(Theme.secondary).fixedSize(horizontal: false, vertical: true)
            if !draft.message.isEmpty { Text(draft.message).font(.system(size: 12)).foregroundStyle(Theme.secondary) }
            HStack {
                Spacer()
                Button("Cancel") { store.sessionRenameDraft = nil }.keyboardShortcut(.cancelAction).disabled(draft.pending)
                Button(draft.pending ? "Renaming…" : "Rename") { draft.submit() }.keyboardShortcut(.defaultAction).disabled(!draft.canSubmit)
            }
        }.padding(22).frame(width: 420).interactiveDismissDisabled(draft.pending)
            .onAppear { nameFocused = true }
    }
}
