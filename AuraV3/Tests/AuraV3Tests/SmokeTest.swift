import XCTest
@testable import AuraV3

final class SmokeTest: XCTestCase {
    func test_packageBuildsAndImports() {
        // The presence of @testable import AuraV3 succeeding is the test.
        XCTAssertTrue(true)
    }
}
