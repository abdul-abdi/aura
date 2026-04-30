import XCTest
@testable import AuraV3

final class LogCheckpointTests: XCTestCase {
    var dbPath: String!

    override func setUp() async throws {
        dbPath = "/tmp/aura_ckpt_\(UUID().uuidString).sqlite"
    }
    override func tearDown() async throws {
        try? FileManager.default.removeItem(atPath: dbPath)
        try? FileManager.default.removeItem(atPath: dbPath + "-wal")
        try? FileManager.default.removeItem(atPath: dbPath + "-shm")
    }

    func test_passiveCheckpointSucceeds() async throws {
        let log = try await Log.open(path: dbPath)
        for i in 0..<100 {
            await log.append(kind: EventKind.stateObservation,
                              payload: .object(["i": .number(Double(i))]))
        }
        try await log.flush()
        try await log.checkpoint()  // PASSIVE — does not throw
        await log.shutdown()
    }

    /// Amendment F: TRUNCATE is gated. Without idle conditions met, returns false.
    func test_checkpointIfIdle_skipsWhenRecentTurnUser() async throws {
        let log = try await Log.open(path: dbPath)
        // Stamp a recent turn.user — should disqualify TRUNCATE.
        await log.append(kind: EventKind.turnUser, payload: .object(["text": .string("hi")]))
        try await log.flush()
        let didTruncate = try await log.checkpointIfIdle(walThresholdBytes: 0)  // size threshold trivially met
        XCTAssertFalse(didTruncate, "recent turn.user should block TRUNCATE")
        await log.shutdown()
    }

    /// Idle conditions met → TRUNCATE should run when WAL exceeds threshold.
    func test_checkpointIfIdle_truncatesWhenIdle() async throws {
        let log = try await Log.open(path: dbPath)
        // Append non-critical events only (no turn.user, no unmatched effect.posted)
        for i in 0..<50 {
            await log.append(kind: EventKind.stateObservation,
                              payload: .object(["i": .number(Double(i))]))
        }
        try await log.flush()
        // WAL has some pages; threshold of 0 means "any size" — should truncate.
        let didTruncate = try await log.checkpointIfIdle(walThresholdBytes: 0)
        XCTAssertTrue(didTruncate, "no recent turn.user, no in-flight effect → TRUNCATE")
        await log.shutdown()
    }

    /// In-flight effect (effect.posted without outcome) blocks TRUNCATE.
    func test_checkpointIfIdle_skipsWhenInFlightEffect() async throws {
        let log = try await Log.open(path: dbPath)
        await log.append(kind: EventKind.effectPosted,
                          correlationID: "cid-1",
                          payload: .object(["intent": .string("click")]))
        // No matching effect.outcome — in-flight.
        try await log.flush()
        let didTruncate = try await log.checkpointIfIdle(walThresholdBytes: 0)
        XCTAssertFalse(didTruncate, "in-flight effect should block TRUNCATE")
        await log.shutdown()
    }
}
