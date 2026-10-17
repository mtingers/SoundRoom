import Foundation

struct TrackRoutingError: LocalizedError {
    public let message: String
    public var errorDescription: String? { message }
    static func message(_ text: String) -> Self { Self(message: text) }
}

public struct TrackRoutingPlan {
    struct Edge {
        var from: Int
        var to: Int
        var gain: Float
        var detector: Bool
    }
    var order: [Int], edges: [Edge]
    var inputLatency: [Int], outputLatency: [Int]
    public init(session: Session) throws {
        let tracks = session.tracks
        let n = tracks.count
        func invalid(_ message: String) -> TrackRoutingError { .message(message) }
        guard n <= 129, session.masterTrack.routing == nil else { throw invalid("Invalid master or track routing.") }
        let active = tracks.contains(where: { $0.routing != nil })
        if active {
            var bytes = (n + 1) * 196608
            for track in tracks + [session.masterTrack] {
                guard track.realtimeEffects.count <= 32, track.clips.count <= 8192 else {
                    throw invalid("A routed track supports at most 32 inserts and 8192 clips.")
                }
                bytes += track.clips.count * 256
                for effect in track.realtimeEffects {
                    switch effect.kind {
                    case .pitchCorrection: bytes += 3 * 1024 * 1024
                    case .echo, .room: bytes += 1024 * 1024
                    case .resonance, .sourceIsolator, .vocalEnhancer: bytes += 512 * 1024
                    case .chorus: bytes += 256 * 1024
                    case .limiter: bytes += 128 * 1024
                    case .guitarRig: bytes += 1024 * 1024
                    case .soundPutty, .utility, .warmth: bytes += 65536
                    default: bytes += 4096
                    }
                    if effect.kind == .compressor && track.routing?.sidechainSourceID != nil { bytes += 65536 }
                }
            }
            guard bytes <= 112 * 1024 * 1024 else {
                throw invalid("Routed DSP storage exceeds the 128 MiB preparation budget.")
            }
        }
        func index(_ id: UUID) throws -> Int {
            guard let i = tracks.firstIndex(where: { $0.id == id }) else {
                throw invalid("Routing refers to a missing track.")
            }
            return i
        }
        var edges: [Edge] = []
        for (i, t) in tracks.enumerated() {
            if let r = t.routing {
                guard ["audio", "bus", "return"].contains(r.role), r.sends.count <= 8 else {
                    throw invalid("Invalid track role or more than eight sends.")
                }
                guard r.role == "audio" || (t.clips.isEmpty && t.recordingTakes.isEmpty && !t.armed) else {
                    throw invalid("Bus and return tracks cannot contain clips or record.")
                }
            }
            let target = try t.routing?.outputID.map(index) ?? n
            guard target == n || tracks[target].routing?.role == "bus" else {
                throw invalid("Outputs must go to a group bus or Master.")
            }
            edges.append(Edge(from: i, to: target, gain: 1, detector: false))
            var seen = Set<UUID>()
            for send in t.routing?.sends ?? [] {
                let target = try index(send.trackID)
                guard seen.insert(send.trackID).inserted, send.level.isFinite, (0...2).contains(send.level),
                    tracks[target].routing?.role == "return"
                else { throw invalid("Invalid post-fader send.") }
                edges.append(Edge(from: i, to: target, gain: send.level, detector: false))
            }
            if let key = t.routing?.sidechainSourceID {
                edges.append(Edge(from: try index(key), to: i, gain: 1, detector: true))
            }
        }
        var degree = [Int](repeating: 0, count: n + 1)
        for edge in edges { degree[edge.to] += 1 }
        var order = degree.indices.filter { degree[$0] == 0 }
        var cursor = 0
        while cursor < order.count {
            let i = order[cursor]
            cursor += 1
            for edge in edges where edge.from == i {
                degree[edge.to] -= 1
                if degree[edge.to] == 0 { order.append(edge.to) }
            }
        }
        guard order.count == n + 1 else { throw invalid("Routing would create an audio or sidechain feedback loop.") }
        let nodes = tracks + [session.masterTrack]
        var input = [Int](repeating: 0, count: n + 1)
        var output = input
        for i in order {
            output[i] = input[i] + nodes[i].realtimeEffects.reduce(0) { $0 + $1.latencyFrames }
            for e in edges where e.from == i { input[e.to] = max(input[e.to], output[i]) }
        }
        var frames = input.prefix(n).reduce(0, +)
        for e in edges {
            let delay = input[e.to] - output[e.from]
            if e.detector {
                var prefix = 0
                for effect in nodes[e.to].realtimeEffects {
                    if effect.kind == .compressor { frames += delay + prefix }
                    prefix += effect.latencyFrames
                }
            } else {
                frames += delay
            }
        }
        if tracks.contains(where: { $0.routing != nil }) && (output[n] > 32768 || frames > 2_000_000) {
            throw invalid("Routed latency or delay memory budget exceeded.")
        }
        self.order = order
        self.edges = edges
        inputLatency = input
        outputLatency = output
    }
}

