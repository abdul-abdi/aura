//! Gemini Live API WebSocket protocol message types.
//!
//! Client-to-server messages are [`Serialize`], server-to-client messages are
//! [`Deserialize`]. Types that appear in both directions (e.g. [`Part`],
//! [`Blob`]) derive both.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Client -> Server messages
// ---------------------------------------------------------------------------

/// Initial setup message sent when a session is opened.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupMessage {
    pub setup: Setup,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Setup {
    pub model: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GenerationConfig>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<Content>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_resumption: Option<SessionResumptionConfig>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window_compression: Option<ContextWindowCompression>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_modalities: Option<Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub speech_config: Option<SpeechConfig>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechConfig {
    pub voice_config: VoiceConfig,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceConfig {
    pub prebuilt_voice_config: PrebuiltVoiceConfig,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrebuiltVoiceConfig {
    pub voice_name: String,
}

/// A content block containing one or more parts.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Content {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,

    pub parts: Vec<Part>,
}

/// A single content part — either text or binary data.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline_data: Option<Blob>,
}

/// Binary data with a MIME type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Blob {
    pub mime_type: String,
    pub data: String,
}

/// A tool definition with function declarations.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub function_declarations: Vec<FunctionDeclaration>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionDeclaration {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResumptionConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextWindowCompression {
    pub sliding_window: SlidingWindow,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlidingWindow {}

