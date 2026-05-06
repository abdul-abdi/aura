import XCTest
@testable import Aura

// NOTE: TranscriptView itself is a SwiftUI View and cannot be unit-tested without
// a host app. These tests cover TranscriptViewModel, which contains all logic.

@MainActor
final class TranscriptViewTests: XCTestCase {
    var dbPath: String!
    var log: Log!

    override func setUp() async throws {
        dbPath = "/tmp/aura_transcript_\(UUID().uuidString).sqlite"
        log = try Log.open(path: dbPath)
    }

    override func tearDown() async throws {
        await log.shutdown()
        try? FileManager.default.removeItem(atPath: dbPath)
        try? FileManager.default.removeItem(atPath: dbPath + "-wal")
        try? FileManager.default.removeItem(atPath: dbPath + "-shm")
    }

    // MARK: - Initial load

    func test_load_allFourKindsProduceCorrectRows() async throws {
        // Seed one event of each kind.
        await log.append(kind: EventKind.turnUser,
                         payload: .object(["text": .string("hello")]))
        await log.append(kind: EventKind.turnAssistant,
                         payload: .object(["text": .string("world")]))
        await log.append(kind: EventKind.turnToolCall,
                         payload: .object([
                             "name": .string("openApp"),
                             "args": .object(["app": .string("Safari")]),
                         ]))
        await log.append(kind: EventKind.turnToolResult,
                         payload: .object([
                             "status": .string("ok"),
                             "detail": .string("done"),
                         ]))
        try await log.flush()

        let vm = TranscriptViewModel(log: log)
        await vm.refresh()

        XCTAssertEqual(vm.rows.count, 4, "Expected 4 rows, got \(vm.rows.count)")

        // Rows are sorted by ts_mono. The order above is insertion order;
        // verify each kind appears exactly once with the right content.
        let userRow = vm.rows.first(where: {
            if case .user = $0 { return true } else { return false }
        })
        let assistantRow = vm.rows.first(where: {
            if case .assistant = $0 { return true } else { return false }
        })
        let toolCallRow = vm.rows.first(where: {
            if case .toolCall = $0 { return true } else { return false }
        })
        let toolResultRow = vm.rows.first(where: {
            if case .toolResult = $0 { return true } else { return false }
        })

        XCTAssertNotNil(userRow)
        XCTAssertNotNil(assistantRow)
        XCTAssertNotNil(toolCallRow)
        XCTAssertNotNil(toolResultRow)

        if case .user(_, let text) = userRow {
            XCTAssertEqual(text, "hello")
        } else {
            XCTFail("user row missing or wrong type")
        }

        if case .assistant(_, let text) = assistantRow {
            XCTAssertEqual(text, "world")
        } else {
            XCTFail("assistant row missing or wrong type")
        }

        if case .toolCall(_, let name, _) = toolCallRow {
            XCTAssertEqual(name, "openApp")
        } else {
            XCTFail("toolCall row missing or wrong type")
        }

        if case .toolResult(_, let success, let detail) = toolResultRow {
            XCTAssertTrue(success)
            XCTAssertEqual(detail, "done")
        } else {
            XCTFail("toolResult row missing or wrong type")
        }
    }

    func test_toolResult_failureStatusMapsToSuccessFalse() async throws {
        await log.append(kind: EventKind.turnToolResult,
                         payload: .object([
                             "status": .string("failed"),
                             "detail": .string("permission denied"),
                         ]))
        try await log.flush()

        let vm = TranscriptViewModel(log: log)
        await vm.refresh()

        XCTAssertEqual(vm.rows.count, 1)
        if case .toolResult(_, let success, let detail) = vm.rows[0] {
            XCTAssertFalse(success)
            XCTAssertEqual(detail, "permission denied")
        } else {
            XCTFail("expected toolResult row")
        }
    }

    // MARK: - Incremental polling

    func test_newEventsAppearAfterSubsequentRefresh() async throws {
        // First batch: one user message already in the log.
        await log.append(kind: EventKind.turnUser,
                         payload: .object(["text": .string("first")]))
        try await log.flush()

        let vm = TranscriptViewModel(log: log)
        await vm.refresh()
        XCTAssertEqual(vm.rows.count, 1)

        // Simulate new event arriving after first refresh.
        await log.append(kind: EventKind.turnAssistant,
                         payload: .object(["text": .string("second")]))
        try await log.flush()

        // Second refresh should pick up the new event without duplicating the first.
        await vm.refresh()
        XCTAssertEqual(vm.rows.count, 2, "Expected 2 rows after second refresh, got \(vm.rows.count)")

        if case .assistant(_, let text) = vm.rows[1] {
            XCTAssertEqual(text, "second")
        } else {
            XCTFail("second row should be an assistant row")
        }
    }

    func test_noRowsDuplicated_onRepeatedRefresh() async throws {
        await log.append(kind: EventKind.turnUser,
                         payload: .object(["text": .string("hi")]))
        try await log.flush()

        let vm = TranscriptViewModel(log: log)
        await vm.refresh()
        await vm.refresh()  // second call with no new events

        XCTAssertEqual(vm.rows.count, 1, "Rows must not duplicate on repeat refresh")
    }

    func test_emptyLogProducesNoRows() async throws {
        let vm = TranscriptViewModel(log: log)
        await vm.refresh()
        XCTAssertTrue(vm.rows.isEmpty)
    }

    func test_defensivePayload_missingTextFieldFallsBackToEmpty() async throws {
        // Payload has no "text" key — should not crash, should produce empty string.
        await log.append(kind: EventKind.turnUser, payload: .object([:]))
        try await log.flush()

        let vm = TranscriptViewModel(log: log)
        await vm.refresh()

        XCTAssertEqual(vm.rows.count, 1)
        if case .user(_, let text) = vm.rows[0] {
            XCTAssertEqual(text, "")
        } else {
            XCTFail("expected user row")
        }
    }
}
