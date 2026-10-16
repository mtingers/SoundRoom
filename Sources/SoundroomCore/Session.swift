import Foundation

public let sessionRate: Double = 48_000

public struct AudioSamples: Equatable, Sendable {
    public private(set) var cacheIdentity = UUID()
    private var resident: [[Float]]
    private var mapping: MappedWaveSamples?
    private var sampleView: AudioSampleView?
    private var displayFrames: Int?
    private var displayChannels: Int?
    /// Client-side shape only. No PCM or hardware is retained by a control client.
    public var isDisplayMetadata: Bool { displayFrames != nil }
    /// Editing callers can materialize PCM explicitly. Playback and project I/O
    /// use metadata/sample access and never need a session-sized channel array.
    public var channels: [[Float]] {
        get {
            guard displayFrames == nil else { return [] }
            guard mapping != nil || sampleView != nil else { return resident }
            return (0..<channelCount).map { channel in (0..<count).map { sample(channel: channel, frame: $0) } }
        }
        set { resident = newValue; mapping = nil; sampleView = nil; displayFrames = nil; displayChannels = nil; cacheIdentity = UUID() }
    }
    public var count: Int { displayFrames ?? sampleView?.frames ?? mapping?.frameCount ?? resident.first?.count ?? 0 }
    public var channelCount: Int { displayChannels ?? sampleView?.channels.count ?? mapping?.channelCount ?? resident.count }
    public var hasConsistentChannels: Bool {
        if let sampleView { return sampleView.source.hasConsistentChannels }
        return mapping != nil || resident.allSatisfy { $0.count == count }
    }
    public var isStreamed: Bool { mapping != nil || (sampleView?.source.isStreamed ?? false) }
    public init(channels: [[Float]]) { resident = channels; mapping = nil; sampleView = nil }
    public init(displayFrames: Int, channels: Int, identity: UUID) {
        resident = []; mapping = nil; sampleView = nil
        self.displayFrames = max(0, displayFrames); displayChannels = max(0, min(64, channels)); cacheIdentity = identity
    }
    init(mapped: MappedWaveSamples) { resident = []; mapping = mapped; sampleView = nil }
    init(view: AudioSampleView) { resident = []; mapping = nil; sampleView = view }
    @inline(__always) public func sample(channel: Int, frame: Int) -> Float {
        if let sampleView { return sampleView.sample(channel: channel, frame: frame) }
        if let mapping { return mapping.sample(channel: channel, frame: frame) }
        guard resident.indices.contains(channel), resident[channel].indices.contains(frame) else { return 0 }
        return resident[channel][frame]
    }
    /// Worker-only PCM read. Call with bounded ranges for export/analysis/seek.
    public func readFrames(_ range: Range<Int>, channels indices: [Int]? = nil) -> [[Float]] {
        let indices = indices ?? Array(0..<channelCount)
        if isDisplayMetadata { return indices.map { _ in [] } }
        if let mapping { return mapping.readFrames(range, channels: indices) }
        if let sampleView { return sampleView.readFrames(range, channels: indices) }
        return indices.map { channel in
            if resident.indices.contains(channel), range.lowerBound >= 0, range.upperBound <= resident[channel].count {
                return Array(resident[channel][range])
            }
            return range.map { sample(channel: channel, frame: $0) }
        }
    }
    /// Worker-only mapped-page hint. Bounded workers use readFrames for PCM.
    public func discardReadPages() { mapping?.discardPages(); sampleView?.source.discardReadPages() }
    public static func == (left: Self, right: Self) -> Bool {
        if left.cacheIdentity == right.cacheIdentity { return true }
        if left.isDisplayMetadata || right.isDisplayMetadata { return false }
        if let a = left.mapping, let b = right.mapping, a === b { return true }
        if !left.isStreamed && !right.isStreamed { return left.channels == right.channels }
        guard left.count == right.count, left.channelCount == right.channelCount else { return false }
        for start in stride(from: 0, to: left.count, by: 8192) {
            let range = start..<min(left.count, start + 8192)
            if left.readFrames(range) != right.readFrames(range) { return false }
        }
        return true
    }
    public func slice(_ range: Range<Int>) -> AudioSamples {
        let lo = max(0, min(count, range.lowerBound)), hi = max(0, min(count, range.upperBound))
        return AudioSamples(channels: readFrames(lo..<max(lo, hi)))
    }
    public func resampled(from rate: Double) -> AudioSamples {
        guard rate > 0, rate != sessionRate, count > 0 else { return self }
        let length = Int((Double(count) * sessionRate / rate).rounded())
        return AudioSamples(channels: channels.map { channel in
            (0..<length).map { i in
                let pos = min(Double(count - 1), Double(i) * rate / sessionRate)
                let a = Int(pos), b = min(count - 1, a + 1), fraction = Float(pos - Double(a))
                return channel[a] * (1 - fraction) + channel[b] * fraction
            }
        })
    }
}

