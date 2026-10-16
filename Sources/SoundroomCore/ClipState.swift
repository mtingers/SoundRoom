import Foundation

/// An immutable audio/edit snapshot without nested history or timeline position.
public struct ClipState: Codable, Equatable, Identifiable, Sendable {
    public var id = UUID()
    public var operation: String
    public var createdAt = Date()
    public var renderEnvelope: ClipRenderEnvelope?
    public var assetID: UUID
    public var name: String
    public var offset: Int
    public var length: Int
    public var soundDesign: SoundDesignPattern?
    public var drumPattern: DrumPattern?
    public var fadeInFrames: Int?
    public var fadeOutFrames: Int?
    public var fadeReferenceOffset: Int?
    public var fadeReferenceLength: Int?
    public var edgeFadeFrames: Int?
    public var edgeProtectionDisabled: Bool?
    public var cutFadeInFrames: Int?
    public var cutFadeOutFrames: Int?

    public init(clip: Clip, operation: String) {
        self.operation = operation
        renderEnvelope = clip.renderEnvelope
        assetID = clip.assetID; name = clip.name; offset = clip.offset; length = clip.length
        soundDesign = clip.soundDesign; drumPattern = clip.drumPattern
        fadeInFrames = clip.fadeInFrames; fadeOutFrames = clip.fadeOutFrames
        fadeReferenceOffset = clip.fadeReferenceOffset; fadeReferenceLength = clip.fadeReferenceLength
        edgeFadeFrames = clip.edgeFadeFrames; edgeProtectionDisabled = clip.edgeProtectionDisabled
        cutFadeInFrames = clip.cutFadeInFrames; cutFadeOutFrames = clip.cutFadeOutFrames
    }
    public func applying(to original: Clip) -> Clip {
        var clip = original
        clip.renderEnvelope = renderEnvelope
        clip.assetID = assetID; clip.name = name; clip.offset = offset; clip.length = length
        clip.soundDesign = soundDesign; clip.drumPattern = drumPattern
        clip.fadeInFrames = fadeInFrames; clip.fadeOutFrames = fadeOutFrames
        clip.fadeReferenceOffset = fadeReferenceOffset; clip.fadeReferenceLength = fadeReferenceLength
        clip.edgeFadeFrames = edgeFadeFrames; clip.edgeProtectionDisabled = edgeProtectionDisabled
        clip.cutFadeInFrames = cutFadeInFrames; clip.cutFadeOutFrames = cutFadeOutFrames
        return clip
    }
    public func matches(_ clip: Clip) -> Bool {
        var other = ClipState(clip: clip, operation: operation)
        other.id = id; other.createdAt = createdAt
        return other == self
    }
}

extension Clip {
    public static let previousStateLimit = 5
    public mutating func remember(_ previous: Clip, operation: String) {
        let state = ClipState(clip: previous, operation: operation)
        guard !state.matches(self) else { return }
        var states = previous.previousStates ?? []
        states.append(state)
        previousStates = Array(states.suffix(Self.previousStateLimit))
    }
    public var referencedAssetIDs: Set<UUID> { Set([assetID] + (previousStates ?? []).map(\.assetID)) }
}

extension Clip {
    public mutating func preserveHistoryFadeReference(from original: Clip, localOffset: Int) {
        preserveFadeEnvelope(from: original, localOffset: localOffset)
    }
}

extension Session {
    /// Preserve audio states once per edit/gesture; timeline moves alone do not add a state.
    public mutating func captureClipHistory(before: Session, operation: String) {
        let originals = Dictionary(uniqueKeysWithValues: before.tracks.flatMap(\.clips).map { ($0.id, $0) })
        for t in tracks.indices {
            for c in tracks[t].clips.indices {
                guard let original = originals[tracks[t].clips[c].id] else { continue }
                tracks[t].clips[c].remember(original, operation: operation)
            }
        }
    }
}
