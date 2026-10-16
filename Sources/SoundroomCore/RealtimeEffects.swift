import Foundation
import Darwin

public enum RealtimeEffectKind: String, CaseIterable, Codable, Identifiable, Sendable {
    case gain, lowPass, highPass, echo, warmth, compressor, room, equalizer, chorus, dither, pitchCorrection, limiter, resonance, vocalEnhancer, sourceIsolator, soundPutty, gate, utility, guitarRig
    public var id: String { rawValue }
    public var name: String {
        switch self {
        case .limiter: return "Master limiter"
        case .resonance: return "Resonance control"
        case .gate: return "Gate / Expander"
        case .guitarRig: return "Guitar rig"
        case .utility: return "Utility / Alignment"
        case .soundPutty: return "Sound Putty · Deep 300"
        case .vocalEnhancer: return "Vocal enhancer"
        case .sourceIsolator: return "Source isolator"
        case .gain: return "Gain"
        case .lowPass: return "Low-pass filter"
        case .echo: return "Echo"
        case .warmth: return "Warmth"
        case .highPass: return "High-pass filter"
        case .compressor: return "Compressor"
        case .room: return "Room reverb"
        case .equalizer: return "Channel EQ"
        case .chorus: return "Chorus"
        case .dither: return "Dither"
        case .pitchCorrection: return "Pitch correction"
        }
    }
    public var defaultAmount: Float {
        switch self {
        case .gain: return 0.5
        case .lowPass: return 0.25
        case .echo: return 0.4
        case .warmth: return 0.3
        case .highPass, .compressor, .room, .equalizer, .chorus, .dither, .pitchCorrection, .limiter, .resonance, .vocalEnhancer, .sourceIsolator, .soundPutty, .gate, .utility, .guitarRig: return 0.5
        }
    }
    public func parameterDisplay(_ amount: Float) -> String {
        let value = normalizedAmount(amount)
        switch self {
        case .gain: return String(format: "%+.1f dB", value * 48 - 24)
        case .lowPass: return String(format: "%.0f Hz", 200 * pow(80, 1 - value))
        case .echo, .warmth, .highPass, .compressor, .room, .equalizer, .chorus, .dither, .pitchCorrection, .limiter, .resonance, .vocalEnhancer, .sourceIsolator, .soundPutty, .gate, .utility, .guitarRig: return String(format: "%.0f%%", value * 100)
        }
    }
}

public struct AutomationPoint: Codable, Equatable, Sendable, Identifiable {
    public var frame: Int
    public var value: Float
    public var curve: AutomationCurve
    public var curveStart: Float
    public var curveEnd: Float
    public var id: Int { frame }
    public init(frame: Int, value: Float, curve: AutomationCurve = .linear, curveStart: Float = 0, curveEnd: Float = 1) {
        self.frame = max(0, frame); self.value = normalizedAmount(value); self.curve = curve
        self.curveStart = normalizedAmount(curveStart)
        self.curveEnd = max(self.curveStart, normalizedAmount(curveEnd))
    }
    private enum CodingKeys: String, CodingKey { case frame, value, curve, curveStart, curveEnd }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        self.init(frame: try c.decode(Int.self, forKey: .frame), value: try c.decode(Float.self, forKey: .value), curve: try c.decodeIfPresent(AutomationCurve.self, forKey: .curve) ?? .linear, curveStart: try c.decodeIfPresent(Float.self, forKey: .curveStart) ?? 0, curveEnd: try c.decodeIfPresent(Float.self, forKey: .curveEnd) ?? 1)
    }
}

public struct RealtimeEffect: Codable, Equatable, Sendable, Identifiable {
    /// Older amount-based algorithms are active only without physical parameter
    /// values or parameter automation. Other insert kinds always use modern DSP.
    public var usesLegacyProcessor: Bool {
        parameters.isEmpty && parameterAutomation.isEmpty && [.gain, .lowPass, .echo, .warmth].contains(kind)
    }
    /// Effective lanes, excluding retained legacy curves after their migration
    /// and unknown parameter keys that the processor does not read.
    public var automatedParameterIDs: Set<String> {
        let known = Set(kind.parameters.map(\.id))
        var keys = Set(parameterAutomation.filter { !$0.value.isEmpty }.map(\.key)).intersection(known)
        let primary: String?
        switch kind {
        case .gain: primary = "gain"
        case .lowPass: primary = "frequency"
        case .echo: primary = "mix"
        case .warmth: primary = "drive"
        default: primary = nil
        }
        if parameters.isEmpty, !automation.isEmpty, let primary,
           parameterAutomation[primary]?.isEmpty != false { keys.insert(primary) }
        return keys
    }
    public var id: UUID
    public var kind: RealtimeEffectKind
    public var amount: Float
    public var enabled: Bool
    public var automation: [AutomationPoint]
    public var parameters: [String: Float]
    public var parameterAutomation: [String: [AutomationPoint]]
    public var selectedAutomationParameter: String?
    public var pitchEdits: [PitchEditRegion]
    public var pitchEditsOnly: Bool
    /// The source already contains this insert's processing in these half-open
    /// timeline ranges. Bypass each stage's wet output with latency-aligned dry.
    public var bakedRanges: [PitchEditRange]
    public init(kind: RealtimeEffectKind, amount: Float? = nil, enabled: Bool = true,
                automation: [AutomationPoint] = [], parameters: [String: Float] = [:],
                parameterAutomation: [String: [AutomationPoint]] = [:], pitchEdits: [PitchEditRegion] = [],
                pitchEditsOnly: Bool = false, bakedRanges: [PitchEditRange] = []) {
        id = UUID(); self.kind = kind
        self.amount = normalizedAmount(amount ?? kind.defaultAmount)
        self.enabled = enabled; self.automation = automation
        self.parameters = parameters.isEmpty && ![.gain, .lowPass, .echo, .warmth].contains(kind) ? kind.defaultParameters : parameters
        self.parameterAutomation = parameterAutomation
        self.selectedAutomationParameter = self.parameters.isEmpty ? nil : kind.parameters.first?.id
        self.pitchEdits = PitchEditRegions.sanitized(pitchEdits)
        self.pitchEditsOnly = pitchEditsOnly
        self.bakedRanges = PitchEditRegions.sanitizedRanges(bakedRanges)
    }

