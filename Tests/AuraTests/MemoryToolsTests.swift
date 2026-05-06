import XCTest
@testable import Aura

final class MemoryToolsTests: XCTestCase {
    var dbPath: String!
    var log: Log!

    override func setUp() async throws {
        dbPath = "/tmp/aura_mem_\(UUID().uuidString).sqlite"
        log = try Log.open(path: dbPath)
    }
    override func tearDown() async throws {
        await log.shutdown()
        try? FileManager.default.removeItem(atPath: dbPath)
        try? FileManager.default.removeItem(atPath: dbPath + "-wal")
        try? FileManager.default.removeItem(atPath: dbPath + "-shm")
    }

    func test_rememberThenRecall_returnsTheFact() async throws {
        let tools = MemoryTools(log: log)
        _ = await tools.remember(entity: "user", predicate: "lives_in", value: "Kenya")
        try await log.flush()
        let result = await tools.recall(query: "Kenya")
        XCTAssertEqual(result.status, .ok)
        guard case .array(let arr) = result.output ?? .null else {
            XCTFail("expected array output"); return
        }
        XCTAssertEqual(arr.count, 1)
        XCTAssertEqual(arr.first?["value"]?.stringValue, "Kenya")
        XCTAssertEqual(arr.first?["_untrusted"], .bool(true), "amendment AJ — recall must mark output untrusted")
        XCTAssertTrue(arr.first?["_text"]?.stringValue?.starts(with: "[UNTRUSTED INPUT") ?? false)
    }

    func test_recall_returnsMostRecentFirst_ampendmentAP() async throws {
        let tools = MemoryTools(log: log)
        _ = await tools.remember(entity: "user", predicate: "fav_color", value: "blue")
        try await log.flush()
        try await Task.sleep(nanoseconds: 1_000_000) // ensure different ts_mono
        _ = await tools.remember(entity: "user", predicate: "fav_color", value: "green")
        try await log.flush()
        let result = await tools.recall(query: "fav_color")
        guard case .array(let arr) = result.output ?? .null else {
            XCTFail("expected array"); return
        }
        XCTAssertEqual(arr.count, 2)
        // most-recent first
        XCTAssertEqual(arr.first?["value"]?.stringValue, "green")
        XCTAssertEqual(arr.last?["value"]?.stringValue, "blue")
    }

    func test_recall_capsAtLimit() async throws {
        let tools = MemoryTools(log: log)
        for i in 0..<12 {
            _ = await tools.remember(entity: "user", predicate: "pref", value: "p\(i)")
        }
        try await log.flush()
        let result = await tools.recall(query: "p")
        guard case .array(let arr) = result.output ?? .null else {
            XCTFail("expected array"); return
        }
        XCTAssertEqual(arr.count, 8, "amendment AP — recall capped at 8")
    }

    func test_forget_makesFactDisappearFromRecall() async throws {
        let tools = MemoryTools(log: log)
        _ = await tools.remember(entity: "user", predicate: "lives_in", value: "Kenya")
        try await log.flush()

        let facts = try await log.query(kind: EventKind.fact, since: 0)
        let id = facts.first!.id

        _ = await tools.forget(id: id)
        try await log.flush()

        let result = await tools.recall(query: "Kenya")
        if case .array(let arr) = result.output ?? .null {
            XCTAssertTrue(arr.isEmpty, "tombstoned fact must not appear in recall")
        } else {
            XCTFail("expected array")
        }
    }

    func test_recallEmptyQuery_returnsAllFactsCappedAtLimit() async throws {
        let tools = MemoryTools(log: log)
        _ = await tools.remember(entity: "user", predicate: "name", value: "Abdullahi")
        _ = await tools.remember(entity: "user", predicate: "lives_in", value: "Kenya")
        try await log.flush()
        let result = await tools.recall(query: "")
        guard case .array(let arr) = result.output ?? .null else {
            XCTFail("expected array"); return
        }
        XCTAssertEqual(arr.count, 2)
    }

    // MARK: - Adapter ToolImpl tests

    func test_rememberToolImpl_failsOnMissingArgs() async {
        let tools = MemoryTools(log: log)
        let impl = tools.rememberToolImpl()
        let result = await impl(.object(["entity": .string("user")]))
        XCTAssertEqual(result.status, .failed)
        XCTAssertEqual(result.failureReason?.code, "preconditionFailed(missing_arg)")
    }

    func test_rememberToolImpl_succeedsOnFullArgs() async throws {
        let tools = MemoryTools(log: log)
        let impl = tools.rememberToolImpl()
        let result = await impl(.object([
            "entity":    .string("user"),
            "predicate": .string("lives_in"),
            "value":     .string("Kenya"),
        ]))
        XCTAssertEqual(result.status, .ok)
        try await log.flush()
        let facts = try await log.query(kind: EventKind.fact, since: 0)
        XCTAssertEqual(facts.count, 1)
    }
}
