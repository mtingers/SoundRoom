import SwiftUI
import SoundroomCore

/// Give Canvas regions the same geometry in the accessibility tree as on
/// screen. Unpositioned virtual children have no frame for spatial navigation.
struct TrackClipAccessibility: View {
    @ObservedObject var store: SessionStore
    let track: Track
    let visiblePixels: Range<Double>?
    private func span(_ clip: Clip) -> Range<Double> {
        let start = Double(clip.start) / sessionRate * store.zoom
        let end = Double(clip.end) / sessionRate * store.zoom
        let lower = max(start, visiblePixels?.lowerBound ?? start)
        return lower..<max(lower, min(end, visiblePixels?.upperBound ?? end))
    }
    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .topLeading) {
                ForEach(track.clips.filter { !span($0).isEmpty }) { clip in
                    let bounds = span(clip)
                    Button { store.choose(track: track.id, clip: clip.id) } label: {
                        Text(clip.name).frame(width: bounds.upperBound - bounds.lowerBound, height: max(1, geometry.size.height - 16)).contentShape(Rectangle())
                    }
                        .buttonStyle(.plain)
                        .position(x: (bounds.lowerBound + bounds.upperBound) / 2, y: geometry.size.height / 2)
                        .accessibilityLabel("\(clip.name), \(track.name)")
                        .accessibilityValue("\(AudioChannelPresentation.label(channels: store.assets[clip.assetID]?.channelCount).map { $0 + ", " } ?? "")Start \(timeString(clip.start)), duration \(timeString(clip.length))")
                        .accessibilityHint("Select clip. More actions include adding to selection and copying.")
                        .accessibilityAddTraits(store.selectedClips.contains(clip.id) ? .isSelected : [])
                        .accessibilityActions {
                            Button("Add or remove from selection") { store.choose(track: track.id, clip: clip.id, extending: true) }
                            Button("Copy clip") { store.choose(track: track.id, clip: clip.id); store.copy() }
                            if clip.editableDrumPattern != nil {
                                Button("Edit beats") { store.choose(track: track.id, clip: clip.id); store.toggleInlineDrums(trackID: track.id, clipID: clip.id) }
                            }
                            if clip.editableSoundDesign != nil {
                                Button("Edit notes inline") { store.choose(track: track.id, clip: clip.id); store.toggleInlineSound(trackID: track.id, clipID: clip.id) }
                            }
                        }.disabled(store.busy || store.isRecording)
                }
            }.frame(width: geometry.size.width, height: geometry.size.height, alignment: .topLeading)
        }.accessibilityElement(children: .contain).accessibilityLabel("\(track.name), \(track.clips.count) audio clips")
    }
}