public struct Clip: Identifiable, Codable, Equatable, Sendable {
    public var id: UUID
    public var assetID: UUID
    public var name: String
    public var start: Int
    public var offset: Int
    public var length: Int
    public var soundDesign: SoundDesignPattern?
    public var drumPattern: DrumPattern?
    public var previousStates: [ClipState]?
    public var renderEnvelope: ClipRenderEnvelope?
    /// New clips use a 5 ms safety envelope. Missing values in legacy projects preserve their sound.
    public var edgeFadeFrames: Int?
    public var edgeProtectionDisabled: Bool?
    public var cutFadeInFrames: Int?
    public var cutFadeOutFrames: Int?
    public var fadeInFrames: Int?
    public var fadeOutFrames: Int?
    /// Fragment position inside the original clip envelope. Splitting and copying
    /// preserve the existing sound rather than starting another fade at each cut.
    public var fadeReferenceOffset: Int?
    public var fadeReferenceLength: Int?
    public func fadeGain(at localFrame: Int) -> Float {
        let envelopeLength = max(1, fadeReferenceLength ?? length)
        let (sum, overflow) = max(0, fadeReferenceOffset ?? 0).addingReportingOverflow(max(0, localFrame))
        let envelopeFrame = overflow ? Int.max : sum
        let fadeIn = min(envelopeLength, max(0, fadeInFrames ?? 0))
        let fadeOut = min(envelopeLength, max(0, fadeOutFrames ?? 0))
        let attack: Float = fadeIn > 0 ? min(1, Float(envelopeFrame) / Float(fadeIn)) : 1
        let release: Float = fadeOut > 0 ? min(1, Float(max(0, envelopeLength - 1 - envelopeFrame)) / Float(fadeOut)) : 1
        return min(attack, release)
    }
    public mutating func resetFadeEnvelope() { fadeReferenceOffset = nil; fadeReferenceLength = nil; renderEnvelope = nil }
    mutating func preserveFadeEnvelope(from original: Clip, localOffset: Int) {
        if var frozen = original.renderEnvelope {
            frozen.referenceOffset = ClipRenderEnvelope.adding(max(0, frozen.referenceOffset), max(0, localOffset))
            renderEnvelope = frozen
        }
        guard (original.fadeInFrames ?? 0) > 0 || (original.fadeOutFrames ?? 0) > 0 || (original.edgeFadeFrames ?? 0) > 0 else { return }
        guard localOffset != 0 || length != original.length else { return }
        fadeReferenceLength = original.fadeReferenceLength ?? original.length
        let (sum, overflow) = max(0, original.fadeReferenceOffset ?? 0).addingReportingOverflow(max(0, localOffset))
        fadeReferenceOffset = overflow ? Int.max : sum
    }
    private enum CodingKeys: String, CodingKey {
        case id, assetID, name, start, offset, length, soundDesign, drumPattern, previousStates, renderEnvelope
        case fadeInFrames, fadeOutFrames, fadeReferenceOffset, fadeReferenceLength
        case edgeFadeFrames, edgeProtectionDisabled, cutFadeInFrames, cutFadeOutFrames
    }
    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        id = try values.decode(UUID.self, forKey: .id)
        assetID = try values.decode(UUID.self, forKey: .assetID)
        name = try values.decode(String.self, forKey: .name)
        start = try values.decode(Int.self, forKey: .start)
        offset = try values.decode(Int.self, forKey: .offset)
        length = try values.decode(Int.self, forKey: .length)
        soundDesign = try values.decodeIfPresent(SoundDesignPattern.self, forKey: .soundDesign)
        drumPattern = try values.decodeIfPresent(DrumPattern.self, forKey: .drumPattern)
        previousStates = try values.decodeIfPresent([ClipState].self, forKey: .previousStates).map { Array($0.suffix(Self.previousStateLimit)) }
        renderEnvelope = try values.decodeIfPresent(ClipRenderEnvelope.self, forKey: .renderEnvelope)
        fadeInFrames = try values.decodeIfPresent(Int.self, forKey: .fadeInFrames)
        fadeOutFrames = try values.decodeIfPresent(Int.self, forKey: .fadeOutFrames)
        fadeReferenceOffset = try values.decodeIfPresent(Int.self, forKey: .fadeReferenceOffset)
        fadeReferenceLength = try values.decodeIfPresent(Int.self, forKey: .fadeReferenceLength)
        edgeFadeFrames = try values.decodeIfPresent(Int.self, forKey: .edgeFadeFrames)
        edgeProtectionDisabled = try values.decodeIfPresent(Bool.self, forKey: .edgeProtectionDisabled)
        cutFadeInFrames = try values.decodeIfPresent(Int.self, forKey: .cutFadeInFrames)
        cutFadeOutFrames = try values.decodeIfPresent(Int.self, forKey: .cutFadeOutFrames)
    }
    public var end: Int { start + length }
    /// 2 ms at the session rate: the shortest edge fade that reliably hides the
    /// click where audio starts or stops mid-waveform.
    public static let defaultEdgeFadeFrames = 96
    public init(assetID: UUID, name: String, start: Int, offset: Int = 0, length: Int, edgeFadeFrames: Int = Clip.defaultEdgeFadeFrames) {
        self.edgeFadeFrames = max(0, edgeFadeFrames)
        self.id = UUID(); self.assetID = assetID; self.name = name; self.start = max(0, start)
        self.offset = offset; self.length = length
    }
}

