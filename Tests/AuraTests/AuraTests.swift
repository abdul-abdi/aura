import CoreGraphics
import Foundation
import XCTest

@testable import Aura

final class AuraTests: XCTestCase {
    func testConfigParsing() {
        let values = Config.parse("""
            # Aura settings
            api_key = "AIzaSyTest1234567890abc"
            model = gemini-3.8-live
            voice="Puck"
            """)
        XCTAssertEqual(values["api_key"], "AIzaSyTest1234567890abc")
        XCTAssertEqual(values["model"], "gemini-3.8-live")
        XCTAssertEqual(values["voice"], "Puck")
        XCTAssertNil(values["# Aura settings"])
    }

    func testSetupMessageMatchesLiveAPI() throws {
        let message = Live.setupMessage(
            model: "gemini-3.8-live", voice: "Kore", instructions: "Be brief.",
            tools: Tools.declarations, resumeHandle: nil)
        XCTAssertTrue(JSONSerialization.isValidJSONObject(message))
        let setup = try XCTUnwrap(message["setup"] as? [String: Any])
        XCTAssertEqual(setup["model"] as? String, "models/gemini-3.8-live")
        XCTAssertNotNil(setup["outputAudioTranscription"])
        XCTAssertNotNil(setup["inputAudioTranscription"])
        XCTAssertNotNil(setup["contextWindowCompression"])
        let generation = try XCTUnwrap(setup["generationConfig"] as? [String: Any])
        XCTAssertEqual(generation["responseModalities"] as? [String], ["AUDIO"])
        // Not accepted by current Live models.
        XCTAssertNil(generation["temperature"])
        XCTAssertNil(generation["thinkingConfig"])
        let tools = try XCTUnwrap(setup["tools"] as? [[String: Any]])
        XCTAssertNotNil(tools.last?["googleSearch"])

        let resumed = Live.setupMessage(
            model: "models/x", voice: "Kore", instructions: "", tools: [], resumeHandle: "h1")
        let resumedSetup = try XCTUnwrap(resumed["setup"] as? [String: Any])
        XCTAssertEqual(resumedSetup["model"] as? String, "models/x")
        XCTAssertEqual((resumedSetup["sessionResumption"] as? [String: Any])?["handle"] as? String, "h1")
    }

    func testToolDeclarationsAreWellFormed() throws {
        let names = Tools.declarations.compactMap { $0["name"] as? String }
        XCTAssertEqual(names.count, Tools.declarations.count)
        XCTAssertEqual(Set(names).count, names.count, "duplicate tool names")
        for declaration in Tools.declarations {
            XCTAssertFalse((declaration["description"] as? String ?? "").isEmpty)
            XCTAssertEqual(declaration["behavior"] as? String, "BLOCKING")
            guard let parameters = declaration["parameters"] as? [String: Any] else { continue }
            let properties = try XCTUnwrap(parameters["properties"] as? [String: Any])
            for required in parameters["required"] as? [String] ?? [] {
                XCTAssertNotNil(properties[required], "\(names) requires missing \(required)")
            }
        }
        XCTAssertTrue(Tools.changesScreen.isSubset(of: Set(names)))
    }

    func testCoordinateMapping() {
        let display = CGRect(x: 100, y: 50, width: 2000, height: 1000)
        XCTAssertEqual(Screen.point(x: 0, y: 0, in: display), CGPoint(x: 100, y: 50))
        XCTAssertEqual(Screen.point(x: 500, y: 500, in: display), CGPoint(x: 1100, y: 550))
        let back = Screen.normalized(CGPoint(x: 1100, y: 550), in: display)
        XCTAssertEqual(back.x, 500)
        XCTAssertEqual(back.y, 500)
    }

    func testKeyCodes() {
        XCTAssertEqual(Input.keyCodes["a"], 0)
        XCTAssertEqual(Input.keyCodes["return"], 36)
        XCTAssertEqual(Input.keyCodes["escape"], 53)
        XCTAssertEqual(Input.keyCodes["f1"], 122)
        XCTAssertEqual(Input.keyCodes["f12"], 111)
    }

    func testInstructionsIncludeMemories() {
        let text = Prompt.instructions(facts: ["Prefers Safari"])
        XCTAssertTrue(text.contains("- Prefers Safari"))
        XCTAssertFalse(Prompt.instructions(facts: []).contains("What you remember"))
    }

    func testToolRejectsMissingArguments() async {
        let result = await Tools.run("open", [:])
        XCTAssertNotNil(result["error"])
    }
}
