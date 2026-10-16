import XCTest
@testable import Soundroom
import SoundroomCore
import SoundroomRemote

final class TimelineViewportTests: XCTestCase {
    func testSelectionPrecisionFieldsRetainExactSampleBoundaries() {
        MainActor.assumeIsolated {
            let store = SessionStore()
            let track = store.session.tracks[0].id
            store.selection = EditSelection(trackID: track, range: 0..<5_000_000)
            store.selectedTrack = track
            for frame in [81_951, 234_146, 1_521_951] {
                store.setSelectionEdge(start: true, seconds: Double(frame) / sessionRate)
                XCTAssertEqual(store.selectedRange?.lowerBound, frame)
            }
        }
    }
    @MainActor func testLoopPrecisionIsSentToServerWithoutAnOptimisticLocalLoop() async throws {
        let store = SessionStore(headless: true), serverID = UUID()
        var commands: [RemoteCommand] = []
        let remote = RemoteGUIController(store: store, credential: .native(try RemoteCredential.generate()), host: "fixture", commandSender: { command, _, _, _ in
            commands.append(command); return .init(requestID: UUID(), revision: 0)
        })
        store.remoteGUI = remote
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .operator))
        defer { remote.disconnect() }
        for boundary in [81_951, 234_146, 1_521_951] {
            store.loopRange = 0..<5_000_000
            store.setLoopEdge(start: true, seconds: Double(boundary) / sessionRate)
            for _ in 0..<50 { await Task.yield() }
            XCTAssertTrue(commands.contains(.setLoop(start: boundary, end: 5_000_000)))
            XCTAssertEqual(store.loopRange, 0..<5_000_000, "Only server state confirms the shared loop")
        }
    }
    private func frame(_ seconds: Double) -> Int { Int(seconds * sessionRate) }
    func testVisibleCursorDoesNotScrollWhenSecondsChange() {
        let view = TimelineViewport(offset: 0, width: 800, contentWidth: 3000)
        for seconds in [0.0, 0.5, 1, 2, 5, 8] {
            XCTAssertNil(view.followDestination(frame: frame(seconds), zoom: 85, playing: true, enabled: true))
        }
    }
    func testFollowUsesViewportAndZoomAndLeavesContextAfterBoundary() {
        let view = TimelineViewport(offset: 400, width: 800, contentWidth: 3000)
        XCTAssertNil(view.followDestination(frame: frame(8), zoom: 100, playing: true, enabled: true))
        XCTAssertEqual(view.followDestination(frame: frame(12), zoom: 100, playing: true, enabled: true), 1040)
        XCTAssertEqual(view.followDestination(frame: frame(12), zoom: 200, playing: true, enabled: true), 2200)
        XCTAssertEqual(view.followDestination(frame: frame(3), zoom: 100, playing: true, enabled: true), 260)
        XCTAssertNil(view.followDestination(frame: frame(12), zoom: 100, playing: false, enabled: true))
        XCTAssertNil(view.followDestination(frame: frame(12), zoom: 100, playing: true, enabled: false))
    }
    func testEntirelyVisibleLoopStaysStationaryAtEitherMarginAndWrap() {
        let view = TimelineViewport(offset: 400, width: 800, contentWidth: 3000)
        let loop = frame(4.1)..<frame(11.9)
        for seconds in [4.1, 6, 11.89, 4.1] {
            XCTAssertNil(view.followDestination(frame: frame(seconds), zoom: 100, playing: true, enabled: true, loop: loop))
        }
        let initial = TimelineViewport(offset: 0, width: 800, contentWidth: 3000)
        let short = frame(0.5)..<frame(1.5)
        for seconds in [0.5, 1, 1.49, 0.5] {
            XCTAssertNil(initial.followDestination(frame: frame(seconds), zoom: 85, playing: true, enabled: true, loop: short))
        }
    }
    func testOffscreenShortLoopRevealsWholeRangeOnceAndLongLoopFollows() {
        let view = TimelineViewport(offset: 0, width: 800, contentWidth: 3000)
        let short = frame(10)..<frame(17.9)
        let desired = try! XCTUnwrap(view.followDestination(frame: frame(10), zoom: 100, playing: true, enabled: true, loop: short))
        XCTAssertEqual(desired, Double(short.upperBound) / sessionRate * 100 - 800, accuracy: 0.000001)
        XCTAssertLessThanOrEqual(desired, 1000)
        let revealed = TimelineViewport(offset: desired, width: 800, contentWidth: 3000)
        XCTAssertNil(revealed.followDestination(frame: frame(17.89), zoom: 100, playing: true, enabled: true, loop: short))
        XCTAssertNil(revealed.followDestination(frame: frame(10), zoom: 100, playing: true, enabled: true, loop: short))
        let long = frame(0)..<frame(25)
        XCTAssertNotNil(view.followDestination(frame: frame(8), zoom: 100, playing: true, enabled: true, loop: long))
        XCTAssertEqual(revealed.followDestination(frame: 0, zoom: 100, playing: true, enabled: true, loop: long), 0)
    }
    func testRevealClampsAtEdgesAndKeepsMarginAndFitDoesNotScroll() {
        let view = TimelineViewport(offset: 500, width: 800, contentWidth: 3000)
        XCTAssertEqual(view.revealDestination(frame: 0, zoom: 100), 0)
        XCTAssertEqual(view.revealDestination(frame: frame(10), zoom: 100), 960)
        XCTAssertEqual(view.revealDestination(frame: frame(60), zoom: 100), 2200)
        let fitted = TimelineViewport(offset: 0, width: 800, contentWidth: 800)
        XCTAssertNil(fitted.followDestination(frame: frame(19), zoom: 40, playing: true, enabled: true))
        XCTAssertEqual(fitted.revealDestination(frame: frame(19), zoom: 40), 0)
    }
    func testDragAutoScrollEdgesBothDirectionsAndCenterStaysStill() {
        let view = TimelineViewport(offset: 500, width: 800, contentWidth: 3000)
        XCTAssertNil(view.dragScrollDestination(pointerX: 400, elapsed: 1 / 30))
        XCTAssertLessThan(view.dragScrollDestination(pointerX: 10, elapsed: 1 / 30)!, 500)
        XCTAssertGreaterThan(view.dragScrollDestination(pointerX: 790, elapsed: 1 / 30)!, 500)
        XCTAssertEqual(view.dragScrollDestination(pointerX: -1000, elapsed: 1 / 30), 470)
        XCTAssertEqual(view.dragScrollDestination(pointerX: 1800, elapsed: 1 / 30), 530)
    }
    func testDragAutoScrollClampsContentAndElapsedAndRejectsInvalidGeometry() {
        let left = TimelineViewport(offset: 5, width: 800, contentWidth: 3000)
        XCTAssertEqual(left.dragScrollDestination(pointerX: -1000, elapsed: 1), 0)
        XCTAssertNil(TimelineViewport(offset: 0, width: 800, contentWidth: 3000).dragScrollDestination(pointerX: -20, elapsed: 1 / 30))
        let right = TimelineViewport(offset: 2195, width: 800, contentWidth: 3000)
        XCTAssertEqual(right.dragScrollDestination(pointerX: 1000, elapsed: 1 / 30), 2200)
        XCTAssertNil(TimelineViewport(offset: 0, width: 800, contentWidth: 800).dragScrollDestination(pointerX: 1000, elapsed: 1 / 30))
        XCTAssertNil(left.dragScrollDestination(pointerX: .nan, elapsed: 0.1))
        XCTAssertNil(left.dragScrollDestination(pointerX: 10, elapsed: 0))
        XCTAssertNil(TimelineViewport().dragScrollDestination(pointerX: 10, elapsed: 0.1))
    }
    func testRemoteTimelineRangeDragLeavesSharedTransportAndPlaybackClockAlone() {
        MainActor.assumeIsolated {
            let store = SessionStore()
            let track = store.session.tracks[0].id
            store.isPlaying = true
            store.playbackPosition.update(400_000)
            store.status = "Playing"
            for end in stride(from: 50_000, through: 500_000, by: 10_000) {
                store.selectTimelineDrag(track: track, start: 24_000, end: end)
            }
            XCTAssertEqual(store.selection, EditSelection(trackID: track, range: 24_000..<500_000))
            XCTAssertEqual(store.playhead, 24_000)
            XCTAssertTrue(store.isPlaying)
            XCTAssertEqual(store.playbackPosition.frame, 400_000)
            // With no remote connection, an accidental transport route changes
            // status to Connecting; selection updates must never route one.
            XCTAssertEqual(store.status, "Playing")
        }
    }
    func testRemoteTimelineRangeDragNormalizesReverseRangeAndHonorsRecordingGuard() {
        MainActor.assumeIsolated {
            let store = SessionStore()
            let track = store.session.tracks[0].id
            store.selectTimelineDrag(track: track, start: 50_000, end: -100)
            XCTAssertEqual(store.selection?.range, 0..<50_000)
            store.isRecording = true
            store.selectTimelineDrag(track: track, start: 10_000, end: 20_000)
            XCTAssertEqual(store.selection?.range, 0..<50_000)
        }
    }
    func testRemoteSelectionEdgeEditingLeavesSharedTransportAndClockAlone() {
        MainActor.assumeIsolated {
            let store = SessionStore()
            let track = store.session.tracks[0].id
            store.selectTimelineDrag(track: track, start: 24_000, end: 96_000)
            store.isPlaying = true; store.status = "Playing"
            store.playbackPosition.update(400_000)
            store.setSelectionEdge(start: false, seconds: 3)
            XCTAssertEqual(store.selection?.range, 24_000..<144_000)
            XCTAssertTrue(store.isPlaying)
            XCTAssertEqual(store.playbackPosition.frame, 400_000)
            XCTAssertEqual(store.status, "Playing")
        }
    }
    func testImmediateEdgeDragScrollsWithoutWaitingForHeldPointerTimer() {
        MainActor.assumeIsolated {
            let interaction = TimelineScrollInteraction()
            interaction.viewport = TimelineViewport(offset: 100, width: 800, contentWidth: 3000)
            var destinations: [Double] = [], dragPoints: [Double] = []
            interaction.requestScroll = { destinations.append($0) }
            interaction.pointerDown(at: 100)
            interaction.beginDrag { dragPoints.append($0) }
            interaction.updateDragPosition(contentX: 1900, at: 100.01)
            interaction.updateDragPosition(contentX: 1909, at: 100.02)
            interaction.endDrag()
            XCTAssertEqual(destinations.count, 2)
            XCTAssertEqual(destinations[0], 109, accuracy: 0.00001)
            XCTAssertEqual(destinations[1], 118, accuracy: 0.00001)
            XCTAssertEqual(dragPoints[0], 1909, accuracy: 0.00001)
            XCTAssertEqual(dragPoints[1], 1918, accuracy: 0.00001)
        }
    }
    func testPointerAndTimerScrollShareOneBoundedClockBudget() {
        var clock = TimelineDragScrollClock()
        clock.begin(at: 100)
        let firstPointer = clock.consume(at: 100.01)
        let heldTick = clock.consume(at: 100.02)
        let nextPointer = clock.consume(at: 100.03)
        XCTAssertEqual(firstPointer + heldTick + nextPointer, 0.03, accuracy: 0.000001)
        XCTAssertEqual(clock.consume(at: 100.03), 0)
        XCTAssertEqual(clock.consume(at: 100.02), 0)
        XCTAssertEqual(clock.consume(at: 110), 0.08)
        XCTAssertEqual(clock.consume(at: .nan), 0)
        clock.reset()
        XCTAssertEqual(clock.consume(at: 200), 0)
    }
    func testAccessibilityScrollIsDistinctFromRequestedScrollAndLayoutChanges() {
        let before = TimelineViewport(offset: 100, width: 800, contentWidth: 3000)
        let after = TimelineViewport(offset: 500, width: 800, contentWidth: 3000)
        XCTAssertTrue(after.isUnrequestedScroll(from: before, pendingDestination: nil))
        XCTAssertFalse(after.isUnrequestedScroll(from: before, pendingDestination: 500))
        XCTAssertFalse(TimelineViewport(offset: 500, width: 900, contentWidth: 3000).isUnrequestedScroll(from: before, pendingDestination: nil))
        XCTAssertFalse(TimelineViewport(offset: 500, width: 800, contentWidth: 6000).isUnrequestedScroll(from: before, pendingDestination: nil))
        XCTAssertFalse(TimelineViewport(offset: 101, width: 800, contentWidth: 3000).isUnrequestedScroll(from: before, pendingDestination: nil))
    }
    func testManualRevealSameFrameProducesDistinctRequestsAndFitUsesSamePath() {
        MainActor.assumeIsolated {
            let store = SessionStore()
            store.playhead = 24_000
            let initial = store.revealRequestSerial
            store.revealPlayhead()
            XCTAssertEqual(store.revealFrame, 24_000)
            XCTAssertEqual(store.revealRequestSerial, initial + 1)
            store.revealPlayhead()
            XCTAssertEqual(store.revealFrame, 24_000)
            XCTAssertEqual(store.revealRequestSerial, initial + 2)
            store.fitSession()
            XCTAssertEqual(store.revealFrame, 0)
            XCTAssertEqual(store.revealRequestSerial, initial + 3)
        }
    }
    @MainActor func testReturnToStartRevealsBeginningWithFollowOffInBothEngineModes() {
        do {
            let store = SessionStore(headless: true)
            store.followPlayhead = false
            store.zoom = 240
            store.playhead = frame(30)
            store.selection = EditSelection(trackID: store.session.tracks[0].id, range: frame(30)..<frame(31))
            store.requestTimelineReveal(store.playhead, onlyIfHidden: true)
            let initial = store.revealRequestSerial
            store.returnToStart()
            XCTAssertEqual(store.playhead, 0)
            XCTAssertNil(store.selection)
            XCTAssertEqual(store.revealFrame, 0)
            XCTAssertEqual(store.revealRequestSerial, initial + 1)
            XCTAssertFalse(store.revealOnlyIfHidden)
            XCTAssertFalse(store.followPlayhead)
            XCTAssertEqual(store.zoom, 240)
            let scrolled = TimelineViewport(offset: 1200, width: 800, contentWidth: 8000)
            XCTAssertEqual(scrolled.revealDestination(frame: store.revealFrame!, zoom: store.zoom), 0)
            // Scrolling away again while the cursor remains at zero must still
            // allow another click to reveal the beginning.
            store.returnToStart()
            XCTAssertEqual(store.revealRequestSerial, initial + 2)
            XCTAssertEqual(store.revealFrame, 0)
        }
    }
    @MainActor func testReturnWhileRemotePlaybackRevealsTheSeekDestination() async throws {
        let store = SessionStore(headless: true), serverID = UUID()
        var seeks: [Int] = []
        let remote = RemoteGUIController(store: store, credential: .native(try RemoteCredential.generate()), host: "fixture", commandSender: { command, _, _, _ in
            guard case .seek(let frame) = command else { throw RemoteError.invalidFrame }
            seeks.append(frame)
            return .init(requestID: UUID(), revision: 0)
        })
        store.remoteGUI = remote
        remote.receiveState(.connected(serverID: serverID, name: "Fixture", role: .operator))
        defer { remote.disconnect() }
        store.playhead = frame(12)
        store.playbackPosition.update(frame(40))
        store.isPlaying = true
        store.followPlayhead = false
        let initial = store.revealRequestSerial
        store.returnToStart()
        XCTAssertEqual(store.revealFrame, frame(12))
        XCTAssertEqual(store.revealRequestSerial, initial + 1)
        XCTAssertTrue(store.isPlaying)
        XCTAssertFalse(store.followPlayhead)
        XCTAssertEqual(store.playhead, frame(12))
        for _ in 0..<50 { await Task.yield() }
        XCTAssertEqual(seeks, [frame(12)])
    }
    @MainActor func testReturnToStartDoesNotNavigateDuringRecording() {
        do {
            let store = SessionStore(headless: true)
            store.playhead = frame(30)
            let selection = EditSelection(trackID: store.session.tracks[0].id, range: frame(30)..<frame(31))
            store.selection = selection
            store.isRecording = true
            let initial = store.revealRequestSerial
            store.returnToStart()
            XCTAssertEqual(store.playhead, frame(30))
            XCTAssertEqual(store.selection, selection)
            XCTAssertEqual(store.revealRequestSerial, initial)
            XCTAssertNil(store.revealFrame)
        }
    }
    func testZoomToLongSelectionFitsWholeRangeWithRevealMargin() {
        MainActor.assumeIsolated {
            let store = SessionStore(), asset = UUID()
            let track = store.session.tracks[0].id
            store.session.tracks[0].clips = [Clip(assetID: asset, name: "Long", start: 0, length: frame(80), edgeFadeFrames: 0)]
            store.selectedTrack = track
            let range = frame(10)..<frame(70)
            store.selection = EditSelection(trackID: track, range: range)
            store.visibleTimelineWidth = 800
            store.zoomToSelection()
            let view = TimelineViewport(offset: 0, width: 800, contentWidth: store.timelineSeconds * store.zoom)
            let offset = view.revealDestination(frame: store.revealFrame!, zoom: store.zoom)
            let start = Double(range.lowerBound) / sessionRate * store.zoom
            let end = Double(range.upperBound) / sessionRate * store.zoom
            XCTAssertGreaterThanOrEqual(start, offset)
            XCTAssertLessThanOrEqual(end, offset + 800)
            XCTAssertEqual(start - offset, 40, accuracy: 0.000001)
        }
    }
}
