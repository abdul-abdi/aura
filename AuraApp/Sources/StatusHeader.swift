import SwiftUI

/// Premium app bar header showing animated status dot, app name, status message, and hotkey hint.
struct StatusHeader: View {
    let dotColor: DotColorName
    let isPulsing: Bool
    let statusMessage: String

    var body: some View {
        HStack(spacing: 10) {
            // Animated dot
            DotView(color: dotColor, isPulsing: isPulsing)
                .frame(width: 10, height: 10)

            // App name + status
            VStack(alignment: .leading, spacing: 1) {
                Text("Aura")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(.primary)

                Text(statusMessage)
                    .font(.system(size: 10))
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
            }

            Spacer()

            // Subtle hotkey hint
            Text("⌘⇧A")
                .font(.system(size: 9, weight: .medium, design: .rounded))
                .foregroundStyle(.quaternary)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
    }
}
