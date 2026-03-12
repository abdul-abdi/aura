import SwiftUI

/// A single row in the activity stream.
/// Renders differently based on event kind: speech (icon + quoted text),
/// tool calls (status icon + monospace name + result), or turn separator.
struct ActivityRow: View {
    let event: ActivityEvent
    @State private var isExpanded = false

    var body: some View {
        switch event.kind {
        case .userSpeech:
            iconRow(symbol: "mic.fill", color: .primary, quoted: true)

        case .userText:
            iconRow(symbol: "text.bubble.fill", color: .primary, quoted: true)

        case .assistantSpeech:
            iconRow(symbol: "speaker.wave.2.fill", color: .primary, quoted: true)

        case .toolCall(let status):
            toolRow(status: status)

        case .turnSeparator:
            separatorRow
        }
    }

    private func iconRow(symbol: String, color: Color, quoted: Bool) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: symbol)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .frame(width: 16)
            Text(quoted ? "\"\(event.text)\"" : event.text)
                .font(.system(size: 13))
                .foregroundStyle(color)
                .textSelection(.enabled)
        }
        .padding(.vertical, 2)
    }

    private func toolRow(status: ToolRunStatus) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 6) {
                Image(systemName: status.symbolName)
                    .font(.system(size: 11))
                    .foregroundStyle(status.color)
                    .frame(width: 16)
                Text(event.text.components(separatedBy: "\n").first ?? event.text)
                    .font(.system(size: 12, design: .monospaced))
                    .foregroundStyle(status == .running ? Color.primary : .secondary)
            }

            // Result (second line onwards) — indented, dimmed
            let lines = event.text.components(separatedBy: "\n").dropFirst()
            if status != .running, let result = lines.first, !result.isEmpty {
                HStack(spacing: 6) {
                    Color.clear.frame(width: 16)
                    Text(result)
                        .font(.system(size: 11))
                        .foregroundStyle(.tertiary)
                        .lineLimit(isExpanded ? nil : 1)
                        .textSelection(.enabled)
                }
                .onTapGesture { isExpanded.toggle() }
            }
        }
        .padding(.vertical, 1)
    }

    private var separatorRow: some View {
        HStack {
            Rectangle()
                .fill(Color.secondary.opacity(0.15))
                .frame(height: 0.5)
        }
        .padding(.vertical, 6)
    }
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
        case .running: return Color(red: 1.0, green: 0.78, blue: 0.28)
        case .completed: return Color(red: 0.30, green: 0.88, blue: 0.52)
        case .failed: return Color(red: 0.92, green: 0.28, blue: 0.28)
        }
    }
}
