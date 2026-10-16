import Foundation

extension Clip {
    /// Recover legacy trims only while they reference the original PCM window.
    public var editableSoundDesign: SoundDesignPattern? {
        if let soundDesign { return soundDesign }
        return (previousStates ?? []).sorted { $0.createdAt > $1.createdAt }.first {
            $0.soundDesign != nil && $0.assetID == assetID && offset >= $0.offset &&
            offset - $0.offset <= $0.length && length <= $0.length - (offset - $0.offset)
        }?.soundDesign
    }

    /// Editing a sliced instrument must not undo its trim or rename/move it.
    public func replacingSoundDesignRender(assetID: UUID, pattern: SoundDesignPattern, frameCount: Int) -> Clip {
        var clip = self
        let whole = editableSoundDesign.map { offset == 0 && length >= $0.sequenceFrames } ?? true
        if whole { clip.offset = 0; clip.length = frameCount }
        else {
            clip.offset = min(offset, max(0, frameCount - 1))
            clip.length = min(length, max(1, frameCount - clip.offset))
        }
        if clip.offset != offset || clip.length != length { clip.resetFadeEnvelope() }
        clip.assetID = assetID; clip.soundDesign = pattern; clip.drumPattern = nil
        return clip
    }
}

extension SoundDesignPattern {
    public var sequenceFrames: Int {
        let tempo = bpm.isFinite ? max(30, min(300, bpm)) : 120
        let length = beats.isFinite ? max(0.25, min(64, beats)) : 4
        return max(1, Int((length * 60 / tempo * sessionRate).rounded()))
    }
}