public struct TrackSend: Codable, Equatable, Sendable {
    public var trackID: UUID
    public var level: Float
    public init(trackID: UUID, level: Float = 0.25) { self.trackID = trackID; self.level = level }
}

/// Sends are post-fader. A nil output goes to Master. Sidechains use post-fader audio.
public struct TrackRouting: Codable, Equatable, Sendable {
    public var role: String
    public var outputID: UUID?
    public var sends: [TrackSend]
    public var sidechainSourceID: UUID?
    public init(role: String = "audio", outputID: UUID? = nil, sends: [TrackSend] = [], sidechainSourceID: UUID? = nil) {
        self.role = role; self.outputID = outputID; self.sends = sends; self.sidechainSourceID = sidechainSourceID
    }
}

public struct Track: Identifiable, Codable, Equatable, Sendable {
    public var id = UUID()
    public var name: String
    public var color: Int
    public var clips: [Clip] = []
    public var recordingTakes: [RecordingTake] = []
    public var activeTakeID: UUID?
    public var gain: Float = 1
    public var pan: Float = 0
    public var volumeAutomationScale: AutomationValueScale?
    public var mixAutomation: [String: [AutomationPoint]] = [:]
    public var muted = false
    public var solo = false
    public var armed = false
    /// nil follows the session's input. Explicit routes persist by stable Core Audio UID.
    public var inputDeviceUID: String?
    public var inputDeviceName: String?
    public var inputChannel = 0
    public var stereoInput = false
    public var deviceInputChannels: [Int] {
        guard inputChannel >= 0, inputChannel < Int.max else { return [] }
        return stereoInput ? [inputChannel, inputChannel + 1] : [inputChannel]
    }
    public var acceptsClips: Bool { routing == nil || routing?.role == "audio" }
    public var routing: TrackRouting?
    public var realtimeEffects: [RealtimeEffect] = []
    public var referencedAssetIDs: Set<UUID> { (clips + recordingTakes.flatMap(\.clips)).reduce(into: Set<UUID>()) { $0.formUnion($1.referencedAssetIDs) } }
    public init(name: String, color: Int) { self.name = name; self.color = color }

