import XCTest
import AppKit
@testable import Aura

final class ClipboardToolsTests: XCTestCase {
    func test_readReturnsClipboardString_withIDPIPrefix() async {
        await MainActor.run {
            let pb = NSPasteboard.general
            pb.clearContents()
            pb.setString("test clipboard content", forType: .string)
        }
        let tools = ClipboardTools()
        let result = await tools.read()
        XCTAssertEqual(result.status, .ok)
        XCTAssertEqual(result.output?["_untrusted"], .bool(true))
        let text = result.output?["_text"]?.stringValue ?? ""
        XCTAssertTrue(text.starts(with: "[UNTRUSTED INPUT"))
        XCTAssertTrue(text.contains("test clipboard content"))
        XCTAssertEqual(result.output?["length"]?.numberValue, 22)
    }

    func test_readReturnsFailedWhenClipboardEmpty() async {
        _ = await MainActor.run {
            NSPasteboard.general.clearContents()
        }
        let tools = ClipboardTools()
        let result = await tools.read()
        XCTAssertEqual(result.status, .failed)
        XCTAssertEqual(result.failureReason, .targetNotFound)
    }
}
