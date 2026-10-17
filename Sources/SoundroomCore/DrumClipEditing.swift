import Foundation

extension Clip {
    /// Older trims discarded the live metadata but retained the unchanged
    /// source in history. Recover only a range of that exact audio asset.
    public var editableDrumPattern: DrumPattern? {
        if let drumPattern { return drumPattern }
        return (previousStates ?? []).sorted { $0.createdAt > $1.createdAt }.first {
            $0.drumPattern != nil && $0.assetID == assetID && offset >= $0.offset && offset - $0.offset <= $0.length
                && length <= $0.length - (offset - $0.offset)
        }?.drumPattern
    }

    /// A complete loop follows pattern length changes. A sliced loop keeps its
    /// source window, so editing hits does not undo a trim or shift a pasted slice.
    public func replacingDrumRender(assetID: UUID, pattern: DrumPattern, frameCount: Int) -> Clip {
        var clip = self
        let whole = editableDrumPattern.map { offset == 0 && length >= $0.renderedLoopFrames } ?? true
        if whole {
            clip.offset = 0
            clip.length = frameCount
        } else {
            clip.offset = min(offset, max(0, frameCount - 1))
            clip.length = min(length, max(1, frameCount - clip.offset))
        }
        if clip.offset != offset || clip.length != length { clip.resetFadeEnvelope() }
        clip.assetID = assetID
        clip.drumPattern = pattern
        clip.soundDesign = nil
        return clip
    }
}