    private enum CodingKeys: String, CodingKey {
        case id, name, color, clips, recordingTakes, activeTakeID, gain, pan, mixAutomation, volumeAutomationScale, muted, solo, armed, inputDeviceUID, inputDeviceName, inputChannel, stereoInput, realtimeEffects, routing
    }
    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        id = try values.decodeIfPresent(UUID.self, forKey: .id) ?? UUID()
        name = try values.decodeIfPresent(String.self, forKey: .name) ?? "Audio"
        color = try values.decodeIfPresent(Int.self, forKey: .color) ?? 0
        clips = try values.decodeIfPresent([Clip].self, forKey: .clips) ?? []
        recordingTakes = try values.decodeIfPresent([RecordingTake].self, forKey: .recordingTakes) ?? []
        activeTakeID = try values.decodeIfPresent(UUID.self, forKey: .activeTakeID)
        gain = try values.decodeIfPresent(Float.self, forKey: .gain) ?? 1
        pan = try values.decodeIfPresent(Float.self, forKey: .pan) ?? 0
        volumeAutomationScale = try values.decodeIfPresent(AutomationValueScale.self, forKey: .volumeAutomationScale)
        mixAutomation = try values.decodeIfPresent([String: [AutomationPoint]].self, forKey: .mixAutomation) ?? [:]
        muted = try values.decodeIfPresent(Bool.self, forKey: .muted) ?? false
        solo = try values.decodeIfPresent(Bool.self, forKey: .solo) ?? false
        armed = try values.decodeIfPresent(Bool.self, forKey: .armed) ?? false
        inputDeviceUID = try values.decodeIfPresent(String.self, forKey: .inputDeviceUID)
        inputDeviceName = try values.decodeIfPresent(String.self, forKey: .inputDeviceName)
        inputChannel = try values.decodeIfPresent(Int.self, forKey: .inputChannel) ?? 0
        stereoInput = try values.decodeIfPresent(Bool.self, forKey: .stereoInput) ?? false
        routing = try values.decodeIfPresent(TrackRouting.self, forKey: .routing)
        realtimeEffects = try values.decodeIfPresent([RealtimeEffect].self, forKey: .realtimeEffects) ?? []
    }
}

