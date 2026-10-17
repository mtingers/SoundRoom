import Foundation

/// A resolved clip envelope retained when an edit changes its neighboring clips.
/// Coordinates are relative to the original clip, so relocation and fragments
/// preserve the sound without depending on the current track's adjacency.
public struct ClipRenderEnvelope: Codable, Equatable, Sendable {
    public var referenceLength: Int
    public var referenceOffset: Int = 0
    public var attackFrames: Int
    public var releaseFrames: Int
    public var boundaryAttackFrames: Int
    public var boundaryReleaseFrames: Int
    public var crossfadeIn: Range<Int>?
    public var crossfadeOut: Range<Int>?
    public var cutReferenceOffset: Int?
    public var cutReferenceLength: Int?
    public var additionalBoundaries: [ClipBoundaryEnvelope]?

    public init(_ envelope: ClipEnvelope) {
        if let existing = envelope.clip.renderEnvelope {
            self = existing
            cutReferenceOffset = existing.referenceOffset
            cutReferenceLength = envelope.clip.length
            if envelope.boundaryAttackFrames > 0 || envelope.boundaryReleaseFrames > 0 {
                let boundary = ClipBoundaryEnvelope(
                    start: existing.referenceOffset, length: envelope.clip.length,
                    attack: envelope.boundaryAttackFrames, release: envelope.boundaryReleaseFrames)
                if !(additionalBoundaries ?? []).contains(boundary) {
                    additionalBoundaries = (additionalBoundaries ?? []) + [boundary]
                }
            }
            return
        }
        referenceLength = envelope.clip.length
        attackFrames = envelope.attackFrames
        releaseFrames = envelope.releaseFrames
        boundaryAttackFrames = envelope.boundaryAttackFrames
        boundaryReleaseFrames = envelope.boundaryReleaseFrames
        let start = envelope.clip.start
        crossfadeIn = envelope.crossfadeIn.map { ($0.lowerBound - start)..<($0.upperBound - start) }
        crossfadeOut = envelope.crossfadeOut.map { ($0.lowerBound - start)..<($0.upperBound - start) }
    }
    static func adding(_ left: Int, _ right: Int) -> Int {
        let (value, overflow) = left.addingReportingOverflow(right)
        return overflow ? (right >= 0 ? Int.max : Int.min) : value
    }
    private enum CodingKeys: String, CodingKey {
        case referenceLength, referenceOffset, attackFrames, releaseFrames, boundaryAttackFrames, boundaryReleaseFrames,
            crossfadeIn, crossfadeOut, cutReferenceOffset, cutReferenceLength, additionalBoundaries
    }
    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        let limit = Int.max / 4
        func integer(_ key: CodingKeys, minimum: Int = 0) throws -> Int {
            max(minimum, min(limit, try values.decodeIfPresent(Int.self, forKey: key) ?? minimum))
        }
        func range(_ key: CodingKeys) throws -> Range<Int>? {
            guard let decoded = try values.decodeIfPresent(Range<Int>.self, forKey: key) else { return nil }
            let low = max(0, min(limit, decoded.lowerBound))
            let high = max(0, min(limit, decoded.upperBound))
            return high > low ? low..<high : nil
        }
        referenceLength = try integer(.referenceLength, minimum: 1)
        referenceOffset = try integer(.referenceOffset)
        attackFrames = try integer(.attackFrames)
        releaseFrames = try integer(.releaseFrames)
        boundaryAttackFrames = try integer(.boundaryAttackFrames)
        boundaryReleaseFrames = try integer(.boundaryReleaseFrames)
        crossfadeIn = try range(.crossfadeIn)
        crossfadeOut = try range(.crossfadeOut)
        cutReferenceOffset = try values.decodeIfPresent(Int.self, forKey: .cutReferenceOffset).map {
            max(0, min(limit, $0))
        }
        cutReferenceLength = try values.decodeIfPresent(Int.self, forKey: .cutReferenceLength).map {
            max(1, min(limit, $0))
        }
        additionalBoundaries = try values.decodeIfPresent([ClipBoundaryEnvelope].self, forKey: .additionalBoundaries)
            .map {
                $0.map {
                    ClipBoundaryEnvelope(
                        start: max(0, min(limit, $0.start)), length: max(1, min(limit, $0.length)),
                        attack: max(0, min(limit, $0.attack)), release: max(0, min(limit, $0.release)))
                }
            }
    }
    public static func unattenuated(length: Int) -> Self {
        let clip = Clip(assetID: UUID(), name: "", start: 0, length: length, edgeFadeFrames: 0)
        return Self(ClipEnvelope(clip: clip, attackFrames: 0, releaseFrames: 0))
    }
}

public struct ClipBoundaryEnvelope: Codable, Equatable, Sendable {
    public var start: Int
    public var length: Int
    public var attack: Int
    public var release: Int
}