/// Real-time audio input sent during a session.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RealtimeInputMessage {
    pub realtime_input: RealtimeInput,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RealtimeInput {
    pub media_chunks: Vec<MediaChunk>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaChunk {
    pub mime_type: String,
    pub data: String,
}

/// Text content sent to the server during a session.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientContentMessage {
    pub client_content: ClientContent,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientContent {
    pub turns: Vec<Content>,
    pub turn_complete: bool,
}

/// Tool response sent back to the server after a function call.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResponseMessage {
    pub tool_response: ToolResponse,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResponse {
    pub function_responses: Vec<FunctionResponse>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionResponse {
    pub id: String,
    pub name: String,
    pub response: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Server -> Client messages
// ---------------------------------------------------------------------------

/// Top-level server message — a flat struct with optional fields.
/// Unknown fields are silently ignored to tolerate API additions.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerMessage {
    #[serde(default)]
    pub setup_complete: Option<SetupComplete>,

    #[serde(default)]
    pub server_content: Option<ServerContent>,

    #[serde(default)]
    pub tool_call: Option<ToolCall>,

    #[serde(default)]
    pub tool_call_cancellation: Option<ToolCallCancellation>,

    #[serde(default)]
    pub go_away: Option<serde_json::Value>,

    #[serde(default)]
    pub session_resumption_update: Option<SessionResumptionUpdate>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupComplete {}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerContent {
    #[serde(default)]
    pub model_turn: Option<ModelTurn>,

    #[serde(default)]
    pub turn_complete: Option<bool>,

    #[serde(default)]
    pub interrupted: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTurn {
    pub parts: Vec<Part>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub function_calls: Vec<FunctionCall>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionCall {
    pub id: String,
    pub name: String,
    pub args: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallCancellation {
    pub ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResumptionUpdate {
    #[serde(default)]
    pub new_handle: Option<String>,
    #[serde(default)]
    pub resumable: Option<bool>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serialize_setup_message() {
        let msg = SetupMessage {
            setup: Setup {
                model: "models/gemini-2.0-flash-exp".into(),
                generation_config: Some(GenerationConfig {
                    temperature: Some(0.7),
                    response_modalities: Some(vec!["AUDIO".into()]),
                    speech_config: Some(SpeechConfig {
                        voice_config: VoiceConfig {
                            prebuilt_voice_config: PrebuiltVoiceConfig {
                                voice_name: "Aoede".into(),
                            },
                        },
                    }),
                }),
                system_instruction: Some(Content {
                    role: None,
                    parts: vec![Part {
                        text: Some("You are a helpful assistant.".into()),
                        inline_data: None,
                    }],
                }),
                tools: None,
                session_resumption: None,
                context_window_compression: None,
            },
        };

        let value = serde_json::to_value(&msg).unwrap();

        assert_eq!(value["setup"]["model"], "models/gemini-2.0-flash-exp");
        assert_eq!(value["setup"]["generationConfig"]["temperature"], 0.7);
        assert_eq!(
            value["setup"]["generationConfig"]["responseModalities"][0],
            "AUDIO"
        );
        assert_eq!(
            value["setup"]["generationConfig"]["speechConfig"]["voiceConfig"]["prebuiltVoiceConfig"]
                ["voiceName"],
            "Aoede"
        );
        assert_eq!(
            value["setup"]["systemInstruction"]["parts"][0]["text"],
            "You are a helpful assistant."
        );
        // Optional fields that are None must be absent
        assert!(value["setup"].get("tools").is_none());
        assert!(value["setup"].get("sessionResumption").is_none());
    }

    #[test]
    fn serialize_realtime_input() {
        let msg = RealtimeInputMessage {
            realtime_input: RealtimeInput {
                media_chunks: vec![MediaChunk {
                    mime_type: "audio/pcm;rate=16000".into(),
                    data: "AQIDBA==".into(),
                }],
            },
        };

        let value = serde_json::to_value(&msg).unwrap();

        assert_eq!(
            value["realtimeInput"]["mediaChunks"][0]["mimeType"],
            "audio/pcm;rate=16000"
        );
        assert_eq!(value["realtimeInput"]["mediaChunks"][0]["data"], "AQIDBA==");
    }

    #[test]
    fn deserialize_setup_complete() {
        let raw = r#"{"setupComplete":{}}"#;
        let msg: ServerMessage = serde_json::from_str(raw).unwrap();

        assert!(msg.setup_complete.is_some());
        assert!(msg.server_content.is_none());
    }

    #[test]
    fn deserialize_server_content_with_audio() {
        let raw = json!({
            "serverContent": {
                "modelTurn": {
                    "parts": [{
                        "inlineData": {
                            "mimeType": "audio/pcm;rate=24000",
                            "data": "AQIDBAUG"
                        }
                    }]
                }
            }
        });

        let msg: ServerMessage = serde_json::from_str(&raw.to_string()).unwrap();
        let content = msg.server_content.unwrap();
        let turn = content.model_turn.unwrap();
        let part = &turn.parts[0];

        let blob = part.inline_data.as_ref().unwrap();
        assert_eq!(blob.mime_type, "audio/pcm;rate=24000");
        assert_eq!(blob.data, "AQIDBAUG");
    }

    #[test]
    fn deserialize_interrupted() {
        let raw = r#"{"serverContent":{"interrupted":true}}"#;
        let msg: ServerMessage = serde_json::from_str(raw).unwrap();

        let content = msg.server_content.unwrap();
        assert_eq!(content.interrupted, Some(true));
        assert!(content.model_turn.is_none());
    }

    #[test]
    fn deserialize_tool_call() {
        let raw = json!({
            "toolCall": {
                "functionCalls": [{
                    "id": "call-123",
                    "name": "get_weather",
                    "args": {
                        "location": "San Francisco"
                    }
                }]
            }
        });

        let msg: ServerMessage = serde_json::from_str(&raw.to_string()).unwrap();
        let tool_call = msg.tool_call.unwrap();
        let fc = &tool_call.function_calls[0];

        assert_eq!(fc.id, "call-123");
        assert_eq!(fc.name, "get_weather");
        assert_eq!(fc.args["location"], "San Francisco");
    }

    #[test]
    fn deserialize_session_resumption_update() {
        let raw = json!({
            "sessionResumptionUpdate": {
                "newHandle": "abc-resume-handle-xyz"
            }
        });

        let msg: ServerMessage = serde_json::from_str(&raw.to_string()).unwrap();
        let update = msg.session_resumption_update.unwrap();
        assert_eq!(update.new_handle.unwrap(), "abc-resume-handle-xyz");
    }

    #[test]
    fn deserialize_session_resumption_update_without_handle() {
        let raw = json!({
            "sessionResumptionUpdate": {
                "resumable": true
            }
        });

        let msg: ServerMessage = serde_json::from_str(&raw.to_string()).unwrap();
        let update = msg.session_resumption_update.unwrap();
        assert!(update.new_handle.is_none());
        assert_eq!(update.resumable, Some(true));
    }

    #[test]
    fn serialize_client_content_message() {
        let msg = ClientContentMessage {
            client_content: ClientContent {
                turns: vec![Content {
                    role: Some("user".into()),
                    parts: vec![Part {
                        text: Some("Hello, world!".into()),
                        inline_data: None,
                    }],
                }],
                turn_complete: true,
            },
        };
        let value = serde_json::to_value(&msg).unwrap();
        assert_eq!(value["clientContent"]["turns"][0]["role"], "user");
        assert_eq!(
            value["clientContent"]["turns"][0]["parts"][0]["text"],
            "Hello, world!"
        );
        assert_eq!(value["clientContent"]["turnComplete"], true);
    }

    #[test]
    fn serialize_tool_response() {
        let msg = ToolResponseMessage {
            tool_response: ToolResponse {
                function_responses: vec![FunctionResponse {
                    id: "call-123".into(),
                    name: "get_weather".into(),
                    response: json!({
                        "temperature": 72,
                        "unit": "fahrenheit"
                    }),
                }],
            },
        };

        let value = serde_json::to_value(&msg).unwrap();

        let resp = &value["toolResponse"]["functionResponses"][0];
        assert_eq!(resp["id"], "call-123");
        assert_eq!(resp["name"], "get_weather");
        assert_eq!(resp["response"]["temperature"], 72);
        assert_eq!(resp["response"]["unit"], "fahrenheit");
    }
}
