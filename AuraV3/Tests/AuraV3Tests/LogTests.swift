import XCTest
@testable import AuraV3

final class LogTests: XCTestCase {
    var dbPath: String!

    override func setUp() async throws {
        dbPath = "/tmp/aura_log_\(UUID().uuidString).sqlite"
    }
    override func tearDown() async throws {
        try? FileManager.default.removeItem(atPath: dbPath)
        try? FileManager.default.removeItem(atPath: dbPath + "-wal")
        try? FileManager.default.removeItem(atPath: dbPath + "-shm")
    }

    func test_appendAndQueryByKind() async throws {
        let log = try await Log.open(path: dbPath)
        await log.append(kind: EventKind.turnUser, payload: .object(["text": .string("hi")]))
        await log.append(kind: EventKind.turnAssistant, payload: .object(["text": .string("hello")]))
        try await log.flush()
        let rows = try await log.query(kind: EventKind.turnUser, since: 0)
        XCTAssertEqual(rows.count, 1)
        XCTAssertEqual(rows.first?.payload["text"]?.stringValue, "hi")
        await log.shutdown()
    }

    func test_correlationLookup_pairsEffectPostedWithOutcome() async throws {
        let log = try await Log.open(path: dbPath)
        let cid = UUID().uuidString
        await log.append(kind: EventKind.effectPosted, correlationID: cid,
                          payload: .object(["intent": .string("click")]))
        await log.append(kind: EventKind.effectOutcome, correlationID: cid,
                          payload: .object(["status": .string("ok")]))
        try await log.flush()
        let pair = try await log.queryCorrelation(cid)
        XCTAssertEqual(pair.count, 2)
        XCTAssertEqual(pair[0].kind, EventKind.effectPosted)
        XCTAssertEqual(pair[1].kind, EventKind.effectOutcome)
        await log.shutdown()
    }

    func test_tombstone_excludesEventFromQuery() async throws {
        let log = try await Log.open(path: dbPath)
        let id = try await log.appendAndReturnID(kind: EventKind.fact,
                                                  payload: .object(["v": .string("kenya")]))
        try await log.flush()
        let beforeTombstone = try await log.query(kind: EventKind.fact, since: 0)
        XCTAssertEqual(beforeTombstone.count, 1)
        await log.append(kind: EventKind.tombstone, tombstoneFor: id, payload: .null)
        try await log.flush()
        let afterTombstone = try await log.query(kind: EventKind.fact, since: 0)
        XCTAssertEqual(afterTombstone.count, 0)
        await log.shutdown()
    }

    func test_orderingIsByMonotonicClock_notWallClock() async throws {
        let log = try await Log.open(path: dbPath)
        await log.appendRaw(kind: EventKind.turnUser, tsWall: 1000, tsMono: 100, payload: .null)
        await log.appendRaw(kind: EventKind.turnUser, tsWall: 999, tsMono: 200, payload: .null)
        await log.appendRaw(kind: EventKind.turnUser, tsWall: 1001, tsMono: 300, payload: .null)
        try await log.flush()
        let rows = try await log.query(kind: EventKind.turnUser, since: 0)
        XCTAssertEqual(rows.map(\.tsMono), [100, 200, 300])
        await log.shutdown()
    }

    /// Amendment D: gemini_call_id roundtrip + indexed lookup.
    func test_geminiCallID_roundtrips() async throws {
        let log = try await Log.open(path: dbPath)
        await log.append(kind: EventKind.turnToolCall,
                          geminiCallID: "call-abc-123",
                          payload: .object(["name": .string("click")]))
        try await log.flush()
        let rows = try await log.queryGeminiCallID("call-abc-123")
        XCTAssertEqual(rows.count, 1)
        XCTAssertEqual(rows.first?.geminiCallID, "call-abc-123")
        await log.shutdown()
    }

    /// Amendment Q: flush returns AFTER the barrier is processed, not via sleep loop.
    func test_flush_barrierWaitsForPriorWrites() async throws {
        let log = try await Log.open(path: dbPath)
        for i in 0..<200 {
            await log.append(kind: EventKind.turnUser, payload: .object(["i": .number(Double(i))]))
        }
        try await log.flush()
        let rows = try await log.query(kind: EventKind.turnUser, since: 0)
        XCTAssertEqual(rows.count, 200, "all 200 writes must be visible after flush")
        await log.shutdown()
    }
}
