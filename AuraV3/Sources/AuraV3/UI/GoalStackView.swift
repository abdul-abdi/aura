import SwiftUI

public struct GoalStackView: View {
    public init() {}
    public var body: some View {
        HStack {
            Text("No active goal")
                .foregroundStyle(.secondary)
                .font(.callout)
                .italic()
            Spacer()
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        .background(Color.gray.opacity(0.05))
    }
}
