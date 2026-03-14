import Foundation

// MARK: - Daemon -> UI Events

/// Events sent from the Rust daemon to the SwiftUI frontend over the Unix socket.
/// Each event is a single JSON line (JSONL).
enum DaemonEvent: Decodable {
    case dotColor(DotColorUpdate)
    case transcript(TranscriptUpdate)
    case toolStatus(ToolStatusUpdate)
    case status(StatusUpdate)
    case recentSessions(RecentSessionsUpdate)
    case shutdown

    private enum CodingKeys: String, CodingKey {
        case type
    }

    private enum EventType: String, Decodable {
        case dotColor = "dot_color"
        case transcript
        case toolStatus = "tool_status"
        case status
        case recentSessions = "recent_sessions"
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
        case .recentSessions:
            self = .recentSessions(try RecentSessionsUpdate(from: decoder))
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

    enum TranscriptSource: Decodable, Equatable {
        case voice
        case text
        case unknown(String)

        init(from decoder: Decoder) throws {
            let container = try decoder.singleValueContainer()
            let rawValue = try container.decode(String.self)
            switch rawValue {
            case "voice": self = .voice
            case "text": self = .text
            default: self = .unknown(rawValue)
            }
        }
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

struct RecentSessionsUpdate: Decodable {
    let sessions: [RecentSession]
}

struct RecentSession: Decodable, Identifiable {
    let sessionId: String
    let summary: String
    let createdAt: String

    var id: String { sessionId }

    private enum CodingKeys: String, CodingKey {
        case sessionId = "session_id"
        case summary
        case createdAt = "created_at"
    }

    /// Format the timestamp for display (e.g., "Today, 2:30 PM" or "Yesterday").
    var displayTime: String {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        guard let date = formatter.date(from: createdAt) ?? ISO8601DateFormatter().date(from: createdAt) else {
            return createdAt
        }

        let calendar = Calendar.current

        if calendar.isDateInToday(date) {
            let timeFormatter = DateFormatter()
            timeFormatter.dateFormat = "h:mm a"
            return "Today, \(timeFormatter.string(from: date))"
        } else if calendar.isDateInYesterday(date) {
            let timeFormatter = DateFormatter()
            timeFormatter.dateFormat = "h:mm a"
            return "Yesterday, \(timeFormatter.string(from: date))"
        } else {
            let dateFormatter = DateFormatter()
            dateFormatter.dateFormat = "MMM d, h:mm a"
            return dateFormatter.string(from: date)
        }
    }
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

// MARK: - Activity Stream Model

/// A single event in the activity stream.
struct ActivityEvent: Identifiable {
    let id: UUID
    let kind: EventKind
    var text: String
    let timestamp: Date

    init(kind: EventKind, text: String) {
        self.id = UUID()
        self.kind = kind
        self.text = text
        self.timestamp = Date()
    }
}

enum EventKind: Equatable {
    case userSpeech          // 🎤 voice transcript
    case userText            // 💬 typed message
    case assistantSpeech     // 🔊 what Aura said
    case toolCall(ToolRunStatus)  // ⚡ tool execution
    case turnSeparator       // ─ ─ visual break
}
