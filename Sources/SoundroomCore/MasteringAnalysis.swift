import Foundation

public struct MasteringSection: Codable, Equatable, Sendable {
    public var start: Int, end: Int
    public var loudness: Float, truePeak: Float, resonance: Float
}
public struct MasteringAnalysis: Codable, Equatable, Sendable {
    public var integratedLUFS: Float, truePeakDB: Float, crestDB: Float
    public var bassFraction: Float, airFraction: Float
    public var sections: [MasteringSection]
    public var analyzedFrames: Int
    public var sourceFingerprint: String? = nil
    public static func fingerprint(_ session: Session) -> String {
        let encoder = JSONEncoder(); encoder.outputFormatting = [.sortedKeys]
        guard let data = try? encoder.encode(session.tracks) else { return "" }
        let hash = data.reduce(UInt64(14695981039346656037)) { ($0 ^ UInt64($1)) &* 1099511628211 }
        return String(hash, radix: 16)
    }
    public func matches(_ session: Session) -> Bool { sourceFingerprint == Self.fingerprint(session) }
    public var isSilent: Bool { truePeakDB < -70 }
    /// Conservative proposal. Corrections are bounded; the limiter's own detector
    /// and spectral processor continue adapting sample-by-sample during playback.
    public func proposal(targetLUFS: Float = -14, intensity: Float = 0.65) -> [RealtimeEffect] {
        guard !isSilent else { return [] }
        let strength = min(1, max(0, intensity))
        var eq = RealtimeEffect(kind: .equalizer)
        // Broad balance measurements are descriptive, not genre/style judgments.
        // Only strongly unbalanced energy earns a small suggested shelf cut.
        eq.parameters["lowGain"] = bassFraction > 0.55 ? -min(1.5, (bassFraction - 0.55) * 6) * strength : 0
        eq.parameters["highGain"] = airFraction > 0.35 ? -min(1.5, (airFraction - 0.35) * 6) * strength : 0
        eq.parameters["lowFrequency"] = 180; eq.parameters["highFrequency"] = 6500
        eq.enabled = abs(eq.parameters["lowGain"]!) + abs(eq.parameters["highGain"]!) > 0.1
        var resonance = RealtimeEffect(kind: .resonance, parameters: RealtimeEffectKind.resonance.presets.last!.values)
        let meanResonance = sections.reduce(Float(0)) { $0 + $1.resonance } / Float(max(1, sections.count))
        resonance.parameters["depth"] = min(4, max(0, meanResonance * 1.5)) * strength
        resonance.enabled = meanResonance > 0.8 && strength > 0
        let depthParameter = RealtimeEffectKind.resonance.parameters.first { $0.id == "depth" }!
        resonance.parameterAutomation["depth"] = smoothedPoints { part in
            depthParameter.normalized(min(4, max(0, part.resonance * 1.5)) * strength)
        }
        var warmth = RealtimeEffect(kind: .warmth, parameters: RealtimeEffectKind.warmth.presets.first { $0.name == "Tape cohesion" }!.values)
        warmth.parameters["drive"] = 3; warmth.parameters["mix"] = 0.2 * strength
        warmth.enabled = false // Creative color requires the musician's choice.
        var limiter = RealtimeEffect(kind: .limiter)
        let requestedGain = min(8, max(-6, targetLUFS - integratedLUFS)) * strength
        limiter.parameters["input"] = requestedGain; limiter.parameters["ceiling"] = -1
        let inputParameter = RealtimeEffectKind.limiter.parameters.first { $0.id == "input" }!
        limiter.parameterAutomation["input"] = smoothedPoints { part in
            if part.loudness < -60 { return inputParameter.normalized(requestedGain) }
            // At most ±1.5 dB section balancing. Preserve the song's intentional
            // macro dynamics and bound expected peak reduction near 3 dB.
            let sectionCorrection = min(1.5, max(-1.5, integratedLUFS - part.loudness)) * strength
            let peakBudget = -1 + 3 - part.truePeak
            return inputParameter.normalized(min(requestedGain + sectionCorrection, max(-6, peakBudget)))
        }
        limiter.selectedAutomationParameter = "input"; resonance.selectedAutomationParameter = "depth"
        return [eq, resonance, warmth, limiter]
    }
    private func smoothedPoints(_ value: (MasteringSection) -> Float) -> [AutomationPoint] {
        guard let first = sections.first else { return [] }
        var points = [AutomationPoint(frame: 0, value: value(first), curve: .smooth)]
        var last = value(first)
        for part in sections {
            let next = value(part)
            // Automation descriptors use normalized units; slew the proposals
            // over whole three-second sections rather than around individual beats.
            last = last * 0.5 + next * 0.5
            points.append(AutomationPoint(frame: (part.start + part.end) / 2, value: last, curve: .smooth))
        }
        points.append(AutomationPoint(frame: analyzedFrames, value: last))
        return points
    }
}

