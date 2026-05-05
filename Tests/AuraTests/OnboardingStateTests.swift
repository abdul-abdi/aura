import XCTest
@testable import Aura

@MainActor
final class OnboardingStateTests: XCTestCase {
    func test_initialStepIsWelcome() {
        let s = OnboardingState()
        XCTAssertEqual(s.step, .welcome)
    }

    func test_advanceFollowsCanonicalOrder() {
        let s = OnboardingState()
        s.advance(); XCTAssertEqual(s.step, .apiKey)
        s.advance(); XCTAssertEqual(s.step, .mic)
        s.advance(); XCTAssertEqual(s.step, .screen)
        s.advance(); XCTAssertEqual(s.step, .accessibility)
        s.advance(); XCTAssertEqual(s.step, .sayHi)
        s.advance(); XCTAssertEqual(s.step, .done)
    }

    func test_advanceStopsAtDone() {
        let s = OnboardingState()
        for _ in 0..<10 { s.advance() }
        XCTAssertEqual(s.step, .done)
    }

    func test_apiKeyIsSavedToKeychain() throws {
        let testService = "dev.abdullahi.Aura.tests.onboarding"
        Keychain.delete(service: testService, account: "api_key")
        let s = OnboardingState(keychainService: testService)
        s.apiKeyInput = "sk-onboarding-1234"
        s.commitAPIKey()
        XCTAssertEqual(Keychain.get(service: testService, account: "api_key"),
                        "sk-onboarding-1234")
        Keychain.delete(service: testService, account: "api_key")
    }

    func test_emptyAPIKey_doesNotCommit() throws {
        let testService = "dev.abdullahi.Aura.tests.onboarding"
        Keychain.delete(service: testService, account: "api_key")
        let s = OnboardingState(keychainService: testService)
        s.apiKeyInput = "  "  // whitespace
        s.commitAPIKey()
        XCTAssertNil(Keychain.get(service: testService, account: "api_key"))
    }
}
