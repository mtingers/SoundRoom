import XCTest

@testable import SoundroomRemote

final class SeparationTimeoutTests: XCTestCase {
    @MainActor func testLearnedJobsAllowBoundedCompletionTimeAfterTheWorkersHour() {
        let separation = RemoteCommand.editor(.init(operation: "effects.apply", flags: ["separateSource": true]))
        XCTAssertEqual(RemoteClient.requestDeadline(separation, timeout: 3600), 3600)
        XCTAssertEqual(RemoteClient.requestDeadline(separation, timeout: 3900), 3900)
        XCTAssertEqual(RemoteClient.requestDeadline(separation, timeout: 7200), 3900)
        XCTAssertEqual(RemoteClient.requestDeadline(separation, timeout: .nan), 30)
        XCTAssertEqual(RemoteClient.requestDeadline(separation, timeout: -1), 0.1)
        XCTAssertEqual(RemoteClient.requestDeadline(.snapshot, timeout: 3600), 600)
        let unrelated = RemoteCommand.editor(.init(operation: "sources.status", flags: ["separateSource": true]))
        XCTAssertEqual(RemoteClient.requestDeadline(unrelated, timeout: 3600), 600)
    }
}
