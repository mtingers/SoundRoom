import Foundation

final class GateDSP {
    var open = false, initialized = false
    var hold = 0
    var gainDB: Float = 0
    var reductionDB: Float = 0
    func process(_ l: Float, _ r: Float, _ v: [Float]) -> (Float, Float) {
        let db = 20 * log10(max(1e-9, max(abs(l), abs(r))))
        if db >= v[0] || open && db >= v[0] - v[6] {
            open = true; hold = Int((v[4] * 48).rounded())
        } else if hold > 0 { hold -= 1 } else { open = false }
        let target: Float = open ? 0 : -min(v[2], max(0, v[0] - db) * (v[1] - 1))
        if !initialized { gainDB = v[1] <= 1 ? 0 : -v[2]; initialized = true }
        let ms = target > gainDB ? v[3] : v[5]
        gainDB += (1 - exp(-1 / (ms * 48))) * (target - gainDB)
        reductionDB = -gainDB
        let gain = pow(Float(10), gainDB / 20)
        return (l * gain, r * gain)
    }
    func deepCopy() -> GateDSP {
        let c = GateDSP(); c.open = open; c.initialized = initialized; c.hold = hold
        c.gainDB = gainDB; c.reductionDB = reductionDB; return c
    }
}

final class UtilityDSP {
    var left = [Float](repeating: 0, count: 4801), right = [Float](repeating: 0, count: 4801)
    var index = 0
    func process(_ l: Float, _ r: Float, _ v: [Float]) -> (Float, Float) {
        var l = l * (v[1] >= 0.5 ? -1 : 1), r = r * (v[2] >= 0.5 ? -1 : 1)
        if v[3] >= 0.5 { swap(&l, &r) }
        let mid = (l + r) * 0.5, side = (l - r) * 0.5 * (v[4] >= 0.5 ? 0 : v[5])
        left[index] = mid + side; right[index] = mid - side
        let gain = pow(Float(10), v[0] / 20)
        let result = (left[(index + 4801 - Int(v[6].rounded())) % 4801] * gain, right[(index + 4801 - Int(v[7].rounded())) % 4801] * gain)
        index = (index + 1) % 4801
        return result
    }
    func deepCopy() -> UtilityDSP {
        let c = UtilityDSP(); c.left = left.withUnsafeBufferPointer { Array($0) }
        c.right = right.withUnsafeBufferPointer { Array($0) }; c.index = index; return c
    }
}
