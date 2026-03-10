import SwiftUI

/// A single message bubble in the conversation.
/// Three visual styles: user (right, accent-tinted), assistant (left, subtle),
/// and tool (full-width, monospaced with status icon).
struct MessageBubble: View {
    let message: ChatMessage

    private static let accentColor = Color(red: 0.30, green: 0.88, blue: 0.52)

    var body: some View {
        switch message.role {
        case .user:
            userBubble
        case .assistant:
            assistantBubble
        case .tool(let status):
            toolBubble(status: status)
        }
    }

    // MARK: - User bubble

    private var userBubble: some View {
        HStack {
            Spacer(minLength: 0)
            Text(message.text)
                .font(.system(size: 13))
                .foregroundStyle(.primary)
                .textSelection(.enabled)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(
                    Self.accentColor.opacity(0.12),
                    in: RoundedRectangle(cornerRadius: 14, style: .continuous)
                )
                .frame(maxWidth: maxBubbleWidth, alignment: .trailing)
        }
    }

    // MARK: - Assistant bubble

    private var assistantBubble: some View {
        HStack {
            Text(message.text)
                .font(.system(size: 13))
                .foregroundStyle(.primary)
                .textSelection(.enabled)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(
                    Color.secondary.opacity(0.08),
                    in: RoundedRectangle(cornerRadius: 14, style: .continuous)
                )
                .frame(maxWidth: maxBubbleWidth, alignment: .leading)
            Spacer(minLength: 0)
        }
    }

    // MARK: - Tool bubble

    private func toolBubble(status: ToolRunStatus) -> some View {
        HStack(spacing: 6) {
            Image(systemName: status.symbolName)
                .font(.caption)
                .foregroundStyle(status.color)

            Text(message.text)
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(.secondary)
                .lineLimit(5)
                .textSelection(.enabled)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .frame(maxWidth: .infinity, alignment: .center)
        .background(
            Color.secondary.opacity(0.04),
            in: RoundedRectangle(cornerRadius: 8, style: .continuous)
        )
        .transition(.opacity)
    }

    private let maxBubbleWidth: CGFloat = 300
}

// MARK: - ToolRunStatus styling

extension ToolRunStatus {
    var symbolName: String {
        switch self {
        case .running: return "bolt.fill"
        case .completed: return "checkmark.circle.fill"
        case .failed: return "xmark.circle.fill"
        }
    }

    var color: Color {
        switch self {
        case .running: return Color(red: 1.0, green: 0.78, blue: 0.28) // Amber
        case .completed: return Color(red: 0.30, green: 0.88, blue: 0.52) // Green
        case .failed: return Color(red: 0.92, green: 0.28, blue: 0.28) // Red
        }
    }
}