public struct Session: Codable, Equatable, Sendable {
    public var version = 5
    public var timing = MusicalTiming.standard
    public var name = "Untitled session"
    public var tracks: [Track] = [Track(name: "Audio 1", color: 0)]
    public static let masterTrackID = UUID(uuidString: "E3E58100-5125-4E6F-B985-10AA11000001")!
    public static var defaultMasterTrack: Track { var t = Track(name: "Master", color: 0); t.id = masterTrackID; return t }
    public var masteringAnalysis: MasteringAnalysis?
    public var masterTrack = Session.defaultMasterTrack
    public init() {}
    private enum CodingKeys: String, CodingKey { case version, name, tracks, masterTrack, masteringAnalysis, timing }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = try c.decodeIfPresent(Int.self, forKey: .version) ?? 1
        name = try c.decodeIfPresent(String.self, forKey: .name) ?? "Untitled session"
        timing = try c.decodeIfPresent(MusicalTiming.self, forKey: .timing) ?? .standard
        guard timing.isValid else { throw DecodingError.dataCorruptedError(forKey: .timing, in: c, debugDescription: "Invalid project tempo or meter") }
        tracks = try c.decodeIfPresent([Track].self, forKey: .tracks) ?? []
        masteringAnalysis = try c.decodeIfPresent(MasteringAnalysis.self, forKey: .masteringAnalysis)
        masterTrack = try c.decodeIfPresent(Track.self, forKey: .masterTrack) ?? Self.defaultMasterTrack
        masterTrack.id = Self.masterTrackID; masterTrack.clips = []; masterTrack.armed = false
    }
    /// Older readers must reject scoped pitch edits instead of silently applying
    /// global tuning or dropping the target curves on a subsequent save.
    public var requiredProjectVersion: Int {
        if (tracks + [masterTrack]).contains(where: { $0.realtimeEffects.contains(where: { $0.kind == .guitarRig }) }) { return max(version, 11) }
        if (tracks + [masterTrack]).contains(where: { $0.routing != nil || $0.realtimeEffects.contains(where: { [.gate, .utility].contains($0.kind) || $0.kind == .equalizer && ($0.parameters.keys.contains(where: { $0.hasPrefix("band") || $0.hasPrefix("dynamic") }) || $0.parameterAutomation.keys.contains(where: { $0.hasPrefix("band") || $0.hasPrefix("dynamic") })) }) }) { return max(version, 10) }
        if timing != .standard { return max(version, 9) }
        let sounds = referencedClips.flatMap { clip in
            ([clip.soundDesign] + (clip.previousStates ?? []).map(\.soundDesign)).compactMap { $0 }
        }
        if sounds.contains(where: { $0.organic.isActive || $0.layers.contains(where: \.hasCustomSound) }) { return max(version, 8) }
        let hasScopedProcessing = tracks.contains { track in
            track.realtimeEffects.contains { !$0.pitchEdits.isEmpty || $0.pitchEditsOnly || !$0.bakedRanges.isEmpty }
        }
        let hasNewInstruments = sounds.contains { p in
            p.instrument != .synthesizer || p.layers.contains(where: { ![SoundDesignPreset.warmKeys, .glassPluck, .roundBass, .widePad, .brightLead, .softDrum].contains($0.preset) })
        } || referencedClips.contains { clip in
            ([clip.drumPattern] + (clip.previousStates ?? []).map(\.drumPattern)).compactMap { $0 }.contains(where: \.usesExtendedModels)
        }
        if masteringAnalysis != nil || masterTrack != Self.defaultMasterTrack || tracks.contains(where: { $0.realtimeEffects.contains { [.limiter, .resonance].contains($0.kind) || $0.kind == .warmth && ($0.parameters["character"] ?? 0) != 0 } }) { return max(version, 7) }
        return max(version, hasNewInstruments ? 6 : hasScopedProcessing ? 5 : version)
    }
    public var referencedClips: [Clip] { tracks.flatMap { $0.clips + $0.recordingTakes.flatMap(\.clips) } }
    public var referencedAssetIDs: Set<UUID> { referencedClips.reduce(into: Set<UUID>()) { $0.formUnion($1.referencedAssetIDs) } }
    public var end: Int { tracks.flatMap(\.clips).map(\.end).max() ?? 0 }
    public mutating func split(trackID: UUID, clipID: UUID, at frame: Int) {
        guard let t = tracks.firstIndex(where: { $0.id == trackID }),
              let c = tracks[t].clips.firstIndex(where: { $0.id == clipID }) else { return }
        let clip = tracks[t].clips[c]
        guard frame > clip.start && frame < clip.end else { return }
        var sourceEnvelope = clip
        if clip.renderEnvelope != nil, let envelope = ClipEnvelope.resolve(track: tracks[t])[clip.id] { sourceEnvelope.renderEnvelope = ClipRenderEnvelope(envelope) }
        var left = sourceEnvelope, right = sourceEnvelope
        left.cutFadeOutFrames = clip.edgeFadeFrames; right.cutFadeInFrames = clip.edgeFadeFrames
        left.length = frame - clip.start
        right.id = UUID(); right.start = frame; right.offset += left.length; right.length -= left.length
        left.preserveFadeEnvelope(from: sourceEnvelope, localOffset: 0)
        right.preserveFadeEnvelope(from: sourceEnvelope, localOffset: left.length)
        left.remember(clip, operation: "Split"); right.remember(clip, operation: "Split")
        tracks[t].clips.replaceSubrange(c...c, with: [left, right])
    }
    public func fragments(trackID: UUID, range: Range<Int>) -> [Clip] {
        guard let track = tracks.first(where: { $0.id == trackID }) else { return [] }
        let envelopes = ClipEnvelope.resolve(track: track)
        return track.clips.compactMap { clip in
            var sourceEnvelope = clip
            if clip.renderEnvelope != nil, let envelope = envelopes[clip.id] { sourceEnvelope.renderEnvelope = ClipRenderEnvelope(envelope) }
            let lo = max(range.lowerBound, clip.start), hi = min(range.upperBound, clip.end)
            guard hi > lo else { return nil }
            var part = clip; part.id = UUID(); part.start = lo - range.lowerBound
            part.offset += lo - clip.start; part.length = hi - lo
            part.preserveFadeEnvelope(from: sourceEnvelope, localOffset: lo - clip.start)
            if lo != clip.start { part.cutFadeInFrames = clip.edgeFadeFrames }
            if hi != clip.end { part.cutFadeOutFrames = clip.edgeFadeFrames }
            return part
        }
    }
    public mutating func delete(trackID: UUID, range: Range<Int>, operation: String = "Delete audio") {
        guard let t = tracks.firstIndex(where: { $0.id == trackID }) else { return }
        let envelopes = ClipEnvelope.resolve(track: tracks[t])
        tracks[t].clips = tracks[t].clips.flatMap { clip -> [Clip] in
            var sourceEnvelope = clip
            if clip.renderEnvelope != nil, let envelope = envelopes[clip.id] { sourceEnvelope.renderEnvelope = ClipRenderEnvelope(envelope) }
            guard clip.start < range.upperBound && clip.end > range.lowerBound else { return [clip] }
            var parts: [Clip] = []
            if clip.start < range.lowerBound { var left = clip; left.length = range.lowerBound - clip.start; left.cutFadeOutFrames = clip.edgeFadeFrames; left.preserveFadeEnvelope(from: sourceEnvelope, localOffset: 0); left.remember(clip, operation: operation); parts.append(left) }
            if clip.end > range.upperBound {
                var right = clip; right.id = UUID(); right.start = range.upperBound
                right.offset += range.upperBound - clip.start; right.length = clip.end - range.upperBound
                right.cutFadeInFrames = clip.edgeFadeFrames; right.preserveFadeEnvelope(from: sourceEnvelope, localOffset: range.upperBound - clip.start); right.remember(clip, operation: operation); parts.append(right)
            }
            return parts
        }
    }
}

