import AppKit
import SoundroomCore
import XCTest

@testable import Soundroom

final class PitchPointEditingTests: XCTestCase {
    func testClickedInsertionKeepsBothCoordinatesInsteadOfFollowingExistingCurve() throws {
        let original = region()
        var editing = PitchPointEditing()
        editing.select(original.points[1].id, extending: false)
        editing.placeInsertion(frame: 5500, note: 64.25)
        XCTAssertTrue(editing.selected.isEmpty)
        let target = editing.insertionTarget(in: [original], defaultFrame: 9000)
        XCTAssertEqual(target.frame, 5500)
        XCTAssertEqual(target.note, 64.25)
        XCTAssertNotEqual(target.note, original.note(at: target.frame))
        // Panning or moving the cursor toward Add must not change the chosen location.
        XCTAssertEqual(editing.insertionTarget(in: [original], defaultFrame: 1000).frame, 5500)
        let result = editing.adding(to: [original], frame: target.frame, note: target.note)
        let point = try XCTUnwrap(result.regions[0].points.first { $0.id == result.point })
        XCTAssertEqual(point.frame, 5500)
        XCTAssertEqual(point.note, 64.25)
    }
    func testAnchorAndRangeSelectionClearOldInsertionLocation() {
        let original = region()
        var editing = PitchPointEditing()
        editing.placeInsertion(frame: 5000, note: 64)
        editing.select(original.points[1].id, extending: false)
        XCTAssertNil(editing.insertionFrame)
        XCTAssertNil(editing.insertionNote)
        XCTAssertEqual(editing.insertionTarget(in: [original], defaultFrame: 5000).frame, original.points[1].frame)
        editing.placeInsertion(frame: 5000, note: 64)
        editing.select(1000..<4000, regions: [original])
        XCTAssertNil(editing.insertionFrame)
        XCTAssertNil(editing.insertionNote)
    }
    func testClickedGapDoesNotAddAPointOrExtendVoicedRegions() {
        var editing = PitchPointEditing()
        editing.placeInsertion(frame: 15000, note: 65)
        let original = [region(), region(20000, 30000)]
        let target = editing.insertionTarget(in: original, defaultFrame: 5000)
        let result = editing.adding(to: original, frame: target.frame, note: target.note)
        XCTAssertNil(result.point)
        XCTAssertEqual(result.regions, original)
    }
    private func region(_ start: Int = 1000, _ end: Int = 10_000) -> PitchEditRegion {
        .init(
            startFrame: start, endFrame: end,
            points: [
                .init(frame: start, note: 69.1), .init(frame: start + 2000, note: 69.8),
                .init(frame: end - 2000, note: 71.2), .init(frame: end, note: 72.1),
            ])
    }
    func testShiftSelectionAndStableIDsSurviveMovement() {
        let original = region()
        var editing = PitchPointEditing()
        let a = original.points[1].id
        let b = original.points[2].id
        editing.select(a, extending: false)
        editing.select(b, extending: true)
        XCTAssertEqual(editing.selected, [a, b])
        let moved = editing.moving([original], frames: 500, notes: 0.15)
        XCTAssertEqual(moved[0].points[1].id, a)
        XCTAssertEqual(moved[0].points[2].id, b)
        XCTAssertEqual(moved[0].points[1].frame, original.points[1].frame + 500)
        XCTAssertEqual(moved[0].points[1].note, original.points[1].note + 0.15, accuracy: 0.00001)
        XCTAssertEqual(moved[0].points[0], original.points[0])
        editing.reconcile(moved)
        XCTAssertEqual(editing.selected, [a, b])
        editing.select(a, extending: true)
        XCTAssertEqual(editing.selected, [b])
    }
    func testDragUpdatesUseOriginalSnapshotAndCannotCrossOtherAnchors() {
        let original = region()
        var editing = PitchPointEditing()
        editing.select(original.points[1].id, extending: false)
        let first = editing.moving([original], frames: 400, notes: 0.1)
        let second = editing.moving([original], frames: 900, notes: 0.2)
        XCTAssertEqual(second[0].points[1].frame, original.points[1].frame + 900)
        XCTAssertNotEqual(second, editing.moving(first, frames: 900, notes: 0.2))
        let right = editing.moving([original], frames: 50_000, notes: 0)
        XCTAssertEqual(right[0].points[1].frame, original.points[2].frame - 1)
        let left = editing.moving([original], frames: -50_000, notes: 0)
        XCTAssertEqual(left[0].points[1].frame, original.points[0].frame + 1)
    }
    func testGroupMovementRespectsRegionBoundariesAndOptionalSnap() {
        let original = region()
        var editing = PitchPointEditing()
        editing.select(1000..<10_001, regions: [original])
        let moved = editing.moving([original], frames: 10_000, notes: 0.2)
        XCTAssertEqual(
            moved[0].points.map(\.frame), original.points.map(\.frame),
            "End-boundary anchors must not shift on pitch edits")
        editing.select(original.points[1].id, extending: false)
        // A plain click on an already selected point preserves group selection; make a new single selection.
        editing = PitchPointEditing()
        editing.select(original.points[1].id, extending: false)
        let free = editing.moving([original], frames: 0, notes: 0.13)
        XCTAssertEqual(free[0].points[1].note, 69.93, accuracy: 0.00001)
        let snapped = editing.moving([original], frames: 0, notes: 0.13, snap: true)
        XCTAssertEqual(snapped[0].points[1].note, 70, accuracy: 0.00001)
        let invalid = editing.moving([original], frames: 0, notes: .nan)
        XCTAssertEqual(invalid, [original])
    }
    func testCurveAndLocalSmoothingLeaveOtherPointsAndGapsUntouched() {
        let a = region()
        let b = region(20_000, 30_000)
        var editing = PitchPointEditing()
        editing.select(a.points[1].id, extending: false)
        var changed = editing.settingCurve(.hold, in: [a, b])
        XCTAssertEqual(changed[0].points[1].curve, .hold)
        XCTAssertEqual(changed[0].points[0], a.points[0])
        XCTAssertEqual(changed[1], b)
        changed[0].points[1].note = 80
        let smoothed = editing.smoothing(changed, amount: 1)
        XCTAssertLessThan(smoothed[0].points[1].note, 80)
        XCTAssertEqual(smoothed[0].points.map(\.frame), a.points.map(\.frame))
        XCTAssertEqual(smoothed[0].points[0], changed[0].points[0])
        XCTAssertEqual(smoothed[0].points.last, changed[0].points.last)
        XCTAssertEqual(smoothed[1], b)
        XCTAssertNil(smoothed[0].note(at: 15_000))
    }
    func testAddDeleteAndRangeSelectionCannotBridgeUnvoicedGap() {
        let a = region()
        let b = region(20_000, 30_000)
        var editing = PitchPointEditing()
        let added = editing.adding(to: [a, b], frame: 5000, note: 70.25)
        let id = try! XCTUnwrap(added.point)
        XCTAssertEqual(added.regions[0].points.count, a.points.count + 1)
        XCTAssertEqual(added.regions[0].points.first { $0.id == id }?.note, 70.25)
        XCTAssertNil(editing.adding(to: [a, b], frame: 15_000, note: 69).point)
        XCTAssertNil(editing.adding(to: added.regions, frame: 5000, note: 69).point)
        editing.select(2500..<6000, regions: added.regions)
        XCTAssertEqual(editing.selected.count, 2)
        let deleted = editing.deleting(added.regions)
        XCTAssertEqual(deleted[0].points.count, 3)
        XCTAssertEqual(deleted[1], b)
        editing.reconcile(deleted)
        XCTAssertTrue(editing.selected.isEmpty)
    }
    private func point(_ frame: Int, note: Float, confidence: Float = 0.99) -> PitchContourPoint {
        .init(frame: frame, frequency: 440 * pow(2, (note - 69) / 12), confidence: confidence)
    }
    func testMeasuredSeedingPreservesExplicitUnvoicedGapsAndPitch() {
        let contour = [
            point(2000, note: 69.2), point(2256, note: 69.3), point(2512, note: 69.4),
            point(2768, note: 69, confidence: 0), point(3024, note: 71.1), point(3280, note: 71.2),
        ]
        let seeds = PitchPointTargets.seed(
            contour: contour, scope: 1000..<4000, reference: 440,
            preset: .performance, toleranceCents: 0)
        XCTAssertEqual(seeds.count, 2)
        XCTAssertEqual(seeds[0].points[0].note, 69.2, accuracy: 0.001)
        XCTAssertEqual(seeds[0].points.count, 3)
        XCTAssertNil(seeds[0].note(at: 2768))
        XCTAssertNil(seeds[1].note(at: 2768))
        XCTAssertTrue(seeds.allSatisfy { $0.startFrame >= 1000 && $0.endFrame <= 4000 })
    }
    func testCurvePresetsChangeTargetsAndPreserveMeasuredSource() {
        let contour = [
            point(2000, note: 69.2), point(12_000, note: 69.3), point(22_000, note: 70.3), point(32_000, note: 70.2),
        ]
        let natural = PitchPointTargets.seed(
            contour: contour, scope: 1000..<34_000, reference: 440, preset: .natural, toleranceCents: 0)
        let tight = PitchPointTargets.seed(
            contour: contour, scope: 1000..<34_000, reference: 440, preset: .tight, toleranceCents: 0)
        let robot = PitchPointTargets.seed(
            contour: contour, scope: 1000..<34_000, reference: 440, preset: .robotic, toleranceCents: 0)
        XCTAssertEqual(natural[0].points[0].note, 69.1, accuracy: 0.001)
        XCTAssertEqual(natural[0].points[0].curve, .smooth)
        XCTAssertEqual(tight[0].points[0].note, 69)
        XCTAssertTrue(robot.flatMap(\.points).allSatisfy { $0.curve == .hold && $0.note.rounded() == $0.note })
        XCTAssertEqual(
            robot[0].points.first { $0.note == 70 }?.frame, 22_000, "Held steps must keep the measured transition time")
        XCTAssertEqual(contour[0].frequency, point(2000, note: 69.2).frequency)
        XCTAssertTrue(
            PitchPointTargets.seed(contour: [point(2000, note: 69, confidence: 0)], scope: 1000..<4000, reference: 440)
                .isEmpty)
    }
    func testSimplificationHonorsCentToleranceAndKeepsEndpoints() {
        let points = [
            PitchEditPoint(frame: 0, note: 69), .init(frame: 1000, note: 69.02), .init(frame: 2000, note: 69),
        ]
        let reduced = PitchPointTargets.simplify(points, toleranceCents: 5)
        XCTAssertEqual(reduced, [points[0], points[2]])
        XCTAssertEqual(PitchPointTargets.simplify(points, toleranceCents: 1), points)
        XCTAssertEqual(PitchPointTargets.simplify(points, toleranceCents: .nan), points)
    }
    func testTargetPresetsRespectKeyScaleAndAbsoluteCustomMask() {
        let cMajor = PitchPointTargets.allowedPitchClasses(parameters: ["scale": 1, "key": 0])
        let dMajor = PitchPointTargets.allowedPitchClasses(parameters: ["scale": 1, "key": 2])
        XCTAssertEqual(PitchPointTargets.nearestAllowed(66.1, mask: cMajor), 67)
        XCTAssertEqual(PitchPointTargets.nearestAllowed(66.1, mask: dMajor), 66)
        let custom = PitchPointTargets.allowedPitchClasses(parameters: ["scale": 3, "key": 7, "mask": 1])
        XCTAssertEqual(custom, 1, "Custom note targets are absolute and never transposed by Key")
        XCTAssertEqual(PitchPointTargets.nearestAllowed(69.3, mask: custom), 72)
        let contour = [point(2000, note: 66.1), point(2256, note: 66.2)]
        let tight = PitchPointTargets.seed(
            contour: contour, scope: 1000..<4000, reference: 440,
            preset: .tight, toleranceCents: 0, allowedPitchClasses: dMajor)
        XCTAssertTrue(tight.flatMap(\.points).allSatisfy { dMajor & (1 << (Int($0.note) % 12)) != 0 })
        let natural = PitchPointTargets.seed(
            contour: contour, scope: 1000..<4000, reference: 440,
            preset: .natural, toleranceCents: 0, allowedPitchClasses: cMajor)
        XCTAssertEqual(natural[0].points[0].note, 66.55, accuracy: 0.001)
    }
    func testNoAllowedNotesPreservesPerformanceAndMalformedParametersStaySafe() {
        let contour = [point(2000, note: 69.3), point(2256, note: 69.4)]
        let targets = PitchPointTargets.seed(
            contour: contour, scope: 1000..<4000, reference: 440,
            preset: .robotic, toleranceCents: 0, allowedPitchClasses: 0)
        XCTAssertEqual(targets[0].points[0].note, 69.3, accuracy: 0.001)
        XCTAssertTrue(targets.flatMap(\.points).allSatisfy { $0.curve == .linear })
        XCTAssertEqual(PitchPointTargets.allowedPitchClasses(parameters: ["scale": .nan, "key": .infinity]), 4095)
        XCTAssertEqual(PitchPointTargets.allowedPitchClasses(parameters: ["scale": 3, "mask": .nan]), 4095)
        XCTAssertEqual(PitchPointTargets.allowedPitchClasses(parameters: ["scale": 3, "mask": 0]), 0)
    }
    func testOffsetBoundaryVibratoDoesNotProduceSemitoneChatter() {
        for frequency in [5.0, 5.5, 6.0] {
            for center in [Float(69.39), 69.48, 69.55, 69.61] {
                let contour = (0..<300).map { index in
                    point(
                        index * 480, note: center + Float(0.32 * sin(2 * Double.pi * frequency * Double(index) / 100)))
                }
                for preset in [PitchCurvePreset.tight, .robotic] {
                    let targets = PitchPointTargets.seed(
                        contour: contour, scope: 0..<144_000, reference: 440,
                        preset: preset, toleranceCents: 0)
                    XCTAssertEqual(
                        Set(targets.flatMap(\.points).map(\.note)).count, 1,
                        "\(frequency) Hz vibrato around \(center) caused \(preset.rawValue) note chatter")
                }
            }
        }
    }
    func testBriefCrossingIsRejectedButSustainedScaleStepKeepsOriginalOnset() {
        let contour = (0..<140).map { index in
            point(index * 480, note: index < 40 ? 69.2 : index < 47 ? 70.2 : index < 65 ? 69.2 : 71.2)
        }
        let mask = PitchPointTargets.allowedPitchClasses(parameters: ["scale": 1, "key": 0])
        let targets = PitchPointTargets.seed(
            contour: contour, scope: 0..<67_200, reference: 440,
            preset: .robotic, toleranceCents: 0, allowedPitchClasses: mask)
        XCTAssertEqual(Set(targets.flatMap(\.points).map(\.note)), [69, 71])
        XCTAssertEqual(
            targets[0].points.first { $0.note == 71 }?.frame, 65 * 480,
            "Offline confirmation must preserve the original sustained transition onset")
        XCTAssertEqual(targets[0].note(at: 43 * 480), 69)
        let natural = PitchPointTargets.seed(
            contour: contour, scope: 0..<67_200, reference: 440,
            preset: .natural, toleranceCents: 0, allowedPitchClasses: mask)
        XCTAssertEqual(natural[0].note(at: 43 * 480) ?? .nan, 69.6, accuracy: 0.001)
        let performance = PitchPointTargets.seed(
            contour: contour, scope: 0..<67_200, reference: 440,
            preset: .performance, toleranceCents: 0, allowedPitchClasses: mask)
        XCTAssertEqual(performance[0].note(at: 43 * 480) ?? .nan, 70.2, accuracy: 0.001)
    }
    func testPresetDecisionResetsAfterUnvoicedGap() {
        var contour = (0..<30).map { point($0 * 480, note: 69.4) }
        contour.append(.init(frame: 14_400, frequency: 0, confidence: 0))
        contour.append(contentsOf: (31..<36).map { point($0 * 480, note: 70.2) })
        let targets = PitchPointTargets.seed(
            contour: contour, scope: 0..<17_280, reference: 440,
            preset: .tight, toleranceCents: 0)
        XCTAssertEqual(targets.count, 2)
        XCTAssertEqual(Set(targets[0].points.map(\.note)), [69])
        XCTAssertEqual(Set(targets[1].points.map(\.note)), [70])
        XCTAssertNil(targets[0].note(at: 14_400))
        XCTAssertNil(targets[1].note(at: 14_400))
    }
    func testHarmonic450And532HzVoicesStayOnOnePresetTargetDuringVibrato() throws {
        for center in [450.0, 532.0] {
            var phase = 0.0
            let samples = (0..<96_000).map { index -> Float in
                let frequency = center * (1 + 0.02 * sin(2 * Double.pi * 5.5 * Double(index) / sessionRate))
                phase += 2 * Double.pi * frequency / sessionRate
                return Float(0.2 * sin(phase) + 0.07 * sin(phase * 2) + 0.035 * sin(phase * 3))
            }
            let audio = AudioSamples(channels: [samples])
            let contour = try PitchContourAnalysis.analyze(audio, maximumPoints: 65_536)
            XCTAssertTrue(contour.contains(where: \.isVoiced))
            let targets = PitchPointTargets.seed(
                contour: contour, scope: 0..<audio.count, reference: 440,
                preset: .tight)
            let expected = Float(Int((69 + 12 * log2(center / 440)).rounded()))
            XCTAssertEqual(
                Set(targets.flatMap(\.points).map(\.note)), [expected],
                "Measured harmonic \(center) Hz input produced intermittent wrong semitones")
        }
    }
    func testAbsoluteViewportSupportsFineZoomAndExplicitSelectedFit() {
        var viewport = PitchPointViewport()
        viewport.fit(scope: 48_000..<96_000, notes: [69, 71])
        viewport.zoomTime(2, around: 72_000)
        XCTAssertEqual(viewport.frames, 60_000..<84_000)
        viewport.zoomTime(1000)
        XCTAssertEqual(viewport.frames.count, 480)
        viewport.panTime(1)
        XCTAssertTrue((48_000..<96_000).contains(viewport.frames.lowerBound))
        viewport.zoomTime(Double.leastNonzeroMagnitude)
        XCTAssertEqual(viewport.frames, 48_000..<96_000)
        viewport.zoomPitch(100)
        XCTAssertEqual(viewport.upper - viewport.lower, 0.5)
        let stable = viewport
        let edited = PitchEditPoint(frame: 72_000, note: 79.25)
        XCTAssertEqual(viewport, stable, "An edit itself must not reframe either axis")
        viewport.fitSelected([edited])
        XCTAssertTrue(viewport.frames.contains(72_000))
        XCTAssertLessThan(viewport.lower, edited.note)
        XCTAssertGreaterThan(viewport.upper, edited.note)
        XCTAssertEqual(viewport.frame(.nan), viewport.frames.lowerBound)
    }
    func testPitchLabelsRetainFractionalCents() {
        XCTAssertEqual(PitchPointEditorView.noteName(69.25, cents: true), "A4 +25.0¢")
        XCTAssertEqual(PitchPointEditorView.noteName(68.9, cents: true), "A4 -10.0¢")
        XCTAssertEqual(PitchPointEditorView.noteName(.nan), "—")
    }
    func testExpandedKeyboardScopeDoesNotCaptureMainWindowOrTextEntry() {
        MainActor.assumeIsolated {
            let main = NSWindow()
            let editor = NSWindow()
            XCTAssertTrue(
                PitchEditorKeyboardRouting.accepts(
                    eventWindow: editor, editorWindow: editor, isKey: true,
                    hasNestedSheet: false, responder: NSView()))
            XCTAssertFalse(
                PitchEditorKeyboardRouting.accepts(
                    eventWindow: main, editorWindow: editor, isKey: true,
                    hasNestedSheet: false, responder: NSView()))
            XCTAssertFalse(
                PitchEditorKeyboardRouting.accepts(
                    eventWindow: editor, editorWindow: editor, isKey: false,
                    hasNestedSheet: false, responder: NSView()))
            XCTAssertFalse(
                PitchEditorKeyboardRouting.accepts(
                    eventWindow: editor, editorWindow: editor, isKey: true,
                    hasNestedSheet: true, responder: NSView()))
            XCTAssertFalse(
                PitchEditorKeyboardRouting.accepts(
                    eventWindow: editor, editorWindow: editor, isKey: true,
                    hasNestedSheet: false, responder: NSTextView()))
        }
    }
    func testExpandedKeyboardRoutesAuditionUndoRedoAndIgnoresUnrelatedShortcuts() {
        XCTAssertEqual(PitchEditorKeyboardRouting.action(keyCode: 49, modifiers: [], repeated: false), .space)
        XCTAssertEqual(PitchEditorKeyboardRouting.action(keyCode: 49, modifiers: [], repeated: true), .consume)
        XCTAssertEqual(PitchEditorKeyboardRouting.action(keyCode: 6, modifiers: .command, repeated: false), .undo)
        XCTAssertEqual(
            PitchEditorKeyboardRouting.action(keyCode: 6, modifiers: [.command, .shift], repeated: false), .redo)
        XCTAssertNil(PitchEditorKeyboardRouting.action(keyCode: 49, modifiers: .command, repeated: false))
        XCTAssertNil(PitchEditorKeyboardRouting.action(keyCode: 6, modifiers: [.command, .option], repeated: false))
        XCTAssertNil(PitchEditorKeyboardRouting.action(keyCode: 51, modifiers: [], repeated: false))
    }
    func testUndoWorksAfterNontextControlsWhileSpaceKeepsNativeControlBehavior() {
        MainActor.assumeIsolated {
            for control in [NSSlider(), NSPopUpButton()] as [NSResponder] {
                XCTAssertTrue(PitchEditorKeyboardRouting.accepts(.undo, responder: control))
                XCTAssertTrue(PitchEditorKeyboardRouting.accepts(.redo, responder: control))
                XCTAssertFalse(PitchEditorKeyboardRouting.accepts(.space, responder: control))
                XCTAssertFalse(PitchEditorKeyboardRouting.accepts(.consume, responder: control))
            }
        }
    }
}
