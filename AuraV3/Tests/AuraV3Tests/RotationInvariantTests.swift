import XCTest
@testable import AuraV3

final class RotationInvariantTests: XCTestCase {
    func test_digestIsPureFunction() {
        let log = StubLogView()
        let d1 = RotationDigest.build(
            log: log, activeGoalStack: ["v3 spec"],
            pendingTool: nil, rollingSummary: "hello", inferredSubgoal: nil
        )
        let d2 = RotationDigest.build(
            log: log, activeGoalStack: ["v3 spec"],
            pendingTool: nil, rollingSummary: "hello", inferredSubgoal: nil
        )
        XCTAssertEqual(d1, d2)
    }

    func test_digestIncludesPendingToolIntent() {
        let log = StubLogView()
        let pending = PendingToolIntent(operation: "click", args: .object(["target": .string("Send")]))
        let digest = RotationDigest.build(
            log: log, activeGoalStack: ["test"],
            pendingTool: pending, rollingSummary: "summary", inferredSubgoal: nil
        )
        XCTAssertTrue(digest.contains("click"))
        XCTAssertTrue(digest.contains("Send"))
    }

    func test_digestIncludesAllNonNilFields() {
        let log = StubLogView()
        let digest = RotationDigest.build(
            log: log, activeGoalStack: ["g1", "g2"],
            pendingTool: nil, rollingSummary: "r", inferredSubgoal: "sg"
        )
        XCTAssertTrue(digest.contains("g1"))
        XCTAssertTrue(digest.contains("g2"))
        XCTAssertTrue(digest.contains("sg"))
        XCTAssertTrue(digest.contains("r"))
    }

    func test_digestOmitsEmptyFields() {
        let log = StubLogView()
        let digest = RotationDigest.build(
            log: log, activeGoalStack: [],
            pendingTool: nil, rollingSummary: "", inferredSubgoal: nil
        )
        // Should still include the header line but no goal stack / context lines.
        XCTAssertTrue(digest.contains("[Aura session continuation]"))
        XCTAssertFalse(digest.contains("Active goal stack"))
        XCTAssertFalse(digest.contains("Recent context"))
    }
}

private struct StubLogView: LogView {
    func recentTurns(limit: Int) async -> [LogEvent] { [] }
}