    private enum CodingKeys: String, CodingKey { case id, kind, amount, enabled, automation, parameters, parameterAutomation, selectedAutomationParameter, pitchEdits, pitchEditsOnly, bakedRanges }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decodeIfPresent(UUID.self, forKey: .id) ?? UUID()
        kind = try c.decode(RealtimeEffectKind.self, forKey: .kind)
        amount = normalizedAmount(try c.decodeIfPresent(Float.self, forKey: .amount) ?? kind.defaultAmount)
        enabled = try c.decodeIfPresent(Bool.self, forKey: .enabled) ?? true
        automation = try c.decodeIfPresent([AutomationPoint].self, forKey: .automation) ?? []
        parameters = try c.decodeIfPresent([String: Float].self, forKey: .parameters) ?? [:]
        parameterAutomation = try c.decodeIfPresent([String: [AutomationPoint]].self, forKey: .parameterAutomation) ?? [:]
        selectedAutomationParameter = try c.decodeIfPresent(String.self, forKey: .selectedAutomationParameter) ?? (parameters.isEmpty ? nil : kind.parameters.first?.id)
        if let key = selectedAutomationParameter, !kind.parameters.contains(where: { $0.id == key }) { selectedAutomationParameter = parameters.isEmpty ? nil : kind.parameters.first?.id }
        pitchEdits = PitchEditRegions.sanitized(try c.decodeIfPresent([PitchEditRegion].self, forKey: .pitchEdits) ?? [])
        pitchEditsOnly = try c.decodeIfPresent(Bool.self, forKey: .pitchEditsOnly) ?? false
        bakedRanges = PitchEditRegions.sanitizedRanges(try c.decodeIfPresent([PitchEditRange].self, forKey: .bakedRanges) ?? [])
    }
    public var activeAutomation: [AutomationPoint] {
        get { selectedAutomationParameter.map { parameterAutomation[$0] ?? [] } ?? automation }
        set { if let key = selectedAutomationParameter { parameterAutomation[key] = newValue } else { automation = newValue } }
    }
    public var activeAutomationAmount: Float {
        guard let key = selectedAutomationParameter, let descriptor = kind.parameters.first(where: { $0.id == key }) else { return normalizedAmount(amount) }
        return descriptor.normalized(parameters[key] ?? descriptor.defaultValue)
    }
    public func automationDisplay(_ normalized: Float) -> String {
        guard let key = selectedAutomationParameter, let descriptor = kind.parameters.first(where: { $0.id == key }) else { return kind.parameterDisplay(normalized) }
        return descriptor.display(descriptor.physical(normalized))
    }
    /// Old projects retain the original one-knob signal path and amount automation.
    public func value(at frame: Int) -> Float { Self.interpolated(automation, at: frame, baseline: normalizedAmount(amount)) }
    /// Normalized automation value for one parameter, with its physical base as fallback.
    public func value(at frame: Int, parameter: String) -> Float {
        guard let descriptor = kind.parameters.first(where: { $0.id == parameter }) else { return 0 }
        let curve = parameterAutomation[parameter] ?? []
        let base = parametersForEditing[parameter] ?? descriptor.defaultValue
        if parameters.isEmpty, curve.isEmpty {
            let primary: String?
            switch kind {
            case .gain: primary = "gain"
            case .lowPass: primary = "frequency"
            case .echo: primary = "mix"
            case .warmth: primary = "drive"
            default: primary = nil
            }
            if parameter == primary {
                var sample = self; sample.amount = value(at: frame); sample.automation = []
                return descriptor.normalized(sample.parametersForEditing[parameter] ?? base)
            }
        }
        return Self.interpolated(curve, at: frame, baseline: descriptor.normalized(base))
    }
    public func parameterValue(_ key: String, at frame: Int) -> Float {
        guard let descriptor = kind.parameters.first(where: { $0.id == key }) else { return 0 }
        return descriptor.physical(value(at: frame, parameter: key))
    }
    static func interpolated(_ points: [AutomationPoint], at frame: Int, baseline: Float) -> Float {
        AutomationInterpolation.value(points, at: frame, baseline: normalizedAmount(baseline))
    }
    /// A bounded -60 dB decay estimate for explicit tail-aware playback/export.
    public var tailFrames: Int {
        guard enabled else { return 0 }
        if kind == .echo {
            let milliseconds = parameters.isEmpty ? Float(250) : maximumParameterValue("time")
            let feedback = parameters.isEmpty ? Float(0.4) : maximumParameterValue("feedback")
            let repeats = feedback > 0 ? ceil(log(0.001) / log(feedback)) : 1
            return Int(min(30, milliseconds / 1000 * max(1, repeats)) * Float(sessionRate))
        }
        if kind == .guitarRig {
            var seconds: Float = maximumParameterValue("pedal") > 0 || maximumParameterValue("amp") > 0 || maximumParameterValue("cabinet") > 0 ? 0.25 : 0
            if maximumParameterValue("modulation") > 0 && maximumParameterValue("modMix") > 0 { seconds += 0.03 }
            if maximumParameterValue("delayMix") > 0 {
                let feedback = maximumParameterValue("delayFeedback")
                seconds += maximumParameterValue("delayTime") / 1000 * (feedback > 0 ? max(1, ceil(log(0.001) / log(feedback))) : 1)
            }
            if maximumParameterValue("reverbMix") > 0 { seconds += maximumParameterValue("reverbDecay") + 0.15 }
            return Int(min(30, seconds) * Float(sessionRate))
        }
        if kind == .soundPutty {
            let minimum = SoundPuttyShape(parameters: parameters).rows[2].min() ?? 0
            return Int(min(30, 6.907756 * Double(maximumParameterValue("decay")) * exp(-2 * minimum)) * sessionRate)
        }
        if kind == .room {
            let dense = maximumParameterValue("character") >= 0.5
            return Int(min(30, maximumParameterValue("decay") + maximumParameterValue("predelay") / 1000 + (dense ? 0.15 : 0)) * Float(sessionRate))
        }
        if kind == .utility { return Int(max(maximumParameterValue("delayL"), maximumParameterValue("delayR"))) }
        if kind == .chorus {
            let delay = (maximumParameterValue("delay") + maximumParameterValue("depth")) / 1000
            let feedback = maximumParameterValue("feedback")
            let repeats = feedback > 0 ? max(1, ceil(log(0.001) / log(feedback))) : 1
            return Int(min(30, delay * repeats) * Float(sessionRate))
        }
        return 0
    }
    private func maximumParameterValue(_ key: String) -> Float {
        guard let descriptor = kind.parameters.first(where: { $0.id == key }) else { return 0 }
        return (parameterAutomation[key] ?? []).reduce(descriptor.clamped(parameters[key] ?? descriptor.defaultValue)) { max($0, descriptor.physical($1.value)) }
    }
    public func process(_ audio: AudioSamples, startFrame: Int = 0, includeTail: Bool = false,
                        progress: (@Sendable (Double) -> Void)? = nil) -> AudioSamples {
        guard audio.count > 0, audio.channelCount > 0, !Task.isCancelled else { return audio }
        let outputCount = audio.count + (includeTail ? tailFrames : 0)
        let count = outputCount + latencyFrames
        let pairs = (audio.channelCount + 1) / 2
        // Guitar rigs have a stereo output even for a mono DI source. Retain
        // the chorus/room right channel when applying the effect to audio.
        let outputChannels = kind == .guitarRig && audio.channelCount == 1 ? 2 : audio.channelCount
        var channels: [[Float]] = [], reportedPercent = -1
        for pair in stride(from: 0, to: audio.channelCount, by: 2) {
            guard !Task.isCancelled else { return audio }
            let processor = EffectProcessor(effect: self)
            var outputLeft = Array(repeating: Float(0), count: outputCount)
            var outputRight = pair + 1 < outputChannels ? Array(repeating: Float(0), count: outputCount) : []
            var offset = 0
            while offset < count {
                // Only this bounded input block is materialized, even for mapped sources.
                guard !Task.isCancelled else { return audio }
                let block = min(8192, count - offset)
                var left: [Float], right: [Float]
                if offset < audio.count {
                    let end = min(audio.count, offset + block)
                    let input = audio.readFrames(offset..<end, channels: [pair, min(pair + 1, audio.channelCount - 1)])
                    left = input[0]; right = input[1]
                    left.append(contentsOf: repeatElement(0, count: block - left.count))
                    right.append(contentsOf: repeatElement(0, count: block - right.count))
                } else {
                    left = Array(repeating: 0, count: block); right = left
                }
                left.withUnsafeMutableBufferPointer { l in right.withUnsafeMutableBufferPointer { r in
                    processor.process(left: l.baseAddress!, right: r.baseAddress!, count: block, position: PitchEditRange.clampedFrame(startFrame) + offset)
                } }
                let first = max(0, latencyFrames - offset), last = min(block, count - offset)
                if last > first {
                    let destination = (offset + first - latencyFrames)..<(offset + last - latencyFrames)
                    outputLeft.replaceSubrange(destination, with: left[first..<last])
                    if pair + 1 < outputChannels { outputRight.replaceSubrange(destination, with: right[first..<last]) }
                }
                offset += block
                let fraction = Double((pair / 2) * count + offset) / Double(pairs * count)
                let percent = Int(fraction * 100)
                if percent != reportedPercent { reportedPercent = percent; progress?(fraction) }
            }
            channels.append(outputLeft)
            if pair + 1 < outputChannels { channels.append(outputRight) }
        }
        return AudioSamples(channels: channels)
    }

}

private func normalizedAmount(_ value: Float) -> Float {
    value.isFinite ? max(0, min(1, value)) : 0.5
}

/// Export/range rendering reconstructs canonical state. Interactive transport
/// uses a short local pre-roll so start latency is independent of song length.
/// Delay/reverb history from before that window restarts at a new playback cue.
public enum RealtimeSeekPolicy: Sendable {
    case continuousHistory
    case interactive
    public static let interactiveHistoryFrames = Int(sessionRate * 0.1)
}

/// Stateful stereo processing shared by playback and offline export. Callers must
/// serialize render and update. The default seek policy initializes from frame
/// zero so range exports match continuous playback. Transport can opt into a
/// bounded local pre-roll without changing export state or source samples.
/// Effects process before track gain/pan; source samples are never modified.
public final class RealtimeMixer {
    public private(set) var position: Int
    public private(set) var end: Int
    public var completed: Bool { position >= end }
    private var routed: RoutedRealtimeGraph?
    private var routingRequested = false
    private var tracks: [TrackProcessor] = []
    private var master: MasterBusProcessor!
    private var outputTransition: MasterOutputTransition?
    private let loopRange: Range<Int>?
    private var hasSolo = false
    private let includeEffectTails: Bool
    private let renderEndFrame: Int?
    private var sourceCache: RealtimeSourceCache?
    private let masterRetirementMailbox = MasterBusRetirementMailbox()
    private let alignedRetirementMailbox = AlignedGraphRetirementMailbox()
    public private(set) var seekWarmupFrames = 0
    public private(set) var restoredCheckpointFrame: Int?
    public var sourceReadUnderruns: Int64 { sourceCache?.missedBlocks ?? 0 }
    /// Call on the preparation worker, after stateful seek warm-up.
    public func prepareRealtimeSources(session: Session, assets: [UUID: AudioSamples], loopRange: Range<Int>? = nil) {
        guard assets.values.contains(where: \.isStreamed) else { return }
        let cache = RealtimeSourceCache(session: session, assets: assets, startFrame: position + (routed?.latency ?? 0), loopRange: loopRange)
        sourceCache = cache
        routed?.sourceCache = cache
        for track in tracks { track.sourceCache = cache }
    }

