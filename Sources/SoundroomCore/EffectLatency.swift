import Foundation

public extension RealtimeEffect {
    /// Bypass preserves processing delay so switching the wet path stays aligned.
    var latencyFrames: Int {
        if kind == .warmth && (!parameters.isEmpty || !parameterAutomation.isEmpty) { return WarmthDSP.latencyFrames }
        if kind == .guitarRig { return GuitarRigDSP.latencyFrames }
        if kind == .pitchCorrection { return PitchCorrectionState.latencyFrames }
        if kind == .limiter { return LimiterDSP.latencyFrames }
        if kind == .resonance { return ResonanceDSP.latencyFrames }
        if kind == .vocalEnhancer { return VocalEnhancerDSP.latencyFrames }
        if kind == .sourceIsolator { return SourceIsolatorDSP.latencyFrames }
        return 0
    }
}
