import XCTest
@testable import Aura

final class LiveFrameCodingTests: XCTestCase {
    func test_liveFrameEnumIsSendableAndCases() {
        let frame: LiveFrame = .audio(Data([0, 1, 2]))
        switch frame {
        case .audio: XCTAssertTrue(true)
        default: XCTFail()
        }
    }

    func test_eventEnum() {
        let e: LiveEvent = .toolCall(callID: "id1", name: "click", args: .null)
        if case .toolCall(let id, _, _) = e { XCTAssertEqual(id, "id1") } else { XCTFail() }
    }

    func test_configDefaultModelIsCurrentLiveModel() {
        let cfg = LiveSessionConfig(apiKey: "test")
        XCTAssertEqual(cfg.model, "gemini-live-2.5-flash-native-audio")
    }
}
