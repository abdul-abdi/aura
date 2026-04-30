import XCTest
@testable import AuraV3

final class EventKindTests: XCTestCase {
    func test_namespaceConventions() {
        XCTAssertEqual(EventKind.turnUser, "turn.user")
        XCTAssertEqual(EventKind.effectPosted, "effect.posted")
        XCTAssertEqual(EventKind.statePermissionLost, "state.permission_lost")
    }

    func test_isOpenSchema_unknownKindIsAccepted() {
        let unknown = "experimental.foo.v2"
        XCTAssertTrue(EventKind.isWellFormed(unknown))
    }

    func test_rejectsEmptyOrWhitespace() {
        XCTAssertFalse(EventKind.isWellFormed(""))
        XCTAssertFalse(EventKind.isWellFormed("   "))
        XCTAssertFalse(EventKind.isWellFormed("foo bar"))
    }
}