public enum Mixer {
    public static func render(session: Session, assets: [UUID: AudioSamples], range: Range<Int>, includeEffectTails: Bool = false, progress: (@Sendable (Double) -> Void)? = nil) -> AudioSamples {
        let length = max(0, range.count)
        var left = Array(repeating: Float(0), count: length), right = left
        guard length > 0 else { return AudioSamples(channels: [left, right]) }
        // A common export range owns the endpoint. Isolated stems must keep
        // processing their decay for the same duration as the full mix.
        let mixer = RealtimeMixer(session: session, assets: assets, startFrame: range.lowerBound, includeEffectTails: includeEffectTails,
                                  renderEndFrame: includeEffectTails ? range.upperBound : nil)
        left.withUnsafeMutableBufferPointer { l in
            right.withUnsafeMutableBufferPointer { r in
                var offset = 0, reportedPercent = -1
                while offset < length {
                    if Task.isCancelled { break }
                    let count = min(8192, length - offset)
                    mixer.render(intoLeft: l.baseAddress!.advanced(by: offset),
                                 intoRight: r.baseAddress!.advanced(by: offset), frameCount: count)
                    offset += count
                    let fraction = Double(offset) / Double(length), percent = Int(fraction * 100)
                    if percent != reportedPercent { reportedPercent = percent; progress?(fraction) }
                }
            }
        }
        return AudioSamples(channels: [left, right])
    }
}
