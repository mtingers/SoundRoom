import Foundation

import Combine
import SoundroomCore

@MainActor final class TakeAudition: ObservableObject {
    @Published private(set) var playing = false
    @Published private(set) var preparing = false
    @Published private(set) var error: String?
    private weak var remoteStore: SessionStore?
    private var generation = 0
    func stop() {
        generation += 1; _ = remoteStore?.remoteRoute("take.stop"); remoteStore = nil
        preparing = false; playing = false
    }
    func toggle(store: SessionStore, takeID: UUID?, range: Range<Int>?) {
        if playing || preparing { stop(); return }
        guard let track = store.currentTrack, let take = track.recordingTakes.first(where: { $0.id == takeID }) ?? track.recordingTakes.first,
              let full = take.range else { return }
        let chosen = range?.isEmpty == false ? range! : full
        let start = max(full.lowerBound, chosen.lowerBound), end = min(full.upperBound, chosen.upperBound)
        guard end > start else { return }


        guard let remote = store.remoteGUI else { error = "Connect to the audio server before previewing."; return }
        remoteStore = store
        generation += 1; let token = generation
        preparing = true; playing = false; error = nil
        remote.route(.init(operation: "take.preview", context: store.remoteContext, integers: ["start": start, "end": end], strings: ["takeID": take.id.uuidString])) { [weak self] reply in
            guard let self, token == generation else { return }
            preparing = false; playing = reply.accepted
            if !reply.accepted { error = reply.message; remoteStore = nil }
        }
        return
    }
}
