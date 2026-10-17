import Foundation

/// Analytic response of the same double-precision sections used by audio processing.
public enum EQResponse {
    public struct Curve: Sendable {
        private let sections: [EQSection]
        private let indices: [Int]
        private let output: Double
        public init(parameters: [String: Float]) {
            let values = RealtimeEffectKind.equalizer.parameters.map {
                $0.clamped(parameters[$0.id] ?? $0.defaultValue)
            }
            var filters = Array(repeating: EQSection(), count: 10)
            EqualizerDSP.configure(&filters, values: values)
            sections = filters
            var active = [values[8] >= 0.5 ? 1 : 0, 2, values[11] >= 0.5 ? 4 : 3]
            if values[13] >= 0.5 { active.append(5) }
            if values[15] >= 0.5 { active.append(6) }
            active += [7, 8, 9]
            indices = active
            output = Double(values[5])
        }
        public func decibels(at frequency: Double) -> Double {
            var result = output
            for index in indices {
                let response = sections[index].response(at: frequency)
                result += 20 * log10(max(1e-15, hypot(response.real, response.imaginary)))
            }
            return result
        }
    }
    public static func decibels(at frequency: Double, parameters: [String: Float]) -> Double {
        Curve(parameters: parameters).decibels(at: frequency)
    }
}
