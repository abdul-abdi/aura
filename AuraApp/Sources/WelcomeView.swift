import SwiftUI

private let auraGreen = Color(red: 0.30, green: 0.88, blue: 0.52)

// MARK: - WelcomeView

struct WelcomeView: View {
    let onContinue: () -> Void

    @State private var apiKey = ""
    @State private var showKey = false
    @State private var appeared = false
    @State private var orbPulse = false

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
            // Pre-populate from existing config
            if let existing = loadExistingKey() {
                apiKey = existing
            }
            // Staggered entry — trigger appeared flag with a slight delay so
            // the view has rendered at its initial (hidden) state first.
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) {
                withAnimation(.spring(response: 0.6, dampingFraction: 0.8)) {
                    appeared = true
                }
            }
            // Orb breathing starts independently
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
            // Glowing orb — layered circles
            ZStack {
                // Outermost diffuse glow
                Circle()
                    .fill(auraGreen.opacity(0.08))
                    .frame(width: 110, height: 110)
                    .scaleEffect(orbPulse ? 1.08 : 0.94)

                // Mid glow ring
                Circle()
                    .fill(auraGreen.opacity(0.14))
                    .frame(width: 82, height: 82)
                    .scaleEffect(orbPulse ? 1.05 : 0.97)

                // Inner soft glow
                Circle()
                    .fill(auraGreen.opacity(0.28))
                    .frame(width: 60, height: 60)
                    .scaleEffect(orbPulse ? 1.03 : 0.98)

                // Core orb
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

            // Title — staggered 0.2 s after orb
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

            // Subtitle — staggered 0.3 s
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
            // Key field + visibility toggle
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

                // Validation checkmark
                if isKeyValid {
                    Image(systemName: "checkmark.circle.fill")
                        .font(.system(size: 14, weight: .medium))
                        .foregroundStyle(auraGreen)
                        .transition(.scale.combined(with: .opacity))
                        .padding(.trailing, 6)
                }

                // Show/hide toggle
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
                RoundedRectangle(cornerRadius: 9, style: .continuous)
                    .fill(Color.primary.opacity(0.05))
                    .overlay(
                        RoundedRectangle(cornerRadius: 9, style: .continuous)
                            .strokeBorder(
                                isKeyValid
                                    ? auraGreen.opacity(0.45)
                                    : Color.primary.opacity(0.12),
                                lineWidth: 1
                            )
                    )
            )
            .animation(.spring(response: 0.3, dampingFraction: 0.7), value: isKeyValid)

            // "Get a free key" link
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
            .buttonStyle(WelcomeButtonStyle(enabled: isKeyValid))
            .disabled(!isKeyValid)
            .animation(.spring(response: 0.3, dampingFraction: 0.7), value: isKeyValid)
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
        // Parse: api_key = "value"
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
        let fm = FileManager.default
        let configDir = fm.homeDirectoryForCurrentUser
            .appendingPathComponent(".config/aura")

        // Create directory — ignore if already exists
        try? fm.createDirectory(
            at: configDir,
            withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700]
        )
        // Enforce directory permissions regardless
        try? fm.setAttributes(
            [.posixPermissions: 0o700],
            ofItemAtPath: configDir.path
        )

        // Write config file
        let configFile = configDir.appendingPathComponent("config.toml")
        let content = "api_key = \"\(apiKey)\"\n"
        try? content.write(to: configFile, atomically: true, encoding: .utf8)
        try? fm.setAttributes(
            [.posixPermissions: 0o600],
            ofItemAtPath: configFile.path
        )

        onContinue()
    }
}

// MARK: - Button Style

private struct WelcomeButtonStyle: ButtonStyle {
    let enabled: Bool

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .background(
                RoundedRectangle(cornerRadius: 9, style: .continuous)
                    .fill(
                        enabled
                            ? AnyShapeStyle(
                                LinearGradient(
                                    colors: [
                                        Color(red: 0.35, green: 0.92, blue: 0.56),
                                        Color(red: 0.25, green: 0.82, blue: 0.46),
                                    ],
                                    startPoint: .topLeading,
                                    endPoint: .bottomTrailing
                                )
                            )
                            : AnyShapeStyle(Color.primary.opacity(0.12))
                    )
            )
            .foregroundStyle(enabled ? Color.black.opacity(0.85) : Color.primary.opacity(0.35))
            .scaleEffect(configuration.isPressed ? 0.97 : 1.0)
            .shadow(
                color: enabled ? auraGreen.opacity(0.35) : .clear,
                radius: configuration.isPressed ? 4 : 8,
                x: 0,
                y: 3
            )
            .animation(.spring(response: 0.2, dampingFraction: 0.7), value: configuration.isPressed)
    }
}
