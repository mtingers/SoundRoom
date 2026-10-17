import Foundation

public enum TrackAutomationParameter: String, CaseIterable, Identifiable, Sendable {
    case volume, pan
    public var id: String { rawValue }
    public var name: String { self == .volume ? "Volume" : "Pan" }
    public func normalized(_ physical: Float) -> Float {
        let safe = physical.isFinite ? physical : (self == .volume ? 1 : 0)
        return max(0, min(1, self == .volume ? safe / 1.5 : (safe + 1) / 2))
    }
    public func physical(_ normalized: Float) -> Float {
        let n = normalized.isFinite ? max(0, min(1, normalized)) : (self == .volume ? 2 / 3 : 0.5)
        return self == .volume ? n * 1.5 : n * 2 - 1
    }
    public func display(_ normalized: Float) -> String {
        let value = physical(normalized)
        if self == .volume { return value <= 0 ? "−∞ dB" : String(format: "%+.1f dB", 20 * log10(value)) }
        return abs(value) < 0.005 ? "Center" : String(format: "%.0f%% %@", abs(value) * 100, value < 0 ? "L" : "R")
    }
}

public extension Track {
    func baseValue(for parameter: TrackAutomationParameter) -> Float {
        parameter.normalized(parameter == .volume ? gain : pan)
    }
    func value(at frame: Int, parameter: TrackAutomationParameter) -> Float {
        AutomationInterpolation.value(
            mixAutomation[parameter.rawValue] ?? [], at: frame, baseline: baseValue(for: parameter),
            scale: parameter == .volume ? (volumeAutomationScale ?? .linear) : .linear)
    }
}
