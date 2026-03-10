import SwiftUI

/// Animated pulsing dot that shows Aura's current state.
/// The hero element of the UI — it breathes to show Aura is alive.
struct DotView: View {
    let color: DotColorName
    let isPulsing: Bool

    @State private var pulseScale: CGFloat = 1.0

    var body: some View {
        Circle()
            .fill(dotColor)
            .frame(width: 10, height: 10)
            .scaleEffect(isPulsing ? pulseScale : 1.0)
            .shadow(
                color: isPulsing ? dotColor.opacity(0.6) : .clear,
                radius: isPulsing ? 6 : 0
            )
            .animation(.spring(response: 0.3, dampingFraction: 0.7), value: color)
            .onChange(of: isPulsing) { _, newValue in
                if newValue {
                    startPulsing()
                } else {
                    pulseScale = 1.0
                }
            }
            .onAppear {
                if isPulsing {
                    startPulsing()
                }
            }
    }

    private var dotColor: Color {
        switch color {
        case .gray: return Color(white: 0.55)
        case .green: return Color(red: 0.30, green: 0.88, blue: 0.52)
        case .amber: return Color(red: 1.0, green: 0.78, blue: 0.28)
        case .red: return Color(red: 0.92, green: 0.28, blue: 0.28)
        }
    }

    private func startPulsing() {
        withAnimation(
            .easeInOut(duration: 0.8)
                .repeatForever(autoreverses: true)
        ) {
            pulseScale = pulseScale == 0.85 ? 1.15 : 0.85
        }
    }
}
