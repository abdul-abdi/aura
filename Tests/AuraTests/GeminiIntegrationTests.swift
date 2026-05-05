import XCTest
@testable import Aura

/// These tests connect to the real Gemini Live API. They are skipped unless
/// AURA_LIVE_TEST=1 is set, because they cost API quota.
final class GeminiIntegrationTests: XCTestCase {
    override func setUp() async throws {
        guard ProcessInfo.processInfo.environment["AURA_LIVE_TEST"] == "1" else {
            throw XCTSkip("Set AURA_LIVE_TEST=1 to run live tests")
        }
    }

    func test_setupCompleteArrives() async throws {
        guard let key = Config.apiKey() else {
            throw XCTSkip("No API key in Keychain")
        }
        let session = GeminiLiveSession(config: LiveSessionConfig(apiKey: key))
        try await session.connect()
        let stream = session.receive()
        var iter = stream.makeAsyncIterator()
        let deadline = Date().addingTimeInterval(10)
        var sawSetup = false
        while let event = await iter.next(), Date() < deadline {
            if case .setupComplete = event { sawSetup = true; break }
        }
        XCTAssertTrue(sawSetup, "setupComplete didn't arrive in 10s")
        await session.close()
    }
}
