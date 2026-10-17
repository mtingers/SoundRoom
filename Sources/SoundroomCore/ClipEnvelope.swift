import Foundation

/// Prepared outside the render callback. Linear overlap fades have constant summed gain,
/// so correlated sources do not acquire a gain bump halfway through a crossfade.
public struct ClipEnvelope: Sendable {
    public let clip: Clip
    public var attackFrames: Int
    public var releaseFrames: Int
    public var boundaryAttackFrames: Int = 0
    public var boundaryReleaseFrames: Int = 0
    public var crossfadeIn: Range<Int>?
    public var crossfadeOut: Range<Int>?

    public func gain(at localFrame: Int) -> Float {
        guard localFrame >= 0, localFrame < clip.length else { return 0 }
        var gain = clip.fadeGain(at: localFrame)
        let (sum, overflow) = localFrame.addingReportingOverflow(max(0, clip.fadeReferenceOffset ?? 0))
        let envelopeFrame = overflow ? Int.max : sum
        let envelopeLength = max(1, clip.fadeReferenceLength ?? clip.length)
        if attackFrames > 0 { gain = min(gain, min(1, Float(envelopeFrame) / Float(attackFrames))) }
        if releaseFrames > 0 {
            gain = min(gain, min(1, Float(max(0, envelopeLength - 1 - envelopeFrame)) / Float(releaseFrames)))
        }
        if let frozen = clip.renderEnvelope {
            let position = ClipRenderEnvelope.adding(max(0, frozen.referenceOffset), localFrame)
            if frozen.boundaryAttackFrames > 0 {
                gain = min(gain, min(1, Float(position) / Float(frozen.boundaryAttackFrames)))
            }
            if frozen.boundaryReleaseFrames > 0 {
                gain = min(
                    gain,
                    min(
                        1,
                        Float(max(0, Double(frozen.referenceLength) - 1 - Double(position)))
                            / Float(frozen.boundaryReleaseFrames)))
            }
            for boundary in frozen.additionalBoundaries ?? [] {
                let local = Double(position) - Double(boundary.start)
                if boundary.attack > 0 { gain = min(gain, min(1, Float(max(0, local)) / Float(boundary.attack))) }
                if boundary.release > 0 {
                    gain = min(
                        gain, min(1, Float(max(0, Double(boundary.length) - 1 - local)) / Float(boundary.release)))
                }
            }
        }
        if boundaryAttackFrames > 0 { gain = min(gain, min(1, Float(localFrame) / Float(boundaryAttackFrames))) }
        if boundaryReleaseFrames > 0 {
            gain = min(gain, min(1, Float(clip.length - 1 - localFrame) / Float(boundaryReleaseFrames)))
        }
        let frame = ClipRenderEnvelope.adding(clip.start, localFrame)
        if let range = crossfadeIn {
            let amount: Float =
                range.count == 1
                ? (frame >= range.lowerBound ? 1 : 0)
                : min(1, max(0, Float(frame - range.lowerBound) / Float(range.count - 1)))
            gain = min(gain, amount)
        }
        if let range = crossfadeOut {
            gain = min(gain, min(1, max(0, Float(range.upperBound - 1 - frame) / Float(max(1, range.count - 1)))))
        }
        return gain
    }

