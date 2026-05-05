import XCTest
@testable import Aura

final class SmokeTest: XCTestCase {
    func test_packageBuildsAndImports() {
        // The presence of @testable import Aura succeeding is the test.
        XCTAssertTrue(true)
    }
}