public enum MasteringAnalyzer {
    /// Bounded scratch; full-mix analysis includes track inserts, gain and pan but
    /// excludes the master rack. Cancellation/progress operate on worker blocks.
    public static func analyze(session: Session, assets: [UUID: AudioSamples], progress: (@Sendable (Double) -> Void)? = nil) throws -> MasteringAnalysis {
        var source = session; source.masterTrack = Session.defaultMasterTrack
        let end = source.end
        guard end > 0 else { return MasteringAnalysis(integratedLUFS: -120, truePeakDB: -120, crestDB: 0, bassFraction: 0, airFraction: 0, sections: [], analyzedFrames: 0) }
        let mixer = RealtimeMixer(session: source, assets: assets)
        let loudness = LoudnessDSP(), detector = TruePeakDSP(), resonances = ResonanceDSP()
        resonances.configure(RealtimeEffectKind.resonance.parameters.map { p in RealtimeEffectKind.resonance.presets.last!.values[p.id] ?? p.defaultValue })
        var bassL = Biquad(), bassR = Biquad(), airL = Biquad(), airR = Biquad()
        bassL.configure(frequency: 250, q: 0.707, mode: .lowPass); bassR = bassL
        airL.configure(frequency: 5000, q: 0.707, mode: .highPass); airR = airL
        let left = UnsafeMutablePointer<Float>.allocate(capacity: 8192), right = UnsafeMutablePointer<Float>.allocate(capacity: 8192)
        defer { left.deallocate(); right.deallocate() }
        var offset = 0, frame = 0, sectionStart = 0, sectionCount = 0
        var peak: Float = 0, sectionPeak: Float = 0
        var power: Double = 0, bass: Double = 0, air: Double = 0, sectionPower: Double = 0, spectralSum: Double = 0
        var blocks: [Double] = [], sections: [MasteringSection] = []
        var reported = -1
        func finishSection() {
            let measured = Float(max(-120, -0.691 + 10 * log10(max(1e-12, sectionPower / Double(max(1, sectionCount))))))
            sections.append(MasteringSection(start: sectionStart, end: frame, loudness: measured,
                truePeak: 20 * log10(max(1e-6, sectionPeak)), resonance: Float(spectralSum / Double(max(1, sectionCount)))))
            sectionStart = frame; sectionCount = 0; sectionPeak = 0; sectionPower = 0; spectralSum = 0
        }
        while offset < end {
            try Task.checkCancellation()
            let count = min(8192, end - offset)
            mixer.render(intoLeft: left, intoRight: right, frameCount: count)
            for i in 0..<count {
                let l = left[i], r = right[i]
                loudness.process(l, r); let p = detector.process(l, r)
                _ = resonances.process(left: l, right: r)
                peak = max(peak, p); sectionPeak = max(sectionPeak, p)
                let a = Double(bassL.filter(l)), b = Double(bassR.filter(r)), c = Double(airL.filter(l)), d = Double(airR.filter(r))
                power += Double(l) * Double(l) + Double(r) * Double(r); bass += a * a + b * b; air += c * c + d * d
                sectionPower += loudness.lastPower; spectralSum += Double(resonances.reductionDB)
                frame += 1; sectionCount += 1
                if frame >= 19200 && frame % 4800 == 0 { blocks.append(pow(10, Double(loudness.momentary + 0.691) / 10)) }
                if sectionCount == 144000 { finishSection() }
            }
            offset += count
            let percent = offset * 100 / end
            if percent != reported { progress?(Double(offset) / Double(end)); reported = percent }
        }
        if sectionCount > 0 { finishSection() }
        // Flush the peak estimator's finite interpolation neighborhood.
        for _ in 0..<12 { peak = max(peak, detector.process(0, 0)) }
        let absolute = blocks.filter { $0 > pow(10, (-70 + 0.691) / 10) }
        let absoluteMean = absolute.reduce(0, +) / Double(max(1, absolute.count))
        let relative = absolute.filter { $0 >= absoluteMean * 0.1 }
        let integratedPower = relative.isEmpty ? sections.reduce(0.0) { $0 + pow(10, Double($1.loudness + 0.691) / 10) * Double($1.end - $1.start) } / Double(end) : relative.reduce(0, +) / Double(relative.count)
        let integrated = Float(max(-120, -0.691 + 10 * log10(max(1e-12, integratedPower))))
        let peakDB = 20 * log10(max(1e-6, peak)), rmsDB = Float(10 * log10(max(1e-12, power / Double(end * 2))))
        return MasteringAnalysis(integratedLUFS: integrated, truePeakDB: peakDB, crestDB: max(0, peakDB - rmsDB),
            bassFraction: Float(bass / max(1e-12, power)), airFraction: Float(air / max(1e-12, power)), sections: sections, analyzedFrames: end, sourceFingerprint: MasteringAnalysis.fingerprint(session))
    }
}
