import XCTest
import SoundroomCore
@testable import Soundroom

final class EffectDestinationTests: XCTestCase {
    @MainActor func testSelectingMasterAndTracksRoutesInsertsAndEditsToActualSelection() {
        let store = SessionStore(headless: true)
        let trackID = store.session.tracks[0].id
        store.choose(track: Session.masterTrackID)
        XCTAssertEqual(store.effectMode, .master)
        store.addRealtimeEffect(.compressor)
        let masterEffect = store.currentRealtimeEffect!
        XCTAssertEqual(store.session.masterTrack.realtimeEffects.map(\.id), [masterEffect.id])
        XCTAssertTrue(store.session.tracks[0].realtimeEffects.isEmpty)

        store.choose(track: trackID)
        XCTAssertEqual(store.effectMode, .realtime)
        XCTAssertNil(store.selectedRealtimeEffect)
        store.addRealtimeEffect(.vocalEnhancer)
        let trackEffect = store.currentRealtimeEffect!
        XCTAssertEqual(store.session.tracks[0].realtimeEffects.map(\.id), [trackEffect.id])
        store.choose(track: Session.masterTrackID)
        XCTAssertEqual(store.currentRealtimeEffect?.id, masterEffect.id)
        store.changeRealtimeEffect(masterEffect.id, name: "Bypass") { $0.enabled = false }
        XCTAssertFalse(store.session.masterTrack.realtimeEffects[0].enabled)
        XCTAssertTrue(store.session.tracks[0].realtimeEffects[0].enabled)

        // Remote snapshot/Undo restoration and other direct assignments share
        // the same destination rule as clicking the track header.
        store.selectedTrack = trackID
        XCTAssertEqual(store.effectMode, .realtime)
        XCTAssertEqual(store.currentRealtimeEffect?.id, trackEffect.id)
    }

    @MainActor func testBlockedSelectionDoesNotChangeDestinationOrAutomationFocus() {
        let store = SessionStore(headless: true)
        let trackID = store.selectedTrack
        store.automationEditing.hasFocus = true
        store.busy = true
        store.choose(track: Session.masterTrackID)
        XCTAssertEqual(store.selectedTrack, trackID)
        XCTAssertEqual(store.effectMode, .realtime)
        XCTAssertTrue(store.automationEditing.hasFocus)
        store.busy = false
        store.choose(track: UUID())
        XCTAssertEqual(store.selectedTrack, trackID)
        XCTAssertEqual(store.effectMode, .realtime)
    }

    @MainActor func testAudioEditDismissalAndMasterAutomationUseSelectedDestination() {
        let store = SessionStore(headless: true)
        let trackID = store.session.tracks[0].id
        store.effectMode = .selection
        store.restoreLiveEffectDestination()
        XCTAssertEqual(store.effectMode, .realtime)
        store.automationTarget = .pan
        store.choose(track: Session.masterTrackID)
        XCTAssertEqual(store.automationTarget, .volume)
        store.effectMode = .selection
        store.restoreLiveEffectDestination()
        XCTAssertEqual(store.effectMode, .master)
        store.select(track: trackID, start: 0, end: 48_000)
        XCTAssertEqual(store.effectMode, .realtime)
        XCTAssertEqual(store.selectedRange, 0..<48_000)
    }

    func testOutputMeterReportsSilenceDecibelsAndClipping() {
        XCTAssertEqual(OutputLevelReadout.spokenLevel(0), "Silent")
        XCTAssertEqual(OutputLevelReadout.spokenLevel(0.5), "-6 decibels")
        XCTAssertEqual(OutputLevelReadout.spokenLevel(1), "Clipping")
        XCTAssertEqual(OutputLevelReadout.spokenLevel(1.2), "Clipping")
    }
}
