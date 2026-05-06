import XCTest
@testable import Aura

final class ToolRegistryTests: XCTestCase {
    var dbPath: String!
    var log: Log!
    var effects: EffectLog!

    override func setUp() async throws {
        dbPath = "/tmp/aura_reg_\(UUID().uuidString).sqlite"
        log = try Log.open(path: dbPath)
        effects = EffectLog(log: log)
    }
    override func tearDown() async throws {
        await log.shutdown()
        try? FileManager.default.removeItem(atPath: dbPath)
        try? FileManager.default.removeItem(atPath: dbPath + "-wal")
        try? FileManager.default.removeItem(atPath: dbPath + "-shm")
    }

    func test_unknownToolReturnsFailed() async {
        let registry = ToolRegistry(effects: effects)
        let result = await registry.dispatch(name: "nonexistent_tool", args: .null)
        XCTAssertEqual(result.status, .failed)
        XCTAssertEqual(result.failureReason?.code, "preconditionFailed(unknown_tool)")
    }

    func test_registeredReadOnlyToolIsCalled() async {
        let registry = ToolRegistry(effects: effects)
        await registry.register("echo") { args in
            return .ok(output: args)
        }
        let result = await registry.dispatch(name: "echo", args: .string("hello"))
        XCTAssertEqual(result.status, .ok)
        XCTAssertEqual(result.output?.stringValue, "hello")
    }

    func test_mutatingToolWritesPostedAndOutcomeEvents() async throws {
        let registry = ToolRegistry(effects: effects)
        await registry.registerMutating("toggle",
            intent: { _ in EffectIntent(operation: "toggle", args: .null) }
        ) { _, _, _ in
            return .ok(detail: "toggled")
        }
        let result = await registry.dispatch(name: "toggle", args: .null, geminiCallID: "call-1")
        XCTAssertEqual(result.status, .ok)
        try await log.flush()
        let posted = try await log.query(kind: EventKind.effectPosted, since: 0)
        let outcomes = try await log.query(kind: EventKind.effectOutcome, since: 0)
        XCTAssertEqual(posted.count, 1)
        XCTAssertEqual(outcomes.count, 1)
        XCTAssertEqual(posted.first?.geminiCallID, "call-1")
        XCTAssertEqual(outcomes.first?.geminiCallID, "call-1")
        XCTAssertEqual(posted.first?.correlationID, outcomes.first?.correlationID)
    }

    func test_concurrentDispatchHonorsMaxConcurrency() async {
        let registry = ToolRegistry(effects: effects, maxConcurrent: 2)
        let counter = ConcurrencyCounter()
        await registry.register("slow") { _ in
            await counter.enter()
            try? await Task.sleep(nanoseconds: 30_000_000)
            await counter.exit()
            return .ok()
        }
        await withTaskGroup(of: Void.self) { group in
            for _ in 0..<5 {
                group.addTask { _ = await registry.dispatch(name: "slow", args: .null) }
            }
        }
        let observed = await counter.observedMax
        XCTAssertLessThanOrEqual(observed, 2)
    }
}

private actor ConcurrencyCounter {
    var value: Int = 0
    var observedMax: Int = 0
    func enter() {
        value += 1
        if value > observedMax { observedMax = value }
    }
    func exit() { value -= 1 }
}
