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

    func testResamplerConvertsTo16kAcrossBuffers() {
        // 48 kHz ramp split into uneven buffers must come out as one continuous 16 kHz ramp.
        let input = (0..<4800).map { Float($0) / 4800 }
        var resampler = Resampler(from: 48_000)
        var output: [Float] = []
        var start = 0
        for size in [1000, 7, 1793, 2000] {
            input[start..<(start + size)].withUnsafeBufferPointer { output += resampler.process($0) }
            start += size
        }
        XCTAssertEqual(output.count, 1600, accuracy: 1)
        for (i, value) in output.enumerated() {
            XCTAssertEqual(value, Float(i * 3) / 4800, accuracy: 1e-4)
        }

        var resampler44 = Resampler(from: 44_100)
        let ones = [Float](repeating: 0.5, count: 4410)
        let out44 = ones.withUnsafeBufferPointer { resampler44.process($0) }
        XCTAssertEqual(out44.count, 1600, accuracy: 1)
        XCTAssertTrue(out44.dropFirst().allSatisfy { abs($0 - 0.5) < 1e-6 })
    }

    func testHistorySearchMatchesAllWordsMostRecentFirst() {
        let history = """
            Oct 1, 2026 at 9:00 AM you: book a dentist appointment
            Oct 2, 2026 at 9:00 AM aura: Your dentist is on Friday
            Oct 3, 2026 at 9:00 AM you: open Safari
            Oct 4, 2026 at 9:00 AM you: move the Dentist appointment
            """
        let found = Memory.search("dentist appointment", in: history, limit: 1)
        XCTAssertEqual(found, ["Oct 4, 2026 at 9:00 AM you: move the Dentist appointment"])
        XCTAssertEqual(Memory.search("dentist", in: history, limit: 10).count, 3)
        XCTAssertTrue(Memory.search("piano", in: history, limit: 10).isEmpty)
    }

    func testToolRejectsMissingArguments() async {
        let result = await Tools.run("open", [:])
        XCTAssertNotNil(result["error"])
    }
}
