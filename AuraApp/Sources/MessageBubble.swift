import SwiftUI

/// Chat bubble view for messages in the conversation.
struct ActivityRow: View {
    let event: ActivityEvent
    @State private var isExpanded = false

    private static let userBubbleColor = Color(red: 0.30, green: 0.88, blue: 0.52)
    private static let assistantBubbleColor = Color.white.opacity(0.1)

    var body: some View {
        switch event.kind {
        case .userSpeech, .userText:
            userBubble
        case .assistantSpeech:
            assistantBubble
        case .toolCall(let status):
            toolPill(status: status)
        case .turnSeparator:
            EmptyView()
        }
    }

    // MARK: - User Bubble (right-aligned, green)

    private var userBubble: some View {
        HStack {
            Spacer(minLength: 60)
            Text(event.text)
                .font(.system(size: 13))
                .foregroundStyle(.white)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(
                    Self.userBubbleColor,
                    in: ChatBubbleShape(isUser: true)
                )
                .textSelection(.enabled)
        }
        .padding(.vertical, 2)
    }

    // MARK: - Assistant Bubble (left-aligned, translucent)

    private var assistantBubble: some View {
        HStack {
            Text(event.text)
                .font(.system(size: 13))
                .foregroundStyle(.primary)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(
                    Self.assistantBubbleColor,
                    in: ChatBubbleShape(isUser: false)
                )
                .textSelection(.enabled)
            Spacer(minLength: 60)
        }
        .padding(.vertical, 2)
    }

    // MARK: - Tool Pill (centered, compact)

    private func toolPill(status: ToolRunStatus) -> some View {
        VStack(spacing: 4) {
            Button {
                withAnimation(.spring(response: 0.25, dampingFraction: 0.8)) {
                    isExpanded.toggle()
                }
            } label: {
                HStack(spacing: 4) {
                    Image(systemName: status.pillSymbol)
                        .font(.system(size: 9, weight: .semibold))
                        .foregroundStyle(status.color)
                        .opacity(status == .running ? 0.8 : 1.0)

                    Text(event.text.components(separatedBy: "\n").first ?? event.text)
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 5)
                .background(
                    Color.secondary.opacity(0.06),
                    in: Capsule()
                )
            }
            .buttonStyle(.plain)
            .opacity(status == .running ? 1.0 : 0.7)

            // Expanded result
            if isExpanded, status != .running {
                let lines = event.text.components(separatedBy: "\n").dropFirst()
                if let result = lines.first, !result.isEmpty {
                    Text(result)
                        .font(.system(size: 11))
                        .foregroundStyle(.tertiary)
                        .padding(.horizontal, 12)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .transition(.opacity.combined(with: .move(edge: .top)))
                }
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 1)
    }
}

// MARK: - Chat Bubble Shape

/// Custom shape with asymmetric corner radii for chat bubble tail effect.
struct ChatBubbleShape: Shape {
    let isUser: Bool

    func path(in rect: CGRect) -> Path {
        let largeRadius: CGFloat = 16
        let smallRadius: CGFloat = 4

        let topLeft = largeRadius
        let topRight = largeRadius
        let bottomLeft: CGFloat = isUser ? largeRadius : smallRadius
        let bottomRight: CGFloat = isUser ? smallRadius : largeRadius

        var path = Path()
        path.move(to: CGPoint(x: rect.minX + topLeft, y: rect.minY))
        path.addLine(to: CGPoint(x: rect.maxX - topRight, y: rect.minY))
        path.addArc(tangent1End: CGPoint(x: rect.maxX, y: rect.minY),
                    tangent2End: CGPoint(x: rect.maxX, y: rect.minY + topRight),
                    radius: topRight)
        path.addLine(to: CGPoint(x: rect.maxX, y: rect.maxY - bottomRight))
        path.addArc(tangent1End: CGPoint(x: rect.maxX, y: rect.maxY),
                    tangent2End: CGPoint(x: rect.maxX - bottomRight, y: rect.maxY),
                    radius: bottomRight)
        path.addLine(to: CGPoint(x: rect.minX + bottomLeft, y: rect.maxY))
        path.addArc(tangent1End: CGPoint(x: rect.minX, y: rect.maxY),
                    tangent2End: CGPoint(x: rect.minX, y: rect.maxY - bottomLeft),
                    radius: bottomLeft)
        path.addLine(to: CGPoint(x: rect.minX, y: rect.minY + topLeft))
        path.addArc(tangent1End: CGPoint(x: rect.minX, y: rect.minY),
                    tangent2End: CGPoint(x: rect.minX + topLeft, y: rect.minY),
                    radius: topLeft)
        return path
    }
}

// MARK: - ToolRunStatus pill styling

extension ToolRunStatus {
    var pillSymbol: String {
        switch self {
        case .running: return "bolt.fill"
        case .completed: return "checkmark"
        case .failed: return "xmark"
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
