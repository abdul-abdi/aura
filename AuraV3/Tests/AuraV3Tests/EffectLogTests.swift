import XCTest
@testable import AuraV3

final class EffectLogTests: XCTestCase {
    var dbPath: String!
    var log: Log!

    override func setUp() async throws {
        dbPath = "/tmp/aura_effect_\(UUID().uuidString).sqlite"
        log = try Log.open(path: dbPath)  // synchronous per amendment J
    }

    override func tearDown() async throws {
        await log.shutdown()
        try? FileManager.default.removeItem(atPath: dbPath)
        try? FileManager.default.removeItem(atPath: dbPath + "-wal")
        try? FileManager.default.removeItem(atPath: dbPath + "-shm")
    }

    func test_executeWithEffect_writesPostedThenOutcome() async throws {
        let effects = EffectLog(log: log)
        let result = try await effects.executeWithEffect(
            intent: EffectIntent(operation: "click", args: .object(["target": .string("Send")])),
            body: { return JSONValue.string("ok") }
        )
        XCTAssertEqual(result.stringValue, "ok")
        try await log.flush()
        let posted = try await log.query(kind: EventKind.effectPosted, since: 0)
        let outcome = try await log.query(kind: EventKind.effectOutcome, since: 0)
        XCTAssertEqual(posted.count, 1)
        XCTAssertEqual(outcome.count, 1)
        XCTAssertEqual(posted[0].correlationID, outcome[0].correlationID)
    }

    func test_orphanDetector_findsPostedWithoutOutcome() async throws {
        await log.append(kind: EventKind.effectPosted,
                          correlationID: "orphan-1",
                          payload: .object(["operation": .string("click")]))
        try await log.flush()
        let orphans = try await EffectLog.findOrphans(log: log)
        XCTAssertEqual(orphans, ["orphan-1"])
    }

    func test_executeWithEffect_evenOnThrow_writesOutcome() async throws {
        let effects = EffectLog(log: log)
        do {
            _ = try await effects.executeWithEffect(
                intent: EffectIntent(operation: "click", args: .null),
                body: { throw NSError(domain: "test", code: 99) }
            )
            XCTFail("expected throw")
        } catch {
            // expected
        }
        try await log.flush()
        let outcome = try await log.query(kind: EventKind.effectOutcome, since: 0)
        XCTAssertEqual(outcome.count, 1)
        XCTAssertEqual(outcome[0].payload["status"]?.stringValue, "failed")
    }

    /// Amendment D: gemini_call_id is stored on both posted AND outcome rows.
    func test_executeWithEffect_storesGeminiCallID() async throws {
        let effects = EffectLog(log: log)
        _ = try await effects.executeWithEffect(
            intent: EffectIntent(operation: "click", args: .null),
            geminiCallID: "call-xyz",
            body: { return JSONValue.null }
        )
        try await log.flush()
        let rows = try await log.queryGeminiCallID("call-xyz")
        XCTAssertEqual(rows.count, 2, "both effect.posted and effect.outcome should store the gemini_call_id")
        XCTAssertEqual(rows.map(\.kind).sorted(), [EventKind.effectOutcome, EventKind.effectPosted].sorted())
    }
}