    public static func resolve(track: Track) -> [UUID: ClipEnvelope] {
        let sorted = track.clips.sorted {
            $0.start == $1.start ? $0.id.uuidString < $1.id.uuidString : $0.start < $1.start
        }
        var result: [UUID: ClipEnvelope] = [:]
        for clip in sorted {
            if let frozen = clip.renderEnvelope {
                // A new cut on a frozen/baked clip adds only a local safety fade.
                // Its original resolved envelope remains in original coordinates.
                let cutStart = frozen.cutReferenceOffset ?? 0
                let cutLength = frozen.cutReferenceLength ?? frozen.referenceLength
                let newAttack = frozen.referenceOffset != cutStart
                let newRelease =
                    ClipRenderEnvelope.adding(frozen.referenceOffset, clip.length)
                    != ClipRenderEnvelope.adding(cutStart, cutLength)
                let limit = max(1, (max(1, clip.length) - 1) / 2)
                let shift = ClipRenderEnvelope.adding(clip.start, -max(0, frozen.referenceOffset))
                result[clip.id] = ClipEnvelope(
                    clip: clip, attackFrames: frozen.attackFrames, releaseFrames: frozen.releaseFrames,
                    boundaryAttackFrames: newAttack && clip.edgeProtectionDisabled != true
                        ? min(max(0, clip.cutFadeInFrames ?? 0), limit) : 0,
                    boundaryReleaseFrames: newRelease && clip.edgeProtectionDisabled != true
                        ? min(max(0, clip.cutFadeOutFrames ?? 0), limit) : 0,
                    crossfadeIn: frozen.crossfadeIn.map {
                        ClipRenderEnvelope.adding(
                            $0.lowerBound, shift)..<ClipRenderEnvelope.adding($0.upperBound, shift)
                    },
                    crossfadeOut: frozen.crossfadeOut.map {
                        ClipRenderEnvelope.adding(
                            $0.lowerBound, shift)..<ClipRenderEnvelope.adding($0.upperBound, shift)
                    })
                continue
            }
            let frames =
                clip.edgeProtectionDisabled == true
                ? 0
                : min(
                    max(0, clip.edgeFadeFrames ?? 0), max(1, (max(1, clip.fadeReferenceLength ?? clip.length) - 1) / 2))
            result[clip.id] = ClipEnvelope(
                clip: clip, attackFrames: frames, releaseFrames: frames,
                boundaryAttackFrames: clip.edgeProtectionDisabled == true
                    ? 0 : min(max(0, clip.cutFadeInFrames ?? 0), max(1, (max(1, clip.length) - 1) / 2)),
                boundaryReleaseFrames: clip.edgeProtectionDisabled == true
                    ? 0 : min(max(0, clip.cutFadeOutFrames ?? 0), max(1, (max(1, clip.length) - 1) / 2)))
        }
        for (index, clip) in sorted.enumerated() {
            guard index + 1 < sorted.count else { continue }
            let next = sorted[index + 1]
            // Repeated untrimmed generated loops carry cyclic tails; their join must not
            // acquire fresh edge dips. Manual fades remain intentional and still apply.
            let cyclicLoop =
                clip.drumPattern != nil && clip.drumPattern == next.drumPattern && clip.offset == 0 && next.offset == 0
                && clip.length == next.length
            let sourceContiguous = clip.assetID == next.assetID && clip.offset + clip.length == next.offset
            if clip.end == next.start && (sourceContiguous || cyclicLoop) {
                result[clip.id]?.boundaryReleaseFrames = 0
                result[next.id]?.boundaryAttackFrames = 0
                if clip.fadeReferenceLength == nil && clip.renderEnvelope == nil { result[clip.id]?.releaseFrames = 0 }
                if next.fadeReferenceLength == nil && next.renderEnvelope == nil { result[next.id]?.attackFrames = 0 }
            } else if next.start > clip.start && next.start < clip.end && next.end >= clip.end
                && clip.edgeProtectionDisabled != true && next.edgeProtectionDisabled != true
                && (clip.edgeFadeFrames ?? 0) > 0 && (next.edgeFadeFrames ?? 0) > 0
            {
                let overlap = next.start..<clip.end
                if clip.renderEnvelope == nil {
                    result[clip.id]?.crossfadeOut = overlap
                    result[clip.id]?.releaseFrames = 0
                    result[clip.id]?.boundaryReleaseFrames = 0
                }
                if next.renderEnvelope == nil {
                    result[next.id]?.crossfadeIn = overlap
                    result[next.id]?.attackFrames = 0
                    result[next.id]?.boundaryAttackFrames = 0
                }
            }
        }
        return result
    }
}
