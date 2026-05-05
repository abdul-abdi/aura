import XCTest
@testable import Aura

final class ReconnectTests: XCTestCase {
    func test_backoffSequenceIsExpected() {
        var bo = Backoff(base: .milliseconds(200), max: .milliseconds(3200), jitter: 0)
        XCTAssertEqual(bo.next(), .milliseconds(200))
        XCTAssertEqual(bo.next(), .milliseconds(400))
        XCTAssertEqual(bo.next(), .milliseconds(800))
        XCTAssertEqual(bo.next(), .milliseconds(1600))
        XCTAssertEqual(bo.next(), .milliseconds(3200))
        XCTAssertEqual(bo.next(), .milliseconds(3200))  // capped
    }

    func test_jitterIsBoundedTo25Pct() {
        let bo = Backoff(base: .milliseconds(1000), max: .milliseconds(8000), jitter: 0.25)
        for _ in 0..<20 {
            let d = bo.peek()
            XCTAssertGreaterThanOrEqual(d, .milliseconds(750))
            XCTAssertLessThanOrEqual(d, .milliseconds(1250))
        }
    }

    func test_resetReturnsToBase() {
        var bo = Backoff(base: .milliseconds(200), max: .seconds(60), jitter: 0)
        _ = bo.next()
        _ = bo.next()
        bo.reset()
        XCTAssertEqual(bo.next(), .milliseconds(200))
    }

    /// Amendment S: rate limit allows first attempt, blocks burst, allows after interval.
    func test_rateLimit_blocksBurstWithinWindow() async throws {
        var rl = RateLimit(minIntervalSeconds: 0.2)
        XCTAssertTrue(rl.tryAcquire(), "first attempt must succeed")
        XCTAssertFalse(rl.tryAcquire(), "second attempt within window blocked")
        try await Task.sleep(nanoseconds: 250 * 1_000_000)
        XCTAssertTrue(rl.tryAcquire(), "after window, attempts succeed again")
    }

    func test_rateLimit_resetClearsLastAttempt() {
        var rl = RateLimit(minIntervalSeconds: 100)
        XCTAssertTrue(rl.tryAcquire())
        XCTAssertFalse(rl.tryAcquire())
        rl.reset()
        XCTAssertTrue(rl.tryAcquire())
    }
}
