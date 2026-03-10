import SwiftUI

/// Text input bar at the bottom of the panel.
/// Supports multiline input — Enter sends, Shift+Enter inserts newline.
struct TextInputView: View {
    let onSend: (String) -> Void

    @State private var text: String = ""
    @FocusState private var isFocused: Bool

    private static let accentColor = Color(red: 0.30, green: 0.88, blue: 0.52)

    private var isEmpty: Bool {
        text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var body: some View {
        VStack(spacing: 0) {
            Divider().opacity(0.5)

            HStack(alignment: .bottom, spacing: 8) {
                TextEditor(text: $text)
                    .font(.system(size: 13))
                    .scrollContentBackground(.hidden)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 4)
                    .frame(minHeight: 32, maxHeight: 80)
                    .fixedSize(horizontal: false, vertical: true)
                    .background(
                        RoundedRectangle(cornerRadius: 10, style: .continuous)
                            .fill(.ultraThinMaterial)
                            .shadow(color: .black.opacity(0.06), radius: 1, x: 0, y: 1)
                    )
                    .overlay(
                        RoundedRectangle(cornerRadius: 10, style: .continuous)
                            .strokeBorder(Color.white.opacity(0.08), lineWidth: 0.5)
                    )
                    .focused($isFocused)
                    .onKeyPress(.return, phases: .down) { keyPress in
                        if keyPress.modifiers.contains(.shift) {
                            return .ignored
                        }
                        sendMessage()
                        return .handled
                    }
                    .overlay(alignment: .topLeading) {
                        if text.isEmpty {
                            Text("Type a message...")
                                .font(.system(size: 13))
                                .foregroundStyle(.tertiary)
                                .padding(.horizontal, 10)
                                .padding(.vertical, 8)
                                .allowsHitTesting(false)
                        }
                    }

                Button(action: sendMessage) {
                    Image(systemName: "arrow.up.circle.fill")
                        .font(.system(size: 24))
                        .foregroundStyle(
                            isEmpty
                                ? Color.secondary.opacity(0.3)
                                : Self.accentColor
                        )
                }
                .buttonStyle(.plain)
                .disabled(isEmpty)
                .padding(.bottom, 4)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
        }
        .onAppear {
            isFocused = true
        }
        .onReceive(
            NotificationCenter.default.publisher(for: NSNotification.Name("AuraPanelDidShow"))
        ) { _ in
            isFocused = true
        }
    }

    private func sendMessage() {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        onSend(trimmed)
        text = ""
    }
}
