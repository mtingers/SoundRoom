import SoundroomCore
import SwiftUI

/// Only this small overlay ticks; instrument controls and the note editor stay idle.
struct DesignerPlayhead: View {
    let viewport: DesignerViewport
    let bpm: Double, beats: Double
    let playing: Bool, recording: Bool
    let started: Double
    let position: (() -> Double)?
    let width: Double, height: Double
    var body: some View {
        SwiftUI.TimelineView(.animation(minimumInterval: 1 / 25, paused: !playing && !recording)) { _ in
            let elapsed = max(0, ProcessInfo.processInfo.systemUptime - started)
            let beat =
                recording
                ? min(beats, elapsed * bpm / 60)
                : (position?() ?? (elapsed * bpm / 60).truncatingRemainder(dividingBy: beats))
            if (playing || recording) && beat >= viewport.start && beat <= viewport.start + viewport.span {
                Rectangle().fill(recording ? Color.red : Theme.ink).frame(width: 1, height: height)
                    .offset(x: (beat - viewport.start) / viewport.span * width)
            }
        }.allowsHitTesting(false).accessibilityHidden(true)
    }
}