    public init(session: Session, assets: [UUID: AudioSamples], startFrame: Int = 0, includeEffectTails: Bool = false, renderEndFrame: Int? = nil, seekCache: DSPSeekCache? = nil, isCancelled: (() -> Bool)? = nil, loopRange: Range<Int>? = nil, seekPolicy: RealtimeSeekPolicy = .continuousHistory) {
        self.loopRange = loopRange
        self.includeEffectTails = includeEffectTails
        self.renderEndFrame = renderEndFrame.map { max(0, $0) }
        let origin = loopRange?.lowerBound ?? min(0, startFrame)
        position = seekPolicy == .interactive && startFrame > origin ? max(origin, startFrame - RealtimeSeekPolicy.interactiveHistoryFrames) : origin
        end = session.end
        if session.tracks.contains(where: { $0.routing != nil }) {
            routingRequested = true
            routed = try? RoutedRealtimeGraph(session: session, assets: assets, startFrame: startFrame, loopRange: loopRange, historyStart: seekPolicy == .interactive ? position : nil, isCancelled: isCancelled)
            end = renderEndFrame ?? session.end + (includeEffectTails ? Self.tailFrames(session: session) : 0)
            position = startFrame
            return
        }
        let primeFrame = position
        let initial = PreparedRealtimeMixerUpdate(session: session, assets: assets, loopRange: loopRange, primeFrame: primeFrame)
        applyPreparedUpdate(initial); initial.releaseRetiredResources()
        // Prime the complete output bus against actual summed audio. Tracks own
        // their insert look-ahead; latency-bearing output inserts own an extra
        // look-ahead budget. With an empty bus this path performs no extra work.
        let savedPosition = position
        if master.lookAheadFrames > 0 {
            position -= master.lookAheadFrames
            let primeL = UnsafeMutablePointer<Float>.allocate(capacity: master.lookAheadFrames)
            let primeR = UnsafeMutablePointer<Float>.allocate(capacity: master.lookAheadFrames)
            render(intoLeft: primeL, intoRight: primeR, frameCount: master.lookAheadFrames)
            primeL.deallocate(); primeR.deallocate(); position = savedPosition
        }
        let needsHistory = !session.masterTrack.realtimeEffects.isEmpty || session.tracks.contains { track in
            track.realtimeEffects.contains {
                $0.kind != .gain && $0.kind != .dither && ($0.kind != .warmth || !$0.parameters.isEmpty || !$0.parameterAutomation.isEmpty)
            }
        }
        if startFrame > position && needsHistory {
            // Locally reset states must never enter the canonical checkpoint
            // cache: exports and aligned live edits rely on exact history.
            let historyCache = seekPolicy == .continuousHistory ? seekCache : nil
            let key = historyCache?.key(session: session, assets: assets, includeTails: includeEffectTails, end: renderEndFrame, loopRange: loopRange)
            if let cache = historyCache, let key, let checkpoint = cache.nearest(key: key, frame: min(startFrame, end)) {
                for track in tracks { for effect in track.effects {
                    if let saved = checkpoint.tracks[track.track.id]?[effect.effect.id] { effect.restoreSeek(saved) }
                } }
                if let bus = checkpoint.master { master = bus.deepCopy() }
                for track in tracks { track.primeDryBaseline(at: checkpoint.frame + master.lookAheadFrames) }
                position = checkpoint.frame; restoredCheckpointFrame = checkpoint.frame
            }
            let warmupStart = position
            let interval = historyCache?.intervalFrames ?? Int.max
            var nextCheckpoint = position.addingReportingOverflow(interval - position % interval).partialValue
            // Allocate once here, never on the audio callback for an ordinary seek.
            let count = 8192
            let left = UnsafeMutablePointer<Float>.allocate(capacity: count)
            let right = UnsafeMutablePointer<Float>.allocate(capacity: count)
            defer { left.deallocate(); right.deallocate() }
            var warmedBlocks = 0
            while position < min(startFrame, end) {
                if Task.isCancelled || isCancelled?() == true { position = startFrame; return }
                render(intoLeft: left, intoRight: right, frameCount: min(count, min(startFrame, end) - position, nextCheckpoint - position))
                if position == nextCheckpoint, let cache = historyCache, let key {
                    var saved: [UUID: [UUID: EffectSeekState]] = [:]
                    for track in tracks { for effect in track.effects { saved[track.track.id, default: [:]][effect.effect.id] = effect.seekSnapshot() } }
                    cache.store(key: key, checkpoint: DSPSeekCheckpoint(frame: position, tracks: saved, master: master.effects.isEmpty ? nil : master.deepCopy()))
                    let next = nextCheckpoint.addingReportingOverflow(interval); nextCheckpoint = next.overflow ? Int.max : next.partialValue
                }
                warmedBlocks += 1
                if warmedBlocks % 32 == 0 { for audio in assets.values { audio.discardReadPages() } }
            }
            seekWarmupFrames = position - warmupStart
            position = startFrame
        } else {
            position = startFrame
        }
    }

    /// Resolve source media and allocate graph/configuration storage away from the render thread.
    public static func prepareUpdate(session: Session, assets: [UUID: AudioSamples], realtimeStartFrame: Int? = nil, loopRange: Range<Int>? = nil) -> PreparedRealtimeMixerUpdate {
        PreparedRealtimeMixerUpdate(session: session, assets: assets, realtimeStartFrame: realtimeStartFrame, loopRange: loopRange)
    }

    /// A layout change requires independent state: even equal total latency can
    /// change the input timeline of an existing processor when inserts move.
    public static func requiresAlignedUpdate(from previous: Session, to incoming: Session) -> Bool {
        (previous.tracks.contains(where: { $0.routing != nil }) || incoming.tracks.contains(where: { $0.routing != nil })) && routedLayoutChanged(previous, incoming) || MasterBusProcessor.layoutChanged(previous.masterTrack, incoming.masterTrack) || incoming.tracks.contains { track in
            guard let old = previous.tracks.first(where: { $0.id == track.id }) else { return track.realtimeEffects.contains { $0.latencyFrames > 0 } }
            return TrackProcessor.layoutChanged(from: old, to: track)
        }
    }
    private static func routedLayoutChanged(_ a: Session, _ b: Session) -> Bool {
        guard a.tracks.count == b.tracks.count, a.timing == b.timing else { return true }
        for (a,b) in zip(a.tracks + [a.masterTrack],b.tracks + [b.masterTrack]) {
            if a.id != b.id || a.clips != b.clips || a.routing?.role != b.routing?.role || a.routing?.outputID != b.routing?.outputID || a.routing?.sidechainSourceID != b.routing?.sidechainSourceID || a.routing?.sends.map(\.trackID) != b.routing?.sends.map(\.trackID) || a.realtimeEffects.count != b.realtimeEffects.count { return true }
            for (a,b) in zip(a.realtimeEffects,b.realtimeEffects) { if a.id != b.id || a.kind != b.kind || a.latencyFrames != b.latencyFrames { return true } }
        }
        return a.tracks.contains(where: { $0.routing != nil }) != b.tracks.contains(where: { $0.routing != nil })
    }
    /// Worker-only. Prepare canonical state at the exact future output frame.
    public static func prepareAlignedUpdate(session: Session, assets: [UUID: AudioSamples], previousSession: Session,
        handoffFrame: Int, loopRange: Range<Int>? = nil, seekCache: DSPSeekCache? = nil,
        isCancelled: (() -> Bool)? = nil) -> PreparedRealtimeMixerUpdate? {
        if session.tracks.contains(where: { $0.routing != nil }) || previousSession.tracks.contains(where: { $0.routing != nil }) {
            let prepared = PreparedRealtimeMixerUpdate(session: session, assets: assets, realtimeStartFrame: handoffFrame, loopRange: loopRange)
            if session.tracks.contains(where: { $0.routing != nil }) {
                prepared.routed = try? RoutedRealtimeGraph(session: session, assets: assets, startFrame: handoffFrame, loopRange: loopRange, isCancelled: isCancelled)
                prepared.alignedRouting = true
            } else {
                let warmed = RealtimeMixer(session: session, assets: assets, startFrame: handoffFrame, includeEffectTails: true, isCancelled: isCancelled, loopRange: loopRange)
                prepared.tracks = warmed.tracks; prepared.master = warmed.master; prepared.alignedMaster = true
            }
            guard !Task.isCancelled, isCancelled?() != true else { return nil }
            prepared.handoffFrame = handoffFrame
            return prepared
        }
        var changed = Set<UUID>()
        for track in session.tracks {
            if let old = previousSession.tracks.first(where: { $0.id == track.id }), TrackProcessor.layoutChanged(from: old, to: track) { changed.insert(track.id) }
            else if !previousSession.tracks.contains(where: { $0.id == track.id }), track.realtimeEffects.contains(where: { $0.latencyFrames > 0 }) { changed.insert(track.id) }
        }
        let masterChanged = MasterBusProcessor.layoutChanged(previousSession.masterTrack, session.masterTrack)
        let budgetChanged = MasterBusProcessor.lookAheadFrames(for: previousSession.masterTrack) != MasterBusProcessor.lookAheadFrames(for: session.masterTrack)
        if budgetChanged { changed = Set(session.tracks.map(\.id)) }
        var subset = session; subset.tracks = masterChanged ? session.tracks : session.tracks.filter { changed.contains($0.id) }
        let warmed = RealtimeMixer(session: subset, assets: assets, startFrame: handoffFrame,
            includeEffectTails: true, seekCache: seekCache, isCancelled: isCancelled, loopRange: loopRange)
        guard isCancelled?() != true, !Task.isCancelled else { return nil }
        let prepared = PreparedRealtimeMixerUpdate(session: session, assets: assets, realtimeStartFrame: handoffFrame, loopRange: loopRange)
        for i in prepared.tracks.indices {
            if let track = warmed.tracks.first(where: { $0.track.id == prepared.tracks[i].track.id }) { prepared.tracks[i] = track }
        }
        if masterChanged { prepared.master = warmed.master; prepared.alignedMaster = true }
        prepared.wholeOutputAligned = budgetChanged
        prepared.handoffFrame = handoffFrame; prepared.alignedTrackIDs = changed
        return prepared
    }

    /// Synchronous convenience for offline callers. Live playback prepares on its worker queue.
    public func update(session: Session, assets: [UUID: AudioSamples]) {
        let prepared: PreparedRealtimeMixerUpdate
        if routingRequested || session.tracks.contains(where: { $0.routing != nil }) {
            if let routed, !Self.routedLayoutChanged(routed.session, session) { prepared = Self.prepareUpdate(session: session, assets: assets, realtimeStartFrame: position, loopRange: loopRange) }
            else { guard let aligned = Self.prepareAlignedUpdate(session: session, assets: assets, previousSession: routed?.session ?? session, handoffFrame: position, loopRange: loopRange) else { return }; prepared = aligned }
        } else if MasterBusProcessor.layoutChanged(master.track, session.masterTrack) {
            var previous = session; previous.tracks = tracks.map(\.track); previous.masterTrack = master.track
            guard let aligned = Self.prepareAlignedUpdate(session: session, assets: assets, previousSession: previous,
                handoffFrame: position, loopRange: loopRange) else { return }
            prepared = aligned
        } else {
            prepared = Self.prepareUpdate(session: session, assets: assets, realtimeStartFrame: position, loopRange: loopRange)
        }
        applyPreparedUpdate(prepared)
        prepared.releaseRetiredResources()
    }

