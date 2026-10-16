import Foundation

/// Sparse project/preset parameters retain every sculpted lane without making
/// 900 curve samples into automation controls or sliders.
public struct SoundPuttyShape: Equatable, Sendable {
    public static let laneCount = 300
    public enum Axis: Int, CaseIterable, Identifiable, Sendable {
        case pitch, energy, damping
        public var id: Int { rawValue }
        public var name: String { ["Pitch", "Energy", "Damping"][rawValue] }
        var prefix: String { ["puttyPitch", "puttyEnergy", "puttyDamping"][rawValue] }
    }
    public let rows: [[Double]]
    public init(parameters: [String: Float]) {
        rows = Axis.allCases.map { axis in
            (0..<Self.laneCount).map { lane in
                let value = parameters[Self.key(axis, lane)] ?? 0
                return Double(value.isFinite ? max(-1, min(1, value)) : 0)
            }
        }
    }
    public static func key(_ axis: Axis, _ lane: Int) -> String { axis.prefix + (lane < 10 ? "00" : lane < 100 ? "0" : "") + String(lane) }
    public static func isShapeKey(_ key: String) -> Bool {
        Axis.allCases.contains { axis in
            guard key.hasPrefix(axis.prefix), let index = Int(key.dropFirst(axis.prefix.count)), (0..<laneCount).contains(index) else { return false }
            return key == Self.key(axis, index)
        }
    }
    public static func sanitizedParameters(_ values: [String: Float]) -> [String: Float] {
        values.reduce(into: [:]) { result, entry in
            if isShapeKey(entry.key) { result[entry.key] = entry.value.isFinite ? max(-1, min(1, entry.value)) : 0 }
        }
    }
    public func brush(axis: Axis, lane: Double, target: Double, radius: Double) -> [String: Float] {
        guard lane.isFinite, target.isFinite, radius.isFinite else { return [:] }
        let center = max(0, min(299, lane)), value = max(-1, min(1, target)), width = max(2, min(90, radius))
        var result: [String: Float] = [:]
        for i in 0..<Self.laneCount {
            let influence = exp(-0.5 * pow((Double(i) - center) / width, 2)) * 0.3
            let before = rows[axis.rawValue][i], after = Float(before + (value - before) * influence)
            if after != Float(before) { result[Self.key(axis, i)] = after }
        }
        return result
    }
    public static var resetParameters: [String: Float] {
        Dictionary(uniqueKeysWithValues: Axis.allCases.flatMap { axis in (0..<laneCount).map { (key(axis, $0), Float(0)) } })
    }
    public static func random(lane: Int, seed: UInt32) -> Double {
        var n = UInt32(lane + 1) &* 2_654_435_761 ^ seed
        n ^= n >> 16; n = n &* 2_246_822_519; n ^= n >> 13
        return Double(n) / 4_294_967_295 * 2 - 1
    }
    public func frequency(lane: Int, parameters: [String: Float]) -> Double {
        frequency(lane: lane, controls: Self.controls(parameters))
    }
    public func frequencies(parameters: [String: Float]) -> [Double] {
        let controls = Self.controls(parameters)
        return (0..<300).map { frequency(lane: $0, controls: controls) }
    }
    private static func controls(_ parameters: [String: Float]) -> [Double] {
        RealtimeEffectKind.soundPutty.parameters.map { Double($0.clamped(parameters[$0.id] ?? $0.defaultValue)) }
    }
    private func frequency(lane: Int, controls: [Double]) -> Double {
        let start = controls[0], end = controls[1], q = pow(Double(lane) / 299, controls[2])
        var frequency = start * pow(end / start, q) * exp(rows[0][lane] * 0.65)
        if controls[6] == 0 {
            let zone = exp(-0.5 * pow((Double(lane) - controls[3]) / controls[4], 2)) * controls[5]
            frequency *= exp(zone * Self.random(lane: lane, seed: UInt32(controls[15])) * 1.5)
        }
        return max(15, min(13_800, frequency))
    }
}
