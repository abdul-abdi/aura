import SwiftUI

private let auraGreen = Color(red: 0.30, green: 0.88, blue: 0.52)
private let auraAmber = Color(red: 1.0, green: 0.78, blue: 0.28)

// MARK: - PermissionsView

struct PermissionsView: View {
    @Bindable var checker: PermissionChecker  // uses @Observable projected via @Bindable
    let onContinue: () -> Void

    // Refresh timer — fires every 2 s so the view updates when the user
    // returns from System Settings without needing a manual action.
    private let timer = Timer.publish(every: 2, on: .main, in: .common).autoconnect()

    var body: some View {
        VStack(spacing: 0) {
            header
            permissionsList
            actions
        }
        .frame(width: 380)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        .onAppear {
            checker.checkAll()
            if checker.allGranted { onContinue() }
        }
        .onReceive(timer) { _ in
            checker.checkAll()
            if checker.allGranted { onContinue() }
        }
    }

    // MARK: Header

    private var header: some View {
        VStack(spacing: 12) {
            // Aura branding dot
            ZStack {
                Circle()
                    .fill(auraGreen.opacity(0.18))
                    .frame(width: 52, height: 52)

                Circle()
                    .fill(auraGreen)
                    .frame(width: 28, height: 28)
            }
            .padding(.top, 28)

            Text("Welcome to Aura")
                .font(.title2)
                .fontWeight(.semibold)
                .foregroundStyle(.primary)

            Text("Aura needs a few permissions to work.\nTap **Grant** to open System Settings,\ntoggle Aura on, then come back here.")
                .font(.callout)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, 24)
        }
        .padding(.bottom, 20)
    }

    // MARK: Permissions list

    private var permissionsList: some View {
        VStack(spacing: 10) {
            PermissionRow(
                icon: "mic.fill",
                title: "Microphone",
                reason: "To hear your voice and respond naturally.",
                granted: checker.micGranted,
                onGrant: { checker.requestMicAccess() }
            )

            PermissionRow(
                icon: "rectangle.dashed.badge.record",
                title: "Screen Recording",
                reason: "To understand what's on your screen when asked.",
                granted: checker.screenGranted,
                onGrant: { checker.requestScreenAccess() }
            )

            PermissionRow(
                icon: "hand.point.up.left.fill",
                title: "Accessibility",
                reason: "To type, click, and control apps on your behalf.",
                granted: checker.accessibilityGranted,
                onGrant: { checker.requestAccessibilityAccess() }
            )
        }
        .padding(.horizontal, 20)
    }

    // MARK: Actions

    private var actions: some View {
        VStack(spacing: 6) {
            Button(action: onContinue) {
                Text("Continue")
                    .font(.body)
                    .fontWeight(.semibold)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 10)
            }
            .buttonStyle(AuraButtonStyle(enabled: checker.allGranted))
            .disabled(!checker.allGranted)
            .animation(.spring(response: 0.3, dampingFraction: 0.7), value: checker.allGranted)

            Button("Skip for now") {
                onContinue()
            }
            .font(.footnote)
            .foregroundStyle(.tertiary)
            .buttonStyle(.plain)
            .padding(.bottom, 4)
        }
        .padding(.horizontal, 20)
        .padding(.top, 18)
        .padding(.bottom, 22)
    }
}

// MARK: - PermissionRow

private struct PermissionRow: View {
    let icon: String
    let title: String
    let reason: String
    let granted: Bool
    let onGrant: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            // State indicator
            Image(systemName: granted ? "checkmark.circle.fill" : "circle")
                .font(.system(size: 22, weight: .medium))
                .foregroundStyle(granted ? auraGreen : auraAmber)
                .animation(.spring(response: 0.3, dampingFraction: 0.7), value: granted)
                .frame(width: 26)

            // Icon + text
            Label {
                VStack(alignment: .leading, spacing: 2) {
                    Text(title)
                        .font(.callout)
                        .fontWeight(.medium)
                        .foregroundStyle(.primary)

                    Text(reason)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            } icon: {
                Image(systemName: icon)
                    .font(.system(size: 14))
                    .foregroundStyle(.secondary)
                    .frame(width: 18)
            }

            Spacer()

            // Grant button — hidden when already granted
            if !granted {
                Button("Grant") { onGrant() }
                    .buttonStyle(GrantButtonStyle())
                    .transition(.opacity.combined(with: .scale))
            } else {
                // Placeholder to prevent layout jump
                Color.clear.frame(width: 52, height: 26)
            }
        }
        .padding(12)
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(Color.white.opacity(0.06))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(
                    granted
                        ? auraGreen.opacity(0.4)
                        : Color.white.opacity(0.15),
                    lineWidth: 1
                )
        )
        .animation(.spring(response: 0.3, dampingFraction: 0.7), value: granted)
    }
}

// MARK: - Button Styles

private struct AuraButtonStyle: ButtonStyle {
    let enabled: Bool

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .background(
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .fill(enabled ? auraGreen : Color.white.opacity(0.08))
            )
            .overlay(
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .strokeBorder(
                        enabled
                            ? auraGreen.opacity(0.6)
                            : Color.white.opacity(0.12),
                        lineWidth: 1
                    )
            )
            .foregroundStyle(enabled ? Color.white : Color.primary.opacity(0.35))
            .scaleEffect(configuration.isPressed ? 0.97 : 1.0)
            .opacity(configuration.isPressed ? 0.85 : 1.0)
            .animation(.spring(response: 0.2, dampingFraction: 0.7), value: configuration.isPressed)
    }
}

private struct GrantButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.caption)
            .fontWeight(.semibold)
            .padding(.horizontal, 12)
            .padding(.vertical, 5)
            .background(
                RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .fill(auraAmber.opacity(0.18))
            )
            .overlay(
                RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .strokeBorder(auraAmber.opacity(0.35), lineWidth: 1)
            )
            .foregroundStyle(auraAmber)
            .scaleEffect(configuration.isPressed ? 0.95 : 1.0)
            .opacity(configuration.isPressed ? 0.85 : 1.0)
            .animation(.spring(response: 0.2, dampingFraction: 0.7), value: configuration.isPressed)
    }
}