    /// Consume exactly once at a render-block boundary. This adopts state references and swaps
    /// already-allocated graphs; retirement must be drained by the caller away from audio.
    @discardableResult public func applyPreparedUpdate(_ prepared: PreparedRealtimeMixerUpdate, smoothManualChanges: Bool = false) -> Bool {
        guard !prepared.applied, prepared.handoffFrame == nil || prepared.handoffFrame == position else { return false }
        if routingRequested || prepared.routingRequested {
            guard !prepared.routingRequested || prepared.routed != nil else { return false }
            if let old = routed, let next = prepared.routed, !prepared.alignedRouting {
                guard next.canAdopt(from: old) else { return false }
                next.adopt(from: old, smooth: smoothManualChanges, discardedStates:&prepared.discardedStates, discardedDelays:&prepared.discardedRoutingDelays)
            } else if prepared.routingRequested && !prepared.alignedRouting && routed == nil { return false }
            if smoothManualChanges, prepared.alignedRouting, let old = routed, let next = prepared.routed {
                prepared.retiredSecondaryRouting = next.beginTransition(from:old)
            }
            prepared.retiredRouting = routed; routed = prepared.routed; routingRequested = prepared.routingRequested
            if routingRequested {
                prepared.retiredTracks = tracks; tracks = []
                prepared.retiredMaster = master; master = nil
                prepared.retiredOutputTransition = outputTransition; outputTransition = nil
                prepared.applied = true
                end = renderEndFrame ?? prepared.sourceEnd + (includeEffectTails ? prepared.tail : 0)
                if let cache = prepared.sourceCache { sourceCache = cache }; routed?.sourceCache = sourceCache
                return true
            }
        }
        if let master, MasterBusProcessor.layoutChanged(master.track, prepared.master.track), !prepared.alignedMaster { return false }
        prepared.applied = true
        prepared.retiredTracks = tracks
        var spareIndex = 0, retiredGraphIndex = 0
        if prepared.wholeOutputAligned, let oldMaster = master {
            prepared.outputTransition.configure(tracks: tracks, master: oldMaster, hasSolo: hasSolo, loopRange: loopRange, predecessor: outputTransition)
            outputTransition = prepared.outputTransition
        }
        for incoming in prepared.tracks {
            // A bus budget change moves every upstream input timeline. Keep all
            // newly warmed track histories independent of the retiring output.
            if prepared.wholeOutputAligned { spareIndex += incoming.effects.count; continue }
            if let previous = tracks.first(where: { $0.track.id == incoming.track.id }) {
                if prepared.alignedTrackIDs.contains(incoming.track.id) {
                    incoming.adoptAlignedTransition(from: previous, discarded: &prepared.discardedGraphs, discardedIndex: &retiredGraphIndex)
                    spareIndex += incoming.effects.count
                    continue
                }
                if smoothManualChanges { incoming.prepareLiveTransition(from: previous) }
                for processor in incoming.effects {
                    if let prior = previous.effects.first(where: { $0.effect.id == processor.effect.id && $0.effect.kind == processor.effect.kind && $0.usesLegacy == processor.usesLegacy }) {
                        prepared.discardedStates[spareIndex] = processor.state
                        processor.state = prior.state
                        if smoothManualChanges { processor.prepareLiveTransition(from: prior) }
                    }
                    spareIndex += 1
                }
                if smoothManualChanges {
                    incoming.transitionChain.prepareLiveTransition(from: previous.transitionChain)
                    incoming.carryAlignedRetirements(from: previous, discarded: &prepared.discardedGraphs, discardedIndex: &retiredGraphIndex)
                }
            } else { spareIndex += incoming.effects.count }
        }
        if let previous = master {
            prepared.retiredMaster = previous
            if !prepared.wholeOutputAligned {
                prepared.master.adopt(from: previous, aligned: prepared.alignedMaster, smooth: smoothManualChanges,
                                      discarded: &prepared.discardedStates, index: &spareIndex)
            }
        }
        master = prepared.master
        tracks = prepared.tracks
        if let cache = prepared.sourceCache { sourceCache = cache }
        for track in tracks { track.sourceCache = sourceCache }
        let tail = includeEffectTails ? prepared.tail : 0
        let (newEnd, overflow) = prepared.sourceEnd.addingReportingOverflow(tail)
        var targetEnd = overflow ? Int.max : newEnd
        if smoothManualChanges && targetEnd < end && position < end {
            // Bypassing the last tail-producing insert must finish its wet/dry ramp
            // before transport completion truncates the old audible decay.
            let finish = position.addingReportingOverflow(LiveRamp.frameCount)
            targetEnd = max(targetEnd, min(end, finish.overflow ? Int.max : finish.partialValue))
        }
        if smoothManualChanges {
            var remaining = 0
            for track in tracks { remaining = max(remaining, track.transitionChain.remainingTailFrames, track.alignedTailFrames) }
            let finish = position.addingReportingOverflow(remaining)
            targetEnd = max(targetEnd, finish.overflow ? Int.max : finish.partialValue)
        }
        // Loop/selection playback owns an exact exclusive endpoint independent of
        // later clip edits or effect-tail changes. Empty portions still render silence.
        end = renderEndFrame ?? targetEnd
        hasSolo = prepared.hasSolo
        return true
    }

    /// Access only from the serialized graph owner. The meter itself is thread safe.
    public func effectMeter(id: UUID) -> EffectMeter? {
        if routingRequested { return routed?.meter(id) }
        for track in tracks {
            if let processor = track.effects.first(where: { $0.effect.id == id }) { return processor.meter }
        }
        return master.effects.first { $0.effect.id == id }?.meter
    }

    public static func tailFrames(session: Session) -> Int {
        if session.tracks.contains(where: { $0.routing != nil }), let plan = try? TrackRoutingPlan(session: session) {
            let nodes = session.tracks + [session.masterTrack]
            var tail = [Int](repeating: 0, count: nodes.count)
            for i in plan.order {
                tail[i] += nodes[i].realtimeEffects.reduce(0) { $0 + $1.tailFrames }
                for edge in plan.edges where edge.from == i && !edge.detector { tail[edge.to] = max(tail[edge.to],tail[i]) }
            }
            return min(Int(sessionRate * 30),tail.last ?? 0)
        }
        return min(Int(sessionRate * 30), (session.tracks.map { min(Int(sessionRate * 30), $0.realtimeEffects.reduce(0) { $0 + $1.tailFrames }) }.max() ?? 0) + session.masterTrack.realtimeEffects.reduce(0) { $0 + $1.tailFrames })
    }

    /// Reserve outside the audio callback when a device uses unusually large blocks.
    public func releaseCompletedGraphResources() { alignedRetirementMailbox.releaseOnWorker(); masterRetirementMailbox.releaseOnWorker() }

    /// Callback-only pre-handoff phase. Superseding edits fade the previous tail
    /// before adding another whole graph, keeping at most two DSP graphs per track.
    public func beginAlignedRetirementFade(_ prepared: PreparedRealtimeMixerUpdate) {
        for track in tracks where prepared.alignedTrackIDs.contains(track.track.id) {
            track.fadeAlignedRetirements()
        }
    }
    public func hasAlignedRetirements(_ prepared: PreparedRealtimeMixerUpdate) -> Bool {
        tracks.contains { prepared.alignedTrackIDs.contains($0.track.id) && $0.alignedTailFrames > 0 }
    }

    public func reserveCapacity(frameCount: Int) {
        guard !routingRequested else { return }
        for track in tracks { track.reserveCapacity(frameCount: frameCount) }; master.reserveCapacity(frameCount)
    }

    /// The serialized render owner may wrap a loop without reconstructing histories.
    /// Automation cursors detect the backward frame jump on the next sample.
    public func rewindForLoop(to frame: Int) { position = max(0, frame) }

    public func render(intoLeft left: UnsafeMutablePointer<Float>, intoRight right: UnsafeMutablePointer<Float>, frameCount: Int) {
        guard frameCount > 0 else { return }
        if routingRequested {
            left.update(repeating: 0, count: frameCount); right.update(repeating: 0, count: frameCount)
            let active = max(0, min(frameCount, end-position)); var offset = 0
            while offset < active { let count = min(8192,active-offset); routed?.render(left: left+offset, right: right+offset, count: count); offset += count }
            for i in 0..<frameCount { if !left[i].isFinite { left[i] = 0 }; if !right[i].isFinite { right[i] = 0 } }
            position += frameCount; return
        }
        sourceCache?.advance(to: position + master.lookAheadFrames)
        left.update(repeating: 0, count: frameCount); right.update(repeating: 0, count: frameCount)
        let activeCount = max(0, min(frameCount, end - position))
        if activeCount > 0 {
            var offset = 0
            while offset < activeCount {
                let block = min(8192, activeCount - offset)
                for processor in tracks {
                    processor.render(at: position + offset + master.lookAheadFrames, count: block, left: left + offset, right: right + offset,
                                     audible: !processor.track.muted && (!hasSolo || processor.track.solo))
                }
                master.process(left: left + offset, right: right + offset, count: block, sourcePosition: position + offset + master.lookAheadFrames, loopRange: loopRange)
                outputTransition?.blend(intoLeft: left + offset, right: right + offset, count: block, position: position + offset)
                offset += block
            }
        }
        // Preserve floating point headroom but keep invalid imported values out of the device.
        for i in 0..<frameCount {
            if !left[i].isFinite { left[i] = 0 }
            if !right[i].isFinite { right[i] = 0 }
        }
        if let retired = outputTransition, retired.completed {
            masterRetirementMailbox.beginPublishing()
            if masterRetirementMailbox.retire(retired) { outputTransition = nil }
            withExtendedLifetime(retired) { masterRetirementMailbox.endPublishing() }
        }
        master.retireCompleted(to: masterRetirementMailbox)
        for track in tracks { track.retireCompletedGraphs(to: alignedRetirementMailbox) }
        position += frameCount
    }
}

