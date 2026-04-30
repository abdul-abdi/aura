import XCTest
@testable import AuraV3

final class KeychainTests: XCTestCase {
    let testService = "dev.abdullahi.AuraV3.tests"

    override func setUp() async throws {
        // Make sure we start clean. (Best-effort; ignore failures.)
        Keychain.delete(service: testService, account: "api_key")
    }

    func test_setThenGet_returnsTheValue() throws {
        try Keychain.set("sk-test-1234", service: testService, account: "api_key")
        XCTAssertEqual(Keychain.get(service: testService, account: "api_key"), "sk-test-1234")
    }

    func test_delete_removesTheValue() throws {
        try Keychain.set("sk-test-1234", service: testService, account: "api_key")
        Keychain.delete(service: testService, account: "api_key")
        XCTAssertNil(Keychain.get(service: testService, account: "api_key"))
    }

    func test_setOverwritesExistingValue() throws {
        try Keychain.set("first", service: testService, account: "api_key")
        try Keychain.set("second", service: testService, account: "api_key")
        XCTAssertEqual(Keychain.get(service: testService, account: "api_key"), "second")
    }
}
