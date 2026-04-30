import Foundation

public enum LiveFrame: Sendable {
    /// 16kHz mono PCM16, raw bytes (will be base64-encoded on the wire).
    case audio(Data)
    /// JPEG-encoded screen frame.
    case video(Data)
    /// Free-form client-content text. Used for whispers and digest injection.
    case clientText(String)
    /// Tool result delivered to the model.
    case toolResult(callID: String, output: JSONValue)
}

public enum LiveEvent: Sendable {
    /// Gemini's PCM16 24kHz audio response, raw bytes (post-base64-decode).
    case audioOut(Data)
    /// Transcribed user speech (incremental).
    case userTranscript(String)
    /// Transcribed assistant speech (incremental).
    case assistantTranscript(String)
    /// Gemini wants to invoke a tool.
    case toolCall(callID: String, name: String, args: JSONValue)
    /// Server signaled barge-in (model interrupted).
    case interrupted
    /// Server-side session resumption handle was rotated.
    case resumptionHandle(String)
    /// `setupComplete` ack received.
    case setupComplete
    /// Connection was lost.
    case closed(reason: String?)
}

public protocol LiveSession: Sendable {
    func send(_ frame: LiveFrame) async throws
    func receive() -> AsyncStream<LiveEvent>
    func close() async
}

public struct LiveSessionConfig: Sendable {
    /// Amendment B: bare model name, NOT the deprecated preview ID.
    public var model: String = "gemini-live-2.5-flash-native-audio"
    public var voice: String = "Kore"
    public var endpoint: URL = URL(string: "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent")!
    public var apiKey: String
    public var resumptionHandle: String? = nil
    public var systemPrompt: String? = nil
    public var toolDeclarations: [JSONValue] = []
    public init(apiKey: String) { self.apiKey = apiKey }
}
