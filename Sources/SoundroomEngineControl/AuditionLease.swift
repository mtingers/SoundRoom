import Foundation

/// Preview ownership is independent of the main transport lease. A workspace's
/// delayed close must never release a different workspace or another client.
public struct AuditionLease: Sendable {
    public enum Kind: String, CaseIterable, Sendable {
        case pitch, effects, designer, drums, take
    }
    public struct Claim: Equatable, Sendable {
        public let owner: UUID
        public let kind: Kind
        public let generation: UUID
    }
    public enum Failure: Error { case ownedByAnotherClient }
    public private(set) var claim: Claim?
    public var owner: UUID? { claim?.owner }
    public init() {}

    @discardableResult public mutating func acquire(owner: UUID, kind: Kind) throws -> Claim {
        if let claim {
            guard claim.owner == owner else { throw Failure.ownedByAnotherClient }
            if claim.kind == kind { return claim }
        }
        let next = Claim(owner: owner, kind: kind, generation: UUID())
        claim = next
        return next
    }
    /// A stale kind guard is an intentional no-op. Ownership is checked before
    /// clearing a matching lease, so a rejected stop cannot mutate it.
    @discardableResult public mutating func stop(owner: UUID, kind: Kind? = nil) throws -> Claim? {
        guard let claim, kind == nil || kind == claim.kind else { return nil }
        guard claim.owner == owner else { throw Failure.ownedByAnotherClient }
        self.claim = nil
        return claim
    }
    public mutating func clear() { claim = nil }
}
