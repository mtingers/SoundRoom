import XCTest
@testable import SoundroomEngineControl

final class AuditionLeaseTests: XCTestCase {
    func testOwnedStopDoesNotReleaseAnotherClient() throws {
        var lease = AuditionLease()
        let owner = UUID(), colleague = UUID()
        let claim = try lease.acquire(owner: owner, kind: .pitch)
        XCTAssertThrowsError(try lease.stop(owner: colleague, kind: .pitch))
        XCTAssertThrowsError(try lease.acquire(owner: colleague, kind: .drums))
        XCTAssertEqual(lease.claim, claim)
        XCTAssertEqual(try lease.stop(owner: owner, kind: .pitch), claim)
        XCTAssertNil(lease.owner)
        XCTAssertNil(try lease.stop(owner: colleague))
    }
    func testDelayedWorkspaceCloseCannotStopReplacementAndGenerationChanges() throws {
        var lease = AuditionLease()
        let owner = UUID()
        let pitch = try lease.acquire(owner: owner, kind: .pitch)
        XCTAssertEqual(try lease.acquire(owner: owner, kind: .pitch), pitch)
        let drums = try lease.acquire(owner: owner, kind: .drums)
        XCTAssertNotEqual(pitch.generation, drums.generation)
        XCTAssertNil(try lease.stop(owner: owner, kind: .pitch))
        XCTAssertEqual(lease.claim, drums)
        XCTAssertEqual(try lease.stop(owner: owner), drums)
        let next = try lease.acquire(owner: owner, kind: .pitch)
        XCTAssertNotEqual(next.generation, pitch.generation)
        lease.clear()
        XCTAssertNil(lease.claim)
    }
}
