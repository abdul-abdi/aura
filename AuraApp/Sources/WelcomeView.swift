import SwiftUI

private let auraGreen = Color(red: 0.30, green: 0.88, blue: 0.52)

// MARK: - WelcomeView

struct WelcomeView: View {
    let onContinue: () -> Void

    @State private var apiKey = ""
    @State private var showKey = false
    @State private var appeared = false
    @State private var orbPulse = false
    @State private var saveError: String?

    private var isKeyValid: Bool {
        apiKey.count >= 20
            && apiKey.allSatisfy { $0.isASCII && !$0.isWhitespace }
    }

    var body: some View {
        VStack(spacing: 0) {
            brandingSection
            Spacer(minLength: 0)
            inputSection
            Spacer(minLength: 0)
            actionsSection
        }
        .padding(.horizontal, 28)
        .padding(.top, 36)
        .padding(.bottom, 24)
        .onAppear {
            if let existing = loadExistingKey() {
                apiKey = existing
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) {
                withAnimation(.spring(response: 0.6, dampingFraction: 0.8)) {
                    appeared = true
                }
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) {
                withAnimation(.easeInOut(duration: 2).repeatForever(autoreverses: true)) {
                    orbPulse = true
                }
            }
        }
    }

    // MARK: - Branding

    private var brandingSection: some View {
        VStack(spacing: 14) {
            ZStack {
                Circle()
                    .fill(auraGreen.opacity(0.08))
                    .frame(width: 110, height: 110)
                    .scaleEffect(orbPulse ? 1.08 : 0.94)

                Circle()
                    .fill(auraGreen.opacity(0.14))
                    .frame(width: 82, height: 82)
                    .scaleEffect(orbPulse ? 1.05 : 0.97)

                Circle()
                    .fill(auraGreen.opacity(0.28))
                    .frame(width: 60, height: 60)
                    .scaleEffect(orbPulse ? 1.03 : 0.98)

                Circle()
                    .fill(
                        RadialGradient(
                            gradient: Gradient(colors: [
                                auraGreen,
                                auraGreen.opacity(0.75),
                            ]),
                            center: .center,
                            startRadius: 4,
                            endRadius: 22
                        )
                    )
                    .frame(width: 44, height: 44)
                    .scaleEffect(orbPulse ? 1.05 : 0.95)
                    .shadow(color: auraGreen.opacity(0.6), radius: 14, x: 0, y: 4)
            }
            .scaleEffect(appeared ? 1.0 : 0.5)
            .opacity(appeared ? 1.0 : 0.0)

            Text("Aura")
                .font(.title)
                .fontWeight(.semibold)
                .foregroundStyle(.primary)
                .opacity(appeared ? 1.0 : 0.0)
                .offset(y: appeared ? 0 : 8)
                .animation(
                    .spring(response: 0.5, dampingFraction: 0.8)
                        .delay(0.15),
                    value: appeared
                )

            Text("Your AI assistant for macOS")
                .font(.callout)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .opacity(appeared ? 1.0 : 0.0)
                .offset(y: appeared ? 0 : 6)
                .animation(
                    .spring(response: 0.5, dampingFraction: 0.8)
                        .delay(0.25),
                    value: appeared
                )
        }
    }

    // MARK: - Input