/// A one-use graph handoff. After publication the producer must not mutate it.
/// DSP state is only accessed by the serialized render owner; retirement only releases references.
public final class PreparedRealtimeMixerUpdate: @unchecked Sendable {
    fileprivate var routed: RoutedRealtimeGraph?
    fileprivate var retiredRouting: RoutedRealtimeGraph?
    fileprivate var retiredSecondaryRouting: RoutedRealtimeGraph?
    fileprivate var discardedRoutingDelays: [AnyObject?] = []
    fileprivate var retiredOutputTransition: MasterOutputTransition?
    fileprivate var routingRequested = false
    fileprivate var alignedRouting = false
    fileprivate var tracks: [TrackProcessor]
    fileprivate var master: MasterBusProcessor
    fileprivate var retiredMaster: MasterBusProcessor?
    fileprivate var alignedMaster = false
    fileprivate var wholeOutputAligned = false
    fileprivate let outputTransition = MasterOutputTransition()
    fileprivate let sourceEnd: Int
    fileprivate let tail: Int
    fileprivate let hasSolo: Bool
    fileprivate let sourceCache: RealtimeSourceCache?
    fileprivate var retiredTracks: [TrackProcessor] = []
    fileprivate var discardedStates: [EffectState?]
    public fileprivate(set) var handoffFrame: Int?
    fileprivate var alignedTrackIDs = Set<UUID>()
    fileprivate var discardedGraphs: [AlignedRetiredTrack?]
    fileprivate var applied = false
    fileprivate init(session: Session, assets: [UUID: AudioSamples], realtimeStartFrame: Int? = nil, loopRange: Range<Int>? = nil, primeFrame: Int? = nil) {
        let routedLatency = session.tracks.contains(where: { $0.routing != nil }) ? ((try? TrackRoutingPlan(session:session))?.outputLatency.last ?? 0) : 0
        sourceCache = realtimeStartFrame.flatMap { frame in assets.values.contains(where: \.isStreamed) ? RealtimeSourceCache(session: session, assets: assets, startFrame: frame + routedLatency, loopRange: loopRange) : nil }
        let preparedMaster = MasterBusProcessor(track: session.masterTrack)
        master = preparedMaster
        let sourceFrame = (realtimeStartFrame ?? 0) + preparedMaster.lookAheadFrames
        routingRequested = session.tracks.contains(where: { $0.routing != nil })
        if routingRequested { routed = try? RoutedRealtimeGraph(session: session, assets: assets, startFrame: realtimeStartFrame ?? 0, loopRange: loopRange, warmHistory: false) }
        tracks = routingRequested ? [] : session.tracks.map { TrackProcessor(track: $0, assets: assets, primeFrame: primeFrame ?? sourceFrame, loopRange: loopRange, outputLatencyFrames: preparedMaster.lookAheadFrames) }
        discardedRoutingDelays = Array(repeating:nil,count:routed?.retirementSlotCount ?? 0)
        sourceEnd = session.end; tail = RealtimeMixer.tailFrames(session: session)
        hasSolo = session.tracks.contains(where: \.solo)
        discardedStates = Array(repeating: nil, count: session.tracks.reduce(session.masterTrack.realtimeEffects.count) { $0 + $1.realtimeEffects.count })
        discardedGraphs = Array(repeating: nil, count: session.tracks.count * TrackProcessor.retirementCapacity)
    }
    /// Call on a worker after handoff. The active mixer retains all adopted state.
    public func releaseRetiredResources() {
        retiredMaster = nil; retiredRouting = nil; retiredSecondaryRouting = nil; retiredOutputTransition = nil
        discardedRoutingDelays = []
        guard applied else { return }
        for graph in discardedGraphs { graph?.releaseInactiveTarget() }
        retiredTracks = []; discardedStates = []; discardedGraphs = []
    }
}

/// A change between the zero-delay and latency-bearing master paths retires the
/// complete old mix. Its tracks keep their own source offset and DSP history;
/// both outputs meet on the same audible timeline before the short crossfade.
/// Storage is constructed on the worker and adopted by reference at handoff.
fileprivate final class MasterOutputTransition {
    private var tracks: [TrackProcessor] = []
    private var master: MasterBusProcessor?
    private var hasSolo = false
    private var loopRange: Range<Int>?
    private var predecessor: MasterOutputTransition?
    private var fadePosition = 0
    private var left = [Float](repeating: 0, count: 8192), right = [Float](repeating: 0, count: 8192)
    var completed: Bool { fadePosition >= LiveRamp.frameCount }
    func configure(tracks: [TrackProcessor], master: MasterBusProcessor, hasSolo: Bool, loopRange: Range<Int>?, predecessor: MasterOutputTransition?) {
        self.tracks = tracks; self.master = master; self.hasSolo = hasSolo
        self.loopRange = loopRange; self.predecessor = predecessor; fadePosition = 0
    }
    func blend(intoLeft outputLeft: UnsafeMutablePointer<Float>, right outputRight: UnsafeMutablePointer<Float>, count: Int, position: Int) {
        guard !completed, let master else { return }
        let active = min(count, LiveRamp.frameCount - fadePosition)
        left.withUnsafeMutableBufferPointer { l in right.withUnsafeMutableBufferPointer { r in
            let lp = l.baseAddress!, rp = r.baseAddress!
            lp.update(repeating: 0, count: active); rp.update(repeating: 0, count: active)
            for track in tracks {
                track.render(at: position + master.lookAheadFrames, count: active, left: lp, right: rp,
                             audible: !track.track.muted && (!hasSolo || track.track.solo))
            }
            master.process(left: lp, right: rp, count: active, sourcePosition: position + master.lookAheadFrames, loopRange: loopRange)
            predecessor?.blend(intoLeft: lp, right: rp, count: active, position: position)
            for i in 0..<active {
                let weight = Float(fadePosition + i + 1) / Float(LiveRamp.frameCount)
                outputLeft[i] = lp[i] * (1 - weight) + outputLeft[i] * weight
                outputRight[i] = rp[i] * (1 - weight) + outputRight[i] * weight
            }
        } }
        fadePosition += active
    }
}

fileprivate struct ResolvedClip {
    var envelope: ClipEnvelope
    var clip: Clip
    var audio: AudioSamples
}

fileprivate final class AlignedGraphRetirementMailbox {
    private let lock = NSLock()
    private var values = [TrackProcessor?](repeating: nil, count: 512)
    private var count = 0
    private let publishers = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    init() { publishers.initialize(to: 0) }
    deinit { publishers.deinitialize(count: 1); publishers.deallocate() }
    func beginPublishing() { OSAtomicIncrement64Barrier(publishers) }
    func endPublishing() { OSAtomicDecrement64Barrier(publishers) }
    func retire(_ track: TrackProcessor) -> Bool {
        guard lock.try() else { return false }; defer { lock.unlock() }
        guard count < values.count else { return false }
        values[count] = track; count += 1; return true
    }
    func releaseOnWorker() {
        lock.lock()
        guard count > 0, OSAtomicAdd64Barrier(0, publishers) == 0 else { lock.unlock(); return }
        let retired = values
        values = [TrackProcessor?](repeating: nil, count: 512); count = 0
        lock.unlock()
        withExtendedLifetime(retired) {}
    }
}

fileprivate final class AlignedRetiredTrack {
    var target: TrackProcessor?
    private(set) var elapsed = 0
    private(set) var remaining = 0
    private var startWeight: Float = 1
    private var keepTail = false
    func configure(target: TrackProcessor, weight: Float, keepTail: Bool) {
        self.target = target; self.startWeight = weight; self.keepTail = keepTail
        remaining = LiveRamp.frameCount + (keepTail ? target.latencyFrames + min(Int(sessionRate * 30), target.track.realtimeEffects.reduce(0) { $0 + $1.tailFrames }) : 0)
        if keepTail { target.beginExcitationRelease() }
    }
    func newWeight(offset: Int = 0) -> Float { min(1, Float(elapsed + offset + 1) / Float(LiveRamp.frameCount)) }
    func dryWeight(offset: Int) -> Float { startWeight * (1 - newWeight(offset: offset)) }
    func render(at position: Int, count: Int, left: UnsafeMutablePointer<Float>, right: UnsafeMutablePointer<Float>,
        scratchLeft: UnsafeMutablePointer<Float>, scratchRight: UnsafeMutablePointer<Float>, normalization: UnsafePointer<Float>) {
        guard let target, remaining > 0 else { return }
        let active = min(count, remaining)
        scratchLeft.update(repeating: 0, count: active); scratchRight.update(repeating: 0, count: active)
        target.sourceCache?.advance(to: position)
        target.renderOwn(at: position, count: active, left: scratchLeft, right: scratchRight, audible: true)
        for i in 0..<active {
            let dry = dryWeight(offset: i)
            let ending = min(1, Float(remaining - i) / Float(LiveRamp.frameCount))
            let wet = keepTail ? startWeight * ending : dry
            left[i] += (scratchLeft[i] * wet + target.baselineLeft[i] * (dry - wet)) * normalization[i]
            right[i] += (scratchRight[i] * wet + target.baselineRight[i] * (dry - wet)) * normalization[i]
        }
        elapsed += active; remaining -= active
    }
    func requestRetirementFade() { remaining = min(remaining, LiveRamp.frameCount) }
    func releaseInactiveTarget() { target = nil }
}

