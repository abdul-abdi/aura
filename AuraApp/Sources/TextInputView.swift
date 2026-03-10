import SwiftUI

/// Text input bar at the bottom of the panel.
/// TextField with send button, submits on Enter.
struct TextInputView: View {
    let onSend: (String) -> Void

    @State private var text: String = ""
    @FocusState private var isFocused: Bool

    private static let accentColor = Color(red: 0.30, green: 0.88, blue: 0.52)

    var body: some View {
        VStack(spacing: 0) {
            Divider()

            HStack(spacing: 8) {
                TextField("Type a message...", text: $text)
                    .textFieldStyle(.plain)
                    .font(.system(size: 13))
                    .padding(.horizontal, 10)
                    .padding(.vertical, 7)
                    .background(
                        RoundedRectangle(cornerRadius: 10, style: .continuous)
                            .fill(Color.primary.opacity(0.04))
                    )
                    .focused($isFocused)
                    .onSubmit {
                        sendMessage()
                    }

                Button(action: sendMessage) {
                    Image(systemName: "arrow.up.circle.fill")
                        .font(.system(size: 24))
                        .foregroundStyle(
                            text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                                ? Color.secondary.opacity(0.3)
                                : Self.accentColor
                        )
                }
                .buttonStyle(.plain)
                .disabled(text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
        }
        // Grab focus when the view first appears (e.g. first launch)
        .onAppear {
            isFocused = true
        }
        // Re-focus every time the panel finishes its show animation
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