    private var inputSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 0) {
                Group {
                    if showKey {
                        TextField("Paste your Gemini API key", text: $apiKey)
                    } else {
                        SecureField("Paste your Gemini API key", text: $apiKey)
                    }
                }
                .font(.system(size: 13))
                .textFieldStyle(.plain)
                .frame(maxWidth: .infinity)
                .autocorrectionDisabled()

                if isKeyValid {
                    Image(systemName: "checkmark.circle.fill")
                        .font(.system(size: 14, weight: .medium))
                        .foregroundStyle(auraGreen)
                        .transition(.scale.combined(with: .opacity))
                        .padding(.trailing, 6)
                }

                Button {
                    showKey.toggle()
                } label: {
                    Image(systemName: showKey ? "eye.slash" : "eye")
                        .font(.system(size: 13))
                        .foregroundStyle(.tertiary)
                }
                .buttonStyle(.plain)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
            .background(
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .fill(.ultraThinMaterial)
                    .shadow(color: .black.opacity(0.06), radius: 1, x: 0, y: 1)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .strokeBorder(
                        isKeyValid
                            ? auraGreen.opacity(0.5)
                            : Color.white.opacity(0.08),
                        lineWidth: 0.5
                    )
            )
            .animation(.spring(response: 0.3, dampingFraction: 0.7), value: isKeyValid)

            HStack(spacing: 3) {
                Text("Don't have a key?")
                    .font(.caption)
                    .foregroundStyle(.tertiary)

                Link("Get one free at Google AI Studio",
                     destination: URL(string: "https://aistudio.google.com/apikey")!)
                    .font(.caption)
                    .foregroundStyle(auraGreen.opacity(0.9))
            }
            .padding(.leading, 2)
        }
        .opacity(appeared ? 1.0 : 0.0)
        .offset(y: appeared ? 0 : 14)
        .animation(
            .spring(response: 0.5, dampingFraction: 0.8)
                .delay(0.35),
            value: appeared
        )
    }

    // MARK: - Actions

    private var actionsSection: some View {
        VStack(spacing: 6) {
            Button(action: saveAPIKey) {
                Text("Get Started")
                    .font(.body)
                    .fontWeight(.semibold)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 10)
            }
            .buttonStyle(GlassButtonStyle(enabled: isKeyValid))
            .disabled(!isKeyValid)
            .animation(.spring(response: 0.3, dampingFraction: 0.7), value: isKeyValid)

            if let error = saveError {
                Text(error)
                    .font(.caption)
                    .foregroundStyle(.red)
                    .multilineTextAlignment(.center)
                    .padding(.top, 4)
            }
        }
        .opacity(appeared ? 1.0 : 0.0)
        .offset(y: appeared ? 0 : 10)
        .animation(
            .spring(response: 0.5, dampingFraction: 0.8)
                .delay(0.45),
            value: appeared
        )
    }

    // MARK: - Config I/O

    private func loadExistingKey() -> String? {
        let configFile = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".config/aura/config.toml")
        guard let contents = try? String(contentsOf: configFile, encoding: .utf8) else {
            return nil
        }
        for line in contents.components(separatedBy: "\n") {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("api_key") {
                let parts = trimmed.components(separatedBy: "=")
                guard parts.count >= 2 else { continue }
                let value = parts.dropFirst().joined(separator: "=")
                    .trimmingCharacters(in: .whitespaces)
                    .trimmingCharacters(in: CharacterSet(charactersIn: "\""))
                if !value.isEmpty { return value }
            }
        }
        return nil
    }

    private func saveAPIKey() {
        saveError = nil
        let fm = FileManager.default
        let configDir = fm.homeDirectoryForCurrentUser
            .appendingPathComponent(".config/aura")

        try? fm.createDirectory(
            at: configDir,
            withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700]
        )
        try? fm.setAttributes(
            [.posixPermissions: 0o700],
            ofItemAtPath: configDir.path
        )

        let configFile = configDir.appendingPathComponent("config.toml")
        let content = "api_key = \"\(apiKey)\"\n"
        do {
            try content.write(to: configFile, atomically: true, encoding: .utf8)
            try? fm.setAttributes(
                [.posixPermissions: 0o600],
                ofItemAtPath: configFile.path
            )
        } catch {
            saveError = "Failed to save API key: \(error.localizedDescription)"
            return
        }

        onContinue()
    }
}

// MARK: - Glass Button Style

/// Button that blends with vibrancy materials — uses a tinted glass fill when enabled,
/// subtle material fill when disabled. Matches macOS popover control aesthetics.
private struct GlassButtonStyle: ButtonStyle {
    let enabled: Bool

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .background(
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .fill(
                        enabled
                            ? AnyShapeStyle(auraGreen.opacity(0.85))
                            : AnyShapeStyle(.ultraThinMaterial)
                    )
            )
            .overlay(
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .strokeBorder(
                        enabled
                            ? auraGreen.opacity(0.4)
                            : Color.white.opacity(0.06),
                        lineWidth: 0.5
                    )
            )
            .foregroundStyle(enabled ? Color.white : Color.primary.opacity(0.3))
            .scaleEffect(configuration.isPressed ? 0.97 : 1.0)
            .opacity(configuration.isPressed ? 0.85 : 1.0)
            .animation(.spring(response: 0.2, dampingFraction: 0.7), value: configuration.isPressed)
    }
}