fileprivate final class TrackProcessor {
    static let retirementCapacity = 1
    let track: Track
    private var retiredGraphs = [AlignedRetiredTrack?](repeating: nil, count: retirementCapacity)
    private let preparedReplacement = AlignedRetiredTrack()
    private var mainCrossfade: AlignedRetiredTrack?
    private var alignedLeft = [Float](repeating: 0, count: 8192), alignedRight = [Float](repeating: 0, count: 8192)
    private var retirementLeft = [Float](repeating: 0, count: 8192), retirementRight = [Float](repeating: 0, count: 8192)
    private var normalization = [Float](repeating: 1, count: 8192)
    fileprivate var baselineLeft = [Float](repeating: 0, count: 8192), baselineRight = [Float](repeating: 0, count: 8192)
    private var dryDelayLeft: [Float], dryDelayRight: [Float]
    private var dryDelayIndex = 0
    private var excitationDelay = Int.max, excitationRamp: LiveRamp?, excitationGain: Float = 1
    private let clips: [ResolvedClip]
    let effects: [EffectProcessor]
    let latencyFrames: Int
    let transitionChain: EffectTransitionChain
    var sourceCache: RealtimeSourceCache?
    private let loopRange: Range<Int>?
    private var gain: Float
    private var pan: Float
    private var gainRamp: LiveRamp?
    private var panRamp: LiveRamp?
    private let volumeCursor: AutomationCursor
    private let panCursor: AutomationCursor
    private var leftScratch = Array(repeating: Float(0), count: 8192)
    private var rightScratch = Array(repeating: Float(0), count: 8192)
    init(track: Track, assets: [UUID: AudioSamples], primeFrame: Int = 0, loopRange: Range<Int>? = nil, outputLatencyFrames: Int = 0) {
        self.track = track; self.loopRange = loopRange
        latencyFrames = track.realtimeEffects.reduce(0) { $0 + $1.latencyFrames }
        dryDelayLeft = [Float](repeating: 0, count: max(1, latencyFrames))
        dryDelayRight = [Float](repeating: 0, count: max(1, latencyFrames))
        pan = track.pan.isFinite ? max(-1, min(1, track.pan)) : 0
        gain = track.gain.isFinite ? track.gain : 0
        volumeCursor = AutomationCursor(points: track.mixAutomation["volume"] ?? [], baseline: track.baseValue(for: .volume), valueScale: track.volumeAutomationScale ?? .linear)
        panCursor = AutomationCursor(points: track.mixAutomation["pan"] ?? [], baseline: track.baseValue(for: .pan))
        let envelopes = ClipEnvelope.resolve(track: track)
        clips = track.clips.compactMap { clip in
            guard let audio = assets[clip.assetID], audio.channelCount > 0 else { return nil }
            return ResolvedClip(envelope: envelopes[clip.id]!, clip: clip, audio: audio)
        }
        effects = track.realtimeEffects.map { EffectProcessor(effect: $0) }
        transitionChain = EffectTransitionChain(processors: effects)
        if outputLatencyFrames > 0 {
            var downstreamLatency = outputLatencyFrames
            for effect in effects.reversed() {
                if effect.effect.kind == .pitchCorrection { effect.pitchMeterDelay = PitchMeterDelay(frames: downstreamLatency) }
                downstreamLatency += effect.effect.latencyFrames
            }
        }
        // Worker-only lookahead prime: first audible frame remains on the source timeline.
        if latencyFrames > 0 {
            let block = min(8192, latencyFrames)
            let left = UnsafeMutablePointer<Float>.allocate(capacity: block)
            let right = UnsafeMutablePointer<Float>.allocate(capacity: block)
            defer { left.deallocate(); right.deallocate() }
            var offset = 0
            while offset < latencyFrames && !Task.isCancelled {
                let count = min(block, latencyFrames - offset)
                render(at: primeFrame - latencyFrames + offset, count: count, left: left, right: right, audible: false)
                offset += count
            }
        }
    }
    /// Checkpoints restore DSP state; the dry comparison path can be reconstructed
    /// directly from its finite lookahead window on the preparation worker.
    func primeDryBaseline(at frame: Int) {
        guard latencyFrames > 0 else { return }
        for i in dryDelayLeft.indices { dryDelayLeft[i] = 0; dryDelayRight[i] = 0 }
        var offset = 0
        while offset < latencyFrames {
            let sourceFrame = loopRange.map { $0.lowerBound + ((frame + offset - $0.lowerBound) % $0.count + $0.count) % $0.count } ?? frame + offset
            let block = min(latencyFrames - offset, loopRange.map { $0.upperBound - sourceFrame } ?? latencyFrames)
            for source in clips {
                let lo = max(sourceFrame, source.clip.start), hi = min(sourceFrame + block, source.clip.end)
                guard hi > lo else { continue }
                let start = source.clip.offset + lo - source.clip.start
                let data = source.audio.readFrames(start..<(start + hi - lo), channels: [0, min(1, source.audio.channelCount - 1)])
                for i in 0..<(hi - lo) {
                    let fade = source.envelope.gain(at: lo + i - source.clip.start), index = offset + lo + i - sourceFrame
                    dryDelayLeft[index] += data[0][i].isFinite ? data[0][i] * fade : 0
                    dryDelayRight[index] += data[1][i].isFinite ? data[1][i] * fade : 0
                }
            }
            offset += block
        }
        dryDelayIndex = 0
    }
    static func layoutChanged(from old: Track, to incoming: Track) -> Bool {
        guard old.realtimeEffects.contains(where: { $0.latencyFrames > 0 }) || incoming.realtimeEffects.contains(where: { $0.latencyFrames > 0 }) else { return false }
        guard old.realtimeEffects.count == incoming.realtimeEffects.count else { return true }
        return zip(old.realtimeEffects, incoming.realtimeEffects).contains { $0.id != $1.id || $0.kind != $1.kind || $0.latencyFrames != $1.latencyFrames }
    }
    var alignedTailFrames: Int { retiredGraphs.reduce(0) { max($0, $1?.remaining ?? 0) } }
    func beginExcitationRelease() { excitationDelay = LiveRamp.frameCount; excitationRamp = LiveRamp(from: 1, to: 0) }
    func carryAlignedRetirements(from previous: TrackProcessor, discarded: inout [AlignedRetiredTrack?], discardedIndex: inout Int, reserveReplacement: Bool = false) {
        var next = 0
        for i in previous.retiredGraphs.indices {
            guard let graph = previous.retiredGraphs[i] else { continue }
            if graph.remaining > 0 && next < retiredGraphs.count - (reserveReplacement ? 1 : 0) {
                retiredGraphs[next] = graph; next += 1
            } else { discarded[discardedIndex] = graph; discardedIndex += 1 }
            previous.retiredGraphs[i] = nil
        }
        mainCrossfade = previous.mainCrossfade
    }
    func adoptAlignedTransition(from previous: TrackProcessor, discarded: inout [AlignedRetiredTrack?], discardedIndex: inout Int) {
        let weight = previous.mainCrossfade?.newWeight() ?? 1
        carryAlignedRetirements(from: previous, discarded: &discarded, discardedIndex: &discardedIndex, reserveReplacement: true)
        let keepTail = previous.track.realtimeEffects.contains { old in
            old.tailFrames > 0 && !track.realtimeEffects.contains(where: { $0.id == old.id && $0.kind == old.kind })
        }
        preparedReplacement.configure(target: previous, weight: weight, keepTail: keepTail)
        let slot = retiredGraphs.firstIndex(where: { $0 == nil })!
        retiredGraphs[slot] = preparedReplacement; mainCrossfade = preparedReplacement
    }
    func fadeAlignedRetirements() {
        for graph in retiredGraphs { graph?.requestRetirementFade() }
    }
    func retireCompletedGraphs(to mailbox: AlignedGraphRetirementMailbox) {
        mailbox.beginPublishing()
        publishCompletedGraphs(to: mailbox)
        mailbox.endPublishing()
    }
    // All temporary strong target references are released before the publisher
    // barrier drops, so the worker remains the owner that destroys graph storage.
    @inline(never) private func publishCompletedGraphs(to mailbox: AlignedGraphRetirementMailbox) {
        for graph in retiredGraphs {
            guard let graph, graph.remaining == 0, let target = graph.target else { continue }
            if mailbox.retire(target) { graph.target = nil }
        }
    }
    func prepareLiveTransition(from previous: TrackProcessor) {
        if !volumeCursor.hasPoints && gain != previous.gain { gainRamp = LiveRamp(from: previous.gain, to: gain) }
        if !panCursor.hasPoints && pan != previous.pan { panRamp = LiveRamp(from: previous.pan, to: pan) }
    }
    func reserveCapacity(frameCount: Int) {
        transitionChain.reserveCapacity(frameCount: frameCount)
        if frameCount > alignedLeft.count {
            alignedLeft = [Float](repeating: 0, count: frameCount); alignedRight = [Float](repeating: 0, count: frameCount)
            retirementLeft = [Float](repeating: 0, count: frameCount); retirementRight = [Float](repeating: 0, count: frameCount)
            baselineLeft = [Float](repeating: 0, count: frameCount); baselineRight = [Float](repeating: 0, count: frameCount)
            normalization = [Float](repeating: 1, count: frameCount)
        }
        if frameCount > leftScratch.count {
            leftScratch = Array(repeating: 0, count: frameCount)
            rightScratch = Array(repeating: 0, count: frameCount)
        }
    }
    func render(at position: Int, count: Int, left: UnsafeMutablePointer<Float>, right: UnsafeMutablePointer<Float>, audible: Bool) {
        guard retiredGraphs.contains(where: { ($0?.remaining ?? 0) > 0 }) else { renderOwn(at: position, count: count, left: left, right: right, audible: audible); return }
        alignedLeft.withUnsafeMutableBufferPointer { al in alignedRight.withUnsafeMutableBufferPointer { ar in
            let l = al.baseAddress!, r = ar.baseAddress!
            l.update(repeating: 0, count: count); r.update(repeating: 0, count: count)
            renderOwn(at: position, count: count, left: l, right: r, audible: true)
            for i in 0..<count {
                let weight = mainCrossfade?.newWeight(offset: i) ?? 1
                var total = weight
                for graph in retiredGraphs { total += graph?.dryWeight(offset: i) ?? 0 }
                normalization[i] = 1 / max(0.000001, total)
                l[i] *= weight * normalization[i]; r[i] *= weight * normalization[i]
            }
            retirementLeft.withUnsafeMutableBufferPointer { rl in retirementRight.withUnsafeMutableBufferPointer { rr in normalization.withUnsafeBufferPointer { n in
                for graph in retiredGraphs { graph?.render(at: position, count: count, left: l, right: r, scratchLeft: rl.baseAddress!, scratchRight: rr.baseAddress!, normalization: n.baseAddress!) }
            } } }
            if audible { for i in 0..<count { left[i] += l[i]; right[i] += r[i] } }
        } }
    }
    func renderOwn(at position: Int, count: Int, left: UnsafeMutablePointer<Float>, right: UnsafeMutablePointer<Float>, audible: Bool) {
        let sourcePosition = position + latencyFrames
        leftScratch.withUnsafeMutableBufferPointer { l in
            rightScratch.withUnsafeMutableBufferPointer { r in
                let scratchL = l.baseAddress!, scratchR = r.baseAddress!
                scratchL.update(repeating: 0, count: count); scratchR.update(repeating: 0, count: count)
                var segmentOffset = 0
                while segmentOffset < count {
                    let sourceFrame: Int
                    if let loopRange { sourceFrame = loopRange.lowerBound + ((sourcePosition + segmentOffset - loopRange.lowerBound) % loopRange.count + loopRange.count) % loopRange.count }
                    else { sourceFrame = sourcePosition + segmentOffset }
                    let segmentCount = min(count - segmentOffset, loopRange.map { $0.upperBound - sourceFrame } ?? count)
                    for source in clips {
                        let clip = source.clip, audio = source.audio
                        let lo = max(sourceFrame, clip.start), hi = min(sourceFrame + segmentCount, clip.end)
                        guard hi > lo else { continue }
                        let offset = clip.offset + lo - clip.start
                        if audio.isStreamed, let sourceCache {
                            sourceCache.withSamples(id: clip.assetID, sourceFrame: offset, count: hi - lo) { first, second, copied, frames in
                                for i in 0..<frames {
                                    let frame = lo + copied + i, fade = source.envelope.gain(at: frame - clip.start)
                                    scratchL[segmentOffset + frame - sourceFrame] += first[i].isFinite ? first[i] * fade : 0
                                    scratchR[segmentOffset + frame - sourceFrame] += second[i].isFinite ? second[i] * fade : 0
                                }
                            }
                        } else {
                            let workerBlock = audio.isStreamed ? audio.readFrames(offset..<(offset + hi - lo), channels: [0, min(1, audio.channelCount - 1)]) : nil
                            for frame in lo..<hi {
                                let index = clip.offset + frame - clip.start
                                guard index >= 0, index < audio.count else { continue }
                                let first = workerBlock?[0][frame - lo] ?? audio.sample(channel: 0, frame: index)
                                let second = workerBlock?[1][frame - lo] ?? audio.sample(channel: min(1, audio.channelCount - 1), frame: index)
                                let fade = source.envelope.gain(at: frame - clip.start)
                                scratchL[segmentOffset + frame - sourceFrame] += first.isFinite ? first * fade : 0
                                scratchR[segmentOffset + frame - sourceFrame] += second.isFinite ? second * fade : 0
                            }
                        }
                    }
                    segmentOffset += segmentCount
                }
                for i in 0..<count {
                    if excitationDelay != Int.max {
                        if excitationDelay > 0 { excitationDelay -= 1 }
                        else if excitationRamp != nil { excitationGain = excitationRamp!.next(); if excitationRamp!.completed { excitationRamp = nil } }
                        scratchL[i] *= excitationGain; scratchR[i] *= excitationGain
                    }
                    if latencyFrames > 0 {
                        baselineLeft[i] = dryDelayLeft[dryDelayIndex]; baselineRight[i] = dryDelayRight[dryDelayIndex]
                        dryDelayLeft[dryDelayIndex] = scratchL[i]; dryDelayRight[dryDelayIndex] = scratchR[i]
                        dryDelayIndex = (dryDelayIndex + 1) % latencyFrames
                    } else { baselineLeft[i] = scratchL[i]; baselineRight[i] = scratchR[i] }
                }
                transitionChain.process(left: scratchL, right: scratchR, count: count, position: sourcePosition, loopRange: loopRange)
                guard audible else { return }
                for i in 0..<count {
                    let timelineFrame = loopRange.map { $0.lowerBound + ((position + i - $0.lowerBound) % $0.count + $0.count) % $0.count } ?? (position + i)
                    if volumeCursor.hasPoints { gain = TrackAutomationParameter.volume.physical(volumeCursor.value(at: timelineFrame)) }
                    else if gainRamp != nil { gain = gainRamp!.next(); if gainRamp!.completed { gainRamp = nil } }
                    if panCursor.hasPoints { pan = TrackAutomationParameter.pan.physical(panCursor.value(at: timelineFrame)) }
                    else if panRamp != nil { pan = panRamp!.next(); if panRamp!.completed { panRamp = nil } }
                    baselineLeft[i] *= gain * (pan > 0 ? 1 - pan : 1)
                    baselineRight[i] *= gain * (pan < 0 ? 1 + pan : 1)
                    left[i] += scratchL[i] * gain * (pan > 0 ? 1 - pan : 1)
                    right[i] += scratchR[i] * gain * (pan < 0 ? 1 + pan : 1)
                }
            }
        }
    }
}

