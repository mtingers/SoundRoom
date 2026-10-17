import SoundroomCore
import XCTest

final class SessionNameTests: XCTestCase {
    func testNormalizedProjectNamesAndUtf8Bounds() throws {
        XCTAssertEqual(try SessionName.validated("  A good session  "), "A good session")
        XCTAssertEqual(try SessionName.projectTitle(" Studio.soundroom "), "Studio")
        XCTAssertEqual(try SessionName.validated(String(repeating: "🎵", count: 32)).utf8.count, 128)
        for invalid in [
            "", " \n", ".hidden", "../escape", "bad\\name", "bad:name", "bad\0name", "bad\nname",
            String(repeating: "🎵", count: 33),
        ] {
            XCTAssertThrowsError(try SessionName.validated(invalid), invalid)
        }
        XCTAssertThrowsError(try SessionName.projectTitle(".soundroom"))
    }
}
