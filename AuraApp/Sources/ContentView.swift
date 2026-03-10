import SwiftUI

/// Root view composing the floating panel's content.
/// Shows the permissions onboarding on first launch; afterwards the normal
/// conversation UI (StatusHeader + ConversationView + TextInputView).
struct ContentView: View {
    @Bindable var appState: AppState

    var body: some View {
        Group {
            if appState.showOnboarding {
                PermissionsView(
                    checker: appState.permissionChecker,
                    onContinue: { appState.completeOnboarding() }
                )
            } else {
                mainContent
            }
        }
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

            ConversationView(messages: appState.messages)
                .frame(maxHeight: .infinity)

            TextInputView { text in
                appState.sendText(text)
            }
        }
    }
}