private final class RoutingDelay {
    var left: [Float], right: [Float], index = 0
    init(_ frames: Int) {
        left = .init(repeating: 0, count: frames)
        right = .init(repeating: 0, count: frames)
    }
    func sample(_ l: Float, _ r: Float) -> (Float, Float) {
        guard !left.isEmpty else { return (l, r) }
        let result = (left[index], right[index])
        left[index] = l
        right[index] = r
        index = (index + 1) % left.count
        return result
    }
}
private final class RoutingBuffer {
    let left = UnsafeMutablePointer<Float>.allocate(capacity: 8192),
        right = UnsafeMutablePointer<Float>.allocate(capacity: 8192)
    init() {
        left.initialize(repeating: 0, count: 8192)
        right.initialize(repeating: 0, count: 8192)
    }
    deinit {
        left.deinitialize(count: 8192)
        right.deinitialize(count: 8192)
        left.deallocate()
        right.deallocate()
    }
    func clear(_ count: Int) {
        left.update(repeating: 0, count: count)
        right.update(repeating: 0, count: count)
    }
}
private final class RoutingNode {
    let track: Track, effects: [EffectProcessor], volume: AutomationCursor, pan: AutomationCursor
    let buffer = RoutingBuffer(), raw = RoutingBuffer(), keyAudio = RoutingBuffer()
    var localDelay: RoutingDelay
    var gainRamp: LiveRamp?, panRamp: LiveRamp?, levelRamp: LiveRamp?
    var currentGain: Float, currentPan: Float, currentLevel: Float = 1
    let sources: [(Clip, ClipEnvelope, AudioSamples)]
    let keys: [RoutingBuffer?]
    let inputLatency: Int, outputLatency: Int
    init(_ track: Track, inputLatency: Int, assets: [UUID: AudioSamples], isMaster: Bool = false) {
        let envelopes = ClipEnvelope.resolve(track: track)
        sources = (isMaster ? [] : track.clips).compactMap { clip in
            guard let audio = assets[clip.assetID], let envelope = envelopes[clip.id], audio.channelCount > 0 else {
                return nil
            }
            return (clip, envelope, audio)
        }
        self.track = track
        self.inputLatency = inputLatency
        currentGain = track.gain
        currentPan = track.pan
        effects = track.realtimeEffects.map { EffectProcessor(effect: $0) }
        outputLatency = inputLatency + track.realtimeEffects.reduce(0) { $0 + $1.latencyFrames }
        keys = effects.map { effect in
            track.routing?.sidechainSourceID != nil && effect.effect.kind == .compressor ? RoutingBuffer() : nil
        }
        localDelay = RoutingDelay(isMaster ? 0 : inputLatency)
        volume = AutomationCursor(
            points: track.mixAutomation["volume"] ?? [], baseline: track.baseValue(for: .volume),
            valueScale: track.volumeAutomationScale ?? .linear)
        pan = AutomationCursor(points: track.mixAutomation["pan"] ?? [], baseline: track.baseValue(for: .pan))
    }
}
private struct PreparedRoutingEdge {
    let from: Int, to: Int, processor: Int?, gain: Float
    var delay: RoutingDelay
    var currentGain: Float = 1
    var gainRamp: LiveRamp?
}

