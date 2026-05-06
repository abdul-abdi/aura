import XCTest
@testable import Aura

final class ToolResultTests: XCTestCase {
    func test_okHasOutputAndNoFailureReason() {
        let r = ToolResult.ok(output: .string("clicked"), detail: "verified by AX diff")
        XCTAssertEqual(r.status, .ok)
        XCTAssertEqual(r.output?.stringValue, "clicked")
        XCTAssertNil(r.failureReason)
    }

    func test_failedHasReasonAndDetail() {
        let r = ToolResult.failed(reason: .noStateChange, detail: "posted CGEvent, no AX diff")
        XCTAssertEqual(r.status, .failed)
        XCTAssertEqual(r.failureReason, .noStateChange)
        XCTAssertEqual(r.detail, "posted CGEvent, no AX diff")
    }

    func test_needsClarificationCarriesQuestion() {
        let r = ToolResult.needsClarification("Which Submit — toolbar or dialog?")
        XCTAssertEqual(r.status, .needsClarification)
        XCTAssertEqual(r.question, "Which Submit — toolbar or dialog?")
    }

    func test_serializesToGeminiResponseShape() {
        let r = ToolResult.failed(reason: .targetNotFound, detail: "no AX match")
        guard case .object(let dict) = r.toGeminiResponse() else {
            XCTFail("expected object"); return
        }
        XCTAssertEqual(dict["status"]?.stringValue, "failed")
        XCTAssertEqual(dict["failure_reason"]?.stringValue, "targetNotFound")
        XCTAssertEqual(dict["detail"]?.stringValue, "no AX match")
    }

    func test_failureReasonCodes_areStableForWire() {
        XCTAssertEqual(ToolFailureReason.noStateChange.code, "noStateChange")
        XCTAssertEqual(ToolFailureReason.targetNotFound.code, "targetNotFound")
        XCTAssertEqual(ToolFailureReason.timeout.code, "timeout")
        XCTAssertEqual(ToolFailureReason.permissionDenied.code, "permissionDenied")
        XCTAssertEqual(ToolFailureReason.rotationForced.code, "rotationForced")
        XCTAssertEqual(ToolFailureReason.rateLimited.code, "rateLimited")
        XCTAssertEqual(ToolFailureReason.preconditionFailed(reason: "x").code, "preconditionFailed(x)")
        XCTAssertEqual(ToolFailureReason.toolError(code: "log_query").code, "toolError(log_query)")
    }
}
