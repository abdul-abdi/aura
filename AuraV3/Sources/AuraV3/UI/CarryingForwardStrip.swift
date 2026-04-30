import SwiftUI

public struct CarryingForwardStrip: View {
    public init() {}
    public var body: some View {
        HStack {
            Text("Carrying forward: —")
                .foregroundStyle(.secondary)
                .font(.caption)
            Spacer()
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 4)
    }
}
