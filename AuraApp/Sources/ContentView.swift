import SwiftUI

/// Root view composing the floating panel's content.
/// Shows the permissions onboarding on first launch; afterwards the normal
/// conversation UI (StatusHeader + ConversationView + TextInputView).
struct ContentView: View {
    @Bindable var appState: AppState

    var body: some View {
        Group {
            switch appState.onboardingStep {
            case .welcome:
                WelcomeView(onContinue: { appState.completeWelcome() })
                    .transition(.asymmetric(
                        insertion: .move(edge: .bottom).combined(with: .opacity),
                        removal: .opacity
                    ))
            case .permissions:
                PermissionsView(
                    checker: appState.permissionChecker,
                    onContinue: { appState.completeOnboarding() }
                )
                .transition(.asymmetric(
                    insertion: .move(edge: .bottom).combined(with: .opacity),
                    removal: .opacity
                ))
            case .done:
                mainContent
                    .transition(.asymmetric(
                        insertion: .move(edge: .bottom).combined(with: .opacity),
                        removal: .opacity
                    ))
            }
        }
        .animation(.spring(response: 0.4, dampingFraction: 0.85), value: appState.onboardingStep)
        .frame(width: 380, height: 520)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
    }

    private var mainContent: some View {
        VStack(spacing: 0) {
            StatusHeader(
                dotColor: appState.dotColor,
                isPulsing: appState.isPulsing,
                statusMessage: appState.statusMessage
            )

            Divider()

            ConversationView(
                events: appState.events,
                connectionState: appState.connectionState,
                isThinking: appState.isThinking,
                onReconnect: { appState.requestReconnect() }
            )
            .frame(maxHeight: .infinity)

            TextInputView { text in
                appState.sendText(text)
            }
        }
    }
}
