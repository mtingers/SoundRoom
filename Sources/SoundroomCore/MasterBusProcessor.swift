import Foundation
import Darwin

/// Latency-bearing output racks reserve a fixed look-ahead budget on the
/// preparation worker. An empty or zero-latency rack adds no delay, preserving
/// the source timeline and update behavior of existing projects.
/// Master inserts are restricted to a total of 4096 frames by project validation.
final class MasterBusProcessor {
    static let lookAheadFrames = 4096
    let lookAheadFrames: Int
    static func lookAheadFrames(for track: Track) -> Int {
        track.realtimeEffects.contains { $0.latencyFrames > 0 } ? Self.lookAheadFrames : 0
    }
    let track: Track
    let effects: [EffectProcessor]
    let chain: EffectTransitionChain
    private var delayLeft: [Float], delayRight: [Float]
    private var delayIndex = 0
    private var gain: Float, gainRamp: LiveRamp?
    private var muteBlend: Float, muteRamp: LiveRamp?
    private let volume: AutomationCursor
    private var previous: MasterBusProcessor?
    private var fadePosition = 0
    private var scratchLeft = [Float](repeating: 0, count: 8192), scratchRight = [Float](repeating: 0, count: 8192)
    init(track: Track) {
        self.track = track; lookAheadFrames = Self.lookAheadFrames(for: track); effects = track.realtimeEffects.map { EffectProcessor(effect: $0) }
        chain = EffectTransitionChain(processors: effects)
        let padding = max(0, lookAheadFrames - track.realtimeEffects.reduce(0) { $0 + $1.latencyFrames })
        delayLeft = [Float](repeating: 0, count: padding); delayRight = delayLeft.map { $0 }
        gain = track.gain.isFinite ? track.gain : 1
        muteBlend = track.muted ? 0 : 1
        volume = AutomationCursor(points: track.mixAutomation["volume"] ?? [], baseline: track.baseValue(for: .volume), valueScale: track.volumeAutomationScale ?? .linear)
        var downstreamLatency = padding
        for effect in effects.reversed() {
            if effect.effect.kind == .pitchCorrection { effect.pitchMeterDelay = PitchMeterDelay(frames: downstreamLatency) }
            downstreamLatency += effect.effect.latencyFrames
        }
    }
    static func layoutChanged(_ a: Track, _ b: Track) -> Bool {
        a.realtimeEffects.map { "\($0.id).\($0.kind).\($0.latencyFrames)" } != b.realtimeEffects.map { "\($0.id).\($0.kind).\($0.latencyFrames)" }
    }
    func adopt(from old: MasterBusProcessor, aligned: Bool, smooth: Bool, discarded: inout [EffectState?], index: inout Int) {
        if aligned {
            // Different look-ahead budgets must be switched by a prepared whole
            // transport graph, rather than feeding the old rack future audio.
            if lookAheadFrames == old.lookAheadFrames { previous = old; fadePosition = 0 }
            return
        }
        for effect in effects {
            if let prior = old.effects.first(where: { $0.effect.id == effect.effect.id && $0.effect.kind == effect.effect.kind }) {
                discarded[index] = effect.state; effect.state = prior.state
                if smooth { effect.prepareLiveTransition(from: prior) }
            }
            index += 1
        }
        // Transfer prepared ring ownership, not its storage, on the callback.
        swap(&delayLeft, &old.delayLeft); swap(&delayRight, &old.delayRight); delayIndex = old.delayIndex
        if smooth {
            gainRamp = LiveRamp(from: old.gain, to: gain); gain = old.gain
            if muteBlend != old.muteBlend { muteRamp = LiveRamp(from: old.muteBlend, to: muteBlend); muteBlend = old.muteBlend }
            chain.prepareLiveTransition(from: old.chain)
        }
    }
    func process(left: UnsafeMutablePointer<Float>, right: UnsafeMutablePointer<Float>, count: Int, sourcePosition: Int, loopRange: Range<Int>?) {
        if previous != nil && fadePosition < LiveRamp.frameCount {
            scratchLeft.withUnsafeMutableBufferPointer { l in scratchRight.withUnsafeMutableBufferPointer { r in
                l.baseAddress!.update(from: left, count: count); r.baseAddress!.update(from: right, count: count)
                previous!.processOwn(left: l.baseAddress!, right: r.baseAddress!, count: count, sourcePosition: sourcePosition, loopRange: loopRange)
            } }
        }
        processOwn(left: left, right: right, count: count, sourcePosition: sourcePosition, loopRange: loopRange)
        if previous != nil && fadePosition < LiveRamp.frameCount {
            for i in 0..<count {
                let fraction = min(1, Float(fadePosition + i + 1) / Float(LiveRamp.frameCount))
                left[i] = scratchLeft[i] * (1 - fraction) + left[i] * fraction
                right[i] = scratchRight[i] * (1 - fraction) + right[i] * fraction
            }
            fadePosition += count
        }
    }
    private func processOwn(left: UnsafeMutablePointer<Float>, right: UnsafeMutablePointer<Float>, count: Int, sourcePosition: Int, loopRange: Range<Int>?) {
        for i in 0..<count {
            let frame = loopRange.map { $0.lowerBound + ((sourcePosition + i - $0.lowerBound) % $0.count + $0.count) % $0.count } ?? max(0, sourcePosition + i)
            if volume.hasPoints { gain = TrackAutomationParameter.volume.physical(volume.value(at: frame)) }
            else if gainRamp != nil { gain = gainRamp!.next(); if gainRamp!.completed { gainRamp = nil } }
            left[i] *= gain; right[i] *= gain
        }
        chain.process(left: left, right: right, count: count, position: sourcePosition, loopRange: loopRange)
        if !delayLeft.isEmpty {
            for i in 0..<count {
                let l = delayLeft[delayIndex], r = delayRight[delayIndex]
                delayLeft[delayIndex] = left[i]; delayRight[delayIndex] = right[i]
                left[i] = l; right[i] = r; delayIndex = (delayIndex + 1) % delayLeft.count
            }
        }
        // Mute controls the audible output, including already ringing tails.
        for i in 0..<count {
            if muteRamp != nil { muteBlend = muteRamp!.next(); if muteRamp!.completed { muteRamp = nil } }
            left[i] *= muteBlend; right[i] *= muteBlend
        }
    }
    /// Release a completed predecessor only on the preparation worker.
    func retireCompleted(to mailbox: MasterBusRetirementMailbox) {
        guard fadePosition >= LiveRamp.frameCount, let retired = previous else { return }
        mailbox.beginPublishing()
        if mailbox.retire(retired) { previous = nil }
        withExtendedLifetime(retired) { mailbox.endPublishing() }
    }
    func reserveCapacity(_ count: Int) {
        chain.reserveCapacity(frameCount: count)
        if scratchLeft.count < count { scratchLeft = [Float](repeating: 0, count: count); scratchRight = scratchLeft.map { $0 } }
    }
    func deepCopy() -> MasterBusProcessor {
        let copy = MasterBusProcessor(track: track)
        for i in effects.indices { copy.effects[i].restoreSeek(effects[i].seekSnapshot()) }
        copy.delayLeft = delayLeft.map { $0 }; copy.delayRight = delayRight.map { $0 }; copy.delayIndex = delayIndex; copy.gain = gain; copy.muteBlend = muteBlend
        return copy
    }
    var storageBytes: Int {
        // Account for owned callback scratch and the transition chain's three
        // reserved work buffers, not just effect history and output padding.
        effects.reduce(0) { $0 + $1.state.seekStorageBytes + 128 }
            + (delayLeft.count + delayRight.count + scratchLeft.count + scratchRight.count + 8192 * 3) * MemoryLayout<Float>.stride + 4096
    }
}

/// Transfer exclusive callback references to the worker without waiting.
final class MasterBusRetirementMailbox {
    private let lock = NSLock()
    private var values = [AnyObject?](repeating: nil, count: 64)
    private var count = 0
    private let publishers = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    init() { publishers.initialize(to: 0) }
    deinit { publishers.deinitialize(count: 1); publishers.deallocate() }
    func beginPublishing() { OSAtomicIncrement64Barrier(publishers) }
    func endPublishing() { OSAtomicDecrement64Barrier(publishers) }
    func retire(_ value: AnyObject) -> Bool {
        guard lock.try() else { return false }; defer { lock.unlock() }
        guard count < values.count else { return false }
        values[count] = value; count += 1; return true
    }
    func releaseOnWorker() {
        lock.lock()
        guard count > 0, OSAtomicAdd64Barrier(0, publishers) == 0 else { lock.unlock(); return }
        let retired = values; values = [AnyObject?](repeating: nil, count: 64); count = 0
        lock.unlock(); withExtendedLifetime(retired) {}
    }
}
