import XCTest
@testable import AuraV3

final class GeminiProtocolTests: XCTestCase {
    func test_setupMessage_usesBareModelName() {
        var cfg = LiveSessionConfig(apiKey: "test")
        cfg.model = "gemini-live-2.5-flash-native-audio"
        let setup = GeminiProtocol.setupMessage(config: cfg)
        let inner = setup["setup"] as? [String: Any]
        XCTAssertEqual(inner?["model"] as? String, "gemini-live-2.5-flash-native-audio",
                       "Amendment B: NO 'models/' prefix")
    }

    func test_setupMessage_includesAudioModalityAndVoice() {
        let cfg = LiveSessionConfig(apiKey: "test")
        let setup = GeminiProtocol.setupMessage(config: cfg)
        let inner = setup["setup"] as? [String: Any]
        let gen = inner?["generationConfig"] as? [String: Any]
        XCTAssertEqual(gen?["responseModalities"] as? [String], ["AUDIO"])
        let voice = (((gen?["speechConfig"] as? [String: Any])?["voiceConfig"] as? [String: Any])?["prebuiltVoiceConfig"] as? [String: Any])?["voiceName"] as? String
        XCTAssertEqual(voice, "Kore")
    }

    func test_setupMessage_includesVADConfig() {
        let cfg = LiveSessionConfig(apiKey: "test")
        let setup = GeminiProtocol.setupMessage(config: cfg)
        let vad = (((setup["setup"] as? [String: Any])?["realtimeInputConfig"] as? [String: Any])?["automaticActivityDetection"] as? [String: Any])
        XCTAssertEqual(vad?["startOfSpeechSensitivity"] as? String, "START_SENSITIVITY_HIGH")
        XCTAssertEqual(vad?["endOfSpeechSensitivity"] as? String, "END_SENSITIVITY_LOW")
        XCTAssertEqual(vad?["prefixPaddingMs"] as? Int, 20)
        XCTAssertEqual(vad?["silenceDurationMs"] as? Int, 200)
    }

    func test_realtimeInputAudio_encodesMimeType() {
        let json = GeminiProtocol.realtimeInputAudio("ZmFrZUF1ZGlv")
        let inner = (json["realtimeInput"] as? [String: Any])?["audio"] as? [String: Any]
        XCTAssertEqual(inner?["mimeType"] as? String, "audio/pcm;rate=16000")
        XCTAssertEqual(inner?["data"] as? String, "ZmFrZUF1ZGlv")
    }

    func test_toolResponse_payloadShape() {
        let out: JSONValue = .object(["status": .string("ok"), "x": .number(42)])
        let json = GeminiProtocol.toolResponse(callID: "call-7", output: out)
        let resp = ((json["toolResponse"] as? [String: Any])?["functionResponses"] as? [[String: Any]])?.first
        XCTAssertEqual(resp?["id"] as? String, "call-7")
        let r = resp?["response"] as? [String: Any]
        XCTAssertEqual(r?["status"] as? String, "ok")
        XCTAssertEqual((r?["x"] as? NSNumber)?.intValue, 42)
    }

    func test_jsonValueToAny_roundtrip() throws {
        let v: JSONValue = .object([
            "n": .null,
            "b": .bool(true),
            "i": .number(42),
            "s": .string("hi"),
            "a": .array([.number(1), .number(2)]),
            "o": .object(["k": .string("v")]),
        ])
        let any = GeminiProtocol.jsonValueToAny(v)
        // Roundtrip through JSONSerialization to verify shape.
        let data = try JSONSerialization.data(withJSONObject: any)
        let decoded = try JSONDecoder().decode(JSONValue.self, from: data)
        XCTAssertEqual(decoded, v)
    }
}
