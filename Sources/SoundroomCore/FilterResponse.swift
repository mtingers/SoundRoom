import Foundation

/// Static magnitude response from the same Float coefficients as the filters.
/// It describes settings, not a live spectrum or current automated parameter.
public enum FilterResponse {
    public static func decibels(at frequency: Double, kind: RealtimeEffectKind, parameters: [String: Float]) -> Double {
        guard kind == .lowPass || kind == .highPass else { return 0 }
        let descriptors = kind.parameters
        let cutoff = descriptors[0].clamped(parameters["frequency"] ?? descriptors[0].defaultValue)
        let q = descriptors[1].clamped(parameters["resonance"] ?? descriptors[1].defaultValue)
        var filter = Biquad()
        filter.configure(frequency: cutoff, q: q, mode: kind)
        let probe = frequency.isFinite ? max(0, min(sessionRate / 2, frequency)) : 0
        let omega = 2 * Double.pi * probe / sessionRate
        let nr = Double(filter.b0) + Double(filter.b1) * cos(omega) + Double(filter.b2) * cos(2 * omega)
        let ni = -Double(filter.b1) * sin(omega) - Double(filter.b2) * sin(2 * omega)
        let dr = 1 + Double(filter.a1) * cos(omega) + Double(filter.a2) * cos(2 * omega)
        let di = -Double(filter.a1) * sin(omega) - Double(filter.a2) * sin(2 * omega)
        let magnitudeSquared = (nr * nr + ni * ni) / max(1e-30, dr * dr + di * di)
        return 10 * log10(max(1e-30, magnitudeSquared))
    }
}
