import XCTest
import SoundroomCore
@testable import Soundroom

final class TrackRoutingEditingTests: XCTestCase {

    func testReturnIsWetAndFeedbackEditsAreRejectedBeforeCommit() {
        MainActor.assumeIsolated {
            let store = SessionStore(); store.addMixTrack(role:"return")
            let track = store.currentTrack!
            XCTAssertFalse(track.acceptsClips); XCTAssertEqual(track.routing?.role,"return")
            XCTAssertEqual(track.realtimeEffects.first?.parameters["mix"],1)
            let before = store.session
            store.setRouting(track.id) { $0.sidechainSourceID = track.id }
            XCTAssertEqual(store.session,before); XCTAssertTrue(store.status.contains("feedback loop"))
        }
    }
    func testClipMovesCannotPlaceAudioOnBusTracks() {
        MainActor.assumeIsolated {
            let store = SessionStore(), asset = UUID()
            var track = Track(name:"Audio",color:0); track.clips = [Clip(assetID:asset,name:"Signal",start:0,length:4800)]
            store.assets[asset] = AudioSamples(channels:[.init(repeating:0.1,count:4800)])
            store.session.tracks = [track]; store.choose(track:track.id); store.selectedClips = [track.clips[0].id]
            store.addMixTrack(role:"bus"); let group = store.selectedTrack!
            store.choose(track:track.id); store.selectedClips = [track.clips[0].id]
            let before = store.session
            store.moveSelectedClips(primary:track.clips[0].id,to:200,targetTrack:group)
            XCTAssertEqual(store.session,before); XCTAssertTrue(store.status.contains("audio or instrument track"))
        }
    }
}
