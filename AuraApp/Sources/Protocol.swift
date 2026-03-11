import Foundation

// MARK: - Daemon -> UI Events

/// Events sent from the Rust daemon to the SwiftUI frontend over the Unix socket.
/// Each event is a single JSON line (JSONL).
enum DaemonEvent: Decodable {
    case dotColor(DotColorUpdate)
    case transcript(TranscriptUpdate)
    case toolStatus(ToolStatusUpdate)
    case status(StatusUpdate)
    case shutdown

    private enum CodingKeys: String, CodingKey {
        case type
    }

    private enum EventType: String, Decodable {
        case dotColor = "dot_color"
        case transcript
        case toolStatus = "tool_status"
        case status
        case shutdown
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let type = try container.decode(EventType.self, forKey: .type)

        switch type {
        case .dotColor:
            self = .dotColor(try DotColorUpdate(from: decoder))
        case .transcript:
            self = .transcript(try TranscriptUpdate(from: decoder))
        case .toolStatus:
            self = .toolStatus(try ToolStatusUpdate(from: decoder))
        case .status:
            self = .status(try StatusUpdate(from: decoder))
        case .shutdown:
            self = .shutdown
        }
    }
}

struct DotColorUpdate: Decodable {
    let color: DotColorName
    let pulsing: Bool
}

enum DotColorName: String, Decodable {
    case gray
    case green
    case amber
    case red
}

struct TranscriptUpdate: Decodable {
    let role: TranscriptRole
    let text: String
    let done: Bool
    let source: TranscriptSource

    enum TranscriptSource: String, Decodable {
        case voice
        case text
    }
}

enum TranscriptRole: String, Decodable {
    case user
    case assistant
}

struct ToolStatusUpdate: Decodable {
    let name: String
    let status: ToolRunStatus
    let output: String?
    let summary: String?
}

enum ToolRunStatus: String, Decodable {
    case running
    case completed
    case failed
}

struct StatusUpdate: Decodable {
    let message: String
}

// MARK: - UI -> Daemon Commands

/// Commands sent from the SwiftUI frontend to the Rust daemon over the Unix socket.
/// Each command is a single JSON line (JSONL).
enum UICommand: Encodable {
    case sendText(String)
    case toggleMic
    case reconnect
    case shutdown

    private enum CodingKeys: String, CodingKey {
        case type
        case text
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .sendText(let text):
            try container.encode("send_text", forKey: .type)
            try container.encode(text, forKey: .text)
        case .toggleMic:
            try container.encode("toggle_mic", forKey: .type)
        case .reconnect:
            try container.encode("reconnect", forKey: .type)
        case .shutdown:
            try container.encode("shutdown", forKey: .type)
        }
    }
}

// MARK: - Message Model

/// A single message displayed in the conversation view.
struct ChatMessage: Identifiable {
    let id: UUID
    let role: MessageRole
    var text: String
    let timestamp: Date

    init(role: MessageRole, text: String) {
        self.id = UUID()
        self.role = role
        self.text = text
        self.timestamp = Date()
    }
}

enum MessageRole {
    case user
    case assistant
    case tool(ToolRunStatus)
}