/// Prepared on the worker. All node buffers and edge compensation are allocated
/// before playback; processing performs no graph traversal allocations or file I/O.
final class RoutedRealtimeGraph {
    let session: Session, assets: [UUID: AudioSamples], loopRange: Range<Int>?, plan: TrackRoutingPlan
    private let nodes: [RoutingNode], audible: [Bool], keyNeeded: [Bool]
    private var edges: [PreparedRoutingEdge]
    let latency: Int
    private var clock: Int
    private var retiring: RoutedRealtimeGraph?
    private var retirementRemaining = 0
    private let retirementScratch = RoutingBuffer()
    var retirementSlotCount: Int { nodes.count + edges.count }
    func beginTransition(from old: RoutedRealtimeGraph) -> RoutedRealtimeGraph? {
        let discarded = old.retiring
        old.retiring = nil
        old.retirementRemaining = 0
        retiring = old
        retirementRemaining = LiveRamp.frameCount
        return discarded
    }
    var sourceCache: RealtimeSourceCache?
    init(
        session: Session, assets: [UUID: AudioSamples], startFrame: Int, loopRange: Range<Int>?,
        warmHistory: Bool = true, historyStart: Int? = nil, isCancelled: (() -> Bool)? = nil
    ) throws {
        self.session = session
        self.assets = assets
        self.loopRange = loopRange
        plan = try TrackRoutingPlan(session: session)
        let tracks = session.tracks + [session.masterTrack]
        let n = tracks.count
        let input = plan.inputLatency
        let output = plan.outputLatency
        latency = output[n - 1]
        guard latency <= 32768 else { throw TrackRoutingError.message("Routed insert latency exceeds 32768 frames.") }
        nodes = tracks.indices.map {
            RoutingNode(tracks[$0], inputLatency: input[$0], assets: assets, isMaster: $0 == n - 1)
        }
        var edges: [PreparedRoutingEdge] = []
        var frames = input.prefix(n - 1).reduce(0, +)
        for e in plan.edges {
            let delay = input[e.to] - output[e.from]
            if e.detector {
                var prefix = 0
                for (p, effect) in nodes[e.to].effects.enumerated() {
                    if effect.effect.kind == .compressor {
                        frames += delay + prefix
                        edges.append(
                            PreparedRoutingEdge(
                                from: e.from, to: e.to, processor: p, gain: 1, delay: RoutingDelay(delay + prefix)))
                    }
                    prefix += effect.effect.latencyFrames
                }
            } else {
                frames += delay
                edges.append(
                    PreparedRoutingEdge(
                        from: e.from, to: e.to, processor: nil, gain: e.gain, delay: RoutingDelay(delay),
                        currentGain: e.gain))
            }
        }
        guard frames <= 2_000_000 else { throw TrackRoutingError.message("Routing delay memory budget exceeded.") }
        self.edges = edges
        var on = tracks.map(\.solo)
        if on.contains(true) {
            for i in plan.order.reversed()
            where on[i] && (i == n - 1 || tracks[i].routing?.role != "audio" && tracks[i].routing != nil) {
                for e in plan.edges where e.to == i && !e.detector { on[e.from] = true }
            }
            for i in plan.order where on[i] { for e in plan.edges where e.from == i && !e.detector { on[e.to] = true } }
        } else {
            on = .init(repeating: true, count: n)
        }
        audible = on
        var needed = [Bool](repeating: false, count: n)
        for edge in plan.edges where edge.detector { needed[edge.from] = true }
        for i in plan.order.reversed() where needed[i] {
            for edge in plan.edges where edge.to == i { needed[edge.from] = true }
        }
        keyNeeded = needed
        for i in nodes.indices { nodes[i].currentLevel = on[i] && !nodes[i].track.muted ? 1 : 0 }
        let origin = max(loopRange?.lowerBound ?? 0, historyStart ?? 0)
        clock = origin
        let scratch = RoutingBuffer()
        if !warmHistory {
            clock = startFrame + latency
            return
        }
        var remaining = latency + max(0, startFrame - origin)
        while remaining > 0 {
            if Task.isCancelled || isCancelled?() == true {
                throw TrackRoutingError.message("Render preparation cancelled.")
            }
            let count = min(8192, remaining)
            render(left: scratch.left, right: scratch.right, count: count)
            remaining -= count
        }
    }
    func canAdopt(from old: RoutedRealtimeGraph) -> Bool {
        guard session.timing == old.session.timing, nodes.count == old.nodes.count, edges.count == old.edges.count
        else { return false }
        for i in nodes.indices {
            let a = nodes[i]
            let b = old.nodes[i]
            guard a.track.id == b.track.id, a.track.clips == b.track.clips, a.inputLatency == b.inputLatency,
                a.outputLatency == b.outputLatency, a.effects.count == b.effects.count
            else { return false }
            for p in a.effects.indices {
                guard a.effects[p].effect.id == b.effects[p].effect.id,
                    a.effects[p].effect.kind == b.effects[p].effect.kind
                else { return false }
            }
        }
        return edges.indices.allSatisfy {
            edges[$0].from == old.edges[$0].from && edges[$0].to == old.edges[$0].to
                && edges[$0].processor == old.edges[$0].processor
        }
    }
    func adopt(
        from old: RoutedRealtimeGraph, smooth: Bool, discardedStates: inout [EffectState?],
        discardedDelays: inout [AnyObject?]
    ) {
        var stateIndex = 0
        var delayIndex = 0
        if old.retirementRemaining > 0 {
            retiring = old.retiring
            retirementRemaining = old.retirementRemaining
        }
        clock = old.clock
        for i in nodes.indices {
            let a = nodes[i]
            let b = old.nodes[i]
            discardedDelays[delayIndex] = a.localDelay
            delayIndex += 1
            a.localDelay = b.localDelay
            a.currentGain = b.currentGain
            a.currentPan = b.currentPan
            a.currentLevel = b.currentLevel
            if smooth {
                a.gainRamp = LiveRamp(from: b.currentGain, to: a.track.gain)
                a.panRamp = LiveRamp(from: b.currentPan, to: a.track.pan)
                a.levelRamp = LiveRamp(from: b.currentLevel, to: audible[i] && !a.track.muted ? 1 : 0)
            } else {
                a.currentGain = a.track.gain
                a.currentPan = a.track.pan
                a.currentLevel = audible[i] && !a.track.muted ? 1 : 0
            }
            for p in a.effects.indices {
                discardedStates[stateIndex] = a.effects[p].state
                stateIndex += 1
                a.effects[p].state = b.effects[p].state
                if smooth { a.effects[p].prepareLiveTransition(from: b.effects[p]) }
            }
        }
        for i in edges.indices {
            discardedDelays[delayIndex] = edges[i].delay
            delayIndex += 1
            edges[i].delay = old.edges[i].delay
            edges[i].currentGain = smooth ? old.edges[i].currentGain : edges[i].gain
            if smooth { edges[i].gainRamp = LiveRamp(from: old.edges[i].currentGain, to: edges[i].gain) }
        }
    }
    func meter(_ id: UUID) -> EffectMeter? { nodes.lazy.flatMap(\.effects).first { $0.effect.id == id }?.meter }
    private func timeline(_ frame: Int) -> Int {
        loopRange.map { $0.lowerBound + ((frame - $0.lowerBound) % $0.count + $0.count) % $0.count } ?? frame
    }
    func render(left: UnsafeMutablePointer<Float>, right: UnsafeMutablePointer<Float>, count: Int) {
        sourceCache?.advance(to: clock)
        for node in nodes {
            node.buffer.clear(count)
            for key in node.keys { key?.clear(count) }
        }
        for i in plan.order {
            let node = nodes[i]
            let b = node.buffer
            node.raw.clear(count)
            var segment = 0
            while segment < count {
                let frame = timeline(clock + segment)
                let block = min(count - segment, loopRange.map { $0.upperBound - frame } ?? count)
                for (clip, envelope, audio) in node.sources {
                    let lo = max(frame, clip.start)
                    let hi = min(frame + block, clip.end)
                    guard hi > lo else { continue }
                    let source = clip.offset + lo - clip.start
                    let offset = segment + lo - frame
                    if audio.isStreamed, let sourceCache {
                        sourceCache.withSamples(id: clip.assetID, sourceFrame: source, count: hi - lo) {
                            l, r, copied, frames in
                            for j in 0..<frames {
                                let gain = envelope.gain(at: lo + copied + j - clip.start)
                                node.raw.left[offset + copied + j] += l[j].isFinite ? l[j] * gain : 0
                                node.raw.right[offset + copied + j] += r[j].isFinite ? r[j] * gain : 0
                            }
                        }
                    } else {
                        let data =
                            audio.isStreamed
                            ? audio.readFrames(
                                source..<(source + hi - lo), channels: [0, min(1, audio.channelCount - 1)]) : nil
                        for j in 0..<(hi - lo) {
                            let l = data?[0][j] ?? audio.sample(channel: 0, frame: source + j)
                            let r =
                                data?[1][j] ?? audio.sample(channel: min(1, audio.channelCount - 1), frame: source + j)
                            let gain = envelope.gain(at: lo + j - clip.start)
                            node.raw.left[offset + j] += l.isFinite ? l * gain : 0
                            node.raw.right[offset + j] += r.isFinite ? r * gain : 0
                        }
                    }
                }
                segment += block
            }
            for j in 0..<count {
                let own = node.localDelay.sample(node.raw.left[j], node.raw.right[j])
                b.left[j] += own.0
                b.right[j] += own.1
            }
            if i == nodes.count - 1 {
                for j in 0..<count {
                    if node.gainRamp != nil {
                        node.currentGain = node.gainRamp!.next()
                        if node.gainRamp!.completed { node.gainRamp = nil }
                    }
                    let frame = timeline(clock + j - node.inputLatency)
                    let gain =
                        node.volume.hasPoints
                        ? TrackAutomationParameter.volume.physical(node.volume.value(at: frame)) : node.currentGain
                    b.left[j] *= gain
                    b.right[j] *= gain
                }
            }
            var prefix = 0
            for (p, effect) in node.effects.enumerated() {
                effect.externalDetector =
                    node.track.routing?.sidechainSourceID != nil && effect.effect.kind == .compressor
                    ? UnsafePointer(node.keys[p]!.left) : nil
                // Split on loop boundaries so every automation cursor sees wrapped time.
                var offset = 0
                while offset < count {
                    let frame = timeline(clock + offset - node.inputLatency - prefix)
                    let block = min(count - offset, loopRange.map { $0.upperBound - frame } ?? count)
                    effect.externalDetectorOffset = offset
                    effect.process(left: b.left + offset, right: b.right + offset, count: block, position: frame)
                    offset += block
                }
                effect.externalDetector = nil
                prefix += effect.effect.latencyFrames
            }
            for j in 0..<count {
                let frame = timeline(clock + j - node.outputLatency)
                if i != nodes.count - 1, node.gainRamp != nil {
                    node.currentGain = node.gainRamp!.next()
                    if node.gainRamp!.completed { node.gainRamp = nil }
                }
                if node.panRamp != nil {
                    node.currentPan = node.panRamp!.next()
                    if node.panRamp!.completed { node.panRamp = nil }
                }
                if node.levelRamp != nil {
                    node.currentLevel = node.levelRamp!.next()
                    if node.levelRamp!.completed { node.levelRamp = nil }
                }
                let gain =
                    node.volume.hasPoints
                    ? TrackAutomationParameter.volume.physical(node.volume.value(at: frame)) : node.currentGain
                let pan =
                    i == nodes.count - 1
                    ? 0
                    : node.pan.hasPoints
                        ? TrackAutomationParameter.pan.physical(node.pan.value(at: frame)) : node.currentPan
                let level: Float = i == nodes.count - 1 ? 1 : gain
                let l = b.left[j] * level * (pan > 0 ? 1 - pan : 1)
                let r = b.right[j] * level * (pan < 0 ? 1 + pan : 1)
                node.keyAudio.left[j] = node.track.muted ? 0 : l
                node.keyAudio.right[j] = node.track.muted ? 0 : r
                b.left[j] = l * node.currentLevel
                b.right[j] = r * node.currentLevel
            }
            for e in edges.indices where edges[e].from == i {
                var edge = edges[e]
                let target = nodes[edge.to]
                for j in 0..<count {
                    let key = edge.processor != nil || keyNeeded[edge.to] && !audible[edge.to]
                    let y = edge.delay.sample(
                        key ? node.keyAudio.left[j] : b.left[j], key ? node.keyAudio.right[j] : b.right[j])
                    if edge.gainRamp != nil {
                        edge.currentGain = edge.gainRamp!.next()
                        if edge.gainRamp!.completed { edge.gainRamp = nil }
                    }
                    if let p = edge.processor {
                        target.keys[p]!.left[j] = max(abs(y.0), abs(y.1))
                    } else {
                        target.buffer.left[j] += y.0 * edge.currentGain
                        target.buffer.right[j] += y.1 * edge.currentGain
                    }
                }
                edges[e] = edge
            }
        }
        let master = nodes[nodes.count - 1].buffer
        left.update(from: master.left, count: count)
        right.update(from: master.right, count: count)
        if let retiring, retirementRemaining > 0 {
            retiring.render(left: retirementScratch.left, right: retirementScratch.right, count: count)
            for j in 0..<min(count, retirementRemaining) {
                let blend = Float(LiveRamp.frameCount - retirementRemaining + j + 1) / Float(LiveRamp.frameCount)
                left[j] = retirementScratch.left[j] + (left[j] - retirementScratch.left[j]) * blend
                right[j] = retirementScratch.right[j] + (right[j] - retirementScratch.right[j]) * blend
            }
            retirementRemaining = max(0, retirementRemaining - count)
        }
        clock += count
    }
}

