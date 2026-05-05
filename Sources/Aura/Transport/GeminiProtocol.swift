import Foundation

/// Wire-format encoders for Gemini's BidiGenerateContent protocol.
/// Reference: https://ai.google.dev/gemini-api/docs/live (April 2026).
enum GeminiProtocol {
    static func setupMessage(config: LiveSessionConfig) -> [String: Any] {
        var setup: [String: Any] = [
            // Amendment B: bare model name (no "models/" prefix).
            "model": config.model,
            "generationConfig": [
                "responseModalities": ["AUDIO"],
                "speechConfig": [
                    "voiceConfig": ["prebuiltVoiceConfig": ["voiceName": config.voice]]
                ],
            ] as [String: Any],
            "realtimeInputConfig": [
                "automaticActivityDetection": [
                    "startOfSpeechSensitivity": "START_SENSITIVITY_HIGH",
                    "endOfSpeechSensitivity": "END_SENSITIVITY_LOW",
                    "prefixPaddingMs": 20,
                    "silenceDurationMs": 200,
                ] as [String: Any]
            ] as [String: Any],
            "outputAudioTranscription": [String: Any](),
            "inputAudioTranscription": [String: Any](),
            // Spec §5.1 — sliding-window context compression at 500K tokens.
            // Server-managed; we declare the strategy and target here.
            "contextWindowCompression": [
                "slidingWindow": ["targetTokens": "500000"]
            ] as [String: Any],
            // Spec §5.1 — affective dialog enabled (model adjusts tone with
            // recognised emotional cues; user-facing setting in §11 toggles it).
            "enableAffectiveDialog": true,
            // Spec §5.1 — proactive audio disabled at the API level. We control
            // proactivity ourselves via §7's local policy pipeline; we don't
            // want the server unilaterally generating audio without our trigger.
            "proactivity": ["proactiveAudio": false] as [String: Any],
        ]
        if let prompt = config.systemPrompt {
            setup["systemInstruction"] = ["parts": [["text": prompt]]]
        }
        // Tool declarations: Plan 2 fills this in. We just pass through if any.
        if !config.toolDeclarations.isEmpty {
            setup["tools"] = config.toolDeclarations.map(jsonValueToAny)
        }
        if let handle = config.resumptionHandle {
            setup["sessionResumption"] = ["handle": handle]
        }
        return ["setup": setup]
    }

    static func realtimeInputAudio(_ base64PCM16: String) -> [String: Any] {
        ["realtimeInput": [
            "audio": ["data": base64PCM16, "mimeType": "audio/pcm;rate=16000"]
        ]]
    }

    static func realtimeInputVideoJPEG(_ base64JPEG: String) -> [String: Any] {
        ["realtimeInput": [
            "video": ["data": base64JPEG, "mimeType": "image/jpeg"]
        ]]
    }

    static func clientContentText(_ text: String) -> [String: Any] {
        ["clientContent": [
            "turns": [["role": "user", "parts": [["text": text]]]],
            "turnComplete": true,
        ]]
    }

    static func toolResponse(callID: String, output: JSONValue) -> [String: Any] {
        let outputAny = jsonValueToAny(output)
        return ["toolResponse": [
            "functionResponses": [["id": callID, "response": outputAny]]
        ]]
    }

    static func jsonValueToAny(_ v: JSONValue) -> Any {
        switch v {
        case .null: return NSNull()
        case .bool(let b): return b
        case .number(let n): return n
        case .string(let s): return s
        case .array(let a): return a.map(jsonValueToAny)
        case .object(let o): return o.mapValues(jsonValueToAny)
        }
    }
}
