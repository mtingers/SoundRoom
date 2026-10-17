import SoundroomCore
import XCTest

@testable import Soundroom

/// Contract checks for the precision workspace. Native sheet/keyboard behavior
/// is qualified separately; these checks exercise the same pinned identities
/// and project transactions used by its controls without opening audio hardware.
final class EffectPrecisionAcceptanceTests: XCTestCase {
    func testSelectionControlsRejectUnknownAndBlockedEditsAndClampPhysicalValues() {
        MainActor.assumeIsolated {
            let store = SessionStore()
            store.chooseEffect(.echo)
            store.effectParameters = [:]
            store.amount = 0.75
            store.setSelectionParameter("unknown", value: 500)
            XCTAssertTrue(store.effectParameters.isEmpty, "An invalid key must not even upgrade the legacy path")
            store.busy = true
            store.setSelectionParameter("time", value: 800)
            store.enableSelectionParameterControls()
            XCTAssertTrue(store.effectParameters.isEmpty)
            store.busy = false
            store.isRecording = true
            store.setSelectionParameter("time", value: 800)
            store.enableSelectionParameterControls()
            XCTAssertTrue(store.effectParameters.isEmpty)
            store.isRecording = false
            store.setSelectionParameter("time", value: 100_000)
            XCTAssertEqual(store.effectParameters["time"], 2000)
            store.setSelectionParameter("feedback", value: -1)
            XCTAssertEqual(store.effectParameters["feedback"], 0)
            store.setSelectionParameter("mix", value: .nan)
            XCTAssertEqual(store.effectParameters["mix"], 0.25)
            store.chooseEffect(.reverse)
            store.setSelectionParameter("mix", value: 0.5)
            store.enableSelectionParameterControls()
            XCTAssertTrue(store.effectParameters.isEmpty)
        }
    }

    func testSelectionWorkspaceRejectsChangedTrackRangeKindAndMode() {
        let track = UUID()
        let other = UUID()
        let range = 1200..<48_000
        let target = EffectWorkspaceTarget.selection(trackID: track, kind: .echo, range: range)
        func matches(
            _ mode: EffectMode = .selection, _ trackID: UUID? = nil,
            _ kind: BuiltInEffect = .echo, _ selection: Range<Int>? = nil
        ) -> Bool {
            target.matches(
                mode: mode, selectedTrackID: trackID ?? track, selectionKind: kind,
                selectionRange: selection ?? range, effects: [])
        }
        XCTAssertTrue(matches())
        XCTAssertFalse(matches(.realtime))
        XCTAssertFalse(matches(.selection, other))
        XCTAssertFalse(matches(.selection, track, .room))
        XCTAssertFalse(matches(.selection, track, .echo, 1201..<48_000))
        XCTAssertFalse(
            target.matches(
                mode: .selection, selectedTrackID: nil, selectionKind: .echo, selectionRange: range, effects: []))
        XCTAssertFalse(
            target.matches(
                mode: .selection, selectedTrackID: track, selectionKind: .echo, selectionRange: nil, effects: []))
    }

    func testRealtimeWorkspacePinsIdentityButAcceptsFreshParametersAndUndoValues() {
        let track = UUID()
        let other = UUID()
        var effect = RealtimeEffect(kind: .echo, parameters: RealtimeEffectKind.echo.defaultParameters)
        let target = EffectWorkspaceTarget.realtime(trackID: track, effectID: effect.id, kind: .echo)
        func matches(_ effects: [RealtimeEffect], mode: EffectMode = .realtime, trackID: UUID? = nil) -> Bool {
            target.matches(
                mode: mode, selectedTrackID: trackID ?? track, selectionKind: .reverse,
                selectionRange: nil, effects: effects)
        }
        XCTAssertTrue(matches([effect]))
        let original = effect
        effect.parameters["time"] = 845
        effect.enabled = false
        effect.parameterAutomation["mix"] = [.init(frame: 3200, value: 0.9)]
        XCTAssertTrue(matches([effect]), "Pinned editing follows current data rather than a sheet-opening snapshot")
        XCTAssertTrue(matches([original]), "Undo may restore prior values for the same identity")
        XCTAssertFalse(matches([effect], mode: .selection))
        XCTAssertFalse(matches([effect], trackID: other))
        XCTAssertFalse(matches([]))
        var replacement = RealtimeEffect(kind: .room, parameters: RealtimeEffectKind.room.defaultParameters)
        replacement.id = effect.id
        XCTAssertFalse(matches([replacement]), "ID reuse cannot route old Echo controls to a Room insert")
        replacement = RealtimeEffect(kind: .echo, parameters: RealtimeEffectKind.echo.defaultParameters)
        XCTAssertFalse(matches([replacement]), "A different insert of the same kind is not the pinned target")
    }

    @MainActor

    private func assertExactAudio(
        _ actual: AudioSamples, _ expected: AudioSamples, _ message: String,
        file: StaticString = #filePath, line: UInt = #line
    ) {
        XCTAssertEqual(actual.count, expected.count, message, file: file, line: line)
        XCTAssertEqual(actual.channelCount, expected.channelCount, message, file: file, line: line)
        guard actual.count == expected.count, actual.channelCount == expected.channelCount else { return }
        var maximumError: Float = 0
        var firstMismatch: String?
        for channel in 0..<actual.channelCount {
            for frame in 0..<actual.count {
                let a = actual.sample(channel: channel, frame: frame)
                let e = expected.sample(channel: channel, frame: frame)
                if a != e {
                    if firstMismatch == nil {
                        firstMismatch = "channel\(channel), frame\(frame), actual\(a), expected\(e)"
                    }
                    maximumError = max(maximumError, abs(a - e))
                }
            }
        }
        XCTAssertNil(
            firstMismatch, "\(message); first \(firstMismatch ?? "none"), maximum error \(maximumError)", file: file,
            line: line)
    }
}