public extension Session {
    /// Track stems contain that track's inserts and upstream groups/keys, before
    /// its output bus and shared returns. All routing dependencies stay intact.
    func isolating(trackID: UUID) throws -> Session {
        guard let target = tracks.firstIndex(where: { $0.id == trackID }) else {
            throw TrackRoutingError.message("Export track is missing.")
        }
        var result = self
        result.masterTrack = Self.defaultMasterTrack
        guard tracks.contains(where: { $0.routing != nil }) else {
            result.tracks = [tracks[target]]
            result.tracks[0].muted = false
            result.tracks[0].solo = false
            return result
        }
        var sink = Track(name: "Render sink", color: 0)
        sink.muted = true
        sink.routing = TrackRouting(role: "bus")
        for i in result.tracks.indices {
            result.tracks[i].solo = false
            var route = result.tracks[i].routing ?? TrackRouting()
            if i == target {
                result.tracks[i].muted = false
                route.outputID = nil
                route.sends = []
            } else if route.outputID == nil {
                route.outputID = sink.id
            }
            result.tracks[i].routing = route
        }
        result.tracks.append(sink)
        let selected = result.tracks.remove(at: target)
        result.tracks.insert(selected, at: 0)
        _ = try TrackRoutingPlan(session: result)
        return result
    }
}
