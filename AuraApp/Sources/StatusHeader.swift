import SwiftUI

/// Header bar showing the animated dot and current status message.
struct StatusHeader: View {
    let dotColor: DotColorName
    let isPulsing: Bool
    let statusMessage: String

    var body: some View {
        HStack(spacing: 8) {
            DotView(color: dotColor, isPulsing: isPulsing)

            Text(statusMessage)
                .font(.caption)
                .fontWeight(.medium)
                .foregroundStyle(.secondary)
                .lineLimit(1)

            Spacer()
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }
}