/// A small value type; ramp storage is prepared before graph publication.
/// Ten milliseconds smooths manual live edits without changing saved/offline DSP.
struct LiveRamp {
    static let frameCount = Int(sessionRate * 0.01)
    let start: Float
    let target: Float
    private var remaining = frameCount
    var completed: Bool { remaining == 0 }
    init(from start: Float, to target: Float) { self.start = start; self.target = target }
    mutating func next() -> Float {
        guard remaining > 0 else { return target }
        remaining -= 1
        if remaining == 0 { return target }
        return start + (target - start) * Float(Self.frameCount - remaining) / Float(Self.frameCount)
    }
}

/// Mutable signal history, adopted by identity when a prepared graph replaces the active graph.
final class EffectState {
    var filterL: Float = 0, filterR: Float = 0
    var delayL: [Float], delayR: [Float]
    var delayIndex = 0
    var advancedDelayL: [Float], advancedDelayR: [Float]
    var advancedDelayIndex = 0
    var biquadsL = [Biquad(), Biquad(), Biquad()]
    var biquadsR = [Biquad(), Biquad(), Biquad()]
    var envelope: Float = 0
    var toneL: Float = 0, toneR: Float = 0
    var roomL: RoomNetwork?, roomR: RoomNetwork?
    var stereoRoom: StereoRoomDSP?
    var chorus: ChorusState?
    var gate: GateDSP?
    var utility: UtilityDSP?
    var equalizer: EqualizerDSP?
    var limiter: LimiterDSP?
    var resonance: ResonanceDSP?
    var putty: SoundPuttyDSP?
    var vocal: VocalEnhancerDSP?
    var isolation: SourceIsolatorDSP?
    var guitarRig: GuitarRigDSP?
    var warmth: WarmthDSP?
    var pitch: PitchCorrectionState?
    var pitchMeterDelay: PitchMeterDelay?
    init(kind: RealtimeEffectKind) {
        roomL = kind == .room ? RoomNetwork(offset: 0) : nil
        roomR = kind == .room ? RoomNetwork(offset: 23) : nil
        stereoRoom = kind == .room ? StereoRoomDSP() : nil
        chorus = kind == .chorus ? ChorusState() : nil
        gate = kind == .gate ? GateDSP() : nil
        utility = kind == .utility ? UtilityDSP() : nil
        equalizer = kind == .equalizer ? EqualizerDSP() : nil
        limiter = kind == .limiter ? LimiterDSP() : nil
        resonance = kind == .resonance ? ResonanceDSP() : nil
        putty = kind == .soundPutty ? SoundPuttyDSP() : nil
        vocal = kind == .vocalEnhancer ? VocalEnhancerDSP() : nil
        isolation = kind == .sourceIsolator ? SourceIsolatorDSP() : nil
        guitarRig = kind == .guitarRig ? GuitarRigDSP() : nil
        warmth = kind == .warmth ? WarmthDSP() : nil
        pitch = kind == .pitchCorrection ? PitchCorrectionState() : nil
        let delayCapacity = kind == .echo ? 96_002 : kind == .room ? 9602 : 0
        advancedDelayL = Array(repeating: 0, count: delayCapacity)
        advancedDelayR = Array(repeating: 0, count: delayCapacity)
        delayL = Array(repeating: 0, count: kind == .echo ? 12_000 : 0)
        delayR = Array(repeating: 0, count: kind == .echo ? 12_000 : 0)
    }
}

