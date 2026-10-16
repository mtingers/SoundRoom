import XCTest
import AppKit
import SwiftUI
import SoundroomCore
@testable import Soundroom

final class PitchEditorLayoutTests: XCTestCase {
    @MainActor func testPreparingAndAuditioningDoNotChangePitchCardGeometry() {
        // The compact card receives 248 pt inside the minimum 280 pt drawer.
        // Measure real SwiftUI layouts rather than duplicating spacing constants.
        for (width, expanded) in [(CGFloat(248), false), (CGFloat(428), false), (CGFloat(760), true)] {
            let idle = size(width: width, expanded: expanded, busy: false, previewing: false)
            let preparing = size(width: width, expanded: expanded, busy: true, previewing: false)
            let auditioning = size(width: width, expanded: expanded, busy: false, previewing: true)
            XCTAssertGreaterThan(idle.height, 180)
            XCTAssertEqual(preparing.width, idle.width, accuracy: 0.5)
            XCTAssertEqual(preparing.height, idle.height, accuracy: 0.5, "A worker spinner must not insert a row above the chart")
            XCTAssertEqual(auditioning.width, idle.width, accuracy: 0.5)
            XCTAssertEqual(auditioning.height, idle.height, accuracy: 0.5, "Audition/Stop must occupy the same space")
        }
    }
    @MainActor private func size(width: CGFloat, expanded: Bool, busy: Bool, previewing: Bool) -> NSSize {
        let region = PitchEditRegion(startFrame: 0, endFrame: 48_000, points: [
            .init(frame: 0, note: 69), .init(frame: 48_000, note: 69)])
        let editor = PitchPointEditorView(regions: .constant([region]), before: [], after: nil, scope: 0..<48_000,
            busy: busy, isPreviewing: previewing, context: "Live targets · Apply selected audio", expanded: expanded)
        let host = NSHostingView(rootView: editor.frame(width: width).fixedSize(horizontal: false, vertical: true))
        host.layoutSubtreeIfNeeded()
        return host.fittingSize
    }
}
