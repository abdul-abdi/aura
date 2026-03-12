import SwiftUI

/// Scrollable activity stream displaying all events.
/// Auto-scrolls to the bottom when new events arrive.
struct ConversationView: View {
    let events: [ActivityEvent]
    let connectionState: AppState.ConnectionState
    let isThinking: Bool
    let onReconnect: () -> Void

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView(.vertical, showsIndicators: true) {
                VStack(spacing: 0) {
                    if connectionState == .disconnected {
                        reconnectBanner
                    }

                    if events.isEmpty && !isThinking {
                        emptyState
                    } else {
                        LazyVStack(spacing: 4) {
                            ForEach(events) { event in
                                ActivityRow(event: event)
                                    .id(event.id)
                                    .transition(
                                        .asymmetric(
                                            insertion: .move(edge: .bottom)
                                                .combined(with: .opacity),
                                            removal: .opacity
                                        )
                                    )
                            }

                            if isThinking {
                                TypingIndicator()
                                    .id("typing-indicator")
                                    .transition(.opacity.combined(with: .move(edge: .bottom)))
                            }
                        }
                        .padding(.horizontal, 12)
                        .padding(.vertical, 8)
                    }
                }
            }
            .onChange(of: events.count) { _, _ in
                if let lastEvent = events.last {
                    withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) {
                        proxy.scrollTo(lastEvent.id, anchor: .bottom)
                    }
                }
            }
            .onChange(of: isThinking) { _, thinking in
                if thinking {
                    withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) {
                        proxy.scrollTo("typing-indicator", anchor: .bottom)
                    }
                }
            }
        }
    }

    private var reconnectBanner: some View {
        Button(action: onReconnect) {
            HStack(spacing: 8) {
                Image(systemName: "arrow.trianglehead.2.counterclockwise")
                    .font(.system(size: 12, weight: .medium))
                Text("Connection lost. Reconnect")
                    .font(.system(size: 12, weight: .medium))
            }
            .foregroundStyle(.white)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 10)
            .background(
                Color(red: 1.0, green: 0.78, blue: 0.28).opacity(0.9),
                in: RoundedRectangle(cornerRadius: 8, style: .continuous)
            )
        }
        .buttonStyle(.plain)
        .padding(.horizontal, 12)
        .padding(.top, 8)
    }

    private var emptyState: some View {
        VStack(spacing: 8) {
            Spacer()

            switch connectionState {
            case .disconnected:
                Image(systemName: "wifi.slash")
                    .font(.system(size: 32, weight: .ultraLight))
                    .foregroundStyle(.tertiary)
                Text("Not connected")
                    .font(.caption)
                    .foregroundStyle(.tertiary)

            case .connecting:
                ProgressView()
                    .scaleEffect(0.8)
                    .tint(Color(red: 0.30, green: 0.88, blue: 0.52))
                Text("Connecting...")
                    .font(.caption)
                    .foregroundStyle(.tertiary)

            case .connected:
                Image(systemName: "waveform")
                    .font(.system(size: 32, weight: .ultraLight))
                    .foregroundStyle(.tertiary)
                Text("Say something or type a message")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }

            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(.top, 60)
    }
}

// MARK: - Typing Indicator

private struct TypingIndicator: View {
    @State private var dotOffsets: [Bool] = [false, false, false]

    var body: some View {
        HStack {
            HStack(spacing: 4) {
                ForEach(0..<3, id: \.self) { index in
                    Circle()
                        .fill(Color.secondary.opacity(0.4))
                        .frame(width: 6, height: 6)
                        .offset(y: dotOffsets[index] ? -4 : 0)
                }
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 12)
            .background(
                Color.secondary.opacity(0.08),
                in: RoundedRectangle(cornerRadius: 14, style: .continuous)
            )
            Spacer(minLength: 0)
        }
        .onAppear { startAnimation() }
    }

    private func startAnimation() {
        for i in 0..<3 {
            withAnimation(
                .easeInOut(duration: 0.4)
                .repeatForever(autoreverses: true)
                .delay(Double(i) * 0.15)
            ) {
                dotOffsets[i] = true
            }
        }
    }
}