final class EffectProcessor {
    var externalDetector: UnsafePointer<Float>?
    var externalDetectorOffset = 0
    let effect: RealtimeEffect
    var state: EffectState
    var meter = EffectMeter()
    var pitchMeterDelay: PitchMeterDelay? {
        get { state.pitchMeterDelay }
        set { state.pitchMeterDelay = newValue }
    }
    var pitchMeterLoopRange: Range<Int>?
    let soundPuttyShape: SoundPuttyShape?
    var soundPuttyShapePending = true
    let descriptors: [EffectParameter]
    var values: [Float]
    let baseValues: [Float]
    var cachedValues: [Float]
    let legacyCursor: AutomationCursor
    let parameterCursors: [AutomationCursor]
    let pitchEditCursor: PitchEditCursor?
    let bakedRangeCursor: PitchBakedRangeCursor?
    let automated: Bool
    let usesLegacy: Bool
    let legacyParameterIndex: Int?
    var parameterRamps: [LiveRamp?]
    var hasParameterRamps = false
    var legacyAmountRamp: LiveRamp?
    var lastLegacyAmount: Float
    var bypassBlend: Float
    var bypassRamp: LiveRamp?
    let noiseSeed: UInt64
    init(effect: RealtimeEffect) {
        self.effect = effect; state = EffectState(kind: effect.kind)
        var uuid = effect.id.uuid
        noiseSeed = withUnsafeBytes(of: &uuid) { bytes in bytes.reduce(UInt64(0xcbf29ce484222325)) { ($0 ^ UInt64($1)) &* 0x100000001b3 } }
        soundPuttyShape = effect.kind == .soundPutty ? SoundPuttyShape(parameters: effect.parameters) : nil
        descriptors = effect.kind.parameters
        values = Array(repeating: 0, count: descriptors.count)
        parameterRamps = Array(repeating: nil, count: descriptors.count)
        lastLegacyAmount = normalizedAmount(effect.amount)
        bypassBlend = effect.enabled ? 1 : 0
        let base = effect.parametersForEditing
        baseValues = descriptors.map { $0.clamped(base[$0.id] ?? $0.defaultValue) }
        for i in values.indices { values[i] = baseValues[i] }
        cachedValues = Array(repeating: .nan, count: descriptors.count)
        legacyCursor = AutomationCursor(points: effect.automation, baseline: normalizedAmount(effect.amount))
        parameterCursors = descriptors.map { descriptor in
            AutomationCursor(points: effect.parameterAutomation[descriptor.id] ?? [], baseline: descriptor.normalized(base[descriptor.id] ?? descriptor.defaultValue))
        }
        pitchEditCursor = effect.kind == .pitchCorrection && !effect.pitchEdits.isEmpty ? PitchEditCursor(regions: effect.pitchEdits) : nil
        bakedRangeCursor = effect.bakedRanges.isEmpty ? nil : PitchBakedRangeCursor(ranges: effect.bakedRanges)
        let legacyKey: String?
        switch effect.kind {
        case .gain: legacyKey = "gain"
        case .lowPass: legacyKey = "frequency"
        case .echo: legacyKey = "mix"
        case .warmth: legacyKey = "drive"
        default: legacyKey = nil
        }
        if effect.parameters.isEmpty, let legacyKey, effect.parameterAutomation[legacyKey]?.isEmpty != false {
            legacyParameterIndex = descriptors.firstIndex { $0.id == legacyKey }
        } else { legacyParameterIndex = nil }
        automated = parameterCursors.contains(where: \.hasPoints) || (legacyParameterIndex != nil && legacyCursor.hasPoints)
        usesLegacy = effect.usesLegacyProcessor
    }
    func prepareLiveTransition(from previous: EffectProcessor) {
        meter = previous.meter
        let targetBlend: Float = effect.enabled ? 1 : 0
        bypassBlend = previous.bypassBlend
        if targetBlend != bypassBlend { bypassRamp = LiveRamp(from: bypassBlend, to: targetBlend) }
        guard usesLegacy == previous.usesLegacy else { return }
        if usesLegacy {
            if effect.automation.isEmpty && previous.effect.automation.isEmpty && effect.amount != previous.lastLegacyAmount {
                legacyAmountRamp = LiveRamp(from: previous.lastLegacyAmount, to: normalizedAmount(effect.amount))
            }
            return
        }
        for i in descriptors.indices {
            let key = descriptors[i].id
            // User-drawn and Write curves retain their original sample-accurate values.
            guard effect.parameterAutomation[key]?.isEmpty != false, previous.effect.parameterAutomation[key]?.isEmpty != false else { continue }
            if legacyParameterIndex == i && (legacyCursor.hasPoints || previous.legacyCursor.hasPoints) { continue }
            guard baseValues[i] != previous.baseValues[i] || previous.parameterRamps[i] != nil else { continue }
            let start = descriptors[i].normalized(previous.values[i]), target = descriptors[i].normalized(baseValues[i])
            if start != target { parameterRamps[i] = LiveRamp(from: start, to: target); hasParameterRamps = true }
        }
    }
    func blendBypass(dry: Float, wet: Float) -> Float {
        if bypassBlend <= 0 || !wet.isFinite { return dry }
        if bypassBlend >= 1 { return wet }
        return dry * (1 - bypassBlend) + wet * bypassBlend
    }
    func isBaked(outputFrame: Int) -> Bool {
        guard let bakedRangeCursor else { return false }
        let sourceFrame: Int
        if let loop = pitchMeterLoopRange, !loop.isEmpty {
            sourceFrame = loop.lowerBound + ((outputFrame - loop.lowerBound) % loop.count + loop.count) % loop.count
        } else { sourceFrame = outputFrame }
        return bakedRangeCursor.contains(sourceFrame)
    }
    func advanceBypass() {
        if bypassRamp != nil { bypassBlend = bypassRamp!.next(); if bypassRamp!.completed { bypassRamp = nil } }
    }
    /// A partial old-project parameter curve can coexist with legacy amount automation.
    /// Convert the interpolated amount itself so nonlinear drive mappings stay exact.
    func legacyParameterValue(at frame: Int, index: Int) -> Float {
        let amount = legacyCursor.value(at: frame)
        let physical: Float
        switch effect.kind {
        case .gain: physical = amount * 48 - 24
        case .lowPass: physical = 200 * pow(80, 1 - amount)
        case .echo: physical = amount * 0.65
        case .warmth: physical = 20 * log10(1 + amount * 8)
        default: physical = baseValues[index]
        }
        return descriptors[index].physical(descriptors[index].normalized(physical))
    }
    func processLegacy(left: UnsafeMutablePointer<Float>, right: UnsafeMutablePointer<Float>, count: Int, position: Int) {
        var measurement = EffectMeterAccumulator()
        for i in 0..<count {
            var amount = legacyCursor.value(at: position + i)
            if legacyAmountRamp != nil { amount = legacyAmountRamp!.next(); if legacyAmountRamp!.completed { legacyAmountRamp = nil } }
            lastLegacyAmount = amount
            let dryL = left[i].isFinite ? left[i] : 0, dryR = right[i].isFinite ? right[i] : 0
            var wetL = dryL, wetR = dryR
            switch effect.kind {
            case .gain:
                let gain = pow(Float(10), (amount * 48 - 24) / 20)
                wetL *= gain; wetR *= gain
            case .lowPass:
                let hz: Float = 200 * pow(80, 1 - amount)
                let coefficient = 1 - exp(-2 * Float.pi * hz / Float(sessionRate))
                state.filterL += coefficient * (dryL - state.filterL); state.filterR += coefficient * (dryR - state.filterR)
                wetL = state.filterL; wetR = state.filterR
            case .echo:
                let delayedL = state.delayL[state.delayIndex], delayedR = state.delayR[state.delayIndex]
                state.delayL[state.delayIndex] = dryL + delayedL * 0.4
                state.delayR[state.delayIndex] = dryR + delayedR * 0.4
                state.delayIndex = (state.delayIndex + 1) % state.delayL.count
                wetL += delayedL * amount * 0.65; wetR += delayedR * amount * 0.65
            case .highPass, .compressor, .room, .equalizer, .chorus, .dither, .pitchCorrection, .limiter, .resonance, .vocalEnhancer, .sourceIsolator, .soundPutty, .gate, .utility, .guitarRig: break
            case .warmth:
                let drive: Float = 1 + amount * 8
                wetL = tanh(dryL * drive) / tanh(drive); wetR = tanh(dryR * drive) / tanh(drive)
            }
            // Bypass remains dry while the state advances, avoiding stale echoes on re-enable.
            advanceBypass()
            let baked = isBaked(outputFrame: position + i)
            left[i] = baked ? dryL : blendBypass(dry: dryL, wet: wetL)
            right[i] = baked ? dryR : blendBypass(dry: dryR, wet: wetR)
            measurement.add(inputL: dryL, inputR: dryR, outputL: left[i], outputR: right[i])
        }
        meter.publish(measurement.finish())
    }
}
